use voice_me_core::{AppEventSender, HotkeyPort, VoiceMeError};

/// Windows `HotkeyPort` adapter. Not yet implemented — see later stories;
/// this is a signature-compatible stub only, matching the
/// `voice-me-tray-windows` precedent.
///
/// When it is built, its backend must be `RegisterHotKey`. Low-level hooks
/// (`SetWindowsHookEx(WH_KEYBOARD_LL)`) and raw keystream readers share a
/// keylogger's signature and are a documented anti-cheat flagging pattern —
/// directly relevant to this epic's "works while a fullscreen game has
/// focus" goal. The `evdev` reader used by the Linux/Wayland adapter must
/// therefore never be ported to this path.
pub struct WindowsHotkeyAdapter;

impl HotkeyPort for WindowsHotkeyAdapter {
    fn start_listening(&self, _hotkey: &str, _events: AppEventSender) -> Result<(), VoiceMeError> {
        todo!()
    }

    fn rebind(&self, _hotkey: &str) -> Result<(), VoiceMeError> {
        todo!()
    }
}
