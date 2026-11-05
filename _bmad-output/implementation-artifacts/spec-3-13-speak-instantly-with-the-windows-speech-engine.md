---
title: 'Speak instantly with the Windows speech engine'
type: 'feature'
created: '2026-09-24'
status: 'done'
baseline_commit: '9df131861232e5e8cbeb0cf50f286252feacee84'
route: 'dispatch'
review_loop_iteration: 1
context:
  - '{project-root}/_bmad-output/implementation-artifacts/epic-3-context.md'
  - '{project-root}/_bmad-output/implementation-artifacts/spec-3-12-speak-instantly-with-espeak-ng-on-linux.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** On Windows, Local → System voice is listed, but it only answers "arrives in a later release". The user wants it to speak through the Windows speech engine, with its installed voices (e.g. Microsoft Tolga for Turkish).

**Approach:** Add a new crate, `voice-me-tts-system-windows`. It implements `TtsPort` with WinRT `Windows.Media.SpeechSynthesis.SpeechSynthesizer` through the `windows` crate:
- `SynthesizeTextToStreamAsync` renders into a stream, never to the speaker.
- The WAV bytes are decoded, downmixed and resampled to 24 kHz mono f32 (AD-11).
- The voice list comes from `SpeechSynthesizer::AllVoices`.
- The composition root and the Dependency Check wire it in the same way Linux's eSpeak NG system voice is wired.

**Decisions (user delegated all choices, 2026-09-24):**
1. **Language codes.** Windows voices report BCP-47 languages (`tr-TR`), while the saved default is `tr`. Matching a stored language to a voice language falls back to the primary subtag (`tr` ↔ `tr-TR`) when there is no exact match. This applies to every stock-voice list, so Linux's eSpeak codes behave exactly as before.
2. **No installed voice for the selected language.** This reuses the existing Backend-tab speech-language error, with Windows' steps (Settings → Time & language → Speech → Add voices). Speak stays blocked through the existing stock-voice resolution. There is no new `DependencyKind`, and `SystemVoiceEngine` is never used for it, because that kind's Install downloads eSpeak NG on Windows.
3. **No Windows speech engine at all** (a WinRT call fails, or no voices are installed). The System voice's Dependency Check gets a manual, speech-blocking `BackendCapability` row naming Windows' steps. There is no Install button.

## Boundaries & Constraints

**Always:**
- Every WinRT call runs on the crate's own thread, which initialises its apartment (multithreaded) and blocks on the async operations. Never on GPUI's thread.
- The generate path has a deadline (60 s). A failure is one `VoiceMeError::SpeechEngine` naming "Windows speech".
- WAV decoding, downmix, resampling and the voice mapping are pure modules compiled on every OS and unit-tested on Linux CI. The WinRT module is `cfg(target_os = "windows")`.
- It is a stock voice: `reference_clip` is ignored, it opens no socket, and it needs no disclosure.

**Never:**
- No SAPI `SpVoice::Speak`, and no playback to the speaker.
- No change to Linux behaviour or to Piper's eSpeak NG row.
- No new crate depends on `voice-me-ui` (AD-1/AD-2).

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Speak Turkish | Saved language `tr`, Tolga (`tr-TR`) installed | Tolga speaks. 24 kHz mono f32 buffer | — |
| Voice list | `AllVoices` returns Tolga, David, Zira | `StockVoice { id: voice Id, language: "tr-TR"/"en-US", language_label: DisplayName, name: DisplayName, priority }`. Languages `tr-TR`, `en-US` | — |
| Language with no voice | Saved `de`, no German voice | Speech-language error naming Windows' Add-voices steps; Speak blocked | Existing stock-voice error path |
| No engine | WinRT fails or zero voices | Blocking manual row naming Windows' steps | — |
| WAV decode | 16-bit PCM 22.05 kHz mono; stereo; unexpected format | Resampled to 24 kHz; downmixed; error | `SpeechEngine("Windows speech …")` |
| Empty text | `""` | `VoiceMeError::EmptyText` | — |

</frozen-after-approval>

## Code Map

- `crates/voice-me-tts-system-linux/src/{lib,wav,voices}.rs` -- the model. `wav.rs`'s `decode_wav`/`downmix`/`resample` are pure but crate-private. Copy them into the new crate: no shared crate exists, and AD-2 keeps adapters separate. Extend the copy to accept 32-bit float (tag 3) and 24/32-bit PCM besides 16-bit.
- `crates/voice-me-core/src/ports.rs:125` -- `TtsPort::{warm_up, is_ready, generate(text, reference_clip, language, voice)}`.
- `crates/voice-me-core/src/state.rs:313` (`resolve_language`, exact case-insensitive match), `:352` (`StockVoice`), `:373` (`stock_voice_languages`), `:649-821` (`BackendSelection::SystemVoice`, listed on every OS). Decision 1's primary-subtag fallback goes in the language matching used by stock voices.
- `crates/voice-me-core/src/speak.rs:137-150` -- `resolve_stock_voice` then `generate(text, None, &voice.language, Some(&voice.id))`. Unchanged.
- `crates/voice-me-deps/src/capability.rs:359-367` + test `:896-901` -- the SystemVoice `cannot_run` "later release" off Linux. On Windows it becomes a Ready row, or decision 3's manual row. `lib.rs:440` + `:998-1015` `system_voice_rows`. Do NOT use `DependencyKind::SystemVoiceEngine` on Windows (`lib.rs:223` routes its Install to the eSpeak MSI).
- `crates/voice-me-app/src/main.rs:562-610` `build_engine` SystemVoice arm (add the Windows adapter). `:2906-2950` `refresh_system_voices` (add a Windows branch listing voices in the background, and decision 2's error text). Tests `:2170-2186`, `:2314-2342` assert "unavailable" off Linux; update them for Windows. `voice-me-app/Cargo.toml:45-54`, the Windows target deps.
- `.github/workflows/ci.yml` -- the Windows job runs `cargo build/test --workspace`. That is the only compile of the WinRT module.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-tts-system-windows/` (new workspace member) -- `Cargo.toml`:
  - `voice-me-core` and `rubato = "5.0.0"`, unconditional;
  - `windows = { version = "0.62", features = ["Foundation", "Foundation_Collections", "Media_SpeechSynthesis", "Storage_Streams", "Win32_System_WinRT"] }` under `cfg(target_os = "windows")`.

  Modules:
  - `wav.rs`: the copied, extended decoder. Tests: 16-bit/22.05 kHz, float, stereo, garbage.
  - `voices.rs`: a pure `to_stock_voices(Vec<RawVoice { id, display_name, language }>) -> Vec<StockVoice>`. Priority is list order; the default voice comes first. Tests.
  - `engine.rs`: `cfg(windows)`. `list_voices()`, `SystemVoiceWindows` implementing `TtsPort`, the worker thread with its MTA and deadline, and reading the stream into bytes with `DataReader`.
  - `lib.rs`: re-exports and `ENGINE_LABEL = "Windows speech"`.
- [x] `crates/voice-me-core/src/state.rs` -- decision 1: the primary-subtag fallback when matching a stored language against a stock-voice list. Tests: `tr` → `tr-TR`; an exact match wins; eSpeak `en-gb` is unchanged.
- [x] `crates/voice-me-deps/src/{capability,lib}.rs` -- on Windows, the SystemVoice rows: Ready when the app reports the engine usable, otherwise decision 3's row. The simplest fit is a Windows `system_voice_rows` that calls a probe function injected like the other probes. Update the old test.
- [x] `crates/voice-me-app/{Cargo.toml,src/main.rs}` -- the Windows dependency; the `build_engine` arm; the `refresh_system_voices` Windows branch; decision 2's text; updated tests.
- [x] `README.md` -- the Windows System voice, and how to add voices.

**Acceptance Criteria:**
- Given Windows with Tolga installed and System voice selected, when the user speaks Turkish, then Tolga's voice plays through voice-me's audio path, never through the speaker directly.
- Given Linux, when the System voice is used, then behaviour is unchanged (all existing tests pass).
- Given CI, then the Windows job compiles the WinRT module and all unit tests pass on both OSes.

## Implementation Notes

- Crate `voice-me-tts-system-windows`: `wav.rs` (the copied decoder, extended to 16/24/32-bit PCM, 32-bit float, `WAVE_FORMAT_EXTENSIBLE`; a declared `data` size that fits is honoured, one past the end is read to EOF), `voices.rs` (`RawVoice`, `default_first`, `to_stock_voices`), `engine.rs` (Windows only: `list_voices`, `count_voices`, `SystemVoiceWindows`; one worker thread per call with `RoInitialize(RO_INIT_MULTITHREADED)`, `join()` on the async ops, `DataReader` into bytes, 60 s `recv_timeout`), `lib.rs` (`ENGINE_LABEL`, `DEADLINE`, `ADD_VOICES_STEP`). **Deviation:** the `windows` features also need `Media_Core` — `SynthesizeTextToStreamAsync` is gated on it in `windows` 0.62.
- Core: `resolve_language` falls back, with no exact match, to a listed tag sharing the primary subtag when one side is a bare subtag (`tr` ↔ `tr-TR`; `en-gb` never matches `en-us`). First listed wins when several share it.
- Deps: `capability::windows_system_voice_row(Result<usize, String>)` — Ready `BackendCapability` row, or `cannot_run` with Windows' steps in the detail (the Dependencies tab does not render manual steps for this kind) and in `manual_steps`, not automatable. `DepsAdapter::with_system_voice_probe`; the app injects `voice_me_tts_system_windows::count_voices`. `capability_row` returns `None` for SystemVoice on Windows.
- App: `build_engine` Windows arm; `refresh_system_voices` Windows branch through the pure `apply_windows_voice_listing` / `windows_voice_missing_note` (decision 2's text, tested on Linux).
- Verified: `cargo test` for `voice-me-tts-system-windows`, `voice-me-core`, `voice-me-deps`, `voice-me-app` pass on Linux; clippy clean for the new crate, core and deps. `cargo check --target x86_64-pc-windows-gnu -p voice-me-tts-system-windows` cannot run here (voice-me-core pulls gpui, whose build script needs `x86_64-w64-mingw32-windres`), so the crate's sources (lib and tests) were type-checked and clippy-clean for the Windows target in a scratch crate against a stub `voice-me-core`. The Windows cfg branches of `voice-me-deps` and `voice-me-app` were not compiled here — CI's Windows job is their first compile. `voice-me-ui` tests were not re-run (disk).

- Review fixes (2026-09-24): the Windows listing clears/sets only its own prefixed messages (`WINDOWS_VOICE_MISSING`, `WINDOWS_VOICE_LIST_ERROR`) and never overwrites a failed language save; a generation counter plus a saved-selection/language check drops late listings; a blank language gets no note; the Add-voices steps attach only to `Ok(0)` (an `Err` names Windows speech and the reason); `WINDOWS_SPEECH_LABEL`, `NO_SYSTEM_VOICE_PROBE` and `WINDOWS_ADD_VOICES_STEPS` in deps are the single source (the app uses the steps; the crate's `ADD_VOICES_STEP` is gone); listing/counting use a 15 s `LIST_DEADLINE`; the voice lookup trims both ids; a `fmt ` chunk under 16 bytes is "cut short"; the core fallback reads `_` and `-` alike; `tests/winrt.rs` runs the live engine on Windows; `deps_adapter()` is extracted with a Windows test that the probe is wired.

## Spec Change Log

## Review Triage Log

Pass 1: blind hunter (B), edge-case hunter (E), verification gap (V).

| Verdict | Finding | Evidence / route |
|---|---|---|
| medium | The Windows listing clears or overwrites an unrelated SpeechLanguage error, such as a failed save (B, E, V) | `apply_windows_voice_listing` calls remove/insert unconditionally; `SetSpeechLanguage` then `run_check` → patch (Edge TTS prefix pattern plus test) |
| medium | A late listing is applied after the backend or language changed (B, E) | No generation guard, unlike Edge TTS → patch |
| medium | The live WinRT path is never run by a test (B, V) | Only pure and early-return tests exist → patch, Windows-only `tests/winrt.rs` that skips without voices |
| medium | The app's Windows probe wiring is untested; losing it blocks Speak (V) | Only the deps test injects a probe → patch, extracted builder plus Windows test |
| low | The Add-voices steps are shown on engine errors and timeouts; the default probe's text is unclear (B) | Real, and a direct fix → patch |
| low | The Dependency Check can block for 60 s on a hung WinRT call (B) | `count_voices` uses DEADLINE → patch, 15 s listing deadline |
| low | A blank language gives `speaks ""` (B, E) | No guard → patch |
| low | Voice id trimmed on list but not on lookup (E) | Direct fix → patch |
| low | A `fmt ` chunk shorter than 16 bytes is read past its end (B, E) | `size.max(16)` → patch |
| low | The Add-voices text is written in 3 places plus the README; a constant is unused (B) | Direct correction → patch, one deps constant |
| low | `primary_subtag` claims `_` support but the exact step compares whole strings (B) | Direct fix → patch, `_` and `-` treated alike |
| low | The fallback now resolves bare codes that other stock-voice backends used to refuse (B, E) | Decision 1 applies it to every stock list. Only codes that were refused before change → rejected (intended by the decision) |
| low | Unbounded detached worker threads after WinRT timeouts (B, E) | Needs a stuck engine plus repeated presses → rejected (adds a worker pool) |
| low | A fixed 60 s deadline may cut very long text (E) | Typed prompts are short → rejected |
| low | A `data` chunk before `fmt ` is refused (B, E) | WinRT writes `fmt ` first → rejected |
| low | `block_align` is not validated (B) | WinRT writes packed PCM → rejected |
| low | Two overlapping notes when the saved language has no voice (V) | Cosmetic → rejected |
| low | `generate` ignores `language` (B) | The voice id decides the voice, as on Linux → rejected |
| low | `wav.rs` is a copy that can diverge from Linux's (B) | Real → defer (shared decode crate) |
| low | Public-but-internal API, README omissions (B) | Cosmetic → rejected |

## Verification

**Commands:**
- `cargo test -p voice-me-tts-system-windows -p voice-me-core` (one crate per run) -- expected: pass
- `cargo test -p voice-me-deps`, `cargo test -p voice-me-app` -- expected: pass
- `cargo check --target x86_64-pc-windows-gnu -p voice-me-tts-system-windows` -- expected: clean (type-checks the WinRT module here)

**Manual checks:**
- On Windows: select System voice, speak Turkish with Tolga installed.
