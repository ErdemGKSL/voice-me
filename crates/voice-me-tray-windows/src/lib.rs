//! `voice-me-tray-windows` — the Windows `TrayPort` adapter.
//!
//! The same `gpui-tray` design as `voice-me-tray-linux`, over its Win32
//! backend (`Shell_NotifyIconW`). The tray is built on GPUI's main thread,
//! because GPUI's `GetMessageW(None)` loop is what delivers the tray's
//! hidden-window messages. Clicks reach the app only through the shared
//! `AppEvent` channel (AD-3).

use gpui_kit::{App, Global, MenuItem, actions};
use gpui_tray::{Icon, Tray};
use voice_me_core::{AppEvent, AppEventSender, TrayPort, VoiceMeError};

actions!(voice_me_tray_windows, [Settings, Quit]);

/// Holds the live `gpui-tray` handle as a GPUI global so it stays open for
/// the lifetime of the app. `Tray::close`/`Drop` remove the notification
/// area icon, so dropping this immediately after `WindowsTrayAdapter::show`
/// returns would make the icon vanish right away.
struct TrayHandle(#[allow(dead_code)] Tray);

impl Global for TrayHandle {}

/// Holds the sender half of the shared `AppEvent` channel (AD-3) as a GPUI
/// global, since the `Settings` action handler fires later, outside
/// `WindowsTrayAdapter::show`'s call frame, and needs somewhere to read it
/// from.
struct EventSender(AppEventSender);

impl Global for EventSender {}

/// Windows `TrayPort` adapter backed by `gpui-tray`'s `Shell_NotifyIconW`
/// backend.
pub struct WindowsTrayAdapter;

impl TrayPort for WindowsTrayAdapter {
    fn show(&self, cx: &mut App, events: AppEventSender) -> Result<(), VoiceMeError> {
        cx.set_global(EventSender(events));
        cx.on_action(on_settings);
        cx.on_action(on_quit);

        let tray = Tray::builder()
            .icon(tray_icon().map_err(|error| VoiceMeError::Other(error.to_string()))?)
            .title("voice-me")
            .tooltip("voice-me")
            .menu(build_menu)
            .build(cx)
            .map_err(|error| VoiceMeError::Other(error.to_string()))?;

        cx.set_global(TrayHandle(tray));
        Ok(())
    }
}

fn build_menu(_cx: &mut App) -> Vec<MenuItem> {
    menu_items()
}

/// The menu itself, separated from `build_menu`'s `App` parameter so its
/// exact contents are testable without a running GPUI app.
fn menu_items() -> Vec<MenuItem> {
    vec![
        MenuItem::action("Settings…", Settings),
        MenuItem::action("Quit", Quit),
    ]
}

fn on_settings(_: &Settings, cx: &mut App) {
    let _ = cx
        .global::<EventSender>()
        .0
        .unbounded_send(AppEvent::SettingsRequested);
}

fn on_quit(_: &Quit, cx: &mut App) {
    cx.quit();
}

/// A small solid-circle placeholder icon, drawn rather than bundled — the
/// same one the Linux tray shows.
fn tray_icon() -> gpui_tray::Result<Icon> {
    const SIZE: u32 = 32;
    let mut rgba = vec![0_u8; (SIZE * SIZE * 4) as usize];

    for y in 0..SIZE {
        for x in 0..SIZE {
            let offset = ((y * SIZE + x) * 4) as usize;
            let dx = 2 * x as i32 - (SIZE as i32 - 1);
            let dy = 2 * y as i32 - (SIZE as i32 - 1);
            if dx * dx + dy * dy <= (2 * 14) * (2 * 14) {
                rgba[offset..offset + 4].copy_from_slice(&[45, 105, 220, 255]);
            }
        }
    }

    Icon::from_rgba(rgba, SIZE, SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::Action as _;

    #[test]
    fn the_menu_is_exactly_settings_then_quit() {
        let items: Vec<(String, &'static str)> = menu_items()
            .into_iter()
            .map(|item| match item {
                MenuItem::Action { name, action, .. } => (name.to_string(), action.name()),
                _ => panic!("the tray menu holds only action items"),
            })
            .collect();

        assert_eq!(
            items,
            [
                ("Settings…".to_string(), Settings.name()),
                ("Quit".to_string(), Quit.name()),
            ]
        );
    }

    #[test]
    fn the_icon_is_a_valid_rgba_image() {
        assert!(tray_icon().is_ok());
    }
}
