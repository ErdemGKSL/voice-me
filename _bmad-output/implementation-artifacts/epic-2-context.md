# Epic 2 Context: Speak Without Speaking (Core Loop)

<!-- Compiled from planning artifacts. Edit freely. Regenerate with compile-epic-context if planning docs change. -->

## Goal

Deliver the product's whole reason to exist, end to end: the user presses a global hotkey from anywhere (including inside a fullscreen game), a minimal overlay appears, they type a line and press Enter, and a moment later that line is spoken in their own cloned voice through a virtual microphone that other apps (voice chat) treat as a real mic. The app lives in the tray between uses. Two technical risks sit inside this one user outcome and are handled by spike stories early in the sequence: GPUI/gpui-kit have no upstream tray or global-hotkey support, and the in-process ONNX inference plus each OS's virtual-mic control surface are unproven.

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

- **Tray residency:** the app runs with no visible window; dismissing the overlay never exits. The tray menu has exactly two items, "Settings…" and "Quit", and follows each OS's native status-icon convention.
- **Global hotkey:** one user-assignable combination, changeable in Settings → Hotkey. It must fire while a fullscreen or borderless-fullscreen game holds focus, on Linux and Windows. A combination already in use elsewhere is rejected inline, naming the conflicting app; the previous working hotkey stays active until a new one is confirmed. Before the capture approach is locked in, check and document whether it could trigger anti-cheat software (BattlEye, EasyAntiCheat).
- **Overlay:** ready for keystrokes within about 200 ms of the hotkey (a target, not yet measured). Enter closes it immediately and triggers the Speak Action. Escape or focus loss closes it and discards the text. It must be fully usable from the keyboard alone, with no click needed to focus the input.
- **Generation:** typed text + active Reference Voice Sample + the selected speech language (independent of UI language) go through `TtsPort`. Local speech languages are only those needing no Python-only normalization (Turkish and English yes; Chinese, Japanese, Hebrew and Korean excluded). If generation fails, an OS-native notification names the short reason. If it is unusually slow, a brief OS-native notification says it is still working. It must never fail silently.
- **Playback:** audio comes out of the Virtual Microphone, never the speakers. It is timed to the Speak Action, and the user's physical mic is unaffected. Speech played out loud is the worst failure this product can have, so an unresolvable or ambiguous device must be a domain error, never a fallback to the default sink.
- **Latency reality:** a CPU-only (Q4) run measured about 20 s per utterance, after a one-off session build of about 90 s. Most of the cost is the vocoder (`conditional_decoder`), and it stays roughly the same whatever the line length. Async dispatch, a warm-up path and a "still loading" state are therefore required.
- No telemetry. Local TTS and audio crates open no sockets.

## Technical Decisions

- **Hexagonal layout:** `voice-me-core` owns domain types, `AppState`, `AppEvent`, all port traits (`HotkeyPort`, `TrayPort`, `TtsPort`, `VirtualMicPort`, `NotificationPort`, `SettingsStore`) and one `thiserror` error enum. Adapters map their own errors into that enum at the boundary, and `anyhow` is used only in `voice-me-app`. No adapter depends on another adapter.
- **Per-OS crates, not `cfg` modules:** `voice-me-{tray,hotkey,audio,notify}-{linux,windows}`, selected by `voice-me-app` through target-specific dependencies.
- **Events and state:** there is one `AppEvent` channel. Adapters hold only the sender, and `voice-me-app` is the only receiver. `AppState` changes only through core use-case functions, and `voice-me-ui` gets updates through `cx.spawn`.
- **Tray:** on Linux it uses the `gpui-tray` crate with its `gpui-kit` feature (not the Adabraka fork). `TrayPort::show` takes `cx: &mut gpui_kit::App`, and `gpui-tray` stays confined to the tray crates. Windows is planned to use the same crate but is unverified. The fallback is `tray-icon`.
- **Hotkey:** the planning docs still list global-hotkey feasibility (including during fullscreen games) as unresolved. Settle it before implementing the hotkey crates.
- **GPUI:** comes only through `gpui-kit` 0.6.4. Never add `gpui` directly.
- **Speak Action (AD-10):** dismissing the overlay is synchronous and never waits on `TtsPort::generate`. Generation runs on `tokio::task::spawn_blocking` through the in-house bridge `voice_me_core::tokio_bridge`: a `TokioRuntime` Global that owns the Runtime, and `spawn_blocking(cx, work)` feeding `cx.background_spawn`, with panics mapped to a domain error. Do not depend on Zed's `gpui_tokio`. Results come back through `AppEvent`.
- **Inference (AD-12):** runs in-process via `ort` 2.0.0-rc.13 with `load-dynamic`: no Python, no child process, no IPC. It uses four ONNX graphs (`speech_encoder`, `embed_tokens`, `language_model`, `conditional_decoder`) plus `tokenizer.json`, all from `onnx-community/chatterbox-multilingual-ONNX`. The loop prepends a `[<lang>]` tag, runs a KV-cached greedy decode with repetition penalty 1.2, and stops at token 6562 or `max_new_tokens`. Sessions are built from file paths (each `.onnx_data` must sit beside its `.onnx`), built once and lazily or on warm-up, and held for the process lifetime. At most one generation runs at a time; later ones queue.
- **Backend resolution (AD-9):** the TTS adapter reads the resolved backend, execution provider and weight variant from `AppState` (Q4 on CPU is the shipped default, FP16 on GPU) and never calls `voice-me-deps`. What the UI reports must come from what was actually acquired at session build. The reference clip is decoded and resampled to 24 kHz mono f32 with symphonia and rubato.
- **Audio buffer (AD-11):** one core type, f32 / 24 000 Hz / mono, is returned by `TtsPort::generate` and accepted by `VirtualMicPort::play`. Any format conversion happens inside the audio adapter.
- **Linux virtual mic:** uses the PulseAudio client API (`libpulse-binding`) with `module-null-sink sink_name=voice-me-sink` plus `module-remap-source source_name=voice-me master=voice-me-sink.monitor`. A single `Audio/Source/Virtual` node does not work. The audio adapter plays with `PA_STREAM_DONT_MOVE`, checks which device the stream actually reached, and refuses unless exactly one device matches. Install unloads any duplicates first. Persistence is a `~/.config/pipewire/pipewire-pulse.conf.d/voice-me.conf` `pulse.cmd` drop-in (not `pipewire.conf.d` `context.objects`). The server does the resampling. No privileges needed.
- **Windows virtual mic:** uses `VirtualDrivers/Virtual-Audio-Driver` 25.7.14. Its programmatic control surface (named pipe or otherwise) is unconfirmed and must be resolved, with a documented fallback, before `voice-me-audio-windows` is built.
- **Logging:** `tracing` is initialized once in `voice-me-app`. Spike outcomes are recorded as architecture decisions and close the related open questions.

## UX & Interaction Patterns

- **Prompt Overlay:** a custom component, not a stock gpui-kit surface. It is borderless and always-on-top, holds one pre-focused `Input` and nothing else, and uses the popover-family elevation, a 12px radius and a fixed, non-resizable width. It is assumed to be centered on the active monitor. Its only motion is a short fade/scale-in and a fade-out.
- **Hotkey capture:** click "Change", press the keys, and the combination renders live as a hotkey chip (monospace 12px medium on a `muted` background) for confirmation before Save. Chips use native modifier names: Ctrl/Alt/Shift/Super on Linux, Win on Windows.
- **After Enter:** a successful Speak Action shows no UI; the audio itself is the confirmation. On failure no overlay reappears; the user presses the hotkey again to retry.
- **Look and copy:** dark theme only, with gpui-kit tokens and a `#7C6AFF` accent. Microcopy is quiet and plain, e.g. "Couldn't generate speech. {reason}." Icon-only controls carry a tooltip and an accessible name.

## Cross-Story Dependencies

- 2.1 → 2.2 (tray approach). 2.2 + 2.3 → 2.4 (the overlay needs tray residency and a hotkey).
- 2.5 → 2.6 (proven inference loop, bridge and buffer type). 2.6 also needs 2.4's Speak Action and Epic 1's Reference Voice Sample.
- 2.7 / 2.8 → 2.9. 2.9 consumes 2.6's AD-11 buffer.
- 2.5's list of required runtime and model files is the input to Epic 3's dependency provisioning. Epic 3 extends `TtsPort` with other backends without changing the Speak Action path.
