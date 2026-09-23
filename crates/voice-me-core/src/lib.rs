//! `voice-me-core` — the hexagon: platform-agnostic domain types, `AppState`,
//! `AppEvent`, and the port traits every adapter implements (AD-1).

pub mod assets;
mod audio;
mod error;
mod event;
mod ports;
mod settings_store;
mod speak;
mod state;
pub mod tokio_bridge;

pub use audio::{AudioBuffer, SAMPLE_RATE};
pub use error::VoiceMeError;
pub use event::AppEvent;
pub use ports::{
    CheckRequest, DependencyProvisioningPort, HotkeyPort, NotificationPort, SettingsStore,
    TrayPort, TtsPort, VirtualMicPort,
};
pub use settings_store::FileSettingsStore;
pub use speak::{
    GENERATION_FAILED_SUMMARY, PLAYBACK_FAILED_SUMMARY, STILL_WORKING_BODY, STILL_WORKING_SUMMARY,
    speak,
};
pub use state::{
    ActiveBackend, ApiKeys, AppState, BackendSelection, DEFAULT_SPEECH_LANGUAGE,
    DEFAULT_UI_LANGUAGE, Dependency, DependencyKind, DependencyOutcome, DependencyReport,
    DependencyStatus, LanguageBackend, LanguageOption, LocalRuntime, RemoteProvider, RemoteSample,
    SpeechBackend, SpeechExecutionTarget, SpeechLanguage, SpeechLanguages, SpeechVoices,
    SpeechWeights, SystemVoice, SystemVoiceRefusal, backend_choices, format_bytes,
    resolve_system_voice, system_voice_languages, system_voices_of,
};

/// Sender half of the shared `AppEvent` channel (AD-3). Adapters hold only
/// this sender half; only `voice-me-app` (the composition root) holds the
/// receiver half.
pub type AppEventSender = futures::channel::mpsc::UnboundedSender<AppEvent>;
