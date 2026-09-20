//! `voice-me-core` — the hexagon: platform-agnostic domain types, `AppState`,
//! `AppEvent`, and the port traits every adapter implements (AD-1).

mod error;
mod event;
mod ports;
mod state;

pub use error::VoiceMeError;
pub use event::AppEvent;
pub use ports::{
    DependencyProvisioningPort, HotkeyPort, SettingsStore, TrayPort, TtsPort, VirtualMicPort,
};
pub use state::AppState;
