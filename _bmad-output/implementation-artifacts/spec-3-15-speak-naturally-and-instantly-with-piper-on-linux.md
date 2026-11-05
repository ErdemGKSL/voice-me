---
title: 'Speak naturally and instantly with Piper on Linux, with a Piper voices tab (Story 3.15)'
type: 'feature'
created: '2026-09-24'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
baseline_commit: '45357e95e5a9702fef615a7ae553f0dbdd5a4fbb'
context: ['{project-root}/_bmad-output/implementation-artifacts/epic-3-context.md']
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** The local choices are Chatterbox, which uses your own voice but takes ~20 s a line and a 1.56 GB download, and eSpeak NG, which is instant but robotic. Nothing sits in between: a natural voice that is instant and small. There is also no way to browse and install extra voices, such as Turkish "fahrettin", or, later, voices Erdem trains himself.

**Approach:**
- Add a **Piper** local backend: a new `voice-me-tts-piper` crate runs a Piper VITS voice on the existing `ort`, CPU only, with phonemes from the `espeak-ng` program. The eSpeak runner moves into a shared `voice-me-espeak` crate.
- Add a **Settings → Piper voices** tab to browse three online catalogs and to download or delete voices.
- On Linux, when no backend is saved, Piper with **fahrettin** is the default.

## Boundaries & Constraints

**Always:**

**Engine (fixture-verified against Piper 1.8.0)**
1. Split the text into clauses at `, : ; . ! ?` followed by whitespace or the end of the text. A trailing unterminated clause counts too.
2. Phonemize each clause: `espeak-ng -q -v <config espeak.voice> --ipa`, text on stdin, run through `voice-me-espeak`. Join the output lines with a space, then strip `\([^)]+\)`.
3. Append the clause's terminator, plus a space after `, : ;`. NFD-decompose into codepoints.
4. `. ! ?` (or the end of the text) closes a sentence. For each sentence, the ids are: BOS `^`, PAD `_`, then each phoneme's ids followed by a PAD, then EOS `$`. Phonemes missing from the map are skipped.
5. Run each sentence with `input` [1, n] i64, `input_lengths` [n], `scales` [noise_scale, length_scale, noise_w] f32 from the config's `inference` block, and `sid` only when `num_speakers` > 1 (use speaker 0).
6. Peak-normalize each sentence (all zeros if the peak is below 1e-8), clip to ±1, concatenate, and resample from the config's `audio.sample_rate` to 24 kHz mono f32 (AD-11).
7. One held `ort` session per selected voice, rebuilt when the voice changes. Warm-up needs no Reference Voice Sample.

**Process and runtime**
- **`voice-me-espeak`** is the only crate that spawns `espeak-ng`: fixed name on PATH, stdin, no shell, 10 s deadline.
- System voice uses it too, and its behaviour is unchanged.
- The ONNX Runtime is the shared CPU library from the one committed runtime (`init_runtime`). Piper never downloads a runtime.

**Catalogs (merged; the first source wins on a duplicate voice key)**
1. **voice-me catalog:** `https://raw.githubusercontent.com/ErdemGKSL/voice-me/main/piper-voices/catalog.json`, fetched live. Each entry has key, name, locale, language label, quality, licence, and the model and config files, each with URL, size and SHA-256. Adding a voice later means editing this file; no release is needed.
2. **Official `rhasspy/piper-voices`:** `voices.json` at pinned revision `c10ece1aade47bb51c153c893d14e5bf8e5b7117`. Files are verified by the size and MD5 given there.
3. **speaches-ai:** the HF API list `models?author=speaches-ai&search=piper-`. Each repo `piper-<locale>-<name>-<quality>` holds `model.onnx` + `config.json`.
   - At download, `?blobs=true` gives the revision and the LFS SHA-256 of `model.onnx`. `config.json` is checked by size and git-blob SHA-1.
   - Files are downloaded at that revision.

**Default voice**
- Built in: `tr_TR-fahrettin-medium` from `speaches-ai/piper-tr_TR-fahrettin-medium` at `aab8f92429ede58091e17de506484a2c84384792`.
  - `model.onnx`: 63 201 294 B, SHA-256 `39081c47270180e8a0dfac69b07bf329fb6d039fcc1279dbe26c2daf2848b190`.
  - `config.json`: 5 022 B, SHA-256 `93741b234acc5f123430a0e343f619e01cfda6b1dc0970ee80bce7da22b1a09b`.
  - Licence: CC0 (NabuCasa dataset).
- The voice-me catalog lists the same entry.

**Storage and downloads**
- Voices are stored at `<cache>/piper/<key>/{model.onnx, config.json, voice.toml}`. `voice.toml` records name, locale, label, quality, source and licence.
- The installed list is read from disk. It supplies Piper's languages (the locale, labelled with the language name) and voices as `StockVoice`s.
- Downloads use deps' `.part` → verify → atomic rename, with progress. Only `voice-me-deps` touches the network. Catalog fetches happen only when the tab is opened or refreshed.

**Piper voices tab**
- Search/filter by language, then rows showing name, language, quality, size, source, licence ("see model card" when the source declares none), and status.
- Actions: Download (with a progress bar), Delete, and "Use" (sets Piper's language and voice).
- A failed catalog is one inline line; the other catalogs still show.

**Backend and Dependencies tabs**
- The Backend tab's Piper language and voice pickers list only installed voices, plus a "Manage voices" link to the tab.
- Dependencies rows for Piper:
  - shared ONNX Runtime (Install);
  - "No Piper voice installed" (Install = fahrettin), or the selected voice missing (Install = that voice when it is known);
  - the eSpeak NG row from 3.12 (manual steps).
- Each of these rows blocks speech (3.4).

**Default selection**
- When no backend is saved: Linux uses Piper and fahrettin (`tr_TR`); other OSes keep bundled CPU.
- An explicit selection is never rewritten.
- A Piper user with no sample gets no startup auto-open.

**Presentation**
- "Piper — natural, instant (stock voice)" is listed first under Local, with the Stock voice tag.
- Piper never sends the sample, never shows a disclosure, and opens no socket.
- A failure is one notification naming Piper and the reason.

**Never:**
- No linking of libespeak-ng, `piper1-gpl` or `piper-phonemize`.
- No reqwest, and no process spawning, outside the crates allowed to use them. No hf-hub crate.
- No GPU path, no `phoneme_silence`, no speaker picker for multi-speaker voices, no Windows engine (3.16: listed, with a "later release" row).
- No mirroring of voices, and no automatic catalog fetch at startup.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| First run | No settings, Linux | Piper + fahrettin selected; rows: runtime, "No Piper voice installed" (Install fahrettin), eSpeak | Overlay blocked until ready |
| Speak | fahrettin installed, "Merhaba Erdem, bu yerel ve anında." | Fixture ids; 24 kHz buffer played | N/A |
| Browse | Open tab | Rows from the three catalogs, deduplicated, filterable; installed rows marked | Failed catalog: one inline line, the others shown |
| Download | Download `tr_TR-dfki-medium` | Progress, then verified and moved into place; it appears in the Backend pickers | Bad hash: `.part` removed, row error |
| Delete | Delete the voice in use | Files removed; the selected voice becomes missing → blocking row with Install | N/A |
| Missing phoneme | Id map lacks a symbol | Symbol skipped, speech continues | N/A |
| eSpeak absent | Not on PATH | Blocking eSpeak row | Speak refused by name |

</frozen-after-approval>

## Code Map

- `crates/voice-me-espeak/` (new) -- move `voice-me-tts-system-linux/src/process.rs` (the `run` bounded runner and `RunError`), `find_program`/`PROGRAM`/`DEADLINE`, `os_release.rs` (`install_step`), and their process tests (the fake-`/bin/sh` + `FAKE_PROGRAMS` pattern). Add `pub fn ipa(program, voice, clause) -> Result<String, RunError>`. Crate is `#![cfg(target_os="linux")]`, depends on core only.
- `crates/voice-me-tts-system-linux/` -- use `voice-me-espeak`; keep its public re-exports (`find_program`, `install_step`) so `voice-me-deps` and the app keep compiling. Behaviour is unchanged.
- `crates/voice-me-tts-piper/` (new) -- pure `phonemes::{split_clauses, sentence_phonemes, to_ids}` (test against `tests/fixtures/piper_ids.json`: 3 captured cases with raw per-clause espeak output → expected ids); `config.rs` (serde for `phoneme_id_map`, `audio.sample_rate`, `espeak.voice`, `inference`, `num_speakers`); `PiperTts: TtsPort` (a `SessionSlot`-like holder keyed by voice id, copying `voice-me-tts/src/lib.rs:42-102`; tensor idioms from `voice-me-tts/src/generate.rs:143-171`; session build as `sessions.rs:305-349` with the CPU EP; errors → `VoiceMeError::SpeechEngine("Piper …")`/`MissingRuntimeAsset`). It takes an injected `runtime_init: Arc<dyn Fn() -> Result<(), VoiceMeError> + Send + Sync>` (AD-1: no dependency on `voice-me-tts`). `ort` as in `voice-me-tts/Cargo.toml:21-31,59-74`, but features `std, ndarray` only and a default `dynamic-runtime` feature; `ndarray 0.17`, `rubato 5` (copy `resample` from `voice-me-tts-system-linux/src/wav.rs:87-106`), `serde_json`, `unicode-normalization`, `regex`-free. Engine code is cross-platform; espeak use is Linux-gated.
- `crates/voice-me-core/src/assets.rs` -- `piper_dir(root)`, `piper_voice_files(root, key)`, `installed_piper_voices(root) -> Vec<InstalledPiperVoice>` (reads `voice.toml`), `PIPER_DEFAULT_VOICE` (key, locale).
- `crates/voice-me-core/src/state.rs` -- `BackendSelection::Piper` (`:577`; `Default` `:593` cfg Linux; `local_target` None; `is_stock_voice` true; `is_system_voice` false; `language_backend`, `label`, `backend_choices` `:733` first among Local). `LanguageBackend::Piper` (`has_voice_list` true, `requires_voice` false). `SpeechLanguages.piper` (default `tr_TR`), `SpeechVoices.piper` (default fahrettin key), `AppState.piper_voices` (not persisted), `stock_voices` arm. `DependencyKind::PiperVoice` (speech-blocking). `DependencyKind` is `Copy` and the provision key, so the voice to install travels in the request (below).
- `crates/voice-me-core/src/settings_store.rs` -- `SelectionFile::Piper` ("piper", `:224-259`); `piper` in `SpeechLanguagesFile`/`SpeechVoicesFile`; migration defaults. Fix test `:1005` for the cfg default.
- `crates/voice-me-core/src/speak.rs:135` -- a Piper branch like SystemVoice's, using `stock_voices(Piper)`.
- `crates/voice-me-core/src/ports.rs` -- `CheckRequest.piper_voice: Option<String>`; `DependencyProvisioningPort::provision` gains the `CheckRequest` (or a `ProvisionTarget` carrying the voice). A new `PiperCatalogPort`: `fetch_catalog() -> Vec<CatalogResult>`, `install(entry, events)`, `delete(key)`. Update the fakes (UI settings/hotkey/voice_setup, app main).
- `crates/voice-me-deps/src/piper.rs` (new) -- the three catalog parsers (pure, fixture-tested), dedupe (`voice-me > official > speaches`), the built-in fahrettin `Asset`s, the installer (reuses `fetch`/`provision::download`/`verify`; extend `verify` to MD5 and git-blob SHA-1 via `md-5`/`sha1` crates) that writes `voice.toml`. `lib.rs:205` gets a Piper arm with `runtime_row` (`:370`), `piper_voice_row` and `system_voice_rows` (`:579`, via `voice-me-espeak`). `capability.rs:296` gets a Piper arm (None on Linux, `cannot_run` "later release" elsewhere). Parameterise `system_voice_engine_row`'s detail so it mentions Piper.
- `crates/voice-me-ui/src/piper_voices.rs` (new tab) and `settings.rs` tab list -- a gpui-kit list (read the `gpui-kit` Coding Guides and `gpui-kit-design-guides` first); actions via a `PiperVoicesAction` enum handled by the root. `backend.rs` -- `BackendKind::of` `:240`, `BackendPanel.piper_voices` + `stock_voices` `:184`, the "not listed yet" note `:1485` for Piper ("No Piper voice installed — Manage voices"), the "Manage voices" button. `dependencies.rs:556` gets the slug `piper-voice`.
- `crates/voice-me-app/src/main.rs` -- `check_request` `:339` (+ the `has_api_key` arm); `selection_library` `:359` returns the bundled dylib for Piper; `build_engine` `:429` Piper branch (Linux: `PiperTts::new(cache, espeak program, runtime_init = || voice_me_tts::sessions::init_runtime(resolve_runtime_dylib(root, None)))`); `should_warm_up` `:528` allows Piper without a sample; `current_state`/`make_panel` merge `installed_piper_voices`; the root handles Piper tab actions (fetch in `background_spawn`, install with progress events, delete then re-check). Cargo: `voice-me-tts-piper` as a dependency, `voice-me-espeak` Linux-only.
- `piper-voices/catalog.json` (new, repo root) -- the fahrettin entry plus a short `README.md` describing the format for custom voices.
- Planning docs -- the 2026-09-24 proposal (addendum: the tab, the three catalogs, fahrettin default; P7 reversed), `epics.md` 3.15 ACs, `EXPERIENCE.md` Piper voices tab row, `ARCHITECTURE-SPINE.md` AD-7 Piper row wording.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-espeak/` + `voice-me-tts-system-linux` -- extract the runner; existing tests pass unchanged or move.
- [x] `crates/voice-me-tts-piper/` -- phonemes, config and engine. Tests: fixture ids (all 3 cases), clause splitting (`1,500` stays one clause, trailing text without a terminator), skipped symbols, normalisation, `sid` only when multi-speaker. A real-engine test that skips itself when `espeak-ng`, `ORT_DYLIB_PATH` or `VOICE_ME_PIPER_TEST_VOICE` is absent.
- [x] `crates/voice-me-core/src/{assets,state,settings_store,speak,ports}.rs` -- types, default, persistence, speak branch. Tests: the matrix rows First run and Speak at core level; settings round trip; other OSes keep the bundled CPU default.
- [x] `crates/voice-me-deps/src/{piper,lib,capability,sources,provision}.rs` + `Cargo.toml` -- catalogs, installer, rows. Tests: each parser from a captured fixture; dedupe order; hash mismatch removes the `.part`; row states (none installed / selected missing / ready).
- [x] `crates/voice-me-ui/src/{piper_voices,settings,backend,dependencies,lib}.rs` -- the tab and pickers. Tests: filter; an installed row shows Delete/Use; a download shows progress; "Use" sends one action; the Backend Piper pickers list installed voices only.
- [x] `crates/voice-me-app/{Cargo.toml,src/main.rs}` + workspace `Cargo.toml` -- wiring. Tests for `check_request`, `selection_library` and `build_engine` Piper arms.
- [x] `piper-voices/{catalog.json,README.md}` and the planning docs above.

**Acceptance Criteria:**
- Given a fresh Linux profile with `espeak-ng` present, when I click Install on the runtime and fahrettin rows and type a Turkish line, then it plays through the Virtual Microphone in fahrettin's voice within about a second.
- Given the Piper voices tab, when I download `tr_TR-dfki-medium` and press Use, then the next line uses dfki.
- Given `cargo test --workspace` and `cargo check --workspace --all-targets`, then all pass, and `voice-me-tests`' egress allowlist is unchanged and green.

## Implementation Notes

- Post-implementation fix (step 3 audit): a tab Download of fahrettin failed with "not in the catalog any more" whenever voice-me's own catalog was unreachable, since the tab then lists fahrettin under speaches-ai but the lookup returned the built-in entry, whose source is voice-me. `piper::voice_for_entry` now looks up by key and source and falls back to the built-in default by key. Test: `a_download_finds_its_voice_by_key_and_source_and_the_default_always`.
- Verification here: every listed crate's tests pass (espeak 11, tts-piper 19, tts-system-linux 14, core 95, deps 80, ui 112, app 44, tests 11). The disk filled up, so the user approved deleting `target/debug/examples` and the stale incremental caches (no `cargo clean`, no dependency rebuild).
- Review patches (pass 1):
  - A named voice missing from an empty cache now asks for that voice, not fahrettin, in both the row and the Install.
  - The tab's "in use" follows the real backend selection.
  - Non-espeak `phoneme_type` voices are refused with a reason.
  - The language filter compares codes, not counts.
  - Use goes through `use_piper_voice`, which saves the language before the voice.
  - Tests added: the catalog port (list → install → exactly one Finished), Install forwarding the Piper voice, `current_state` merging installed voices, and "Manage voices" switching the tab.
  - The app tests that read the cache root share one lock, so the env-setting test cannot race the comparison test.
- Verified after the patches:
  - Tests pass: espeak 11, tts-piper 20, tts-system-linux 14, core 95, deps 82, ui 114, app 46, tests 11.
  - `cargo fmt --check` is clean.
  - Clippy shows only the two existing warnings (`type_complexity` on `apply_selection`, `assert!(true)` in `voice-me-tests`).
  - The manual app checks were not run (no display in this session).

## Spec Change Log

## Review Triage Log

Pass 1 (blind-hunter, edge-case-hunter, verification-gap).

| Verdict | Finding | Evidence |
|---|---|---|
| false | Existing Linux profiles with no saved backend move to Piper (blind) | That is the approved intent: the frozen block says "When no backend is saved: Linux uses Piper", and the proposal names the consequence → rejected |
| medium | Deleting the only installed voice (the selected one, e.g. dfki) gives "No Piper voice installed — Install fahrettin". Install fetches fahrettin and the row stays blocked (blind, edge ×2) | Confirmed: `piper_voice_row` and `provision_piper_voice` both check "nothing installed" before looking at the named voice. The matrix Delete row wants that voice's Install → patch |
| low | Row Install and tab Download can fetch the same key at once, using separate in-flight sets (blind, edge) | Real, but it needs two clicks on two tabs. A clash fails the hash check and is removed, never installed. The fix adds a shared guard → rejected |
| false | Delete can run during a download (blind) | Delete is only offered on installed rows (those with `voice.toml`, written last). A downloading voice has no manifest, so it is not installed → rejected |
| low | `install_voice` keeps an existing file without re-checking it (blind, edge) | A file only takes its final name after verification. A mismatch needs the same key installed from two sources with an interruption in between. Rare, and the fix adds hashing → rejected |
| medium | The tab shows "in use", and Use looks like it works, while another backend is selected (blind) | Confirmed: `make_piper_panel` forces `BackendSelection::Piper` before `piper_voice_for` → patch (status reflects the real selection) |
| low | After Use, warm-up still targets the old voice; the first line pays ~1 s (blind) | Real, but it costs about one second once per switch. The fix rebuilds or re-warms the engine → rejected |
| low | `normalize` lets NaN through (blind, edge) | Real only if the graph ever outputs NaN, which has not been seen. The fix adds a branch → rejected |
| false | The speaches-ai list is paginated and catalog fetches are sequential (blind) | `curl` without `limit` returns all 124 repositories and no `Link` header. Sequential fetches are a latency nicety → rejected |
| false | Download progress rescans the disk with no throttle (blind) | `ProgressReporter` throttles to `PROGRESS_INTERVAL` (100 ms). Each rescan reads a handful of small files → rejected |
| low | The language filter rebuilds only when the number of languages changes (blind, edge) | Confirmed. Fixed by comparing codes instead of counts (direct correction) → patch |
| low | The filtered locale can vanish and leave "No voices match" (edge) | Rare (it needs the last voice of the filtered locale to go), and the fix adds a branch → rejected |
| maybe-false | Fixed 64 px rows may clip the progress or error line (blind) | Not rendered here. It would only be cosmetic (low) → rejected. A manual check of the tab settles it |
| low | Clause splitting ignores quotes, `…`, and has no length cap (blind) | eSpeak still phonemizes such text; only a pause is lost. The overlay takes single short lines → rejected |
| false | Spec and sprint status disagree; the note about deleting target files (blind) | Sprint status moves at step 5. The deletion was approved by the user and is recorded as a fact → rejected |
| low | A saved voice whose locale is not the saved language reads Ready (edge) | Only reachable by hand-editing; Use and the pickers save matching pairs, and Speak refuses it by name → rejected |
| medium | A voice whose `phoneme_type` is not espeak installs, then fails or speaks garbage (edge) | Confirmed: `PiperConfig` ignores `phoneme_type`, and the official catalog has non-espeak voices → patch (refuse with a reason) |
| false | A cleared Piper voice is re-seeded to fahrettin (edge) | Any save writes the whole file, including `speech_languages.piper`, so the seeding branch never runs again → rejected |
| medium | The `PiperCatalogPort` adapter methods (fetch → install → Finished) are untested (verification-gap) | Pre-verified → patch |
| medium | The real engine (`ensure_voice` rebuild, `run_graph` inputs) runs only in a test that skips in CI (verification-gap) | Pre-verified. The fix needs either a session-builder test seam or CI downloading the runtime and a voice → defer |
| medium | Use's save order (language before voice) is untested (verification-gap) | Pre-verified → patch |
| medium | Install on the Piper row passing `piper_voice` is untested in the UI (verification-gap) | Pre-verified → patch |
| medium | Install on "No Piper voice installed" fetching fahrettin is untested (verification-gap) | Pre-verified. It needs an injectable default-voice source → defer |
| medium | `current_state` merging the installed Piper voices is untested (verification-gap) | Pre-verified → patch |
| low | "Manage voices" switching the Settings tab is untested (verification-gap) | Pre-verified → patch |

## Design Notes

- **Why the default voice is built in:** a pinned fahrettin entry lets first run work without fetching a catalog.
- **Why three digests:** each source publishes a different one. The voice-me catalog carries SHA-256, the official `voices.json` carries MD5, and the HF API gives an LFS SHA-256 for `model.onnx` plus a git-blob SHA-1 for `config.json`. Each is checked with the digest its source publishes, over HTTPS. That trust is weaker than code-pinned SHA-256, but for user-chosen voices it is the only integrity data there is. The default voice keeps a SHA-256 pinned in code.
- **Why the runtime init is injected:** the runtime guard lives in `voice-me-tts`, which is a sibling adapter. Passing the init in keeps AD-1 intact while ORT stays committed once per process.

## Verification

**Commands:**
- `cargo test -p voice-me-espeak -p voice-me-tts-piper -p voice-me-tts-system-linux -p voice-me-core -p voice-me-deps -p voice-me-ui -p voice-me-tests -p voice-me-app` -- expected: all pass
- `cargo check --workspace --all-targets && cargo clippy --workspace --all-targets` -- expected: no new warnings
- `cargo fmt --check` -- expected: clean
- The real-engine Piper test with the runtime and fahrettin downloaded to the scratchpad -- expected: 24 kHz audio of about 2–3 s, generated in under a second

**Manual checks:**
- Run the app on a fresh profile: install the runtime and fahrettin, speak Turkish. Open Piper voices, filter to Turkish, download dfki, press Use, speak, delete it, and confirm the blocking row.
