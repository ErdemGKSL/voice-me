---
title: "PRD: voice-me"
status: final
created: 2026-09-20
updated: 2026-09-20
---

# PRD: voice-me

## 0. Document Purpose

This PRD is for the creator (Erdem, sole builder) as the working spec for v1 of voice-me — a personal, hobby, open-source project. It builds directly on `brief.md` and `addendum.md` (`_bmad-output/planning-artifacts/briefs/brief-voice-me-2026-09-20/`), which already cover the concept, differentiators, and initial risk research; this document does not repeat that narrative — it turns it into features, functional requirements, and scope boundaries. Codebase/crate structure and specific driver research live in this run's own `addendum.md`, not here.

## 1. Vision

voice-me lets you "speak" in your own cloned voice without speaking out loud. A global hotkey pops open a tiny overlay over whatever you're doing — a game, a call — you type a line, hit Enter, and it's gone; a moment later, the line plays through a virtual microphone in your own voice, so anything listening to your mic hears it as if you'd said it. It runs entirely local, with no account and no cloud calls, as a single native app the creator actually uses during real gaming sessions.

## 2. Target User

### 2.1 Jobs To Be Done

- As the builder, I want to "say" something in voice chat when I can't or don't want to speak out loud (shared room, late night, sore throat, or just not wanting to say it live) — without falling back to slower, easily-missed text chat.
- I want to say things in a language I type more comfortably than I pronounce, still heard as my own voice.
- [ASSUMPTION] If shared later: other gamers with the same situational-mute constraint want the same thing without sending anything off their machine.

### 2.2 Non-Users (v1)

- Anyone wanting real-time, continuous voice transformation while actually speaking (that's a voice changer; this is discrete typed-line playback).
- macOS users (platform deferred past v1).
- Anyone who needs cloud/hybrid inference or multi-device sync — this is single-machine, local-only by design.

### 2.3 Key User Journeys

- **UJ-1. Erdem calls out a line mid-match without speaking.** Erdem is mid-game, voice-chat active with teammates, in a shared room and can't speak out loud. He presses his configured hotkey (e.g. `Alt+Shift+Space`); the Prompt Overlay appears on top of the game without stealing full focus from the game's audio/input state. He types "sağ tarafa geçin" and hits Enter. The Prompt Overlay vanishes instantly. Within about a second, his cloned voice says the line, routed through the Virtual Microphone — teammates hear it exactly like a normal callout. **Edge case:** if TTS generation takes noticeably longer than expected, he gets some visual/audio cue that it's still working rather than silence with no feedback (see the related NFR in §4.2).
- **UJ-2. Erdem sets up his voice once.** First run, Erdem is walked through recording a short reference clip of his own voice directly in the app, picks a global hotkey, and confirms the app can see a virtual microphone in his OS's sound settings — all in a few minutes, in Turkish (his chosen UI language), with no manual file editing.

## 3. Glossary

- **Reference Voice Sample** — the short audio clip of the user's own voice, recorded or imported once, used by the TTS engine to clone their voice for every generated line.
- **Prompt Overlay** — the small, borderless, always-on-top single-line text input that appears on hotkey press and closes on Enter (or Escape to cancel).
- **Speak Action** — the act of the Prompt Overlay closing on Enter, triggering TTS generation and playback for the typed line.
- **TTS Engine** — Chatterbox-Multilingual V3, the local voice-cloning text-to-speech model that turns (text, language, Reference Voice Sample) into audio.
- **Virtual Microphone** — an OS-level virtual audio input device that other applications (games, Discord, Zoom) can select as their microphone; voice-me plays generated audio into it instead of a physical mic.
- **Sidecar Process** — the local background process that hosts the TTS Engine's Python/PyTorch runtime, managed and provisioned automatically by the main app (detailed in `addendum.md`).
- **Dependency Check** — the app's on-demand scan for locally missing components the Sidecar Process needs (e.g. the bundled Python runtime, GPU acceleration libraries), surfaced in-app rather than as a setup wizard.
- **Preset Phrase** — (v2+, not in v1) a fixed phrase bound directly to a hotkey, spoken with no Prompt Overlay shown at all.
- **GPUI** — the underlying Rust GPU-accelerated UI framework (Zed Industries) the app's interface is built on.
- **gpui-kit** — the component/design-system layer built on top of GPUI (`gpui_kit::component`, `gpui_kit::base`, `gpui_kit::assets`) that voice-me's UI is implemented with, rather than raw GPUI primitives.

## 4. Features

### 4.1 Voice Setup
**Description:** One-time (repeatable) flow where the user records or imports a Reference Voice Sample directly in the app. Realizes UJ-2.

#### FR-1: Record or import a Reference Voice Sample
User can record a short voice clip using their system microphone directly in-app, or import an existing audio file, to use as their Reference Voice Sample.

**Consequences (testable):**
- A recorded or imported clip is saved locally and selectable as the active Reference Voice Sample.
- The user can re-record or replace the Reference Voice Sample at any time from settings.
- [ASSUMPTION] Minimum/maximum clip length and supported import formats are enforced with a clear in-app message when violated — exact bounds to be set during implementation, not user-specified yet.

**Notes:** `[NOTE FOR PM]` Chatterbox's documented recommended reference-clip length/quality should set the enforced bounds — verify against model docs during implementation, not assumed here.

### 4.2 Hotkey-Triggered Prompt Overlay & Speak
**Description:** The core interaction loop. Realizes UJ-1.

`[NOTE FOR PM]` GPUI has no built-in system tray or global hotkey support upstream (pre-1.0); a community fork ("Adabraka GPUI") reportedly adds both but is unverified. This is a real feasibility risk for FR-2 and FR-3, tracked as Open Question 6 — do not assume either is solved by GPUI alone.

#### FR-2: Run quietly in the background
The app runs as a background/tray-resident process between uses — it does not need a visible window open to respond to the hotkey, and exiting the Prompt Overlay leaves the app running, listening for the next hotkey press.

**Consequences (testable):**
- After closing or dismissing the Prompt Overlay, the app remains active and responds to the next hotkey press without the user needing to relaunch it.
- The app is reachable (status, settings, quit) via a system tray/menu-bar-equivalent presence. `[ASSUMPTION: exact tray UX per OS not yet designed]`

#### FR-3: Configure a global hotkey
User can assign and change a single global hotkey combination that summons the Prompt Overlay from anywhere, including while a game has input focus.

**Consequences (testable):**
- The hotkey works while a fullscreen or borderless-fullscreen game has focus, on both v1 target platforms (Linux, Windows).
- Assigning a hotkey already in use elsewhere on the system is surfaced to the user rather than silently failing.

**Feature-specific NFRs:**
- `[NOTE FOR PM]` Global input hooking can trigger anti-cheat systems (e.g. BattlEye, EasyAntiCheat) in some games — this is a real compatibility risk for the stated "works during games" goal and should be investigated, not assumed safe, before relying on any specific hotkey-capture approach.

#### FR-4: Prompt Overlay summon, type, and dismiss
Pressing the configured hotkey opens a minimal, borderless, always-on-top single-line Prompt Overlay; pressing Enter closes it immediately and triggers the Speak Action with the typed text; pressing Escape closes it without speaking.

**Consequences (testable):**
- The Overlay appears and is ready to receive keystrokes without a perceptible delay (`[ASSUMPTION]` target under 200ms from hotkey press to ready-for-input — no measured baseline yet).
- The Overlay closes immediately on Enter, before TTS generation completes (generation and playback happen after the UI is already gone).
- Escape or losing focus without pressing Enter discards the typed text and does not trigger playback.

#### FR-5: Text-to-speech generation via the TTS Engine
On a Speak Action, the typed text plus the active Reference Voice Sample and selected language are sent to the TTS Engine (Chatterbox-Multilingual V3) via the Sidecar Process, producing audio in the user's cloned voice.

**Consequences (testable):**
- Generated audio is in the language the user selected for speech output (independent of UI language), from Chatterbox-Multilingual V3's supported language set.
- If generation fails (Sidecar Process unavailable, model error), the user sees a clear in-app failure indication rather than silence with no feedback.

**Feature-specific NFRs:**
- `[ASSUMPTION]` End-to-end latency (Enter press → audio starts playing) should feel usable in live voice chat; no hard numeric target is set yet — flagged as an Open Question pending a real measurement during implementation.

#### FR-6: Playback through the Virtual Microphone
Generated audio plays out through the Virtual Microphone device rather than the system's default speaker output, so any application using that device as its mic input receives it as live audio.

**Consequences (testable):**
- On Linux and Windows, an application selecting the Virtual Microphone as its input device receives the generated audio, timed to the Speak Action.
- The user's real physical microphone is unaffected — voice-me does not mute, replace, or otherwise interfere with it.

### 4.3 Dependency Management
**Description:** Keeps the "single executable, no visible installer" promise honest without silently failing when something's missing.

#### FR-7: In-app Dependency Check and guided/one-click setup
On first run and on demand, the app checks for locally required components (the bundled Python runtime, GPU acceleration libraries, the chosen Virtual Microphone driver) and, where feasible, offers a one-click action to install or provision what's missing, inside the app UI.

**Consequences (testable):**
- A missing dependency is named specifically to the user (not a generic error), with either a one-click fix or clear manual next steps if automation isn't possible for that item.
- The app functions in a reduced/CPU-only mode (the "Chatterbox-Nano" CPU-oriented model variant) when GPU acceleration isn't available, rather than failing outright. `[ASSUMPTION]`

**Out of Scope:**
- A traditional setup wizard or requirements page shown before the app can be used at all — dependency handling is in-app and as-needed, not a blocking upfront step.

### 4.4 Localization
**Description:** UI-level language support, distinct from the TTS speech language.

#### FR-8: Turkish and English UI
The application interface (menus, settings, Prompt Overlay chrome, error messages) is available in Turkish and English, user-selectable.

**Consequences (testable):**
- Switching UI language takes effect without requiring a restart. `[ASSUMPTION]`
- UI language selection is independent from the TTS speech `language_id` used for generated audio (a Turkish UI can still generate English speech and vice versa).

### 4.5 Look & Feel
**Description:** The whole interface — Prompt Overlay, settings, tray menu, voice setup — should read as modern and clean, not a bare-bones dev tool. This is cross-cutting across every other feature in this section, rather than its own user-facing capability.

#### FR-9: Consistent, modern visual design via gpui-kit
The UI is implemented using gpui-kit components (`gpui_kit::component`, `gpui_kit::base`) rather than hand-rolled GPUI primitives wherever a suitable component exists, and follows the gpui-kit Design Guides for spacing, typography, color, density, and interaction states.

**Consequences (testable):**
- Standard UI elements (buttons, inputs, dialogs, the settings surface) are gpui-kit components, not one-off custom implementations, except where gpui-kit has no equivalent.
- Visual design decisions (color theme, spacing, component choice) are checked against `gpui-kit-design-guides` before being considered done, not designed ad hoc.
- `[NOTE FOR PM]` gpui-kit is a component/design layer on top of GPUI — it does not itself add system tray or global hotkey support (see FR-2/FR-3, Open Question 6). Using gpui-kit for visual polish does not resolve that separate feasibility risk.

## 5. Non-Goals (Explicit)

- Not a live, continuous voice changer — no real-time pass-through voice transformation.
- Not a cloud service — no server-side inference, no accounts, no telemetry beyond what stays on-device.
- Not a monetized product in v1 — free and open source.
- Not targeting macOS in v1.
- Not building Preset Phrase (hotkey → fixed phrase, no overlay) in v1 — real planned feature, deferred (see `brief.md` § Possible Future Features).
- Not supporting speech languages beyond what Chatterbox-Multilingual V3 ships with.

## 6. MVP Scope

### 6.1 In Scope
- Voice Setup (FR-1)
- Background/tray operation and global hotkey configuration through the Prompt Overlay speak flow (FR-2 through FR-6)
- Virtual Microphone output on both Linux and Windows (developed/validated on Linux first, then Windows, following whichever machine is in hand — not a strict phase gate)
- In-app Dependency Check with one-click setup where feasible (FR-7)
- Turkish/English UI (FR-8)
- Modern, clean visual design built with gpui-kit components, per gpui-kit's Design Guides (FR-9)
- Rust workspace structured as multiple crates (binary, lib(s), tests) — see `addendum.md` for the proposed breakdown

### 6.2 Out of Scope for MVP
- macOS support — deferred, no notarization/system-extension work scoped
- Preset Phrase hotkey-to-fixed-phrase mapping — deferred to v2, tracked in `brief.md`
- Cloud/hybrid inference — local-only is a hard requirement, not a v1-only default
- Accounts, licensing, monetization
- Saved/favorite phrase library, per-game profiles, and the other items under `brief.md` § Possible Future Features

## 7. Success Metrics

Hobby-scale — kept intentionally light:

**Primary**
- **SM-1**: Erdem uses voice-me during real gaming sessions instead of falling back to text chat or staying muted, at least weekly. Validates FR-2 through FR-6.

**Secondary**
- **SM-2**: End-to-end latency (hotkey press → audio heard) feels usable in live voice chat, based on the creator's own experience (no numeric target yet — see Open Questions). Validates FR-4, FR-5.
- **SM-3**: First-run setup (Voice Setup + Dependency Check) completes without needing a terminal or external documentation. Validates FR-1, FR-7.

**Counter-metrics (do not optimize)**
- **SM-C1**: Do not chase maximum voice-clone fidelity or additional TTS features at the expense of end-to-end latency — a slightly-less-perfect voice that responds fast beats a perfect one that breaks the live-chat moment. Counterbalances SM-2.

**Aspirational (not a v1 target)**
- If shared publicly beyond personal use, organic adoption/interest from other gamers with the same situational-mute need would validate it beyond a personal tool — not tracked or optimized for in v1.

## 8. Open Questions

1. What end-to-end latency (hotkey → audio playing) is actually achievable with Chatterbox-Multilingual V3 on typical consumer hardware, with and without GPU? No target is set until this is measured (affects FR-5, SM-2).
2. Does the chosen Windows virtual audio driver (`VirtualDrivers/Virtual-Audio-Driver`, release 25.7.14) actually support named-pipe/IPC control out of the box, or does that require a custom build from the maintainer as the general README suggests? (affects FR-6, addendum architecture notes)
3. Does global hotkey capture during fullscreen games risk conflicts with anti-cheat software in any target games? Needs investigation before committing to a specific hotkey-capture implementation (affects FR-3).
4. What are Chatterbox-Multilingual V3's actual minimum reference-clip length/quality recommendations? Should set the bounds enforced in FR-1.
5. Is the "bundled Python sidecar" packaging approach (see brief `addendum.md`) actually deliverable as a clean single-executable experience, or will early spikes surface blockers requiring a different approach?
6. Can GPUI (pre-1.0, no upstream tray or global-hotkey support) actually deliver FR-2 and FR-3 directly, or does this project need to depend on the unofficial "Adabraka GPUI" fork, or build platform-native tray/hotkey shims by hand? This is a foundational feasibility question for the whole UI shell, not a detail.

## 9. Assumptions Index

- §4.1 FR-1 — Minimum/maximum reference-clip length and import formats not yet specified; to be set from Chatterbox's own guidance during implementation.
- §4.2 FR-2 — Exact tray UX (icon, menu contents) per OS not yet designed.
- §4.2 FR-4 — Target under ~200ms hotkey-to-ready-for-input latency for the Prompt Overlay; no measured baseline yet.
- §4.2 FR-5 — No hard numeric end-to-end latency target set yet; tracked as Open Question 1.
- §4.3 FR-7 — App should degrade to a reduced/CPU-only mode when no GPU is available, rather than failing outright.
- §4.4 FR-8 — UI language switch takes effect live, without an app restart.
- §4.5 FR-9 — gpui-kit is assumed to cover enough standard components (buttons, inputs, dialogs) to avoid hand-rolled UI for most of the app; not yet verified against the actual component set.
