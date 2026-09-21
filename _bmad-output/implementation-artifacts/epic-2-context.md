# Epic 2 Context: Speak Without Speaking (Core Loop)

<!-- Compiled from planning artifacts. Edit freely. Regenerate with compile-epic-context if planning docs change. -->

## Goal

Users press a global hotkey from anywhere — including inside a fullscreen game — get a minimal overlay, type a line, hit Enter, and a moment later hear that line spoken in their own cloned voice through a virtual microphone that other apps pick up as input. This is the product's entire reason to exist, delivered end to end: background/tray presence, hotkey configuration, the Prompt Overlay, speech generation by running Chatterbox-Multilingual V3's ONNX export in-process, and playback through a Virtual Microphone. Two distinct technical risks live inside this epic — GPUI/gpui-kit's missing tray+hotkey support, and the in-process ONNX inference loop plus the virtual-mic control surfaces — resolved via spike stories early rather than by splitting the epic, since both belong to one inseparable user outcome.

## Stories

- Story 2.1: Spike — Background/Tray Presence Feasibility on GPUI/gpui-kit
- Story 2.2: Run as a Background/Tray-Resident Process
- Story 2.3: Configure a Global Hotkey
- Story 2.4: Summon, Type, and Dismiss the Prompt Overlay
- Story 2.5: Spike — In-Process Chatterbox Inference on ONNX Runtime
- Story 2.6: Generate Speech from the Prompt Overlay Text
- Story 2.7: Spike — Linux Virtual Microphone via PipeWire
- Story 2.8: Spike — Windows Virtual Microphone Control Surface
- Story 2.9: Play Generated Audio Through the Virtual Microphone

## Requirements & Constraints

- The app runs tray-resident between uses: dismissing the Prompt Overlay never quits it. It stays reachable via a tray icon whose menu has exactly two items, "Settings…" and "Quit" — no status submenu.
- A single configurable global hotkey (unset until first configured) summons the Prompt Overlay from anywhere, including while a fullscreen or borderless-fullscreen game holds input focus, on both Linux and Windows. An already-in-use combination is surfaced inline naming the conflicting app, rejected before Save, never silently dropped; the previously working hotkey stays active until a new one is confirmed.
- Prompt Overlay: borderless, always-on-top, fixed non-resizable comfortable width, one single-line input and nothing else, pre-focused on appear so typing starts with no extra click. Target roughly 200ms from hotkey press to ready-for-keystrokes (an unmeasured aim, not a hard gate). Enter closes it immediately and fires the Speak Action without waiting on generation; Escape or focus loss closes it and discards the text. Only a short fade/scale-in on summon and fade-out on dismiss — no other animation anywhere.
- On a Speak Action the typed text, the active Reference Voice Sample, and the selected speech language go to the in-process inference engine; output is in the selected speech language, independent of the UI language. v1 speech languages are limited to those needing no Python-only text normalization — Turkish and English are in; Chinese, Japanese, Hebrew and Korean are out of v1.
- A generation failure surfaces as an OS-native notification naming the short reason — never silence, and no overlay reappears automatically (the user re-presses the hotkey to retry). Unusually slow generation gets a brief OS-native notification saying it's still working; there is no blocking UI, since the overlay is already gone.
- Generated audio plays through the Virtual Microphone device rather than the default speaker, on both Linux and Windows, timed to the Speak Action, so any app selecting it as input receives it. The user's real physical microphone is untouched.
- End-to-end latency (hotkey → audio playing) has no numeric target yet; it must feel usable in live voice chat and is something to measure during implementation. Explicitly do not trade this feel away for marginal voice-clone fidelity.
- Whether the chosen hotkey-capture approach risks anti-cheat conflicts (BattlEye, EasyAntiCheat) must be investigated and documented as part of the hotkey story before it is considered done.
- Hotkey chips render platform-native modifier names (Linux Ctrl/Alt/Shift/Super; Windows Ctrl/Alt/Shift/Win); the tray icon follows each OS's native status-icon convention.
- Accessibility: the Overlay is fully keyboard-operable from summon to dismissal with no extra click; icon-only controls (tray icon) carry a tooltip and accessible name; status is never conveyed by color alone.
- UI uses gpui-kit components wherever a suitable one exists and follows the gpui-kit Design Guides — checked before any story here is done.

## Technical Decisions

- Hexagonal/ports-and-adapters: `voice-me-core` owns domain types, the single `AppState`, and every port trait (`HotkeyPort`, `VirtualMicPort`, `TrayPort`, `TtsPort`), and depends on no adapter crate. Per-OS capabilities are separate crates, not `cfg`-gated modules: `voice-me-hotkey-{linux,windows}`, `voice-me-audio-{linux,windows}`, `voice-me-tray-{linux,windows}`. No adapter depends on a sibling — cross-adapter effects flow only through `AppState`/`AppEvent`.
- Single `AppState`/`AppEvent` ownership: only core use-cases mutate `AppState`; `voice-me-app` (composition root) is the sole `AppEvent` receiver and routes results into core and on to the UI. One `thiserror` domain error enum in core — adapters map their own failures (session/inference, driver, I/O) into it at the port boundary; `anyhow` aggregates only at the composition root.
- **TTS runs in-process — no Python, no child process, no IPC (AD-12).** `voice-me-tts` implements `TtsPort` by loading Chatterbox-Multilingual V3's ONNX export via the `ort` crate: `speech_encoder`, `embed_tokens`, `language_model`, `conditional_decoder`, plus `tokenizer.json` (all MIT, from `onnx-community/chatterbox-multilingual-ONNX`). The generation loop — prepend the language tag, tokenize, encode the reference clip once, run the KV-cached greedy decode with repetition penalty until the stop token or the token cap, then decode to waveform — is ported to Rust; it is tensor arithmetic, not model code. `ort` is configured `load-dynamic` so the ONNX Runtime library comes from the provisioned cache directory and is never downloaded by the build.
- **Inference session lifecycle and Speak sequencing (AD-10).** Overlay dismissal on Enter is synchronous and unconditional — the UI never awaits `TtsPort::generate`. The four `ort` sessions are built once, lazily (first generation or explicit warm-up), then held for the process lifetime — session construction is the multi-second cost, per-utterance inference is not. Exactly one generation runs at a time; a second Speak Action arriving mid-generation is queued, not run concurrently. All of this lives inside `voice-me-tts`; failures reach core only as the domain error enum.
- **Tokio/GPUI bridging (AD-5).** One in-house reimplementation of Zed's `gpui_tokio` pattern, owned by `voice-me-core` and shared by `voice-me-tts` and `voice-me-deps` — never a dependency on Zed's own workspace-internal crate, and never reinvented per adapter. Generation is CPU/GPU-bound and therefore runs on `tokio::task::spawn_blocking`, never on an async worker thread; GPUI's own `cx.spawn`/`cx.background_spawn` is reserved for UI-side async.
- **Shared audio buffer (AD-11).** One core-defined buffer type crosses `TtsPort::generate` → `VirtualMicPort::play` — never a raw byte slice or per-adapter struct. Its format is now fixed: **24 000 Hz, mono, 32-bit float** — straight out of `conditional_decoder.onnx` with no conversion on the TTS side. Any conversion to what a virtual-mic driver expects (typically 48 kHz) happens inside the `voice-me-audio-*` adapter. Reference clips are decoded and resampled to the same 24 kHz mono f32 before encoding (symphonia + rubato).
- **Backend selection (AD-9).** Which execution provider is usable (CUDA on NVIDIA, DirectML on Windows, CPU otherwise) is detected by `voice-me-deps` and reported only via `AppEvent`; core records it in `AppState` as one backend value. `voice-me-tts` reads that and picks both the execution provider and the matching language-model weight variant — FP16 on GPU, Q4 on CPU — and never calls `voice-me-deps` directly.
- No network egress outside `voice-me-deps`'s declared fetches; the inference engine opens no socket at all. Enforced by a CI check, not review discipline.
- **Tray (Linux) resolved:** `voice-me-tray-linux` uses the `gpui-tray` crate (v0.1, crates.io) rather than the unverified "Adabraka GPUI" fork — native to GPUI/gpui-kit, no second event loop. `TrayPort::show` takes a `cx: &mut gpui_kit::App`; core depends on gpui-kit for that context type only. Verified via DBus StatusNotifierItem registration. Fallback if it breaks: `tray-icon` (tauri-apps).
- **Tray (Windows) deferred:** no Windows dev environment available; `voice-me-tray-windows` remains a `todo!()` stub. Intended approach is the same crate's Windows backend (`Shell_NotifyIconW` + hidden top-level window + Win32 menus).
- **Hotkey feasibility still unresolved** on both OSes — must be settled in the hotkey story before either hotkey adapter is implemented.
- Linux Virtual Microphone: PipeWire/PulseAudio null-sink, no kernel driver and no elevated privileges (to be proven by spike). Windows Virtual Microphone: `VirtualDrivers/Virtual-Audio-Driver` 25.7.14 (SignPath-signed); its named-pipe/IPC control surface from an unsigned app is unconfirmed — the spike must confirm it or pick a documented fallback (custom build, other interface, other driver).
- Settings (hotkey binding, active Reference Voice Sample path, UI language, selected mic device) live in one TOML file behind `SettingsStore`, implemented inside `voice-me-core`; adapters never touch it. Large runtime assets (ONNX Runtime library and execution providers, model weights, driver installer) live in a separate cache directory owned solely by `voice-me-deps`.

## UX & Interaction Patterns

- Idle: tray icon only, no window. Overlay open: input focused, empty or mid-type, no other chrome. Success needs no confirmation UI — the audio playing through the Virtual Microphone is the confirmation.
- The Prompt Overlay is a custom composition, not a stock gpui-kit surface: popover-family elevation/shadow, the overlay-specific 12px radius, dark theme only, one `Input` and nothing else. No language switcher, menu, autocomplete, or command mode in it; no second global hotkey, no drag-and-drop, no resizing.
- Hotkey capture (Settings → Hotkey): click "Change", press the combination, it renders live as a hotkey chip for confirmation before Save; a conflict shows inline at the field ("Already used by {app}.").
- Hotkey chip typography is the reserved monospace shortcut style (12px, medium) on a `muted` background — never used for prose.
- Microcopy is quiet and direct, no exclamation marks or cutesy framing ("Couldn't generate speech. Try again." not "Oops! Something went wrong.").
- Blocking missing-dependency state (owned by Epic 3, but the Overlay must accommodate it): the Overlay still opens on hotkey press and shows an inline notice instead of accepting input until resolved.

## Cross-Story Dependencies

- Story 2.2 depends on Story 2.1's spike outcome (tray approach chosen: `gpui-tray` on Linux; Windows deferred/stubbed).
- Story 2.4 depends on Story 2.2 (tray presence running) and Story 2.3 (a hotkey configured).
- Story 2.6 depends on Story 2.4 (Speak Action triggered), on Epic 1 (an active Reference Voice Sample exists), and on Story 2.5's spike for the ported generation loop, the measured CPU/GPU latency, the session build-once confirmation, the Tokio/GPUI bridge, and the shared audio buffer type.
- Story 2.5 also produces the list of ONNX Runtime library and execution-provider files required per backend — a direct input to Epic 3's provisioning work.
- Story 2.9 depends on Story 2.6 (generated audio exists) and on Stories 2.7/2.8 (a working Virtual Microphone adapter per OS).
- Windows-side work here (tray, hotkey, virtual mic) is blocked on access to a Windows machine/toolchain, unavailable when the tray spike ran.
- Interlocks with Epic 3: Story 2.6 reads execution-provider capability from `AppState`, which Epic 3's dependency check populates; Story 3.4's missing-dependency notice renders inside the Overlay built in Story 2.4.
