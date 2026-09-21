//! `voice-me-hotkey-linux` — the Linux `HotkeyPort` adapter (spec-2-3).
//!
//! Two backends, selected at **runtime** (never at compile time) from the
//! session environment:
//!
//! - **X11** ([`x11`]) — `global-hotkey`'s `XGrabKey` registration. A true
//!   exclusive grab: the combination does not reach other applications, and a
//!   second registration of the same combination fails, which is what makes
//!   inline conflict detection possible on this path.
//! - **Wayland** ([`evdev`]) — a passive read of `/dev/input/event*`.
//!   `global-hotkey` is X11-only and GNOME does not implement the
//!   GlobalShortcuts portal, so this is the only path that works in a
//!   GNOME/Wayland session. Three accepted consequences (spec-2-3 decision
//!   1): the combination *also* reaches the focused application; nothing is
//!   registered, so conflicts cannot be detected at all; and it needs read
//!   access to the input devices, i.e. `input`-group membership.
//!
//! Presses are signalled only through the shared `AppEvent` channel (AD-3):
//! this crate never depends on `voice-me-ui` and never touches a window.

mod evdev;
mod x11;

use std::sync::{Mutex, MutexGuard};

use voice_me_core::{AppEventSender, HotkeyPort, VoiceMeError};

/// Which backend a session calls for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    /// `global-hotkey` / `XGrabKey`.
    X11,
    /// Raw `evdev` read of `/dev/input/event*`.
    Wayland,
}

/// Decide the backend from the environment. Runtime, not compile-time: the
/// same binary runs under either session type.
pub fn session_kind() -> SessionKind {
    session_kind_for(
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
        std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
    )
}

/// The decision itself, separated from reading the environment so it can be
/// tested without mutating process env (`unsafe` under edition 2024, and
/// racy across parallel tests). `XDG_SESSION_TYPE` alone is not always set
/// (some login managers omit it), so `WAYLAND_DISPLAY` is checked first and
/// X11 is the fallback — picking X11 for a Wayland session would bind a
/// hotkey that "succeeds" and then never fires.
fn session_kind_for(wayland_display_set: bool, xdg_session_type: Option<&str>) -> SessionKind {
    if wayland_display_set {
        return SessionKind::Wayland;
    }
    match xdg_session_type {
        Some(kind) if kind.eq_ignore_ascii_case("wayland") => SessionKind::Wayland,
        _ => SessionKind::X11,
    }
}

enum Backend {
    X11(x11::X11Backend),
    Evdev(evdev::EvdevBackend),
}

impl Backend {
    fn rebind(&mut self, hotkey: &str) -> Result<(), VoiceMeError> {
        match self {
            Backend::X11(backend) => backend.rebind(hotkey),
            Backend::Evdev(backend) => backend.rebind(hotkey),
        }
    }
}

/// Linux `HotkeyPort` adapter.
///
/// The backend is created lazily, on the first `start_listening`/`rebind`
/// call that actually has a combination to bind. That laziness is the reason
/// an app which has never had a hotkey configured starts with no hotkey
/// active *and no error* — in particular, a Wayland session with no
/// `input`-group membership only reports the permission problem once the
/// user actually asks for a hotkey.
pub struct LinuxHotkeyAdapter {
    /// Where presses are sent. Kept here rather than only on
    /// `start_listening`'s frame because the very first hotkey a user ever
    /// configures is bound through `rebind` (from the Settings UI), with no
    /// preceding `start_listening` call to carry the sender in.
    events: Mutex<Option<AppEventSender>>,
    backend: Mutex<Option<Backend>>,
}

impl LinuxHotkeyAdapter {
    /// Build the adapter with the sender half of the shared `AppEvent`
    /// channel (AD-3). Nothing is opened, registered or read until a
    /// combination is actually bound.
    pub fn new(events: AppEventSender) -> Self {
        Self {
            events: Mutex::new(Some(events)),
            backend: Mutex::new(None),
        }
    }

    fn bind(&self, hotkey: &str) -> Result<(), VoiceMeError> {
        let mut backend = lock(&self.backend);
        if let Some(backend) = backend.as_mut() {
            return backend.rebind(hotkey);
        }

        let events = lock(&self.events).clone().ok_or_else(|| {
            VoiceMeError::Other("hotkey adapter has no AppEvent channel".to_string())
        })?;

        let started = match session_kind() {
            SessionKind::X11 => Backend::X11(x11::X11Backend::start(hotkey, events)?),
            SessionKind::Wayland => Backend::Evdev(evdev::EvdevBackend::start(hotkey, events)?),
        };
        *backend = Some(started);
        Ok(())
    }
}

impl HotkeyPort for LinuxHotkeyAdapter {
    fn start_listening(&self, hotkey: &str, events: AppEventSender) -> Result<(), VoiceMeError> {
        *lock(&self.events) = Some(events);
        self.bind(hotkey)
    }

    fn rebind(&self, hotkey: &str) -> Result<(), VoiceMeError> {
        self.bind(hotkey)
    }
}

/// Lock helper that treats a poisoned mutex as usable: a panicking backend
/// thread must not permanently disable hotkey configuration for the rest of
/// the run, and every value behind these mutexes is replaced wholesale
/// rather than mutated in place, so there is no half-updated state to
/// protect against.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wayland_display_selects_the_evdev_backend() {
        assert_eq!(
            session_kind_for(true, Some("x11")),
            SessionKind::Wayland,
            "a live WAYLAND_DISPLAY outranks whatever XDG_SESSION_TYPE claims"
        );
    }

    #[test]
    fn xdg_session_type_decides_when_wayland_display_is_unset() {
        assert_eq!(
            session_kind_for(false, Some("wayland")),
            SessionKind::Wayland
        );
        assert_eq!(session_kind_for(false, Some("x11")), SessionKind::X11);
    }

    #[test]
    fn an_unknown_session_falls_back_to_x11() {
        assert_eq!(session_kind_for(false, None), SessionKind::X11);
    }
}
