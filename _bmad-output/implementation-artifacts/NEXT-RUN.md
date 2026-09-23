# Next run: finish Story 3.12 (eSpeak NG on Linux)

Status as of 2026-09-23: 3.12 is implemented and committed as work in progress. Its spec (`spec-3-12-speak-instantly-with-espeak-ng-on-linux.md`) is `in-review`, and sprint status still says `in-progress`. Review pass 1 ran (blind-hunter, edge-case-hunter, verification-gap) and was triaged. The run stopped before the patches were applied.

Resume with `/bmad-build` on the 3.12 spec. It routes to step 4 (review). Apply the patches below, re-run verification, and write the triage log. Then finish step 5: mark the spec `done`, set sprint status to `review`, and commit. Delete this file, and its line in `AGENTS.md`, once they are done.

## Patches to apply (all verified real)

1. **Linux gate.** `crates/voice-me-tts-system-linux/src/lib.rs` and `tests/espeak.rs` have no `#![cfg(target_os = "linux")]`. The Windows CI job compiles the whole workspace and fails on `std::os::unix`. Gate both, as `voice-me-audio-linux/src/lib.rs:29` does.
2. **CI runs the real engine.** Add `espeak-ng` to the `build-ubuntu` apt-get list in `.github/workflows/ci.yml`. Without it, `tests/espeak.rs` skips itself in CI.
3. **Show a failed voice listing.**
   - `refresh_system_voices` in `crates/voice-me-app/src/main.rs` (~1885) only calls `eprintln!` when `list_voices()` fails. The eSpeak NG row stays Ready, and the user is sent to Dependencies, which shows nothing wrong.
   - Capture `backend_errors`. On `Err`, insert `BackendArea::SpeechLanguage`: "Couldn't list the System voice's voices: {error}". On `Ok`, remove it. Push the panel after either.
   - Fix the comment above the closure, which wrongly says "the engine row already says why".
4. **Engine-neutral wording.** `crates/voice-me-core/src/state.rs:324` and `crates/voice-me-ui/src/backend.rs:858` point at Settings → Dependencies and name "eSpeak NG". The System voice is platform-neutral (3.13 is Windows). Point both at Settings → Backend and name no engine. Update any test in `speak.rs` (~953) that asserts the old text.
5. **No auto-open for System voice users.** With System voice selected and no Reference Voice Sample, Settings auto-opens on every launch (`main.rs` ~2472, `if !has_active_sample`). Skip that startup auto-open when the loaded selection `is_stock_voice()`. Leave the Settings window's own `has_active_sample` (~2256) alone.
6. **Assert the note's text.** The UI test `an_unlisted_voice_or_language_shows_the_placeholder_and_a_note` (`backend.rs` ~3649) only checks that the note exists, and both branches share its id. Assert the text: the empty-list case names Settings → Backend, and the `"xx"` case names `xx`.

## Defer (append to `deferred-work.md`)

- **A late report after a backend switch can start the wrong warm-up.** A late SystemVoice or Remote Dependency Check report that lands after a switch to CPU passes `for_current_selection`, because `resolve_backend` maps both to the CPU placeholder (`main.rs` ~2565). The ONNX warm-up can then start with the runtime unverified. This was already true for Remote selections, before 3.12.
- **The Speak Action's state assembly is untested.** Nothing tests the root's refresh-and-speak path: `refresh_system_voices` → `current_state(…, &system_voices)` → `speak`. To test it, move that assembly out of `main()` into a function that can be tested. This goes with the existing `backend_actions` deferral from 3.11.

## Rejected (log them as rejected in the spec's Review Triage Log)

- **Speaking before the async voice list lands.** The window is about 50 ms, and a user cannot reach it by typing.
- **Out-of-order refresh overwrite, and a stale list after deselecting.** Rare, and the lists are identical.
- **Grandchild processes holding the pipes past a kill.** `espeak-ng` spawns none, and the fix would need process groups.
- **32-bit overflow in the WAV chunk offset.** Only 64-bit targets are built.
- **The fixed 10 s deadline.** The spec sets it, and eSpeak runs far faster than real time.
- **The copied `resample`.** The spec directed the copy.
- **The quadratic language grouping.** With about 140 voices, it costs nothing.
- **`save_speech_voice` accepting any value.** The only caller is the UI, which sends listed ids.

## Also note

- The implementation agent twice saw `espeak-ng --voices` time out under `cargo test`, right after a rebuild. It could not reproduce this, and gave `--voices` no stdin afterwards (`Stdio::null()`). If it recurs, the voice list comes back empty. Patch 3 makes that failure visible in the UI.
- The manual checks in the spec's Verification section have not been run, nor has the AC "plays within about a second".
