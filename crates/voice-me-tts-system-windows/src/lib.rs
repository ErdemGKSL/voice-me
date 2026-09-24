//! `voice-me-tts-system-windows` — the System voice on Windows (Story 3.13).
//!
//! A [`voice_me_core::TtsPort`] over WinRT's
//! `Windows.Media.SpeechSynthesis.SpeechSynthesizer`, with the voices
//! Windows has installed (e.g. Microsoft Tolga for Turkish). The engine
//! renders into a stream, never to the speaker; its WAV is decoded,
//! downmixed and resampled here to AD-11's 24 kHz mono f32.
//!
//! The WAV decoding and the voice mapping are pure and compiled on every
//! OS, so they are tested on Linux CI too; the WinRT module is Windows
//! only. A stock voice: no Reference Voice Sample, no socket (AD-8), no
//! disclosure.

#[cfg(target_os = "windows")]
mod engine;
mod voices;
mod wav;

use std::time::Duration;

use voice_me_core::VoiceMeError;

#[cfg(target_os = "windows")]
pub use engine::{SystemVoiceWindows, count_voices, list_voices};
pub use voices::{RawVoice, default_first, to_stock_voices};
pub use wav::decode_wav;

/// The engine's name as the user reads it.
pub const ENGINE_LABEL: &str = "Windows speech";

/// How long speaking one line may take.
pub const DEADLINE: Duration = Duration::from_secs(60);

/// How long listing or counting the voices may take. Shorter than
/// [`DEADLINE`]: the Dependency Check waits on it.
pub const LIST_DEADLINE: Duration = Duration::from_secs(15);

/// A failure, naming the engine and the reason.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn failure(reason: String) -> VoiceMeError {
    VoiceMeError::SpeechEngine(format!("{ENGINE_LABEL} {reason}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_names_windows_speech() {
        assert_eq!(
            failure("lists no installed voices".to_string()).to_string(),
            VoiceMeError::SpeechEngine("Windows speech lists no installed voices".to_string())
                .to_string()
        );
    }
}
