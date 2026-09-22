use voice_me_core::{NotificationPort, VoiceMeError};

/// Windows `NotificationPort` adapter, intended to be backed by a toast
/// notification. Not yet implemented — Windows is compiled-only in this dev
/// environment, the same position `voice-me-tray-windows` and
/// `voice-me-hotkey-windows` are in; see `ARCHITECTURE-SPINE.md`'s Deferred
/// section.
///
/// A toast additionally needs a registered AppUserModelID (i.e. an installed
/// shortcut) to appear at all, which is an installer concern this story does
/// not own.
pub struct WindowsNotificationAdapter;

impl NotificationPort for WindowsNotificationAdapter {
    /// Returns an error rather than `todo!()`, unlike the other Windows
    /// stubs, because this one sits on a path that is actually taken: the
    /// Speak Action calls it on *every* failure and on every cold start.
    /// A panic there would replace a reportable failure with a dead app, and
    /// the port's contract already says an undeliverable notification is a
    /// domain error the caller logs.
    fn notify(&self, _summary: &str, _body: &str) -> Result<(), VoiceMeError> {
        Err(VoiceMeError::Other(
            "desktop notifications are not implemented on Windows yet".to_string(),
        ))
    }
}
