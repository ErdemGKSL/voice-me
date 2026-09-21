---
stepsCompleted: [1, 2, 3]
inputDocuments:
  - _bmad-output/planning-artifacts/prds/prd-voice-me-2026-09-20/prd.md
  - _bmad-output/planning-artifacts/prds/prd-voice-me-2026-09-20/addendum.md
  - _bmad-output/planning-artifacts/architecture/architecture-voice-me-2026-09-20/ARCHITECTURE-SPINE.md
  - _bmad-output/planning-artifacts/ux-designs/ux-voice-me-2026-09-20/DESIGN.md
  - _bmad-output/planning-artifacts/ux-designs/ux-voice-me-2026-09-20/EXPERIENCE.md
  - _bmad-output/specs/spec-voice-me/SPEC.md
---

# voice-me - Epic Breakdown

## Overview

This document provides the complete epic and story breakdown for voice-me, decomposing the requirements from the PRD, UX Design, and Architecture Spine into implementable stories.

## Requirements Inventory

### Functional Requirements

FR1: User can record a short voice clip using their system microphone directly in-app, or import an existing audio file, to use as their Reference Voice Sample; the clip is saved locally, selectable as active, and re-recordable/replaceable at any time from settings.

FR2: The app runs as a background/tray-resident process between uses — no visible window is needed to respond to the hotkey, and dismissing the Prompt Overlay leaves the app running and reachable (status, settings, quit) via a system tray/menu-bar presence.

FR3: User can assign and change a single global hotkey combination that summons the Prompt Overlay from anywhere, including while a game has input focus, on both Linux and Windows; assigning a combination already in use elsewhere is surfaced, not silently dropped.

FR4: Pressing the configured hotkey opens a minimal, borderless, always-on-top single-line Prompt Overlay; pressing Enter closes it immediately and triggers the Speak Action with the typed text (generation happens after the UI is gone); pressing Escape or losing focus closes it without speaking and discards the text.

FR5: On a Speak Action, the typed text plus the active Reference Voice Sample and selected speech language are passed to the Inference Engine (Chatterbox-Multilingual V3's ONNX export, run in-process on ONNX Runtime — Architecture Spine AD-12), producing audio in the user's cloned voice; a generation failure surfaces a clear failure indication rather than silence. v1 speech languages are limited to those needing no Python-only text normalization (Turkish and English included; Chinese, Japanese, Hebrew and Korean excluded).

FR6: Generated audio plays out through the Virtual Microphone device (not the default speaker) on both Linux and Windows, timed to the Speak Action, so any application selecting it as input receives the audio; the user's real physical microphone is unaffected.

FR7: On first run and on demand, the app checks for locally required components (the ONNX Runtime shared library, GPU execution providers, the ONNX model weights, the Virtual Microphone driver) and offers one-click provisioning inside the app UI where feasible, naming any unfixable-automatically dependency specifically with manual next steps; the app degrades to a CPU-only mode — the Q4-quantized language model on the CPU execution provider — rather than failing outright when no GPU is available.

FR8: The application interface (menus, settings, Prompt Overlay chrome, error messages) is available in Turkish and English, user-selectable, switching without requiring a restart, and independent of the TTS speech language.

FR9: The UI is implemented using gpui-kit components rather than hand-rolled GPUI primitives wherever a suitable component exists, and follows the gpui-kit Design Guides for spacing, typography, color, density, and interaction states, checked before being considered done.

### NonFunctional Requirements

NFR1 (Performance): The Prompt Overlay must appear and be ready for keystrokes within roughly 200ms of the hotkey press (unmeasured target, PRD Assumptions Index). End-to-end latency from Enter to audible playback must feel usable in live voice chat; no hard numeric target is set until measured (PRD Open Question 1).

NFR2 (Compatibility/Risk): Global hotkey capture must function while a fullscreen or borderless-fullscreen game holds input focus, on Linux and Windows, without being blocked by the OS or the game. Whether this risks conflicts with anti-cheat software (BattlEye, EasyAntiCheat) in specific games is unresolved and must be investigated before the hotkey-capture implementation is locked in (PRD Open Question 3).

NFR3 (Privacy/Security): The app must be local-only — no cloud calls, no accounts, no telemetry. Network egress is limited to the dependency-provisioning adapter's fetches from this project's own GitHub Releases; every other component is fully offline — the Inference Engine runs in-process and opens no socket at all (Architecture Spine AD-8, AD-12), enforced by a CI check rather than review discipline alone.

NFR4 (Reliability): The app must degrade gracefully — to a CPU-only mode when no GPU is available (FR7), and to a clear, specific in-app or OS-notification failure state (never silent nothing) when TTS generation or a dependency check fails.

NFR5 (Usability): Switching UI language must take effect live, without an app restart (PRD Assumptions Index).

NFR6 (Distribution): The app ships as a single native executable per platform with no traditional installer or setup wizard; any supporting driver/runtime is installed and managed by the app itself on first run, not by a separate installer step.

### Additional Requirements

**Starter Template:** None. voice-me is a from-scratch Rust workspace on the Architecture Spine's own hexagonal/ports-and-adapters paradigm — there is no scaffolded starter/greenfield template to clone. Epic 1's first story is standing up the Cargo workspace and crate skeleton per the Structural Seed below, not adopting a template.

**Architecture paradigm and boundaries (binding, from ARCHITECTURE-SPINE.md):**
- Hexagonal/Ports-and-Adapters: `voice-me-core` holds domain types, `AppState`, `AppEvent`, and every port trait (`HotkeyPort`, `VirtualMicPort`, `TrayPort`, `TtsPort`, `DependencyProvisioningPort`, `SettingsStore`); it depends on no other `voice-me-*` crate (AD-1).
- Per-OS capabilities ship as separate crates, not `cfg`-gated modules: `voice-me-hotkey-{linux,windows}`, `voice-me-audio-{linux,windows}`, `voice-me-tray-{linux,windows}` (AD-2).
- Single `AppState`/`AppEvent` ownership: only `voice-me-core` use-cases mutate `AppState`; `voice-me-app` is the sole `AppEvent` receiver (AD-3).
- One `thiserror` domain error enum in core; adapters map their own errors at the boundary; `anyhow` aggregates at the composition root (AD-4).
- Tokio runs alongside GPUI's own executor via one in-house bridge (Zed's `gpui_tokio` pattern reimplemented, not depended on directly — that crate is workspace-internal to `zed-industries/zed` and not consumable as-is) (AD-5).
- Settings (one TOML) and the Reference Voice Sample (a file path) live behind `SettingsStore`, implemented inside `voice-me-core` itself; large runtime assets (ONNX Runtime library and execution providers, model weights, driver installer) live in a separate cache directory owned by `voice-me-deps` (AD-6).
- CI/release: GitHub Actions, separate Linux/Windows build jobs (no macOS), binaries and mirrored runtime assets on this repo's GitHub Releases (AD-7).
- No network egress outside `voice-me-deps`'s declared fetches, anywhere in the app (AD-8).
- GPU-capability detection is core-owned: `voice-me-deps` reports via `AppEvent` only; `voice-me-tts` never calls `voice-me-deps` directly (AD-9).
- Speak Action sequencing: Prompt Overlay dismissal is synchronous and never waits on `TtsPort::generate`; ONNX session lifecycle (built once lazily, held for the process lifetime, one generation in flight at a time) is entirely internal to `voice-me-tts` (AD-10).
- TTS inference runs in-process via the `ort` crate — no Python, no child process, no IPC; generation runs on `spawn_blocking` because it is CPU/GPU-bound (AD-12, AD-5).
- One shared audio buffer type crosses the `TtsPort` → `VirtualMicPort` boundary — never a raw byte slice or a per-adapter struct; its format is fixed at 24 kHz mono f32 (AD-11).

**Crate/workspace structure (Structural Seed):** `voice-me-app` (bin, composition root) · `voice-me-core` · `voice-me-ui` (gpui-kit) · `voice-me-hotkey-linux` / `voice-me-hotkey-windows` · `voice-me-audio-linux` / `voice-me-audio-windows` · `voice-me-tray-linux` / `voice-me-tray-windows` · `voice-me-tts` · `voice-me-deps` · `voice-me-i18n` · `voice-me-tests`.

**Infrastructure/deployment:** GitHub Actions CI with separate `ubuntu-latest` and `windows-latest` jobs; GitHub Releases as the sole distribution and runtime-asset-hosting channel (AD-7). No server/cloud environment exists.

**Integration requirements:** Linux Virtual Microphone via a PipeWire/PulseAudio null-sink (no driver, no signing). Windows Virtual Microphone via `VirtualDrivers/Virtual-Audio-Driver` (release 25.7.14, SignPath-signed) — its named-pipe/IPC control surface is unconfirmed and must be verified before `voice-me-audio-windows` is implemented (PRD Open Question 2). Chatterbox-Multilingual V3 runs **in-process** inside `voice-me-tts` on ONNX Runtime via the `ort` crate (AD-12) — four ONNX graphs (`speech_encoder`, `embed_tokens`, `language_model`, `conditional_decoder`) plus `tokenizer.json`, all MIT-licensed, from `onnx-community/chatterbox-multilingual-ONNX`. No Python, no sidecar, no IPC. `voice-me-deps` provisions the ONNX Runtime shared library and execution providers (CUDA / DirectML / CPU) alongside the weights.

**Monitoring/logging:** `tracing`, initialized once centrally in `voice-me-app`; no crate sets up its own logger.

**Foundational unresolved risk (flag prominently):** Neither GPUI nor gpui-kit provide system tray or global-hotkey support upstream (PRD Open Question 6 / Architecture Spine Deferred) — the unofficial "Adabraka GPUI" fork claims to but is unverified. This is isolated behind the `voice-me-tray-*` and `voice-me-hotkey-*` adapter crates specifically so its resolution doesn't ripple elsewhere, but it must be resolved early (a spike) since FR2, FR3, and FR4 all depend on it.

### UX Design Requirements

UX-DR1: Inherit gpui-kit/gpui-component theme tokens wholesale (colors, spacing, radius, density, state treatment); override only the primary accent color (`#7C6AFF`, violet) and the Prompt Overlay's own rounder corner radius (12px) as the sole brand-layer deltas.

UX-DR2: Dark theme only for v1 — no light theme or theme switcher is in scope.

UX-DR3: A monospace "shortcut" typography style (12px, medium) is reserved exclusively for rendering hotkey chips; never used for prose.

UX-DR4: Build a custom **Prompt Overlay** component — not a stock gpui-kit surface. Borderless, always-on-top, one `Input` and nothing else, gpui-kit's popover-family elevation/shadow treatment, the overlay-specific radius token, fixed non-resizable comfortable width.

UX-DR5: Build a **Hotkey chip** component (monospace shortcut style, `muted` background) used in Settings and during hotkey capture.

UX-DR6: Build a **Recording indicator** component (primary-violet dot/waveform) shown only while capturing a Reference Voice Sample in Voice Setup — the one place the accent color appears outside a primary button.

UX-DR7: Settings is one window with four sections (Voice, Hotkey, Dependencies, General) via gpui-kit `Tabs` — no sidebar workspace shell at this scope.

UX-DR8: Tray menu has exactly two items: "Settings…" and "Quit" — no status submenu.

UX-DR9: Prompt Overlay behavior — appears with its `Input` pre-focused so typing starts immediately (`[ASSUMPTION: centered on the active/focused monitor]`); `Enter` closes it and fires the Speak Action without waiting on generation; `Escape` or focus loss closes it and discards the text.

UX-DR10: Hotkey capture field — clicking "Change" captures the next key combination live, renders it as a hotkey chip for confirmation, and rejects an already-in-use combination inline before Save (not after).

UX-DR11: Voice recorder component — Record/Stop with the recording indicator, inline playback before Accept; Import is an alternative entry via a standard file picker, converging on the same accept/re-record actions.

UX-DR12: Dependency row component — name, status `Badge` (ready/missing/installing), one-click "Install" when automatable, else a short manual-steps link.

UX-DR13: UI language selector (Settings → General) applies instantly to every open surface with no restart, and is independent of the TTS speech-language selection in Settings → Voice.

UX-DR14: When TTS generation is unusually slow, a brief OS-native notification reports it's still working (no blocking UI, since the overlay is already closed by then).

UX-DR15: A Speak Action failure surfaces via an OS-native notification naming the short reason — this notification itself is the "clear failure" surface (FR5); no overlay reappears automatically.

UX-DR16: First run with no Reference Voice Sample yet auto-opens Settings → Voice to an empty state ("Record your voice to get started") with a single primary action, rather than dropping straight to the tray.

UX-DR17: A blocking missing-dependency state auto-opens Settings → Dependencies; the Prompt Overlay still opens on hotkey press but shows an inline notice instead of accepting input until resolved.

UX-DR18: No-GPU is an informational "CPU mode" `Badge` in Settings → Dependencies, not an error.

UX-DR19: A hotkey conflict shows an inline error at the capture field naming the conflicting app; the previous working hotkey stays active until a new one is confirmed.

UX-DR20 (Accessibility): The Prompt Overlay must be operable keyboard-only from summon to dismissal, including auto-focus on appear with no extra click. Settings follows standard focus order and visible focus rings. Icon-only controls (tray icon, any icon buttons) carry a tooltip and accessible name. Status is never color-only (e.g. the "missing" badge carries the word, not just a color).

UX-DR21 (Platform): Hotkey chips render platform-native modifier names (Linux: Ctrl/Alt/Shift/Super; Windows: Ctrl/Alt/Shift/Win); the tray icon follows each OS's native status-icon area convention.

UX-DR22 (Motion): The Prompt Overlay uses only a short fade/scale-in on summon and fade-out on dismiss; no other animation anywhere in the app.

### FR Coverage Map

FR1: Epic 1 - Record/import/replace the Reference Voice Sample
FR2: Epic 2 - Background/tray-resident operation
FR3: Epic 2 - Global hotkey configuration
FR4: Epic 2 - Prompt Overlay summon/type/dismiss
FR5: Epic 2 - TTS generation via the cloned voice
FR6: Epic 2 - Playback through the Virtual Microphone
FR7: Epic 3 - Dependency Check and one-click provisioning
FR8: Epic 4 - Turkish/English UI
FR9: Cross-cutting - gpui-kit visual design, applied as an acceptance-criteria bar within every epic's stories rather than its own epic (per PRD §4.5, not a standalone user-facing capability)

## Epic List

### Epic 1: Voice Identity Setup
Users can record their own voice directly in the app, or import an existing audio file, and use it as their Reference Voice Sample — saved locally, selectable, and replaceable at any time. This is a complete, demonstrable capability on its own: record/import, hear it back, done. Epic 1's first story also stands up the Cargo workspace and crate skeleton per the Architecture Spine's Structural Seed (no starter template exists to adopt — this is a from-scratch hexagonal workspace).
**FRs covered:** FR1

### Epic 2: Speak Without Speaking (Core Loop)
Users press a global hotkey from anywhere — including inside a fullscreen game — get a minimal overlay, type a line, hit Enter, and a moment later hear that line spoken in their own cloned voice through a virtual microphone that other apps pick up as input. This is the product's entire reason to exist, delivered end to end. Two distinct technical risks live inside this epic (GPUI/gpui-kit's missing tray+hotkey support; the ONNX inference port and the virtual-mic-driver control surface) and are spiked early via story sequencing rather than epic splitting, since both belong to one inseparable user outcome.
**FRs covered:** FR2, FR3, FR4, FR5, FR6

### Epic 3: Never Get Stuck on Setup
The app detects locally missing runtime dependencies itself and offers one-click provisioning from inside its own UI, naming anything it can't fix automatically with clear manual steps; it degrades to a CPU-only mode rather than failing outright when no GPU is available.
**FRs covered:** FR7

### Epic 4: Use It In Your Language
The interface is available in Turkish and English, switchable instantly without a restart, independent of the TTS speech language.
**FRs covered:** FR8

## Epic 1: Voice Identity Setup

Users can record their own voice directly in the app, or import an existing audio file, and use it as their Reference Voice Sample — saved locally, selectable, and replaceable at any time.

### Story 1.1: Set Up the Project Workspace

As Erdem,
I want the Cargo workspace and crate skeleton scaffolded per the Architecture Spine,
So that every feature story has a place to add code without inventing structure ad hoc.

**Acceptance Criteria:**

**Given** an empty repository
**When** the workspace is scaffolded
**Then** a Cargo workspace exists with member crates `voice-me-app` (bin), `voice-me-core`, `voice-me-ui`, `voice-me-i18n`, `voice-me-tests`, plus empty stub crates for `voice-me-hotkey-{linux,windows}`, `voice-me-audio-{linux,windows}`, `voice-me-tray-{linux,windows}`, `voice-me-tts`, `voice-me-deps`
**And** `voice-me-core` contains stub `AppState`, `AppEvent`, and the empty port trait definitions (`HotkeyPort`, `VirtualMicPort`, `TrayPort`, `TtsPort`, `DependencyProvisioningPort`, `SettingsStore`) per AD-1
**And** the workspace builds with `cargo build`
**And** GitHub Actions CI runs separate Linux and Windows build jobs per AD-7

### Story 1.2: Record a Reference Voice Sample

As Erdem,
I want to record a short clip of my own voice directly in the app,
So that voice-me has a Reference Voice Sample to clone my voice from.

**Acceptance Criteria:**

**Given** no Reference Voice Sample exists yet
**When** I open Voice Setup, press Record, speak, then press Stop
**Then** the clip is saved via `SettingsStore` (AD-6) and becomes the active Reference Voice Sample
**And** I can play it back inline before accepting (UX-DR11)
**And** Record/Stop/Playback are gpui-kit components per gpui-kit-design-guides (FR9)
**And** a Recording indicator (UX-DR6) shows while capture is active

### Story 1.3: Import an Existing Audio File as the Reference Voice Sample

As Erdem,
I want to import an existing audio file as my Reference Voice Sample,
So that I can reuse a clip I already have instead of recording live.

**Acceptance Criteria:**

**Given** the app is running
**When** I choose "Import" in Voice Setup and pick a file via the system file picker
**Then** the file is validated and becomes the active Reference Voice Sample the same way a recording would
**And** an unsupported format or an out-of-bounds clip shows a clear in-app message rather than failing silently
**And** I can play it back inline before accepting, same as a recording

### Story 1.4: Replace My Reference Voice Sample

As Erdem,
I want to re-record or replace my Reference Voice Sample at any time from settings,
So that I can update my voice profile without touching files manually.

**Acceptance Criteria:**

**Given** an active Reference Voice Sample already exists
**When** I choose to re-record or re-import from Settings → Voice
**Then** the new clip replaces the old one as active once accepted
**And** this can be repeated any number of times without restarting the app

### Story 1.5: First-Run Voice Setup Prompt

As Erdem,
I want the app to walk me straight into Voice Setup the first time I run it,
So that I'm never stuck looking at a bare tray icon wondering what to do.

**Acceptance Criteria:**

**Given** this is the first launch with no Reference Voice Sample yet
**When** the app starts
**Then** Settings → Voice auto-opens to the empty state "Record your voice to get started" with a single primary action (UX-DR16)
**And** once a sample is accepted, this auto-open never fires again

## Epic 2: Speak Without Speaking (Core Loop)

Users press a global hotkey from anywhere — including inside a fullscreen game — get a minimal overlay, type a line, hit Enter, and a moment later hear that line spoken in their own cloned voice through a virtual microphone that other apps pick up as input.

### Story 2.1: Spike — Background/Tray Presence Feasibility on GPUI/gpui-kit

As Erdem,
I want to validate that the app can run as a tray-resident background process using GPUI/gpui-kit or a fallback,
So that the foundational tray/hotkey risk (PRD Open Question 6) is resolved before building the rest of the core loop on it.

**Acceptance Criteria:**

**Given** `voice-me-tray-linux` and `voice-me-tray-windows` are empty stub crates
**When** a minimal spike implements a system tray icon on each platform (a hand-rolled per-OS shim, or evaluating the "Adabraka GPUI" fork)
**Then** the app shows a tray icon and responds to a "Quit" click on both Linux and Windows
**And** the chosen approach is recorded as a decision (architecture memlog/spine addendum)
**And** PRD Open Question 6 is marked resolved, or a documented fallback plan exists if not

### Story 2.2: Run as a Background/Tray-Resident Process

As Erdem,
I want the app to keep running in the tray after I dismiss the Prompt Overlay,
So that I never have to relaunch it to use the hotkey again.

**Acceptance Criteria:**

**Given** the app has just started, or the Prompt Overlay was just dismissed
**When** no window is open
**Then** the app remains active in the background, reachable via the tray icon with exactly "Settings…" and "Quit" (UX-DR8)
**And** the tray menu opens Settings or quits the app on click
**And** the tray icon follows each OS's native status-icon area convention (UX-DR21)

### Story 2.3: Configure a Global Hotkey

As Erdem,
I want to assign and change a single global hotkey that summons the Prompt Overlay from anywhere,
So that I can trigger it even while a game has focus.

**Acceptance Criteria:**

**Given** Settings → Hotkey is open
**When** I click "Change" and press a key combination
**Then** it's captured live and rendered as a hotkey chip (UX-DR5, UX-DR10) for confirmation before Save
**And** an already-in-use combination is rejected inline with the conflicting app named (UX-DR19), not silently
**And** once saved, the hotkey works while a fullscreen or borderless-fullscreen game has input focus, on both Linux and Windows (NFR2)
**And** hotkey chips render platform-native modifier names (UX-DR21)
**And** whether this hotkey-capture approach risks anti-cheat conflicts (BattlEye, EasyAntiCheat) has been investigated and documented before this story is considered done (PRD Open Question 3)

### Story 2.4: Summon, Type, and Dismiss the Prompt Overlay

As Erdem,
I want pressing my hotkey to open a minimal overlay I can type into and dismiss instantly,
So that the interaction never breaks my flow.

**Acceptance Criteria:**

**Given** a hotkey is configured (Story 2.3) and tray presence is running (Story 2.2)
**When** I press the hotkey
**Then** the Prompt Overlay appears with its Input pre-focused, ready for keystrokes without perceptible delay (~200ms target, NFR1) (UX-DR9)
**And** pressing Enter closes the Overlay immediately and triggers the Speak Action with the typed text, without waiting on generation
**And** pressing Escape, or losing focus, closes the Overlay and discards the typed text without speaking
**And** the Overlay is a custom gpui-kit-based component (not a stock surface), fixed non-resizable width, with the overlay-specific corner radius and elevation (UX-DR4), and only a short fade/scale-in and fade-out animation (UX-DR22)
**And** the Overlay is fully keyboard-operable from summon to dismissal with no extra click required (UX-DR20)

### Story 2.5: Spike — In-Process Chatterbox Inference on ONNX Runtime

As Erdem,
I want to validate that Chatterbox-Multilingual V3's ONNX export runs in-process from Rust and is fast enough to be usable,
So that the remaining TTS risk — the ported generation loop and real latency (PRD Open Question 1) — is resolved before the full pipeline is wired.

**Acceptance Criteria:**

**Given** `voice-me-tts` is an empty stub crate
**When** a minimal spike loads the four ONNX graphs plus `tokenizer.json` with the `ort` crate and runs the AD-12 generation loop end to end
**Then** typed Turkish and English text plus a sample Reference Voice Sample produce audible, recognizably-cloned 24 kHz audio on the developer's machine, with no Python installed or invoked
**And** the reference clip is decoded and resampled to 24 kHz mono f32 from at least one non-wav source format
**And** generation latency is measured and recorded for both paths available on this machine — CPU (Q4) and GPU (FP16, CUDA) — as the first real data for PRD Open Question 1, along with a note on whether Q4 degrades the cloned voice audibly versus FP16
**And** one-off session-construction cost is measured separately from per-utterance cost, confirming AD-10's build-once/hold decision
**And** the in-house Tokio/GPUI bridge (AD-5) is proven to work end-to-end with generation running on `spawn_blocking`
**And** the shared audio buffer type (AD-11) is implemented in `voice-me-core` as 24 kHz mono f32
**And** the outcome (viable as-is / needs quantization or backend changes) is documented, including which ONNX Runtime library and execution-provider files had to be present for each path — the input Story 3.2's provisioning work depends on

### Story 2.6: Generate Speech from the Prompt Overlay Text

As Erdem,
I want my typed line spoken in my own cloned voice, generated automatically after I hit Enter,
So that I don't have to do anything beyond typing and pressing Enter.

**Acceptance Criteria:**

**Given** a Speak Action has been triggered (Story 2.4) and a Reference Voice Sample exists (Epic 1)
**When** the typed text, active Reference Voice Sample, and selected speech language are passed to the in-process Inference Engine via `TtsPort` (AD-12)
**Then** generated audio is produced in the selected speech language, independent of the UI language
**And** the ONNX sessions are built once and reused across Speak Actions; a second Speak Action arriving mid-generation is queued, not run concurrently (AD-10)
**And** if generation fails, an OS-native notification names the failure clearly (UX-DR15) rather than silence
**And** if generation takes unusually long, a brief OS-native notification reports it's still working (UX-DR14)
**And** hardware capability (which execution provider, and therefore which language-model weight variant) is read from `AppState` per AD-9, never queried directly from `voice-me-deps`

### Story 2.7: Spike — Linux Virtual Microphone via PipeWire

As Erdem,
I want to validate that a PipeWire/PulseAudio null-sink can serve as the Virtual Microphone on Linux,
So that FR6's Linux implementation has a proven approach before building on it.

**Acceptance Criteria:**

**Given** `voice-me-audio-linux` is an empty stub crate
**When** a minimal spike creates a null-sink virtual audio source and plays a test buffer into it
**Then** another application can select that device as its microphone input and receive the audio
**And** the approach requires no elevated privileges or kernel driver

### Story 2.8: Spike — Windows Virtual Microphone Control Surface

As Erdem,
I want to validate whether VirtualDrivers/Virtual-Audio-Driver (25.7.14) can be controlled programmatically from an unsigned app,
So that PRD Open Question 2 is resolved before FR6's Windows implementation is built on an unconfirmed assumption.

**Acceptance Criteria:**

**Given** the driver is installed on a Windows test machine
**When** `voice-me-audio-windows` attempts to control it via named pipe or another interface
**Then** either named-pipe/IPC control is confirmed to work out of the box, or a documented fallback (custom build, alternative interface, different driver) is chosen
**And** the outcome updates the Architecture Spine addendum and PRD Open Question 2

### Story 2.9: Play Generated Audio Through the Virtual Microphone

As Erdem,
I want the generated audio to come out of a virtual microphone that other apps treat as a real mic,
So that my teammates in voice chat hear it as if I'd spoken.

**Acceptance Criteria:**

**Given** generated audio exists (Story 2.6) and a Virtual Microphone is available (Stories 2.7/2.8)
**When** the audio is routed to the Virtual Microphone via `VirtualMicPort`
**Then** an application selecting the Virtual Microphone as input receives the audio, timed to the Speak Action, on both Linux and Windows
**And** the user's real physical microphone is unaffected
**And** the shared audio buffer type (AD-11) is what crosses this boundary, with any format conversion happening inside the adapter

## Epic 3: Never Get Stuck on Setup

The app detects locally missing runtime dependencies itself and offers one-click provisioning from inside its own UI, naming anything it can't fix automatically with clear manual steps; it degrades to a CPU-only mode rather than failing outright when no GPU is available.

### Story 3.1: Detect Missing Dependencies

As Erdem,
I want the app to check for locally required runtime components on first run and on demand,
So that I always know what's missing instead of hitting a cryptic crash.

**Acceptance Criteria:**

**Given** the app starts, or I open Settings → Dependencies
**When** the Dependency Check runs via `DependencyProvisioningPort`
**Then** each dependency (the ONNX Runtime shared library, GPU execution providers, the Chatterbox ONNX weights, the Virtual Microphone driver) is listed as a Dependency row with status ready/missing (UX-DR12)
**And** the check completes and reports results without requiring a terminal or external documentation

### Story 3.2: One-Click Provision a Missing Dependency

As Erdem,
I want to fix a missing dependency with one click from inside the app,
So that I never have to open a terminal or read a wiki to get voice-me working.

**Acceptance Criteria:**

**Given** a dependency is listed as missing and is automatable
**When** I click "Install" on that Dependency row
**Then** the app provisions it from this repo's GitHub Releases (AD-7) and the row updates to "ready" on success
**And** the model-weight download — the largest asset, up to ~1.5 GB depending on the variant chosen for this machine's execution provider — reports progress and survives being resumed rather than appearing frozen
**And** a failure during provisioning shows a clear, specific message naming what went wrong
**And** a dependency that isn't automatable shows a short manual-steps link instead of an Install button

### Story 3.3: Degrade to CPU-Only Mode Without a GPU

As Erdem,
I want the app to keep working in a slower CPU-only mode when no GPU is available,
So that a missing GPU never fully blocks me from using voice-me.

**Acceptance Criteria:**

**Given** no GPU execution provider is detected during the Dependency Check
**When** TTS generation is requested
**Then** the app runs the same Chatterbox model on the CPU execution provider with the Q4-quantized language model automatically, without failing (AD-12)
**And** Settings → Dependencies shows an informational "CPU mode" badge, not an error (UX-DR18)
**And** this capability detection flows from `voice-me-deps` to `voice-me-core` via `AppEvent` only, never a direct call to `voice-me-tts` (AD-9)
**And** only the weight variant the detected backend needs is downloaded — a CPU-only machine never fetches the FP16 backbone

### Story 3.4: Block the Overlay Gracefully on a Missing Dependency

As Erdem,
I want the Prompt Overlay to tell me what's missing instead of silently failing,
So that I'm never confused about why speaking isn't working.

**Acceptance Criteria:**

**Given** a required dependency is still missing
**When** I press the hotkey
**Then** Settings → Dependencies auto-opens naming the specific blocker (UX-DR17)
**And** the Prompt Overlay still opens on hotkey press but shows an inline notice instead of accepting input until the dependency is resolved
**And** once resolved, the Prompt Overlay returns to normal behavior without an app restart (NFR6)

## Epic 4: Use It In Your Language

The interface is available in Turkish and English, switchable instantly without a restart, independent of the TTS speech language.

### Story 4.1: Turkish and English UI Toggle

As Erdem,
I want to switch the entire interface between Turkish and English,
So that I can use voice-me comfortably in either language.

**Acceptance Criteria:**

**Given** Settings → General is open
**When** I select Turkish or English from the UI language selector (UX-DR13)
**Then** every open surface (menus, Settings, Prompt Overlay chrome, error/notification messages) re-renders in the selected language immediately, without an app restart (NFR5)
**And** the TTS speech language selection in Settings → Voice is unaffected by this change, and vice versa

### Story 4.2: Persist and Apply UI Language on Launch

As Erdem,
I want my chosen UI language to be remembered,
So that I don't have to reselect it every time I open the app.

**Acceptance Criteria:**

**Given** a UI language was previously selected and saved via `SettingsStore`
**When** the app launches again
**Then** every surface renders in that saved language from the first frame, with no flash of the other language or a restart required
