---
id: SPEC-voice-me
companions:
  - glossary.md
  - ../../planning-artifacts/architecture/architecture-voice-me-2026-09-20/ARCHITECTURE-SPINE.md
  - ../../planning-artifacts/prds/prd-voice-me-2026-09-20/addendum.md
sources:
  - ../../planning-artifacts/prds/prd-voice-me-2026-09-20/prd.md
  - ../../planning-artifacts/briefs/brief-voice-me-2026-09-20/brief.md
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete, preservation-validated contract for what to build, test, and validate. Source documents listed in frontmatter are for traceability — consult them only if you need narrative rationale or prose color this contract intentionally omits.

# SPEC: voice-me

## Why

A vision to realize, for its own builder first: Erdem wants to "speak" in voice chat — mid-game, mid-call — without speaking out loud, when a shared room, a sore throat, or simply not wanting to say something live rules that out. Typed text chat is too slow and easily missed in fast gameplay; staying muted forfeits real coordination. voice-me closes that gap with a global hotkey, a cloned voice, and a virtual microphone, entirely on the user's own machine.

## Capabilities

- **CAP-1 — Voice Setup**
  - **intent:** The user can record their own voice directly in the app, or import an existing audio file, to use as their Reference Voice Sample.
  - **success:** A recorded or imported clip is saved and selectable as the active Reference Voice Sample; the user can re-record or replace it at any time.

- **CAP-2 — Background/tray operation**
  - **intent:** The app runs as a background/tray-resident process between uses, always ready for the next hotkey press.
  - **success:** After the Prompt Overlay is dismissed, the app stays active and responds to the next hotkey without relaunching; it is reachable via a system tray/menu-bar presence.

- **CAP-3 — Global hotkey configuration**
  - **intent:** The user can assign and change a single global hotkey that summons the Prompt Overlay from anywhere, including while a game has input focus.
  - **success:** The hotkey works with a fullscreen or borderless-fullscreen game focused, on both Linux and Windows; assigning a combination already in use elsewhere is surfaced, not silently dropped.

- **CAP-4 — Prompt Overlay summon, type, dismiss**
  - **intent:** Pressing the hotkey opens a minimal overlay; pressing Enter closes it instantly and triggers speaking the typed line; Escape cancels without speaking.
  - **success:** The overlay appears without perceptible delay; it closes immediately on Enter, before generation completes; Escape or losing focus discards the typed text.

- **CAP-5 — TTS generation via the cloned voice**
  - **intent:** The typed text, the active Reference Voice Sample, and the selected speech language produce spoken audio in the user's own voice.
  - **success:** Generated audio is in the selected speech language; a generation failure surfaces a clear in-app failure, never silent nothing.
  - **how:** the model runs in-process on ONNX Runtime — no Python runtime and no separate process (Architecture Spine AD-12).

- **CAP-6 — Playback through the Virtual Microphone**
  - **intent:** Generated audio plays through a Virtual Microphone device that other applications can select as their microphone input.
  - **success:** An application selecting the Virtual Microphone on Linux or Windows receives the generated audio, timed to the Speak Action; the user's real physical microphone is untouched.

- **CAP-7 — Dependency Check and provisioning**
  - **intent:** The app detects locally missing runtime dependencies and offers one-click provisioning from inside its own UI.
  - **success:** A missing dependency is named specifically, with either a one-click fix or clear manual steps; the app degrades to a CPU-only mode rather than failing outright when no GPU is available.

- **CAP-8 — Turkish/English UI**
  - **intent:** The interface is available in Turkish and English, independent of the TTS speech language.
  - **success:** Switching UI language takes effect without a restart; UI language and speech language can differ.

- **CAP-9 — Modern, clean visual design**
  - **intent:** The interface presents a modern, clean, consistent look and feel across every screen, reading as a considered product rather than a bare dev tool.
  - **success:** Every screen uses the adopted component library rather than one-off custom widgets; visual decisions are checked against the adopted design guide (see Constraints) before being called done.

## Constraints

- Local-only inference; no cloud calls, no accounts, no telemetry — network egress is limited to `voice-me-deps`'s declared dependency fetches (see Architecture Spine AD-8).
- v1 targets Linux and Windows only; macOS is explicitly deferred.
- Free and open source; GitHub Actions CI (separate Linux/Windows build jobs), with distribution and runtime-asset hosting via this repo's own GitHub Releases (see Architecture Spine AD-7).
- UI is implemented with gpui-kit components, not raw GPUI primitives.
- Rust workspace on a hexagonal/ports-and-adapters paradigm; per-OS capabilities (hotkey, virtual mic, tray) ship as separate crates; a single `AppState`/`AppEvent` model owns cross-crate state — full detail in the adopted Architecture Spine companion (AD-1, AD-2, AD-3).

## Non-goals

- Live, continuous voice-changing — this is discrete typed-line playback, not real-time voice pass-through.
- macOS support in v1.
- Preset Phrase (a hotkey bound directly to a fixed spoken phrase, no overlay) in v1 — planned for a later version.
- Cloud or hybrid inference.
- Accounts, licensing, monetization.
- Speech languages beyond what Chatterbox-Multilingual V3 supports natively — and, in v1, further narrowed to the languages whose text needs no Python-only normalization, which excludes Chinese, Japanese, Hebrew and Korean but keeps Turkish and English (Architecture Spine AD-12).
- Watermarking generated audio in v1: Resemble's optional Perth watermarker is Python-only and has no Rust/ONNX equivalent (Architecture Spine AD-12).

## Success signal

Erdem uses voice-me during real gaming sessions at least weekly, instead of falling back to text chat or staying muted, and end-to-end latency (hotkey press to audio heard) feels usable in live voice chat — judged from that real use, not a lab benchmark.

## Assumptions

- Minimum/maximum Reference Voice Sample length and supported import formats are not yet fixed; to be set from Chatterbox's own guidance during implementation. Whatever the clip's source format, it is decoded and resampled to 24 kHz mono f32 before reaching the speech encoder.
- Exact tray UX (icon, menu contents) per OS is not yet designed.
- Prompt Overlay hotkey-to-ready-for-input latency target is under ~200ms; no measured baseline yet.
- gpui-kit is assumed to cover enough standard components (buttons, inputs, dialogs) to avoid hand-rolled UI for most of the app; not verified against gpui-kit's actual component set.

## Open Questions

- What end-to-end latency (hotkey press to audio playing) is actually achievable with Chatterbox-Multilingual V3 on typical consumer hardware, with and without a GPU? In-process ONNX Runtime removes process-startup cost but not the autoregressive decode loop itself, and the CPU path runs a Q4-quantized backbone.
- Does the chosen Windows virtual audio driver (`VirtualDrivers/Virtual-Audio-Driver`, release 25.7.14) support named-pipe/IPC control out of the box, or does that require a custom build from the maintainer?
- Does global hotkey capture during fullscreen games risk conflicts with anti-cheat software (e.g. BattlEye, EasyAntiCheat) in any target games?
- What are Chatterbox-Multilingual V3's actual minimum reference-clip length/quality recommendations?
- ~~Is the bundled-Python-sidecar packaging approach actually deliverable as a clean single-executable experience?~~ **Resolved 2026-09-21 by dropping the premise:** Chatterbox-Multilingual V3 ships a complete ONNX export (MIT), so inference runs in-process via the `ort` crate and there is no Python to package (Architecture Spine AD-12). The packaging question that replaces it is smaller: how the ONNX Runtime shared library and its GPU execution providers get provisioned per OS.
- Can GPUI/gpui-kit actually deliver tray + global-hotkey support (neither provides it upstream) — or is the unofficial "Adabraka GPUI" fork, or a hand-rolled per-OS shim, needed?
- ~~Do Chatterbox's model weights fit within GitHub Releases' practical hosting limits, and are they legally redistributable there at all?~~ **Resolved 2026-09-21:** the ONNX weights are MIT-licensed and every file v1 ships is under the 2 GB per-asset limit (largest: `language_model_fp16.onnx_data`, 1.04 GB); only the unused FP32 backbone (2.08 GB) exceeds it (Architecture Spine AD-7).
- ~~What exact audio buffer format crosses the TTS-to-Virtual-Microphone boundary?~~ **Resolved 2026-09-21:** 32-bit float, 24 000 Hz, mono — Chatterbox's own output rate, converted to whatever the driver needs inside each `voice-me-audio-*` adapter (Architecture Spine AD-11).
