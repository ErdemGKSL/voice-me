//! `voice-me-ui` — gpui-kit views (Prompt Overlay, settings, voice setup,
//! tray menu).

mod dependencies;
mod hotkey;
mod prompt_overlay;
mod settings;
mod voice_setup;

pub use dependencies::{
    API_KEY_STORAGE_NOTICE, BackendAction, BackendActions, BackendArea, BackendPanel,
    DependenciesView, RowProvisioning, blocker_notice,
};
pub use hotkey::{CaptureOutcome, HotkeyView, capture_keystroke, display_hotkey};
pub use prompt_overlay::{ConfirmDisclosure, DISCLOSURE_ITEMS, PromptOverlayView};
pub use settings::{DependenciesTab, SettingsView};
pub use voice_setup::{
    MAX_RECORDING_SECS, MIN_RECORDING_SECS, VoiceSetupView, recording_meets_minimum_duration,
    should_auto_stop,
};
