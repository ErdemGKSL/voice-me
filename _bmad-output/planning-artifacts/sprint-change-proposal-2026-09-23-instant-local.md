# Sprint Change Proposal (addendum) — Instant local system voices: eSpeak NG (Linux), WinRT (Windows)

**Date:** 2026-09-23
**Author:** Developer (correct-course), for Erdem (product owner)
**Mode:** Batch
**Builds on:** `sprint-change-proposal-2026-09-23.md` (Backend tab, per-backend language, Azure), approved earlier the same day
**Status:** Approved by Erdem, 2026-09-23 — edits applied to PRD, architecture spine, EXPERIENCE.md, epics.md and sprint-status.yaml

---

## 1. Issue Summary

**Trigger.** Straight after approving the Backend-tab proposal, the product owner asked for an **instant local backend** on each OS:

- **Linux:** eSpeak NG.
- **Windows:** the system's own speech engine. The product owner supplied a SAPI `SpVoice` snippet as a sketch.
- **macOS:** to be considered later.

**Category.** A new requirement from the stakeholder.

**Evidence and constraints, checked 2026-09-23 on the dev machine:**

- **eSpeak NG output.** eSpeak NG 1.52.0 is installed, Turkish voice `tr` included, 142 voices in total. It renders "Merhaba Erdem, bu yerel ve anında." to 2.68 s of audio with no perceptible wait. The output is **22 050 Hz, mono, 16-bit** WAV, so it must be resampled to AD-11's 24 kHz.
- **eSpeak NG license.** eSpeak NG is **GPL-3.0-or-later**. This repository has **no license file yet**, so linking it in-process would effectively decide voice-me's license.
  - **Decision:** run the `espeak-ng` program as a separate process.
- **The Windows sketch cannot be used as written.** `ISpeechVoice::Speak` plays to the **default speaker**, while voice-me must get the audio as a buffer for the Virtual Microphone (AD-11).
  - **Decision:** use WinRT `Windows.Media.SpeechSynthesis.SpeechSynthesizer`, from the same `windows` crate. It returns a WAV stream and sees the modern "OneCore" voices, including Turkish (Tolga) when the Turkish language pack is installed.
- **Windows cannot be verified here.**
  - There is no Windows toolchain on the dev machine; CI's Windows job compiles and tests.
  - The Windows app cannot start at all today: the tray is `todo!()` (existing deferred-work entry).
- **Order.** Decision: both stories come after 3.10 (Backend tab) and 3.11 (per-backend language), and **before Azure**.

---

## 2. Impact Analysis

| # | Item | Status | Finding |
|---|---|---|---|
| 1.1–1.3 | Trigger and evidence | [x] | See §1 |
| 2.1 | Current epic | [x] | Epic 3 can still be completed; it gains two stories |
| 2.2 | Epic-level changes | [!] | Stories 3.12 (eSpeak NG) and 3.13 (Windows) added; Azure renumbered 3.12 → 3.14 (unbuilt, added today) |
| 2.3–2.4 | Other epics | [N/A] | Epic 4 is unaffected; macOS remains out of v1 |
| 2.5 | Order | [!] | 3.10 → 3.11 → 3.12 → 3.13 → 3.14, then 3.7 / 3.8 / 3.9 |
| 3.1 | PRD | [!] | FR-5, FR-10, §6.1, §6.2 |
| 3.2 | Architecture | [!] | AD-2 (two new per-OS crates), AD-12 (a bounded child-process exception), AD-8 (unchanged: both offline), Stack, Structural seed, Capability map, Deferred (macOS) |
| 3.3 | UX | [!] | Backend tab: Local splits into "Chatterbox — your voice" and "System voice — instant"; a new state for "engine or voice for this language not installed" |
| 3.4 | Other | [!] | `sprint-status.yaml`; the `voice-me-tests` egress allowlist is unchanged (no network) |
| 4.1 | Direct adjustment | Viable | Effort Low–Medium each, risk Low (Linux) and Medium (Windows, unverifiable here) |
| 4.2–4.3 | Rollback / MVP review | Not needed | Additive only |

**Technical shape.** Both new backends are local and stock-voice: offline, no key, no disclosure, and they never receive the Reference Voice Sample.

- **`voice-me-tts-system-linux`:**
  - Implements `TtsPort` by running `espeak-ng -v <voice> --stdout`, with the text written to **stdin** (never argv) and a deadline enforced.
  - Parses the WAV and resamples 22 050 → 24 000 Hz.
  - Lists languages and voices by parsing `espeak-ng --voices`.
- **`voice-me-tts-system-windows`:**
  - Implements `TtsPort` with WinRT `SpeechSynthesizer::SynthesizeTextToStreamAsync`, running on a thread that owns its COM apartment.
  - Decodes the WAV stream and resamples it to 24 kHz.
  - Lists languages and voices via `SpeechSynthesizer::AllVoices`.
- **`voice-me-deps`:**
  - **Linux:** reports `espeak-ng` missing (not on `PATH`) as a dependency row with manual install steps. It cannot be installed automatically: it is a system package.
  - **Both OSes:** a selected language with no installed system voice is reported as a speech-blocking row with the OS's manual steps.
- **Core:** a `SystemVoice` local backend kind. `resolve_backend` must not treat it as an ONNX target.

---

## 3. Recommended Approach

**Direct adjustment:** two new stories in Epic 3, placed ahead of Azure.

Decisions, approved together with this addendum:

- **E1 — eSpeak NG runs as a separate process.** voice-me's future license stays open. AD-12's "no child process" rule remains for Chatterbox and gains one bounded exception:
  - a fixed binary name, resolved on `PATH`;
  - text passed on stdin;
  - a 10 s deadline;
  - no shell.
- **E2 — Windows uses WinRT `SpeechSynthesizer`**, not the SAPI `Speak` sketch (which plays to the speaker).
- **E3 — The language list comes from the engine** (eSpeak NG's voice list, or Windows' installed voices). When a language has more than one voice, a voice picker appears.
- **E4 — macOS is deferred.** It is recorded in Architecture → Deferred as "`AVSpeechSynthesizer.write(_:toBufferCallback:)` into a `voice-me-tts-system-macos` crate", so the Backend tab's "System voice" slot is ready for it.
- **E5 — Windows is compile-and-unit-test only in this story.** It is verified manually once the Windows app can start (Story 2.8 and the tray `todo!()`).

---

## 4. Detailed Change Proposals

### 4.1 PRD — `prd.md`

**FR-5.** The stock-voice sentence widens.

OLD:
> …a stock-voice backend (Azure Neural TTS) produces audio in the stock voice the user selected, and never receives the sample.

NEW:
> …a stock-voice backend — the local instant system voice (eSpeak NG on Linux, the Windows speech engine on Windows) or Azure Neural TTS — produces audio in the stock voice the user selected, and never receives the sample.

**FR-10.**

OLD:
> Local backends (CPU, CUDA, WebGPU) run entirely on the machine.

NEW:
> Local backends run entirely on the machine: Chatterbox in the user's cloned voice (CPU, CUDA, WebGPU), and an instant system voice (eSpeak NG on Linux, the Windows speech engine on Windows) that needs no model download.

**§6.1 In Scope.** Add:
> - An instant local system-voice backend: eSpeak NG on Linux, the Windows speech engine on Windows (FR-5, FR-10)

**§6.2 Out of Scope.** Add:
> - A macOS system-voice backend — follows macOS support itself (deferred)

### 4.2 Architecture — `ARCHITECTURE-SPINE.md`

**AD-2.** Append:
> The instant system-voice backend follows the same rule: `voice-me-tts-system-linux` (eSpeak NG) and `voice-me-tts-system-windows` (WinRT `SpeechSynthesizer`), wired in the composition root; a macOS crate joins them when macOS does.

**AD-8.** Append to the offline list:
> …`voice-me-tts-system-linux` and `voice-me-tts-system-windows` open no socket.

**AD-12.** Add a bullet:
> - **One bounded child-process exception (2026-09-23):** `voice-me-tts-system-linux` runs the system's `espeak-ng` program — resolved by fixed name on `PATH`, text on stdin never argv, no shell, a 10 s deadline — because eSpeak NG is GPL-3.0 and a separate process keeps voice-me's own licence undecided. This rule's "no child process" still binds Chatterbox.

**Stack.** Add rows:
> | eSpeak NG | system package, run as a process — Linux instant backend (GPL-3.0, not linked) |
> | `windows` crate, `Media_SpeechSynthesis` | WinRT `SpeechSynthesizer` — Windows instant backend |

**Capability map, FR-5 row.** Add `voice-me-tts-system-linux, voice-me-tts-system-windows` to "Lives in", and add AD-2.

**Deferred.** Add:
> - macOS system voice — `AVSpeechSynthesizer.write(_:toBufferCallback:)` in a `voice-me-tts-system-macos` crate, when macOS is in scope.

### 4.3 UX — `EXPERIENCE.md`

**Backend selector row.**

OLD:
> (Local: bundled CPU and each added runtime's targets; …)

NEW:
> (Local: "Chatterbox — your voice" with the bundled CPU and each added runtime's targets, and "System voice — instant" — eSpeak NG on Linux, Windows voices on Windows, tagged "stock voice"; …)

**New state row:**
> | System voice unavailable | Settings → Backend, and a speech-blocking row in Dependencies | Linux: "eSpeak NG is not installed" with the distro install command; either OS: "No system voice for Turkish" with the OS's steps to add one. Never a silent switch to another voice or language. |

### 4.4 Epics — `epics.md`

**Renumbering.** "Story 3.12: Generate Through Azure Neural TTS" becomes **Story 3.14**, and the references to it are updated.

**Epic 3 scope-note addendum.** Append:
> Instant local system voices are added too — eSpeak NG on Linux, the Windows speech engine on Windows; macOS later. See `sprint-change-proposal-2026-09-23-instant-local.md`.

**FR5 / FR10 inventory lines.** Mirror the PRD edits in §4.1.

**New stories, inserted before Azure:**

```
### Story 3.12: Speak Instantly With eSpeak NG on Linux

As Erdem,
I want an instant local voice on Linux,
So that a line is spoken the moment I press Enter, even without the model download or a key.

**Acceptance Criteria:**

**Given** I select Local → System voice in Settings → Backend on Linux
**When** I perform a Speak Action
**Then** `voice-me-tts-system-linux` generates it by running `espeak-ng` (fixed name on PATH, text on stdin, no shell, 10 s deadline) behind the same `TtsPort`, resampled to 24 kHz mono f32 and played through the Virtual Microphone unchanged (AD-11, AD-12)
**And** the speech language list is eSpeak NG's own, and a voice picker appears when a language has more than one voice (Story 3.11)
**And** it is labelled "stock voice", never receives the Reference Voice Sample, opens no socket and needs no disclosure (AD-8)
**And** `espeak-ng` missing from PATH is a dependency row with the distro's install command as manual steps, and blocks speech while this backend is selected (Story 3.4)
**And** a failure (non-zero exit, timeout, unreadable output) is one notification naming eSpeak NG and the reason
```

```
### Story 3.13: Speak Instantly With the Windows Speech Engine

As Erdem,
I want an instant local voice on Windows,
So that Windows gets the same no-download, no-key option Linux has.

**Acceptance Criteria:**

**Given** I select Local → System voice in Settings → Backend on Windows
**When** I perform a Speak Action
**Then** `voice-me-tts-system-windows` generates it with WinRT `SpeechSynthesizer::SynthesizeTextToStreamAsync` — never to the speaker — decoded and resampled to 24 kHz mono f32 (AD-11)
**And** the speech language and voice lists come from the installed voices (`SpeechSynthesizer::AllVoices`); a language with no installed voice is a speech-blocking row naming Windows' steps to add one
**And** it is labelled "stock voice", never receives the Reference Voice Sample, opens no socket and needs no disclosure
**And** the crate compiles and its unit tests pass on CI's Windows job; manual verification waits until the Windows app can start (Story 2.8, tray)
```

### 4.5 Sprint status

After `3-11-choose-the-speech-language-per-backend`:

```yaml
  3-12-speak-instantly-with-espeak-ng-on-linux: backlog
  3-13-speak-instantly-with-the-windows-speech-engine: backlog
  3-14-generate-through-azure-neural-tts: backlog
```

This replaces `3-12-generate-through-azure-neural-tts`, which is unbuilt.

---

## 5. Implementation Handoff

**Scope: Moderate.** This adds two stories, renumbers one unbuilt story, and adds one architecture exception (AD-12).

| Who | Responsibility |
|---|---|
| Erdem | Approve this addendum, including E1–E5 |
| Developer (correct-course) | Apply §4.1–§4.5; `epic-3-context.md` regenerates on the next `bmad-build` |
| Developer (`bmad-build`) | 3.10 → 3.11 → 3.12 → 3.13 → 3.14 |

**Success criteria:**
- On this Linux machine, with System voice selected, a Turkish line is heard through the Virtual Microphone with no perceptible generation wait.
- No voice-me crate links libespeak-ng.
- The Windows crate builds and passes its tests on CI's Windows job.
