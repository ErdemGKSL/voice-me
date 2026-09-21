//! X11 backend: `global-hotkey`, whose Linux implementation is `XGrabKey`.
//!
//! A real exclusive grab — the combination does not reach the focused
//! application, and grabbing a combination another client already holds
//! fails with `BadAccess`, which `global-hotkey` reports as
//! `Error::AlreadyRegistered`. That is the only conflict signal available on
//! any platform here, and it is what [`X11Backend::rebind`] turns into
//! [`VoiceMeError::HotkeyAlreadyInUse`].

use std::str::FromStr as _;

use global_hotkey::hotkey::HotKey;
use global_hotkey::{Error as HotKeyError, GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use voice_me_core::{AppEvent, AppEventSender, VoiceMeError};

pub(crate) struct X11Backend {
    /// Held for the lifetime of the adapter: dropping the manager tells its
    /// worker thread to exit and releases every grab, exactly like
    /// `LinuxTrayAdapter`'s `TrayHandle`.
    manager: GlobalHotKeyManager,
    current: Option<HotKey>,
}

impl X11Backend {
    pub(crate) fn start(hotkey: &str, events: AppEventSender) -> Result<Self, VoiceMeError> {
        let manager = GlobalHotKeyManager::new().map_err(|err| {
            VoiceMeError::Other(format!("failed to start the X11 hotkey manager: {err}"))
        })?;
        let mut backend = Self {
            manager,
            current: None,
        };
        // Register before the pump starts, so a conflict on the saved
        // combination surfaces as this call's `Err` rather than as a thread
        // that quietly never fires.
        backend.rebind(hotkey)?;
        spawn_event_pump(events);
        Ok(backend)
    }

    /// Register `hotkey`, then release the previous combination — in that
    /// order, so a rejected new combination leaves the old one still live
    /// (spec-2-3: "the previously saved hotkey stays active until a new one
    /// is confirmed working").
    pub(crate) fn rebind(&mut self, hotkey: &str) -> Result<(), VoiceMeError> {
        let next = parse_hotkey(hotkey)?;
        if self.current == Some(next) {
            // Already the live binding. Re-registering it would fail as a
            // self-conflict, which would be a nonsense error to show.
            return Ok(());
        }

        self.manager.register(next).map_err(map_register_error)?;
        if let Some(previous) = self.current.replace(next) {
            // Best-effort: the new grab is already in place, so a failure to
            // release the old one must not fail the rebind. The worst case
            // is a stale grab that no longer maps to anything the app acts
            // on, released when the manager drops at exit.
            let _ = self.manager.unregister(previous);
        }
        Ok(())
    }
}

fn parse_hotkey(hotkey: &str) -> Result<HotKey, VoiceMeError> {
    HotKey::from_str(hotkey)
        .map_err(|err| VoiceMeError::Other(format!("unsupported hotkey \"{hotkey}\": {err}")))
}

fn map_register_error(error: HotKeyError) -> VoiceMeError {
    match error {
        HotKeyError::AlreadyRegistered(_) => VoiceMeError::HotkeyAlreadyInUse,
        other => VoiceMeError::Other(format!("failed to register the hotkey: {other}")),
    }
}

/// `global-hotkey` delivers events on a process-wide crossbeam channel;
/// forward them onto the shared `AppEvent` channel (AD-3) from a thread of
/// our own. Only `Pressed` is forwarded, so one physical press produces
/// exactly one `AppEvent::HotkeyPressed` rather than a press/release pair.
fn spawn_event_pump(events: AppEventSender) {
    let spawned = std::thread::Builder::new()
        .name("voice-me-hotkey-x11".to_string())
        .spawn(move || {
            let receiver = GlobalHotKeyEvent::receiver();
            while let Ok(event) = receiver.recv() {
                if event.state() != HotKeyState::Pressed {
                    continue;
                }
                // Send failure means the receiver (the composition root) is
                // gone — the app is shutting down; stop pumping.
                if events.unbounded_send(AppEvent::HotkeyPressed).is_err() {
                    break;
                }
            }
        });

    // A hotkey failure must never cost the app its tray presence, so this
    // is reported rather than panicked — matching `evdev.rs`'s
    // `spawn_reader`.
    if let Err(err) = spawned {
        eprintln!("voice-me: failed to start the X11 hotkey event pump: {err}");
    }
}

#[cfg(test)]
mod tests {
    //! `map_register_error` is the product's only conflict signal — the UI's
    //! own conflict test injects an already-mapped `VoiceMeError` through a
    //! fake port, so this is the one place the mapping itself is checked.
    //! Pure function: no X server involved.

    use super::*;

    #[test]
    fn an_already_registered_combination_maps_to_the_conflict_variant() {
        let hotkey = parse_hotkey("Ctrl+Alt+KeyV").unwrap();

        assert!(matches!(
            map_register_error(HotKeyError::AlreadyRegistered(hotkey)),
            VoiceMeError::HotkeyAlreadyInUse
        ));
    }

    #[test]
    fn any_other_registration_failure_stays_generic() {
        // A conflict is the only failure the UI turns into "try another
        // combination"; everything else must not masquerade as one.
        assert!(matches!(
            map_register_error(HotKeyError::FailedToRegister("no display".to_string())),
            VoiceMeError::Other(_)
        ));
    }
}
