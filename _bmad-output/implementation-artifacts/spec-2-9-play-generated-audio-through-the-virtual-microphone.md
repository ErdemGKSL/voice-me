---
title: 'Play Generated Audio Through the Virtual Microphone'
type: 'feature'
created: '2026-09-22'
status: 'draft'
route: 'dispatch'
review_loop_iteration: 0
context: ['_bmad-output/implementation-artifacts/spec-2-7-spike-linux-virtual-microphone-via-pipewire.md']
baseline_commit: 'PENDING — set to the commit that lands Story 2.7'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Story 2.6 generates the utterance and drops it: the Speak Action ends at a log line and an env-gated debug wav, so nothing the user types is ever heard by anyone. Story 2.7 built and proved the Linux adapter that can play it, but nothing constructs or calls that adapter — `voice-me-app` does not even depend on the crate.

**Approach:** Close the core loop. Hand the generated `AudioBuffer` straight from `TtsPort::generate` to `VirtualMicPort::play` inside the `speak` use-case, compose the per-OS virtual-mic adapter in the composition root, and delete the `VOICE_ME_DEBUG_WAV_DIR` seam that stood in for playback.

## Boundaries & Constraints

**Always:** The AD-11 `AudioBuffer` crosses `TtsPort::generate` → `VirtualMicPort::play` unconverted; conversion stays inside `voice-me-audio-*`. Playback is driven from the `speak` use-case, not the composition root — what the user is told when audio has nowhere to go is domain policy, the same reason `speak` already owns failure notification. Everything runs on the AD-5 blocking bridge, never the GPUI thread; the overlay's fire-and-forget dismissal is untouched. Failures reach core as `VoiceMeError` (AD-4) and reach the user through `NotificationPort` — never silence, never a panic. No code path here touches the real microphone.

**Never:** No change to 2.7's proven mechanism — device name, drop-in, exactly-one-device guard, presence check are reused as-is, not "improved". No Windows implementation (2.8). No device-selection UI or setting. No in-overlay error surface (3.4). No dependency-check or provisioning UI (Epic 3). No resampler or format conversion outside the audio adapter. No new `VirtualMicPort` method beyond playback.

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

</frozen-after-approval>

## Open Questions

- **Who installs the virtual microphone, and when?** Story 2.7 shipped `install()`/`uninstall()` but nothing calls them outside the spike binary, so on a fresh machine every Speak Action would end in the "not installed" notification. Options: **A — the app ensures it at startup** (one `install()` on launch when the device is absent; the loop works out of the box on first run, but the app writes a file outside its own directories without being asked, and Epic 3 later has to take that surface over rather than introduce it) / **B — Epic 3 owns it entirely** (2.9 ships the honest failure notification and the device is installed by running `mic-spike` until Epic 3 builds the dependency row; the story's own acceptance then depends on a manual step) / **C — install on demand from the failure path** (first Speak Action on an uninstalled machine installs, then plays; no startup cost, but a side effect fires from an error path and the first utterance is delayed).

- **What "on both Linux and Windows" means for this story.** The epic's acceptance criterion names both OSes, but Story 2.8 (the Windows control surface) is still `backlog` and blocked on machine access, and `WindowsVirtualMicAdapter::play` is a `todo!()` that would panic on the first Speak Action. Options: **A — ship the Linux half, make Windows honest** (wire Linux fully, turn the Windows `todo!()` into a `VirtualMicUnavailable` domain error so a Windows build notifies instead of dying, and record the Windows half as deferred to after 2.8; the story closes with a known, written-down gap) / **B — hold 2.9 until 2.8 lands** (the AC is met in full, but the core loop stays unclosed on Linux for as long as the Windows machine is unavailable) / **C — Linux half only, Windows untouched** (least work, but leaves a live panic on a path the app actually takes).

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
- [ ] `crates/voice-me-core/src/ports.rs` -- add `Send + Sync` to `VirtualMicPort` and state why in its doc -- it crosses the AD-5 bridge like the other two ports
- [ ] `crates/voice-me-core/src/speak.rs` -- take the mic port, play the generated buffer before returning, and give a playback failure its own notification summary rather than "couldn't generate speech" -- the error variant exists precisely so the user is told the truth about which half failed
- [ ] `crates/voice-me-core/src/speak.rs` (tests) -- a fake `VirtualMicPort` recording what it was handed, plus the matrix rows: the happy buffer reaches the mic unchanged, an unavailable mic notifies once with its own wording and does not report generation failure, and a generation failure never calls `play`
- [ ] `crates/voice-me-audio-windows/src/lib.rs` -- replace `todo!()` with a `VirtualMicUnavailable` domain error naming Story 2.8 -- resolution depends on Open Question 2
- [ ] `crates/voice-me-app/Cargo.toml`, `src/main.rs` -- depend on the per-OS audio crate, construct the adapter beside the notification port, pass it into `speak` from the `SpeakRequested` closure, and delete `DEBUG_WAV_DIR_VAR`/`report_generated`/`write_wav`/`hound` and the wav test -- the composition root is the only place allowed to wire this, and the seam's replacement has arrived
- [ ] `crates/voice-me-app/src/main.rs` (device availability at startup) -- scope set by Open Question 1
- [ ] `README.md` -- replace the debug-wav instructions with how the loop actually works now, including how the device gets installed -- the current text tells users nothing is played
- [ ] `crates/voice-me-audio-linux/tests/virtual_mic.rs` -- one `#[ignore]`d end-to-end row: a generated-shaped buffer played through the adapter is captured non-silent by an independent client -- the existing four rows prove the device, not the Speak path

**Acceptance Criteria:**
- Given the device is installed and a Reference Voice Sample is saved, when the hotkey is pressed and a line is typed and Enter is hit, then the overlay closes immediately and an application with `voice-me` selected as its microphone hears that line in the cloned voice, while the default speakers stay silent and the real microphone is unaffected
- Given the app is running, when nothing is spoken, then no audio and no device activity is produced, and the user's physical microphone is never opened by voice-me
- Given no debug environment variable is set anywhere, when audio is generated, then it is played rather than written to disk — `VOICE_ME_DEBUG_WAV_DIR` no longer exists in the workspace
- Given `cargo check --workspace`, `cargo test --workspace`, `cargo clippy --workspace` and `cargo fmt --check` on a machine with no audio server, then all pass and no new test requires one

## Implementation Notes

## Spec Change Log

## Review Triage Log

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
