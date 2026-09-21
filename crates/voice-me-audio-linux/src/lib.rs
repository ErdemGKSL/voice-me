use voice_me_core::{AudioBuffer, VirtualMicPort, VoiceMeError};

/// Linux `VirtualMicPort` adapter (PipeWire/PulseAudio null-sink). Not yet
/// implemented — see later stories.
pub struct LinuxVirtualMicAdapter;

impl VirtualMicPort for LinuxVirtualMicAdapter {
    fn play(&self, _audio: &AudioBuffer) -> Result<(), VoiceMeError> {
        todo!()
    }
}
