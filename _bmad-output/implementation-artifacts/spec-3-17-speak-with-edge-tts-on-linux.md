---
title: 'Speak with Edge TTS on Linux (Story 3.17)'
type: 'feature'
created: '2026-09-24'
status: 'draft'
route: 'dispatch'
review_loop_iteration: 0
context: ['{project-root}/_bmad-output/implementation-artifacts/epic-3-context.md', '{project-root}/_bmad-output/planning-artifacts/sprint-change-proposal-2026-09-24-edge-tts.md']
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Microsoft's neural voices are available only through Azure, which needs a key. There is no free, keyless way to get them.

**Approach:** Add a **Remote → Edge TTS** stock-voice provider. A new Linux-gated crate `voice-me-tts-edge` runs the user-installed `edge-tts` program as a bounded child process. Edge TTS is always selectable. When the program is missing, a speech-blocking manual row asks the user to install it.

## Boundaries & Constraints

**Always:**
- `RemoteProvider::EdgeTts` has the label "Edge TTS", serde name `edge_tts`, `is_stock_voice` = true, and is listed after Azure. A new `RemoteProvider::needs_api_key()` is false only for EdgeTts. It gates the key capability row, the Backend key input, and `disclosure_needed`'s key guard.
- Selecting Edge TTS never checks for the program. Presence is checked by the Dependency Check.
- **Program lookup:** `edge-tts` on `PATH`, then `$HOME/.local/bin/edge-tts` (the file must be executable). Engine and row use the same resolved path.
- **Speaking:** argv is exactly `--voice=<id> -f - --write-media -`. The UTF-8 text goes on stdin. No shell, 30 s deadline, run through the existing bounded runner. stdout is MP3 (24 kHz mono); decode it with `symphonia` to f32. Resample only if the rate is not 24 000.
- **Voices:** from `--list-voices` (null stdin, 15 s deadline), parsed by a pure function.
  - Skip the header and the `---` rule. The first field is the id and the second is the gender.
  - Locale = the id without its last `-` segment. Name = that last segment with `MultilingualNeural` or `Neural` removed, plus " (Gender)".
  - Language label = the locale. Priority = list order.
  - Zero voices is an error.
- **Language and voice:** defaults are `tr-TR` and the language's first listed voice (`requires_voice` = false). Values are validated through `resolve_stock_voice` and refused by name. Persistence is `edge_tts` in `[speech_languages]`/`[speech_voices]`. The voice list is held in `AppState.edge_tts_voices`, not persisted, and refreshed in the background on each check when Edge TTS is selected and the program is found.
- **Disclosure:** before the first request, and before any process is spawned. It lists the typed text, the language and the voice. Its note says: "Edge TTS sends the text to Microsoft's Edge Read Aloud service. It is free, needs no account, and is not an official API: it may stop working at any time."
- **Row:** `DependencyKind::EdgeTtsProgram` blocks speech and has manual steps only. `provision` refuses it.
  - Ready: "Found at {path}."
  - Missing: title "edge-tts", detail "edge-tts is not installed. Please install it: `pipx install edge-tts`". The steps are the distro's pipx command from `/etc/os-release`, then "Press Check again.":
    - arch: `sudo pacman -S python-pipx`
    - debian/ubuntu: `sudo apt install pipx`
    - fedora: `sudo dnf install pipx`
    - opensuse: `sudo zypper install python3-pipx`
    - other: "Or: `pip install --user edge-tts`"
- **Off Linux:** Edge TTS is listed, and a `cannot_run` row says "Edge TTS isn't available on this OS yet." No engine is built.
- **Failures:** each failure is one `SpeechEngine("Edge TTS …")`:
  - exit ≠ 0: the last non-empty stderr line, capped at 200 chars;
  - timeout;
  - "returned no audio" when stdout is empty or cannot be decoded;
  - a spawn `NotFound`: "edge-tts is not installed. Please install it: pipx install edge-tts".

**Never:**
- No reqwest in the new crate, and no route through `voice-me-tts-remote`.
- No Install button, and voice-me never runs pip.
- No text on argv.
- No Windows engine. No rate, pitch or volume controls.
- No copy button. Dependencies has none today.
- The egress allowlist does not change.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected | Error handling |
|---|---|---|---|
| Not installed | Edge TTS selected, no program | Selection saved; blocking row; overlay blocked | N/A |
| pipx install | Program only in `~/.local/bin`, not on PATH | Row ready with that path; Speak works | N/A |
| Speak | `tr-TR`, voice unset, disclosure confirmed | `--voice=tr-TR-AhmetNeural`; 24 kHz buffer | N/A |
| No disclosure | Not yet confirmed | Refused before spawn | One notification |
| Offline | Exit 1, traceback | Nothing played | "Edge TTS …: ClientConnectorError: …" |
| Timeout | More than 30 s | Child killed and reaped | One notification |
| Hostile text | `--voice=x; $(rm …)` | Spoken literally; not on argv | N/A |
| Other OS | Windows | Listed; cannot-run row | N/A |

</frozen-after-approval>

## Code Map

- `crates/voice-me-tts-system-linux/src/lib.rs:17` -- make `process` public (`pub mod process;`) so the new crate reuses `process::{run, RunError}`. This is the one runner. 3.15 is moving it to `voice-me-espeak`; whichever story merges second switches the import.
- `crates/voice-me-tts-edge/` (new, `#![cfg(target_os="linux")]`):
  - Deps: core, `voice-me-tts-system-linux`, `symphonia` 0.6.1 with `mp3`, `rubato` only if a resample is kept (copy from system-linux `wav.rs`).
  - `find_program`, `speak_args`, `parse_voices`, `decode_mp3`, `list_voices`.
  - `EdgeTts: TtsPort` with `warm_up` no-op and `is_ready` true. A test-only constructor overrides the program path (the 3.12 pattern).
  - The pipx install step comes from the os-release parser. Reuse `install_command_for`'s distro detection by adding a sibling in this crate that reads `/etc/os-release` the same way.
  - Fixtures are already in `tests/fixtures/{list-voices-7.2.8.txt,tone.mp3}`.
- `crates/voice-me-core/src/state.rs`:
  - `RemoteProvider` :162, `ALL` :171, `label` :178, `is_stock_voice` :189.
  - `speech_languages` :228 goes in the empty arm; `has_voice_list` :236.
  - `SpeechLanguages` :514/:525/`get` :559 gets `edge_tts` (default `DEFAULT_EDGE_TTS_LOCALE = "tr-TR"`, re-exported in `lib.rs:27`). `SpeechVoices` :541/`get` :548.
  - `ApiKeys::get/set` :763-788: EdgeTts → None, and `set` is a no-op.
  - `AppState.edge_tts_voices` next to `azure_voices` :1133/:1158; `stock_voices` :1174.
  - `DependencyKind::EdgeTtsProgram` :893; `blocks_speech` :915.
- `crates/voice-me-core/src/speak.rs:135-190` -- an EdgeTts branch shaped like the SystemVoice one (resolve against `edge_tts_voices`), with the disclosure check before `generate(text, None, &lang, Some(&id))`.
- `crates/voice-me-core/src/settings_store.rs` -- `SpeechLanguagesFile` :140/`entry` :144/`to_state` :154, `SpeechVoicesFile` :193/`entry` :197, default :332, migration `get_or_insert_with` :416, `build_state` :480.
- `crates/voice-me-deps/src/capability.rs` -- skip the no-key refusal :309 when `!needs_api_key()`. Add an EdgeTts arm: None on Linux, `cannot_run` elsewhere (SystemVoice pattern :348). Add `edge_tts_program_row(found, steps)` beside `system_voice_engine_row` :368.
- `crates/voice-me-deps/src/lib.rs` -- `provision_row` :141 manual refusal; `engine_rows` :217 adds a `Remote(EdgeTts)` guard calling `edge_tts_rows()` (Linux, :578 pattern; empty elsewhere). `Cargo.toml` adds the crate Linux-only. Test beside :1163.
- `crates/voice-me-ui/src/backend.rs`:
  - `BackendPanel.edge_tts_voices` :152-212, `stock_voices` :184.
  - `BackendKind::label` :257 becomes "Remote — online providers".
  - Filter `key_inputs` :531 by `needs_api_key`.
  - `options_section` :927: for EdgeTts, language, voice and a stock-voice note; no key section.
  - `speech_language_note` :1485 gets an Edge "not listed yet" case; `provider_slug` :1535 gets `edge-tts`.
- `crates/voice-me-ui/src/prompt_overlay.rs:105` -- a `for_provider` EdgeTts arm, like Azure's items, with the note above.
- `crates/voice-me-ui/src/dependencies.rs:550` -- slug `edge-tts-program`.
- `crates/voice-me-app/src/main.rs`:
  - `check_request` :339: `has_api_key` true for keyless.
  - `build_engine` :456: an EdgeTts arm, Linux → `EdgeTts::new()`, else unavailable.
  - `disclosure_needed` :504: include EdgeTts with no key guard.
  - `current_state` :634 plus its callers, and `make_panel` :2043: merge `edge_tts_voices`.
  - New `refresh_edge_tts_voices`, modelled on `refresh_system_voices` :2096. Its error is "Couldn't list Edge TTS voices: …" in `SpeechLanguage`. It is called from `run_check` :2236 and startup :2829.
  - `Cargo.toml` :28 adds the crate Linux-only; workspace `Cargo.toml` adds the member.
- `.github/workflows/ci.yml:17-31` -- after apt, `pipx install edge-tts==7.2.8`.

## Tasks & Acceptance

**Execution:**
- [ ] `crates/voice-me-tts-edge/` + system-linux `pub mod process` + workspace member.
  - Tests: parse the fixture (header and rule skipped; `tr-TR-EmelNeural` → `tr-TR`, "Emel (Female)"; `zh-CN-liaoning-…`, `iu-Cans-CA-…`, `…MultilingualNeural`); zero voices → error.
  - Tests: argv excludes hostile text; `tone.mp3` decodes to ≈12 000 samples at 24 kHz.
  - Tests with a fake program: deadline, last stderr line, empty stdout, NotFound message; `find_program` via a temp `HOME` with an empty `PATH`; the os-release pipx step per distro.
  - `tests/edge_tts.rs`: skips when the program is absent. Otherwise it lists voices and asserts `tr-TR-EmelNeural`. Synthesis runs only with `VOICE_ME_EDGE_TTS_ONLINE=1`.
- [ ] `crates/voice-me-core/src/{state,speak,settings_store,lib}.rs`.
  - Tests: matrix rows Speak, No disclosure, and unlisted language/voice at core level; settings round trip; `needs_api_key`; `blocks_speech`.
- [ ] `crates/voice-me-deps/src/{capability,lib}.rs` + `Cargo.toml`.
  - Tests: missing → blocking manual row; present → ready; no key row for EdgeTts; `provision` refused; non-Linux cannot-run (cfg-gated); no eSpeak row.
- [ ] `crates/voice-me-ui/src/{backend,prompt_overlay,dependencies}.rs`.
  - Tests: no key input for Edge; the voice picker lists Edge voices with the default selected; the disclosure note; the slug.
- [ ] `crates/voice-me-app/{Cargo.toml,src/main.rs}`.
  - Tests: `build_engine` Edge arm (Linux); `check_request`; `disclosure_needed` with no key; the list error clears only its own error.
- [ ] `.github/workflows/ci.yml` -- pipx install.

**Acceptance Criteria:**
- Given a Linux machine without `edge-tts`, when I select Remote → Edge TTS, then it stays selected, Dependencies shows the blocking "please install it" row, and the overlay is blocked.
- Given `cargo test --workspace` and `cargo check --workspace --all-targets`, then all pass, and the egress allowlist is unchanged.

## Implementation Notes

## Spec Change Log

## Review Triage Log

## Verification

**Commands:**
- `cargo test -p voice-me-tts-edge -p voice-me-tts-system-linux -p voice-me-core -p voice-me-deps -p voice-me-ui -p voice-me-app -p voice-me-tests` -- expected: all pass
- `cargo check --workspace --all-targets && cargo clippy --workspace --all-targets` -- expected: no new warnings
- `cargo fmt --check` -- expected: clean

**Manual checks:**
- On the dev machine, select Edge TTS without the program: the row blocks. Run `pipx install edge-tts`, press Check, confirm the disclosure, and speak Turkish: it plays through the Virtual Microphone.
