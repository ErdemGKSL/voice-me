---
title: 'Play Generated Audio Through the Virtual Microphone'
type: 'feature'
created: '2026-09-22'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
context: ['_bmad-output/implementation-artifacts/spec-2-7-spike-linux-virtual-microphone-via-pipewire.md']
baseline_commit: '85f3d86a31c9bd799d6ac8c723713fd66c6022e8'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Story 2.6 generates the utterance and drops it: the Speak Action ends at a log line and an env-gated debug wav, so nothing the user types is ever heard by anyone. Story 2.7 built and proved the Linux adapter that can play it, but nothing constructs or calls that adapter — `voice-me-app` does not even depend on the crate.

**Approach:** Close the core loop. Hand the generated `AudioBuffer` straight from `TtsPort::generate` to `VirtualMicPort::play` inside the `speak` use-case, compose the per-OS virtual-mic adapter in the composition root, and delete the `VOICE_ME_DEBUG_WAV_DIR` seam that stood in for playback.

## Boundaries & Constraints

**Always:** The AD-11 `AudioBuffer` crosses `TtsPort::generate` → `VirtualMicPort::play` unconverted; conversion stays inside `voice-me-audio-*`. Playback is driven from the `speak` use-case, not the composition root — what the user is told when audio has nowhere to go is domain policy, the same reason `speak` already owns failure notification. Everything runs on the AD-5 blocking bridge, never the GPUI thread; the overlay's fire-and-forget dismissal is untouched. Failures reach core as `VoiceMeError` (AD-4) and reach the user through `NotificationPort` — never silence, never a panic. No code path here touches the real microphone.

**Never:** No change to 2.7's proven mechanism — device name, drop-in, exactly-one-device guard, presence check are reused as-is, not "improved". No Windows implementation (2.8). No device-selection UI or setting. No in-overlay error surface (3.4). No dependency-check or provisioning UI (Epic 3). No resampler or format conversion outside the audio adapter. No new `VirtualMicPort` method beyond playback.

**Decisions taken (human, 2026-09-22):**

1. **The app ensures the device at startup.** On launch, if the `voice-me` source is absent, the composition root calls 2.7's idempotent `install()` once, off the main thread, so the core loop works on a fresh machine with no manual step. A failed install degrades like every other adapter failure in `main` — logged, app still usable, the Speak Action's own notification is what tells the user — never a panic and never a blocked launch. Uninstall stays manual (`mic-spike --uninstall`); Epic 3 later presents both as a dependency row and drives 2.7's API rather than reimplementing it.
2. **This story ships the Linux half, and makes Windows honest rather than fatal.** `WindowsVirtualMicAdapter::play` becomes a `VirtualMicUnavailable` domain error naming Story 2.8, so a Windows build notifies instead of panicking on the first Speak Action. The epic's "on both Linux and Windows" criterion is therefore met on Linux only, and the Windows half is deferred to after 2.8 — a known, recorded gap, not an oversight.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Happy path | Device installed, hotkey → text → Enter | Overlay closes instantly; a capture client on `voice-me` receives the utterance, timed to the Speak Action; no wav written, no speaker output | N/A |
| Device not installed | No `voice-me` source | Nothing plays; one notification that names the missing virtual microphone, distinct from a generation failure | `VirtualMicUnavailable` from `play` |
| No audio server | Headless / no PulseAudio socket | Same: one notification naming the server as the problem, no panic | `VirtualMicUnavailable` |
| Duplicate devices | Two sources named `voice-me` | Refuses to play rather than risk the speakers; notification says to reinstall | `VirtualMicUnavailable` |
| Generation fails | `ort` error, missing asset, missing sample | Unchanged from 2.6: one "couldn't generate" notification, `play` never called | Existing `speak` paths |
| Second Speak mid-playback | One utterance still draining | Queued behind the first (the 2.6 session mutex already serialises); both are heard, in full | N/A |
| Empty buffer | Zero-sample `AudioBuffer` | Returns `Ok`, plays nothing, never hangs on drain | N/A |
| Startup, device absent | Fresh machine | Installed once in the background; launch is not blocked or delayed | N/A |
| Startup, install fails | No audio server, unwritable config dir | Logged, app still starts and stays usable; the first Speak Action's notification names the problem | No panic |
| Startup, device present | Already installed | Nothing loaded, nothing written, still exactly one device | N/A |
| Speak on Windows | Any text | One notification naming the unimplemented Windows virtual microphone | `VirtualMicUnavailable`, never `todo!()` |

</frozen-after-approval>

## Code Map

- `crates/voice-me-core/src/ports.rs:34` -- `VirtualMicPort::play(&self, &AudioBuffer) -> Result<(), VoiceMeError>`. Signature is right; it lacks `: Send + Sync`, needed to be held as `Arc<dyn …>` across `spawn_blocking` — `TtsPort` (`:70`) and `NotificationPort` (`:138`) carry it for that exact reason. That bound is the only port change.
- `crates/voice-me-core/src/speak.rs` -- `speak` (`:50`) wraps `speak_inner` (`:72`) so every failure notifies exactly once; notification consts at `:29`–`:36`. `speak_inner`'s tail is `tts.generate(...)` (`:115`) — bind it and play. The module doc (`:8`) and `speak`'s doc (`:47`) both say the story stops at generation; both go stale.
- `crates/voice-me-core/src/error.rs:81` -- `VirtualMicUnavailable(String)`, added by 2.7 with a doc anticipating this caller: it exists so "speech is fine, only its way out is missing" stays distinguishable. Reuse it; add nothing.
- `crates/voice-me-audio-linux/src/lib.rs` -- `new()` (`:72`) / `with_config_dir` (`:79`), `install` (`:97`), `uninstall` (`:116`), `is_present` (`:134`), `device_count` (`:141`), `impl VirtualMicPort` (`:146`). `play` already short-circuits an empty buffer, refuses unless exactly one device carries the name, and maps Pulse failures to `VirtualMicUnavailable`. Do not touch it; read its module doc on the three silent-routing traps first.
- `crates/voice-me-audio-windows/src/lib.rs:7` -- `play` is `todo!()`, on a path the Speak Action actually takes.
- `crates/voice-me-app/src/main.rs` -- composition root. Per-OS adapter pattern at `:42`–`:53` and `:433`–`:447`. `DEBUG_WAV_DIR_VAR` (`:103`), `report_generated` (`:162`), `write_wav` (`:190`), wav test (`:326`) are the seam to delete. `SpeakRequested` arm (`:642`) is the wiring point: clone the mic port into the `spawn_blocking` closure beside `notifications`.
- `crates/voice-me-app/Cargo.toml` -- depends on neither audio crate today; per-OS deps at `:24`–`:32`. `hound` (`:17`) and dev-dep `tempfile` (`:34`) exist only for the wav seam.
- `crates/voice-me-audio-linux/tests/virtual_mic.rs` -- four `#[ignore]`d live-device tests plus the `CARGO_BIN_EXE_mic-spike` harness; the model for anything needing a real server. CI has `libpulse-dev` but no running server, so nothing new may require one.
- `README.md:33` -- the "Nothing is played yet" paragraph and its `VOICE_ME_DEBUG_WAV_DIR` instructions, now false.
- No fake `VirtualMicPort` exists; `speak.rs`'s test module (`:118`, `FakeNotifier`/`FakeTts`) is where one belongs, and all nine `speak(...)` call sites there need the new argument.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-core/src/ports.rs` -- add `Send + Sync` to `VirtualMicPort` and state why in its doc -- it crosses the AD-5 bridge like the other two ports
- [x] `crates/voice-me-core/src/speak.rs` -- take the mic port, play the generated buffer before returning, and give a playback failure its own notification summary rather than "couldn't generate speech" -- the error variant exists precisely so the user is told the truth about which half failed
- [x] `crates/voice-me-core/src/speak.rs` (tests) -- a fake `VirtualMicPort` recording what it was handed, plus the matrix rows: the happy buffer reaches the mic unchanged, an unavailable mic notifies once with its own wording and does not report generation failure, and a generation failure never calls `play`
- [x] `crates/voice-me-audio-windows/src/lib.rs` -- replace `todo!()` with a `VirtualMicUnavailable` domain error naming Story 2.8 (Decision 2) -- this stub sits on a path the Speak Action actually takes, unlike the tray and hotkey stubs
- [x] `crates/voice-me-app/Cargo.toml`, `src/main.rs` -- depend on the per-OS audio crate, construct the adapter beside the notification port, pass it into `speak` from the `SpeakRequested` closure, and delete `DEBUG_WAV_DIR_VAR`/`report_generated`/`write_wav`/`hound` and the wav test -- the composition root is the only place allowed to wire this, and the seam's replacement has arrived
- [x] `crates/voice-me-app/src/main.rs` -- ensure the device at startup: when absent, one background `install()`, failures logged and non-fatal (Decision 1) -- without it a fresh machine can never be heard, and a blocking Pulse call on the GPUI thread would stall launch
- [x] `README.md` -- replace the debug-wav instructions with how the loop actually works now, including how the device gets installed -- the current text tells users nothing is played
- [x] `crates/voice-me-audio-linux/tests/virtual_mic.rs` -- one `#[ignore]`d end-to-end row: a generated-shaped buffer played through the adapter is captured non-silent by an independent client -- the existing four rows prove the device, not the Speak path

**Acceptance Criteria:**
- Given the device is installed and a Reference Voice Sample is saved, when the hotkey is pressed and a line is typed and Enter is hit, then the overlay closes immediately and an application with `voice-me` selected as its microphone hears that line in the cloned voice, while the default speakers stay silent and the real microphone is unaffected
- Given the app is running, when nothing is spoken, then no audio and no device activity is produced, and the user's physical microphone is never opened by voice-me
- Given no debug environment variable is set anywhere, when audio is generated, then it is played rather than written to disk — `VOICE_ME_DEBUG_WAV_DIR` no longer exists in the workspace
- Given a machine where the device was never installed, when the app is launched and a line is spoken, then it is heard — no manual setup step — and given the install cannot be performed, then the app still starts and the user is told why nothing was heard
- Given a Windows build, when a line is spoken, then a notification names the missing Windows virtual microphone and the process does not panic
- Given `cargo check --workspace`, `cargo test --workspace`, `cargo clippy --workspace` and `cargo fmt --check` on a machine with no audio server, then all pass and no new test requires one

## Implementation Notes

**A stand-in port rather than an `Option`.** `LinuxVirtualMicAdapter::new()`
can fail (an unresolvable config directory), which would have left the
composition root holding `Option<Arc<dyn VirtualMicPort>>` and re-deciding,
in the event loop, what the user is told — the exact duplication the story's
Design Notes argue against for playback itself. Instead `main.rs` falls back
to `UnavailableVirtualMic(reason)`, a two-line port whose `play` returns
`VirtualMicUnavailable` carrying the original reason. The failure then travels
the one notification path that is already tested, and the `SpeakRequested` arm
gained no second branch.

**Playback is serialised here, not inherited from the TTS mutex.** The
matrix's "queued behind the first, both heard in full" row does not fall out
of spec-2-6's session mutex: that lock is released when `generate` returns,
before `play` is called, so two Speak Actions overlap in the device whenever
generation is faster than the previous utterance takes to drain — the normal
case on a `cuda` build. A module-level `Mutex` in `speak.rs`, held across the
`play` call, is what makes the second wait; no port method and no public
surface. `FakeMic` now carries the same in-flight/overlapped pair
`ExclusiveTts` uses, and its playback is deliberately slower than the stub
generation, so removing the lock fails the concurrency test.

**Playback failure gets its own title, not its own path.** `speak` still
notifies exactly once from the one `Err` arm; it now picks
`PLAYBACK_FAILED_SUMMARY` over `GENERATION_FAILED_SUMMARY` when the error is
`VirtualMicUnavailable`. That keeps the exactly-once promise structurally
(one call site) while telling the user which half failed, which is what the
variant exists for.

**The startup install runs on GPUI's background executor, not the Tokio
bridge.** `ensure_virtual_microphone` is dispatched with `cx.background_spawn`
because the Tokio runtime is itself allowed to fail at startup, and the device
has to be ensured on that run too. It checks `is_present()` first so an
already-installed machine loads no module and writes no file, and every
failure — including "no audio server to ask" — is logged and swallowed.

**The new live row drives the whole Speak Action, through `mic-spike`.**
spec-2-7 established that a playback stream opened from the cargo *test*
binary is routed to the default sink, so the new end-to-end row cannot call
`speak` in-process either. `mic-spike` gained `--speak`, which runs
`voice_me_core::speak` over a stub `TtsPort` returning an utterance-shaped
buffer (an enveloped harmonic stack, not a full-scale tone) and a stderr
notifier. The test spawns it exactly as the existing row does; the shared
capture/peak helpers are now factored out of both.

**Verification performed.** `cargo check --workspace`, `cargo test
--workspace`, `cargo clippy --workspace --all-targets` and `cargo fmt
--check` are clean, with only the pre-existing `assert!(true)` warning in
`voice-me-tests`. All five live-device rows pass here, the new Speak-path one
included — `parecord` on `voice-me` heard the utterance the Speak Action
produced. The tests leave the live device unloaded (they always did), so
`mic-spike` was run afterwards to put it back; `pactl list short sources`
shows exactly one `voice-me` entry. Not done: a real end-to-end run of
`cargo run -p voice-me-app` with a provisioned model cache, and anything on
Windows.

### Verification pass (build workflow, step-03)

The matrix audit found four rows with no *running* test — the three startup
rows and the Windows row — all four of them rows this story's own decisions
added. Covered rather than deferred:

- `ensure_virtual_microphone` was split into `ensure_device(&dyn
  VirtualMicInstaller) -> EnsureOutcome` plus a thin logging wrapper. The
  trait is private to `main.rs`, so `voice-me-audio-linux` gains no public
  surface for one call site; what becomes testable is the part that is
  actually a decision — check first, install only if absent, and never
  attempt an install after a failed check, because no audio server to ask is
  also no audio server to install into. Three tests, no audio server needed.
- `voice-me-audio-windows` gained a test asserting `play` returns
  `VirtualMicUnavailable` naming Story 2.8. The crate is not target-gated, so
  it runs on Linux — the one Windows behaviour this story can actually prove
  here. The assertion is on the variant, because that is what `speak` matches
  on to pick the playback title over the generation one.

The `deferred-work.md` entry claiming the startup install is untestable
without a GPUI harness was therefore wrong and is left in place only as a
record; the sequencing it names is now covered.

**Rows still covered only by `#[ignore]`d tests:** the happy path's
"a capture client receives it" half, no-audio-server, and duplicate-devices.
That is by design and by this spec's own acceptance criterion — CI has
`libpulse-dev` but no running audio server. Each was run here: all five live
rows pass, including the new Speak-path one, and the run was repeated
independently by this verification pass. The duplicate-device *refusal*
branch (`lib.rs:172`) has no test at either level; it is 2.7's code, which
this spec forbids touching, and it was observed working only indirectly.

**Verified here:** `cargo check`, `cargo test --workspace` (161 passing, 5
ignored), `cargo clippy --workspace --all-targets` and `cargo fmt --check`
are clean but for the pre-existing `assert!(true)` in `voice-me-tests`.
`a_speak_action_reaches_a_capture_client` passes against the real audio
server. Running the live rows uninstalls the device, so `mic-spike` was run
afterwards; `pactl list short sources` shows exactly one `voice-me` entry and
the drop-in is back at
`~/.config/pipewire/pipewire-pulse.conf.d/voice-me.conf`. Incidentally
confirmed: with the device absent, `mic-spike --play-only` refused with
`VirtualMicUnavailable` rather than playing to the speakers — the failure
mode this story exists to prevent, observed doing the right thing.

**Not verified:** a full `cargo run -p voice-me-app` with the provisioned
model cache (hotkey → type → Enter → heard), and anything on Windows.

**Final state after the review loop:** `cargo test --workspace` 162 passing,
5 ignored (the live rows); `cargo clippy --workspace --all-targets` and
`cargo fmt --check` clean but for the pre-existing `assert!(true)` in
`voice-me-tests`. All five live rows pass twice in a row from a machine with
no device installed. The playback mutex was checked by removing it: the
concurrency test fails with "two streams into one device mix rather than
queue" and passes with it restored. The machine is left with exactly one
`voice-me` source and the drop-in in place.

## Spec Change Log

## Review Triage Log

Three layers (blind-hunter, edge-case-hunter, verification-gap) over the full
diff, 2026-09-22. Twenty findings; each verified at its cited location before
a verdict.

**Patched**

- **Nothing serialises playback** — `medium`, found independently by two
  layers. The 2.6 session mutex lives inside `voice-me-tts::generate` and is
  released before `speak` calls `play`, so the frozen matrix row's
  parenthetical ("the 2.6 session mutex already serialises") is factually
  wrong. Two Speak Actions can hold playback streams at once whenever
  generation outruns the previous utterance's drain — unreachable on this
  CPU build at a 0.10× real-time factor, reachable on the `cuda` variant
  AD-7 ships. The row's promised behaviour ("both heard in full") is now
  enforced at the `speak` level rather than inherited by accident.
- **The concurrency test could not detect that overlap** — `medium`.
  `assert_eq!(mic.played().len(), 2)` holds whether the two playbacks were
  sequential or simultaneous; the fake mic now carries the in-flight/
  overlapped pair `ExclusiveTts` already used for generation.
- **`FakeMic` recorded before it failed, and healed itself after one call** —
  `low`. `play` pushed the buffer before consulting `fail_with`, and
  `take()` made every later call succeed, so "an unavailable mic is handed
  the buffer at most once" was untestable.
- **The test-helper extraction regressed device cleanup** — `medium`.
  `assert!(played.status.success())` moved inside
  `capture_while_mic_spike_runs`, ahead of both callers' `uninstall()`; the
  original ordering uninstalled first. A failing `mic-spike` — the case being
  debugged — panicked and left a null-sink module loaded on the developer's
  machine.
- **The startup ensure built the adapter twice** — `low`. One unresolvable
  config directory produced two different log lines from two constructions,
  which can in principle disagree.
- **The failing-`install()` arm of `ensure_device` had no test** — `medium`
  (verification-gap's primary finding). `FakeInstaller::install` was
  hardcoded to `Ok`, so the non-fatal-install promise — the one thing between
  a read-only `~/.config` and a panic in a detached startup task — rested on
  nothing.
- **`FakeInstaller::is_present` rewrapped its own error** — `low`. It proved
  the wrong error shape reached the log.
- **A successful Speak Action logged nothing** — `low`. Deleting
  `report_generated` took the duration line with it and left `speak`'s
  documented return value unused, making a silent-but-working run
  indistinguishable from a silent-and-broken one.
- **`peak_of`'s doc described a function that was not written** — `low`. It
  promised a returned sample count that is an internal assertion.
- **The new `deferred-work.md` entry contradicted this spec in the same
  commit** — `medium`, found by all three layers. It claimed the startup
  install was untestable without a GPUI harness; the same diff made it
  testable. A retracted item left standing in a backlog others act from gets
  picked up as real work.
- **The second capture test in a run could never pass** — `medium`, found by
  this verification pass rather than by a review layer, and caused by this
  story. `parecord` is started before the device exists and each capture
  test uninstalls on its way out, so the first one to run attached only
  because the developer's machine happened to have a device installed, and
  the second attached to nothing and reported silence — which reads exactly
  like a routing failure. With one capture test the sequence never arose;
  adding the Speak-path row exposed it. `mic-spike` gained `--install-only`
  and the helper stands the device up with it before attaching the recorder.
  The playing child still installs for itself — `--play-only` was tried and
  routes to the default sink, confirming that only the process which loaded
  the module can address it by name, and that pipewire-pulse moves an
  attached recorder onto the replacement node. Verified from a clean state:
  two consecutive full runs of the five live rows pass, where before the
  fix the run failed unless a device was already present.


**Deferred**

- **A Windows build panics at launch, before any Speak Action** — `high` if
  Windows were in scope, but pre-existing and not caused by this story.
  `WindowsTrayAdapter::show` is still `todo!()` (spec-2-1 deferred it for
  want of a toolchain) and `main.rs` calls it unconditionally, so Decision
  2's "notifies instead of panicking" is true of the virtual-mic path and
  moot in practice — the process never reaches a Speak Action. Worth knowing
  before 2.8 is judged complete.

**Rejected**

- **`--speak` reinstalls the device under the attached capture client** —
  `false`. The test comment says the child installs deliberately: a device
  created by the *test* process is one the child cannot address by name.
  Both live rows pass, twice, including an independent run in this
  verification pass.
- **The startup ensure races a Speak Action on a fresh machine** — `low`,
  rejected. Install is a module load and a file write; the first utterance
  on a fresh machine is behind an 86–110 s session build and a Reference
  Voice Sample that Epic 1 must already have recorded. The fix — a readiness
  latch the `SpeakRequested` arm awaits — is real machinery for a window
  nobody can hit.
- **A failed startup install is invisible until the first Speak Action** —
  `false`. That is Decision 1 as written: the Speak Action's own
  notification is what tells the user, and the AC says so.
- **`install()` could return `Ok` with no device present** — `false`.
  `install` unloads every match, then loads, and reports a failed load as an
  error; 2.7's live rows confirm a successful install yields exactly one
  device.
- **Status metadata is inconsistent across tracking files** — `false`. The
  spec is `in-review` and `sprint-status.yaml` moves to `review` at
  presentation; the triage log is this section, written now.
- **README gaps (`--speak` undocumented, `XDG_CONFIG_HOME` unmentioned,
  assumes a source checkout)** — `low`, rejected. Nothing it says is untrue
  for the path a user actually takes; `--speak` is a developer harness and
  `XDG_CONFIG_HOME` is how the tests avoid the real config, not something a
  user sets.

## Design Notes

**Why `play` belongs inside `speak`.** `speak` exists to make one promise: every failure of the Speak Action tells the user exactly once, in words. Playback failure is a failure of the Speak Action — a line nobody heard — so calling `play` from the composition root would mean re-implementing that promise where it is only testable through a running GPUI app. Cost: a fifth parameter and nine touched call sites in `speak`'s own tests. Alternative: a second, untested notification path.

**Silence is the failure mode this story must not have.** Per 2.7, a buffer aimed at a device that is missing, duplicated or wrongly addressed does not error — it comes out of the speakers. The guards against that are in the adapter and already proven. This story adds the layer above: `VirtualMicUnavailable` must reach the user as its own notification, because a missing device and a failed generation have different fixes.

## Verification

**Commands:**
- `cargo check --workspace`, `cargo test --workspace`, `cargo clippy --workspace`, `cargo fmt --check` -- expected: clean, no audio server needed
- `cargo test -p voice-me-audio-linux --test virtual_mic -- --ignored --test-threads=1` -- on this desktop: all live-device rows pass, including the new Speak-path row
- `cargo run -p voice-me-app` with the model cache provisioned -- expected: hotkey → type → Enter → the utterance arrives on the `voice-me` device

**Manual checks:**
- `pactl list short sources | grep voice-me` -- exactly one entry
- A real consumer (Sound settings' input meter, a browser mic test, a voice-chat app) with `voice-me` selected: the meter moves on the typed line and the speakers stay silent
- Uninstall the device, then speak: a notification names the missing virtual microphone, and nothing is heard on the speakers
