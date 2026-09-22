use voice_me_core::{AudioBuffer, VirtualMicPort, VoiceMeError};

/// Windows `VirtualMicPort` adapter (controls the signed Virtual-Audio-Driver).
/// Not yet implemented — Story 2.8 builds it.
pub struct WindowsVirtualMicAdapter;

impl VirtualMicPort for WindowsVirtualMicAdapter {
    /// Reports the gap instead of panicking.
    ///
    /// Unlike the tray and hotkey stubs, this one sits on a path the Speak
    /// Action actually takes: with Story 2.9 wiring playback into `speak`, a
    /// `todo!()` here would abort the process on the first line a Windows
    /// user typed. A domain error instead means the same thing every other
    /// adapter failure means — one notification naming what is missing, and
    /// an app that keeps running (spec-2-9 Decision 2).
    fn play(&self, _audio: &AudioBuffer) -> Result<(), VoiceMeError> {
        Err(VoiceMeError::VirtualMicUnavailable(
            "the Windows virtual microphone is not implemented yet (Story 2.8) — \
             voice-me can generate speech on this machine but has nowhere to play it"
                .to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The matrix row this stub exists for: a Speak Action on Windows ends
    /// in a notification naming the gap, not in an aborted process. The
    /// assertion is deliberately about the *variant*, because that is what
    /// `speak` matches on to choose the playback title over the generation
    /// one.
    #[test]
    fn a_speak_action_on_windows_reports_the_gap_instead_of_panicking() {
        let error = WindowsVirtualMicAdapter
            .play(&AudioBuffer::new(vec![0.1; 24]))
            .unwrap_err();

        assert!(
            matches!(&error, VoiceMeError::VirtualMicUnavailable(reason)
                if reason.contains("2.8")),
            "the user has to be told which half is missing: {error}"
        );
    }
}
