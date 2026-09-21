//! `voice-me-app` — composition root.
//!
//! Story 2.2's slice: the app is always tray-resident (`TrayPort::show` is
//! called unconditionally at startup) and the Settings window only opens
//! automatically on first run (no active Reference Voice Sample yet). Once
//! running, the tray's "Settings…" item (re)opens/activates that same
//! window on demand via the shared `AppEvent` channel (AD-3).
//!
//! Story 2.3 adds the `HotkeyPort` adapter alongside the tray: a hotkey
//! saved in a previous run is re-activated at startup, and each press
//! arrives here as `AppEvent::HotkeyPressed`.
//!
//! Story 2.4 makes that press do something: it summons the Prompt Overlay,
//! a borderless always-on-top window holding one `Input`. Confirming the
//! line comes back here as `AppEvent::SpeakRequested` (logged only until
//! Story 2.6 owns generation).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::component::Root;
use gpui_kit::{
    App, AppContext as _, QuitMode, WindowBackgroundAppearance, WindowBounds, WindowDecorations,
    WindowHandle, WindowKind, WindowOptions, px, size,
};
use voice_me_core::{AppEvent, FileSettingsStore, HotkeyPort, SettingsStore};
use voice_me_ui::{PromptOverlayView, SettingsView};

#[cfg(target_os = "linux")]
use voice_me_hotkey_linux::LinuxHotkeyAdapter;
#[cfg(target_os = "windows")]
use voice_me_hotkey_windows::WindowsHotkeyAdapter;
#[cfg(target_os = "linux")]
use voice_me_tray_linux::LinuxTrayAdapter;
#[cfg(target_os = "windows")]
use voice_me_tray_windows::WindowsTrayAdapter;

use voice_me_core::TrayPort as _;

/// The overlay's fixed comfortable width, and a height sized to one line of
/// input plus the surface's padding (spec-2-4 Design Notes).
const OVERLAY_WIDTH: f32 = 560.;
const OVERLAY_HEIGHT: f32 = 84.;

/// Always-on-top is a two-tier capability, mirroring Story 2.3's two hotkey
/// backends. `WindowKind::PopUp` is a real override-redirect, taskbar-less,
/// above-everything window under X11. Wayland has no equivalent in this GPUI
/// version — `PopUp` falls through to a plain xdg_toplevel and the
/// compositor places it itself — so a Wayland session gets an ordinary
/// focused window that may sit below a fullscreen game. `LayerShell` is
/// deliberately not used: GNOME/Mutter does not implement
/// `zwlr_layer_shell_v1`, so it would add a second unverifiable path for no
/// gain. The difference is documented in the README rather than worked
/// around.
#[cfg(target_os = "linux")]
fn overlay_window_kind() -> WindowKind {
    overlay_window_kind_for(voice_me_hotkey_linux::session_kind())
}

/// The decision itself, separated from reading the session the way
/// `voice_me_hotkey_linux::session_kind_for` is, so both arms can be
/// asserted without a window server.
#[cfg(target_os = "linux")]
fn overlay_window_kind_for(session: voice_me_hotkey_linux::SessionKind) -> WindowKind {
    match session {
        voice_me_hotkey_linux::SessionKind::X11 => WindowKind::PopUp,
        // Asking for `PopUp` here would not fail — it would simply behave
        // like `Normal`. Saying `Normal` keeps the code honest about what
        // this session actually gets.
        voice_me_hotkey_linux::SessionKind::Wayland => WindowKind::Normal,
    }
}

#[cfg(not(target_os = "linux"))]
fn overlay_window_kind() -> WindowKind {
    WindowKind::PopUp
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use voice_me_hotkey_linux::SessionKind;

    #[test]
    fn an_x11_session_gets_a_true_always_on_top_overlay() {
        assert_eq!(
            overlay_window_kind_for(SessionKind::X11),
            WindowKind::PopUp,
            "X11 is the only session where the overlay can be drawn over a fullscreen window"
        );
    }

    #[test]
    fn a_wayland_session_gets_a_plain_window() {
        assert_eq!(
            overlay_window_kind_for(SessionKind::Wayland),
            WindowKind::Normal,
            "`PopUp` would silently behave like `Normal` here; say so outright"
        );
    }
}

fn main() {
    let settings_store: Arc<dyn SettingsStore> =
        Arc::new(FileSettingsStore::new().expect("failed to resolve settings/data directories"));

    // Story 1.5: determine whether an active Reference Voice Sample already
    // exists before deciding whether to auto-open the window at startup
    // (Story 2.2 — first run only; otherwise the app stays tray-only). A
    // load failure is treated the same as "no sample" — falling back to the
    // empty-state copy is safe, whereas silently treating an unreadable
    // store as "has a sample" would hide the first-run prompt from someone
    // who actually needs it.
    let has_active_sample = settings_store
        .load()
        .ok()
        .is_some_and(|state| state.reference_voice_sample.is_some());

    // Tray residency (Story 2.2) only holds under an explicit quit mode.
    // gpui's default quits the process the moment the last window closes on
    // non-macOS — which would make the Prompt Overlay's own dismissal
    // (Story 2.4) kill a tray-only session on the first `Enter` or
    // `Escape`. The tray's "Quit" item already calls `cx.quit()` itself.
    let app = gpui_kit::application()
        .with_quit_mode(QuitMode::Explicit)
        .with_assets(gpui_kit::assets::Assets);

    app.run(move |cx| {
        gpui_kit::init(cx);

        let (event_tx, mut event_rx) = mpsc::unbounded::<AppEvent>();

        // The hotkey adapter is built here, with the shared `AppEvent`
        // sender, but binds nothing until there is a combination to bind —
        // so an app that has never had a hotkey configured starts with no
        // hotkey active and no error (including on a Wayland session with
        // no `input`-group membership).
        #[cfg(target_os = "linux")]
        let hotkey_port: Arc<dyn HotkeyPort> = Arc::new(LinuxHotkeyAdapter::new(event_tx.clone()));
        #[cfg(target_os = "windows")]
        let hotkey_port: Arc<dyn HotkeyPort> = Arc::new(WindowsHotkeyAdapter);

        // A failed `start_listening` (most importantly: no read access to
        // `/dev/input` on Wayland) must not stop the app — it starts
        // normally and the Hotkey tab says what the problem is, in words,
        // whenever the window is opened.
        let hotkey_startup_error: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

        // Tracks the currently open Settings window (if any), so a second
        // "Settings…" click activates the existing window instead of
        // opening a duplicate.
        let window_slot: Rc<RefCell<Option<WindowHandle<Root>>>> = Rc::new(RefCell::new(None));

        // The same one-at-a-time guarantee for the Prompt Overlay: a press
        // while one is already open activates it instead of stacking a
        // second window on top.
        let overlay_slot: Rc<RefCell<Option<WindowHandle<Root>>>> = Rc::new(RefCell::new(None));

        let open_settings = {
            let settings_store = settings_store.clone();
            let hotkey_port = hotkey_port.clone();
            let hotkey_startup_error = hotkey_startup_error.clone();
            let window_slot = window_slot.clone();
            move |cx: &mut App| {
                if let Some(handle) = window_slot.borrow().as_ref() {
                    let _ = handle.update(cx, |_, window, _| window.activate_window());
                    return;
                }

                // Reload fresh each time a window opens (not just once at
                // binary startup) so re-opening after a sample was saved
                // shows the normal view, not the first-run empty state.
                let loaded_state = settings_store.load().ok();
                let has_active_sample = loaded_state
                    .as_ref()
                    .is_some_and(|state| state.reference_voice_sample.is_some());
                let saved_hotkey = loaded_state.as_ref().and_then(|state| state.hotkey.clone());
                let selected_mic_device = loaded_state.and_then(|state| state.selected_mic_device);
                let hotkey_startup_error = hotkey_startup_error.borrow().clone();

                let settings_store = settings_store.clone();
                let hotkey_port = hotkey_port.clone();
                let window_slot_on_close = window_slot.clone();
                let handle = match cx.open_window(WindowOptions::default(), move |window, cx| {
                    let view = cx.new(|cx| {
                        SettingsView::new(
                            settings_store.clone(),
                            hotkey_port.clone(),
                            has_active_sample,
                            selected_mic_device,
                            saved_hotkey,
                            hotkey_startup_error,
                            window,
                            cx,
                        )
                    });
                    window.on_window_should_close(cx, move |_window, _cx| {
                        *window_slot_on_close.borrow_mut() = None;
                        true
                    });
                    cx.new(|cx| Root::new(view, window, cx))
                }) {
                    Ok(handle) => handle,
                    Err(error) => {
                        eprintln!("failed to open window: {error}");
                        return;
                    }
                };

                *window_slot.borrow_mut() = Some(handle);
            }
        };

        let open_overlay = {
            let overlay_slot = overlay_slot.clone();
            let event_tx = event_tx.clone();
            move |cx: &mut App| {
                // Re-summon while one is open: activate it, keeping whatever
                // is already typed. The view closes itself with
                // `remove_window`, which never runs `on_window_should_close`,
                // so a stale handle is normal — `update` failing is how we
                // learn the window is gone, and the slot is cleared so the
                // press still gets a fresh overlay.
                let existing = *overlay_slot.borrow();
                if let Some(handle) = existing {
                    if handle
                        .update(cx, |_, window, _| window.activate_window())
                        .is_ok()
                    {
                        return;
                    }
                    *overlay_slot.borrow_mut() = None;
                }

                let event_tx = event_tx.clone();
                let overlay_slot_on_close = overlay_slot.clone();
                let options = WindowOptions {
                    titlebar: None,
                    // Client-side decorations are what make a borderless
                    // window possible at all on Linux.
                    window_decorations: Some(WindowDecorations::Client),
                    window_background: WindowBackgroundAppearance::Transparent,
                    window_bounds: Some(WindowBounds::centered(
                        size(px(OVERLAY_WIDTH), px(OVERLAY_HEIGHT)),
                        cx,
                    )),
                    kind: overlay_window_kind(),
                    focus: true,
                    is_resizable: false,
                    is_movable: false,
                    is_minimizable: false,
                    ..Default::default()
                };

                let handle = match cx.open_window(options, move |window, cx| {
                    let view = cx.new(|cx| PromptOverlayView::new(event_tx.clone(), window, cx));
                    window.on_window_should_close(cx, move |_window, _cx| {
                        *overlay_slot_on_close.borrow_mut() = None;
                        true
                    });
                    cx.new(|cx| Root::new(view, window, cx))
                }) {
                    Ok(handle) => handle,
                    Err(error) => {
                        // The app stays tray-resident: a window that would
                        // not open is not a reason to lose the tray.
                        eprintln!("failed to open the prompt overlay: {error}");
                        return;
                    }
                };

                *overlay_slot.borrow_mut() = Some(handle);
            }
        };

        // Tray registration failure leaves the app with no way in unless we
        // fall back to opening the window — otherwise a headless process
        // with neither tray nor window would be silently unreachable.
        #[cfg(target_os = "linux")]
        if let Err(error) = LinuxTrayAdapter.show(cx, event_tx.clone()) {
            eprintln!("failed to show tray: {error}");
            open_settings(cx);
        }
        #[cfg(target_os = "windows")]
        if let Err(error) = WindowsTrayAdapter.show(cx, event_tx.clone()) {
            eprintln!("failed to show tray: {error}");
            open_settings(cx);
        }

        // Story 2.3: re-activate a hotkey saved in a previous run, after
        // the tray is up so a hotkey failure never costs the app its tray
        // presence. Nothing is bound when none was ever saved.
        if let Some(saved_hotkey) = settings_store.load().ok().and_then(|state| state.hotkey)
            && let Err(error) = hotkey_port.start_listening(&saved_hotkey, event_tx.clone())
        {
            eprintln!("failed to activate the hotkey {saved_hotkey}: {error}");
            *hotkey_startup_error.borrow_mut() = Some(error.to_string());
        }

        // First run only (Story 1.5 behavior, unchanged): auto-open the
        // window in addition to the tray being present. Otherwise the app
        // starts tray-only. Safe to call even if the tray-failure fallback
        // above already opened the window — `open_settings` activates the
        // existing window instead of duplicating it.
        if !has_active_sample {
            open_settings(cx);
        }

        cx.spawn(async move |cx| {
            while let Some(event) = event_rx.next().await {
                match event {
                    AppEvent::SettingsRequested => {
                        cx.update(|cx| open_settings(cx));
                    }
                    AppEvent::HotkeyPressed => {
                        cx.update(|cx| open_overlay(cx));
                    }
                    // Story 2.4 ends here: the Speak Action is observed and
                    // logged, nothing more. Story 2.6 owns generation and
                    // Story 2.9 playback.
                    AppEvent::SpeakRequested { text } => {
                        println!("speak requested: {text}");
                    }
                    // Not-yet-relevant variants (`#[non_exhaustive]` requires
                    // a wildcard arm).
                    _ => {}
                }
            }
        })
        .detach();
    });
}
