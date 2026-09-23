# Sprint Change Proposal — Backend tab, per-backend speech language, Azure Neural TTS

**Date:** 2026-09-23
**Author:** Developer (correct-course), for Erdem (product owner)
**Mode:** Batch
**Status:** Approved by Erdem, 2026-09-23 — edits applied to PRD, architecture spine, EXPERIENCE.md, epics.md and sprint-status.yaml

---

## 1. Issue Summary

**Trigger.** On 2026-09-23, right after Story 3.6 (DeepInfra) was verified against the live API, the product owner asked for three changes.

1. **Azure Neural TTS as a remote backend, using Microsoft's standard stock neural voices.** These voices are not cloned from the user's sample. Azure Personal Voice, which needs Microsoft's limited-access approval, is explicitly out of scope.
2. **Backend choice gets its own Settings → Backend tab.** Today it sits inside Settings → Dependencies. The user first picks the kind of backend (Local or Remote); Remote then branches into providers (DeepInfra, fal.ai, Azure). Only the selected backend's options are shown.
3. **Speech language is chosen per backend in that tab.** Each backend lists only the languages it supports, independently of the UI language.

**Category.** A new requirement from the stakeholder (items 1–2), plus a requirements gap found during implementation (item 3).

**Evidence for item 3:**
- Spec-2-6 Decision 2 stored `speech_language` (default `"tr"`) and postponed its selector to Epic 4.
- Epic 4's two stories cover only the UI language. Story 4.1 even refers to "the TTS speech language selection in Settings → Voice", as if it already existed.
- No story builds that selector. Today the language can only be changed by hand-editing `settings.toml`, and `speak_inner` hard-codes `["tr", "en"]` for every backend. That includes DeepInfra, whose hosted model supports 23 languages.

**Evidence for item 2:**
- Settings → Dependencies now holds the backend `Select`, the added runtimes, both API keys, the remote sample state and the capability row. The GPU picker (Story 3.9) and fal.ai (Story 3.7) would add more still.
- The UX spine promised "one primary view" per section. That tab no longer is one.

**Evidence for item 1:** this is a product decision. It reverses PRD §6.2's exclusion of "remote models that cannot clone a voice (stock-voice-only providers)". Azure's REST API was checked on 2026-09-23. Both `POST https://{region}.tts.speech.microsoft.com/cognitiveservices/v1` and `GET …/cognitiveservices/voices/list` exist, and both answer 401 to a bad key. Its `riff-24khz-16bit-mono-pcm` output format is AD-11's buffer format with no resampling.

---

## 2. Impact Analysis

### Checklist results

| # | Item | Status | Finding |
|---|---|---|---|
| 1.1 | Triggering story | [x] | Story 3.6 (done, `d0e6161`); a product-owner request after live verification |
| 1.2 | Core problem | [x] | A new requirement (Azure, Backend tab) plus a gap (no speech-language selector) |
| 1.3 | Evidence | [x] | See §1 |
| 2.1 | Current epic | [x] | Epic 3 can still be completed; it gains three stories |
| 2.2 | Epic-level changes | [!] | Epic 3: scope note and FR coverage widened; Stories 3.10, 3.11 and 3.12 added; 3.7 and 3.9 retargeted to the Backend tab |
| 2.3 | Remaining epics | [!] | Epic 4: Story 4.1 points at the wrong tab for speech language |
| 2.4 | Obsolete or new epics | [x] | None made obsolete; no new epic needed |
| 2.5 | Order and priority | [!] | 3.10 → 3.11 → 3.12 go ahead of the remaining 3.7 / 3.8 / 3.9 |
| 3.1 | PRD | [!] | FR-5, FR-10, §5 Non-Goals, §6.1 and §6.2 |
| 3.2 | Architecture | [!] | AD-9, AD-12 (language scope), AD-13, Stack, Capability map |
| 3.3 | UX | [!] | EXPERIENCE.md sections table and component and state rows; UX-DR7 and UX-DR13; a new UX-DR23 |
| 3.4 | Other artifacts | [!] | `sprint-status.yaml` (three new keys); the `voice-me-tests` egress allowlist is unchanged (Azure lives in `voice-me-tts-remote`) |
| 4.1 | Direct adjustment | Viable | Effort Medium, risk Low. Everything is additive stories inside Epic 3 |
| 4.2 | Rollback | Not viable | Nothing shipped is wrong; 3.3, 3.5 and 3.6 stay, and 3.10 moves their UI |
| 4.3 | MVP review | Not needed | MVP grows by one provider and one tab; nothing leaves scope |

### Epic impact

- **Epic 3** grows from 9 to 12 stories. Done work stays valid:
  - The backend selection, keys and remote-sample state from Stories 3.3, 3.5 and 3.6 are *moved*, not rewritten.
  - The `SpeechProvider` abstraction from 3.6 gains a second shape: a stock-voice provider that uploads no sample.
- **Epic 4** gets a one-line AC correction and nothing else.

### Technical impact

- **Settings file.** `speech_language: String` becomes per-backend state: a language per backend, plus an Azure voice name. Old files migrate, and the existing value seeds the local and DeepInfra entries.
- **Core.** The language check in `speak_inner` moves from the fixed `["tr", "en"]` to "is this language in the selected backend's supported set".
- **`voice-me-tts-remote`:**
  - Gains an Azure provider, authenticated with `Ocp-Apim-Subscription-Key` and a region.
  - Requests are SSML (`<voice name=…>` plus `xml:lang`) and return `riff-24khz-16bit-mono-pcm` directly.
  - The provider trait separates cloning providers (upload a sample, reference it by id) from stock-voice providers (take a voice name, never touch the sample).
- **UI.** A new `backend.rs` tab. The backend section moves out of `dependencies.rs`, and Dependencies keeps only the dependency rows and the capability row.
- **Egress.** Unchanged. Azure calls go through `voice-me-tts-remote`, which is already allowed network access (AD-8).

---

## 3. Recommended Approach

**Direct adjustment:** three new stories in Epic 3, taken in the order the product owner set.

1. **3.10 Settings → Backend tab.** Restructure only: Local / Remote first, then the provider, then that backend's options. It ships on the existing CPU, added-runtime, DeepInfra and fal.ai entries.
2. **3.11 Per-backend speech language.** Fills the missing selector, built on 3.10's tab.
3. **3.12 Azure Neural TTS.** A stock-voice provider inside the 3.10 and 3.11 structure, with its own language (locale) and voice pickers.

**Why this order:** 3.12 needs a place for region and voice settings (3.10) and a per-backend language (3.11). Built first, Azure would have to be squeezed into the Dependencies tab and then moved.

- **Effort:** Medium. Each story is about the size of 3.3 or 3.6.
- **Risk:** Low.
  - The one real product risk is Azure's stock voice being mistaken for the user's own. It is handled by stating it in the Backend tab and in the Azure disclosure.
  - The one technical risk is migrating old settings files. It is covered by a round-trip test.
- **Timeline:** three build cycles, ahead of 3.7, 3.8 and 3.9.

Decisions recommended here, which approving this proposal also approves:

- **D1 — Azure's voice list is fetched live, on demand.**
  - When the user opens the Azure voice picker, `voice-me-tts-remote` calls `GET /cognitiveservices/voices/list` with the key and region, and the result is cached for the session.
  - This request carries only the key: no text, no language, no sample. So it is allowed before the per-provider disclosure is confirmed, and AD-13 records it as the one pre-disclosure call.
  - Alternative: a static list shipped in the binary. It needs no network, but goes stale as Microsoft adds voices.
- **D2 — Azure's disclosure is shaped for Azure.** It lists the typed text, the language, and the selected voice name, and says plainly that speech will be in a Microsoft voice, *not* the user's own. No Reference Voice Sample is ever sent to Azure, and Azure has no "sample held on provider" line.
- **D3 — Supported languages per backend.**
  - **Local ONNX:** `tr` and `en` (AD-12's Python-free normalization limit, unchanged).
  - **DeepInfra (`ResembleAI/chatterbox-multilingual`):** the 23 languages its model card lists. Normalization runs on their side, so AD-12's limit does not apply.
  - **fal.ai:** decided in Story 3.7.
  - **Azure:** the locales present in its voice list.
- **D4 — Defaults and migration.**
  - An existing `speech_language` seeds the Local and DeepInfra languages.
  - Azure defaults to `tr-TR` with no voice chosen. Until a voice is picked, the Backend tab says so, and the Speak Action is blocked by Story 3.4's gate as a capability row ("Azure has no voice selected").

---

## 4. Detailed Change Proposals

### 4.1 PRD — `prds/prd-voice-me-2026-09-20/prd.md`

**FR-5, description.**

OLD:
> On a Speak Action, the typed text plus the active Reference Voice Sample and selected language are handed to the **selected speech backend** — a local ONNX backend running in-process, or a remote speech API (FR-10) — producing audio in the user's cloned voice.

NEW:
> On a Speak Action, the typed text plus the selected backend's speech language are handed to the **selected speech backend** — a local ONNX backend running in-process, or a remote speech API (FR-10). A voice-cloning backend also receives the active Reference Voice Sample and produces audio in the user's cloned voice; a stock-voice backend (Azure Neural TTS) produces audio in the stock voice the user selected, and never receives the sample.

**FR-5, first consequence.**

OLD:
> - Generated audio is in the language the user selected for speech output (independent of UI language), from Chatterbox-Multilingual V3's supported language set.

NEW:
> - Generated audio is in the speech language the user selected **for the active backend** (independent of UI language), from that backend's own supported set; each backend remembers its own choice.

**FR-10, description.**

OLD:
> The user selects which speech backend generates their voice, from Settings. Local backends (CPU, CUDA, WebGPU) run entirely on the machine. A remote backend (DeepInfra `ensembleAI/chatterbox-multilingual`, fal.ai) generates through a third-party API using a key the user supplies.

NEW:
> The user selects which speech backend generates their voice, from Settings → Backend: first Local or Remote, then the specific backend. Local backends (CPU, CUDA, WebGPU) run entirely on the machine. Remote backends generate through a third-party API using a key the user supplies: voice-cloning providers (DeepInfra `ResembleAI/chatterbox-multilingual`, fal.ai) and one stock-voice provider (Azure Neural TTS, standard neural voices). Only the selected backend's options are shown, including its speech language and, for Azure, its region and voice.

**FR-10, consequences.** The disclosure bullet changes and one bullet is added.

OLD:
> - Before the first byte leaves the machine, the app states plainly what is sent — the typed text and the Reference Voice Sample — and to which provider, and the user confirms it once per provider.

NEW:
> - Before any text leaves the machine, the app states plainly what is sent to that provider and the user confirms it once per provider: for a cloning provider the typed text, the language tag and the Reference Voice Sample; for Azure the typed text, the language and the voice name, together with the statement that speech will be in a Microsoft voice rather than the user's own. The only earlier request is Azure's voice list, which carries the key alone.
> - A stock-voice backend is labelled as such wherever it is selected, so it is never mistaken for the user's cloned voice.

**§5 Non-Goals.**

OLD:
> - Not supporting speech languages beyond what Chatterbox-Multilingual V3 ships with.

NEW:
> - Not supporting speech languages beyond what each backend itself supports — local Chatterbox is further limited to languages needing no Python-only normalization (AD-12).

**§6.1 In Scope.**

OLD:
> - Remote generation through DeepInfra and fal.ai with user-supplied keys (FR-10)

NEW:
> - Remote generation through DeepInfra and fal.ai (voice cloning) and Azure Neural TTS (standard stock voices) with user-supplied keys (FR-10)
> - A Settings → Backend tab with per-backend options and per-backend speech language (FR-5, FR-10)

**§6.2 Out of Scope.**

OLD:
> - Remote models that cannot clone a voice (stock-voice-only providers) — the AD-13 abstraction allows them, v1 ships only voice-cloning providers

NEW:
> - Stock-voice-only providers other than Azure Neural TTS, and Azure Personal Voice (it requires Microsoft's limited-access approval) — revised 2026-09-23, product-owner decision

Rationale: product-owner decision, 2026-09-23. It reverses the stock-voice exclusion for Azure only.

### 4.2 Architecture — `architecture/architecture-voice-me-2026-09-20/ARCHITECTURE-SPINE.md`

**AD-9, rule item 1.** Append:
> …and, **per backend**, the speech language (and for a stock-voice backend, the voice). Each backend keeps its own; switching backends never rewrites another backend's language.

**AD-12, language scope consequence.** Append:
> This limit binds the **local** backend only. A remote backend's language set is whatever its provider serves (normalization runs on the provider's side), and core validates the selected language against the selected backend's set rather than a global list.

**AD-13, rule.**

OLD:
> …one `SpeechProvider` trait per vendor (DeepInfra `ResembleAI/chatterbox-multilingual` first, fal.ai second)…

NEW:
> …one `SpeechProvider` per vendor, in two shapes: **cloning providers** (DeepInfra `ResembleAI/chatterbox-multilingual`, fal.ai), which upload the Reference Voice Sample once and reference it by id, and **stock-voice providers** (Azure Neural TTS), which take a voice name and never receive the sample…

**AD-13, disclosure bullet.** Append:
> Exactly one request may precede the confirmation: a stock-voice provider's **voice list**, which carries the key alone — no text, language or sample — and is made only when the user opens that provider's voice picker.

**AD-13, credentials.** Append:
> Azure additionally stores its **region** (not a secret) beside the key.

**Stack table.**

OLD:
> | Speech providers (external) | DeepInfra `ResembleAI/chatterbox-multilingual` (default), fal.ai — user-supplied keys, AD-13 |

NEW:
> | Speech providers (external) | DeepInfra `ResembleAI/chatterbox-multilingual` (default), fal.ai — voice cloning; Azure Neural TTS (REST, SSML, `riff-24khz-16bit-mono-pcm`) — stock voices; all user-supplied keys, AD-13 |

**Capability map, FR-10 row.**

OLD:
> | FR-10 Backend selection and remote disclosure | voice-me-ui, voice-me-core, voice-me-tts-remote | AD-3, AD-6, AD-8, AD-9, AD-13 |

NEW:
> | FR-10 Backend selection, per-backend options and language, remote disclosure | voice-me-ui (Settings → Backend), voice-me-core, voice-me-tts-remote | AD-3, AD-6, AD-8, AD-9, AD-12, AD-13 |

### 4.3 UX — `ux-designs/ux-voice-me-2026-09-20/EXPERIENCE.md`

**Sections table.** One row is edited and one is added.

OLD:
> | Settings — Voice | Tray menu → Settings | Record/import Reference Voice Sample, pick speech language |

NEW:
> | Settings — Voice | Tray menu → Settings | Record/import Reference Voice Sample |
> | Settings — Backend | Tray menu → Settings | Local/Remote → backend; that backend's options: speech language, key, region, voice, sample state, GPU device |

**The paragraph under the table:** "four sections" becomes "five sections".

**Component rows.** Backend selector, API key field, Remote sample state and GPU device selector change location from "Settings → Dependencies" to "Settings → Backend". The backend selector row becomes:
> | Backend selector | Settings → Backend, top | Two steps: a Local/Remote choice, then a `Select` of that kind's backends (Local: bundled CPU and each added runtime's targets; Remote: DeepInfra, fal.ai, Azure — Azure tagged "stock voice"). Below it, only the selected backend's options. Beneath all, read-only, what the selection **actually acquired** (AD-9). |

**New component rows:**
> | Speech language selector | Settings → Backend, per backend | A `Select` of the languages the selected backend supports; each backend remembers its own. Independent of the UI language. |
> | Azure voice picker | Settings → Backend, Azure selected | Region `Input`, then a `Select` of the voices for the chosen locale, fetched on demand with the key (the one pre-disclosure request). States plainly: "Speech will be in this Microsoft voice, not yours." |

**State rows.** "CPU backend selected" and "Remote backend selected, no key" move to Settings → Backend. The capability row (a backend that can't run here) stays in Dependencies, since it blocks speech, with a link to the Backend tab. New state row:
> | Azure selected, no voice | Settings → Backend | "Pick a voice for Azure" inline; the Speak Action is blocked by Story 3.4's rules. |

**UI language selector row.**

OLD:
> …Independent of the TTS speech language set in Settings → Voice…

NEW:
> …Independent of each backend's speech language set in Settings → Backend…

### 4.4 Epics — `epics.md`

**Requirements inventory.**
- **FR5:** replace "…selected speech language are passed to the Inference Engine (…) producing audio in the user's cloned voice" with the new PRD FR-5 wording from §4.1, and keep the local-backend language limit as a local-only clause.
- **FR10:** add "…from Settings → Backend…, including Azure Neural TTS as a stock-voice provider".
- **UX-DR7:** OLD "four sections (Voice, Hotkey, Dependencies, General)", NEW "five sections (Voice, Hotkey, Backend, Dependencies, General)".
- **UX-DR13:** OLD "…independent of the TTS speech-language selection in Settings → Voice", NEW "…independent of each backend's speech language in Settings → Backend".
- **New UX-DR23:** "Settings → Backend is two-step (Local/Remote, then backend) and shows only the selected backend's options; a stock-voice backend is labelled 'stock voice' wherever it is selected."

**Epic 3 heading.** The description gains:
> …and to set, per backend, its speech language and options in a dedicated Settings → Backend tab — including Azure Neural TTS as a stock-voice remote backend.

The scope note gains:
> **2026-09-23 addendum:** backend choice and per-backend options move to Settings → Backend; speech language becomes per-backend; Azure Neural TTS (standard voices) is added. See `sprint-change-proposal-2026-09-23.md`.

**Retargeted stories.**
- **Story 3.7 (fal.ai):** AC "switching providers is the same backend-selection act as Story 3.5" gains "…in Settings → Backend (Story 3.10), with fal.ai's own supported speech languages (Story 3.11)".
- **Story 3.9 (GPU):** OLD "Settings → Dependencies lists the devices…", NEW "Settings → Backend lists the devices…".

**New stories:**

```
### Story 3.10: Give Backends Their Own Settings Tab

As Erdem,
I want backend choice and its options in a Settings → Backend tab of their own,
So that I pick a kind of backend, then one backend, and see only what that one needs.

**Acceptance Criteria:**

**Given** Settings is open
**When** I open the Backend tab
**Then** I choose Local or Remote first, then a backend of that kind (Local: the bundled CPU runtime and each added runtime's targets; Remote: DeepInfra, fal.ai, Azure)
**And** only the selected backend's options appear — added runtimes for Local; API key with its plaintext notice, and remote sample state for a cloning provider
**And** the Selected / Active lines and the "CPU mode" badge move here unchanged (AD-9, UX-DR18)
**And** Settings → Dependencies keeps only dependency rows and the speech-blocking capability row, which links to the Backend tab
**And** every behaviour of Stories 3.3, 3.5 and 3.6 is preserved — this story moves UI, it changes no backend logic
```

```
### Story 3.11: Choose the Speech Language per Backend

As Erdem,
I want to pick the speech language for each backend in Settings → Backend,
So that I don't hand-edit settings.toml and each backend offers only what it can speak.

**Acceptance Criteria:**

**Given** a backend is selected in Settings → Backend
**When** I open its speech language selector
**Then** it lists only that backend's languages — local ONNX: Turkish and English (AD-12); DeepInfra: the 23 languages of `ResembleAI/chatterbox-multilingual`
**And** the choice persists per backend through `SettingsStore` and applies to the next Speak Action with no restart; switching backends restores that backend's own choice
**And** core validates the Speak Action's language against the selected backend's set, not a global list
**And** an existing `speech_language` in settings.toml seeds the Local and DeepInfra choices on first load
**And** the UI language (Epic 4) is untouched by any of this, and vice versa
```

```
### Story 3.12: Generate Through Azure Neural TTS

As Erdem,
I want to use Azure's standard neural voices with my own key,
So that I have a fast remote option even when I don't need my cloned voice.

**Acceptance Criteria:**

**Given** I select Remote → Azure in Settings → Backend
**When** I enter my key and region and open the voice picker
**Then** the voice list is fetched from Azure with the key alone — the one request allowed before the disclosure (AD-13) — and filtered to the selected locale
**And** Azure is a stock-voice `SpeechProvider` in `voice-me-tts-remote` behind the same `TtsPort`; it never receives the Reference Voice Sample and shows no "sample held on provider" line
**And** before the first line, the one-time disclosure names Azure, lists the typed text, the language and the voice name, and says speech will be in a Microsoft voice, not mine; core enforces it (FR-10)
**And** generation requests SSML with `riff-24khz-16bit-mono-pcm`, which plays through the Virtual Microphone unchanged (AD-11)
**And** no key, no region, or no voice selected is a speech-blocking capability row naming what is missing; a provider failure (rejected key, timeout, provider error) is one notification naming Azure and the reason, never re-sent
**And** the key stays plaintext-with-notice and out of logs; the region is stored beside it
```

**Story 4.1.**

OLD:
> **And** the TTS speech language selection in Settings → Voice is unaffected by this change, and vice versa

NEW:
> **And** each backend's speech language in Settings → Backend is unaffected by this change, and vice versa

### 4.5 Sprint status — `implementation-artifacts/sprint-status.yaml`

Add, after `3-9-choose-which-gpu-runs-generation`:

```yaml
  3-10-give-backends-their-own-settings-tab: backlog
  3-11-choose-the-speech-language-per-backend: backlog
  3-12-generate-through-azure-neural-tts: backlog   # renumbered 3-14 by the instant-local addendum
```

The build order is 3.10 → 3.11 → 3.12, then 3.7 / 3.8 / 3.9.

---

## 5. Implementation Handoff

**Scope: Moderate.** The backlog is reorganized and the PRD's scope boundary changes, but no epic is replanned and no architecture decision is reversed. AD-13 widens; nothing is superseded.

| Who | Responsibility |
|---|---|
| Product owner (Erdem) | Approve this proposal, including decisions D1–D4 |
| Developer (correct-course) | Apply §4.1–§4.5 to the planning artifacts, and regenerate `epic-3-context.md` so the next `bmad-build` picks it up |
| Developer (`bmad-build`) | Build 3.10, then 3.11, then 3.12, one spec each |

**Success criteria:**
- The planning artifacts carry the edits above, with no remaining text that says stock-voice providers are out of scope for Azure, or that speech language lives in Settings → Voice.
- `sprint-status.yaml` lists 3.10–3.12 as backlog, and `bmad-build 3.10` resolves it without ambiguity.
- After 3.12: a line typed with Azure selected plays through the Virtual Microphone in the chosen stock voice. The Reference Voice Sample never leaves the machine for Azure.
