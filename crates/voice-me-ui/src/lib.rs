//! `voice-me-ui` — gpui-kit views (Prompt Overlay, settings, voice setup,
//! tray menu).

mod hotkey;
mod settings;
mod voice_setup;

pub use hotkey::{CaptureOutcome, HotkeyView, capture_keystroke, display_hotkey};
pub use settings::SettingsView;
pub use voice_setup::{
    MAX_RECORDING_SECS, MIN_RECORDING_SECS, VoiceSetupView, recording_meets_minimum_duration,
    should_auto_stop,
};
