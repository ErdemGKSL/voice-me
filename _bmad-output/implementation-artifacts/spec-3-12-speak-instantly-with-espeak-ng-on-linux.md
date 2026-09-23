---
title: 'Speak instantly with eSpeak NG on Linux (Story 3.12)'
type: 'feature'
created: '2026-09-23'
status: 'in-review'
route: 'dispatch'
review_loop_iteration: 0
baseline_commit: '558a13a10c30fafa870586714092ded74d30506b'
context: ['{project-root}/_bmad-output/implementation-artifacts/epic-3-context.md']
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Every local backend needs the 1.56 GB model and a Reference Voice Sample, and each line takes about 20 s. Linux has no instant, zero-download voice.

**Approach:** Add a new local backend, **System voice**. A new crate, `voice-me-tts-system-linux`, runs `espeak-ng` as a bounded child process behind `TtsPort`. Core learns a stock-voice backend that takes a language and a voice rather than the sample. Settings → Backend lists eSpeak NG's languages, with a voice picker when a language has several voices. A missing `espeak-ng` is a speech-blocking dependency row with the distro's install command.

## Boundaries & Constraints

**Always:**
- **Child process (AD-12's one exception):**
  - `Command::new("espeak-ng")`, resolved on PATH, with no shell.
  - Args exactly `-v <voice id> -b 1 --stdout`; the text goes on stdin, never argv.
  - stdout is read on its own thread so a full pipe can't deadlock.
  - 10 s deadline: on timeout, kill and reap the child.
- **Audio:** eSpeak's WAV is 16-bit mono 22 050 Hz with a *streaming* header (the RIFF and `data` sizes are `0x7ffff…`). Decode to the end of the stream, ignoring the declared length, and resample to 24 kHz mono f32 (AD-11).
- **Voices:** from `espeak-ng --voices`, run the same way (10 s deadline).
  - Each line: `Pty Language Age/Gender VoiceName File [Other…]`. The id is `File` (unique; `-v gmw/en-US` works). The name is `VoiceName` with `_` → space. `variant` lines are skipped.
- **Stock voice:**
  - No Reference Voice Sample check and no sample passed.
  - No disclosure, no socket, no network dependency.
  - Labelled "stock voice" in the backend `Select` entry, with a "Stock voice" tag beside *Selected*.
- **Each eSpeak language code is its own language (answered 2026-09-23).** The language list is the distinct `Language` column values: `en-gb`, `en-us` and `en-029` are three languages. A language's voices are the voices with exactly that code, so the voice picker appears only where a code has several voices (e.g. `yue`: Cantonese and Cantonese Jyutping). A language is labelled with its top-priority voice's name ("English (Great Britain)"), and the list is sorted by label.
- **Per-backend persistence (3.11 pattern):**
  - The System voice language defaults to `tr`; the legacy key does not seed it.
  - The voice is stored separately. Unset means the language's top-priority voice (lowest `Pty`, then list order), shown as selected in the picker.
  - Saving a new System voice language clears the stored voice.
- **Core validates against the listed voices.**
  - The language must be the language of at least one listed voice.
  - A stored voice must be one of that language's voices.
  - Otherwise the Speak Action is refused by name (no substitution), and the picker shows its placeholder with a note, as in 3.11.
- **The dependency row is speech-blocking, with manual steps only** (no Install). The install command comes from `/etc/os-release` `ID`/`ID_LIKE`:
  - arch → `sudo pacman -S espeak-ng`
  - debian/ubuntu → `sudo apt install espeak-ng`
  - fedora → `sudo dnf install espeak-ng`
  - opensuse → `sudo zypper install espeak-ng`
  - otherwise → "Install the espeak-ng package with your distribution's package manager."

  A found binary gives a ready row naming its path.
- **Failures:** non-zero exit (with trimmed stderr), timeout, or unreadable output → one notification whose body names eSpeak NG and the reason. This comes through `speak`'s existing single notification.
- The voice list is refreshed in the background by the root whenever it runs the Dependency Check with System voice selected. It is held in the root, not persisted, and merged into `AppState` and the panel, like `dependencies`.

**Never:**
- No linking to eSpeak NG (GPL-3.0) and no `espeak` crate.
- No macOS; Windows is Story 3.13. On non-Linux, System voice is listed but its capability row says it arrives in a later release (the fal.ai precedent), and no engine is built.
- No change to the ONNX engine's behaviour, the restart logic, or AD-8's allowlist. `resolve_backend` must not treat System voice as an ONNX target.
- No "Active" line work for System voice. It stays "not started yet", as DeepInfra's does.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Speak | System voice, `tr`, eSpeak present, no sample | `espeak-ng -v trk/tr -b 1 --stdout` with the text on stdin; 24 kHz buffer played | N/A |
| Several voices | Language `yue`, voice unset | Picker lists yue's two voices with the top-priority one selected; Speak uses it | N/A |
| Pick a voice | Confirm "Chinese (Cantonese, latin as Jyutping)" | `SetSpeechVoice(SystemVoice, Some("sit/yue-Latn-jyutping"))` once; the next Speak uses it | Save failure is shown inline |
| Change language | `yue` + `sit/yue-Latn-jyutping` → pick `tr` | Language saved, voice cleared; the picker is hidden (one voice) | N/A |
| Missing binary | `espeak-ng` not on PATH | Blocking "eSpeak NG" row with the distro command; the overlay is blocked (3.4) | N/A |
| Out of set | Language `xx`, or stored voice not of the language | Refused by name before the engine; placeholder + note in the tab | One notification |
| Engine failure | Exit ≠ 0 / >10 s / garbage stdout | Child killed on timeout; one notification naming eSpeak NG and the reason | N/A |
| Hostile text | Text starting with `-v` or with `;` / `$(…)` | Spoken literally; argv never contains it | N/A |

</frozen-after-approval>

## Code Map

- `crates/voice-me-tts-system-linux/` (new, workspace member) -- `SystemVoiceLinux` implementing `TtsPort` (`warm_up` no-op, `is_ready` true) and `pub fn list_voices() -> Result<Vec<SystemVoice>, VoiceMeError>`. Its deps are `voice-me-core`, `rubato` (copy `resample` from `voice-me-tts-remote/src/wav.rs:58-84`) and no hound; parse the RIFF by hand. Put the pure parsers (the voices text, the WAV bytes, the os-release command) in functions tested with fixtures.
- `crates/voice-me-core/src/ports.rs:110-162` -- `TtsPort::generate(text, reference_clip: Option<&Path>, language, voice: Option<&str>)`. Update the implementors: `voice-me-tts/src/lib.rs:228`, `voice-me-tts-remote/src/lib.rs:163`, the fakes in `speak.rs:307,609`, and `mic-spike.rs:167`. Cloning adapters return `NoReferenceVoiceSample` on `None`.
- `crates/voice-me-core/src/state.rs` -- `BackendSelection::SystemVoice`: `label`, `local_target() = None`, `is_cpu` false, a stock-voice predicate. It is listed by `backend_choices` after the local runtimes. Add `LanguageBackend::SystemVoice` (its set is dynamic, so give the static `speech_languages()` a sibling that takes `&[SystemVoice]`). Add `SystemVoice { id, language, name, priority }` plus helpers for the distinct languages (each labelled with its top-priority voice's name) and each language's voices, `SpeechLanguages.system_voice`, the stored voice, and `AppState.system_voices` (not persisted).
- `crates/voice-me-core/src/settings_store.rs` -- `system_voice` in `[speech_languages]`, a lenient `[speech_voices]` table, and `save_speech_voice`. The SystemVoice language save clears the voice. Add `SettingsStore::save_speech_voice` to `ports.rs` and the four fakes (see spec-3-11 Code Map).
- `crates/voice-me-core/src/speak.rs:104-160` -- the SystemVoice branch: validate the language and voice, skip the sample and disclosure checks, pass `None` and the voice id.
- `crates/voice-me-core/src/state.rs:567-592` -- `DependencyKind::SystemVoiceEngine`, speech-blocking. Its UI slug is in `voice-me-ui/src/dependencies.rs:551`.
- `crates/voice-me-deps/src/capability.rs:295` + `lib.rs:200-230` -- SystemVoice: on Linux, the eSpeak NG row (PATH lookup plus the os-release command); on other OSes, the `cannot_run` "later release" row. No engine rows (`local_target()` is `None`).
- `crates/voice-me-ui/src/backend.rs` -- the entry in the Local `Select`, the Stock voice tag, the language `Select` fed from the panel's `system_voices`, and a voice `Select` (`backend-speech-voice`, `-select`, `-note`, `-error`) shown when the language has more than one voice. Add `BackendAction::SetSpeechVoice(LanguageBackend, Option<String>)` and `BackendArea::SpeechVoice`, plus `system_voices` on the panel.
- `crates/voice-me-app/src/main.rs:328-500,1744-1800,2039` -- `build_engine`: SystemVoice → the Linux adapter under `cfg(target_os = "linux")`, `unavailable` otherwise. `check_request`, `resolve_backend` → CPU placeholder. In `run_check`, list the voices in the background when System voice is selected, store them, and push the panel. `current_state` merges the list in. Handle the `SetSpeechVoice` arm. Add the crate as a Linux-only dependency in `voice-me-app/Cargo.toml`.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-tts-system-linux/` -- the crate, the adapter and `list_voices`. Unit tests: parsing a captured `--voices` fixture (variant skipped, ids and names); decoding a streaming-header WAV and checking its length ≈ ×24000/22050; the os-release command per distro; argv never carries the text. Add an `#[ignore]`-free integration test that skips itself when `espeak-ng` is absent and otherwise generates "merhaba" at 24 kHz. Also test the deadline with a fake binary via an overridable program path (a test-only constructor).
- [x] `crates/voice-me-core/src/{ports,state,settings_store,speak}.rs` + `lib.rs` -- the port change, types, persistence and speak branch. Tests: the matrix rows Speak, Several voices, Change language and Out of set, at the core level; the settings round trip for language and voice; the cloning backends still require the sample.
- [x] `crates/voice-me-tts/src/lib.rs`, `crates/voice-me-tts-remote/src/lib.rs`, `mic-spike.rs` -- the signature change only.
- [x] `crates/voice-me-deps/src/{capability,lib}.rs` -- the row. Tests: missing → blocking with manual steps; present → ready; non-Linux → cannot run (cfg-gated).
- [x] `crates/voice-me-ui/src/{backend,dependencies}.rs` + fakes -- the entry, tag, language and voice pickers, and notes. Tests: several voices show the picker with the default selected; a single voice hides it; a pick sends once; a language change leaves only the language action.
- [x] `crates/voice-me-app/{Cargo.toml,src/main.rs}` + `Cargo.toml` (workspace) -- the wiring.

**Acceptance Criteria:**
- Given System voice selected on this Linux machine with no Reference Voice Sample, when I type a line, then it plays through the Virtual Microphone at 24 kHz within about a second.
- Given `cargo test --workspace` and `cargo check --workspace --all-targets`, then all pass, and `voice-me-tests`' egress allowlist is unchanged and green.

## Implementation Notes

- Core: `BackendSelection::SystemVoice` (label "System voice — instant (stock voice)", `local_target() = None`, `is_stock_voice()`), `LanguageBackend::SystemVoice` with `language_options(&[SystemVoice])` / `resolve_language(saved, &[SystemVoice])` as the dynamic siblings of the static set, `SystemVoice`, `system_voice_languages`, `system_voices_of`, `resolve_system_voice` + `SystemVoiceRefusal`, `SpeechLanguages.system_voice`, `SpeechVoices` on `AppState.speech_voices`, `AppState.system_voices`, `DependencyKind::SystemVoiceEngine`. System voice language codes match case-insensitively after trimming (eSpeak has `chr-US-Qaaa-x-west`). An empty voice list is refused as its own message ("no voices listed yet").
- Settings: `[backend_selection] kind = "system_voice"`, `system_voice` in `[speech_languages]`, lenient `[speech_voices]`. `save_speech_language` clears the voice only when the code actually changes.
- Crate: `voice-me-tts-system-linux` (voices parser, hand-rolled RIFF parser that reads `data` to EOF, os-release command, bounded runner). Failures are `VoiceMeError::SpeechEngine("eSpeak NG …")`. `--voices` runs with a null stdin. `voice-me-deps` depends on it (Linux only) for `find_program` / `install_step`, following the `voice-me-audio-linux` precedent.
- UI: Stock voice tag `backend-stock-voice`; voice picker also shown for a one-voice language when the stored voice is not that language's, so it can be fixed. An empty list shows a "not listed yet" language note.
- App: `refresh_system_voices` runs from `run_check` and the startup check when the System voice is saved; a list that fails to read becomes empty.
- Verified: the listed `cargo test` crates (plus `voice-me-tts`) pass; `cargo check --workspace --all-targets` clean; clippy has no new warnings (the pre-existing `type_complexity` on `apply_selection` and `assert!(true)` in `voice-me-tests` remain); `cargo fmt --check` clean. Manual app checks not run.

## Spec Change Log

## Review Triage Log

## Verification

**Commands:**
- `cargo test -p voice-me-core -p voice-me-ui -p voice-me-deps -p voice-me-tts-system-linux -p voice-me-tts-remote -p voice-me-tests -p voice-me-app` -- expected: all pass
- `cargo check --workspace --all-targets && cargo clippy --workspace --all-targets` -- expected: no new warnings
- `cargo fmt --check` -- expected: clean

**Manual checks:**
- Run the app and select Local → System voice. Speak Turkish, then English (America). Pick Cantonese and switch between its two voices. Rename `espeak-ng` out of PATH (or run with an empty PATH) and confirm that the Dependencies row blocks the overlay.
