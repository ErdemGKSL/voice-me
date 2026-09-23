---
title: 'Generate speech through DeepInfra with the user''s own key (Story 3.6)'
type: 'feature'
created: '2026-09-23'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
baseline_commit: 'e68d7c03a4948be06ff7f95a37e5566e50f9f833'
context: ['{project-root}/_bmad-output/implementation-artifacts/epic-3-context.md']
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Selecting DeepInfra today only produces a blocking row that says "Remote generation through DeepInfra arrives in a later voice-me release". Local CPU generation takes about 20 seconds per line on this machine, and a user with their own DeepInfra key has no way to use it.

**Approach:** Add a new crate, `voice-me-tts-remote`. It implements `TtsPort` over a `SpeechProvider` trait, and its first implementation calls DeepInfra's `ResembleAI/chatterbox-multilingual` model. The Speak Action path does not change: the composition root just puts this engine in the slot when DeepInfra is selected. Before anything is sent, `voice-me-core` requires a one-time, per-provider confirmation of exactly what leaves the machine. The Reference Voice Sample is uploaded once as a DeepInfra voice, and later requests reference it by `voice_id`. That id is keyed by the sample's SHA-256, so re-recording triggers a fresh upload. The Dependencies tab shows that the sample is held on DeepInfra, with a **Delete from DeepInfra** action. Every provider failure ends in the existing single failure notification, naming the provider and the reason.

## Boundaries & Constraints

**Always:**
- **What leaves the machine:** only the typed text, the language tag (`language_id`), and the Reference Voice Sample WAV. The upload's `name` and `description` are the fixed strings "voice-me reference sample" and "Uploaded by voice-me". Nothing about the machine, the user, or usage is sent.
- **Core enforces the disclosure.** `speak_inner` refuses a remote selection whose provider is not in `AppState.confirmed_disclosures`, before it calls `tts.generate`. The adapter is not trusted to check.
- **Every request has a deadline:** 120 s for inference, 60 s for the upload, and 30 s for a delete. No request is ever retried automatically.
- **The key never leaves memory except as the auth header.** It never appears in `Debug` output, error strings or logs. Error bodies from DeepInfra are shortened to their `detail` message.
- **Output is 24 kHz mono f32 (AD-11).** The adapter asks for `response_format: "wav"`, decodes it with `hound`, and resamples with `rubato` when the provider's rate differs.
- **Nothing above `TtsPort` is provider-specific.** Core, the UI and the app see only `RemoteProvider`, `VoiceMeError`, and plain strings.

**Never:**
- **No fal.ai generation** (Story 3.7). fal.ai keeps its current "arrives in a later release" row.
- **No reachability or key-validation probe before the disclosure is confirmed.** No byte goes to the provider until the user has confirmed.
- **No silent fallback to CPU.** No `webhook`, no streaming, and no tuning parameters beyond the defaults.
- **Local backends are not touched.**

## Decisions

1. **Disclosure lives in the Prompt Overlay (answered 2026-09-23).** On a hotkey press, if DeepInfra is selected, it has a key, and its disclosure is unconfirmed, the overlay opens in a "confirm first" state. That state names the provider and the three things sent (the typed text, the language tag, the Reference Voice Sample), with **Send to DeepInfra** (Enter) and Cancel (Escape). Confirming records the provider in settings through a root callback, and the same overlay then becomes the normal text input. Cancel dismisses it and records nothing. Core's gate stays the enforcement; the overlay is only the surface.
2. **Size:** the full spec is kept as-is, knowing it is above the token target (answered 2026-09-23).

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| First use | DeepInfra selected, key saved, not confirmed | Overlay opens in the confirm state (Decision 1); no request made | Declining leaves DeepInfra selected, still unconfirmed |
| First Speak after confirming | No stored voice id | Upload → store `(deepinfra, sha256) → voice_id` → inference → playback | Upload failure: one notification, no inference |
| Later Speak | Stored id with the current hash | Inference only, with the stored `voice_id` | N/A |
| Re-recorded sample | Stored hash ≠ current hash | The old voice is deleted (best effort), then the new sample is uploaded | If the delete fails, it is logged, the stale id is dropped, and the upload goes ahead |
| Rejected key | HTTP 401/403 | Notification: "DeepInfra rejected the API key." | No re-send |
| Timeout | No response before the deadline | "DeepInfra did not answer within 120 seconds." | No re-send |
| Provider error | 4xx/5xx with `detail` | "DeepInfra: <detail>" (shortened) | No re-send |
| Stored voice gone | 404 or unknown `voice_id` | The id is dropped; the notification says the sample will be uploaded again on the next line | No automatic re-send |
| Delete clicked | Stored id exists | DELETE; the id is removed from settings; the row shows "Not held on DeepInfra" | An error is shown inline; the id is kept |
| No key | DeepInfra selected, key empty | The existing blocking "no API key" row | N/A |

</frozen-after-approval>

## Code Map

- `crates/voice-me-core/src/ports.rs:108-160` -- `TtsPort` (`warm_up`, `is_ready`, `generate(text, reference_clip, language)`) is unchanged. The remote adapter's `warm_up` is a no-op and `is_ready` returns true.
- `crates/voice-me-core/src/error.rs:5-84` -- add `VoiceMeError::Provider { provider: String, reason: String }` (Display: "{provider}: {reason}") and `DisclosureNotConfirmed(String)`.
- `crates/voice-me-core/src/state.rs:135,161,300,606` -- `RemoteProvider`, `BackendSelection`, `ApiKeys` and `AppState`. Add `AppState.confirmed_disclosures: Vec<RemoteProvider>` and `remote_samples: Vec<RemoteSample { provider, sample_sha256: String, voice_id: String }>`.
- `crates/voice-me-core/src/settings_store.rs:46-74,324-341` -- add both fields to `SettingsFile`, each with a serde default and read leniently. Add `save_disclosure_confirmed(provider)`, `save_remote_sample(provider, Option<RemoteSample>)`, and `load_remote_sample(provider)` in the `save_api_key` pattern. The sample always lives at `data_dir/reference_voice_sample.wav` (line 23/211) and has no id today, so the hash is computed by the adapter.
- `crates/voice-me-core/src/speak.rs:104-160` -- add the disclosure gate in `speak_inner`, before the reference-sample check. `speak` (line 76) already turns any error into the single `GENERATION_FAILED_SUMMARY` notification; reuse it as-is.
- `crates/voice-me-tts-remote/` (new) -- `SpeechProvider` trait (`upload_sample`, `delete_sample`, `synthesize` → WAV bytes, `label`). `DeepInfra` implements it with `reqwest` 0.12 (`rustls-tls`, `json`, `multipart`), `serde_json`, and `base64` for the `audio` data-URL. `RemoteTtsAdapter<P: SpeechProvider>` implements `TtsPort`; it takes a `SettingsStore` handle for the voice-id cache and runs async calls through the same `block_on` pattern as `voice-me-deps/src/lib.rs:316`. Endpoints: `POST https://api.deepinfra.com/v1/inference/ResembleAI/chatterbox-multilingual` with JSON `{text, voice_id, language_id, response_format:"wav"}` → `{audio: "data:audio/wav;base64,…"}` (the data-URL prefix is from DeepInfra convention, not verified here — accept bare base64 too); `POST /v1/voices/add` as multipart (`audio`, `name`, `description`) → `{voice_id}`; `DELETE /v1/voices/{id}`. Auth is `Authorization: Bearer <key>`. Base URL is injectable for tests.
- `crates/voice-me-deps/src/capability.rs:306-314` -- DeepInfra with a key returns `None` (it can run). fal.ai keeps the existing sentence. Update the test at `:646`.
- `crates/voice-me-app/src/main.rs:405-424` -- in `build_engine`, `Remote(DeepInfra)` builds `RemoteTtsAdapter`, and fal.ai keeps the "later release" message. Also wire the disclosure confirm and sample delete callbacks (save, then push panel). Update the AD-8 comment in `crates/voice-me-app/Cargo.toml:17-18`.
- `crates/voice-me-ui/src/dependencies.rs:881-1015` -- below the API keys, add a "Voice sample on DeepInfra" line ("Held on DeepInfra's servers" + **Delete from DeepInfra**, or "Not held on DeepInfra"), fed by `panel` and acting through a new `BackendAction::DeleteRemoteSample(provider)`.
- `crates/voice-me-ui/src/prompt_overlay.rs:69-300` -- the confirm-first state (Decision 1), built like the `blocked` constructor at `:90`.
- `crates/voice-me-tests/src/lib.rs` -- add a manifest allowlist test: only `voice-me-deps` and `voice-me-tts-remote` may declare `reqwest`. This stands in for the CI egress check the epic assumes and that does not exist yet.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-core/src/{error,state,settings_store,speak}.rs` -- add the new errors, the disclosure and remote-sample state and persistence, and the core disclosure gate. Test: settings round-trip, and `speak` refusing an unconfirmed remote selection before `generate` is called (asserted with `FakeTts`).
- [x] `crates/voice-me-tts-remote/**` + root `Cargo.toml` members -- add the provider trait, DeepInfra, and the adapter. Test every matrix row against a local mock HTTP server on `127.0.0.1` (a `tokio` listener; no new mock crate), and test WAV decode and resample.
- [x] `crates/voice-me-deps/src/capability.rs` -- DeepInfra with a key can run.
- [x] `crates/voice-me-app/src/main.rs` + `Cargo.toml` -- build the engine and wire the callbacks. Test `build_engine` for DeepInfra vs fal.ai.
- [x] `crates/voice-me-ui/src/{dependencies,prompt_overlay}.rs` -- add the sample-state line with delete, and the disclosure surface, with UI tests (confirm fires once; the delete callback fires).
- [x] `crates/voice-me-tests/src/lib.rs` -- add the `reqwest` allowlist test.
- [x] `_bmad-output/planning-artifacts/epics.md` + `ARCHITECTURE-SPINE.md` -- correct the model id `ensembleAI` → `ResembleAI`.

**Acceptance Criteria:**
- Given DeepInfra is selected with a valid key and the disclosure confirmed, when a line is spoken, then it plays through the Virtual Microphone exactly as a local line does.
- Given any failure, then exactly one notification appears, titled "Couldn't generate speech.", whose body names DeepInfra and the reason.
- Given the key is set, when logs and settings `Debug` output are inspected, then the key never appears.

## Implementation Notes

- **Provider error wording.** The Code Map gives `Provider`'s Display as "{provider}: {reason}", while the frozen matrix wants "DeepInfra rejected the API key." and "DeepInfra did not answer within 120 seconds." (no colon). Display follows the matrix: a `reason` that already opens with the provider's name is shown as-is, anything else is prefixed "{provider}: " (`error.rs::provider_message`).
- **Key read per request.** `RemoteTtsAdapter` reads the key from `SettingsStore` on every call rather than holding it, so saving a new key needs no engine rebuild. It never appears in errors, logs or `Debug` (tests assert it).
- **Settings handle.** The adapter and the sample delete need a `Send + Sync` store, so the composition root keeps the `FileSettingsStore` both as `Arc<dyn SettingsStore>` and as `SharedSettingsStore` (`Arc<dyn SettingsStore + Send + Sync>`). `SettingsStore` itself is unchanged apart from three methods (`load_remote_sample` has a default implementation).
- **Serialization.** A crate-wide lock serializes generation and Settings' delete, so an upload and a delete never cross.
- **Overlay height.** The confirm-first overlay opens at 196 px and keeps that height after confirming. It is not resized back to 84 px, because the overlay window is non-resizable and resizing behaves differently under X11 and Wayland.
- **Unknown voice detection.** A 404, or a 4xx whose `detail` mentions a voice together with "not found"/"unknown"/"does not exist"/"invalid", counts as "stored voice gone" on inference and delete. A delete of a voice that is already gone counts as success.
- **Delete runs** on GPUI's background executor, not the Tokio bridge, so it works even when the runtime failed to start. The Settings panel is also pushed after every Speak Action, so a sample uploaded by a line shows up as held.

## Spec Change Log

## Review Triage Log

Pass 1 (blind-hunter, edge-case-hunter, verification-gap).

| Verdict | Finding | Evidence |
|---|---|---|
| medium | Any inference 404, or any detail with "voice" + "invalid", counts as `VoiceGone`, so the id is dropped and each later line uploads another orphaned copy (blind, edge, verification-gap other) | Confirmed: `classify(.., voice_call=true)` maps every 404 on inference; a retired model path would loop upload → 404 → drop. → patch (inference: `VoiceGone` only when the detail names the voice) |
| medium | A WAV the adapter cannot decode gives a notification that never names DeepInfra (edge, claim) | Confirmed: `decode_wav` returns `UnsupportedAudioInput`, which contradicts the AC "body names DeepInfra and the reason". → patch |
| medium | Upload and delete deadlines are never exercised (verification-gap, pre-verified) | Swapping the deadlines passes the suite. → patch (two tests) |
| medium | The multi-thread `block_on` branch the app actually uses is untested (verification-gap, pre-verified) | All adapter tests run from a thread with no Tokio context. → patch (bridge test) |
| low | A zero-frame WAV succeeds silently, with no audio and no notification (blind, edge) | Confirmed in `wav.rs`; a one-line check. → patch |
| low | `describe` formats `reqwest` errors with their URL, contradicting its own doc comment (blind) | Confirmed: `without_url()` is not called there; the key is never in the URL. Direct correction → patch |
| low | The confirm-first overlay is offered for fal.ai while the check is `Pending` (edge) | Confirmed: `disclosure_needed` ignores whether the provider can generate. Direct correction (`DeepInfra` only) → patch |
| low | The disclosure copy says the sample stays "until you delete it", but re-recording also replaces it (blind) | Confirmed; a wording correction → patch |
| low | The new code is not rustfmt-formatted (blind) | `cargo fmt --check` reports diffs only in changed files. → patch |
| medium | Confirming in the overlay is never shown to persist the confirmation (verification-gap, pre-verified) | The callback is built inline in `main()`; testing it needs a helper extracted from `main`. → defer |
| low | A failed delete of the stale voice on re-record drops its id, leaving an untracked copy on DeepInfra (blind, verification-gap other) | Real, but it is the frozen matrix's "Re-recorded sample" row; the fix edits the spec. Rejected; raised with the human |
| low | Settings writes from the generation thread can race main-thread saves and lose one (blind) | Confirmed: `FileSettingsStore` has no lock, but the window is one TOML read-modify-write while a user saves at the same instant; the fix adds a guard. Rejected |
| low | Upload succeeds but saving its id fails, orphaning the voice (edge) | Needs an unwritable settings file mid-run; the fix adds a branch. Rejected |
| low | A DELETE 404 for another reason counts as deleted (edge) | The path is fixed `/v1/voices/{id}`; a 404 there is the voice being gone. Rejected |
| low | Settings' delete waits on the generation lock, up to ~3.5 min (blind, edge) | Only while a line is in flight; the fix adds a try-lock path. Rejected |
| low | Deleting after the key was removed fails (blind) | The message says to add a key, which is the fix. Rejected |
| low | `voice_id` is put in the URL path unencoded (blind, edge) | Ids come from DeepInfra or a hand-edited file. Rejected |
| low | Response bodies have no size cap (blind) | Needs a misbehaving endpoint; the fix adds a guard. Rejected |
| low | A failed confirmation save is only logged, and the question repeats (blind) | Needs an unwritable settings file. Rejected |
| low | The egress test covers `reqwest` only (blind) | The spec names `reqwest`; broader bans add a list. Rejected |
| maybe-false | The 196 px confirm overlay may clip, and stays tall after confirming (blind) | Settled by the manual check; it would only be low. Rejected |
| low | No test for the root's `DeleteRemoteSample` handler (blind) | Built inline in `main()`, like the confirm callback; the delete itself is covered in `tests.rs`. Rejected |
| false | There is no way to revoke a confirmed disclosure (blind) | The intent is a one-time confirmation; revoking is not asked for. |
| false | The language tag is sent unmapped (blind) | `speak_inner` only lets `tr`/`en` through, and both are valid `language_id` codes. |
| false | The sample may not be a WAV (blind) | `SettingsStore::save_reference_voice_sample` takes WAV bytes and writes `reference_voice_sample.wav`; imports are converted before saving. |
| false | Spec and sprint status disagree (blind) | Sprint status is synced in step 5. |

## Verification

**Commands:**
- `cargo check --workspace` -- expected: clean, with no new warnings.
- `cargo test --workspace` -- expected: green, with no real network access (a mock server on loopback only).

**Manual checks:**
- With a real DeepInfra key: confirm the disclosure, speak a Turkish line, and hear it in the voice of the sample; `settings.toml` holds `voice_id`. Delete it from Settings; DeepInfra's `GET /v1/voices` no longer lists it.
