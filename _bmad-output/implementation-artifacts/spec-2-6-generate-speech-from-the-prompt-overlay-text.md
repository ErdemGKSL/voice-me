---
title: 'Generate Speech from the Prompt Overlay Text'
type: 'feature'
created: '2026-09-22'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
context: ['_bmad-output/implementation-artifacts/spec-2-5-spike-in-process-chatterbox-inference-on-onnx-runtime.md']
baseline_commit: 'b163a54336a875103d3366b4ab376e961bb75077'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** `AppEvent::SpeakRequested` currently ends at `println!` in `voice-me-app`, and `TtsAdapter::generate` is a `todo!()`. Story 2.5 proved the engine runs and left the three things it deliberately did not decide — session lifecycle, queueing, and who says "it failed" — to this story.

**Approach:** Make the Speak Action produce audio: hold the four ONNX sessions for the process lifetime behind `TtsPort`, serialize generations, dispatch each one off the GPUI thread through the AD-5 bridge, and give failure and unusual slowness an OS-native notification behind a new core port.

## Boundaries & Constraints

**Always:** Session lifecycle, warm-up and queueing live inside `voice-me-tts` (AD-10) — `voice-me-core` and `voice-me-ui` never see an `ort` type. Generation runs on `tokio_bridge::spawn_blocking`, never on the GPUI main thread, and the overlay's existing fire-and-forget dismissal is untouched. Exactly one generation runs at a time; a Speak Action arriving during one is queued. `voice-me-tts` reads the execution target and weight variant from `AppState` only (AD-9) and never calls `voice-me-deps`. Failures reach core as `VoiceMeError` (AD-4). No network (AD-8).

**Never:** No `VirtualMicPort::play` call and no playback — Story 2.9 owns that, and its adapter is still `todo!()`. No dependency detection, no provisioning, no Dependency Check wiring (Epic 3); `AppState`'s backend value is resolved from this build's compile-time variant alone for now. No overlay UI change, no in-overlay progress or error surface (Story 3.4 owns the overlay's inline notice). No Settings → Voice speech-language selector (Decision 2). No Windows verification beyond compiling.

**Decisions taken (human, 2026-09-22):**

1. **Q4 is the shipped CPU language-model default** — it is already provisioned and measured, and 354 MB against FP32's 2.08 GB. This reverses the FP32 direction recorded in the architecture spine (commit `b163a54`) on the size/speed trade-off rather than on the listening verdict, and the spine is amended in this story to say so. `q4f16` is not tested here.
2. **Speech language is a persisted setting, not yet a UI control.** `AppState` + `settings.toml` gain `speech_language` (default `"tr"`), satisfying FR5's "selected speech language"; the Settings → Voice selector UX-DR13 describes is deferred to Epic 4, where the UI-language selector is built.
3. **Warm-up is eager at startup, but only when a Reference Voice Sample already exists.** A first-run user who has not set one up pays nothing; everyone else has the 86–110 s build behind them before their first Speak Action.
4. **The "still working" notification fires only while a Speak Action is waiting on session construction** — the genuinely unusual wait. A normal ~20 s generation notifies nothing. This closes PRD Open Question 1 for this surface without inventing a latency target.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| First Speak Action | Sessions not yet built, sample present | Sessions built once, then audio generated; later actions reuse them | N/A |
| Second Speak mid-generation | One generation in flight | Queued and run after it, never concurrently; both produce audio | N/A |
| No Reference Voice Sample | `AppState.reference_voice_sample` is `None` | No generation attempted; notification naming the missing sample | Domain error before any session work |
| Missing model file / ONNX Runtime | Cache dir incomplete | Notification naming the exact missing path | `MissingRuntimeAsset`, never a panic |
| Engine failure mid-generation | `ort` returns an error (incl. a lost GPU device) | Notification "Couldn't generate speech." plus the short reason; nothing is played | `SpeechEngine`; corrupted audio never surfaces as success |
| Speak Action waiting on session build | Sessions not yet ready (cold start, or warm-up still running) | One brief "still working" notification, at most once per action; the audio still arrives afterwards | N/A |
| Empty text | Whitespace-only line | Never reaches here — the overlay drops it; `generate` still rejects it | `EmptyText` |

</frozen-after-approval>

## Code Map

- `crates/voice-me-tts/src/lib.rs` -- `TtsAdapter` is a unit struct whose `generate` is `todo!()`. Give it the session slot (`Mutex<Option<Sessions>>` — the mutex *is* AD-10's queue) plus a `warm_up`. Reuse `sessions::Sessions::build(cache, variant, target, verbose)`, `reference::load_reference_clip`, `generate::generate(&mut Sessions, text, language, &AudioBuffer, GenerationSettings)` unchanged — do not re-derive the loop.
- `crates/voice-me-tts/src/sessions.rs` -- `LanguageModel {Q4,Fp16,Fp32}`, `ExecutionTarget {Cpu, WebGpu{device_id}}`, `ModelCache::from_env()`. These stay `voice-me-tts`-private vocabulary; map to them from the new core type.
- `crates/voice-me-core/src/state.rs` -- add the AD-9 resolved backend (execution target + optional device + weight variant) to `AppState`. Core-side type, since core cannot depend on `voice-me-tts`.
- `crates/voice-me-core/src/ports.rs` -- `TtsPort` gains warm-up; add the new notification port next to `TrayPort` (same driven-adapter shape).
- `crates/voice-me-core/src/error.rs` -- add the "no active Reference Voice Sample" variant; follow the existing style (each variant documents *why* it is its own variant).
- `crates/voice-me-core/src/settings_store.rs` -- `SettingsFile` + `build_state`: where a persisted speech language and device selection would land; `save_*` methods return a fresh `AppState`.
- `crates/voice-me-core/src/tokio_bridge.rs` -- `TokioRuntime::install(cx)` / `spawn_blocking(cx, work)`. **`install` is never called in `main.rs` today** — only in `examples/tts-spike.rs:125`.
- `crates/voice-me-app/src/main.rs:~315` -- the single `AppEvent` receiver; `AppEvent::SpeakRequested { text } => println!(...)` is the exact hook to replace. Adapters are composed above it; `Cargo.toml` has no `voice-me-tts` edge yet.
- `crates/voice-me-tray-linux/src/lib.rs` -- the newtype-`Global` pattern (`TrayHandle`, `EventSender`) for holding long-lived adapter state.
- `crates/voice-me-ui/src/prompt_overlay.rs` -- `speak()` sends `AppEvent::SpeakRequested` and dismisses. **Do not change it**; its tests pin the trim/empty/once-only contract.
- `crates/voice-me-tts/examples/tts-spike.rs` -- the working end-to-end driver, including `ORT_DYLIB_PATH`/cache setup; the README documents fetching the assets.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-core/src/state.rs`, `src/error.rs`, `src/lib.rs` -- the AD-9 backend value on `AppState` and the missing-sample error variant -- the vocabulary everything below reads
- [x] `crates/voice-me-core/src/settings_store.rs` -- persist and load `speech_language` (default `"tr"`, Decision 2) -- FR5 requires a *selected* language, not a constant
- [x] `crates/voice-me-core/src/ports.rs` -- `TtsPort::warm_up` and the notification port -- AD-10's explicit warm-up and UX-DR14/15's only surface
- [x] `crates/voice-me-notify*` (new adapter crate(s)) -- OS-native notification delivery, Linux implemented and Windows compiled-only -- there is no notification mechanism in the workspace today
- [x] `crates/voice-me-tts/src/lib.rs` -- session slot, warm-up, serialized `generate`, `AppState`→`ExecutionTarget`/`LanguageModel` mapping -- AD-10's whole contract, in the one crate allowed to know `ort`
- [x] `crates/voice-me-app/src/main.rs`, `Cargo.toml` -- install the Tokio runtime, compose the TTS and notification adapters, and replace the `SpeakRequested` arm with dispatch + slow/failure notifications -- the composition root is the only place allowed to wire this
- [x] `_bmad-output/planning-artifacts/architecture/.../ARCHITECTURE-SPINE.md` -- amend the CPU weight default to Q4 with the size/speed reasoning (Decision 1) -- the spine currently says FP32 and would otherwise contradict the code
- [x] tests -- cover the I/O matrix rows: queueing (two actions, one at a time), missing sample, missing asset path in the message, engine failure, and the slow-notification firing once -- against fake ports, with no model files and no network

**Acceptance Criteria:**
- Given a provisioned cache and a saved Reference Voice Sample, when the app starts, then warm-up runs eagerly in the background (Decision 3) and a first run with no saved sample builds nothing
- Given a provisioned cache, a saved Reference Voice Sample and the app running, when the hotkey is pressed, a line is typed and Enter is hit, then the overlay closes instantly and audio is generated in the selected speech language with no Python process and no UI freeze
- Given a generation is in flight, when a second Speak Action arrives, then it runs only after the first finishes, on the same sessions, with no second session build
- Given generation fails for any reason, then exactly one OS-native notification names the failure and no audio is handed onward; given a Speak Action lands while the sessions are still being built, then one "still working" notification appears and the audio still arrives afterwards
- Given `cargo check --workspace`, `cargo test --workspace` and `cargo clippy --workspace`, then all pass with no model files, no ONNX Runtime library and no network access

## Implementation Notes

**This spec was regenerated after the fact.** The implementation already
existed, uncommitted, when this planning pass ran; the original spec file was
overwritten before that was noticed. The frozen block above was re-derived
from the same four decisions the code cites by number (`spec-2-6 Decision
1..4` in `state.rs`, `settings_store.rs`, `ports.rs`, `main.rs`) and the human
re-confirmed all four, so intent and code agree — but the Code Map and task
list here describe work that was already done, not work planned ahead of it.

**Shape of what landed.** `SessionSlot<T>` in `voice-me-tts` is AD-10 in one
type: a `Mutex<Option<T>>` whose lock is held across the whole generation, so
"built once, held, one at a time" needs no channel or worker thread. It is
generic purely so the contract is testable without 1.56 GB of weights. An
`AtomicBool` mirrors readiness beside the mutex, because answering `is_ready`
by trying to lock would report "not ready" throughout a perfectly ordinary
utterance and fire the still-loading notification for nothing. A failed build
leaves the slot empty so the next Speak Action retries — provisioning the
missing file is exactly the fix, and a remembered error would outlive it.
Mutex poisoning is recovered rather than propagated: one panicking generation
must not wedge every later one.

`voice-me-core::speak` is the first use-case function in the hexagon. It lives
there rather than in `main.rs` because deciding *what the user is told when
generation fails* is domain policy, and in the composition root it would only
be testable through a running GPUI app.

### Verification pass (build workflow, step-03)

Both findings were in `voice-me-app`, and both are fixed:

1. **A `.expect()` on `TtsAdapter::from_state` killed the app at launch** when
   the model-cache directory could not be resolved. Every other adapter
   failure in `main` degrades — a tray, hotkey or Tokio failure all just log
   and leave the app usable — so this one contradicted the function's own
   rule, and it failed hardest for a user who has nothing provisioned yet.
   The port is now `Option<Arc<dyn TtsPort>>`; a Speak Action with no engine
   notifies instead, honouring UX-DR15 rather than dying.
2. **One clippy warning** (`let_unit_value` on the `SpeakRequested` arm's
   `cx.update`), against the spec's own "no new warnings" bar.

Matrix audit: all seven rows have a covering test that runs. The
"missing ONNX Runtime" half of row 4 is covered by the dylib error path
spec-2-5 already exercised, not by a new test here; the missing-model-file
half has two.

`cargo check`, `cargo test` (134 passing) and `cargo clippy` are all clean
across the workspace. The end-to-end run — hotkey, type, Enter, listen to the
wav — has **not** been performed in this session and remains the one
unverified acceptance criterion.

## Spec Change Log

## Review Triage Log

Three layers (blind-hunter, edge-case-hunter, verification-gap) over the full
diff, 2026-09-22. Twenty findings; every one verified at its cited location
before a verdict.

**Patched**

- **A failed Tokio-runtime install was documented as graceful degradation but
  guaranteed a panic** — `high`. `main.rs` logged and continued, while
  `tokio_bridge::spawn_blocking` resolves the runtime through
  `cx.global::<TokioRuntime>()`, which panics when the global was never set
  (`tokio_bridge.rs:89`). Both the eager warm-up and every Speak Action would
  have aborted the process. A failed install now disables the engine the same
  way an unresolvable cache directory does. All three layers found it.
- **`WindowsNotificationAdapter::notify` was `todo!()` on a live failure
  path** — `medium`. Unlike the tray and hotkey Windows stubs, this one is
  *called*: `speak` notifies on every failure and every cold start, so the
  first failed generation on Windows would panic instead of reporting
  anything. Now returns a domain error, which is what the port's contract
  already expects of an undeliverable notification.
- **The engine-unavailable notification threw away its reason** — `medium`,
  and introduced by this build's own verification pass. The real error went to
  stderr at startup and the user got "could not be set up on this machine",
  the unactionable shape the rest of this story avoids. The reason is now
  carried to the notification.
- **That notification also ran a synchronous D-Bus call on GPUI's main
  thread** — `low`, and it falsified `NotificationPort`'s own doc claim that
  every notification originates off the main thread. Moved to
  `cx.background_spawn` — not the Tokio bridge, since a dead runtime is one of
  the reasons that branch is reached.
- **`speech_language` was unvalidated** — `medium`. It flows straight into
  `format!("[{}]{}")` in the tokenizer, and Decision 2 makes hand-editing
  `settings.toml` the only way to set it, so a typo is the *expected* failure
  mode — `speech_language = "turkish"` would have produced plausible audio in
  the wrong language with nothing to diagnose. Now refused by name against the
  two languages AD-12 allows, with trimming and case-folding so a hand-edited
  file still works.
- **`VOICE_ME_DEBUG_WAV_DIR` silently dropped output when the directory did
  not exist** — `medium`, including for the README's own `/tmp/voice-me`
  example on a fresh boot. `create_dir_all` added, plus a round-trip test that
  pins rate, channel count and length.
- **The Speak Action wiring in the composition root had no test at all** —
  `medium` (verification-gap's primary finding, filed as `patch`). `main.rs`'s
  only tests covered the overlay window kind. Four tests added: settings are
  re-read per action (the decision that makes a sample recorded after launch
  count), the resolved backend rides along, and an unavailable engine reaches
  `notify` with its reason rather than dropping the line.
- **`TtsPort::warm_up`'s documented race guarantee had no test** — `low`.
  Concurrent `ensure` + `with` now asserts one build, not two.
- **A test comment claimed serialization is tested "against the real
  `TtsAdapter`"** — `low`. It is tested against `SessionSlot`; the comment now
  says so and why.
- **`save_hotkey_leaves_the_other_settings_intact` never pinned a non-default
  `speech_language`** — `low`. A save that reset a hand-chosen language would
  have passed CI; it now asserts `"en"` survives.
- **Two `cargo fmt` failures** — `low`, against the spec's own "no new
  warnings" bar.

**Deferred**

- **A permanently failed session build shows the "still getting ready" notice
  on every press** — `medium`. With no model files, `is_ready` stays false, so
  each Speak Action promises "this happens once and takes about a minute, your
  line will be spoken" and then reports the failure. The eventual failure
  notification does correct it, and the smallest honest fix needs a new signal
  on `TtsPort` ("a build has already failed"), which is public surface rather
  than a patch.
- **The queue behind the sessions mutex is unordered and unbounded** — `low`.
  `std::sync::Mutex` gives no FIFO guarantee, so three rapid Speak Actions can
  be generated out of the order typed, and each waiter parks a blocking-pool
  thread. Neither the spec nor the epic AC promises ordering — both say
  "queued, not concurrent" — and a FIFO ticket queue is real machinery.
- **`unsafe { set_var }` in the notify-linux test** — `low`. Its SAFETY note
  claims a single-threaded body, which holds only because that crate has
  exactly one test; a second one makes it racy.

**Rejected**

- **"Still working" is not deduplicated across queued actions** — `false`.
  The matrix says "at most once per action" and the README says a Speak Action
  gets one; per-action is the specified behaviour, not a defect.
- **`cx.update`'s `Err` is ignored, dropping the line silently** — `false`.
  That `cx.update` returns `()`, which is exactly why clippy flagged the
  `let _ =` as `let_unit_value`. There is no `Err` to handle.
- **`resolved_speech_backend` hardcodes CPU while claiming to read the build
  variant** — `false`. Its doc says the only honest input *is* which variant
  the binary was compiled as, and everything CI builds is `cpu`. It never
  claims to consult a `cfg`.
- **Warm-up never fires for a sample recorded after startup** — `low`,
  rejected. This is Decision 3 as written ("eager at startup, but only when a
  Reference Voice Sample already exists"), and the cold-start notification is
  what covers that user's first Speak Action.
- **`hound` is an unconditional dependency for a debug-only seam** — `low`,
  rejected. Story 2.9 deletes the seam; a feature gate now is complexity with
  a known expiry date.
- **`current_state` does file I/O on the main thread** — `low`, rejected. A
  small TOML read, and the same thing `has_active_sample` and `open_settings`
  already do at startup.


## Design Notes

**Queueing is a mutex, not a queue.** `TtsPort::generate` is already a blocking call driven through `spawn_blocking`; a `Mutex<Option<Sessions>>` held across the generation gives "one at a time, second one waits" with no channel, no worker thread and no ordering machinery. `generate::generate` takes `&mut Sessions`, which forces the same exclusivity anyway.

**Where the audio goes in this story.** `VirtualMicPort::play` is `todo!()` until Story 2.9, so calling it would panic. This story stops at a produced `AudioBuffer`: log its duration, and — env-gated on `VOICE_ME_DEBUG_WAV_DIR` — write it as a wav so the end-to-end path is listenable. That gate is the temporary seam 2.9 removes.

**Backend resolution without Epic 3.** `voice-me-deps` emits nothing yet and `DependencyProvisioningPort::check` has no `events` parameter. Resolve `AppState`'s backend from this build's compile-time variant alone and leave the detection input for Story 3.1 — do not change the deps port here.

## Verification

**Commands:**
- `cargo check --workspace` -- expected: clean, including both Windows stubs
- `cargo test --workspace` -- expected: the new queueing/failure/notification tests pass fixture-free
- `cargo clippy --workspace` -- expected: no new warnings
- `cargo run -p voice-me-app` with the cache provisioned -- expected: hotkey → type → Enter → overlay closes, generation logged, wav written under `VOICE_ME_DEBUG_WAV_DIR`

**Manual checks:**
- Listen to the written wav: the typed line, in the cloned voice, in the selected speech language
- Point the cache at an empty directory: a notification names the missing path instead of a panic or silence
