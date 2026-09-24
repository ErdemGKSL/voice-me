//! `voice-me-ui` — gpui-kit views (Prompt Overlay, settings, voice setup,
//! tray menu).

mod backend;
mod dependencies;
mod hotkey;
mod piper_voices;
mod prompt_overlay;
mod settings;
mod voice_setup;

pub use backend::{
    API_KEY_STORAGE_NOTICE, AZURE_PICK_A_VOICE, AZURE_STOCK_VOICE_NOTE, BackendAction,
    BackendActions, BackendArea, BackendPanel, BackendView, OpenPiperVoicesTab,
    PIPER_NO_VOICE_NOTE,
};
pub use dependencies::{DependenciesView, OpenBackendTab, RowProvisioning, blocker_notice};
pub use hotkey::{CaptureOutcome, HotkeyView, capture_keystroke, display_hotkey};
pub use piper_voices::{
    PiperCatalogState, PiperVoicesAction, PiperVoicesActions, PiperVoicesPanel, PiperVoicesView,
    VoiceDownload, VoiceRow, filter_rows, voice_rows,
};
pub use prompt_overlay::{
    ConfirmDisclosure, DISCLOSURE_ITEMS, DisclosureText, PROMPT_BAR_HEIGHT, PromptOverlayView,
};
pub use settings::{DependenciesTab, PiperVoicesTab, SettingsView};
pub use voice_setup::{
    MAX_RECORDING_SECS, MIN_RECORDING_SECS, VoiceSetupView, recording_meets_minimum_duration,
    should_auto_stop,
};
