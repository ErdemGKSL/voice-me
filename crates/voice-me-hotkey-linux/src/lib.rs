use voice_me_core::{HotkeyPort, VoiceMeError};

/// Linux `HotkeyPort` adapter. Not yet implemented — see later stories.
pub struct LinuxHotkeyAdapter;

impl HotkeyPort for LinuxHotkeyAdapter {
    fn start_listening(&self, _hotkey: &str) -> Result<(), VoiceMeError> {
        todo!()
    }
}
