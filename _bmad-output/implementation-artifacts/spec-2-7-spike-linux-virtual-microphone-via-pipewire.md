---
title: 'Spike — Linux Virtual Microphone via PipeWire'
type: 'feature'
created: '2026-09-22'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
context: []
baseline_commit: 'a2a37a1041b5487bd31d828949c74ea90441bde3'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** `voice-me-audio-linux` is a four-line `todo!()` stub, and FR-6's whole Linux half — that a null-sink can serve as a real Virtual Microphone with no kernel driver and no elevated privileges — is an unverified bet. Story 2.9 wires playback on top of it, and Story 2.6 already produces an `AudioBuffer` that currently has nowhere to go but a debug wav.

**Approach:** Prove the mechanism end to end on this machine, then land it as real adapter code: create the virtual source device, write an `AudioBuffer` into it, and confirm an independent capture client selecting that device receives the audio. Record the decision (mechanism, library, fallback, cleanup) in `ARCHITECTURE-SPINE.md`, resolving the Linux half of FR-6.

## Boundaries & Constraints

**Always:** Per the spec-2-1/spec-2-5 precedent, the output is real module code in `voice-me-audio-linux` implementing `VirtualMicPort`, plus an example binary for manual verification — not throwaway code. Device creation, format handling and cleanup stay inside that crate (AD-2). The adapter takes the AD-11 `AudioBuffer` (24 kHz mono f32) as-is and lets the audio server resample — no hand-rolled resampler. No root, no kernel module, no `sudo`. `play` blocks until the buffer is consumed, since Story 2.9 drives it through the AD-5 bridge.

**Never:** No wiring into `voice-me-app` and no removal of the `VOICE_ME_DEBUG_WAV_DIR` seam — Story 2.9 owns both. No device-selection UI and no `AppState`/settings field for the device — nothing in Epic 2 chooses between devices. No Windows work (Story 2.8). No change to `TtsPort` or the Speak Action.

**Decisions taken (human, 2026-09-22):**

1. **The PulseAudio client API (`libpulse-binding`) is the mechanism, not libpipewire.** Verified on this machine: `paplay --device=<name>` into a node created with `media.class=Audio/Source/Virtual` reaches a capture client, while `pw-play --target=<id>` is silently ignored by WirePlumber's policy and plays out the speakers instead. Pulse gives module load/unload and playback-by-device-name through one library that is already on the system, needs no `bindgen`/`clang` build dependency, and keeps working on a classic-PulseAudio host (where the same module yields a sink plus its `.monitor` source rather than a virtual source). `pipewire` 0.10 (the app owning the node itself, so it vanishes with the process) is the recorded fallback if Pulse proves insufficient.

2. **The device is persistent and installed once.** A one-time setup writes `~/.config/pipewire/pipewire.conf.d/voice-me.conf`, so the audio server creates the Virtual Microphone at login whether or not voice-me is running — this is what UJ-2 promises ("confirms the app can see a virtual microphone in his OS's sound settings") and it means a game or Discord keeps the device selected between sessions. The cost is accepted deliberately: voice-me writes one file outside its own directories, and therefore owes an uninstall path. The file only takes effect when the audio server next starts, so the installer also loads the equivalent module for the *current* session; on a classic-PulseAudio host, where this config directory means nothing, that runtime load is the whole mechanism.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Happy path | Device installed, a 3 s `AudioBuffer` | An independent capture client on that device receives the same audio; `play` returns after it is drained | N/A |
| Install, twice | `voice-me.conf` already written and the module already loaded | Idempotent: still exactly one device, no duplicate module | N/A |
| Install, no config dir | `~/.config/pipewire/` absent | Created, file written, current session still gets the runtime load | Error naming the path if it cannot be written |
| Uninstall when not installed | No conf file, no module | Succeeds silently | N/A |
| No audio server | `PULSE_SERVER`/socket absent (CI, a headless box) | No device, no panic; every path returns a domain error naming the server as the problem | `VoiceMeError` at the port boundary |
| Empty buffer | `AudioBuffer::default()` | Nothing written, returns `Ok`; never an underrun or a hung drain | N/A |
| Classic PulseAudio host | No PipeWire, `pipewire.conf.d` meaningless | The runtime module load is the whole device; a `.monitor` source rather than a virtual source, and the spike records that as the observed difference | N/A (not verifiable on this machine) |

</frozen-after-approval>

## Code Map

- `crates/voice-me-audio-linux/src/lib.rs` -- `LinuxVirtualMicAdapter`: `install`/`uninstall`/`is_present`/`device_count`, and `VirtualMicPort::play`. Holds no connection between calls.
- `crates/voice-me-audio-linux/src/config.rs` (private to the crate) -- the persistent drop-in: its contents, its path, write/remove, and the shared `null_sink_arguments()` both the drop-in and the runtime load read.
- `crates/voice-me-audio-linux/src/pulse.rs` -- a blocking `PulseSession` over the PulseAudio client API: connect, count/find sources, load and unload the module.
- `crates/voice-me-audio-linux/src/bin/mic-spike.rs` -- manual verification, and what the end-to-end test drives.
- `crates/voice-me-audio-linux/tests/virtual_mic.rs` -- the four matrix rows that need a real audio server.
- `crates/voice-me-audio-linux/Cargo.toml` -- only depends on `voice-me-core` today. Add `libpulse-binding` 2.30 (and `libpulse-simple-binding` 2.29 if the simple API is enough — it is, for "write a whole buffer and drain"). Both link against the system `libpulse`, already present here.
- `crates/voice-me-core/src/audio.rs` -- `AudioBuffer`: `samples() -> &[f32]`, `SAMPLE_RATE = 24_000`, mono, `duration()`. Hand these samples to a stream opened as `f32le, 1ch, 24000Hz`; do not convert.
- `crates/voice-me-core/src/ports.rs` -- `VirtualMicPort::play(&self, audio: &AudioBuffer)`. The trait is already the right shape; do not change it.
- `crates/voice-me-core/src/error.rs` -- add a variant for "the virtual microphone could not be created/played" if no existing one fits; follow the file's convention of documenting why each variant exists.
- `crates/voice-me-tray-linux/examples/spike.rs`, `crates/voice-me-tts/examples/tts-spike.rs` -- the established shape for a manually-runnable spike binary.
- `_bmad-output/planning-artifacts/architecture/architecture-voice-me-2026-09-20/ARCHITECTURE-SPINE.md` -- the Deferred section names the Windows control surface (line ~257) but records nothing for Linux; AD-11 (line ~135) already says conversion happens here. Add the Linux resolution alongside the tray one (line ~255): mechanism, library, lifetime, fallback.

**Verified mechanism (do not re-derive).** A `module-null-sink` loaded with `media.class=Audio/Source/Virtual sink_name=voice-me channel_map=mono` is a node that `pactl list short sources` shows as a capture source (it is never listed as a Pulse *sink*, yet Pulse playback addressed to it by name reaches it). It has an `input_MONO` port (fed) and a `capture_MONO` port (read). The three ways this silently plays to the speakers instead — a `pipewire.conf.d` node, two devices sharing the name, and a device that is not there at all — are written up in `src/config.rs`'s and `src/lib.rs`'s module docs and in the architecture spine; read those before changing any of it.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-audio-linux/Cargo.toml` -- add the `libpulse` binding(s), target-gated so the Windows CI job still builds -- the crate has no way to talk to the audio server today
- [x] `crates/voice-me-audio-linux/src/lib.rs`, `src/config.rs`, `src/pulse.rs` -- install the persistent device (write `voice-me.conf`, plus a runtime module load so this session has it without a logout), an uninstall that removes both, an idempotent install that never yields two devices, and `play` as a blocking write-and-drain at 24 kHz mono f32 -- this is the spike's entire product (Decision 2)
- [x] `crates/voice-me-core/src/error.rs` -- a domain variant for virtual-mic failure, if none fits -- failures must reach core as `VoiceMeError`, never a panic
- [x] `crates/voice-me-audio-linux/src/bin/mic-spike.rs` (a bin, not an example — see Implementation Notes) -- stand the device up, play a generated tone, hold the device open long enough to capture from -- the only manual verification path, and how Story 2.9 will debug
- [x] `crates/voice-me-audio-linux` (tests, plus `tests/virtual_mic.rs`) -- what needs no audio server: the generated `voice-me.conf` contents, install/uninstall against a temp config dir, empty buffer, device-unavailable error mapping. Mark anything needing a live server `#[ignore]` and say so -- CI has no audio server
- [x] `_bmad-output/planning-artifacts/architecture/.../ARCHITECTURE-SPINE.md` -- record the Linux resolution and the fallback -- the spine currently says only "null-sink, to be proven"
- [x] `crates/voice-me-audio-windows/src/lib.rs` -- unchanged: `VirtualMicPort` did not move, so nothing forced a stub edit. `.github/workflows/ci.yml` gained `libpulse-dev` and the new deps are `cfg(target_os = "linux")`-gated, which is what keeps both CI jobs building

**Acceptance Criteria:**
- Given the example is run here, when it starts, then a voice-me-named device appears in `pactl list short sources` and in another application's microphone list, with no `sudo` and no kernel module
- Given that device is selected by an independent capture client, when the example plays a buffer, then the client receives non-silent audio — proven by capturing to a wav and measuring peak amplitude, not by assertion
- Given install has run, when the machine is rebooted (or the audio server restarted) with voice-me not running, then the device is still present and selectable; and given install runs twice, then there is still exactly one device
- Given uninstall runs, then both `voice-me.conf` and the live device are gone and nothing voice-me created remains
- Given `cargo check`, `cargo test` and `cargo clippy` over the workspace on a machine with no audio server, then all pass and no test requires one
- Given the spike is done, then `ARCHITECTURE-SPINE.md` names the mechanism, library, lifetime and fallback, and says plainly whether the approach is proven

## Implementation Notes

**The mechanism is the one the spec chose; the details it named were wrong,
and finding out how cost most of this spike.** Both frozen decisions stand —
the PulseAudio client API, and a device installed once that persists — but
`~/.config/pipewire/pipewire.conf.d/voice-me.conf`, named in Decision 2, does
not work. A `context.objects` drop-in there produces a node that looks
correct in every observable way (name, `media.class`, an `input_MONO` and a
`capture_MONO` port) and that no PulseAudio client can address: the name does
not resolve, and `pa_simple_new` answers an unresolvable device by
substituting the default sink rather than by failing. The generated line
comes out of the speakers. The working form is a **`pipewire-pulse`** drop-in
at `~/.config/pipewire/pipewire-pulse.conf.d/voice-me.conf` holding a
`pulse.cmd = [ { cmd = "load-module" … } ]` entry — same file count, same
one-time install, same uninstall, same directory tree, one directory over.
Flagging rather than editing the frozen text: the decision's substance is
intact, only the path it illustrated is.

**Every silence in this spike had the same cause, and it is now designed
against.** Duplicate nodes sharing the name `voice-me` — which my own
experiments kept creating — make PulseAudio resolve the name ambiguously and
fall back to the default sink. So `install` unloads *every* matching module
before loading one, `play` refuses unless exactly one device carries the
name, and the playback stream's application name is deliberately not
`voice-me`, because pipewire-pulse names a stream's node after its
application and would otherwise publish the duplicate itself. For this
product an unnoticed fallback to the speakers is not a degraded outcome; it
is the outcome the product exists to prevent.

**`mic-spike` is a `src/bin`, not an example.** A playback stream opened from
the cargo *test* binary is routed to the default sink even though its node
carries the correct `target.object = voice-me`, with exactly one device
present; the same library call from an ordinary binary is routed correctly
every time, across restarts. The difference could not be pinned down — the
stream properties are identical but for `application.process.binary`, and
neither duplicate devices, saved routing state, the recording client, nor the
application name explains it. Making it a bin lets `tests/virtual_mic.rs`
drive it through `CARGO_BIN_EXE_mic-spike`, so the acceptance criterion is
covered by a test that actually runs rather than by a shell transcript. It
also cost the wav-playing argument, which needed `hound` as a real
dependency; the tone is what proves the device.

**Verification performed (re-run after review triage).** `cargo check`, `cargo test`, `cargo clippy` and
`cargo fmt --check` are clean across the workspace, with the one pre-existing
`assert!(true)` warning in `voice-me-tests` untouched. The four real-device
tests pass here. Manually, from a clean audio state: install creates exactly
one `voice-me` source; an independent `pw-record` on it captured the played
tone at 0.37 and 0.76 of full scale; a full `pipewire`/`pipewire-pulse`/
`wireplumber` restart with voice-me not running left the device present and
still working; install twice leaves one device and one module; uninstall
removes the drop-in and the device, and uninstalling again is quiet. Not
done: listening to a real generated utterance (rather than a tone) through
the device, any classic-PulseAudio host, and an actual Windows build of the
workspace — the bin's target gating is reasoned and structurally checked,
not compiled for Windows, since no Windows toolchain exists here.

### Correction (after Story 2.9 shipped, 2026-09-22)

**The mechanism recorded above is wrong, and the verification that "proved"
it could not have caught the error.** A single `module-null-sink` published
as `media.class=Audio/Source/Virtual` is *not addressable by name from
another process*: playback streams resolve device names against sinks, a
virtual source is not one, and pipewire-pulse answers a name it cannot
resolve by substituting the **default sink**. Every generated line therefore
came out of the user's speakers, with the stream still carrying
`target.object = voice-me` — the exact failure this spike's own notes call
the worst this product has.

It looked verified because every check ran inside a process that had *just
loaded the module*, which is the one case that works. `mic-spike` installs
and then plays; the live tests drove `mic-spike`. The app installs once at
startup and plays later, so it failed on every utterance, and that is how it
was found — by the human running it. `paplay --device=voice-me` into such a
node captures silence, which settles it independently of any voice-me code.

The "unexplained routing asymmetry" between the test binary and the bin,
recorded above as unexplainable, was the same thing all along: neither had
loaded the module.

**What replaced it** (see `ARCHITECTURE-SPINE.md`): a plain
`module-null-sink` named `voice-me-sink`, which any process can address by
name, plus a `module-remap-source` republishing its monitor as `voice-me` —
still an ordinary microphone in the user's list, not a "Monitor of". Above
it, `play` now opens its stream with `PA_STREAM_DONT_MOVE` and then asks the
server which device the stream actually reached, refusing to play at all
unless it is the expected one. A new live row
(`audio_from_a_process_that_did_not_create_the_device_still_reaches_it`)
covers the case none of the original four did; with the old target it fails,
naming the speakers it would have played to.

## Spec Change Log

## Review Triage Log

Three layers (blind-hunter, edge-case-hunter, verification-gap) over the full
diff, 2026-09-22. Twenty-three findings; every one verified at its cited
location before a verdict.

**Patched**

- **`src/bin/mic-spike.rs` had no target gate, so the Windows CI job could
  not build** — `high`, found by all three layers and reproduced by one in a
  scratch crate. `lib.rs` is `#![cfg(target_os = "linux")]`, so on Windows
  the library compiles to nothing and the bin's `use voice_me_audio_linux::…`
  fails to resolve — and `build-windows` runs `cargo build --workspace`,
  which builds bins. The spec's own task line claimed the cfg-gated deps kept
  both jobs building; that was true of the dependencies and false of the new
  target. Every item in the bin is now gated, with a non-Linux `main` that
  says so.
- **The end-to-end test wrote the developer's real PipeWire drop-in and left
  it behind** — `medium`. `fixture()` promised a throwaway config directory,
  but the test spawns `mic-spike`, which builds its adapter from
  `LinuxVirtualMicAdapter::new()` — the real `~/.config`. Confirmed on this
  machine: after the first test run, `~/.config/pipewire/pipewire-pulse.conf.d/voice-me.conf`
  existed with *no* device, the worst of both. The child now runs with
  `XDG_CONFIG_HOME` pointed at the temp directory.
- **`install` wrote the drop-in before touching the audio server** —
  `medium`. A failed install on a headless box still arranged for the device
  at the next login, after telling the user it had failed. The live device is
  created first and the file written last.
- **`uninstall` failed on a machine with no audio server** — `medium`, and a
  direct contradiction of the matrix row that says it succeeds silently. No
  server means no live device to remove, which is the state uninstall is
  reaching for; it now returns `Ok` once the drop-in is gone.
- **`loaded_null_sinks` matched modules by substring**, so
  `sink_name=voice-me-other` — someone else's device — would have been
  unloaded by our install or uninstall, against that function's own doc.
  `medium`. Now a whole-token match.
- **The drop-in's directory, the spike's central discovery, was asserted by
  no test** — `medium` (verification-gap's primary finding). Every file test
  addressed the path through `conf_path` itself, so changing
  `pipewire-pulse.conf.d` back to `pipewire.conf.d` — the path the frozen
  spec text still names, which invites exactly that edit — kept
  `cargo test --workspace` green while silently making the device
  unplayable. Now pinned against the literal path.
- **CI ran no tests at all** — `medium`. Both jobs ended at
  `cargo build --workspace`, so none of this crate's server-free tests ever
  executed anywhere automatic. Pre-existing (already deferred from
  spec-2-4), but this story is the one that makes it load-bearing, and the
  fix is one line: the Ubuntu job now runs `cargo test --workspace`.
- **`source_exists` was a near-duplicate of `source_count` and its doc
  described code that does not exist** — `low`. It claimed to be "the
  idempotency check for install", which `install` never called. Deleted;
  `is_present` reads `source_count(..)? > 0`.
- **A silent capture failure was reported as a routing failure** — `low`. If
  `parecord` never attached, `peak` folded to `0.0` and the assertion blamed
  routing, sending the reader hunting in the wrong place. The test now
  asserts something was captured before interpreting its amplitude, and
  reaps the recorder before the `expect` that could panic past it.
- **The `voice-me-core` dev-dependency was redundant and its comment wrong**
  — `low`. Regular `[dependencies]` are available to integration tests;
  removed, and the tests still compile.
- **`pub mod config` exported the whole drop-in mechanism** — `low`. Nothing
  outside the crate used it, and AD-2 puts the mechanism inside the adapter.
  Now private.
- **A mistyped `--uninstall` silently installed and played a tone** — `low`.
  Unknown arguments now exit 2 by name.
- **Stray trailing whitespace in `deferred-work.md`** — `low`.

**Deferred**

- **Nothing bounds how long a PulseAudio operation can block** — `medium`,
  unverified. `connect` and `wait` loop on a blocking mainloop iteration and
  escape only on completion or a failed context; a server that stays `Ready`
  but never answers would hang `play` forever on the AD-5 blocking pool, with
  no error and no speech. Never observed; a deadline across both loops is
  real machinery.
- **`play`'s device check is a TOCTOU window** — `low`/`medium`, unverified.
  The device could be unloaded between `source_count` and `Simple::new`, and
  the stream would then land on the default sink. The simple API offers no
  way to ask an open stream which device it actually got.
- **A failed `load_null_sink` leaves the machine with no device at all**,
  because install unloads every existing module first — `medium`. Install
  does report the failure. Restoring the previous module means remembering
  and replaying its original arguments.
- **The `#[ignore]`d real-device tests still run nowhere automatic.** They
  need a live PipeWire session, which the story deliberately keeps out of CI.

**Rejected**

- **`debug_assert!(spec.is_valid())` is compiled out in release** — `false`.
  The sample spec is built from three constants; there is no runtime input
  that could make it invalid.
- **`sprint-status.yaml` says `in-progress` while the tasks are ticked** —
  `false`. That is the workflow's own in-flight state, written at step-03 and
  advanced at the end of the run.
- **`directories` is pinned separately in two crates with no workspace
  dependency table** — `low`, rejected. The spine already records the
  workspace's pinning inconsistency as a thing to normalise once; doing it
  here means a structural change across crates for no behaviour.
- **`install`/`uninstall` share an identical unload loop that should be one
  helper** — `low`, rejected. Three lines, two call sites, and the two
  functions read better with the loop in view than behind a name.
- **The test tears down a `voice-me` device the developer had installed** —
  `low`, rejected as a defect and documented instead. There is one audio
  server and one device name; no per-test isolation exists for that, and
  `mic-spike` puts it back. The fixture's doc comment now says so rather
  than claiming isolation it cannot deliver.

## Design Notes

**Why the buffer is not converted here.** AD-11 says conversion to what the driver wants happens in this adapter — but with a Pulse/PipeWire stream, "conversion" means declaring `pa_sample_spec { format: F32LE, rate: 24000, channels: 1 }` and letting the server resample to the device's 48 kHz. Writing a resampler here would be re-implementing what the audio server does on every stream.

## Verification

**Commands:**
- `cargo check --workspace`, `cargo test --workspace`, `cargo clippy --workspace` -- expected: clean, with no audio server needed
- `cargo run -p voice-me-audio-linux --bin mic-spike` -- expected: device created, buffer played, clean exit (`-- --uninstall` removes it)
- `cargo test -p voice-me-audio-linux --test virtual_mic -- --ignored --test-threads=1` -- on a desktop: the four real-device rows pass

**Manual checks:**
- `pactl list short sources | grep voice-me` while it runs -- expected: exactly one entry
- `pw-record --target=<id> /tmp/cap.wav` during playback, then measure the peak -- expected: clearly non-zero
- A real consumer (Sound settings' input meter, or a browser mic test) with the device selected -- expected: the meter moves
