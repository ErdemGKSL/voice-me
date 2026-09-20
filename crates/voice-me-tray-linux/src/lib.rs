use voice_me_core::{TrayPort, VoiceMeError};

/// Linux `TrayPort` adapter. Not yet implemented — see later stories.
pub struct LinuxTrayAdapter;

impl TrayPort for LinuxTrayAdapter {
    fn show(&self) -> Result<(), VoiceMeError> {
        todo!()
    }
}
