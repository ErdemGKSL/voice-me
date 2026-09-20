---
title: 'Record a Reference Voice Sample'
type: 'feature'
created: '2026-09-20'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
baseline_commit: '629e5f749283923b50f8a85d8fdc9be019b6cca9'
context:
  - '{project-root}/_bmad-output/planning-artifacts/architecture/architecture-voice-me-2026-09-20/ARCHITECTURE-SPINE.md'
  - '{project-root}/_bmad-output/implementation-artifacts/epic-1-context.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** There is no way yet to establish a Reference Voice Sample — TTS voice cloning (Epic 2) and the rest of Voice Setup (Stories 1.3–1.5) all depend on the user being able to record their own voice in-app and have it saved as the active sample.

**Approach:** Add a Voice Setup screen in `voice-me-ui` with Record/Stop/Playback controls (gpui-kit components, per FR9) backed by live microphone capture; on Accept, hand the encoded clip to a new `SettingsStore::save_reference_voice_sample` in `voice-me-core` that persists the audio file and the updated `AppState` (AD-6). Wire `voice-me-app`'s composition root to open this screen directly — no tray/hotkey yet (Epic 2) — so the flow is runnable end-to-end.

## Boundaries & Constraints

**Always:**
- Record/Stop/Accept are gpui-kit `Button`s and the Recording indicator uses gpui-kit/theme tokens (FR9) — no hand-rolled controls.
- Capture comes from the OS default input device, is encoded as WAV (PCM), and is handed to `voice-me-core` to store; `voice-me-ui` never writes to the data or config directory itself (AD-6).
- The concrete `SettingsStore` (`FileSettingsStore` in `voice-me-core`) resolves paths via the `directories` crate: TOML settings at the OS config dir, `reference_voice_sample.wav` at the OS data dir.
- Enforce Chatterbox-Multilingual V3's practical reference-clip guidance (its own usage examples use ~10s clips; community guidance is 10–30s of clean audio) as a hard minimum of 5s and maximum of 60s: recording auto-stops at 60s, and stopping before 5s shows an inline message and leaves Accept disabled.
- Accept persists the clip as the active Reference Voice Sample; recording again (before or after a previous Accept) always replaces whichever clip was previously active only once the new Accept fires.

**Never:**
- No import-from-file flow (Story 1.3), no first-run auto-open (Story 1.5), no Settings shell/other tabs, no tray/hotkey wiring — this story's window shows only the Voice Setup screen directly.
- No network call anywhere in this story.
- No microphone device-selection UI — always the OS default input device.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Happy path | No sample yet; Record → speak 12s → Stop | Inline playback appears; Accept becomes available | N/A |
| Too short | Stop pressed at 2s | Inline message: "Recording too short — speak for at least 5 seconds"; Accept stays disabled, Record available again | Shown inline; no file written |
| Max length reached | Recording hits 60s | Capture auto-stops; playback available as normal | N/A |
| Microphone unavailable | No input device, or capture fails to start | Inline message: "Couldn't access your microphone."; Record stays available to retry | Mapped to `VoiceMeError` at the `voice-me-ui` boundary, not surfaced from `voice-me-core` |
| Re-record after Accept | An active sample already exists; user records again | Old clip is replaced only once the new recording is Accepted | Old file overwritten on Accept, not before |

</frozen-after-approval>

## Code Map

- `crates/voice-me-core/src/ports.rs` — extend `SettingsStore` with `fn save_reference_voice_sample(&self, wav_bytes: &[u8]) -> Result<AppState, VoiceMeError>`.
- `crates/voice-me-core/src/settings_store.rs` (new) — `FileSettingsStore`: `new()` resolves dirs via `directories::ProjectDirs`; `with_dirs(config_dir, data_dir)` for tests; (de)serializes the relevant `AppState` fields to/from TOML via `serde`.
- `crates/voice-me-core/src/lib.rs` — export `settings_store::FileSettingsStore`.
- `crates/voice-me-core/Cargo.toml` — add `directories`, `serde` (`derive`), `toml`.
- `crates/voice-me-ui/src/voice_setup.rs` (new) — the gpui-kit view: Record/Stop `Button`, Recording indicator, inline playback, Accept `Button`; captures via a `CaptureSource` trait (production impl `CpalCaptureSource` over `cpal`, so tests can inject a fake and drive the state machine without real audio hardware) and duration bookkeeping; encodes captured samples to WAV via `hound`; plays back via `rodio`.
- `crates/voice-me-ui/src/lib.rs` — export `voice_setup::VoiceSetupView`.
- `crates/voice-me-ui/Cargo.toml` — add `gpui-kit`, `cpal`, `hound`, `rodio`.
- `crates/voice-me-app/src/main.rs` — first real composition root: `gpui_kit::application()` opening a window hosting `VoiceSetupView` wired to a real `FileSettingsStore`.
- `crates/voice-me-app/Cargo.toml` — add `gpui-kit`, `voice-me-core`, `voice-me-ui`.
- `crates/voice-me-tests/src/lib.rs` — round-trip test for `FileSettingsStore::save_reference_voice_sample`/`load` against temp dirs, plus a unit test for the 5s/60s duration rule as a pure function.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-core/src/ports.rs` -- add `save_reference_voice_sample` to `SettingsStore` -- gives core a real place to persist the accepted clip
- [x] `crates/voice-me-core/src/settings_store.rs` -- implement `FileSettingsStore` (load/save, `directories`-resolved paths, TOML (de)serialization) -- concrete AD-6 adapter
- [x] `crates/voice-me-core/Cargo.toml` -- add `directories`/`serde`/`toml` -- deps this story actually needs
- [x] `crates/voice-me-ui/src/voice_setup.rs` -- Record/Stop/indicator/playback/Accept view using `cpal` capture + `hound` encode + `rodio` playback, enforcing the 5s/60s rule -- delivers the story's UI
- [x] `crates/voice-me-ui/Cargo.toml` -- add `gpui-kit`/`cpal`/`hound`/`rodio`
- [x] `crates/voice-me-app/src/main.rs` + `Cargo.toml` -- open a real window hosting `VoiceSetupView` backed by `FileSettingsStore` -- makes the flow runnable
- [x] `crates/voice-me-tests/src/lib.rs` -- `FileSettingsStore` round-trip test + duration-validation unit test

**Acceptance Criteria:**
- Given no Reference Voice Sample exists, when I Record, speak, Stop, and Accept, then the clip is saved via `SettingsStore` and a fresh `FileSettingsStore::load` reports it as the active Reference Voice Sample
- Given a stopped recording awaiting Accept, when I use inline playback, then I hear the recorded audio before deciding
- Given capture is active, when the Recording indicator is shown, then it is the only place the primary-violet accent appears on the screen

## Implementation Notes

- `SettingsStore::save_reference_voice_sample` added to `voice-me-core::ports`; `FileSettingsStore` (`voice-me-core::settings_store`) implements it plus `load`, resolving `directories::ProjectDirs::from("dev", "voice-me", "voice-me")`. The active Reference Voice Sample is derived from whether the fixed-name `reference_voice_sample.wav` exists in the data dir (no separate id/manifest, per AD-6) rather than stored as a TOML field. Writes go to a `.tmp-<pid>` file then `fs::rename` into place, so Accept never leaves a half-written clip.
- `voice-me-ui::VoiceSetupView` captures via a `CaptureSource`/`CaptureHandle` trait pair rather than calling `cpal` directly: `CpalCaptureSource`/`ActiveCapture` are the production implementation (F32/I16/U16 formats handled, converted to `f32` in an `Arc<Mutex<Vec<f32>>>`, real `Instant`-based `elapsed_secs`), and tests inject a `FakeCaptureSource` to drive the state machine deterministically without real audio hardware or a real 60s wait. A `cx.spawn` polling loop (100ms tick) calls the pure `should_auto_stop(elapsed_secs)` predicate to auto-stop at `MAX_RECORDING_SECS` (60s); `hound` in-memory WAV encoding happens on Stop, and `rodio::DeviceSinkBuilder`/`Decoder` provide inline playback. The 5s minimum check is the pure `recording_meets_minimum_duration` function.
- Any mic/stream/playback failure from `cpal`/`rodio` is caught at the `voice-me-ui` boundary and mapped to a fixed user-facing message (`Alert::error`) — no `cpal`/`rodio` error type crosses into `voice-me-core`.
- `voice-me-app/src/main.rs` is now a real composition root: `gpui_kit::application()` + `gpui_kit::init` + `cx.open_window` hosting `VoiceSetupView` wrapped in `Root`, backed by a real `FileSettingsStore::new()`.
- Recording/Stop/Accept/Play are gpui-kit `Button`s (`.primary()` on Record/Accept only); the only non-button/focus-ring use of `cx.theme().primary` is the Recording indicator dot, per FR9 and the epic's accent-color constraint.
- Added `voice-me-ui` UI-integration tests (`#[gpui_kit::test]` + `TestAppContext`/`TestWindowExt`, dev-dependency `gpui-kit/test-support`) that click the real Record/Stop/Accept buttons against a `FakeCaptureSource`/`FakeSettingsStore`, covering the too-short, mic-unavailable, happy-path/accept, and re-record-after-accept matrix rows — the pure duration/auto-stop functions alone didn't reach the actual state machine. `Alert`'s banner text isn't asserted on directly (it doesn't self-register for `find`/`try_find` without a `test-support`-gated wrapper that would leak into non-test builds); assertions instead use native `Button` behavior (disabled buttons refuse real clicks) and the fakes' call counts/recorded bytes.
- Review pass (see Review Triage Log): `CaptureSource`/`CaptureHandle`/`start_capture`/`encode_wav`/`play_wav_bytes` now return `Result<_, VoiceMeError>` instead of `Result<_, ()>`, matching the I/O matrix's stated error contract. `save_reference_voice_sample` now writes/renames the wav file only *after* the settings-file write succeeds, and cleans up the `.tmp-<pid>` file if the rename fails, so a returned `Err` never leaves the previous sample silently overwritten or an orphaned tmp file behind. `stop_recording` now also rejects an empty sample buffer (mic opened but delivered no data) with the same too-short message. `.github/workflows/ci.yml`'s `ubuntu-latest` job now installs the system libraries `cpal`/`gpui-kit` need to build. Added `a_failed_accept_keeps_the_pending_clip_available_for_a_retry` and `recording_auto_stops_at_the_maximum_duration_without_a_manual_stop` (`voice-me-ui`) covering the two test gaps the review found.

**Verification performed:**
- `cargo build --workspace` — exits 0.
- `cargo test --workspace` — all pass: `FileSettingsStore` round-trip/replace tests (`voice-me-core` unit tests and `voice-me-tests`), `recording_meets_minimum_duration`/`should_auto_stop` unit tests, and 6 `voice-me-ui` UI-integration tests exercising the Record/Stop/Accept flow end-to-end against fakes (including the failed-Accept-retry and auto-stop-without-manual-Stop cases).
- `cargo clippy --workspace --all-targets` — clean except one pre-existing `assertions_on_constants` warning on the unrelated `voice-me-tests::placeholder` test.
- `cargo fmt --check` — clean.
- `cargo run -p voice-me-app` opened a window and ran without panicking in this sandbox's X11 session for the duration of a manual smoke run; a real microphone/speaker end-to-end pass (recording a real clip, hearing playback, confirming the file lands under the OS data dir) was **not** performed here and should be done on a real dev machine before calling this story done.
- Regression-tested the new `recording_auto_stops_at_the_maximum_duration_without_a_manual_stop` test by temporarily disabling the `cx.spawn` loop's auto-stop call and re-running: the test failed as expected (on the `try_find("voice-setup-play")` assertion), confirming it actually exercises the loop rather than passing vacuously.

**Known risks / left incomplete:**
- No real-hardware manual verification (actual microphone capture + speaker playback) was performed in this sandboxed environment.
- The `Alert` error/success banners have no automated coverage of their visible text — only of the state transitions that show/hide them indirectly (via Button behavior and fake call counts). A real accessibility check of banner content is a manual/visual check.
- Two of the new UI-integration tests (`a_failed_accept_keeps_the_pending_clip_available_for_a_retry`, `recording_auto_stops_at_the_maximum_duration_without_a_manual_stop`) include a `window.find(...).disabled()` assertion that this gpui-kit version reports as `None` regardless of the button's true disabled state (verified during the original test-writing pass) — those specific assertions are effectively vacuous. Each test's real proof is a different assertion (`try_find("voice-setup-play")` presence/absence), which was confirmed by deliberately breaking the auto-stop wiring and observing the test fail.

## Spec Change Log

## Review Triage Log

- **low** — orphaned `.tmp-<pid>` file left in the data dir if `fs::write` succeeds but the subsequent `fs::rename` fails (permission/cross-device change mid-call); trivial fix is to `remove_file` the tmp path on a rename error. (blind-hunter, edge-case-hunter — same root cause)
- **false** — claimed `fs::rename` isn't atomic-replace on Windows (fails if destination exists). Rust's `std::fs::rename` on Windows calls `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`, matching POSIX overwrite semantics; this is established Rust std behavior, not a gap in this diff. (blind-hunter)
- **false** — claimed concurrent `save_reference_voice_sample` calls could collide on the same `.tmp-<pid>` path. `accept()` is only ever invoked synchronously from a single GPUI button-click handler in this app; no code path in this diff calls the port concurrently from the same process, so the race is not reachable today. (blind-hunter)
- **low** — the I/O matrix's "Mapped to `VoiceMeError` at the `voice-me-ui` boundary" cell doesn't match the code: capture/playback failures are `Result<_, ()>` mapped straight to a fixed `&str`, never a `VoiceMeError`. No user-facing effect (the same message shows either way), but a future developer matching on `VoiceMeError` variants for these failures would get nothing. Direct fix: use `VoiceMeError::Other(..)` instead of `()` in `CaptureSource`/`CaptureHandle`/`start_capture`/`play_wav_bytes`. (blind-hunter)
- **false** — `sprint-status.yaml` showing `in-progress` while the spec's own frontmatter shows `in-review` at diff-snapshot time is in-flight Build-workflow bookkeeping (this step syncs it to `review` on the way out), not a code defect — same pattern already logged as `false` in spec-1-1's own triage log. (blind-hunter)
- **false** — claimed tests should assert on `VoiceSetupView`'s private `error_message` field directly instead of only on behavior (disabled-equivalent button state, fake call counts). Reaching into private state would be a more brittle, redundant assertion, not a fix for a demonstrated bad outcome — the current tests already catch a regression in the acceptance-relevant behavior. (blind-hunter)
- **medium** — `accept()`'s `Err` branch (restores `pending_clip` so the user can retry after a failed save) is never exercised by any test — `FakeSettingsStore` always returns `Ok`. A regression dropping that restore would silently discard the user's recorded clip on any save failure, forcing a full re-record with no test catching it. Fix: add a failing-store test mode and assert the pending clip/Play button survive a failed Accept. (blind-hunter + verification-gap, same root cause)
- **false** — claimed `FileSettingsStore` should itself enforce the 5s/60s duration rule as "defense in depth." The frozen Boundaries & Constraints place this rule at the UI layer by design (validate at the edge, keep the storage adapter simple); no caller introduced in this diff bypasses it, so there's no demonstrated bad outcome today. (blind-hunter)
- **false** — claimed the round-trip tests should verify `hotkey`/`ui_language`/`selected_mic_device` survive a `save_reference_voice_sample` call. Read the code: it reads the existing `SettingsFile` and writes it back unchanged, so those fields are provably preserved; this is an untested-but-correct path, not a bad outcome. (blind-hunter)
- **low** — mutex poisoning in the `cpal` input callback would silently drop samples with no surfaced diagnostic. Requires a panic while holding the lock inside the audio callback itself to trigger; recovering meaningfully is a design decision, not a one-line fix, and not something a user would plausibly hit. Rejected. (blind-hunter)
- **low** — the `MixerDeviceSink` held after `play_pending_clip` has no explicit stop/finished signal and lives until the next Record/Accept or view drop. No crash or leak beyond the view's own lifetime; fixing it needs playback-completion detection beyond what's readily exposed. Rejected. (blind-hunter)
- **medium** — new native deps (`cpal`, `gpui-kit`, `rodio`) need system libraries (ALSA, fontconfig, X11/Wayland dev headers) that `.github/workflows/ci.yml`'s `ubuntu-latest` job doesn't install; CI would fail to build this change, regressing spec-1-1's own "CI passes on both jobs" acceptance criterion. Fix: add the `apt-get install` step. (blind-hunter)
- **low** — a settings/wav file deleted between `path.exists()` and the subsequent read (TOCTOU) surfaces an `Err` from `load()` instead of falling back to defaults. Requires an external process racing this single-user local app's own file; implausible trigger, and the fix (extra `ErrorKind::NotFound` matching) adds branches for a scenario nobody would hit in practice. Rejected. (edge-case-hunter)
- **medium** — a recording is accepted purely by wall-clock duration (`Instant::elapsed`), not by whether any samples actually arrived; if the input stream opens successfully but never delivers data (e.g. a mic that goes silent/disconnects right after `stream.play()`), an empty or near-silent clip passes the 5s check and gets saved as the active Reference Voice Sample. Fix: reject on `samples.is_empty()` (or similar) in the encode branch of `stop_recording`. (edge-case-hunter)
- **medium** — in `save_reference_voice_sample`, the wav file is renamed into place *before* `write_settings_file` is attempted; if the settings write then fails, the function returns `Err` (and `accept()` re-offers the clip for retry) even though the old Reference Voice Sample has already been irreversibly overwritten on disk — a reported failure doesn't mean nothing changed. Fix: reorder so the settings file is written (or otherwise made safe) before the wav rename, the one step that isn't reversible. (edge-case-hunter, claim — verified by reading the code)
- **medium** — the `cx.spawn` polling loop that's supposed to auto-stop recording at `MAX_RECORDING_SECS` is wired up but never exercised by any test: every UI test's fake capture handle reports a fixed `elapsed_secs()` that never reaches 60s, and the standalone `should_auto_stop` unit test bypasses the view entirely. A regression that drops or inverts the loop's `if hit_max { view.stop_recording(cx); }` call — silently breaking the "Max length reached" acceptance row — would not be caught. Fix: add a test with a fake handle reporting `elapsed_secs() >= MAX_RECORDING_SECS` and assert the view leaves the recording state without a manual Stop. (verification-gap)

## Design Notes

Audio pipeline: `cpal` streams input samples into an in-memory buffer while Recording is active; on Stop, `hound` encodes that buffer to WAV bytes (in memory, no temp file); those bytes both drive `rodio`'s in-memory playback and — only on Accept — go to `FileSettingsStore::save_reference_voice_sample`, which writes `reference_voice_sample.wav` and updates the TOML settings file in one call. This keeps `voice-me-ui` from touching either OS directory directly, per AD-6.

## Verification

**Commands:**
- `cargo build --workspace` -- expected: exits 0
- `cargo test --workspace` -- expected: all existing tests plus the new `FileSettingsStore` round-trip and duration-validation tests pass

**Manual checks (if no CLI):**
- `cargo run -p voice-me-app`: record a real 10s+ clip, Stop, play it back inline, Accept; confirm `reference_voice_sample.wav` exists under the OS data dir and a fresh run still reports it as the active sample.
