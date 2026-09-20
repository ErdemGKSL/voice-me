use voice_me_core::{TtsPort, VoiceMeError};

/// `TtsPort` adapter — owns the Chatterbox sidecar Process lifecycle and IPC.
/// Not yet implemented — see later stories.
pub struct TtsAdapter;

impl TtsPort for TtsAdapter {
    fn generate(&self, _text: &str) -> Result<Vec<u8>, VoiceMeError> {
        todo!()
    }
}
