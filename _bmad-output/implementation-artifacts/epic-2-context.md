# Epic 2 Context: Speak Without Speaking (Core Loop)

<!-- Compiled from planning artifacts. Edit freely. Regenerate with compile-epic-context if planning docs change. -->

## Goal

Users press a global hotkey from anywhere — including inside a fullscreen game — get a minimal overlay, type a line, hit Enter, and a moment later hear that line spoken in their own cloned voice through a virtual microphone that other apps pick up as input. This is the product's entire reason to exist, delivered end to end: background/tray presence, hotkey configuration, the Prompt Overlay, TTS generation via in-process Chatterbox ONNX inference, and playback through a Virtual Microphone. Two distinct technical risks live inside this epic — GPUI/gpui-kit's missing tray+hotkey support, and the ONNX inference port plus the virtual-mic-driver control surface — resolved via spike stories early rather than a separate epic, since both belong to one inseparable user outcome.

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

- The app must run tray-resident between uses: dismissing the Prompt Overlay never quits the app; it stays reachable via a tray icon with exactly two items, "Settings…" and "Quit" — no status submenu.
- A single configurable global hotkey (default: unset until first configured) must summon the Prompt Overlay from anywhere, including while a fullscreen or borderless-fullscreen game holds input focus, on both Linux and Windows. Assigning a combination already in use elsewhere must be surfaced inline (naming the conflicting app), not silently dropped or accepted; the previously working hotkey stays active until a new one is confirmed.
- The Prompt Overlay: borderless, always-on-top, fixed non-resizable comfortable width, single-line `Input`, pre-focused on appear so typing starts with no extra click. Should be ready for keystrokes within roughly 200ms of the hotkey press (unmeasured target, not a hard number). `Enter` closes it immediately and fires the Speak Action without waiting on generation. `Escape` or focus loss closes it and discards the text without speaking. Only a short fade/scale-in on summon and fade-out on dismiss — no other animation.
- On Speak Action: typed text, the active Reference Voice Sample, and the selected speech language go to the in-process Inference Engine (Chatterbox-Multilingual V3's ONNX export on ONNX Runtime, AD-12); generated audio comes back in the selected speech language, independent of UI language. A generation failure must surface as a clear OS-native notification naming the short reason — never silence, and no overlay reappears automatically (user just re-invokes the hotkey to retry). If generation is unusually slow, a brief OS-native notification reports it's still working (no blocking UI, since the overlay is already gone).
- Generated audio must play through the Virtual Microphone device (not the default speaker) on both Linux and Windows, timed to the Speak Action, so any app selecting it as input receives the audio; the user's real physical microphone must be unaffected.
- End-to-end latency (hotkey → audio playing) has no fixed numeric target yet — it must "feel usable" in live voice chat; treat exact latency as something to measure during implementation, not a spec to hit blindly. Do not trade this feel away for marginal voice-clone fidelity gains.
- Whether hotkey capture risks anti-cheat conflicts (BattlEye, EasyAntiCheat) in target games must be investigated and documented as part of Story 2.3, before the hotkey-capture implementation is considered done.
- Hardware capability (GPU vs. CPU-only) must be read from `AppState`, never queried directly by the TTS adapter from the dependency adapter.
- Hotkey chips render platform-native modifier names (Linux: Ctrl/Alt/Shift/Super; Windows: Ctrl/Alt/Shift/Win); the tray icon follows each OS's native status-icon area convention (GNOME/KDE tray area on Linux, notification area on Windows).
- Accessibility: the Prompt Overlay must be fully keyboard-operable from summon to dismissal with no extra click; icon-only controls (tray icon) carry a tooltip and accessible name.
- UI components should use gpui-kit wherever a suitable component exists, following the gpui-kit Design Guides — checked before any story in this epic is considered done.

## Technical Decisions

- Hexagonal/ports-and-adapters: `voice-me-core` owns domain types, the single `AppState`, and every port trait (`HotkeyPort`, `VirtualMicPort`, `TrayPort`, `TtsPort`); it has zero dependency on any adapter crate. Per-OS capabilities are separate crates, not `cfg`-gated modules: `voice-me-hotkey-{linux,windows}`, `voice-me-audio-{linux,windows}`, `voice-me-tray-{linux,windows}`. No adapter depends on a sibling adapter — cross-adapter effects flow only through `AppState`/`AppEvent`.
- Single `AppState`/`AppEvent` ownership: only `voice-me-core` use-cases mutate `AppState` (hotkey binding, active Reference Voice Sample, UI language, selected virtual mic device). `voice-me-app` (composition root) is the sole `AppEvent` receiver and routes results into `voice-me-core` and up to `voice-me-ui` via `cx.spawn`.
- One `thiserror` domain error enum in `voice-me-core`; each adapter maps its own errors (IPC, driver, I/O) into it at the port boundary; `voice-me-app` aggregates with `anyhow` at the composition root.
- Tokio runs alongside GPUI's own executor via one in-house bridge crate/utility owned by `voice-me-core` (reimplementing Zed's `gpui_tokio` pattern — not a direct dependency on that crate, which is workspace-internal to zed-industries/zed). `voice-me-tts` (ONNX inference, on `spawn_blocking` since it is CPU/GPU-bound) and `voice-me-deps` run on Tokio; GPUI's `cx.spawn`/`cx.background_spawn` stays for UI-side async only.
- Speak Action sequencing (AD-10): dismissing the Prompt Overlay on Enter is synchronous and unconditional — `voice-me-ui` never waits on `TtsPort::generate`. Generation runs as a Tokio task dispatched immediately after dismissal; its result reaches the UI later via `AppEvent`. ONNX session lifecycle (built once lazily, held for the process lifetime, one generation in flight at a time) is owned entirely inside `voice-me-tts`.
- One shared audio buffer type (AD-11) crosses the `TtsPort` → `VirtualMicPort` boundary — never a raw byte slice or a per-adapter struct. Exact format (sample rate, bit depth, channel layout) is not yet fixed; determined by what Chatterbox actually outputs (Story 2.5) and finalized when the `voice-me-audio-*` adapters are built. Any format conversion happens inside the audio adapter.
- Hardware capability detection is core-owned (AD-9): `voice-me-deps` reports GPU availability only via `AppEvent`; `voice-me-tts` reads capability from `AppState`, never calls `voice-me-deps` directly.
- No network egress outside `voice-me-deps`'s declared GitHub-Releases fetches; the Inference Engine runs in-process and opens no socket at all (AD-12). Enforced by a CI check, not review discipline.
- **Tray (Linux) resolved**: `voice-me-tray-linux` uses the `gpui-tray` crate (v0.1, crates.io) rather than the unverified "Adabraka GPUI" fork — native to GPUI/gpui-kit, no second event loop to bridge. `TrayPort::show` takes a `cx: &mut gpui_kit::App` parameter; `voice-me-core` depends on `gpui-kit` for this context type only. Verified via DBus-level StatusNotifierItem registration. Fallback if `gpui-tray` proves broken: `tray-icon` (tauri-apps).
- **Tray (Windows) deferred**: no Windows dev environment was available to verify; `voice-me-tray-windows` stays a `todo!()` stub. Intended approach: same `gpui-tray` crate, Windows backend via `Shell_NotifyIconW` + hidden top-level window + Win32 menus. Resolve before Windows hotkey/audio adapters that assume tray presence.
- **Hotkey feasibility still unresolved** for both OSes — must be resolved in Story 2.3 before implementing `voice-me-hotkey-linux`/`-windows`.
- Linux Virtual Microphone: PipeWire/PulseAudio null-sink, no driver or elevated privileges needed (to be proven in Story 2.7).
- Windows Virtual Microphone: `VirtualDrivers/Virtual-Audio-Driver` (release 25.7.14, SignPath-signed); its named-pipe/IPC control surface from an unsigned app is unconfirmed — Story 2.8 must confirm it works out of the box or document a fallback (custom build, alternative interface, different driver).
- TTS runs in-process via the `ort` crate against `onnx-community/chatterbox-multilingual-ONNX` (MIT): `speech_encoder`, `embed_tokens`, `language_model` (FP16 on GPU / Q4 on CPU), `conditional_decoder`, plus `tokenizer.json`. No Python, no child process, no IPC (AD-12). Output is 24 kHz mono f32 (AD-11). v1 speech languages exclude zh/ja/he/ko, whose text normalization is Python-only.
- Settings (hotkey binding, active Reference Voice Sample path, UI language, selected mic device) live in one TOML file via `SettingsStore`, implemented inside `voice-me-core` — adapters never touch it directly.

## UX & Interaction Patterns

- Idle state: tray icon only, no window. Overlay-open state: `Input` focused, empty or mid-type, no other chrome.
- After Enter, there's no blocking UI while generation runs (the overlay is already closed). Slow generation → brief OS notification "still working." Success → no confirmation UI; the played audio through the Virtual Microphone is itself the confirmation. Failure → OS notification naming the short reason, e.g. "Couldn't generate speech. {short reason}."
- Hotkey conflict shown inline at the Settings → Hotkey capture field, e.g. "Already used by {app}," rejected before Save, not after.
- Blocking missing-dependency state: Settings → Dependencies auto-opens; the Prompt Overlay still opens on hotkey press but shows an inline notice instead of accepting input until resolved (this behavior belongs to Epic 3 but the Overlay must accommodate it).
- No second global hotkey, no in-overlay menu/autocomplete/command mode, no drag-and-drop, no resizable overlay — all explicitly out of scope for v1.
- Settings → Hotkey capture: click "Change," press the combination, it's captured live and rendered as a hotkey chip for confirmation before Save.
- Hotkey chip typography: monospace "shortcut" style (12px, medium weight), reserved exclusively for hotkey display, never prose; `muted` background.
- Voice/tone for microcopy: quiet, direct, no exclamation marks or cutesy framing (e.g. "Hotkey already in use by {app}." not "Error: hotkey conflict detected."; "Couldn't generate speech. Try again." not "Oops! Something went wrong.").
- Status is never color-only — any failure/conflict messaging carries words, not just color.

## Cross-Story Dependencies

- Story 2.2 (tray-resident background operation) depends on Story 2.1's spike outcome (tray approach chosen: `gpui-tray` on Linux; Windows deferred/stubbed).
- Story 2.4 (Prompt Overlay summon/dismiss) depends on both Story 2.2 (tray presence running) and Story 2.3 (a hotkey configured).
- Story 2.6 (speech generation) depends on Story 2.4 (Speak Action triggered) and on Epic 1 (an active Reference Voice Sample must exist), and depends on Story 2.5's spike outcome for the ported generation loop, the measured latency, and the confirmed shared audio buffer type.
- Story 2.9 (playback through the Virtual Microphone) depends on Story 2.6 (generated audio exists) and on Stories 2.7/2.8 (a working Virtual Microphone adapter per OS).
- Windows-side work in this epic (hotkey, tray, virtual mic) is generally blocked on dev-environment access to a Windows machine/toolchain, which was unavailable when the architecture spine was last updated — Story 2.1 explicitly deferred `voice-me-tray-windows`, and Story 2.8 exists specifically to resolve the Windows virtual-mic control-surface unknown before that adapter is built.
- Epic 3 (dependency management) and this epic interlock at the edges: Story 2.6 must read GPU/CPU capability from `AppState` (populated by Epic 3's dependency check), and the Overlay's missing-dependency inline notice (Epic 3, Story 3.4) is a behavior the Overlay built in Story 2.4 must be able to display.
