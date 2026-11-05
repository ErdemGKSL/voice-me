use gpui_kit::{App, Global, MenuItem, actions};
use gpui_tray::{Icon, Tray};
use voice_me_core::{AppEvent, AppEventSender, TrayPort, TrayVisualState, VoiceMeError};

actions!(voice_me_tray_linux, [Settings, Quit]);

/// Holds the live `gpui-tray` handle as a GPUI global so it stays open for
/// the lifetime of the app. `Tray::close`/`Drop` tear the native item down,
/// so dropping this immediately after `LinuxTrayAdapter::show` returns would
/// make the icon vanish right away.
struct TrayHandle(#[allow(dead_code)] Tray);

impl Global for TrayHandle {}

/// Holds the sender half of the shared `AppEvent` channel (AD-3) as a GPUI
/// global, since the `Settings` action handler fires later, outside
/// `LinuxTrayAdapter::show`'s call frame, and needs somewhere to read it
/// from.
struct EventSender(AppEventSender);

impl Global for EventSender {}

/// Linux `TrayPort` adapter backed by `gpui-tray`'s native StatusNotifierItem
/// + DBusMenu backend (no GTK, no second event loop — see spec-2-1).
pub struct LinuxTrayAdapter;

impl TrayPort for LinuxTrayAdapter {
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

    #[test]
    fn each_state_has_a_distinct_shape_and_named_tooltip() {
        let states = [
            TrayVisualState::Starting,
            TrayVisualState::Ready,
            TrayVisualState::Generating,
            TrayVisualState::Playing,
            TrayVisualState::Attention("speech blocked".into()),
        ];
        let icons: Vec<_> = states
            .iter()
            .map(|state| tray_icon(state).unwrap())
            .collect();
        for (index, icon) in icons.iter().enumerate() {
            assert!(icons.iter().skip(index + 1).all(|other| other != icon));
        }
        assert_eq!(
            states[4].tooltip(),
            "voice-me — attention needed; open Settings"
        );
    }
}
