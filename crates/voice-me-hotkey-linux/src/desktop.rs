//! Which desktop this session runs, and the pure converters from the
//! persisted accelerator syntax (`Ctrl+Alt+KeyV`, spec 2.3) into the forms
//! GNOME and KDE store a shortcut in.
//!
//! Everything here is pure — no environment reads beyond [`desktop`], no
//! D-Bus, no process — so the whole mapping is unit-tested against every
//! token the Hotkey tab can emit.

use voice_me_core::VoiceMeError;

/// The desktop environment, as far as the hotkey is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desktop {
    /// GNOME (including Ubuntu's `ubuntu:GNOME`): a custom keybinding in
    /// `org.gnome.settings-daemon.plugins.media-keys`.
    Gnome,
    /// KDE Plasma: a `kglobalaccel` component.
    Kde,
    /// Anything else (Sway, Hyprland, XFCE…): the X11 or evdev backend.
    Other,
}

/// Decide the desktop from `XDG_CURRENT_DESKTOP`. Runtime, not
/// compile-time: the same binary runs everywhere.
pub fn desktop() -> Desktop {
    desktop_for(std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref())
}

/// The decision itself, separated from the environment read so it can be
/// tested without mutating process env. `XDG_CURRENT_DESKTOP` is a
/// colon-separated list (`ubuntu:GNOME`, `KDE`, `sway`); any entry naming
/// GNOME or KDE decides it, case-insensitively.
pub(crate) fn desktop_for(xdg_current_desktop: Option<&str>) -> Desktop {
    let Some(value) = xdg_current_desktop else {
        return Desktop::Other;
    };
    for entry in value.split(':').map(str::trim) {
        if entry.eq_ignore_ascii_case("gnome") {
            return Desktop::Gnome;
        }
        if entry.eq_ignore_ascii_case("kde") {
            return Desktop::Kde;
        }
    }
    Desktop::Other
}

/// The modifier half of a persisted accelerator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Mods {
    pub(crate) ctrl: bool,
    pub(crate) alt: bool,
    pub(crate) shift: bool,
    pub(crate) super_: bool,
}

/// Split a persisted accelerator into its modifiers and its one key token.
pub(crate) fn split_accelerator(accelerator: &str) -> Result<(Mods, &str), VoiceMeError> {
    let mut mods = Mods::default();
    let mut key = "";
    for token in accelerator.split('+').map(str::trim) {
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.ctrl = true,
            "alt" | "option" => mods.alt = true,
            "shift" => mods.shift = true,
            "super" | "meta" | "cmd" | "command" | "win" => mods.super_ = true,
            "" => {}
            _ if key.is_empty() => key = token,
            _ => {
                return Err(VoiceMeError::Other(format!(
                    "hotkey \"{accelerator}\" names more than one key"
                )));
            }
        }
    }
    if key.is_empty() {
        return Err(VoiceMeError::Other(format!(
            "hotkey \"{accelerator}\" has no key besides its modifiers"
        )));
    }
    Ok((mods, key))
}

fn unmappable(key: &str) -> VoiceMeError {
    VoiceMeError::Other(format!(
        "the key \"{key}\" has no name the desktop's shortcut settings understand"
    ))
}

/// The persisted accelerator in GTK accelerator syntax, as GNOME's custom
/// keybindings store it: `Ctrl+Alt+KeyV` → `<Control><Alt>v`.
pub fn to_gnome(accelerator: &str) -> Result<String, VoiceMeError> {
    let (mods, key) = split_accelerator(accelerator)?;
    let key = keysym_name(key)?;

    let mut out = String::new();
    if mods.ctrl {
        out.push_str("<Control>");
    }
    if mods.alt {
        out.push_str("<Alt>");
    }
    if mods.shift {
        out.push_str("<Shift>");
    }
    if mods.super_ {
        out.push_str("<Super>");
    }
    out.push_str(&key);
    Ok(out)
}

/// The xkb keysym name of one accelerator key token — the key half of
/// both GTK accelerators and XDG portal triggers.
pub(crate) fn keysym_name(token: &str) -> Result<String, VoiceMeError> {
    gnome_key(token).ok_or_else(|| unmappable(token))
}

fn gnome_key(token: &str) -> Option<String> {
    if let Some(letter) = single_char(token, "Key").filter(char::is_ascii_alphabetic) {
        return Some(letter.to_ascii_lowercase().to_string());
    }
    if let Some(digit) = single_char(token, "Digit").filter(char::is_ascii_digit) {
        return Some(digit.to_string());
    }
    if let Some(n) = function_key(token) {
        return Some(format!("F{n}"));
    }
    Some(
        match token {
            "Space" => "space",
            "Enter" => "Return",
            "Tab" => "Tab",
            "Backspace" => "BackSpace",
            "Delete" => "Delete",
            "Insert" => "Insert",
            "Home" => "Home",
            "End" => "End",
            "PageUp" => "Page_Up",
            "PageDown" => "Page_Down",
            "ArrowUp" => "Up",
            "ArrowDown" => "Down",
            "ArrowLeft" => "Left",
            "ArrowRight" => "Right",
            "Minus" => "minus",
            "Equal" => "equal",
            "BracketLeft" => "bracketleft",
            "BracketRight" => "bracketright",
            "Semicolon" => "semicolon",
            "Quote" => "apostrophe",
            "Backquote" => "grave",
            "Backslash" => "backslash",
            "Comma" => "comma",
            "Period" => "period",
            "Slash" => "slash",
            _ => return None,
        }
        .to_string(),
    )
}

// `Qt::KeyboardModifier` bits, as `kglobalaccel` stores them OR-ed into
// the key code.
const QT_SHIFT: i32 = 0x0200_0000;
const QT_CONTROL: i32 = 0x0400_0000;
const QT_ALT: i32 = 0x0800_0000;
const QT_META: i32 = 0x1000_0000;

/// The persisted accelerator as a Qt key combination — modifier bits OR-ed
/// with the `Qt::Key` code — which is what `kglobalaccel`'s `setShortcut`
/// takes: `Ctrl+Alt+KeyV` → `Qt::CTRL | Qt::ALT | Qt::Key_V`.
pub fn to_qt_key(accelerator: &str) -> Result<i32, VoiceMeError> {
    let (mods, key) = split_accelerator(accelerator)?;
    let mut combination = qt_key(key).ok_or_else(|| unmappable(key))?;
    if mods.ctrl {
        combination |= QT_CONTROL;
    }
    if mods.alt {
        combination |= QT_ALT;
    }
    if mods.shift {
        combination |= QT_SHIFT;
    }
    if mods.super_ {
        combination |= QT_META;
    }
    Ok(combination)
}

/// `Qt::Key` code for one accelerator key token.
fn qt_key(token: &str) -> Option<i32> {
    if let Some(letter) = single_char(token, "Key").filter(char::is_ascii_alphabetic) {
        // Qt::Key_A..Key_Z are the upper-case ASCII codes.
        return Some(letter.to_ascii_uppercase() as i32);
    }
    if let Some(digit) = single_char(token, "Digit").filter(char::is_ascii_digit) {
        // Qt::Key_0..Key_9 are the ASCII codes.
        return Some(digit as i32);
    }
    if let Some(n) = function_key(token) {
        // Qt::Key_F1 = 0x01000030, consecutive.
        return Some(0x0100_0030 + (n as i32 - 1));
    }
    Some(match token {
        "Space" => 0x20,
        "Enter" => 0x0100_0004, // Key_Return (the main Enter key)
        "Tab" => 0x0100_0001,
        "Backspace" => 0x0100_0003,
        "Insert" => 0x0100_0006,
        "Delete" => 0x0100_0007,
        "Home" => 0x0100_0010,
        "End" => 0x0100_0011,
        "ArrowLeft" => 0x0100_0012,
        "ArrowUp" => 0x0100_0013,
        "ArrowRight" => 0x0100_0014,
        "ArrowDown" => 0x0100_0015,
        "PageUp" => 0x0100_0016,
        "PageDown" => 0x0100_0017,
        "Minus" => 0x2d,
        "Equal" => 0x3d,
        "BracketLeft" => 0x5b,
        "BracketRight" => 0x5d,
        "Semicolon" => 0x3b,
        "Quote" => 0x27,     // Key_Apostrophe
        "Backquote" => 0x60, // Key_QuoteLeft
        "Backslash" => 0x5c,
        "Comma" => 0x2c,
        "Period" => 0x2e,
        "Slash" => 0x2f,
        _ => return None,
    })
}

/// `KeyV` → `'V'` for `prefix = "Key"`: exactly one character after it.
fn single_char(token: &str, prefix: &str) -> Option<char> {
    let rest = token.strip_prefix(prefix)?;
    let mut chars = rest.chars();
    let ch = chars.next()?;
    chars.next().is_none().then_some(ch)
}

/// `F1`..`F12` → 1..12.
fn function_key(token: &str) -> Option<u8> {
    let n: u8 = token.strip_prefix('F')?.parse().ok()?;
    (1..=12).contains(&n).then_some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnome_and_kde_are_found_anywhere_in_the_list_in_any_case() {
        assert_eq!(desktop_for(Some("ubuntu:GNOME")), Desktop::Gnome);
        assert_eq!(desktop_for(Some("GNOME")), Desktop::Gnome);
        assert_eq!(desktop_for(Some("gnome-classic:gnome")), Desktop::Gnome);
        assert_eq!(desktop_for(Some("KDE")), Desktop::Kde);
        assert_eq!(desktop_for(Some("kde")), Desktop::Kde);
    }

    #[test]
    fn other_desktops_and_an_unset_variable_are_other() {
        assert_eq!(desktop_for(Some("sway")), Desktop::Other);
        assert_eq!(desktop_for(Some("Hyprland")), Desktop::Other);
        assert_eq!(desktop_for(Some("X-Cinnamon")), Desktop::Other);
        // A substring is not a match: GNOME-Flashback is its own entry
        // only when it is spelt out as a list element.
        assert_eq!(desktop_for(Some("GNOME-Flashback")), Desktop::Other);
        assert_eq!(desktop_for(Some("")), Desktop::Other);
        assert_eq!(desktop_for(None), Desktop::Other);
    }

    #[test]
    fn gnome_form_of_the_spec_examples() {
        assert_eq!(to_gnome("Ctrl+Alt+KeyV").unwrap(), "<Control><Alt>v");
        assert_eq!(to_gnome("Super+F9").unwrap(), "<Super>F9");
        assert_eq!(
            to_gnome("Ctrl+Alt+Shift+Super+Digit1").unwrap(),
            "<Control><Alt><Shift><Super>1"
        );
        assert_eq!(to_gnome("F5").unwrap(), "F5");
        assert_eq!(to_gnome("Ctrl+PageDown").unwrap(), "<Control>Page_Down");
    }

    #[test]
    fn qt_form_of_the_spec_examples() {
        assert_eq!(
            to_qt_key("Ctrl+Alt+KeyV").unwrap(),
            QT_CONTROL | QT_ALT | 0x56
        );
        assert_eq!(to_qt_key("Super+F9").unwrap(), QT_META | 0x0100_0038);
        assert_eq!(to_qt_key("Shift+Digit1").unwrap(), QT_SHIFT | 0x31);
        assert_eq!(to_qt_key("F12").unwrap(), 0x0100_003b);
    }

    #[test]
    fn modifier_order_in_the_input_does_not_matter() {
        assert_eq!(to_gnome("Alt+Ctrl+KeyB").unwrap(), "<Control><Alt>b");
        assert_eq!(
            to_qt_key("Alt+Ctrl+KeyB").unwrap(),
            to_qt_key("Ctrl+Alt+KeyB").unwrap()
        );
    }

    #[test]
    fn an_unmappable_key_is_refused_naming_the_key() {
        for convert in [
            |a: &str| to_gnome(a).map(|_| ()),
            |a: &str| to_qt_key(a).map(|_| ()),
        ] {
            let error = convert("Ctrl+AudioVolumeUp").unwrap_err();
            match error {
                VoiceMeError::Other(message) => assert!(
                    message.contains("AudioVolumeUp"),
                    "the message must name the key: {message}"
                ),
                other => panic!("expected VoiceMeError::Other, got {other:?}"),
            }
            assert!(convert("Ctrl+F13").is_err());
            assert!(convert("Ctrl+KeyVV").is_err());
        }
    }

    #[test]
    fn a_modifier_only_or_two_key_string_is_refused() {
        assert!(to_gnome("Ctrl+Alt").is_err());
        assert!(to_gnome("").is_err());
        assert!(to_gnome("KeyA+KeyB").is_err());
        assert!(to_qt_key("Ctrl+Alt").is_err());
    }

    /// Every key token `voice-me-ui`'s `accelerator_code` can emit must map
    /// to a distinct name on both desktops — a gap would make Save refuse a
    /// combination the Hotkey tab happily captured, and a collision would
    /// bind a different key than the chip shows.
    #[test]
    fn every_key_the_ui_can_capture_maps_on_both_desktops() {
        let mut gnome = std::collections::HashSet::new();
        let mut qt = std::collections::HashSet::new();
        for token in crate::UI_KEY_TOKENS {
            let hotkey = format!("Ctrl+Alt+{token}");
            let g =
                to_gnome(&hotkey).unwrap_or_else(|err| panic!("{token} has no GNOME name: {err}"));
            let q = to_qt_key(&hotkey)
                .unwrap_or_else(|err| panic!("{token} has no Qt key code: {err}"));
            assert!(g.starts_with("<Control><Alt>") && g.len() > "<Control><Alt>".len());
            assert_eq!(q & (QT_CONTROL | QT_ALT), QT_CONTROL | QT_ALT);
            assert!(gnome.insert(g), "{token} collides with another GNOME name");
            assert!(qt.insert(q), "{token} collides with another Qt key code");
        }
        assert_eq!(gnome.len(), crate::UI_KEY_TOKENS.len());
    }
}
