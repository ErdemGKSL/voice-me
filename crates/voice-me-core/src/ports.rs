use crate::AppEventSender;
use crate::error::VoiceMeError;
use crate::state::AppState;

/// Driving adapter port: captures the configured global hotkey per OS.
pub trait HotkeyPort {
    /// Start listening for `hotkey`, signaling each press through `events`.
    ///
    /// `events` is the sender half of the shared `AppEvent` channel (AD-3),
    /// threaded in the same way as `TrayPort::show`: the adapter signals
    /// presses only by sending `AppEvent::HotkeyPressed`, never by depending
    /// on `voice-me-ui` or touching windows itself.
    ///
    /// Called once at startup by the composition root, and only when a
    /// hotkey was actually saved in a previous run — an app that has never
    /// had one configured must start with no hotkey active and no error.
    fn start_listening(&self, hotkey: &str, events: AppEventSender) -> Result<(), VoiceMeError>;

    /// Make `hotkey` the live combination, replacing whichever one is
    /// currently bound.
    ///
    /// This is the "confirm it works before persisting it" step the Settings
    /// UI calls on Save: on a backend that can detect a conflict it returns
    /// [`VoiceMeError::HotkeyAlreadyInUse`] *without* disturbing the
    /// previously bound combination, so the caller can surface the conflict
    /// and leave both the old binding and `settings.toml` untouched. Any
    /// other `Err` leaves the old binding in place too.
    fn rebind(&self, hotkey: &str) -> Result<(), VoiceMeError>;
}

/// Driven adapter port: plays generated audio through the OS virtual microphone.
pub trait VirtualMicPort {
    /// Play a buffer of generated speech through the virtual microphone device.
    fn play(&self, audio: &[u8]) -> Result<(), VoiceMeError>;
}

/// Driven adapter port: manages the OS system tray presence.
pub trait TrayPort {
    /// Show the application's tray icon and menu.
    ///
    /// Takes a `gpui_kit::App` context because tray-backed implementations
    /// (e.g. `gpui-tray`, see spec-2-1) build their native tray item and
    /// dispatch menu-item clicks as ordinary GPUI actions through `cx` —
    /// there is no way to stand up or drive a tray without it. `voice-me-core`
    /// depends on `gpui-kit` only for this context type, never on the tray
    /// crate itself, which stays confined to the `voice-me-tray-*` adapters.
    ///
    /// `events` is the sender half of the shared `AppEvent` channel (AD-3):
    /// adapters use it to signal actions like "Settings…" clicked back to
    /// the composition root, without depending on `voice-me-ui` or calling
    /// window APIs themselves.
    fn show(&self, cx: &mut gpui_kit::App, events: AppEventSender) -> Result<(), VoiceMeError>;
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

    /// Persist the selected input (microphone) device name, or `None` to
    /// clear the selection back to the OS default. Returns the resulting
    /// `AppState`.
    fn save_selected_mic_device(&self, device: Option<&str>) -> Result<AppState, VoiceMeError>;

    /// Persist the configured global hotkey, or `None` to clear it. The
    /// string is stored in `global-hotkey`'s accelerator syntax (e.g.
    /// `Ctrl+Alt+KeyV`) so it round-trips through `HotKey::from_str` with no
    /// bespoke parser; the display form shown on a chip is derived for
    /// rendering only and never persisted. Returns the resulting `AppState`.
    fn save_hotkey(&self, hotkey: Option<&str>) -> Result<AppState, VoiceMeError>;
}
