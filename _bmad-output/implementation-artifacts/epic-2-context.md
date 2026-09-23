# Epic 2 Context: Speak Without Speaking (Core Loop)

<!-- Compiled from planning artifacts. Edit freely. Regenerate with compile-epic-context if planning docs change. -->

## Goal

The user presses a global hotkey from anywhere, including inside a fullscreen game. A minimal overlay appears, they type a line and press Enter, and shortly afterwards the line plays in their own cloned voice through a virtual microphone that other apps treat as a real mic. This is the whole reason the product exists, delivered end to end on Linux and Windows. The epic carries two foundational risks, and spikes resolve each one early. The first is that neither GPUI nor gpui-kit provides a system tray or global hotkey. The second is in-process ONNX inference together with the virtual-mic control surface. Linux is largely built. The current work is the Windows half of the tray (Story 2.2) and of the global hotkey through `RegisterHotKey` (Story 2.3).

## Stories

- Story 2.1: Spike: Background/Tray Presence Feasibility on GPUI/gpui-kit
- Story 2.2: Run as a Background/Tray-Resident Process
- Story 2.3: Configure a Global Hotkey
- Story 2.4: Summon, Type, and Dismiss the Prompt Overlay
- Story 2.5: Spike: In-Process Chatterbox Inference on ONNX Runtime
- Story 2.6: Generate Speech from the Prompt Overlay Text
- Story 2.7: Spike: Linux Virtual Microphone via PipeWire
- Story 2.8: Spike: Windows Virtual Microphone Control Surface
- Story 2.9: Play Generated Audio Through the Virtual Microphone

## Requirements & Constraints

- **Tray residency:** the app keeps running with no window open. It stays reachable through a tray icon whose menu has exactly two items, "Settings…" and "Quit", with no status submenu. Dismissing the overlay or closing Settings drops the app back to the tray.
- **Global hotkey:** one user-assignable combination. It must fire while a fullscreen or borderless-fullscreen game has input focus, on both Linux and Windows. A combination already in use is rejected inline, naming the conflicting app. It is never dropped silently. The previous working hotkey stays active until a new one is confirmed, and a failed Save leaves both the old binding and the settings file untouched.
- **Anti-cheat safety (must hold on Windows):** hotkey capture must not look like a keylogger to BattlEye or EasyAntiCheat. The investigation is complete. Message-based registration (`RegisterHotKey` on Windows, `XGrabKey` on X11) counts as ordinary app behaviour. Low-level hooks (`SetWindowsHookEx(WH_KEYBOARD_LL)`) and raw keystream readers carry a keylogger's signature and are a documented flagging pattern. Windows must therefore use `RegisterHotKey` only. Never use a low-level hook, a raw-input keystream, or evdev-style reading there.
- **Overlay:** appears ready for keystrokes within about 200ms of the hotkey. Enter closes it at once and fires the Speak Action without waiting on generation. Escape or focus loss closes it and discards the text.
- **Failing honestly:** generation failures and slow generation reach the user as OS-native notifications, never as silence. A hotkey or tray failure must never cost the app its tray presence, and it must be surfaced in words.
- **Audio output:** the Virtual Microphone carries the generated audio, and the physical mic is unaffected. Windows uses VirtualDrivers/Virtual-Audio-Driver 25.7.14, but how to control it is still unconfirmed (Story 2.8, still in backlog).
- **Distribution:** one native executable per OS with no installer.
- **Network:** no network egress from any Epic 2 adapter (hotkey, tray, audio).

## Technical Decisions

- **Hexagonal design:** `voice-me-core` owns every port trait (`HotkeyPort`, `TrayPort`, `VirtualMicPort`, `TtsPort`), `AppState` and `AppEvent`, and depends on no other `voice-me-*` crate. Adapters never depend on each other.
- **Per-OS crates, not `cfg` modules:** Windows work lives in `voice-me-tray-windows` and `voice-me-hotkey-windows`, and each implements the core port. `voice-me-app` selects crates through `[target.'cfg(target_os = ...)'.dependencies]`, so Windows-only dependencies must never leak into Linux builds.
- **Single state and event channel:** adapters hold only the `AppEvent` sender. They signal things like `HotkeyPressed` and `SettingsRequested` through it and never touch windows, the UI crate, or `AppState` directly. The composition root pumps events.
- **Errors:** adapters map their own OS/library errors into the core `thiserror` enum at the port boundary. `VoiceMeError::Other(String)` is the established wrapper, and `HotkeyAlreadyInUse` is the conflict signal. `anyhow` is used only in `voice-me-app`, and logging is `tracing`, initialised once there.
- **Tray:** use the `gpui-tray` crate (0.1, Apache-2.0) with its `gpui-kit` feature and `default-features = false`. Do not use the unverified "Adabraka GPUI" fork. Tray menus are plain `gpui::MenuItem`s that dispatch GPUI actions. The live tray handle must be kept in a GPUI `Global`, or the icon disappears. `TrayPort::show` takes `cx: &mut gpui_kit::App` plus the event sender. On Windows, `gpui-tray` uses `Shell_NotifyIconW` with a hidden top-level window and Win32 menus, and the icon sits in the notification area. If `gpui-tray` breaks, the fallback is `tray-icon` (tauri-apps), with its own event receiver pumped from `voice-me-app`.
- **Deferred Windows status:** the Windows tray and hotkey are both still `todo!()` stubs. Only their signatures match the trait. Because `main.rs` calls them unconditionally, a Windows build currently panics at startup and on every hotkey Save. Replacing these stubs is the current work. The tray should be resolved and verified before any Windows adapter that assumes tray presence.
- **Hotkey persistence:** the settings file stores hotkeys in `global-hotkey`'s accelerator syntax, for example `Ctrl+Alt+KeyV`, which round-trips through `HotKey::from_str`. The chip's display form is derived for rendering only and is never persisted. Linux uses `global-hotkey` on X11 and passive evdev on Wayland, chosen at runtime. The Windows path is `RegisterHotKey`, either directly or through `global-hotkey`'s Windows backend, which also uses it. Conflict detection comes from registration failure.
- **Rebind order:** register the new combination before unregistering the old one, so a rejected Save never leaves the user without a hotkey.
- **Stack:** Rust 1.98.1 on edition 2024. gpui-kit 0.6.4 is the only way GPUI enters the project, so never add `gpui` directly. CI runs separate `ubuntu-latest` and `windows-latest` jobs.

## UX & Interaction Patterns

- **Hotkey chips** use platform-native modifier names: Windows shows `Ctrl`/`Alt`/`Shift`/`Win`, and Linux shows `Super`, never `Meta`. Chips use a 12px medium monospace "shortcut" style on a `muted` background, and that style is used nowhere else.
- **Capture flow in Settings → Hotkey:** the user clicks "Change", presses a combination, it is captured live and shown as a chip, then the user Saves. A conflict shows inline at the field as "Already used by {app}."
- **Tray icon:** follows each OS's native status-icon convention and carries a tooltip and accessible name.
- **Overlay:** a custom borderless, always-on-top component with a fixed width and 12px radius, containing a pre-focused `Input` and nothing else. The only animation is a short fade/scale-in and fade-out, and it is fully keyboard-operable.
- **First-run default:** no hotkey is set by default, so first-run setup requires picking one.

## Cross-Story Dependencies

- Story 2.2 (tray) must work on Windows before the Windows hotkey is useful end to end, because the app has no other presence.
- Story 2.3 (hotkey) is what Story 2.4 (overlay) relies on. Its `AppEvent::HotkeyPressed` feeds the overlay.
- Story 2.4 triggers Story 2.6 (generation), which feeds Story 2.9 (playback). On Windows, Story 2.9 is blocked on Story 2.8 (the driver control surface).
- Epic 3 relies on the Settings shell (the Hotkey and Backend tabs) and on the fail-honestly surfaces.
