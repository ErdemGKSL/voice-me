---
title: 'Generate through Azure Neural TTS (Story 3.14)'
type: 'feature'
created: '2026-09-24'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
baseline_commit: '2a28f928fcdb8bba158ac758ae9804f44cf7d388'
context: ['{project-root}/_bmad-output/implementation-artifacts/epic-3-context.md']
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Every remote backend clones the user's voice from a Reference Voice Sample. There is no fast remote option for a user who is happy with a standard voice.

**Approach:** Add **Azure** as a stock-voice remote provider in `voice-me-tts-remote`, behind the same `TtsPort`. It uses the user's key and region. Settings → Backend gets Azure's region, a locale picker and a voice picker, all fed from Azure's live voice list. Core enforces an Azure-shaped one-time disclosure.

## Boundaries & Constraints

**Always:**
- **REST.** Base `https://{region}.tts.speech.microsoft.com`, header `Ocp-Apim-Subscription-Key`.
  - Voices: `GET /cognitiveservices/voices/list`.
  - Speech: `POST /cognitiveservices/v1` with `Content-Type: application/ssml+xml` and `X-Microsoft-OutputFormat: riff-24khz-16bit-mono-pcm`.
  - SSML: `<speak version='1.0' xml:lang='{locale}'><voice name='{voice}'>{text}</voice></speak>`, with the text XML-escaped.
  - The reply is decoded with the existing `decode_wav` (24 kHz mono, no resampling, AD-11).
- **The region** is stored in settings next to the key. It is not secret. It is saved trimmed and lowercased, and only `[a-z0-9]+` is accepted; anything else is an inline error, so the region can never change the host.
- **The voice list (D1)** is fetched in the background whenever a Dependency Check runs with Azure selected and a key and region saved. The result is cached in the root for the session and never persisted. It is the only Azure request allowed before the disclosure: key only, no text. A failed fetch shows inline beside the speech language ("Couldn't list Azure's voices: …"). Saving a new key or region drops the cached list.
- **Locales and voices.** The languages are the distinct `Locale` values in the list, compared case-insensitively and each labelled with its `LocaleName`. A locale's voices are the entries with that `Locale`, labelled `"{LocalName} ({Gender})"`, with `ShortName` as the id. The language defaults to `tr-TR`. The voice defaults to **none** (D4), and saving a new locale clears the stored voice.
- **Stock voice.** Azure never receives or reads the Reference Voice Sample, shows no "sample held on provider" line, and carries the "Stock voice" tag. The Backend tab says "Speech will be in this Microsoft voice, not yours."
- **Disclosure (D2).** It is required before the first Azure line, is per provider, and is enforced in core. It lists the typed text, the language and the voice name, and says the speech will be in a Microsoft voice, not the user's.
- **Blocking capability rows:**
  - no key → "Azure has no API key — add one in Settings → Backend."
  - no region → "Azure has no region — add one in Settings → Backend."
  - no voice → "Azure has no voice selected — pick one in Settings → Backend."

  The check makes no network call.
- **Failures.** A rejected key (401/403), a timeout (30 s for speech, 15 s for the voice list), or a provider error produces one notification naming Azure and the reason. Nothing is re-sent.
- The key stays plaintext-with-notice and is redacted in every `Debug` output.

**Never:**
- No Azure Speech SDK, no Personal Voice, no SSML controls (style, rate), and no voice list shipped in the binary.
- No change to AD-8's egress allowlist, to DeepInfra's behaviour, or to the eSpeak NG paths.
- A stored locale or voice is never replaced with a different one.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Speak | Azure, key, `westeurope`, `tr-TR`, `tr-TR-EmelNeural`, disclosure confirmed | One POST with the SSML above; the 24 kHz buffer is played; no sample is read | N/A |
| First line | Same, disclosure not confirmed | The overlay shows Azure's disclosure; nothing is sent until the user confirms | N/A |
| No voice | Key and region, voice unset | Blocking row "Azure has no voice selected…"; the overlay is blocked (3.4) | N/A |
| Change locale | `tr-TR` + Emel → pick `en-US` | Locale saved, voice cleared, "Pick a voice for Azure" shown | Save failure is shown inline |
| Bad region | "west europe!" | Not saved; inline error | N/A |
| Rejected key | 401 on the voice list or on speech | List: inline error, list empty. Speech: one notification "Azure: rejected key" | Not re-sent |
| Stale voice | Stored voice not in the fetched list | Refused by name before any request; placeholder + note in the tab | One notification |
| List not fetched | Speak before the list arrives, or after it failed | Speaks the stored locale and voice (the voice row already guarantees one); Azure's own error surfaces if it is invalid | One notification |
| Hostile text | `</voice><voice name='x'>` or `&` | Escaped; spoken literally | N/A |

</frozen-after-approval>

## Code Map

- `crates/voice-me-core/src/state.rs` -- `RemoteProvider` (`:134`, `ALL` `:142`) gains `Azure` ("Azure"). Update the exhaustive matches: `:145`, `:183-188`, `:452-458`, and `ApiKeys` get/set/redacting Debug (`:631-678`, new `azure` field). `BackendSelection::is_stock_voice` (`:510`) becomes `SystemVoice | Remote(Azure)`. Add `is_system_voice()` and switch the eSpeak-only callers to it (`voice-me-deps/src/lib.rs:225`, `voice-me-app/src/main.rs` `refresh_system_voices`). The first-run skip and the Stock voice tag keep `is_stock_voice`. `LanguageBackend::Azure`: dynamic like SystemVoice (`has_voice_choice`, `language_options`, `resolve_language`, and `has_speech_language` true while the list is empty). `SpeechLanguages.azure` (default `tr-TR`), `SpeechVoices.azure`, `AppState.azure_region` and `AppState.azure_voices` (the latter not persisted).
- **Stock-voice type** -- generalise `SystemVoice {id, language, name, priority}` into a shared stock-voice type that also carries a language label. eSpeak supplies its top-priority voice name as the label; Azure supplies `LocaleName`. Reuse `system_voice_languages`/`system_voices_of`/`resolve_system_voice` generalised, not duplicated. Renaming is allowed.
- `crates/voice-me-core/src/settings_store.rs` -- `azure` entries in `SpeechLanguagesFile` (`:104,130,136`) and `SpeechVoicesFile` (`:156,166`), a lenient `azure_region`, and the migration default (`:360-380`, System voice pattern). `SettingsStore::save_azure_region` goes in `ports.rs:258-335`, and every fake gets it: `voice-me-ui/src/{settings,hotkey,voice_setup}.rs`, `voice-me-app/src/main.rs:~903,~932`.
- `crates/voice-me-core/src/ports.rs:19-32` -- `CheckRequest` gains `has_region` and `has_voice`. Update its literals (`capability.rs` tests, `deps/lib.rs` tests, `main.rs:337`).
- `crates/voice-me-core/src/speak.rs:132-176` -- an Azure branch: resolve the locale and voice against `azure_voices` when non-empty, then run the disclosure check, then `generate(text, None, locale, Some(voice))`. No sample check.
- `crates/voice-me-tts-remote/src/` -- new `azure.rs`: `Azure::new`/`with_base_url` (tests), `list_voices(key, region)` and `synthesize(key, region, locale, voice, text)`. Move DeepInfra's private `classify`/`detail`/`describe`/`shorten` (`deepinfra.rs:154-227`) into shared code; do not copy them. `AzureTtsAdapter: TtsPort` reads the key and region from the store at `generate`, requires `voice`, never touches `reference_clip`, and uses `block_on` (`lib.rs:258`). Add an Azure arm to `delete_held_sample` (`lib.rs:194`) that says Azure holds no samples. Test with `mock.rs`.
- `crates/voice-me-deps/src/capability.rs:308-347` -- the Azure arm: key → region → voice rows.
- `crates/voice-me-ui/src/backend.rs` --
  - Azure falls into the Remote `Select` through `ALL`.
  - `options_section` (`:820-839`) adds, for Azure: a region `Input` + Save (the key-input pattern `:492-504,672-686,1214`, `BackendArea::AzureRegion`, `BackendAction::SaveAzureRegion(Option<String>)`, with a `Debug` arm), then the language and voice sections.
  - Generalise the hard-coded SystemVoice voice logic (`voice_pick :337`, `shows_voice_picker :643`, the voice-select subscription `:472-490`, `speech_voice_section :893`, `speech_language_note :1318`) by backend.
  - Add the "Pick a voice for Azure" and "Microsoft voice" lines.
  - `provider_slug` `:1363`. Keep `SAMPLE_HOLDING_PROVIDERS` DeepInfra-only.
- `crates/voice-me-ui/src/prompt_overlay.rs:78-365` -- the disclosure takes its item list per provider instead of the fixed `DISCLOSURE_ITEMS`. Raise `OVERLAY_DISCLOSURE_HEIGHT` (`main.rs:116`) if Azure's text needs more room.
- `crates/voice-me-app/src/main.rs` --
  - `build_engine` (`:421-468`): an Azure arm before the "later release" catch-all.
  - `check_request` (`:337`): the new facts.
  - `disclosure_needed` (`:487`): include Azure.
  - `refresh_azure_voices`: modelled on `refresh_system_voices` (`:1898-1942`); `background_spawn` and push the panel.
  - `current_state` and `make_panel` merge the cache.
  - `backend_actions` (`:2049-2246`): `SaveAzureRegion`. Saving the key, the region, the locale or the voice re-runs the check.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-core/src/{state,ports,settings_store,speak}.rs` -- the provider, types, persistence, check facts and speak branch. Tests: the matrix rows Speak, First line, Change locale, Stale voice and List not fetched at core level; the settings round trip for region, locale and voice; key redaction in `Debug`.
- [x] `crates/voice-me-tts-remote/src/{azure,provider,deepinfra,lib}.rs` -- the provider and adapter. Mock tests: exact headers, SSML and escaping; voice-list parsing; 401 → rejected key; timeout; a real 24 kHz RIFF reply decodes unchanged; the sample is never read.
- [x] `crates/voice-me-deps/src/{capability,lib}.rs` -- the three rows. Test each, and that Azure gets no eSpeak row.
- [x] `crates/voice-me-ui/src/{backend,prompt_overlay,lib}.rs` + fakes -- the Azure options and the per-provider disclosure. Tests: the region save and invalid-region error; the voice picker with no voice shows "Pick a voice for Azure"; a locale change sends only the language action; no sample line; the disclosure items name the voice.
- [x] `crates/voice-me-app/src/main.rs` -- the wiring.

**Acceptance Criteria:**
- Given Azure selected with a valid key, region and voice, when I confirm the disclosure and type a line, then it plays through the Virtual Microphone in that Microsoft voice.
- Given `cargo test --workspace` and `cargo check --workspace --all-targets`, then all pass, and `voice-me-tests`' egress allowlist is unchanged and green.

## Spec Change Log

## Review Triage Log

Pass 1 (blind-hunter, edge-case-hunter, verification-gap).

| Verdict | Finding | Evidence |
|---|---|---|
| low | Azure's error body never reaches the user: the shared `detail()` reads only `detail`, but Azure answers `{"error":{"message":…}}` (blind) | Confirmed in `http.rs`. The notification still names Azure and the HTTP status, but not Azure's reason. Also reading `error.message` is a direct correction → patch |
| medium | A successful voice-list fetch clears `BackendArea::SpeechLanguage`, including an unrelated "couldn't save the language" error (blind, edge, verification-gap other) | Confirmed: `refresh_azure_voices` removes the area on `Ok`, and `SetSpeechLanguage` now triggers a check → patch (clear only the fetch's own error) |
| low | The list error stays after the key or region is removed (edge) | Confirmed: `refresh_azure_voices` returns early without clearing it. A backend switch is already cleared by `apply_selection` → patch, with the previous finding |
| medium | The session cache refetches the whole list on every check, now including every locale or voice pick (blind) | Confirmed. D1 says the list is cached for the session → patch (skip the fetch while a list for the current key and region is held) |
| low | Overlapping fetches, or a transient failure, can replace a good list with an empty one (blind, edge) | Real, but made moot by the cache patch: with a list held, no refetch happens until a key or region save drops it. The spec's matrix wants an empty list on a failed fetch → rejected after the patch |
| medium | `check_request`'s `has_region`/`has_voice`, `disclosure_needed`'s Azure arm, `build_engine`'s Azure arm and `disclosure_text` are untested (verification-gap) | Pre-verified. All four are pure functions in `main.rs`'s test reach → patch |
| medium | The `run_check` calls added to `SetSpeechLanguage`/`SetSpeechVoice`, `drop_azure_voices` and the generation guard are untested (verification-gap) | Pre-verified. They are closures inside `fn main`, the same gap as 3.11's and 3.12's `backend_actions` deferrals → defer |
| maybe-false | Azure's disclosure (an extra note and a long voice item) may not fit `OVERLAY_DISCLOSURE_HEIGHT` (blind) | No test renders it. Medium if true (clipped disclosure). Settled by the manual check → defer |
| low | A case-only locale change (`tr-tr` → `tr-TR`) clears the stored voice (blind) | Confirmed: `save_speech_language` compares exactly, while the spec compares locales case-insensitively. A direct correction → patch |
| low | The region's inline error vanishes when an unrelated panel push arrives (blind) | Real only while a fetch is in flight when an invalid region is saved. The fix routes the error through the root → rejected |
| low | The disclosure shows the locale code rather than its `LocaleName`, and has an unreachable empty fallback (blind) | The code names the language; the spec does not require the display name → rejected |
| false | The disclosure is not re-asked when the voice or locale changes (blind) | The epic context and 3.6 set the confirmation per provider, "asked once per provider" → rejected |
| low | Saving an unchanged key or region drops the cached list (blind) | One extra fetch on a rare action → rejected |
| low | Deprecated voices are kept, and voices are not sorted (blind) | Azure's list is GA voices in name order. Rare → rejected |
| false | `Azure::with_base_url` is `pub` and bypasses the region check (blind) | The same test seam as DeepInfra's `with_base_url`. The app only ever builds `Azure::new()`, and the rule protects against a user-typed region → rejected |
| low | Azure synthesis holds the process-wide sample lock (blind) | DeepInfra sample work can only overlap from the Backend tab's delete while Azure is selected. Rare → rejected |
| false | The spec status disagrees with sprint status, and the matrix wording "Azure: rejected key" differs from the code (blind) | Sprint status moves at step 5. The matrix names the content, not exact copy, and editing the spec's frozen text is out → rejected |
| low | XML-illegal control characters in the text make Azure answer 400 (edge) | Not typeable in the overlay, and the error still names Azure. The fix adds a filter → rejected |
| low | An empty `LocalName`/`LocaleName` gives a blank label (edge) | Azure always fills them → rejected |
| low | A hand-edited blank locale with no list fetched sends `xml:lang=''` (edge) | Hand-edited only, and Azure's error names the problem → rejected |

## Design Notes

D1 says the list is fetched "when the user opens the voice picker". The gpui-kit `Select` has no open hook, so the list is fetched on each check run instead. Selecting Azure, or saving its key or region, triggers a check, so the list is still live, on demand and session-cached.

## Implementation Notes

- `SystemVoice` is now `StockVoice {id, language, language_label, name, priority}`; `stock_voice_languages`/`stock_voices_of`/`resolve_stock_voice` serve both backends. Locales compare case-insensitively. `StockVoiceRefusal` names the backend and adds `NoVoice` (Azure has no default voice, `LanguageBackend::requires_voice`).
- `LanguageBackend::Remote(Azure)` is the Azure language backend; `has_voice_list()` marks the dynamic ones. `AppState::stock_voices(backend)` picks the list.
- The region is validated by `parse_azure_region` in the UI (inline error, never sent), in `save_azure_region`, on load (a bad hand-edited value loads as none) and again before `Azure` builds a URL.
- Shared HTTP code lives in `voice-me-tts-remote/src/http.rs`; Azure calls use `Call::Stock`, which never maps to `VoiceGone`. Azure sits beside `SpeechProvider` (it is not a cloning provider). The mock server now serves binary bodies.
- The voice-list fetch is guarded by a generation counter, so a fetch in flight when the key or region changes is discarded. A fetch error goes to `BackendArea::SpeechLanguage`, like the System voice's listing error.
- Azure's Backend tab order: key, region, language, voice, then "Pick a voice for Azure" (while none is saved) and the Microsoft-voice line.
- `OVERLAY_DISCLOSURE_HEIGHT` is unchanged: Azure's three short items plus the note are no taller than DeepInfra's long sample item.
- Review patches (2026-09-24):
  - `http::detail` reads Azure's `{"error":{"message":…}}`.
  - The voice-list refresh keeps a held list (saving a key or region drops it), and clears only its own "Couldn't list Azure's voices" error, including when the key or region is removed.
  - A case-only locale change keeps the Azure voice.
  - New `main.rs` tests cover `build_engine`, `check_request`, `disclosure_needed` and `disclosure_text` for Azure, plus the error ownership.
- Verified after the review patches (user's call, because disk space is short): `cargo check --workspace --all-targets` is clean and `cargo fmt --check` is clean. The patched tests were not run. Before the patches, the spec's full `cargo test` list passed; afterwards the `voice-me-tests` egress test failed to link because the disk was full, and no `Cargo.toml` changed.

## Verification

**Commands:**
- `cargo test -p voice-me-core -p voice-me-ui -p voice-me-deps -p voice-me-tts-remote -p voice-me-tests -p voice-me-app` -- expected: all pass
- `cargo check --workspace --all-targets && cargo clippy --workspace --all-targets` -- expected: no new warnings
- `cargo fmt --check` -- expected: clean

**Manual checks:**
- With a real Azure key: select Remote → Azure, then save the region. The locale list fills. Pick `tr-TR` and a voice, confirm the disclosure, and speak. Switch to `en-US` and pick a voice. Enter a bad key and confirm the inline list error and the speech notification.
