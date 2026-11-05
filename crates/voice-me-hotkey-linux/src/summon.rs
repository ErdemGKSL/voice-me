//! The one IPC voice-me exposes: a session-bus service a desktop shortcut
//! (or a compositor binding) can call to summon the Prompt Overlay.
//!
//! `gdbus call --session --dest dev.voice_me.VoiceMe --object-path
//! /dev/voice_me/VoiceMe --method dev.voice_me.VoiceMe.Summon` reaches the
//! running app and becomes one `AppEvent::HotkeyPressed` on the shared
//! channel (AD-3). No executable path is involved, so moving the binary
//! never breaks a binding, and when voice-me is not running the call simply
//! fails — nothing is auto-started (no `.service` file is installed).

use voice_me_core::{AppEvent, AppEventSender, VoiceMeError};

/// Well-known bus name the service owns.
pub const SUMMON_BUS_NAME: &str = "dev.voice_me.VoiceMe";
/// Object path the interface is served at.
pub const SUMMON_OBJECT_PATH: &str = "/dev/voice_me/VoiceMe";
/// Interface name; its one method is `Summon`.
pub const SUMMON_INTERFACE: &str = "dev.voice_me.VoiceMe";

/// The shell command that summons the running voice-me — what the GNOME
/// custom keybinding runs, and what the Hotkey tab tells users of other
/// desktops to bind in their compositor.
pub const SUMMON_COMMAND: &str = "gdbus call --session --dest dev.voice_me.VoiceMe \
     --object-path /dev/voice_me/VoiceMe --method dev.voice_me.VoiceMe.Summon";

struct Summon {
    events: AppEventSender,
}

#[zbus::interface(name = "dev.voice_me.VoiceMe")]
impl Summon {
    /// Summon the Prompt Overlay, exactly as a hotkey press would.
    fn summon(&self) {
        // A send failure means the composition root's receiver is gone —
        // the app is shutting down; there is no one left to summon.
        let _ = self.events.unbounded_send(AppEvent::HotkeyPressed);
    }
}

/// The running service. Holding the connection keeps the name owned and
/// the object served; zbus's own executor thread answers the calls.
pub(crate) struct SummonService {
    _connection: zbus::blocking::Connection,
}

impl SummonService {
    /// Own [`SUMMON_BUS_NAME`] on the session bus and serve `Summon`.
    ///
    /// Fails when there is no session bus or the name is already owned —
    /// most likely by a second voice-me, whose shortcut would then summon
    /// the *other* instance.
    pub(crate) fn start(events: AppEventSender) -> Result<Self, VoiceMeError> {
        let connection = zbus::blocking::connection::Builder::session()
            .map(|builder| {
                // zbus defaults to taking the name over from its current
                // owner (and to letting others take it from us). Neither:
                // a second voice-me must fail here, not steal the shortcut.
                builder
                    .allow_name_replacements(false)
                    .replace_existing_names(false)
            })
            .and_then(|builder| builder.serve_at(SUMMON_OBJECT_PATH, Summon { events }))
            .and_then(|builder| builder.name(SUMMON_BUS_NAME))
            .and_then(|builder| builder.build())
            .map_err(|err| match err {
                zbus::Error::NameTaken => VoiceMeError::Other(format!(
                    "the D-Bus name {SUMMON_BUS_NAME} is already owned — is another voice-me running?"
                )),
                other => VoiceMeError::Other(format!(
                    "failed to start the D-Bus summon service: {other}"
                )),
            })?;
        Ok(Self {
            _connection: connection,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End to end over a real session bus: the service owns the name, the
    /// exact [`SUMMON_COMMAND`] reaches it, and one call is one
    /// `HotkeyPressed`; a second owner is refused. Opt-in with
    /// `VOICE_ME_TEST_DBUS=1`, under `dbus-run-session` (needs `gdbus`) —
    /// never against a developer's real session bus.
    #[test]
    fn the_summon_command_reaches_the_service() {
        if std::env::var_os("VOICE_ME_TEST_DBUS").is_none() {
            eprintln!("skipped: set VOICE_ME_TEST_DBUS=1 under dbus-run-session to run");
            return;
        }
        let (events, mut receiver) = futures::channel::mpsc::unbounded();
        let _service = SummonService::start(events.clone()).expect("own the summon name");

        let args: Vec<&str> = SUMMON_COMMAND.split(' ').collect();
        let status = std::process::Command::new(args[0])
            .args(&args[1..])
            .stdout(std::process::Stdio::null())
            .status()
            .expect("run gdbus");
        assert!(status.success(), "gdbus call failed: {status}");
        assert!(matches!(receiver.try_recv(), Ok(AppEvent::HotkeyPressed)));

        assert!(
            SummonService::start(events).is_err(),
            "a second owner of the name must be refused"
        );
    }

    #[test]
    fn the_command_names_the_served_name_path_and_method() {
        assert!(SUMMON_COMMAND.starts_with("gdbus call --session "));
        assert!(SUMMON_COMMAND.contains(&format!("--dest {SUMMON_BUS_NAME} ")));
        assert!(SUMMON_COMMAND.contains(&format!("--object-path {SUMMON_OBJECT_PATH} ")));
        assert!(SUMMON_COMMAND.ends_with(&format!("--method {SUMMON_INTERFACE}.Summon")));
        assert!(
            !SUMMON_COMMAND.contains("  "),
            "the line continuation must not leave doubled spaces"
        );
    }
}
