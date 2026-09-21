//! `voice-me-app` — composition root.
//!
//! Story 2.2's slice: the app is always tray-resident (`TrayPort::show` is
//! called unconditionally at startup) and the Voice Setup window only opens
//! automatically on first run (no active Reference Voice Sample yet). Once
//! running, the tray's "Settings…" item (re)opens/activates that same
//! window on demand via the shared `AppEvent` channel (AD-3).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::component::Root;
use gpui_kit::{App, AppContext as _, WindowHandle, WindowOptions};
use voice_me_core::{AppEvent, FileSettingsStore, SettingsStore};
use voice_me_ui::VoiceSetupView;

#[cfg(target_os = "linux")]
use voice_me_tray_linux::LinuxTrayAdapter;
#[cfg(target_os = "windows")]
use voice_me_tray_windows::WindowsTrayAdapter;

use voice_me_core::TrayPort as _;

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

    let app = gpui_kit::application().with_assets(gpui_kit::assets::Assets);

    app.run(move |cx| {
        gpui_kit::init(cx);

        let (event_tx, mut event_rx) = mpsc::unbounded::<AppEvent>();

        // Tracks the currently open Voice Setup window (if any), so a
        // second "Settings…" click activates the existing window instead of
        // opening a duplicate.
        let window_slot: Rc<RefCell<Option<WindowHandle<Root>>>> = Rc::new(RefCell::new(None));

        let open_settings = {
            let settings_store = settings_store.clone();
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
                let selected_mic_device = loaded_state.and_then(|state| state.selected_mic_device);

                let settings_store = settings_store.clone();
                let window_slot_on_close = window_slot.clone();
                let handle = match cx.open_window(WindowOptions::default(), move |window, cx| {
                    let view = cx.new(|cx| {
                        VoiceSetupView::new(
                            settings_store.clone(),
                            has_active_sample,
                            selected_mic_device,
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
                        let _ = cx.update(|cx| open_settings(cx));
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
