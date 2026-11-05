//! KDE backend: voice-me registers itself as a `kglobalaccel` component
//! (`voice-me`, action `summon`) and listens for the component's
//! `globalShortcutPressed` signal — the same mechanism native KDE
//! applications use, so the shortcut shows in System Settings → Shortcuts
//! under voice-me, and KWin grabs it exclusively.
//!
//! Unlike GNOME, kglobalaccel reports conflicts: `setShortcut` answers with
//! the keys it actually assigned, and an empty answer means the combination
//! belongs to someone else — [`VoiceMeError::HotkeyAlreadyInUse`], with the
//! previous keys put back.

use voice_me_core::{AppEvent, AppEventSender, VoiceMeError};
use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::OwnedObjectPath;

use crate::desktop::to_qt_key;

const SERVICE: &str = "org.kde.kglobalaccel";
const PATH: &str = "/kglobalaccel";
const INTERFACE: &str = "org.kde.KGlobalAccel";
const COMPONENT_INTERFACE: &str = "org.kde.kglobalaccel.Component";

const COMPONENT: &str = "voice-me";
const ACTION: &str = "summon";
const COMPONENT_FRIENDLY: &str = "voice-me";
const ACTION_FRIENDLY: &str = "Summon voice-me";

// `KGlobalAccel::SetShortcutFlag`.
const SET_PRESENT: u32 = 2;
const NO_AUTOLOADING: u32 = 4;

pub(crate) struct KdeBackend {
    connection: Connection,
}

impl KdeBackend {
    /// Register the component, assign `hotkey`, and start forwarding
    /// presses. Fails — so the adapter can fall back — when there is no
    /// session bus or no kglobalaccel on it.
    pub(crate) fn start(hotkey: &str, events: AppEventSender) -> Result<Self, VoiceMeError> {
        let connection = Connection::session()
            .map_err(|err| VoiceMeError::Other(format!("no D-Bus session bus: {err}")))?;
        let mut backend = Self { connection };

        backend.call::<_, ()>("doRegister", &(action_id(),))?;
        backend.rebind(hotkey)?;

        let component: OwnedObjectPath = backend.call("getComponent", &(COMPONENT,))?;
        spawn_listener(&backend.connection, component, events)?;
        Ok(backend)
    }

    /// Assign `hotkey`, checking what kglobalaccel actually assigned. On a
    /// conflict the previous keys are restored, so the old combination
    /// stays live until a new one is confirmed (spec 2.3).
    pub(crate) fn rebind(&mut self, hotkey: &str) -> Result<(), VoiceMeError> {
        let key = to_qt_key(hotkey)?;
        // Propagated: without the previous keys a refusal could only
        // "restore" nothing, wiping the old combination.
        let previous: Vec<i32> = self.call("shortcut", &(action_id(),))?;

        let assigned: Vec<i32> = self.call(
            "setShortcut",
            &(action_id(), vec![key], SET_PRESENT | NO_AUTOLOADING),
        )?;
        if refused(&assigned, key) {
            let _ = self.call::<_, Vec<i32>>(
                "setShortcut",
                &(action_id(), previous, SET_PRESENT | NO_AUTOLOADING),
            );
            return Err(VoiceMeError::HotkeyAlreadyInUse);
        }
        Ok(())
    }

    fn call<B, R>(&self, method: &str, body: &B) -> Result<R, VoiceMeError>
    where
        B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
    {
        let fail =
            |err: zbus::Error| VoiceMeError::Other(format!("kglobalaccel {method} failed: {err}"));
        self.connection
            .call_method(Some(SERVICE), PATH, Some(INTERFACE), method, body)
            .map_err(fail)?
            .body()
            .deserialize()
            .map_err(fail)
    }
}

/// `[componentUnique, actionUnique, componentFriendly, actionFriendly]`.
fn action_id() -> Vec<&'static str> {
    vec![COMPONENT, ACTION, COMPONENT_FRIENDLY, ACTION_FRIENDLY]
}

/// Whether `setShortcut`'s answer means `requested` was refused: KF5
/// answers a refused key with an empty list or a lone `0`, and any answer
/// that does not hold the requested key is not the binding asked for.
fn refused(assigned: &[i32], requested: i32) -> bool {
    !assigned.contains(&requested)
}

/// Forward `globalShortcutPressed(component, action, timestamp)` for our
/// action to the shared `AppEvent` channel (AD-3).
fn spawn_listener(
    connection: &Connection,
    component: OwnedObjectPath,
    events: AppEventSender,
) -> Result<(), VoiceMeError> {
    let fail = |err: zbus::Error| {
        VoiceMeError::Other(format!(
            "failed to subscribe to kglobalaccel's pressed signal: {err}"
        ))
    };
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(COMPONENT_INTERFACE)
        .map_err(fail)?
        .member("globalShortcutPressed")
        .map_err(fail)?
        .path(component.into_inner())
        .map_err(fail)?
        .build();
    let messages = MessageIterator::for_match_rule(rule, connection, None).map_err(fail)?;

    std::thread::Builder::new()
        .name("voice-me-hotkey-kde".to_string())
        .spawn(move || {
            for message in messages {
                let Ok(message) = message else { continue };
                let Ok((component, action, _timestamp)) =
                    message.body().deserialize::<(String, String, i64)>()
                else {
                    continue;
                };
                if component == COMPONENT
                    && action == ACTION
                    && events.unbounded_send(AppEvent::HotkeyPressed).is_err()
                {
                    // The composition root is gone; the app is exiting.
                    break;
                }
            }
        })
        .map_err(|err| {
            VoiceMeError::Other(format!("failed to start the KDE hotkey listener: {err}"))
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;

    /// One recorded `setShortcut(actionId, keys, flags)` call.
    type SetCall = (Vec<String>, Vec<i32>, u32);

    /// A stand-in kglobalaccel: records what it was asked, and refuses any
    /// key in `taken` the way the real one does — with an empty answer.
    #[derive(Clone, Default)]
    struct FakeKGlobalAccel {
        keys: Arc<Mutex<Vec<i32>>>,
        set_calls: Arc<Mutex<Vec<SetCall>>>,
        taken: Arc<Mutex<Vec<i32>>>,
    }

    #[zbus::interface(name = "org.kde.KGlobalAccel")]
    impl FakeKGlobalAccel {
        #[zbus(name = "doRegister")]
        fn do_register(&self, _action_id: Vec<String>) {}

        #[zbus(name = "shortcut")]
        fn shortcut(&self, _action_id: Vec<String>) -> Vec<i32> {
            self.keys.lock().unwrap().clone()
        }

        #[zbus(name = "setShortcut")]
        fn set_shortcut(&self, action_id: Vec<String>, keys: Vec<i32>, flags: u32) -> Vec<i32> {
            self.set_calls
                .lock()
                .unwrap()
                .push((action_id, keys.clone(), flags));
            if keys
                .iter()
                .any(|key| self.taken.lock().unwrap().contains(key))
            {
                return Vec::new();
            }
            *self.keys.lock().unwrap() = keys.clone();
            keys
        }

        #[zbus(name = "getComponent")]
        fn get_component(&self, _component: String) -> OwnedObjectPath {
            OwnedObjectPath::try_from("/component/voice_me").unwrap()
        }
    }

    /// The whole KDE exchange against a fake kglobalaccel on a real
    /// session bus: register, assign, forward a press, refuse a taken
    /// combination and restore the previous keys. Opt-in with
    /// `VOICE_ME_TEST_DBUS=1`, under `dbus-run-session` — never against a
    /// developer's real bus; one test, since
    /// the fake owns a well-known name.
    #[test]
    fn the_kglobalaccel_exchange_against_a_fake() {
        if std::env::var_os("VOICE_ME_TEST_DBUS").is_none() {
            eprintln!("skipped: set VOICE_ME_TEST_DBUS=1 under dbus-run-session to run");
            return;
        }
        let fake = FakeKGlobalAccel::default();
        let service = zbus::blocking::connection::Builder::session()
            .unwrap()
            .serve_at(PATH, fake.clone())
            .unwrap()
            .name(SERVICE)
            .unwrap()
            .build()
            .expect("own the fake kglobalaccel name");

        let (events, mut receiver) = futures::channel::mpsc::unbounded();
        let mut backend = KdeBackend::start("Ctrl+Alt+KeyV", events).expect("bind on KDE");
        let ctrl_alt_v = to_qt_key("Ctrl+Alt+KeyV").unwrap();
        {
            let calls = fake.set_calls.lock().unwrap();
            let (action_id, keys, flags) = calls.last().unwrap();
            assert_eq!(action_id[..2], ["voice-me", "summon"]);
            assert_eq!(keys, &vec![ctrl_alt_v]);
            assert_eq!(*flags, SET_PRESENT | NO_AUTOLOADING);
        }

        // A press: only our component/action becomes an event.
        for (component, action) in [("other", "summon"), ("voice-me", "summon")] {
            service
                .emit_signal(
                    None::<()>,
                    "/component/voice_me",
                    COMPONENT_INTERFACE,
                    "globalShortcutPressed",
                    &(component, action, 0_i64),
                )
                .unwrap();
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut presses = 0;
        while std::time::Instant::now() < deadline && presses == 0 {
            while let Ok(AppEvent::HotkeyPressed) = receiver.try_recv() {
                presses += 1;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(100));
        while let Ok(AppEvent::HotkeyPressed) = receiver.try_recv() {
            presses += 1;
        }
        assert_eq!(presses, 1, "exactly the one press for voice-me/summon");

        // A taken combination: refused as a conflict, old keys restored.
        let super_f9 = to_qt_key("Super+F9").unwrap();
        fake.taken.lock().unwrap().push(super_f9);
        assert!(matches!(
            backend.rebind("Super+F9"),
            Err(VoiceMeError::HotkeyAlreadyInUse)
        ));
        assert_eq!(*fake.keys.lock().unwrap(), vec![ctrl_alt_v]);
        assert_eq!(
            fake.set_calls.lock().unwrap().last().unwrap().1,
            vec![ctrl_alt_v],
            "the previous keys are put back"
        );

        // A free one binds.
        backend.rebind("Ctrl+Alt+KeyB").unwrap();
        assert_eq!(
            *fake.keys.lock().unwrap(),
            vec![to_qt_key("Ctrl+Alt+KeyB").unwrap()]
        );
    }

    #[test]
    fn an_answer_without_the_requested_key_is_a_refusal() {
        let key = to_qt_key("Ctrl+Alt+KeyV").unwrap();
        assert!(refused(&[], key));
        assert!(refused(&[0], key));
        assert!(refused(&[to_qt_key("Ctrl+Alt+KeyB").unwrap()], key));
        assert!(!refused(&[key], key));
    }

    #[test]
    fn the_action_id_names_component_and_action() {
        let id = action_id();
        assert_eq!(id.len(), 4);
        assert_eq!(id[0], "voice-me");
        assert_eq!(id[1], "summon");
    }
}
