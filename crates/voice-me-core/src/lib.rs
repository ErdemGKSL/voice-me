//! `voice-me-core` — the hexagon: platform-agnostic domain types, `AppState`,
//! `AppEvent`, and the port traits every adapter implements (AD-1).

mod error;
mod event;
mod ports;
mod settings_store;
mod state;

pub use error::VoiceMeError;
pub use event::AppEvent;
pub use ports::{
    DependencyProvisioningPort, HotkeyPort, SettingsStore, TrayPort, TtsPort, VirtualMicPort,
};
pub use settings_store::FileSettingsStore;
pub use state::AppState;

/// Sender half of the shared `AppEvent` channel (AD-3). Adapters hold only
/// this sender half; only `voice-me-app` (the composition root) holds the
/// receiver half.
pub type AppEventSender = futures::channel::mpsc::UnboundedSender<AppEvent>;
