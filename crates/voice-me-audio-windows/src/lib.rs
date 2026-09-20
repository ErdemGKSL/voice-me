use voice_me_core::{VirtualMicPort, VoiceMeError};

/// Windows `VirtualMicPort` adapter (controls the signed Virtual-Audio-Driver).
/// Not yet implemented — see later stories.
pub struct WindowsVirtualMicAdapter;

impl VirtualMicPort for WindowsVirtualMicAdapter {
    fn play(&self, _audio: &[u8]) -> Result<(), VoiceMeError> {
        todo!()
    }
}
