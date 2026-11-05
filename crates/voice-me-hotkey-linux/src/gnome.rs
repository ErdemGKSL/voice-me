//! GNOME backend: one custom keybinding in
//! `org.gnome.settings-daemon.plugins.media-keys`, whose command is the
//! `gdbus call` that reaches [`crate::summon`].
//!
//! gnome-settings-daemon grabs the combination exclusively, so it does not
//! reach the focused window, and the user sees and edits it under
//! Settings → Keyboard → Custom Shortcuts as "voice-me".
//!
//! The entry lives at one fixed path, [`VOICE_ME_PATH`], updated in place.
//! The list of custom keybindings is only ever *appended to*, and only when
//! the path is missing — other entries in it are never touched.
//!
//! Every read and write goes through the `gsettings` program with a
//! deadline, so a wedged dconf can never hang the Save button.

use std::io::Read as _;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use voice_me_core::VoiceMeError;

use crate::desktop::to_gnome;
use crate::summon::SUMMON_COMMAND;

const MEDIA_KEYS_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys";
const LIST_KEY: &str = "custom-keybindings";
const CUSTOM_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";
/// voice-me's own custom keybinding.
pub(crate) const VOICE_ME_PATH: &str =
    "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/voice-me/";
/// What Settings → Keyboard → Custom Shortcuts lists the entry as.
const ENTRY_NAME: &str = "voice-me";
/// Upper bound on one `gsettings` run.
const GSETTINGS_DEADLINE: Duration = Duration::from_secs(5);

pub(crate) struct GnomeBackend;

impl GnomeBackend {
    /// Write the binding for `hotkey`. Fails — so the adapter can fall back
    /// — when `gsettings` or the media-keys schema is missing.
    pub(crate) fn start(hotkey: &str) -> Result<Self, VoiceMeError> {
        let mut backend = Self;
        backend.rebind(hotkey)?;
        Ok(backend)
    }

    /// Point the entry at `hotkey`. The binding key is written first and is
    /// put back if anything after it fails, so a failed Save leaves the
    /// previous combination live (spec 2.3). GNOME reports no conflicts, so
    /// a combination another shortcut already uses is not detectable here.
    pub(crate) fn rebind(&mut self, hotkey: &str) -> Result<(), VoiceMeError> {
        let binding = to_gnome(hotkey)?;

        // Read the list first: this is also the check that gsettings and
        // the schema exist at all, before anything is written.
        let list = gsettings(&["get", MEDIA_KEYS_SCHEMA, LIST_KEY])?;
        let entries = parse_string_list(&list).ok_or_else(|| {
            VoiceMeError::Other(format!(
                "could not read GNOME's custom keybindings list: {}",
                list.trim()
            ))
        })?;

        let custom = format!("{CUSTOM_SCHEMA}:{VOICE_ME_PATH}");
        let previous = gsettings(&["get", &custom, "binding"]).ok();

        let written = (|| {
            gsettings(&["set", &custom, "binding", &gvariant_string(&binding)])?;
            gsettings(&["set", &custom, "name", &gvariant_string(ENTRY_NAME)])?;
            gsettings(&["set", &custom, "command", &gvariant_string(SUMMON_COMMAND)])?;
            if !entries.iter().any(|entry| entry == VOICE_ME_PATH) {
                gsettings(&[
                    "set",
                    MEDIA_KEYS_SCHEMA,
                    LIST_KEY,
                    &with_voice_me_path(&list),
                ])?;
            }
            Ok(())
        })();

        if written.is_err()
            && let Some(previous) = previous
        {
            // Best effort: `previous` is already GVariant text.
            let _ = gsettings(&["set", &custom, "binding", previous.trim()]);
        }
        written
    }
}

/// The custom-keybindings list (GVariant text, as `gsettings get` prints
/// it) with [`VOICE_ME_PATH`] present exactly once. Every other entry is
/// kept, in order. An unparseable input is returned unchanged, so a caller
/// can never write back a list it did not understand.
pub(crate) fn with_voice_me_path(list_gvariant: &str) -> String {
    let Some(mut entries) = parse_string_list(list_gvariant) else {
        return list_gvariant.to_string();
    };
    if !entries.iter().any(|entry| entry == VOICE_ME_PATH) {
        entries.push(VOICE_ME_PATH.to_string());
    }
    let quoted: Vec<String> = entries.iter().map(|entry| gvariant_string(entry)).collect();
    format!("[{}]", quoted.join(", "))
}

/// Parse a GVariant `as` in text form: `@as []`, `[]`, or
/// `['a', "b", …]`. `None` for anything else.
pub(crate) fn parse_string_list(text: &str) -> Option<Vec<String>> {
    let text = text.trim();
    let text = text
        .strip_prefix("@as")
        .map(str::trim_start)
        .unwrap_or(text);
    let inner = text.strip_prefix('[')?.strip_suffix(']')?;

    let mut entries = Vec::new();
    let mut chars = inner.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let Some(quote) = chars.next() else {
            break;
        };
        if quote != '\'' && quote != '"' {
            return None;
        }
        let mut entry = String::new();
        loop {
            match chars.next()? {
                '\\' => entry.push(chars.next()?),
                c if c == quote => break,
                c => entry.push(c),
            }
        }
        entries.push(entry);
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        match chars.next() {
            None => break,
            Some(',') => {}
            Some(_) => return None,
        }
    }
    Some(entries)
}

/// A string as GVariant text: single-quoted, with `\` and `'` escaped.
fn gvariant_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for c in value.chars() {
        if c == '\\' || c == '\'' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// Run `gsettings` with `args` (no shell), bounded by
/// [`GSETTINGS_DEADLINE`]. Returns its stdout.
fn gsettings(args: &[&str]) -> Result<String, VoiceMeError> {
    let fail = |reason: String| {
        VoiceMeError::Other(format!("gsettings {} failed: {reason}", args.join(" ")))
    };

    let mut child = Command::new("gsettings")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| fail(err.to_string()))?;

    let deadline = Instant::now() + GSETTINGS_DEADLINE;
    let status = loop {
        match child.try_wait().map_err(|err| fail(err.to_string()))? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(fail(format!(
                    "no answer within {}s",
                    GSETTINGS_DEADLINE.as_secs()
                )));
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };

    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    if !status.success() {
        return Err(fail(if stderr.trim().is_empty() {
            status.to_string()
        } else {
            stderr.trim().to_string()
        }));
    }
    Ok(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The GNOME matrix rows against a real `gsettings`, opt-in: run with
    /// `VOICE_ME_TEST_GSETTINGS=1`, `GSETTINGS_BACKEND=keyfile`, a throwaway
    /// `XDG_CONFIG_HOME`, and `GSETTINGS_SCHEMA_DIR` holding the two
    /// media-keys schemas. Never run against a real desktop's dconf.
    #[test]
    fn save_and_rebind_through_real_gsettings() {
        if std::env::var_os("VOICE_ME_TEST_GSETTINGS").is_none() {
            eprintln!("skipped: set VOICE_ME_TEST_GSETTINGS to run");
            return;
        }
        let custom = format!("{CUSTOM_SCHEMA}:{VOICE_ME_PATH}");
        gsettings(&["set", MEDIA_KEYS_SCHEMA, LIST_KEY, &format!("['{OTHER}']")]).unwrap();

        // GNOME save.
        let mut backend = GnomeBackend::start("Ctrl+Alt+KeyV").unwrap();
        assert_eq!(
            gsettings(&["get", &custom, "binding"]).unwrap().trim(),
            "'<Control><Alt>v'"
        );
        assert_eq!(
            gsettings(&["get", &custom, "name"]).unwrap().trim(),
            "'voice-me'"
        );
        let command = gsettings(&["get", &custom, "command"]).unwrap();
        assert_eq!(
            parse_string_list(&format!("[{}]", command.trim())).unwrap(),
            vec![SUMMON_COMMAND.to_string()]
        );
        let list = || {
            parse_string_list(&gsettings(&["get", MEDIA_KEYS_SCHEMA, LIST_KEY]).unwrap()).unwrap()
        };
        assert_eq!(list(), vec![OTHER.to_string(), VOICE_ME_PATH.to_string()]);

        // GNOME rebind: same path, new binding, list unchanged.
        backend.rebind("Super+F9").unwrap();
        assert_eq!(
            gsettings(&["get", &custom, "binding"]).unwrap().trim(),
            "'<Super>F9'"
        );
        assert_eq!(list(), vec![OTHER.to_string(), VOICE_ME_PATH.to_string()]);

        // Unmappable: refused, binding untouched.
        assert!(backend.rebind("Ctrl+AudioVolumeUp").is_err());
        assert_eq!(
            gsettings(&["get", &custom, "binding"]).unwrap().trim(),
            "'<Super>F9'"
        );
    }

    const OTHER: &str = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/custom0/";
    const OTHER2: &str =
        "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/custom1/";

    fn expected(entries: &[&str]) -> String {
        let quoted: Vec<String> = entries.iter().map(|e| format!("'{e}'")).collect();
        format!("[{}]", quoted.join(", "))
    }

    #[test]
    fn an_empty_list_gains_the_voice_me_path() {
        assert_eq!(with_voice_me_path("[]"), expected(&[VOICE_ME_PATH]));
    }

    #[test]
    fn gsettings_typed_empty_list_gains_the_voice_me_path() {
        assert_eq!(with_voice_me_path("@as []\n"), expected(&[VOICE_ME_PATH]));
    }

    #[test]
    fn an_existing_entry_is_not_added_twice() {
        let list = expected(&[OTHER, VOICE_ME_PATH]);
        assert_eq!(with_voice_me_path(&list), list);
        assert_eq!(
            with_voice_me_path(&format!("{list}\n")),
            list,
            "the trailing newline gsettings prints changes nothing"
        );
    }

    #[test]
    fn other_entries_are_kept_in_order_and_voice_me_is_appended() {
        let list = format!("['{OTHER}', \"{OTHER2}\"]");
        assert_eq!(
            with_voice_me_path(&list),
            expected(&[OTHER, OTHER2, VOICE_ME_PATH])
        );
    }

    #[test]
    fn an_unparseable_list_is_returned_unchanged() {
        for garbage in ["", "nonsense", "['unterminated", "['a' 'b']", "[42]"] {
            assert_eq!(with_voice_me_path(garbage), garbage);
            assert!(parse_string_list(garbage).is_none(), "{garbage}");
        }
    }

    #[test]
    fn quoting_round_trips_through_the_parser() {
        let tricky = r"it's a \ path";
        let text = format!("[{}]", gvariant_string(tricky));
        assert_eq!(parse_string_list(&text).unwrap(), vec![tricky.to_string()]);
    }

    #[test]
    fn the_command_is_quoted_as_one_gvariant_string() {
        let quoted = gvariant_string(SUMMON_COMMAND);
        assert_eq!(
            parse_string_list(&format!("[{quoted}]")).unwrap(),
            vec![SUMMON_COMMAND.to_string()]
        );
    }
}
