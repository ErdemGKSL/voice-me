//! `voice-me-notify-linux` — the Linux `NotificationPort` adapter.
//!
//! One freedesktop `org.freedesktop.Notifications` method call per
//! notification, through `notify-rust`'s pure-Rust `zbus` backend. There is
//! no daemon of our own, no tray balloon, and no state: the desktop's own
//! notification service owns presentation, stacking and dismissal, which is
//! exactly what "OS-native" means in UX-DR14/15.

use voice_me_core::{NotificationPort, VoiceMeError};

/// The application name the desktop groups these notifications under, and
/// the icon it looks up. `dialog-information` is in every freedesktop icon
/// theme; voice-me has no installed `.desktop` file yet, so asking for its
/// own icon would leave a blank square on most desktops.
const APP_NAME: &str = "voice-me";
const ICON: &str = "dialog-information";

/// Linux `NotificationPort` adapter backed by the desktop's notification
/// service.
pub struct LinuxNotificationAdapter;

impl NotificationPort for LinuxNotificationAdapter {
    fn notify(&self, summary: &str, body: &str) -> Result<(), VoiceMeError> {
        notify_rust::Notification::new()
            .appname(APP_NAME)
            .summary(summary)
            .body(body)
            .icon(ICON)
            .show()
            // `Ok` carries a live handle that can close or update the
            // notification later. Nothing here wants that, and holding it
            // would keep a bus connection alive for a message already
            // delivered — so it is dropped deliberately rather than
            // returned through the port.
            .map(drop)
            .map_err(|error| {
                // A desktop with no notification daemon running is the
                // common case here (a bare window manager), not a bug —
                // hence a plain domain error the caller logs, not a panic.
                VoiceMeError::Other(format!("could not show a desktop notification: {error}"))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// There is no notification daemon in CI, and this adapter is a thin
    /// pass-through to one — so the only thing worth pinning without a bus
    /// is that a failure to deliver comes back as a domain error rather
    /// than a panic or a silent success. `speak` depends on exactly that:
    /// it logs a delivery failure and returns the *original* error.
    #[test]
    fn delivery_failure_is_a_domain_error_rather_than_a_panic() {
        // Point the client at a socket that cannot exist, so the bus
        // connection is guaranteed to fail regardless of the environment
        // the test runs in.
        // SAFETY: single-threaded test body; nothing else reads the
        // environment concurrently here.
        unsafe {
            std::env::set_var(
                "DBUS_SESSION_BUS_ADDRESS",
                "unix:path=/nonexistent/voice-me-bus",
            );
        }

        let result = LinuxNotificationAdapter.notify("summary", "body");

        assert!(
            matches!(result, Err(VoiceMeError::Other(_))),
            "an undeliverable notification must be reportable, not fatal"
        );
    }
}
