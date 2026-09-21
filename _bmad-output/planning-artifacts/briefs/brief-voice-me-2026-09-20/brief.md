---
title: "Product Brief: voice-me"
status: draft
created: 2026-09-20
updated: 2026-09-20 (rev 2)
---

# Product Brief: voice-me

## Executive Summary

voice-me is a small desktop utility that lets you "talk" in your own cloned voice without actually speaking — useful mid-game, mid-call, or anywhere talking out loud isn't an option (late at night, an open office, a sore throat, or you just don't want to say it out loud). Press a hotkey, a tiny prompt appears over whatever you're doing, type a line, hit Enter — the prompt vanishes and your line plays back through a virtual microphone, in your own voice, so anything listening to your mic (a game's voice chat, Discord, Zoom) hears it as if you'd said it.

It's built as a single native executable (Rust + GPUI), runs entirely on the user's own machine (no cloud calls, no account, nothing leaves the device), and is free and open source. The voice itself comes from Chatterbox-Multilingual V3, an open, MIT-licensed voice-cloning TTS model — the user records a short reference clip of themselves once, and every generated line is spoken in that voice, in any of the ~23-25 languages the model supports.

## The Problem

Voice chat during games (and calls in general) assumes you can and want to speak out loud, continuously, in real time. That assumption breaks down constantly: you're in a shared room and can't talk without waking someone up or getting weird looks, your mic is bad or your voice is hoarse, you're multilingual and want to say something in a language you don't speak confidently out loud, or you simply want to say something short and precise without the friction of speaking, being heard mid-sentence by teammates, or fumbling the words live.

Today the workarounds are: type in text chat (slower, often not read in time during fast gameplay, and splits attention from the game), stay muted and miss out on voice coordination, or just don't say the thing. None of these give you the actual voice-chat experience — being *heard*, in a voice, at the moment it matters — without speaking.

## The Solution

voice-me sits quietly in the background. A user-defined global hotkey (works from inside a game, no alt-tabbing) pops a minimal, single-line prompt box. The user types, presses Enter, and the box disappears immediately — the interaction is designed to take a couple of seconds and not break flow. Behind the scenes, the typed text plus the user's own reference voice sample is fed to Chatterbox-Multilingual V3, and the resulting audio is played out through a virtual microphone device that the OS and other apps see as a normal input device — so whatever the user is already in (game voice chat, Discord, a call) just hears it, no special integration needed on the receiving end.

Setup is a one-time thing: record or import a reference voice clip, pick a hotkey, and go. If a required local dependency is missing (e.g. a Python/CUDA component the TTS engine needs), the app detects it and offers to install it from inside the UI — ideally one click — rather than sending the user to a README.

## What Makes This Different

The honest differentiator is the specific combination, not any one piece alone: existing voice changers (Voicemod, Voice.ai) transform your live voice in real time but require you to actually speak; typed-text TTS tools generally don't target "sound like *me*" or "route into a virtual mic other apps pick up as input" as a combined, game-first workflow. [ASSUMPTION] No existing tool was found in research that does exactly "type → your own cloned voice → virtual mic, positioned for in-game use" — this looks like open ground, but that also means there's no proven reference architecture to lean on (see Key Risks & Unknowns).

The other differentiator is what this *isn't*: no cloud processing, no account, no subscription, fully open source. For a privacy- and control-conscious user, that's a real and simple pitch, not a fabricated moat.

## Who This Serves

Primarily: the creator, as a personal hobby project — someone who plays voice-chat games and wants an occasional "spoken" line without speaking, in their own voice, without sending anything off their machine. [ASSUMPTION] Likely secondary audience if shared publicly: other gamers with similar situational-mute needs, and multilingual users who want to "speak" a language they type more comfortably than they pronounce. Since this starts as a hobby/open-source project, "success" is primarily about it working well for its own creator first.

## Technical Approach (proposed)

This is a starting sketch for the architecture conversation (`bmad-architecture` should own the real decisions) — flagged here because it materially affects scope and risk.

- **Shell & UI — GPUI (Rust).** Fits the "single native executable" goal and the target platforms. [ASSUMPTION] GPUI is pre-1.0 and has **no built-in system tray or global hotkey support** upstream — both are required here (background tray presence, hotkey works while a game has focus). Plan to either build these thin platform-native shims directly, or evaluate the community "Adabraka GPUI" fork that reportedly adds tray/hotkey/daemon-mode — but treat that as an unverified dependency to validate early, not assume.
- **TTS engine — Chatterbox-Multilingual V3.** *(Superseded 2026-09-21: a complete ONNX export does exist — `onnx-community/chatterbox-multilingual-ONNX`, MIT — so the model runs in-process from Rust via `ort` and the bundled-Python plan below is dropped. See Architecture Spine AD-12.)* No official ONNX or Rust-native binding exists today; the model runs via Python + PyTorch. [ASSUMPTION] Recommended approach: ship the Rust/GPUI app as the single user-facing executable, and have it manage a bundled, self-contained Python runtime (e.g. `python-build-standalone`) plus the model as a local sidecar process, talking over a local IPC channel (stdin/stdout or a local socket). The user never sees or manually installs Python — the app provisions it invisibly on first run, with GPU acceleration (CUDA/ROCm) auto-detected and used when present, falling back to the CPU-oriented "Chatterbox-Nano" variant otherwise. This preserves "no visible installer, no manual dependency step" without requiring a real ONNX/Rust port (out of scope for a hobby project's first version).
- **Virtual microphone — genuinely OS-specific, budget for this.** v1 targets Linux and Windows. Linux is straightforward (a PipeWire/PulseAudio null-sink, no driver, no signing). Windows requires an actual signed driver alongside the app (signed kernel/WDM virtual audio driver, VB-Cable's model). macOS (notarized CoreAudio HAL plug-in/driver extension, BlackHole's model) is deferred past v1. [ASSUMPTION] "Single executable" should be understood as "one app binary, driver installed and managed automatically by that app on first run" rather than literally zero supporting files — that distinction is worth being upfront about with any future users.
- **Hotkey overlay window.** A small, always-on-top, borderless prompt window that can be summoned/dismissed near-instantly from a global hotkey and grabs focus just long enough to type one line — this is a well-understood pattern (similar to Spotlight/Alfred/Raycast/PowerToys Run) and is the least risky part of the architecture.
- **Development sequencing.** Linux is built and validated first, then Windows — but pragmatically, work follows whichever machine is currently in front of the developer rather than a strict phase gate.

## Key Risks & Unknowns

- **Packaging Chatterbox without a visible Python install is the single biggest technical risk.** It's believed feasible (embedded/bundled Python runtime) but not yet proven for this specific model; worth a small spike before committing further.
- **Windows virtual-mic driver needs code signing** (EV certificate + driver signing/WHQL). This has real cost/process implications for a free hobby project and may require accepting an "unsigned driver, extra user trust step" experience early on, or investigating how VoxBooster claims to avoid a kernel driver entirely.
- **GPUI's tray/hotkey gap and pre-1.0 status** mean some foundational UI plumbing may need to be built by hand or borrowed from an unofficial fork — both add schedule risk and a dependency on a smaller community project.
- **No reference architecture exists** for the combined "typed text → cloned voice → virtual mic, game-first" flow — this is greenfield engineering, not integration of a known pattern.

Fuller research notes and sourcing live in `addendum.md`.

## Scope

**In for v1:**
- Record/import a reference voice clip (once, in-app)
- Global user-configurable hotkey → minimal prompt overlay → Enter to speak and dismiss
- Text-to-speech via Chatterbox-Multilingual V3, output routed to a virtual microphone
- Virtual mic creation on both Linux and Windows (developed and validated on Linux first, then Windows — following whichever machine is in hand rather than a strict gate)
- In-app dependency detection with guided/one-click setup for anything missing (Python runtime, GPU acceleration components)
- UI in Turkish and English

**Explicitly out for v1:**
- macOS support (deferred beyond v1 — notarized driver extension work not scoped yet)
- Predefined/preset hotkeys that speak a fixed phrase directly without opening the prompt overlay (e.g. `Alt+Shift+1` → "Merhaba") — planned, tracked under Possible Future Features, not in v1
- Cloud or hybrid inference (local-only is a hard requirement, not just a default)
- Accounts, licensing, monetization (free and open source)
- Real-time/continuous voice transformation (this is discrete, typed-line playback, not a live voice changer)
- Support for languages beyond what Chatterbox-Multilingual V3 ships with natively

## Possible Future Features (not committed, for later brainstorming)

A few directions worth exploring once v1 works, not commitments — captured here so they aren't lost:
- **Predefined hotkey → fixed phrase mapping** (e.g. `Alt+Shift+1` = "Merhaba"), spoken instantly with no prompt overlay at all — confirmed direction for a later version, deliberately kept out of v1
- A small library of saved/favorite phrases for one-key replay (common callouts, quick reactions) instead of retyping
- Per-game or per-app hotkey/profile switching (different voice or phrase sets per context)
- Adjustable delivery style/emotion if Chatterbox exposes such controls
- A push-to-type-then-auto-send mode vs. the current "type then Enter" — worth user-testing which feels faster mid-game
- Optional visual/audio confirmation that generation is in progress (latency is currently unknown and could be the biggest UX risk if generation takes more than ~1-2 seconds)

## Success Criteria

Since this is a personal hobby project, success is intentionally simple and self-referential first:
- The creator actually uses it during real gaming sessions instead of falling back to text chat or staying muted
- End-to-end latency (hotkey press → audio heard by teammates) feels usable in a live voice-chat context, not just technically functional
- Setup, including any auto-installed dependencies, works without the user needing to open a terminal or read a wiki
- [ASSUMPTION] If shared publicly later: organic adoption/stars from other gamers with the same situational-mute need would validate it beyond a personal tool

## Vision

If this works well personally, the natural next step is sharing it as a small open-source tool for the (likely niche but real) audience of gamers and voice-chat users who want to speak without speaking — in their own voice, in their own language, without sending anything to the cloud. It stays intentionally small in scope: a focused utility, not a platform.
