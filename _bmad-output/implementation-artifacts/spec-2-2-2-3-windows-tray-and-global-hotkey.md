---
title: 'Windows Tray and Global Hotkey'
type: 'feature'
created: '2026-09-23'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
context: []
baseline_commit: '28bf6f2e184a3292aa7a5d6f76ad86951b6faa82'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** On Windows the app cannot run: the workspace does not compile (two Linux-only crates are not target-gated), and once it does, `WindowsTrayAdapter::show` and `WindowsHotkeyAdapter` are `todo!()`, so startup panics and no global hotkey can be bound.

**Approach:** Gate the Linux-only crates so the workspace builds on Windows, then implement the Windows tray (same `gpui-tray` design as Linux, Win32 backend) and the Windows hotkey (`global-hotkey`, which uses `RegisterHotKey` on Windows), mirroring the Linux adapters. Hotkey chips show `Win` instead of `Super` on Windows (UX-DR21).

## Boundaries & Constraints

**Always:** Tray menu is exactly "Settings…" then "Quit", with tooltip "voice-me", and the tray handle lives in a GPUI `Global`. Adapters talk to the app only through `AppEventSender` (AD-3) and never depend on `voice-me-ui` or on each other. The hotkey registers the new combination before unregistering the old one, and a combination already registered anywhere maps to `VoiceMeError::HotkeyAlreadyInUse`. The persisted accelerator string is unchanged (`Super+…` stays `Super` on disk); only the displayed chip says `Win`. `GlobalHotKeyManager` and the tray are created on GPUI's main thread, because GPUI's `GetMessageW(None)` loop is what delivers their hidden windows' messages.

**Never:** No `SetWindowsHookEx`, low-level keyboard hook, raw input or keystream reading (anti-cheat: keylogger signature). No toast notifications, Windows speech engine (3.13) or virtual mic (2.8) — all deferred. No 3.12 review patches beyond adding the `cfg` gate the Windows build needs. No `cfg` modules inside shared crates for capabilities (AD-2); the one allowed `cfg!` is the chip's display label.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Startup, sample saved | Windows, active sample | Tray icon in notification area, no window, process stays up | Tray failure → existing fallback opens Settings |
| Tray Settings… | click | Settings window opens (or existing one activates) | N/A |
| Tray Quit | click | Process exits | N/A |
| Saved hotkey | `Ctrl+Alt+KeyV` in settings | Registered at startup; each press → one `HotkeyPressed`, even with another app focused | Register failure → existing Hotkey-tab error path |
| Conflict | combination held by another process | Save rejected inline, old binding and settings file unchanged | `HotkeyAlreadyInUse` |
| Rebind | new valid combination | New fires, old no longer does | N/A |
| No hotkey ever set | first run | No manager created, no error | N/A |

</frozen-after-approval>

## Code Map

- `crates/voice-me-tts-system-linux/src/lib.rs`, `tests/espeak.rs` -- add `#![cfg(target_os = "linux")]`, like `voice-me-audio-linux/src/lib.rs:29`. Today they fail on Windows (`std::os::unix`).
- `crates/voice-me-hotkey-linux/src/lib.rs`, `examples/hotkey-spike.rs` -- same gate. The `evdev` imports fail on Windows. Gate the example's body the way `voice-me-audio-linux/src/bin/mic-spike.rs:29-56` does (a non-Linux `main` stub).
- `crates/voice-me-tray-linux/src/lib.rs` -- reference implementation (L5 actions, L29-41 globals + builder, L46-51 menu, L66-82 drawn RGBA icon). Its code is platform-neutral, so copy it into `voice-me-tray-windows` with the namespace and names renamed.
- `crates/voice-me-tray-windows/src/lib.rs`, `Cargo.toml` -- replace `todo!()`. Add `gpui-tray = { version = "0.1", default-features = false, features = ["gpui-kit"] }`. Its Windows backend uses `Shell_NotifyIconW` and re-adds the icon on `TaskbarCreated`.
- `crates/voice-me-hotkey-linux/src/x11.rs` -- reference: `X11Backend { manager, current }`, `start` (L26-40), `rebind` (L46-64), `parse_hotkey`, `map_register_error` (L71-76, `AlreadyRegistered` → `HotkeyAlreadyInUse`), `spawn_event_pump` (L82-105, forwards `Pressed` only), and tests at L107+. Copy the adapter's lazy `Mutex<Option<…>>` shape from `lib.rs` (`new(events)`, `bind`, `lock`) without the session detection.
- `crates/voice-me-hotkey-windows/src/lib.rs`, `Cargo.toml` -- replace `todo!()`. Add `global-hotkey = "0.8"` (Windows backend: `RegisterHotKey` on a hidden window; `ERROR_HOTKEY_ALREADY_REGISTERED` → `Error::AlreadyRegistered`). Keep the existing anti-cheat doc comment. Add `WindowsHotkeyAdapter::new(events: AppEventSender)`.
- `crates/voice-me-app/src/main.rs` -- L1774-1775: build `WindowsHotkeyAdapter::new(event_tx.clone())` to match Linux L1772-1773. The tray call at L2451-2455 and `start_listening` at L2461 need no change.
- `crates/voice-me-ui/src/hotkey.rs` -- `display_hotkey` L119: show `Win` when `cfg!(target_os = "windows")`, `Super` otherwise. Adjust the test at L1085-1087 so it asserts the platform's label. `capture_keystroke` L79 keeps writing `Super`.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-tts-system-linux/src/lib.rs`, `tests/espeak.rs`, `crates/voice-me-hotkey-linux/src/lib.rs`, `examples/hotkey-spike.rs` -- Linux-only gates -- the workspace compiles on Windows
- [x] `crates/voice-me-tray-windows/src/lib.rs`, `Cargo.toml` -- real `TrayPort` -- startup no longer panics and the tray works
- [x] `crates/voice-me-hotkey-windows/src/lib.rs`, `Cargo.toml` -- real `HotkeyPort` over `global-hotkey`, plus unit tests: parse round-trip, error mapping, and a registration conflict between two managers in the same process mapping to `HotkeyAlreadyInUse` -- global hotkey with conflict detection
- [x] `crates/voice-me-app/src/main.rs` -- construct the adapter with the sender -- wiring
- [x] `crates/voice-me-ui/src/hotkey.rs` -- `Win` label and its test -- UX-DR21

**Acceptance Criteria:**
- Given this Windows machine, when `cargo build --workspace` and `cargo test --workspace` run, then both succeed, and so does the Linux CI job
- Given the built `voice-me.exe` with a saved hotkey, when a synthesized press of that combination is sent (e.g. `keybd_event` from PowerShell) while another window has focus, then the Prompt Overlay appears

## Implementation Notes

- Outside the Code Map: `voice-me-notify-linux`'s one test is now `cfg(all(test, target_os = "linux"))`. On Windows `notify-rust` shows a toast instead of using D-Bus, so the test failed and blocked `cargo test --workspace`.
- `main.rs`: `#[allow(clippy::arc_with_non_send_sync)]` on the Windows `hotkey_port`. `GlobalHotKeyManager` holds an `HWND`, so the adapter is not `Send`. It must stay on the main thread anyway.
- The real-registration conflict test is `cfg(target_os = "windows")`, because the Ubuntu job also tests this crate and has no X server.
- Manual run: the hotkey registered, and a second `RegisterHotKey` of the same combination got error 1409. A `keybd_event` press opened the overlay. **Risk:** when Settings was already open and a dependency blocker existed, the overlay closed again after about 80 ms. GPUI's Windows `activate()` is deferred through `executor.spawn`, so the "Settings first, overlay second" activation in the `HotkeyPressed` handler actually ran in the opposite order. Settings took focus back, and the overlay closed itself because it lost activation. With Settings closed, the overlay stayed open. Not fixed here (out of scope).
- Matrix test audit (step 3): added `menu_items()`, split out of `build_menu` so it can be tested, plus two tray tests (`the_menu_is_exactly_settings_then_quit`, `the_icon_is_a_valid_rgba_image`) and the hotkey test `no_combination_means_nothing_is_registered`. How each row is covered: Conflict and Rebind by `a_combination_held_elsewhere_is_a_conflict_and_keeps_the_old_binding`; No hotkey by the new lazy test; the tray-menu contents by the menu test. Startup tray, Settings… click, Quit click and the focused-elsewhere press need the real notification area or real input, so they are covered by the manual checks only. The overlay-on-press check passed in the manual run.
- `cargo test --workspace --no-fail-fast` on Windows: every suite passes. One earlier full run failed `voice-me-ui` `dependencies::tests::check_again_and_install_forward_the_selected_backend` (a gpui `test_scheduler.rs:193` panic) while the machine was still compiling. It passes alone three times and in the full re-run. It is a timing flake, and it is not in this diff.

## Spec Change Log

## Review Triage Log

Pass 1 (blind-hunter, edge-case-hunter, verification-gap):

- **The pump forwards any `Pressed` id, and the `rebind` comment says a stale registration "maps to nothing"** (blind, edge claim). `low`. The comment is false: `spawn_event_pump` never checks `event.id()`. The only reachable stale registration is a failed `unregister`, and no second manager exists in the app. Route: patch the comment. The id filter is rejected: it adds shared state for a case users won't meet.
- **Pump spawn failure returns `Ok`** (blind, edge). `low`. Verified at `Backend::start`. The fix is a direct `?`. Route: patch.
- **A later `start_listening` sender is ignored once a backend exists** (blind, edge). `false`. Every sender is a clone of the one `event_tx` (`main.rs:1780`, `main.rs:2466`), so presses reach the same receiver either way.
- **`events: Mutex<Option<…>>` holds an unreachable `None` branch** (blind). `low`, developer-only. It mirrors `LinuxHotkeyAdapter`, as the Code Map directed. Rejected.
- **Nothing enforces the main-thread requirement** (blind). `low`. Every caller (`main.rs` startup, the Settings UI) runs on GPUI's main thread. A guard would add complexity with no reachable trigger. Rejected.
- **`evdev`/`global-hotkey` are not target-gated in `voice-me-hotkey-linux/Cargo.toml`** (blind). `low`. They compile on Windows once and are cached, and `global-hotkey` is needed on Windows anyway. Rejected.
- **The Pressed-only filter is untested** (verification-gap, blind). `medium`, pre-verified. Route: patch (a `forwards` helper plus tests).
- **Rebinding the live combination is untested** (verification-gap). `medium`, pre-verified. Route: patch.
- **The adapter-level first-time `rebind` path is untested** (verification-gap, blind). `medium`, pre-verified. Route: patch.
- **The tray `Settings…` action sending `SettingsRequested` is untested** (verification-gap). `medium`, pre-verified. It needs a GPUI `TestAppContext` harness, and the Linux tray has the same gap. Route: defer.
- **The real-registration test may fail if the CI runner refuses `RegisterHotKey`** (blind). `maybe-false`. It would be `medium`. The first `windows-latest` run settles it. Route: defer (unverified).
- **Pump threads leak, blocked on the global receiver** (blind, edge). `low`. In the app the adapter lives for the whole process. In tests the leaked threads read no one's events. Rejected.
- **Tray code duplicates the Linux tray** (blind). `false` as a defect: AD-2 requires separate per-OS crates that never depend on each other.
- **A left click on the tray icon does nothing** (blind). `low`. The spec fixes a two-item menu, and adding a click handler is more than a direct correction. Rejected.
- **`show` registers the handlers before `build` can fail, and calling `show` twice would register twice** (blind). `low`/`false`. On failure the app falls back to opening Settings, and the leftover handlers are harmless. `show` is called once. Rejected.
- **The `display_hotkey` test repeats the `cfg!` it tests** (blind). `low`. The fix is a direct correction. Route: patch.
- **No test that capture persists `Super` on Windows** (blind). `low`. `capture_keystroke` writes `Super` with no platform branch. Rejected.
- **`any_other_registration_failure_stays_generic` uses a made-up error, and a parseable key with no Windows virtual-key code fails with a raw message** (blind). `false` for the test: it exercises the `other` arm that every non-conflict variant takes. The raw message is `low`, rare, and the fix is more than a direct correction. Rejected.

## Verification

**Commands:**
- `cargo build --workspace` -- expected: success on Windows
- `cargo test --workspace` -- expected: success on Windows (the Linux-gated tests compile to nothing)

**Manual checks (if no CLI):**
- Run `target/debug/voice-me.exe` with a saved sample: the process stays alive and a tray icon appears. Right-click opens "Settings…" / "Quit"; each works.
- In Settings → Hotkey, save `Ctrl+Alt+V`: the chip reads `Ctrl+Alt+V`, and `Win` shows for the Windows key. A synthesized press opens the overlay. With a second process holding the combination via `RegisterHotKey`, Save shows the inline conflict.
