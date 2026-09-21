//! Wayland backend: a passive read of the raw keyboard device streams under
//! `/dev/input`.
//!
//! Deliberately *not* an exclusive grab. `EVIOCGRAB` would steal all input
//! from every other application, so this path only reads: the combination
//! also reaches whatever window has focus, and — since nothing is registered
//! with any server — a conflict with another application cannot be detected
//! at all. Both are accepted consequences of the platform, not defects
//! (spec-2-3 decision 1).
//!
//! The matching itself ([`Matcher`]) is a pure state machine over
//! `(code, value)` pairs, which is what makes it testable without any
//! `/dev/input` access or real hardware.

use std::collections::HashSet;
use std::io;
use std::path::Path;
use std::str::FromStr as _;
use std::sync::{Arc, Mutex};

use ::evdev::{Device, EventType, KeyCode};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use voice_me_core::{AppEvent, AppEventSender, VoiceMeError};

/// Linux input event codes (`input-event-codes.h`) for the modifier keys,
/// left and right variants tracked separately because the kernel reports
/// them as distinct keys.
const KEY_LEFTCTRL: u16 = 29;
const KEY_RIGHTCTRL: u16 = 97;
const KEY_LEFTSHIFT: u16 = 42;
const KEY_RIGHTSHIFT: u16 = 54;
const KEY_LEFTALT: u16 = 56;
const KEY_RIGHTALT: u16 = 100;
const KEY_LEFTMETA: u16 = 125;
const KEY_RIGHTMETA: u16 = 126;

/// Key event values as reported by the kernel.
const VALUE_RELEASE: i32 = 0;
const VALUE_PRESS: i32 = 1;
// 2 is autorepeat — deliberately never matched, so holding the combination
// down produces exactly one `AppEvent::HotkeyPressed`.

/// A hotkey resolved onto raw Linux keycodes: the required modifier set plus
/// the single non-modifier key that triggers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Combination {
    ctrl: bool,
    alt: bool,
    shift: bool,
    meta: bool,
    /// `0` (`KEY_RESERVED`) means "nothing bound" and never matches.
    key: u16,
}

impl Combination {
    /// Parse the persisted accelerator form (e.g. `Ctrl+Alt+KeyV`) by
    /// running it through `global-hotkey`'s own parser and then mapping its
    /// `Code` onto a Linux keycode — so both backends agree, by
    /// construction, on what a stored string means.
    pub(crate) fn parse(hotkey: &str) -> Result<Self, VoiceMeError> {
        let parsed = HotKey::from_str(hotkey).map_err(|err| {
            VoiceMeError::Other(format!("unsupported hotkey \"{hotkey}\": {err}"))
        })?;
        let key = linux_keycode(parsed.key).ok_or_else(|| {
            VoiceMeError::Other(format!(
                "hotkey \"{hotkey}\" uses a key this backend cannot read from /dev/input"
            ))
        })?;
        Ok(Self {
            ctrl: parsed.mods.contains(Modifiers::CONTROL),
            alt: parsed.mods.contains(Modifiers::ALT),
            shift: parsed.mods.contains(Modifiers::SHIFT),
            meta: parsed.mods.contains(Modifiers::SUPER),
            key,
        })
    }
}

fn is_modifier(code: u16) -> bool {
    matches!(
        code,
        KEY_LEFTCTRL
            | KEY_RIGHTCTRL
            | KEY_LEFTSHIFT
            | KEY_RIGHTSHIFT
            | KEY_LEFTALT
            | KEY_RIGHTALT
            | KEY_LEFTMETA
            | KEY_RIGHTMETA
    )
}

/// The pure part of the Wayland backend: tracks which modifier keys are
/// currently held and reports whether a given key event completes the bound
/// combination.
///
/// One `Matcher` per device — modifier state is physical, and two keyboards
/// each have their own.
pub(crate) struct Matcher {
    combination: Combination,
    held: HashSet<u16>,
}

impl Matcher {
    pub(crate) fn new(combination: Combination) -> Self {
        Self {
            combination,
            held: HashSet::new(),
        }
    }

    /// Swap the bound combination without disturbing the tracked modifier
    /// state — a rebind happens while the user is physically holding keys.
    pub(crate) fn set_combination(&mut self, combination: Combination) {
        self.combination = combination;
    }

    /// Feed one `EV_KEY` event. Returns `true` exactly when this event
    /// completes the bound combination.
    pub(crate) fn on_key_event(&mut self, code: u16, value: i32) -> bool {
        if is_modifier(code) {
            match value {
                VALUE_PRESS => {
                    self.held.insert(code);
                }
                VALUE_RELEASE => {
                    self.held.remove(&code);
                }
                // Autorepeat on a held modifier changes nothing.
                _ => {}
            }
            return false;
        }

        if value != VALUE_PRESS || self.combination.key == 0 {
            return false;
        }

        code == self.combination.key && self.held_modifiers() == self.modifier_requirement()
    }

    fn held_modifiers(&self) -> (bool, bool, bool, bool) {
        let held = |a: u16, b: u16| self.held.contains(&a) || self.held.contains(&b);
        (
            held(KEY_LEFTCTRL, KEY_RIGHTCTRL),
            held(KEY_LEFTALT, KEY_RIGHTALT),
            held(KEY_LEFTSHIFT, KEY_RIGHTSHIFT),
            held(KEY_LEFTMETA, KEY_RIGHTMETA),
        )
    }

    fn modifier_requirement(&self) -> (bool, bool, bool, bool) {
        (
            self.combination.ctrl,
            self.combination.alt,
            self.combination.shift,
            self.combination.meta,
        )
    }
}

/// The running Wayland backend. Owns nothing but the shared combination —
/// one reader thread per keyboard device runs for the lifetime of the
/// process, and a rebind is just a write into this cell.
pub(crate) struct EvdevBackend {
    combination: Arc<Mutex<Combination>>,
}

impl EvdevBackend {
    pub(crate) fn start(hotkey: &str, events: AppEventSender) -> Result<Self, VoiceMeError> {
        let combination = Combination::parse(hotkey)?;
        let devices = open_keyboards()?;
        let shared = Arc::new(Mutex::new(combination));

        for device in devices {
            spawn_reader(device, shared.clone(), events.clone());
        }

        Ok(Self {
            combination: shared,
        })
    }

    pub(crate) fn rebind(&mut self, hotkey: &str) -> Result<(), VoiceMeError> {
        // Parse first: an unparseable string must not disturb the running
        // combination. There is no registration on this path, so there is
        // nothing else that can fail and nothing that can report a conflict.
        let combination = Combination::parse(hotkey)?;
        *crate::lock(&self.combination) = combination;
        Ok(())
    }
}

/// Open every readable keyboard device under `/dev/input`.
///
/// `evdev::enumerate()` silently skips devices it cannot open, which would
/// turn a missing-permission session into a silent no-op — the one outcome
/// spec-2-3 forbids. So the directory is walked directly and a
/// `PermissionDenied` is surfaced as
/// [`VoiceMeError::InputDevicePermissionDenied`], whose message names the
/// `input` group.
fn open_keyboards() -> Result<Vec<Device>, VoiceMeError> {
    let entries = match std::fs::read_dir("/dev/input") {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            return Err(VoiceMeError::InputDevicePermissionDenied);
        }
        Err(err) => return Err(err.into()),
    };

    let mut devices = Vec::new();
    let mut permission_denied = false;
    let mut unreadable_keyboard = false;

    for entry in entries.flatten() {
        let path = entry.path();
        let is_event_device = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("event"));
        if !is_event_device {
            continue;
        }

        match Device::open(&path) {
            Ok(device) if is_keyboard(&device) => devices.push(device),
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
                permission_denied = true;
                unreadable_keyboard |= sysfs_reports_keyboard(&path);
            }
            // Anything else (a device that vanished mid-walk, an unusual
            // node) is simply not a keyboard we can read.
            Err(_) => {}
        }
    }

    // A readable pseudo-keyboard (a uinput node, a VM's tablet driver)
    // alongside an unreadable real one would otherwise start a reader that
    // can never see the combination — the silent no-op this function exists
    // to prevent. Only a node sysfs positively identifies as a keyboard
    // counts, so the ordinary case (unreadable mice/switches, readable
    // keyboards) keeps working.
    if unreadable_keyboard {
        return Err(VoiceMeError::InputDevicePermissionDenied);
    }

    if devices.is_empty() {
        return Err(if permission_denied {
            VoiceMeError::InputDevicePermissionDenied
        } else {
            VoiceMeError::Other("no readable keyboard devices found under /dev/input".to_string())
        });
    }

    Ok(devices)
}

/// Treat a device that can report `KEY_A` as a keyboard. Mice, touchpads and
/// lid/power switches all expose `EV_KEY` too, but none of them report
/// letter keys.
fn is_keyboard(device: &Device) -> bool {
    device
        .supported_keys()
        .is_some_and(|keys| keys.contains(KeyCode::KEY_A))
}

/// The same `KEY_A` question as [`is_keyboard`], asked of a device that
/// could not be opened. `/sys/class/input/<eventN>/device/capabilities/key`
/// is world-readable and holds the key bitmap as hex words, least
/// significant word last, so `KEY_A` (30) lives in that last word. An
/// absent or unparseable file answers "not a keyboard" — this may only make
/// the permission check stricter where it is certain, never where it is
/// guessing.
fn sysfs_reports_keyboard(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let capabilities =
        match std::fs::read_to_string(format!("/sys/class/input/{name}/device/capabilities/key")) {
            Ok(contents) => contents,
            Err(_) => return false,
        };
    capabilities
        .split_whitespace()
        .next_back()
        .and_then(|word| u64::from_str_radix(word, 16).ok())
        .is_some_and(|bits| bits & (1 << KeyCode::KEY_A.code()) != 0)
}

fn spawn_reader(mut device: Device, combination: Arc<Mutex<Combination>>, events: AppEventSender) {
    let name = device.name().unwrap_or("input device").to_string();
    let spawned = std::thread::Builder::new()
        .name("voice-me-hotkey-evdev".to_string())
        .spawn(move || {
            let mut matcher = Matcher::new(*crate::lock(&combination));
            loop {
                let batch: Vec<(u16, i32)> = match device.fetch_events() {
                    Ok(events) => events
                        .filter(|event| event.event_type() == EventType::KEY)
                        .map(|event| (event.code(), event.value()))
                        .collect(),
                    Err(err) => {
                        // The device went away (unplugged, suspend) — stop
                        // reading this one; the others keep working.
                        eprintln!("voice-me: stopped reading {name}: {err}");
                        return;
                    }
                };

                for (code, value) in batch {
                    matcher.set_combination(*crate::lock(&combination));
                    if matcher.on_key_event(code, value)
                        && events.unbounded_send(AppEvent::HotkeyPressed).is_err()
                    {
                        // The composition root is gone — the app is exiting.
                        return;
                    }
                }
            }
        });

    if let Err(err) = spawned {
        eprintln!("voice-me: failed to start a hotkey reader thread: {err}");
    }
}

/// Map `global-hotkey`'s `Code` onto the Linux keycode the kernel reports
/// for the same physical key. Only the keys a person would plausibly bind a
/// push-to-talk hotkey to; anything else is rejected at parse time with a
/// message rather than silently never matching.
fn linux_keycode(code: Code) -> Option<u16> {
    Some(match code {
        Code::KeyA => 30,
        Code::KeyB => 48,
        Code::KeyC => 46,
        Code::KeyD => 32,
        Code::KeyE => 18,
        Code::KeyF => 33,
        Code::KeyG => 34,
        Code::KeyH => 35,
        Code::KeyI => 23,
        Code::KeyJ => 36,
        Code::KeyK => 37,
        Code::KeyL => 38,
        Code::KeyM => 50,
        Code::KeyN => 49,
        Code::KeyO => 24,
        Code::KeyP => 25,
        Code::KeyQ => 16,
        Code::KeyR => 19,
        Code::KeyS => 31,
        Code::KeyT => 20,
        Code::KeyU => 22,
        Code::KeyV => 47,
        Code::KeyW => 17,
        Code::KeyX => 45,
        Code::KeyY => 21,
        Code::KeyZ => 44,
        Code::Digit1 => 2,
        Code::Digit2 => 3,
        Code::Digit3 => 4,
        Code::Digit4 => 5,
        Code::Digit5 => 6,
        Code::Digit6 => 7,
        Code::Digit7 => 8,
        Code::Digit8 => 9,
        Code::Digit9 => 10,
        Code::Digit0 => 11,
        Code::Minus => 12,
        Code::Equal => 13,
        Code::Backspace => 14,
        Code::Tab => 15,
        Code::BracketLeft => 26,
        Code::BracketRight => 27,
        Code::Enter => 28,
        Code::Semicolon => 39,
        Code::Quote => 40,
        Code::Backquote => 41,
        Code::Backslash => 43,
        Code::Comma => 51,
        Code::Period => 52,
        Code::Slash => 53,
        Code::Space => 57,
        Code::Escape => 1,
        Code::F1 => 59,
        Code::F2 => 60,
        Code::F3 => 61,
        Code::F4 => 62,
        Code::F5 => 63,
        Code::F6 => 64,
        Code::F7 => 65,
        Code::F8 => 66,
        Code::F9 => 67,
        Code::F10 => 68,
        Code::F11 => 87,
        Code::F12 => 88,
        Code::Insert => 110,
        Code::Delete => 111,
        Code::Home => 102,
        Code::End => 107,
        Code::PageUp => 104,
        Code::PageDown => 109,
        Code::ArrowUp => 103,
        Code::ArrowLeft => 105,
        Code::ArrowRight => 106,
        Code::ArrowDown => 108,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    //! The matcher is the only part of this backend testable without real
    //! hardware or `/dev/input` access, so it is exercised directly against
    //! synthetic `(code, value)` sequences — exactly the pairs the reader
    //! thread feeds it.

    use super::*;

    const KEY_V: u16 = 47;
    const KEY_B: u16 = 48;

    fn ctrl_alt_v() -> Combination {
        Combination::parse("Ctrl+Alt+KeyV").unwrap()
    }

    #[test]
    fn parsing_resolves_modifiers_and_the_trigger_key() {
        let combination = ctrl_alt_v();
        assert_eq!(
            combination,
            Combination {
                ctrl: true,
                alt: true,
                shift: false,
                meta: false,
                key: KEY_V,
            }
        );
    }

    #[test]
    fn parsing_rejects_a_string_that_is_not_a_combination() {
        assert!(Combination::parse("Ctrl+Alt").is_err());
        assert!(Combination::parse("").is_err());
    }

    #[test]
    fn the_full_combination_fires_once_per_press() {
        let mut matcher = Matcher::new(ctrl_alt_v());

        assert!(!matcher.on_key_event(KEY_LEFTCTRL, VALUE_PRESS));
        assert!(!matcher.on_key_event(KEY_LEFTALT, VALUE_PRESS));
        assert!(matcher.on_key_event(KEY_V, VALUE_PRESS));
        // The release of the trigger key is not a second press.
        assert!(!matcher.on_key_event(KEY_V, VALUE_RELEASE));
        // Holding it down (autorepeat) is not a second press either.
        assert!(!matcher.on_key_event(KEY_V, 2));

        // Pressing it again, still holding the modifiers, fires again.
        assert!(matcher.on_key_event(KEY_V, VALUE_PRESS));
    }

    #[test]
    fn right_hand_modifiers_are_equivalent_to_left_hand_ones() {
        let mut matcher = Matcher::new(ctrl_alt_v());

        matcher.on_key_event(KEY_RIGHTCTRL, VALUE_PRESS);
        matcher.on_key_event(KEY_RIGHTALT, VALUE_PRESS);
        assert!(matcher.on_key_event(KEY_V, VALUE_PRESS));
    }

    #[test]
    fn a_missing_modifier_does_not_fire() {
        let mut matcher = Matcher::new(ctrl_alt_v());

        matcher.on_key_event(KEY_LEFTCTRL, VALUE_PRESS);
        assert!(
            !matcher.on_key_event(KEY_V, VALUE_PRESS),
            "Ctrl+V must not trigger a Ctrl+Alt+V binding"
        );
    }

    #[test]
    fn an_extra_modifier_does_not_fire() {
        let mut matcher = Matcher::new(ctrl_alt_v());

        matcher.on_key_event(KEY_LEFTCTRL, VALUE_PRESS);
        matcher.on_key_event(KEY_LEFTALT, VALUE_PRESS);
        matcher.on_key_event(KEY_LEFTSHIFT, VALUE_PRESS);
        assert!(
            !matcher.on_key_event(KEY_V, VALUE_PRESS),
            "Ctrl+Alt+Shift+V must not trigger a Ctrl+Alt+V binding"
        );
    }

    #[test]
    fn releasing_a_modifier_stops_the_combination_matching() {
        let mut matcher = Matcher::new(ctrl_alt_v());

        matcher.on_key_event(KEY_LEFTCTRL, VALUE_PRESS);
        matcher.on_key_event(KEY_LEFTALT, VALUE_PRESS);
        matcher.on_key_event(KEY_LEFTALT, VALUE_RELEASE);
        assert!(!matcher.on_key_event(KEY_V, VALUE_PRESS));
    }

    #[test]
    fn a_different_trigger_key_does_not_fire() {
        let mut matcher = Matcher::new(ctrl_alt_v());

        matcher.on_key_event(KEY_LEFTCTRL, VALUE_PRESS);
        matcher.on_key_event(KEY_LEFTALT, VALUE_PRESS);
        assert!(!matcher.on_key_event(KEY_B, VALUE_PRESS));
    }

    #[test]
    fn rebinding_swaps_which_combination_fires_without_losing_held_modifiers() {
        let mut matcher = Matcher::new(ctrl_alt_v());

        matcher.on_key_event(KEY_LEFTCTRL, VALUE_PRESS);
        matcher.on_key_event(KEY_LEFTALT, VALUE_PRESS);

        matcher.set_combination(Combination::parse("Ctrl+Alt+KeyB").unwrap());

        assert!(
            !matcher.on_key_event(KEY_V, VALUE_PRESS),
            "the old combination must stop firing once it has been replaced"
        );
        assert!(
            matcher.on_key_event(KEY_B, VALUE_PRESS),
            "the new combination fires against modifiers held across the rebind"
        );
    }

    #[test]
    fn an_unbound_matcher_never_fires() {
        let mut matcher = Matcher::new(Combination::default());

        assert!(!matcher.on_key_event(KEY_V, VALUE_PRESS));
        assert!(!matcher.on_key_event(0, VALUE_PRESS));
    }

    /// Every accelerator token `voice-me-ui`'s `accelerator_code` can emit.
    /// The two tables are connected only by convention, and a typo in
    /// either would degrade to "the hotkey silently never fires", so the
    /// contract is asserted here rather than assumed.
    const UI_KEY_TOKENS: &[&str] = &[
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

    #[test]
    fn every_key_the_ui_can_capture_parses_into_a_combination() {
        for token in UI_KEY_TOKENS {
            let hotkey = format!("Ctrl+Alt+{token}");
            assert!(
                Combination::parse(&hotkey).is_ok(),
                "the UI can capture {token}, so this backend must be able to bind it"
            );
        }
    }

    #[test]
    fn a_modifier_free_combination_requires_no_modifiers_held() {
        let mut matcher = Matcher::new(Combination::parse("F9").unwrap());

        assert!(matcher.on_key_event(67, VALUE_PRESS));

        matcher.on_key_event(KEY_LEFTCTRL, VALUE_PRESS);
        assert!(
            !matcher.on_key_event(67, VALUE_PRESS),
            "Ctrl+F9 must not trigger a bare F9 binding"
        );
    }

    /// Spec-2-3's "Save, Wayland" matrix row: this backend registers nothing,
    /// so it can never report a conflict — every valid combination binds,
    /// including one another application is already using. That is a
    /// documented capability difference from the X11 path, not an oversight,
    /// so it is pinned here: a refactor that started returning
    /// `HotkeyAlreadyInUse` would silently contradict what the Hotkey tab
    /// tells the user on a Wayland session.
    #[test]
    fn rebinding_never_reports_a_conflict() {
        // Built directly rather than through `start`, which would need real
        // `/dev/input` access; `rebind` only ever touches this cell.
        let mut backend = EvdevBackend {
            combination: Arc::new(Mutex::new(ctrl_alt_v())),
        };

        for combination in ["Ctrl+Alt+KeyB", "Ctrl+Alt+KeyV", "F9"] {
            assert!(
                backend.rebind(combination).is_ok(),
                "{combination} must bind without a conflict check"
            );
        }

        assert_eq!(
            *crate::lock(&backend.combination),
            Combination::parse("F9").unwrap()
        );

        // An unparseable string is still rejected, and must leave the
        // running combination alone.
        assert!(backend.rebind("not a combination").is_err());
        assert_eq!(
            *crate::lock(&backend.combination),
            Combination::parse("F9").unwrap()
        );
    }
}
