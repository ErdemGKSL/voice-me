//! `voice-me-hotkey-windows` — the Windows `HotkeyPort` adapter.
//!
//! The backend is `global-hotkey`, whose Windows implementation is
//! `RegisterHotKey` on a hidden window. Low-level hooks
//! (`SetWindowsHookEx(WH_KEYBOARD_LL)`) and raw keystream readers share a
//! keylogger's signature and are a documented anti-cheat flagging pattern —
//! directly relevant to this epic's "works while a fullscreen game has
//! focus" goal. The `evdev` reader used by the Linux/Wayland adapter must
//! therefore never be ported to this path.
//!
//! `WM_HOTKEY` is posted to that hidden window, so the manager must be
//! created on a thread that pumps messages. That is GPUI's main thread:
//! both `start_listening` (startup) and `rebind` (the Settings UI) are
//! called there, and GPUI's `GetMessageW(None)` loop delivers the message.
//!
//! Presses are signalled only through the shared `AppEvent` channel (AD-3):
//! this crate never depends on `voice-me-ui`.

use std::str::FromStr as _;
use std::sync::{Mutex, MutexGuard};

use global_hotkey::hotkey::HotKey;
use global_hotkey::{Error as HotKeyError, GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use voice_me_core::{AppEvent, AppEventSender, HotkeyPort, VoiceMeError};

/// Windows `HotkeyPort` adapter.
///
/// The manager is created lazily, on the first `start_listening`/`rebind`
/// call that actually has a combination to bind, so an app that has never
/// had a hotkey configured starts with no hotkey registered and no error.
pub struct WindowsHotkeyAdapter {
    /// Where presses are sent. Kept here rather than only on
    /// `start_listening`'s frame because the very first hotkey a user ever
    /// configures is bound through `rebind` (from the Settings UI), with no
    /// preceding `start_listening` call to carry the sender in.
    events: Mutex<Option<AppEventSender>>,
    backend: Mutex<Option<Backend>>,
}

impl WindowsHotkeyAdapter {
    /// Build the adapter with the sender half of the shared `AppEvent`
    /// channel (AD-3). Nothing is registered until a combination is
    /// actually bound.
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

        *backend = Some(Backend::start(hotkey, events)?);
        Ok(())
    }
}

impl HotkeyPort for WindowsHotkeyAdapter {
    fn start_listening(&self, hotkey: &str, events: AppEventSender) -> Result<(), VoiceMeError> {
        *lock(&self.events) = Some(events);
        self.bind(hotkey)
    }

    fn rebind(&self, hotkey: &str) -> Result<(), VoiceMeError> {
        self.bind(hotkey)
    }
}

struct Backend {
    /// Held for the lifetime of the adapter: dropping the manager destroys
    /// its hidden window, which releases every registration.
    manager: GlobalHotKeyManager,
    current: Option<HotKey>,
}

impl Backend {
    fn start(hotkey: &str, events: AppEventSender) -> Result<Self, VoiceMeError> {
        let manager = GlobalHotKeyManager::new().map_err(|err| {
            VoiceMeError::Other(format!("failed to start the Windows hotkey manager: {err}"))
        })?;
        let mut backend = Self {
            manager,
            current: None,
        };
        // Register before the pump starts, so a conflict on the saved
        // combination surfaces as this call's `Err` rather than as a thread
        // that quietly never fires.
        backend.rebind(hotkey)?;
        // A pump that failed to start would leave a registered hotkey that
        // never fires, so that is this call's error too. Dropping `backend`
        // on this path releases the registration.
        spawn_event_pump(events).map_err(|err| {
            VoiceMeError::Other(format!(
                "failed to start the Windows hotkey event pump: {err}"
            ))
        })?;
        Ok(backend)
    }

    /// Register `hotkey`, then release the previous combination — in that
    /// order, so a rejected new combination leaves the old one still live.
    fn rebind(&mut self, hotkey: &str) -> Result<(), VoiceMeError> {
        let next = parse_hotkey(hotkey)?;
        if self.current == Some(next) {
            // Already the live binding. Re-registering it would fail as a
            // self-conflict, which would be a nonsense error to show.
            return Ok(());
        }

        self.manager.register(next).map_err(map_register_error)?;
        if let Some(previous) = self.current.replace(next) {
            // Best-effort: the new registration is already in place, so a
            // failure to release the old one must not fail the rebind. The
            // worst case is a stale registration that still fires: the pump
            // forwards every `Pressed` regardless of id, so the old
            // combination would also send `HotkeyPressed` until the manager
            // drops.
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
///
/// A spawn failure is returned rather than panicked: a hotkey failure must
/// never cost the app its tray presence.
fn spawn_event_pump(events: AppEventSender) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("voice-me-hotkey-windows".to_string())
        .spawn(move || {
            let receiver = GlobalHotKeyEvent::receiver();
            while let Ok(event) = receiver.recv() {
                if !forwards(&event) {
                    continue;
                }
                // Send failure means the receiver (the composition root) is
                // gone — the app is shutting down; stop pumping.
                if events.unbounded_send(AppEvent::HotkeyPressed).is_err() {
                    break;
                }
            }
        })
        .map(drop)
}

/// Whether the pump turns `event` into an `AppEvent::HotkeyPressed`: only
/// the press, never the release, so one press is one event.
fn forwards(event: &GlobalHotKeyEvent) -> bool {
    event.state() == HotKeyState::Pressed
}

/// Lock helper that treats a poisoned mutex as usable: every value behind
/// these mutexes is replaced wholesale rather than mutated in place, so
/// there is no half-updated state to protect against.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_persisted_accelerator_parses_and_round_trips() {
        let hotkey = parse_hotkey("Ctrl+Alt+KeyV").unwrap();

        assert_eq!(parse_hotkey(&hotkey.into_string()).unwrap(), hotkey);
        // `Super` is what the Settings UI persists for the Windows key.
        assert!(parse_hotkey("Shift+Super+Digit1").is_ok());
        assert!(matches!(
            parse_hotkey("Ctrl+Alt+NotAKey"),
            Err(VoiceMeError::Other(_))
        ));
    }

    #[test]
    fn an_already_registered_combination_maps_to_the_conflict_variant() {
        let hotkey = parse_hotkey("Ctrl+Alt+KeyV").unwrap();

        assert!(matches!(
            map_register_error(HotKeyError::AlreadyRegistered(hotkey)),
            VoiceMeError::HotkeyAlreadyInUse
        ));
    }

    #[test]
    fn no_combination_means_nothing_is_registered() {
        // A first run with no hotkey never calls `start_listening`/`rebind`,
        // so no manager (and no hidden window) exists and nothing can fail.
        let (events, _receiver) = futures::channel::mpsc::unbounded();
        let adapter = WindowsHotkeyAdapter::new(events);

        assert!(lock(&adapter.backend).is_none());
    }

    #[test]
    fn only_the_press_is_forwarded_so_one_press_is_one_event() {
        let id = parse_hotkey("Ctrl+Alt+KeyV").unwrap().id();

        assert!(forwards(&GlobalHotKeyEvent {
            id,
            state: HotKeyState::Pressed,
        }));
        assert!(!forwards(&GlobalHotKeyEvent {
            id,
            state: HotKeyState::Released,
        }));
    }

    #[test]
    fn any_other_registration_failure_stays_generic() {
        // A conflict is the only failure the UI turns into "try another
        // combination"; everything else must not masquerade as one.
        assert!(matches!(
            map_register_error(HotKeyError::FailedToRegister("no window".to_string())),
            VoiceMeError::Other(_)
        ));
    }

    // Registers for real, so Windows only: the Ubuntu CI job also builds and
    // tests this crate, and there `global-hotkey` would need an X server.
    #[cfg(target_os = "windows")]
    #[test]
    fn a_combination_held_elsewhere_is_a_conflict_and_keeps_the_old_binding() {
        // Unusual enough that nothing else on the machine holds them.
        let held = parse_hotkey("Ctrl+Alt+Shift+F13").unwrap();
        let ours = parse_hotkey("Ctrl+Alt+Shift+F14").unwrap();

        // Stands in for another process: a second manager, with its own
        // hidden window, holding the combination first.
        let holder = GlobalHotKeyManager::new().unwrap();
        holder.register(held).unwrap();

        let (events, _receiver) = futures::channel::mpsc::unbounded();
        let mut backend = Backend::start("Ctrl+Alt+Shift+F14", events).unwrap();

        assert!(matches!(
            backend.rebind("Ctrl+Alt+Shift+F13"),
            Err(VoiceMeError::HotkeyAlreadyInUse)
        ));
        assert_eq!(backend.current, Some(ours), "the old binding stays live");
        assert!(
            matches!(
                holder.register(ours),
                Err(HotKeyError::AlreadyRegistered(_))
            ),
            "the old combination is still registered by the backend"
        );

        // Re-saving the live combination is not a self-conflict.
        backend.rebind("Ctrl+Alt+Shift+F14").unwrap();
        assert_eq!(backend.current, Some(ours));

        // Once the holder lets go, the rebind succeeds and the old
        // combination is released: the holder can now take it.
        holder.unregister(held).unwrap();
        backend.rebind("Ctrl+Alt+Shift+F13").unwrap();
        holder.register(ours).unwrap();
        holder.unregister(ours).unwrap();
    }

    // Registers for real — Windows only, as above.
    #[cfg(target_os = "windows")]
    #[test]
    fn the_first_hotkey_is_bound_through_rebind_and_later_rebinds_reuse_the_backend() {
        let first = parse_hotkey("Ctrl+Alt+Shift+F15").unwrap();
        let second = parse_hotkey("Ctrl+Alt+Shift+F16").unwrap();
        let (events, _receiver) = futures::channel::mpsc::unbounded();
        // The first hotkey a user configures comes from Settings: `rebind`,
        // with no `start_listening` before it.
        let adapter = WindowsHotkeyAdapter::new(events);

        adapter.rebind("Ctrl+Alt+Shift+F15").unwrap();
        let backend_before = {
            let backend = lock(&adapter.backend);
            let backend = backend.as_ref().expect("the first rebind starts a backend");
            assert_eq!(backend.current, Some(first));
            backend as *const Backend
        };

        adapter.rebind("Ctrl+Alt+Shift+F16").unwrap();
        {
            let backend = lock(&adapter.backend);
            let backend = backend.as_ref().expect("the backend is kept");
            assert_eq!(
                backend as *const Backend, backend_before,
                "the second rebind reuses the same backend"
            );
            assert_eq!(backend.current, Some(second));
        }

        // Release what the test registered.
        if let Some(backend) = lock(&adapter.backend).take() {
            let _ = backend.manager.unregister(second);
        }
    }
}
