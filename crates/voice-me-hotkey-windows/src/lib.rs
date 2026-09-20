use voice_me_core::{HotkeyPort, VoiceMeError};

/// Windows `HotkeyPort` adapter. Not yet implemented — see later stories.
pub struct WindowsHotkeyAdapter;

impl HotkeyPort for WindowsHotkeyAdapter {
    fn start_listening(&self, _hotkey: &str) -> Result<(), VoiceMeError> {
        todo!()
    }
}
