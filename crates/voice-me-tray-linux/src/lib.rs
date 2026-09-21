use gpui_kit::{App, Global, MenuItem, actions};
use gpui_tray::{Icon, Tray};
use voice_me_core::{AppEvent, AppEventSender, TrayPort, VoiceMeError};

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

/// A small solid-circle placeholder icon (no bundled asset needed for this
/// spike, matching `gpui-tray`'s own example pattern).
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
