use crate::state::{DependencyKind, DependencyReport};

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
    /// Provisioning one dependency row moved forward (Story 3.2).
    ///
    /// Sent by `voice-me-deps` while it downloads, at most about ten times a
    /// second per row, so the Dependencies tab can say "installing" with a
    /// figure that grows. `total_bytes` is what *this* run has to fetch,
    /// counting only what was still missing — and bytes a resumed `.part`
    /// already held count as done from the first event. `0` means the step
    /// has no byte count at all (the Virtual Microphone), which the UI
    /// shows as "installing" with no figure.
    ProvisioningProgress {
        kind: DependencyKind,
        done_bytes: u64,
        total_bytes: u64,
    },
    /// Provisioning one dependency row ended, one way or the other.
    ///
    /// Exactly one per `provision` call that started, success *and*
    /// failure alike: failures travel on the channel rather than only as a
    /// return value, so there is one path from the adapter to the screen.
    /// The error is already a sentence naming the file and the reason; the
    /// composition root re-runs the Dependency Check either way, because a
    /// failed run can still have completed some files.
    ProvisioningFinished {
        kind: DependencyKind,
        result: Result<(), String>,
    },
}
