# Sprint Change Proposal — On-device Piper TTS (neural, ONNX, CPU)

**Date:** 2026-09-24
**Author:** Developer (correct-course), for Erdem (product owner)
**Mode:** Batch
**Builds on:** `sprint-change-proposal-2026-09-23-instant-local.md` (eSpeak NG, Story 3.12 — now in review)
**Status:** Approved by Erdem, 2026-09-24 — edits applied to PRD, architecture spine, EXPERIENCE.md, epics.md, sprint-status.yaml, epic-3-context.md and deferred-work.md

**Revision (2026-09-24, on merging to `main`):** Azure was built as Story 3.14 on `main` while this proposal was open. The planned renumbering (Windows engine 3.13 → 3.14, Azure 3.14 → 3.15) is dropped. Piper on Linux becomes **3.15** and is still next up. Piper on Windows stays **3.16**.

---

## 1. Issue Summary

**Trigger.** With Story 3.12 (eSpeak NG) in review, the product owner asked for an **on-device Piper TTS backend**: Piper is the fastest neural option, runs on ONNX, and is light enough that CPU-only is fine.

**Category.** A new requirement from the stakeholder.

**The gap it fills.** Today's two local choices sit at opposite ends:

- **Chatterbox:** it speaks in the user's own voice, but takes ~20 s per line on CPU (RTF 0.10×) and needs a 1.56 GB download.
- **eSpeak NG:** it is instant, but it sounds robotic.

Piper is a neural stock voice that is nearly as fast as eSpeak NG and sounds natural.

**Evidence, measured 2026-09-24 in the cloud dev container** (4 vCPU Intel Xeon @ 2.10 GHz, CPU only, `piper-tts` 1.8.0 on `onnxruntime` 1.30.0, used only as a measuring tool):

| Voice | Load (once) | Line | Audio | Synthesis | RTF |
|---|---|---|---|---|---|
| `tr_TR-dfki-medium` | 1.26 s | "Merhaba Erdem, bu yerel ve anında." | 2.24 s | **0.081 s** | 0.036 |
| `tr_TR-dfki-medium` | 1.43 s | "Sağ tarafa geçin, arkadan geliyorlar, dikkatli olun." | 3.41 s | **0.122 s** | 0.036 |
| `en_US-lessac-medium` | 1.11 s | "Hello Erdem, this is local and instant." | 2.43 s | **0.086 s** | 0.035 |

- **A voice is one ONNX file.** Each voice is one VITS graph (`<voice>.onnx`, 63 MB at "medium") plus a small `<voice>.onnx.json`. The JSON holds the sample rate (22 050 Hz), the eSpeak voice (`tr`) and the `phoneme_id_map`. It runs on the ONNX Runtime CPU library that `voice-me-deps` already provisions (8.7 MB). There is no new runtime and no execution provider.
- **Phonemes come from eSpeak NG.** Piper takes eSpeak NG IPA phonemes as input. Running `espeak-ng -q -v tr --ipa` (1.51) on both Turkish lines gave **exactly the same phoneme sequence** as Piper's own in-library phonemizer.
  - The one difference is punctuation. The CLI turns clause breaks (`,` `.` `!`) into newlines, while Piper keeps them as phoneme ids. The adapter splits the text at clause punctuation and puts the mark back after each clause's phonemes.
  - Do **not** use `--ipa=3`: it inserts U+200D tie characters (`t‍ʃ`) that are not in the phoneme map.
  - So Story 3.12's AD-12 child-process exception covers phonemization as it is. voice-me links no GPL code.
- **Licences:**
  - **Engine:** the maintained engine (`OHF-Voice/piper1-gpl`) is **GPL-3.0**, and the original `rhasspy/piper` is MIT but archived. voice-me uses **neither**: it runs the voice's ONNX graph through its own `ort`, as it already does for Chatterbox.
  - **Voices:** each voice carries its own licence. **The only Turkish voice in `rhasspy/piper-voices` (`tr_TR-dfki-medium`, 1 of 177 voices) is CC BY-NC-SA 4.0**, which is non-commercial, share-alike, with attribution. That suits a free, open-source hobby app, but it is shown to the user and the voice is **not mirrored**. It is fetched from Hugging Face at a pinned revision, which AD-7 already allows.
- **Output format.** Output is 22 050 Hz mono int16. Like eSpeak NG, it is resampled to AD-11's 24 kHz mono f32 inside the adapter.

---

## 2. Impact Analysis

| # | Item | Status | Finding |
|---|---|---|---|
| 1.1–1.3 | Trigger and evidence | [x] | See §1 |
| 2.1 | Current epic | [x] | Epic 3 can still be completed. It gains two stories |
| 2.2 | Epic-level changes | [!] | New Story 3.15 (Piper on Linux, next up) and Story 3.16 (Piper on Windows). No existing story is renumbered |
| 2.3–2.4 | Other epics | [N/A] | Epic 1: Story 1.5's first-run voice-setup prompt already stays quiet for stock-voice backends (3.12). Epic 4 is unaffected |
| 2.5 | Order | [!] | **3.15 Piper** next (3.14 Azure is already built), then 3.13 Windows engine, **3.16 Piper on Windows**, then 3.7 / 3.8 / 3.9 |
| 3.1 | PRD | [!] | FR-5, FR-7, FR-10 (**first-run default becomes Piper**), §6.1, §6.2, Glossary |
| 3.2 | Architecture | [!] | AD-7 (backend table), AD-8, **AD-9 (the unset default)**, AD-12 (exception widened to phonemization, one shared runner), Stack, Structural seed, Capability map |
| 3.3 | UX | [!] | The Backend selector gains "Piper — natural, instant"; the voice picker shows each voice's licence; new state rows |
| 3.4 | Other | [!] | `sprint-status.yaml`, `epic-3-context.md` (story list and Piper decisions), `deferred-work.md` (unchanged). The `voice-me-tests` egress allowlist does not change |
| 4.1 | Direct adjustment | Viable | Effort: Medium. Risk: Low on Linux (measured above) and Medium on Windows (cannot be verified here) |
| 4.2–4.3 | Rollback / MVP review | Not needed | Additive. The only behaviour change is the first-run default |

**Technical shape:**

- **`voice-me-tts-piper`** (new lib crate, cross-platform, offline) implements `TtsPort`:
  1. Split the text into clauses at `, . ! ? ; :`.
  2. Phonemize each clause with the shared eSpeak NG runner (`--ipa`, text on stdin), then map the IPA characters through the voice's `phoneme_id_map`: BOS `^`, a pad `_` after each id, and EOS `$`. Put each clause's punctuation id back after its phonemes.
  3. Run the VITS graph on `ort` (CPU execution provider). Inputs are `input`, `input_lengths` and `scales` (noise 0.667, length 1.0, noise_w 0.8, all taken from the voice JSON).
  4. Resample 22 050 → 24 000 Hz mono f32.

  The `ort` session is built once per selected voice and held (AD-10). Changing the voice rebuilds it (~1.3 s).
- **The eSpeak NG process runner moves into a shared lib crate, `voice-me-espeak`.** It resolves the fixed name on `PATH`, passes text on stdin, uses no shell and enforces a 10 s deadline. Today it lives in `voice-me-tts-system-linux/src/process.rs`. Both `voice-me-tts-system-linux` (audio) and `voice-me-tts-piper` (phonemes) call it. It is a utility, not a port adapter, so AD-1 still holds, and the AD-12 exception stays in **one** place.
- **`voice-me-deps`:**
  - A pinned **voice table** holds the voice id, locale, quality, URL at a pinned `rhasspy/piper-voices` revision, size, SHA-256 of both files, and licence. It starts with `tr_TR-dfki-medium` and a few English "medium" voices. Adding voices means adding rows.
  - The selected voice is a one-click row: `.part`, SHA-256 check, then atomic rename, like the Chatterbox weights. The ONNX Runtime CPU library row is shared with Chatterbox.
  - A missing `espeak-ng` is the same manual-steps row as in 3.12.
- **Core:**
  - Add a `Piper` local backend kind. It is resolved as an ONNX target that always uses the CPU execution provider, and it never asks for Chatterbox weights or GPU libraries.
  - Its speech-language set is the locales in the voice table. A voice picker appears when a locale has more than one voice.
  - **When no backend is selected, Piper is the choice wherever it is built for this OS.** Until Story 3.16, that means Linux only. Elsewhere the default stays CPU Chatterbox.

---

## 3. Recommended Approach

**Direct adjustment:** two new stories in Epic 3. Piper on Linux goes next, and Piper on Windows goes last.

Decisions (P1–P3 by Erdem on 2026-09-24, P4–P7 recommended with this proposal):

- **P1 — Linux now, Windows later.** Story 3.15 ships Piper on Linux. Story 3.16 adds Windows, where `voice-me-deps` provisions eSpeak NG (the official release, run as a separate program — see P5). That story is compile-and-unit-test on CI only, like 3.13.
- **P2 — Piper is next.** It is numbered **3.15** because Azure was built as 3.14 while this proposal was open (see the revision note), but it is the next story to build.
- **P3 — Piper becomes the first-run default** where it is built. A new user hears a natural voice after a ~72 MB download (the 63 MB voice plus the 8.7 MB runtime), not a 1.56 GB one. Chatterbox, which is "your voice", is one selection away.
  - **Consequence:** a `settings.toml` with no backend selected now resolves to Piper, not CPU Chatterbox. An explicit selection is never rewritten.
  - The default voice matches the speech language: `tr_TR-dfki-medium` for Turkish.
- **P4 — Run the ONNX graph ourselves, never the Piper engine.** voice-me uses no `piper1-gpl` or `piper-phonemize` code. Phonemes come from the eSpeak NG program (AD-12 exception), and inference runs through the existing `ort`. voice-me's licence stays undecided.
- **P5 — Voices are fetched from the Hugging Face origin at a pinned revision and not mirrored.** Their licences vary (Turkish: CC BY-NC-SA 4.0). The licence and attribution are shown in the voice picker. This follows AD-7's "any stable, versioned, resumable URL".
- **P6 — CPU only.** Piper gets no GPU backend or device picker. At RTF ~0.04 there is nothing to gain.
- **P7 — Voice catalogue is a pinned, curated table in `voice-me-deps`, not a live fetch of `voices.json`.** That keeps SHA-256 pins in code, as the epic context requires, and it keeps the voice list offline.
  **Reversed by the addendum below (2026-09-24).**

---

## 4. Detailed Change Proposals

### 4.1 PRD — `prd.md`

**Glossary.** Add:
> - **Piper** — an on-device neural text-to-speech voice (VITS, one ONNX file per voice) that voice-me runs on the CPU through its own ONNX Runtime; a stock voice, not the user's cloned one.

**FR-5.** The stock-voice sentence widens.

OLD:
> …a stock-voice backend — the local instant system voice (eSpeak NG on Linux, the Windows speech engine on Windows) or Azure Neural TTS — produces audio…

NEW:
> …a stock-voice backend — Piper (local neural voices), the local instant system voice (eSpeak NG on Linux, the Windows speech engine on Windows) or Azure Neural TTS — produces audio…

**FR-7.** In the checked-components list, after "the Chatterbox model weights", add:
> , the selected Piper voice

**FR-10.**

OLD:
> Local backends run entirely on the machine: Chatterbox in the user's cloned voice (CPU, CUDA, WebGPU), and an instant system voice (eSpeak NG on Linux, the Windows speech engine on Windows) that needs no model download.

NEW:
> Local backends run entirely on the machine: Chatterbox in the user's cloned voice (CPU, CUDA, WebGPU); Piper, natural neural stock voices on the CPU, each a single small voice download; and an instant system voice (eSpeak NG on Linux, the Windows speech engine on Windows) that needs no model download.

FR-10 consequence, first bullet.

OLD:
> - A local backend is the default on first run; no network call is possible until the user selects a remote backend and enters a key.

NEW:
> - A local backend is the default on first run — Piper, where this OS has it (Linux; Windows from Story 3.16), otherwise Chatterbox on the CPU; no network call is possible until the user selects a remote backend and enters a key. (Downloading the selected local voice or weights is dependency provisioning, FR-7, not generation.)

**§6.1 In Scope.** Add:
> - Piper on-device neural voices, CPU-only, the first-run default (FR-5, FR-7, FR-10)

**§6.2 Out of Scope.** Add:
> - GPU acceleration for Piper — not needed at its speed
> - Training or importing custom Piper voices — only voices in voice-me's pinned voice table

**§9 Assumptions Index.** Add:
> - §4.6 FR-10 — Piper voice licences vary per voice (the Turkish voice is CC BY-NC-SA 4.0); acceptable for a free, open-source app, shown to the user, and revisited if voice-me is ever monetized.

### 4.2 Architecture — `ARCHITECTURE-SPINE.md`

**AD-7, the backend table.** Add a row:
> | Piper | the same core CPU library, no provider libs | one voice, ~63 MB (`<voice>.onnx` + `.onnx.json`) from `rhasspy/piper-voices` at a pinned revision, not mirrored (per-voice licences) | `voice-me-tts-piper`; CPU execution provider only |

**AD-8.** Append to the offline list:
> …and `voice-me-tts-piper` and `voice-me-espeak` open no socket.

**AD-9, rule 1.**

OLD:
> Unset means the CPU backend with no device preference — the safe floor, not a guess.

NEW:
> Unset means **Piper** where this OS's build has it, otherwise the CPU backend with no device preference (revised 2026-09-24, product-owner decision). An explicit selection is never rewritten.

**AD-12, the child-process exception bullet.** Append:
> Piper (2026-09-24) uses the same program for **phonemization** (`--ipa`, never `--ipa=3`, whose tie characters are not in Piper's phoneme map). The runner lives in exactly one crate, `voice-me-espeak`, which both `voice-me-tts-system-linux` and `voice-me-tts-piper` call; no other crate spawns a process. Piper's VITS graph runs in-process on `ort` like Chatterbox; no Piper engine code (`piper1-gpl`, GPL-3.0; `piper-phonemize`) is linked.

**Stack.** Add a row:
> | Piper voices (external) | `rhasspy/piper-voices` at a pinned revision — VITS ONNX, 22 050 Hz, per-voice licence; run on the pinned `ort`/ONNX Runtime CPU |

**Structural seed.** Add:
```text
    voice-me-tts-piper/         # lib: TtsPort adapter - Piper VITS voices on ort (CPU), phonemes via voice-me-espeak
    voice-me-espeak/            # lib: the one espeak-ng process runner (AD-12 exception) - audio for system-linux, IPA for piper
```

**Capability map, FR-5 row.** Add `voice-me-tts-piper, voice-me-espeak`.

### 4.3 UX — `EXPERIENCE.md`

**Backend selector row.**

OLD:
> (Local: "Chatterbox — your voice" with the bundled CPU and each added runtime's targets, and "System voice — instant" — …

NEW:
> (Local: "Piper — natural, instant", tagged "stock voice", listed first and the first-run default where available; "Chatterbox — your voice" with the bundled CPU and each added runtime's targets; and "System voice — instant" — …

**New state rows:**
> | Piper voice not downloaded | Settings → Backend, and a speech-blocking row in Dependencies | "Turkish voice (dfki, 63 MB) is not downloaded" with Install and a progress bar; the voice picker shows each voice's size and licence ("CC BY-NC-SA 4.0 — non-commercial") beside its name. |
> | Piper phonemizer missing | Dependencies | The same "eSpeak NG is not installed" row as System voice, noting that Piper needs it to read text. |

### 4.4 Epics — `epics.md`

**No renumbering.** In Story 3.10, "System voice from Story 3.12/3.13" becomes "System voice from Story 3.12/3.13, and Piper from 3.15/3.16".

**Epic 3 scope note (both copies).** Append:
> **2026-09-24 addendum:** Piper on-device neural voices are added — CPU-only on the shared ONNX Runtime, phonemes from the eSpeak NG program — and become the first-run default where available (Linux now, Windows in 3.16). See `sprint-change-proposal-2026-09-24-piper.md`.

**FR5 / FR10 inventory lines.** Mirror the PRD edits in §4.1.

**New story, appended after Azure (3.14):**

```
### Story 3.15: Speak Naturally and Instantly With Piper on Linux

As Erdem,
I want a natural-sounding on-device voice that speaks as fast as eSpeak NG,
So that I get a usable voice from the first run without a 1.56 GB download or a 20-second wait.

**Acceptance Criteria:**

**Given** I select Local → Piper in Settings → Backend on Linux, or no backend was ever selected (first run)
**When** I perform a Speak Action
**Then** `voice-me-tts-piper` phonemizes the text through `voice-me-espeak` (the one eSpeak NG runner: fixed name on PATH, `--ipa`, text on stdin, no shell, 10 s deadline), maps the phonemes through the voice's `phoneme_id_map` with clause punctuation kept, runs the voice's VITS graph in-process on `ort` with the CPU execution provider, and returns 24 kHz mono f32 played through the Virtual Microphone unchanged (AD-11, AD-12)
**And** the `ort` session is built once per selected voice and held; a line of ~3 s is ready in well under a second on the dev machine (measured reference: 0.08–0.12 s on 4 vCPU)
**And** the speech languages are the locales in `voice-me-deps`' pinned voice table (Turkish `tr_TR-dfki-medium` first), with a voice picker when a locale has more than one voice; each voice shows its size and licence
**And** the selected voice missing is a one-click dependency row (pinned Hugging Face revision, SHA-256-checked, resumable, not mirrored); the ONNX Runtime CPU library row is shared with Chatterbox; `espeak-ng` missing is the manual-steps row from 3.12 — each blocks speech while Piper is selected (Story 3.4)
**And** it is labelled "stock voice", never receives the Reference Voice Sample, opens no socket and needs no disclosure (AD-8)
**And** with no backend in settings, core resolves Piper (AD-9); an explicit selection is never changed; Settings does not auto-open for a missing Reference Voice Sample while Piper is selected
**And** `voice-me-tts-system-linux` now uses `voice-me-espeak` too, with Story 3.12's behaviour unchanged
**And** a failure (phonemizer, unknown phoneme, inference) is one notification naming Piper and the reason
```

**New story, appended after 3.15:**

```
### Story 3.16: Speak With Piper on Windows

As Erdem,
I want Piper on Windows as well,
So that Windows gets the same natural, instant default as Linux.

**Acceptance Criteria:**

**Given** I select Local → Piper in Settings → Backend on Windows, or no backend was ever selected
**When** I perform a Speak Action
**Then** the same `voice-me-tts-piper` generates it, with `voice-me-espeak` running the eSpeak NG program on Windows (still a separate process, never linked)
**And** `voice-me-deps` provisions eSpeak NG on Windows with one click from the official eSpeak NG release (pinned URL and SHA-256), and `voice-me-espeak` finds it there as well as on PATH
**And** the first-run default on Windows becomes Piper (AD-9)
**And** the crates compile and their unit tests pass on CI's Windows job; manual verification waits until the Windows app can start (Story 2.8, tray)
```

### 4.5 Sprint status — `sprint-status.yaml`

After `3-14-generate-through-azure-neural-tts`, add:
```yaml
  3-15-speak-naturally-and-instantly-with-piper-on-linux: backlog  # next up (sprint change 2026-09-24)
  3-16-speak-with-piper-on-windows: backlog
```

### 4.6 Implementation context

- **`epic-3-context.md`:**
  - Update the story list.
  - Add the Piper decisions: P4 (graph on `ort`, no Piper engine), P5 (pinned origin, not mirrored, licence shown), P7 (curated voice table), and the `--ipa` / punctuation detail.
  - Change "If nothing is selected, the backend is CPU" to the AD-9 wording above.
  - "Stories 3.12, 3.13 and 3.14 add entries" becomes "3.12–3.16".
- **Built specs are not rewritten.** Spec files for finished stories (3.10, 3.11, 3.12) keep their historical numbering.

---

## 5. Implementation Handoff

**Scope: Moderate.** The change adds two stories, widens one architecture exception (AD-12) and changes one default (AD-9).

| Who | Responsibility |
|---|---|
| Erdem | Approve this proposal, including P4–P7 |
| Developer (correct-course) | Apply §4.1–§4.6 |
| Developer (`bmad-build`) | 3.15 (Piper on Linux) next, then 3.13 and 3.16 on Windows |

**Success criteria:**

- On the Linux dev machine, a fresh profile with no settings resolves to Piper. After one-click installs (voice + runtime) and eSpeak NG on `PATH`, a Turkish line is heard through the Virtual Microphone in well under a second.
- No voice-me crate links libespeak-ng, `piper1-gpl` or `piper-phonemize`. Only `voice-me-espeak` spawns a process.
- The phonemes `voice-me-tts-piper` produces for the §1 Turkish lines match Piper's reference sequence, which is pinned as a test fixture.
- The Windows crates build and pass their tests on CI's Windows job (3.16).

---

## Addendum (2026-09-24): the Piper voices tab, three catalogs, fahrettin default

Agreed while specifying Story 3.15; supersedes P7 and the `tr_TR-dfki-medium` default.

- **P7 reversed — voices come from three live catalogs, browsed in a new Settings → Piper voices tab.** In precedence order (the first source wins a duplicate voice key): voice-me's own `piper-voices/catalog.json` in this repository (URL, size and SHA-256 per file; adding a voice is an edit of that file, no release needed), `rhasspy/piper-voices`' `voices.json` at pinned revision `c10ece1aade47bb51c153c893d14e5bf8e5b7117` (size and MD5), and the `speaches-ai` Hugging Face repositories (`models?author=speaches-ai&search=piper-`; at download, `?blobs=true` gives the revision, the LFS SHA-256 of `model.onnx` and the Git blob SHA-1 of `config.json`). Each file is checked with the digest its source publishes. Catalogs are fetched only when the tab is opened or refreshed, never at startup.
- **Default voice: `tr_TR-fahrettin-medium` (CC0, NabuCasa dataset)** from `speaches-ai/piper-tr_TR-fahrettin-medium` at `aab8f92429ede58091e17de506484a2c84384792`, SHA-256 of both files pinned in code, so a first run needs no catalog. `tr_TR-dfki-medium` (CC BY-NC-SA 4.0) becomes an optional download.
- **Storage:** `<cache>/piper/<key>/{model.onnx, config.json, voice.toml}`; the installed list is read from disk and supplies Piper's languages and voices.
- **Tab:** search and language filter; rows with name, language, quality, size, source, licence ("see model card" when none is declared) and status; Download (progress), Delete, Use. The Backend tab's Piper pickers list installed voices only, with a "Manage voices" link.
