use crate::state::DependencyReport;

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
    ///
    /// The report travels *on the event*, and nowhere else: `voice-me-deps`
    /// holds nothing after sending it, and the composition root is the only
    /// thing that keeps it (AD-3). That is what stops a second source of
    /// truth from appearing beside `AppState`.
    DependencyCheckCompleted {
        /// What the check found, for the backend it was run against.
        report: DependencyReport,
    },
    /// The tray's "Settings…" menu item was clicked.
    SettingsRequested,
    /// The Speak Action: the Prompt Overlay was confirmed with a non-empty
    /// line of text.
    ///
    /// This is the *only* route the Speak Action takes out of the UI
    /// (AD-3/AD-10): the overlay view sends this and closes, without waiting
    /// on generation or playback. Whoever handles it downstream (Story 2.6)
    /// owns everything that happens next.
    SpeakRequested {
        /// The text the user typed, trimmed of surrounding whitespace.
        text: String,
    },
}
