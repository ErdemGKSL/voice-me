/// Events emitted by adapters onto the single shared `AppEvent` channel (AD-3).
///
/// Only `voice-me-app` (the composition root) holds the receiver; every
/// adapter holds only the sender half.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    /// The configured global hotkey was pressed.
    HotkeyPressed,
    /// `voice-me-deps`'s Dependency Check finished running.
    DependencyCheckCompleted,
    /// The tray's "Settings…" menu item was clicked.
    SettingsRequested,
}
