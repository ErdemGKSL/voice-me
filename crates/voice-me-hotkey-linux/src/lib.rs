//! `voice-me-hotkey-linux` — the Linux `HotkeyPort` adapter (spec-2-3, and
//! spec-native-gnome-kde-hotkey).
//!
//! Five backends, selected at **runtime** (never at compile time), tried in
//! this order until one binds (that spec's decision 3):
//!
//! 1. **GNOME** ([`gnome`]) / **KDE** ([`kde`]), chosen from
//!    `XDG_CURRENT_DESKTOP` in any session type — the desktop's own
//!    shortcut system: an exclusive grab the user sees and edits in System
//!    Settings. GNOME runs the `gdbus call` [`SUMMON_COMMAND`], which
//!    reaches this process's D-Bus summon service ([`summon`]); KDE signals
//!    the registered `kglobalaccel` component directly.
//! 2. **Portal** ([`portal`]) on Wayland — xdg-desktop-portal
//!    `GlobalShortcuts`, for compositors that implement it (Hyprland…).
//! 3. **X11** ([`x11`]) on X11, then
//! 4. **evdev** ([`evdev`]) — the last resort everywhere.
//!
//! A backend that is unavailable or fails the first bind is skipped, with
//! the reason on stderr — except a conflict (`HotkeyAlreadyInUse`), which
//! ends the attempt so a fallback never binds the same combination a second
//! time. Once one is running, later rebinds stay on it and
//! report its errors (a conflict included) rather than hopping backends.
//!
//! The spec-2-3 descriptions of the last two:
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

// This crate *is* the Linux capability (AD-2): its `evdev` backend does not
// build on Windows, so the whole body is gated rather than the workspace
// being made unbuildable there. Windows is `voice-me-hotkey-windows`.
#![cfg(target_os = "linux")]

mod desktop;
mod evdev;
mod gnome;
mod kde;
mod portal;
mod summon;
mod x11;

use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};

use voice_me_core::{AppEventSender, HotkeyPort, VoiceMeError};

pub use desktop::{Desktop, desktop, to_gnome, to_qt_key};
pub use summon::{SUMMON_BUS_NAME, SUMMON_COMMAND, SUMMON_INTERFACE, SUMMON_OBJECT_PATH};

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
    Gnome(gnome::GnomeBackend),
    Kde(kde::KdeBackend),
    Portal(portal::PortalBackend),
    X11(x11::X11Backend),
    Evdev(evdev::EvdevBackend),
}

impl Backend {
    fn kind(&self) -> ActiveBackend {
        match self {
            Backend::Gnome(_) => ActiveBackend::Gnome,
            Backend::Kde(_) => ActiveBackend::Kde,
            Backend::Portal(_) => ActiveBackend::Portal,
            Backend::X11(_) => ActiveBackend::X11,
            Backend::Evdev(_) => ActiveBackend::Evdev,
        }
    }

    fn rebind(&mut self, hotkey: &str) -> Result<(), VoiceMeError> {
        match self {
            Backend::Gnome(backend) => backend.rebind(hotkey),
            Backend::Kde(backend) => backend.rebind(hotkey),
            Backend::Portal(backend) => backend.rebind(hotkey),
            Backend::X11(backend) => backend.rebind(hotkey),
            Backend::Evdev(backend) => backend.rebind(hotkey),
        }
    }
}

/// A hotkey backend: one step of decision 3's order, and what
/// [`LinuxHotkeyAdapter::active_backend`] reports once one has bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveBackend {
    Gnome,
    Kde,
    Portal,
    X11,
    Evdev,
}

impl ActiveBackend {
    fn label(self) -> &'static str {
        match self {
            ActiveBackend::Gnome => "GNOME custom keybinding",
            ActiveBackend::Kde => "KDE kglobalaccel",
            ActiveBackend::Portal => "GlobalShortcuts portal",
            ActiveBackend::X11 => "X11 grab",
            ActiveBackend::Evdev => "evdev",
        }
    }
}

/// The backends to try, in order, for a desktop and session type:
/// GNOME/KDE native → portal (Wayland) → X11 grab (X11) → evdev. Pure, so
/// the order itself is tested.
fn backend_order(desktop: Desktop, session: SessionKind) -> Vec<ActiveBackend> {
    let mut order = Vec::with_capacity(3);
    match desktop {
        Desktop::Gnome => order.push(ActiveBackend::Gnome),
        Desktop::Kde => order.push(ActiveBackend::Kde),
        Desktop::Other => {}
    }
    match session {
        SessionKind::Wayland => order.push(ActiveBackend::Portal),
        SessionKind::X11 => order.push(ActiveBackend::X11),
    }
    order.push(ActiveBackend::Evdev);
    order
}

/// The D-Bus summon service, started once when the adapter is created.
enum SummonState {
    Running(#[allow(dead_code)] summon::SummonService),
    Failed(String),
}

/// Try `order` until one backend starts, per decision 3. Pure over its
/// inputs so the policy is unit-tested:
/// - `precheck` (GNOME/KDE can name the key) failing refuses the hotkey
///   before anything is started;
/// - GNOME is skipped when the summon service is not running
///   (`summon_error`), since its shortcut's command would reach nothing;
/// - a `HotkeyAlreadyInUse` ends the attempt at once — a fallback must
///   never cover a conflict, or the combination would fire twice;
/// - otherwise each failure moves on, and the last one is returned.
fn start_first<B>(
    hotkey: &str,
    order: &[ActiveBackend],
    precheck: Result<(), VoiceMeError>,
    summon_error: Option<&str>,
    mut start: impl FnMut(ActiveBackend) -> Result<B, VoiceMeError>,
) -> Result<B, VoiceMeError> {
    precheck?;
    let mut last_error = None;
    for &kind in order {
        let started = match (kind, summon_error) {
            (ActiveBackend::Gnome, Some(reason)) => Err(VoiceMeError::Other(reason.to_string())),
            _ => start(kind),
        };
        match started {
            Ok(backend) => return Ok(backend),
            Err(VoiceMeError::HotkeyAlreadyInUse) => return Err(VoiceMeError::HotkeyAlreadyInUse),
            Err(error) => {
                if kind != ActiveBackend::Evdev {
                    eprintln!(
                        "voice-me: the {} could not bind {hotkey} ({error}); trying the next hotkey backend",
                        kind.label()
                    );
                }
                last_error = Some(error);
            }
        }
    }
    Err(last_error
        .unwrap_or_else(|| VoiceMeError::Other("no hotkey backend is available".to_string())))
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
    summon: Mutex<SummonState>,
}

impl LinuxHotkeyAdapter {
    /// Build the adapter with the sender half of the shared `AppEvent`
    /// channel (AD-3), and start the D-Bus summon service on it — at once,
    /// so a compositor binding of the `gdbus call` works before any hotkey
    /// has been saved. No hotkey is registered or read until a combination
    /// is actually bound.
    pub fn new(events: AppEventSender) -> Self {
        let summon = match summon::SummonService::start(events.clone()) {
            Ok(service) => SummonState::Running(service),
            Err(error) => {
                eprintln!("voice-me: {error}");
                SummonState::Failed(error.to_string())
            }
        };
        Self {
            events: Mutex::new(Some(events)),
            backend: Mutex::new(None),
            summon: Mutex::new(summon),
        }
    }

    /// Which backend is bound, or `None` before the first successful bind.
    pub fn active_backend(&self) -> Option<ActiveBackend> {
        lock(&self.backend).as_ref().map(Backend::kind)
    }

    fn bind(&self, hotkey: &str) -> Result<(), VoiceMeError> {
        let mut backend = lock(&self.backend);
        if let Some(backend) = backend.as_mut() {
            return backend.rebind(hotkey);
        }

        let events = lock(&self.events).clone().ok_or_else(|| {
            VoiceMeError::Other("hotkey adapter has no AppEvent channel".to_string())
        })?;

        let desktop = desktop();
        // An unmappable key is refused outright on GNOME/KDE rather than
        // quietly handed to a fallback backend: Save must say so.
        let precheck = match desktop {
            Desktop::Gnome => to_gnome(hotkey).map(|_| ()),
            Desktop::Kde => to_qt_key(hotkey).map(|_| ()),
            Desktop::Other => Ok(()),
        };
        let summon_error = match &*lock(&self.summon) {
            SummonState::Failed(reason) => Some(reason.clone()),
            SummonState::Running(_) => None,
        };

        let started = start_first(
            hotkey,
            &backend_order(desktop, session_kind()),
            precheck,
            summon_error.as_deref(),
            |kind| match kind {
                ActiveBackend::Gnome => gnome::GnomeBackend::start(hotkey).map(Backend::Gnome),
                ActiveBackend::Kde => {
                    kde::KdeBackend::start(hotkey, events.clone()).map(Backend::Kde)
                }
                ActiveBackend::Portal => {
                    portal::PortalBackend::start(hotkey, events.clone()).map(Backend::Portal)
                }
                ActiveBackend::X11 => {
                    x11::X11Backend::start(hotkey, events.clone()).map(Backend::X11)
                }
                ActiveBackend::Evdev => {
                    evdev::EvdevBackend::start(hotkey, events.clone()).map(Backend::Evdev)
                }
            },
        )?;
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

/// Open the desktop's own keyboard-shortcut settings:
/// `gnome-control-center keyboard` on GNOME, `systemsettings kcm_keys` on
/// KDE (`systemsettings5` on Plasma 5). Other desktops have no such page.
pub fn open_shortcut_settings() -> Result<(), VoiceMeError> {
    let candidates: &[(&str, &[&str])] = match desktop() {
        Desktop::Gnome => &[("gnome-control-center", &["keyboard"])],
        Desktop::Kde => &[
            ("systemsettings", &["kcm_keys"]),
            ("systemsettings5", &["kcm_keys"]),
        ],
        Desktop::Other => {
            return Err(VoiceMeError::Other(
                "this desktop has no keyboard-shortcut settings voice-me knows how to open"
                    .to_string(),
            ));
        }
    };

    let mut last_error = None;
    for (program, args) in candidates {
        match Command::new(program)
            .args(*args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                // Reap it whenever the user closes it, so no zombie is
                // left behind for the life of the app.
                let _ = std::thread::Builder::new()
                    .name("voice-me-shortcut-settings".to_string())
                    .spawn(move || {
                        let _ = child.wait();
                    });
                return Ok(());
            }
            Err(error) => last_error = Some(format!("{program}: {error}")),
        }
    }
    Err(VoiceMeError::Other(format!(
        "couldn't open the keyboard-shortcut settings ({})",
        last_error.unwrap_or_default()
    )))
}

/// Lock helper that treats a poisoned mutex as usable: a panicking backend
/// thread must not permanently disable hotkey configuration for the rest of
/// the run, and every value behind these mutexes is replaced wholesale
/// rather than mutated in place, so there is no half-updated state to
/// protect against.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

/// Every accelerator key token `voice-me-ui`'s `accelerator_code` can
/// emit. The UI and this crate are connected only by convention, so every
/// backend's converter is tested against this list.
#[cfg(test)]
pub(crate) const UI_KEY_TOKENS: &[&str] = &[
    "KeyA",
    "KeyB",
    "KeyC",
    "KeyD",
    "KeyE",
    "KeyF",
    "KeyG",
    "KeyH",
    "KeyI",
    "KeyJ",
    "KeyK",
    "KeyL",
    "KeyM",
    "KeyN",
    "KeyO",
    "KeyP",
    "KeyQ",
    "KeyR",
    "KeyS",
    "KeyT",
    "KeyU",
    "KeyV",
    "KeyW",
    "KeyX",
    "KeyY",
    "KeyZ",
    "Digit0",
    "Digit1",
    "Digit2",
    "Digit3",
    "Digit4",
    "Digit5",
    "Digit6",
    "Digit7",
    "Digit8",
    "Digit9",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
    "Space",
    "Enter",
    "Tab",
    "Backspace",
    "Delete",
    "Insert",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "Minus",
    "Equal",
    "BracketLeft",
    "BracketRight",
    "Semicolon",
    "Quote",
    "Backquote",
    "Backslash",
    "Comma",
    "Period",
    "Slash",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnome_and_kde_try_their_native_backend_first() {
        assert_eq!(
            backend_order(Desktop::Gnome, SessionKind::Wayland),
            vec![
                ActiveBackend::Gnome,
                ActiveBackend::Portal,
                ActiveBackend::Evdev
            ]
        );
        assert_eq!(
            backend_order(Desktop::Kde, SessionKind::X11),
            vec![ActiveBackend::Kde, ActiveBackend::X11, ActiveBackend::Evdev]
        );
    }

    #[test]
    fn other_desktops_try_the_portal_on_wayland_and_the_grab_on_x11() {
        assert_eq!(
            backend_order(Desktop::Other, SessionKind::Wayland),
            vec![ActiveBackend::Portal, ActiveBackend::Evdev]
        );
        assert_eq!(
            backend_order(Desktop::Other, SessionKind::X11),
            vec![ActiveBackend::X11, ActiveBackend::Evdev]
        );
    }

    use std::cell::RefCell;

    const ALL: &[ActiveBackend] = &[
        ActiveBackend::Gnome,
        ActiveBackend::Portal,
        ActiveBackend::Evdev,
    ];

    fn other(message: &str) -> VoiceMeError {
        VoiceMeError::Other(message.to_string())
    }

    #[test]
    fn the_first_backend_that_starts_ends_the_loop() {
        let tried = RefCell::new(Vec::new());
        let started = start_first("Ctrl+Alt+KeyV", ALL, Ok(()), None, |kind| {
            tried.borrow_mut().push(kind);
            match kind {
                ActiveBackend::Gnome => Err(other("no gsettings")),
                _ => Ok(kind),
            }
        });
        assert_eq!(started.unwrap(), ActiveBackend::Portal);
        assert_eq!(
            *tried.borrow(),
            vec![ActiveBackend::Gnome, ActiveBackend::Portal],
            "evdev is never started once the portal bound"
        );
    }

    #[test]
    fn a_conflict_ends_the_loop_at_once() {
        let tried = RefCell::new(Vec::new());
        let started: Result<ActiveBackend, _> = start_first(
            "Ctrl+Alt+KeyV",
            &[ActiveBackend::Kde, ActiveBackend::X11, ActiveBackend::Evdev],
            Ok(()),
            None,
            |kind| {
                tried.borrow_mut().push(kind);
                Err(VoiceMeError::HotkeyAlreadyInUse)
            },
        );
        assert!(matches!(started, Err(VoiceMeError::HotkeyAlreadyInUse)));
        assert_eq!(*tried.borrow(), vec![ActiveBackend::Kde]);
    }

    #[test]
    fn gnome_is_skipped_when_the_summon_service_failed() {
        let tried = RefCell::new(Vec::new());
        let started = start_first("Ctrl+Alt+KeyV", ALL, Ok(()), Some("name taken"), |kind| {
            tried.borrow_mut().push(kind);
            Ok(kind)
        });
        assert_eq!(started.unwrap(), ActiveBackend::Portal);
        assert_eq!(*tried.borrow(), vec![ActiveBackend::Portal]);
    }

    #[test]
    fn an_unmappable_key_is_refused_before_any_backend_starts() {
        let tried = RefCell::new(Vec::new());
        let started = start_first(
            "Ctrl+AudioVolumeUp",
            ALL,
            to_gnome("Ctrl+AudioVolumeUp").map(|_| ()),
            None,
            |kind| {
                tried.borrow_mut().push(kind);
                Ok(kind)
            },
        );
        assert!(matches!(started, Err(VoiceMeError::Other(m)) if m.contains("AudioVolumeUp")));
        assert!(tried.borrow().is_empty());
    }

    #[test]
    fn when_every_backend_fails_the_last_error_is_returned() {
        let started: Result<ActiveBackend, _> =
            start_first("Ctrl+Alt+KeyV", ALL, Ok(()), None, |kind| {
                Err(match kind {
                    ActiveBackend::Evdev => VoiceMeError::InputDevicePermissionDenied,
                    _ => other("unavailable"),
                })
            });
        assert!(matches!(
            started,
            Err(VoiceMeError::InputDevicePermissionDenied)
        ));
    }

    #[test]
    fn evdev_is_the_last_resort_everywhere() {
        for desktop in [Desktop::Gnome, Desktop::Kde, Desktop::Other] {
            for session in [SessionKind::X11, SessionKind::Wayland] {
                let order = backend_order(desktop, session);
                assert_eq!(order.last(), Some(&ActiveBackend::Evdev));
                assert_eq!(
                    order.iter().filter(|k| **k == ActiveBackend::Evdev).count(),
                    1
                );
            }
        }
    }

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
