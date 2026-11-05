//! `voice-me-tray-windows` — the Windows `TrayPort` adapter.
//!
//! The same `gpui-tray` design as `voice-me-tray-linux`, over its Win32
//! backend (`Shell_NotifyIconW`). The tray is built on GPUI's main thread,
//! because GPUI's `GetMessageW(None)` loop is what delivers the tray's
//! hidden-window messages. Clicks reach the app only through the shared
//! `AppEvent` channel (AD-3).

use gpui_kit::{App, Global, MenuItem, actions};
use gpui_tray::{Icon, Tray};
use voice_me_core::{AppEvent, AppEventSender, TrayPort, TrayVisualState, VoiceMeError};

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
            .icon(
                tray_icon(&TrayVisualState::Starting)
                    .map_err(|error| VoiceMeError::Other(error.to_string()))?,
            )
            .title("voice-me")
            .tooltip(TrayVisualState::Starting.tooltip())
            .menu(build_menu)
            .build(cx)
            .map_err(|error| VoiceMeError::Other(error.to_string()))?;

        cx.set_global(TrayHandle(tray));
        Ok(())
    }

    fn set_visual(&self, cx: &mut App, state: &TrayVisualState) -> Result<(), VoiceMeError> {
        if !cx.has_global::<TrayHandle>() {
            return Ok(());
        }
        let icon = tray_icon(state).map_err(|error| VoiceMeError::Other(error.to_string()))?;
        let tray = cx.global::<TrayHandle>().0.clone();
        tray.set_icon(Some(icon), cx)
            .map_err(|error| VoiceMeError::Other(error.to_string()))?;
        tray.set_tooltip(Some(state.tooltip()), cx)
            .map_err(|error| VoiceMeError::Other(error.to_string()))
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

/// Raster exports of the same five-bar waveform identity. The source SVG
/// and platform PNG/ICO exports live alongside these RGBA bytes.
fn tray_icon(state: &TrayVisualState) -> gpui_tray::Result<Icon> {
    let bytes: &[u8] = match state {
        TrayVisualState::Starting => include_bytes!("../../../assets/tray/starting.rgba"),
        TrayVisualState::Ready => include_bytes!("../../../assets/tray/ready.rgba"),
        TrayVisualState::Generating => include_bytes!("../../../assets/tray/generating.rgba"),
        TrayVisualState::Playing => include_bytes!("../../../assets/tray/playing.rgba"),
        TrayVisualState::Attention(_) => include_bytes!("../../../assets/tray/attention.rgba"),
    };
    Icon::from_rgba(bytes.to_vec(), 32, 32)
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
        let states = [
            TrayVisualState::Starting,
            TrayVisualState::Ready,
            TrayVisualState::Generating,
            TrayVisualState::Playing,
            TrayVisualState::Attention("missing microphone".into()),
        ];
        let icons: Vec<_> = states
            .iter()
            .map(|state| tray_icon(state).unwrap())
            .collect();
        for (index, icon) in icons.iter().enumerate() {
            assert!(icons.iter().skip(index + 1).all(|other| other != icon));
            assert!(states[index].tooltip().contains(match index {
                0 => "starting",
                1 => "ready",
                2 => "generating",
                3 => "playing",
                _ => "open Settings",
            }));
        }
    }
}
