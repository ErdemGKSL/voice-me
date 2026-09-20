use crate::error::VoiceMeError;
use crate::state::AppState;

/// Driving adapter port: captures the configured global hotkey per OS.
pub trait HotkeyPort {
    /// Start listening for the configured hotkey, signaling presses via `AppEvent`.
    fn start_listening(&self, hotkey: &str) -> Result<(), VoiceMeError>;
}

/// Driven adapter port: plays generated audio through the OS virtual microphone.
pub trait VirtualMicPort {
    /// Play a buffer of generated speech through the virtual microphone device.
    fn play(&self, audio: &[u8]) -> Result<(), VoiceMeError>;
}

/// Driven adapter port: manages the OS system tray presence.
pub trait TrayPort {
    /// Show the application's tray icon and menu.
    fn show(&self) -> Result<(), VoiceMeError>;
}

/// Driven adapter port: generates speech via the Chatterbox sidecar.
pub trait TtsPort {
    /// Generate speech audio for the given text.
    fn generate(&self, text: &str) -> Result<Vec<u8>, VoiceMeError>;
}

/// Driven adapter port: detects and provisions runtime dependencies.
pub trait DependencyProvisioningPort {
    /// Run the Dependency Check, reporting results via `AppEvent`.
    fn check(&self) -> Result<(), VoiceMeError>;
}

/// Port for reading/writing persisted settings (implemented inside
/// `voice-me-core` itself, per AD-6 — not a separate adapter crate).
pub trait SettingsStore {
    /// Load settings into an `AppState`.
    fn load(&self) -> Result<AppState, VoiceMeError>;

    /// Persist `wav_bytes` as the active Reference Voice Sample (AD-6):
    /// writes the audio file to the OS data directory and updates the
    /// settings, replacing whichever clip was previously active. Returns the
    /// resulting `AppState`. Adapters (e.g. `voice-me-ui`'s recorder) call
    /// this instead of writing to the data directory themselves.
    fn save_reference_voice_sample(&self, wav_bytes: &[u8]) -> Result<AppState, VoiceMeError>;
}
