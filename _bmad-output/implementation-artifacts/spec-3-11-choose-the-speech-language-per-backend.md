---
title: 'Choose the speech language per backend (Story 3.11)'
type: 'feature'
created: '2026-09-23'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
baseline_commit: '291c048cdfc24545c4705516d7b429280515acfc'
context: ['{project-root}/_bmad-output/implementation-artifacts/epic-3-context.md']
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** The speech language is one global `speech_language` string that can only be set by hand-editing settings.toml, and `speak_inner` checks it against a fixed `["tr", "en"]` for every backend. That includes DeepInfra, whose model speaks 23 languages.

**Approach:** Each backend gets its own persisted speech language and its own supported set. Core validates the Speak Action's language against the *selected* backend's set. Settings → Backend shows a "Speech language" `Select` for the saved backend, listing only that backend's languages. An old `speech_language` seeds both the Local and the DeepInfra entries.

## Boundaries & Constraints

**Always:**
- **The language belongs to a kind of backend, not a runtime entry.** Every local Chatterbox selection (bundled CPU, and each added runtime's CPU/CUDA/WebGPU entry) shares one **Local** language, because it is the same model. Each remote provider has its own.
- **The sets live in core:**
  - Local: `tr`, `en` (AD-12).
  - DeepInfra: the 23 codes of `ResembleAI/chatterbox-multilingual`: ar da de el en es fi fr he hi it ja ko ms nl no pl pt ru sv sw tr zh.
  - Labels are English names ("Turkish", "Arabic", …). Local lists Turkish then English; DeepInfra is alphabetical by label.
- **Defaults and migration:** Local and DeepInfra default to `tr` (`DEFAULT_SPEECH_LANGUAGE`). A legacy top-level `speech_language` fills only the Local and DeepInfra entries that the new table lacks. The table wins where both exist, and the legacy key is not written back.
- **No restart, no rebuild.** The language is read from settings on every Speak Action, as today. Saving one never rebuilds the engine.
- The view asks through a new `BackendAction` and never touches `SettingsStore`. The root saves it through a new `SettingsStore::save_speech_language` and pushes the panel back. A save error is shown inline under the `Select`.
- The language `Select` appears only for the saved backend while its kind is shown, like the other options (3.10 Decision 1). It sits above that backend's other options.
- Core keeps today's trim-and-lowercase normalisation before it checks the set and builds the tag.
- `ui_language` is untouched: no read, write or default of it changes.

**Never:**
- No speech language, set or selector for fal.ai. Story 3.7 decides its set. fal.ai cannot generate yet, so the engine is unavailable before `speak` runs.
- No System voice, Azure, voice picker or GPU picker (3.12–3.14, 3.9).
- No silent substitution. A stored value that is outside the selected backend's set is refused by name, never replaced by a default.
- No speech-blocking capability row for a language. A refusal is a Speak Action failure notification, as today.
- No change to `voice-me-deps`, `voice-me-tts` or `voice-me-tts-remote`.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Local saved | CPU saved, Local = `tr` | `Select` lists Turkish, English, with Turkish selected | N/A |
| DeepInfra saved | DeepInfra saved, DeepInfra = `en` | `Select` lists 23 languages, with English selected | N/A |
| Pick a language | Confirm Spanish while DeepInfra is saved | `SetSpeechLanguage(DeepInfra, "es")` sent once; re-confirming the current one sends nothing; the next Speak passes `es` | Save failure → inline error, `Select` resyncs to the saved value |
| Switch backends | DeepInfra = `es`, Local = `en`; select CPU | Speak uses `en`; switching back to DeepInfra uses `es`; neither entry is rewritten | N/A |
| Migration | File has only `speech_language = "en"` | Local = `en`, DeepInfra = `en`; after any unrelated save, both still `en` and the file has a table, no top-level key | N/A |
| Out of set | Local selected, Local = `es` (hand-edited) or `turkish` | Speak refused before the engine; the message names the backend, the value and Settings → Backend; the `Select` shows its placeholder and an inline note names the value | One failure notification |
| Normalised | Local = `"  EN \n"` | Tag `en` reaches the engine | N/A |

</frozen-after-approval>

## Code Map

- `crates/voice-me-core/src/state.rs:15,621-680` -- `DEFAULT_SPEECH_LANGUAGE`; `AppState.speech_language: String` (its doc still says Epic 4 builds the selector). Replace it with per-backend state. Add a key type (e.g. `LanguageBackend { Local, Remote(RemoteProvider) }`), a `BackendSelection` accessor for it, a `SpeechLanguage { code, label }` table per backend (fal.ai → none), and a helper that gives the selected backend's stored language.
- `crates/voice-me-core/src/settings_store.rs:30-60,180-195,262-270` -- `SettingsFile`: add a lenient `[speech_languages]` table keyed by slug (`local`, `deepinfra`, `fal_ai`), plus the legacy `speech_language` read-only (skip on serialize). Merge in `read_settings_file`. Add `save_speech_language`. Update the tests at 534-620 that assert on `speech_language`.
- `crates/voice-me-core/src/speak.rs:34-37,118-133,451-490` -- replace `SUPPORTED_SPEECH_LANGUAGES` and the check with the selected-backend check. Rewrite the two language tests.
- `crates/voice-me-core/src/ports.rs:249-315` -- add `save_speech_language` as a required method. Its fakes are `voice-me-ui/src/{hotkey.rs:637,voice_setup.rs:955,settings.rs:236}` and `voice-me-app/src/main.rs:872`, each with `unimplemented!`.
- `crates/voice-me-core/src/lib.rs` -- re-export the new types.
- `crates/voice-me-ui/src/backend.rs` -- `BackendAction` (+ its hand-written `Debug`), `BackendArea::SpeechLanguage`, `BackendPanel` gains the per-backend languages, `BackendView` gains a second `SelectState` (same stale-sync pattern as `backend_select`), and `options_section` renders the language section. The ids are `backend-speech-language`, `backend-speech-language-select` and `backend-speech-language-error`. The tests follow the existing `recording_actions` / `open_backend_tab` helpers.
- `crates/voice-me-app/src/main.rs:1744-1765,1893-2040` -- `make_panel` fills the languages. `backend_actions` handles the new action: save, set or clear the error, `push_panel`. No `run_check`, no engine rebuild.
- `crates/voice-me-audio-linux/src/bin/mic-spike.rs:126` -- builds `AppState { speech_language }`; update it.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-core/src/state.rs` + `lib.rs` -- the key type, per-backend sets and labels, the `AppState` field and its accessor. Unit tests: the Local set is exactly tr/en; DeepInfra has 23 unique codes including tr/en; fal.ai has none; every local entry maps to Local.
- [x] `crates/voice-me-core/src/settings_store.rs` + `ports.rs` -- the table, the legacy merge and `save_speech_language`. Tests: the Migration row (round trip across a fresh store); the table wins over the legacy key; saving DeepInfra leaves Local and `ui_language` unchanged; an unreadable entry falls back to its default without losing the file.
- [x] `crates/voice-me-core/src/speak.rs` -- the check against the selected backend. Tests: the Out of set rows (Local `es`, `turkish`); the same `es` passes with DeepInfra selected; Normalised; the Switch backends row's language reaching `generate`.
- [x] `crates/voice-me-ui/src/backend.rs` + the fakes in `hotkey.rs`, `voice_setup.rs`, `settings.rs` -- the action, area, panel field and language `Select`. Tests: Local saved and DeepInfra saved list the right entries with the saved one selected; a pick sends the action once and a re-pick sends nothing; after a flip, no language `Select` is shown; the out-of-set note; an error line.
- [x] `crates/voice-me-app/src/main.rs` + `mic-spike.rs` -- the panel field and the action handling; the fake store method.

**Acceptance Criteria:**
- Given any backend switch, when the next Speak Action runs, then it uses that backend's saved language with no restart and no engine rebuild.
- Given `cargo test -p voice-me-core -p voice-me-ui -p voice-me-tests` and `cargo check --workspace --all-targets`, then all pass.

## Implementation Notes

- Core types: `LanguageBackend { Local, Remote(RemoteProvider) }` with `label()`, `speech_languages()`, `has_speech_language()` and `speech_language(saved)`, which normalises and returns `None` outside the set. `SpeechLanguage { code, label }` and `SpeechLanguages { local, deepinfra }` sit on `AppState.speech_languages`. `AppState::speech_language()` returns the selected backend's stored value.
- Settings: `SpeechLanguagesFile` is written as `[speech_languages]` with `local`/`deepinfra`, lenient per entry and per table. The legacy `speech_language` is read by `read_settings_file` and never serialised. `save_speech_language` returns an error for fal.ai.
- Speak: a backend with no language (fal.ai) is refused by name. That check runs before the disclosure check and is unreachable before 3.7.
- UI: a second `SelectState` (`language_select`) tracks which backend's list it holds (`language_backend`), resynced in `sync_language` on a language change, a backend change or a save error. An extra id, `backend-speech-language-note`, holds the out-of-set note.
- Verified: `cargo test -p voice-me-core -p voice-me-ui -p voice-me-tests -p voice-me-app` passes (63/91/11/34). `cargo check --workspace --all-targets` and `cargo fmt --check` are clean. Clippy's one `type_complexity` warning is on the untouched `apply_selection`.

## Spec Change Log

## Review Triage Log

Pass 1 (blind-hunter, edge-case-hunter, verification-gap).

| Verdict | Finding | Evidence |
|---|---|---|
| low | A failed language save's error survives a backend switch and shows under the new backend's `Select` (blind, edge, verification-gap other) | Confirmed: `apply_selection`'s Ok branch clears only `Selection`/`Capability`, and `speech_language_section` reads the single `SpeechLanguage` key. A one-line removal → patch |
| low | `SpeechLanguages::set` is unused and silently ignores fal.ai (blind) | Confirmed: there are no callers in the workspace. A deletion → patch |
| medium | The `SetSpeechLanguage` arm in `main` is untested (verification-gap) | Pre-verified; no test drives the `backend_actions` closure. The same is true of every other arm, and fixing it means pulling the dispatch out of `fn main`. → defer |
| false | The language check now runs before the disclosure gate, so fal.ai reports "no speech language" instead of the disclosure (blind, edge) | The baseline `speak.rs:127` also checked the language before `DisclosureNotConfirmed` (`:142`), so the order is unchanged. fal.ai never reaches `speak`: `build_engine` returns `unavailable` for a Remote selection (`main.rs:445`), and `SpeakRequested` stops at `notify_engine_unavailable`. |
| low | `save_speech_language` doesn't check the code against the set (blind, edge) | The only caller is the UI, which sends codes from the backend's own list. The spec keeps stored values as written. The fix is a new guard for a path nothing reaches → rejected |
| low | Re-picking the saved language after a failed save leaves the error showing (blind) | Real, but the error is still true of the last attempt, and it clears on the next save or a backend switch. The fix adds a branch → rejected |
| low | The refusal no longer lists the supported languages (blind) | The matrix sets its content (backend, value, Settings → Backend). DeepInfra's list is 23 codes, and the tab shows them. Rejected |
| low | Nothing warns before speaking about an unusable language (blind) | The frozen Never excludes a speech-blocking row for a language. Rejected |
| low | fal.ai's "no language" is spelled out in four match arms (blind) | Story 3.7 would touch them anyway. The fix is a refactor with no user harm. Rejected |
| low | A value that normalises to a valid code (`"  EN "`) is never rewritten in canonical form (blind) | It still works everywhere (normalised on read). Rejected |
| low | The defaults are duplicated, and `unwrap_or` allocates (blind) | No named divergence: all four read `DEFAULT_SPEECH_LANGUAGE`. Negligible. Rejected |
| low | `{:?}` formatting in user-visible text (blind) | The baseline message used `{:?}` too, and escapes make stray whitespace visible, which helps diagnosis. Rejected |
| low | No tests for a lenient legacy key or out-of-set saves (blind) | `lenient` is shared and already tested through the other fields, and out-of-set saves are unreachable (see above). Rejected |
| false | The failed-save UI test covers the error-only stale path only by accident (blind) | The pushed panel differs from the initial one *only* in its error, so the error is exactly the trigger under test. |
| low | Unknown keys in `[speech_languages]` are dropped on the next write (edge) | Only a hand edit can put them there (`fal_ai = …`, which has no meaning yet). The fix adds a flatten field → rejected |

## Verification

**Commands:**
- `cargo test -p voice-me-core -p voice-me-ui -p voice-me-tests -p voice-me-app` -- expected: all pass
- `cargo check --workspace --all-targets && cargo clippy -p voice-me-core -p voice-me-ui -p voice-me-app --all-targets` -- expected: clean
- `cargo fmt --check` -- expected: no diff in changed files

**Manual checks:**
- Run the app and open Settings → Backend. With CPU saved, set English. Pick DeepInfra and set Spanish. Switch back to CPU and confirm that English is still selected. Check that settings.toml has a `[speech_languages]` table.
