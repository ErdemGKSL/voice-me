---
title: 'Import an Existing Audio File as the Reference Voice Sample'
type: 'feature'
created: '2026-09-20'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
baseline_commit: '40e438077e73738ac0f4dcc11ddec45a6b7f0dba'
context:
  - '{project-root}/_bmad-output/planning-artifacts/architecture/architecture-voice-me-2026-09-20/ARCHITECTURE-SPINE.md'
  - '{project-root}/_bmad-output/implementation-artifacts/epic-1-context.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Voice Setup (Story 1.2) only lets the user establish a Reference Voice Sample by recording live; there is no way to reuse a clip the user already has, forcing a live re-record even when a good sample already exists as a file.

**Approach:** Add an "Import" `Button` beside Record in `VoiceSetupView` that opens GPUI's native file picker (`cx.prompt_for_paths`), decodes the chosen file (WAV/MP3 — the two formats Chatterbox-Multilingual V3's own reference-audio docs support), validates its duration against the same 5–60s bounds as recording, and — once valid — converges on the exact same `PendingClip`/Play/Accept flow Story 1.2 already implements, so `SettingsStore::save_reference_voice_sample` sees no difference between a recorded and an imported clip.

## Boundaries & Constraints

**Always:**
- Import is a gpui-kit `Button` beside Record, using `cx.prompt_for_paths` (GPUI's own native file-picker API) — no hand-rolled dialog, no third-party file-dialog crate.
- Supported import formats are exactly WAV and MP3 (`rodio`'s `wav`/`mp3` decode features) — the two formats Chatterbox-Multilingual V3's documented reference-audio upload supports. Any file that fails to decode (wrong format, corrupt, or a format outside these two) shows "Unsupported file format." rather than failing silently.
- A picked file's *decoded* duration is checked against the existing `MIN_RECORDING_SECS`/`MAX_RECORDING_SECS` (5s/60s) constants from Story 1.2 — same bounds, same "never silent failure" rule, distinct wording so the user knows it came from an import.
- A file that passes validation is decoded to samples and re-encoded via the existing `encode_wav`, producing one `PendingClip` — the same representation Story 1.2's Record/Stop path produces — so Play/Accept/re-record/re-import all reuse those existing methods unmodified.
- Import is disabled while a recording is in progress; Record and Import are both disabled while a file dialog is open, so only one capture/import flow runs at a time.
- Cancelling the file picker (no path chosen) leaves all existing view state (any prior pending clip, error, or success banner) untouched.

**Never:**
- No drag-and-drop import, no multi-file import.
- No changes to `SettingsStore`/`FileSettingsStore` — Accept already persists whatever `PendingClip.wav_bytes` holds, regardless of origin.
- No Settings shell (Story 1.4) or first-run auto-open (Story 1.5) — this story's window still shows only the Voice Setup screen directly.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Happy path (WAV) | Import → pick a valid 12s `.wav` | Inline playback appears; Accept becomes available, same as a recording | N/A |
| Happy path (MP3) | Import → pick a valid 12s `.mp3` | Decodes and converges on the same pending-clip flow as `.wav` | N/A |
| Unsupported format | Import → pick a `.flac`/`.txt`/corrupt file | Inline message: "Unsupported file format."; Accept stays disabled | Mapped at the `voice-me-ui` boundary; no file written |
| Imported clip too short | Picked file decodes to <5s | Inline message: "Imported clip too short — must be at least 5 seconds"; Accept stays disabled | No file written |
| Imported clip too long | Picked file decodes to >60s | Inline message: "Imported clip too long — must be at most 60 seconds"; Accept stays disabled | No file written |
| Dialog cancelled | User closes the file picker without choosing | No visible change; any prior pending clip/error/success state is preserved | N/A |
| Selected file unreadable | File removed/permission error after picking | Inline message: "Couldn't read the selected file." | Mapped to a fixed message; no file written |

</frozen-after-approval>

## Code Map

- `crates/voice-me-ui/Cargo.toml` — add the `mp3` feature to the existing `rodio` dependency (alongside `playback`/`wav`), and add `tempfile` as a dev-dependency (mirrors `voice-me-core`/`voice-me-tests`) for writing fixture audio files in tests.
- `crates/voice-me-ui/src/voice_setup.rs` — extend the existing `VoiceSetupView` (Story 1.2, do not restructure it):
  - New `ImportSource` trait (mirrors `CaptureSource`'s test-seam pattern): `fn pick_file(&self, cx: &mut Context<VoiceSetupView>) -> Task<Option<PathBuf>>`. Production impl `GpuiImportSource` wraps `cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Import".into()) })`.
  - `VoiceSetupView` gains an `import_source: Arc<dyn ImportSource>` field (defaulted to `GpuiImportSource` in `new`, injectable via a `new_with_capture_and_import_sources` test constructor) and an `is_importing: bool` field.
  - New `fn start_import(&mut self, cx)`: sets `is_importing = true`, spawns via `cx.spawn` to await `import_source.pick_file(cx)`, then dispatches to a new `fn handle_imported_path(&mut self, path: Option<PathBuf>, cx)` that does the actual read/decode/validate and only then clears `error_message`/`pending_clip`/`just_saved`/`_playback` (so a cancel is a true no-op).
  - New free function `decode_audio_file(bytes: &[u8]) -> Result<(Vec<f32>, u32, u16), VoiceMeError>` using `rodio::Decoder` over an in-memory `Cursor` (mirrors `play_wav_bytes`'s existing `Decoder` usage) — read channels/sample rate before consuming the decoder as a `Vec<f32>` sample iterator.
  - `render()`: add an `Import` `Button` next to Record, `.disabled(is_recording || is_importing)`; disable Record with `.disabled(is_recording || is_importing)` too.
- Reused as-is, no changes needed: `PendingClip`, `accept()`, `play_pending_clip()`, `encode_wav`, `recording_meets_minimum_duration`/`MIN_RECORDING_SECS`/`MAX_RECORDING_SECS` (reused directly for the imported-duration check), `SettingsStore`/`FileSettingsStore`.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-ui/Cargo.toml` -- add `mp3` to the `rodio` feature list; add `tempfile` dev-dependency -- decode `.mp3` imports and build test fixture files
- [x] `crates/voice-me-ui/src/voice_setup.rs` -- add `ImportSource`/`GpuiImportSource`, `decode_audio_file`, `start_import`/`handle_imported_path`, and the Import `Button` in `render()` -- delivers the import flow, converging on the existing pending-clip/Play/Accept path
- [x] `crates/voice-me-ui/src/voice_setup.rs` (`#[cfg(test)]`) -- add a `FakeImportSource` and UI-integration tests covering the I/O matrix rows above (valid wav, valid mp3, unsupported format, too short, too long, cancelled)

**Acceptance Criteria:**
- Given no Reference Voice Sample exists, when I choose Import, pick a valid `.wav` file, and Accept, then a fresh `FileSettingsStore::load` reports it as the active Reference Voice Sample, the same as an accepted recording would
- Given a file has been imported and is pending Accept, when I use inline Play, then I hear the imported audio before deciding
- Given an active Reference Voice Sample already exists, when I import and Accept a new file, then the old clip is replaced only once that Accept succeeds (same replace-on-Accept rule as re-recording)

## Implementation Notes

- `ImportSource`/`GpuiImportSource` added exactly as planned; `GpuiImportSource::pick_file` wraps `cx.prompt_for_paths` and converts its `oneshot::Receiver` into a `Task<Option<PathBuf>>` via `cx.foreground_executor().spawn`, taking only the last selected path (picker is single-file: `multiple: false`).
- `decode_audio_file` reads `channels`/`sample_rate` off the `rodio::Decoder` (via the `Source` trait) before consuming it as a `Vec<f32>` sample iterator, mirroring `play_wav_bytes`'s existing `Decoder` usage; any decode failure (wrong format, corrupt bytes, or a codec outside the enabled `wav`/`mp3` rodio features) is reported as one `VoiceMeError` case, mapped to `UNSUPPORTED_FORMAT_MESSAGE` at the call site.
- `handle_imported_path` defers clearing `pending_clip`/`error_message`/`just_saved`/`_playback` until *after* the file is read and decoded successfully enough to know a real new candidate clip exists — so a cancelled dialog, or a read failure before decode, does not wipe prior state; a decode failure or an out-of-bounds duration does still clear the old pending clip before showing the new error (matches the existing Record/Stop rejection behavior, which also drops the old pending clip on a rejected new attempt).
- Record and Import are cross-disabled (`is_recording || is_importing`) so only one capture/import flow runs at a time, per the frozen "Always" rule.
- Test fixtures needed no new runtime dependency: valid WAV fixtures reuse the existing `encode_wav`; a valid MP3 fixture is a hand-assembled minimal silent MPEG-1 Layer III frame (all-zero side info, `part2_3_length = 0`, no bit-reservoir dependency) repeated to reach the target duration — verified to actually decode via `rodio::Decoder` in the test run itself, though it is synthetic rather than produced by a real encoder.
- Review pass (see Review Triage Log): added the missing "selected file unreadable" test; `voice-setup-accept`/`voice-setup-play` are now also disabled while `is_importing` (an in-flight import can no longer race a stale Accept/Play into acting on a `pending_clip` it's about to reset), covered by a new test using an `ImportSource` whose `Task` never resolves; `ImportSource::pick_file` now returns `Task<Result<Option<PathBuf>, VoiceMeError>>` so a genuine file-picker failure shows `PICKER_FAILED_MESSAGE` instead of being indistinguishable from a cancel; the `encode_wav` failure branch now uses a named `IMPORT_ENCODE_FAILED_MESSAGE` constant; the three-statement pending-state reset is now a single `reset_pending_state` helper; and the `write_temp_file` test helper no longer leaks fixture files (`NamedTempFile`'s guard is held for the test's duration instead of `.keep()`-and-drop).

**Verification performed:**
- `cargo build --workspace` — exits 0.
- `cargo test --workspace` — all pass, including 9 new `voice-me-ui` import tests (valid wav, valid mp3, unsupported format, too short, too long, cancelled, unreadable path, picker failure, accept/play inert during an in-flight import) alongside the unchanged Story 1.2 tests — 15 total in `voice-me-ui`.
- `cargo clippy --workspace --all-targets` — clean except the pre-existing unrelated `voice-me-tests::placeholder` warning.
- `cargo fmt --check` — clean.

**Known risks / left incomplete:**
- No real-hardware/manual pass (`cargo run -p voice-me-app`, picking a real `.wav`/`.mp3` via the actual OS file dialog) was performed in this sandboxed environment — only the fake-`ImportSource` automated path is verified.
- The MP3 fixture is a synthetic single fixed-bitrate/sample-rate/mono silent frame, not output from a real encoder; a real-world `.mp3` (VBR, ID3 tags, stereo, etc.) exercises the same `rodio::Decoder` call but wasn't itself tested here.

## Spec Change Log

## Review Triage Log

- **patch** — the "Selected file unreadable" I/O matrix row (`FILE_UNREADABLE_MESSAGE`) has no covering test — only wav/mp3/unsupported-format/too-short/too-long/cancelled are exercised, so the `std::fs::read` failure branch in `handle_imported_path` is unverified, failing the Matrix Test Audit. Fix: add a test that points a `FakeImportSource` at a path that doesn't exist and asserts the message/no-save outcome. (blind-hunter)
- **medium** — `voice-setup-accept` is disabled only by `!has_pending_clip`, not by `is_importing`; between an Import click and the picker resolving, an existing `pending_clip` (from an earlier recording/import, including one left behind by a *failed* Accept awaiting retry) can still be Accepted. `handle_imported_path` then unconditionally resets `pending_clip`/`just_saved` once the import resolves, silently erasing that in-flight Accept's outcome (a shown "Saved" confirmation, or a failed-Accept's retry state) without any error or indication. Fix: disable Accept (and Play, for consistency) while `is_importing` is true. (edge-case-hunter, verification-gap — same root cause)
- **patch** — the `encode_wav` failure branch in `handle_imported_path` uses an inline string literal instead of a named `*_MESSAGE` constant like every other branch in this file, breaking the established convention. Fix: name it `IMPORT_ENCODE_FAILED_MESSAGE`. (blind-hunter)
- **patch** — `GpuiImportSource::pick_file` folds a genuine file-picker failure (`Ok(Err(_))`/`Err(_)` from the prompt's receiver — documented as possible on Linux if the picker can't be opened) into the same `_ => None` arm as a user cancelling, so a real failure produces no message and is indistinguishable from "nothing happened," unlike every other failure path in this file. Fix: widen `ImportSource::pick_file` to `Task<Result<Option<PathBuf>, VoiceMeError>>`, mirroring the existing `CaptureSource::start() -> Result<_, VoiceMeError>` convention in the same file, and show a message on `Err`. (blind-hunter)
- **low** — no upfront file-size cap before `decode_audio_file` fully decodes a picked file into memory; an arbitrarily large file is read and decoded in full before the 5–60s bounds can reject it. Requires the user to deliberately pick an oversized file of their own through the native OS dialog in this single-user local app; the fix (a size cap or streaming duration check) is more than a direct correction and no demonstrated bad outcome exists today. Rejected. (blind-hunter, edge-case-hunter — same root cause)
- **low** — `Decoder`'s `Iterator::next` returns `None` on a mid-stream decode error the same as on a clean end-of-stream (rodio/symphonia give no way to distinguish the two through the public API used here), so a file that decodes cleanly for a while then hits corrupt data yields a shorter clip rather than an "Unsupported file format" error. Mitigated by the story's own required inline-Play-before-Accept step, which lets the user hear and reject a truncated/garbled result before it's ever saved; the fix would need deeper `symphonia` error-reporting access than `rodio::Decoder` exposes. Rejected. (blind-hunter)
- **low** — the `encode_wav` failure branch in `handle_imported_path` (like its pre-existing twin in `stop_recording` from Story 1.2) has no covering test; `hound::WavWriter` failing on the already-valid `channels`/`sample_rate`/`samples` this method is called with has no demonstrated trigger, matching the same untested-but-implausible pattern already accepted in Story 1.2's own review. Rejected. (blind-hunter)
- **low** — clicking Import twice before the first click's re-render disables it could start two concurrent `pick_file` tasks, with the later one's result winning. Requires a double-click faster than a single render frame; the same class of unaddressed reentrancy already exists on Record/Stop in Story 1.2 and wasn't required there either. Rejected. (edge-case-hunter)
- **low** — the `write_temp_file` test helper persists and leaks every fixture file (via `NamedTempFile::keep()`) instead of keeping the `NamedTempFile` guard alive for the test's duration; harmless (OS temp dir) but accumulates across runs. Fix is a direct, contained change to the test helper only. (blind-hunter)
- **low** — the three-statement `pending_clip = None; just_saved = false; _playback = None;` reset is duplicated across `handle_imported_path`'s unreadable-file, decode-failure, and pre-duration-check branches; a future edit to one call site could drift from the others. Fix: extract a small `reset_pending_state(&mut self)` helper. (blind-hunter)

## Design Notes

Decode-then-re-encode (rather than storing the original file bytes) keeps `SettingsStore` and the Play/Accept code path single-shaped: whether a `PendingClip` came from the microphone or an imported file, it is always PCM samples run through the same `encode_wav`, so nothing downstream needs to know or care which path produced it.

## Verification

**Commands:**
- `cargo build --workspace` -- expected: exits 0
- `cargo test --workspace` -- expected: all existing tests plus the new import UI-integration tests pass
- `cargo clippy --workspace --all-targets` -- expected: clean (aside from the pre-existing unrelated `voice-me-tests::placeholder` warning)
- `cargo fmt --check` -- expected: clean

**Manual checks (if no CLI):**
- `cargo run -p voice-me-app`: click Import, pick a real short `.wav` and a real short `.mp3` from disk, confirm inline playback and Accept for both, and confirm `reference_voice_sample.wav` under the OS data dir updates each time.
