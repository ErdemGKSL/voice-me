//! Portal backend: the xdg-desktop-portal `GlobalShortcuts` interface,
//! through `ashpd`. Tried on Wayland sessions whose desktop is neither
//! GNOME nor KDE (or whose native path failed) — Hyprland, for example —
//! before falling back to the passive evdev read.
//!
//! One session per process, one shortcut in it: `summon`, with the saved
//! accelerator as its *preferred* trigger. The compositor decides the final
//! trigger (some ask the user, some ignore the preference and let the user
//! assign it in their own config), so this backend cannot detect conflicts.
//! `Activated` for `summon` becomes `AppEvent::HotkeyPressed` (AD-3).
//!
//! `ashpd` is async; it runs on this crate's own threads with
//! `futures::executor::block_on`, never on GPUI's executor, and every call
//! the Save button waits on is bounded: [`PORTAL_DEADLINE`] for opening the
//! portal and the session, [`BIND_DEADLINE`] for the bind itself, which a
//! portal may hold open on a confirmation dialog. Timing out early there
//! would fall back to evdev while the portal still binds later, and every
//! press would then fire twice.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use ashpd::desktop::Session;
use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut};
use futures::StreamExt as _;
use voice_me_core::{AppEvent, AppEventSender, VoiceMeError};

use crate::desktop::{keysym_name, split_accelerator};

/// The one shortcut id voice-me binds.
const SHORTCUT_ID: &str = "summon";
const SHORTCUT_DESCRIPTION: &str = "Summon voice-me";
/// Upper bound on one portal round trip the caller waits for.
const PORTAL_DEADLINE: Duration = Duration::from_secs(10);
/// Upper bound on `BindShortcuts`, long enough for a user to answer the
/// portal's confirmation dialog.
const BIND_DEADLINE: Duration = Duration::from_secs(120);

pub(crate) struct PortalBackend {
    portal: Arc<GlobalShortcuts>,
    session: Arc<Session<GlobalShortcuts>>,
}

impl PortalBackend {
    /// Open the portal, create a session, bind `summon`, and start
    /// forwarding `Activated`. Fails — so the adapter can fall back — when
    /// there is no portal, it lacks `GlobalShortcuts`, or the bind fails.
    pub(crate) fn start(hotkey: &str, events: AppEventSender) -> Result<Self, VoiceMeError> {
        let trigger = to_portal_trigger(hotkey)?;

        let (portal, session) = bounded(PORTAL_DEADLINE, async {
            let portal = GlobalShortcuts::new().await?;
            let session = portal.create_session(Default::default()).await?;
            Ok((portal, session))
        })?;
        let backend = Self {
            portal: Arc::new(portal),
            session: Arc::new(session),
        };

        spawn_listener(backend.portal.clone(), events)?;
        backend.bind(trigger)?;
        Ok(backend)
    }

    /// Bind `summon` again in the same session with the new preferred
    /// trigger.
    pub(crate) fn rebind(&mut self, hotkey: &str) -> Result<(), VoiceMeError> {
        let trigger = to_portal_trigger(hotkey)?;
        self.bind(trigger)
    }

    fn bind(&self, trigger: String) -> Result<(), VoiceMeError> {
        let portal = self.portal.clone();
        let session = self.session.clone();
        let bound = bounded(BIND_DEADLINE, async move {
            let shortcut =
                NewShortcut::new(SHORTCUT_ID, SHORTCUT_DESCRIPTION).preferred_trigger(&*trigger);
            let request = portal
                .bind_shortcuts(&session, &[shortcut], None, BindShortcutsOptions::default())
                .await?;
            let response = request.response()?;
            Ok(response
                .shortcuts()
                .iter()
                .any(|shortcut| shortcut.id() == SHORTCUT_ID))
        })?;
        if bound {
            Ok(())
        } else {
            Err(VoiceMeError::Other(
                "the GlobalShortcuts portal did not bind the shortcut".to_string(),
            ))
        }
    }
}

/// Run `future` to completion on its own thread, waiting at most
/// `deadline` for it.
fn bounded<T, F>(deadline: Duration, future: F) -> Result<T, VoiceMeError>
where
    T: Send + 'static,
    F: std::future::Future<Output = Result<T, ashpd::Error>> + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("voice-me-hotkey-portal-call".to_string())
        .spawn(move || {
            let _ = tx.send(futures::executor::block_on(future));
        })
        .map_err(|err| VoiceMeError::Other(format!("failed to reach the portal: {err}")))?;

    match rx.recv_timeout(deadline) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(VoiceMeError::Other(format!(
            "GlobalShortcuts portal: {err}"
        ))),
        Err(_) => Err(VoiceMeError::Other(format!(
            "the GlobalShortcuts portal did not answer within {}s",
            deadline.as_secs()
        ))),
    }
}

/// Forward `Activated` for `summon` to the shared channel, from a thread of
/// our own for the lifetime of the process.
fn spawn_listener(
    portal: Arc<GlobalShortcuts>,
    events: AppEventSender,
) -> Result<(), VoiceMeError> {
    // Subscribe before binding, so a press right after the bind is not
    // lost; the subscription itself is bounded like every other call.
    let activated = {
        let portal = portal.clone();
        bounded(
            PORTAL_DEADLINE,
            async move { portal.receive_activated().await },
        )?
    };
    std::thread::Builder::new()
        .name("voice-me-hotkey-portal".to_string())
        .spawn(move || {
            // Keep the proxy alive as long as the stream it feeds.
            let _portal = portal;
            futures::executor::block_on(async move {
                let mut activated = std::pin::pin!(activated);
                while let Some(activation) = activated.next().await {
                    if activation.shortcut_id() == SHORTCUT_ID
                        && events.unbounded_send(AppEvent::HotkeyPressed).is_err()
                    {
                        // The composition root is gone; the app is exiting.
                        break;
                    }
                }
            });
        })
        .map_err(|err| {
            VoiceMeError::Other(format!("failed to start the portal hotkey listener: {err}"))
        })?;
    Ok(())
}

/// The persisted accelerator in the XDG "shortcuts" specification's
/// trigger syntax, which the portal takes as the preferred trigger:
/// `Ctrl+Alt+KeyV` → `CTRL+ALT+v`, `Super+F9` → `LOGO+F9`. Keys use their
/// xkb keysym names.
pub(crate) fn to_portal_trigger(accelerator: &str) -> Result<String, VoiceMeError> {
    let (modifiers, key) = split_accelerator(accelerator)?;
    let mut parts = Vec::new();
    if modifiers.ctrl {
        parts.push("CTRL".to_string());
    }
    if modifiers.alt {
        parts.push("ALT".to_string());
    }
    if modifiers.shift {
        parts.push("SHIFT".to_string());
    }
    if modifiers.super_ {
        parts.push("LOGO".to_string());
    }
    parts.push(keysym_name(key)?);
    Ok(parts.join("+"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_examples_convert_to_the_xdg_trigger_syntax() {
        assert_eq!(to_portal_trigger("Ctrl+Alt+KeyV").unwrap(), "CTRL+ALT+v");
        assert_eq!(to_portal_trigger("Super+F9").unwrap(), "LOGO+F9");
        assert_eq!(
            to_portal_trigger("Shift+Ctrl+ArrowUp").unwrap(),
            "CTRL+SHIFT+Up"
        );
        assert_eq!(to_portal_trigger("F5").unwrap(), "F5");
    }

    #[test]
    fn unmappable_and_modifier_only_strings_are_refused() {
        assert!(to_portal_trigger("Ctrl+AudioVolumeUp").is_err());
        assert!(to_portal_trigger("Ctrl+Alt").is_err());
    }

    #[test]
    fn every_key_the_ui_can_capture_has_a_trigger() {
        for token in crate::UI_KEY_TOKENS {
            let trigger = to_portal_trigger(&format!("Ctrl+Alt+{token}"))
                .unwrap_or_else(|err| panic!("{token} has no portal trigger: {err}"));
            assert!(trigger.starts_with("CTRL+ALT+"), "{trigger}");
        }
    }
}
