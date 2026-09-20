use voice_me_core::{TrayPort, VoiceMeError};

/// Windows `TrayPort` adapter. Not yet implemented — see later stories.
pub struct WindowsTrayAdapter;

impl TrayPort for WindowsTrayAdapter {
    fn show(&self) -> Result<(), VoiceMeError> {
        todo!()
    }
}
