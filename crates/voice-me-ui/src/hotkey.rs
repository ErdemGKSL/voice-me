//! Hotkey settings (Story 2.3): the Change → press a combination → chip
//! preview → Save flow.
//!
//! The view never talks to the OS itself. It asks `HotkeyPort::rebind` to
//! make a combination live *first* and only persists through
//! `SettingsStore::save_hotkey` once that succeeded, so a rejected
//! combination leaves both the previous binding and `settings.toml`
//! untouched. `HotkeyPort` is injected as a trait object for the same reason
//! `voice_setup.rs` injects `CaptureSource`: tests drive the whole flow —
//! conflicts, permission failures, saves — against a fake, with no real
//! X11/evdev backend in the loop.

use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _,
    alert::Alert,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, FocusHandle, Focusable, FontWeight, InteractiveElement as _, IntoElement,
    KeyDownEvent, Modifiers, ModifiersChangedEvent, ParentElement as _, Render, SharedString,
    Styled as _, TestSupportExt as _, Window, div, px,
};
use voice_me_core::{HotkeyPort, SettingsStore, VoiceMeError};

const CAPTURE_PROMPT: &str = "Press the combination you want to use. Escape cancels.";
const MODIFIER_ONLY_MESSAGE: &str =
    "Add a key to the modifiers — Ctrl or Alt on their own can't be a hotkey.";
const UNSUPPORTED_KEY_MESSAGE: &str = "That key can't be used as a hotkey. Try another.";
const CONFLICT_MESSAGE: &str = "This combination is already in use. Try another.";
const NO_HOTKEY_LABEL: &str = "None set";

/// What a key press during capture amounts to. Pure — no view state, no
/// window, no OS — so every row of the story's capture matrix can be
/// asserted directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureOutcome {
    /// Escape: leave capture, change nothing.
    Cancelled,
    /// A modifier on its own. Capture stays open, waiting for a real key.
    ModifierOnly,
    /// A key this app can't bind (media keys, dead keys, …).
    UnsupportedKey,
    /// A complete combination, in the persisted accelerator syntax
    /// (e.g. `Ctrl+Alt+KeyV`).
    Captured(String),
}

/// Interpret one captured keystroke. `key` is GPUI's key name (`"v"`,
/// `"f5"`, `"control_l"`, `"escape"`), `modifiers` the state at the time it
/// was pressed.
pub fn capture_keystroke(key: &str, modifiers: Modifiers) -> CaptureOutcome {
    let key = key.to_ascii_lowercase();

    if key == "escape" {
        return CaptureOutcome::Cancelled;
    }
    if is_modifier_key_name(&key) {
        return CaptureOutcome::ModifierOnly;
    }
    let Some(code) = accelerator_code(&key) else {
        return CaptureOutcome::UnsupportedKey;
    };

    let mut parts = Vec::new();
    if modifiers.control {
        parts.push("Ctrl".to_string());
    }
    if modifiers.alt {
        parts.push("Alt".to_string());
    }
    if modifiers.shift {
        parts.push("Shift".to_string());
    }
    if modifiers.platform {
        parts.push("Super".to_string());
    }
    parts.push(code);
    CaptureOutcome::Captured(parts.join("+"))
}

/// The chip's display form of a persisted accelerator string:
/// `Ctrl+Alt+KeyV` → `Ctrl+Alt+V`, `Super` rather than `Meta`. Derived for
/// rendering only — never persisted, so the stored string always stays
/// parseable by the adapters.
pub fn display_hotkey(accelerator: &str) -> String {
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut super_ = false;
    let mut key = String::new();

    for token in accelerator.split('+') {
        let token = token.trim();
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "alt" | "option" => alt = true,
            "shift" => shift = true,
            "super" | "meta" | "cmd" | "command" => super_ = true,
            "" => {}
            _ => key = display_key(token),
        }
    }

    let mut parts = Vec::new();
    if ctrl {
        parts.push("Ctrl".to_string());
    }
    if alt {
        parts.push("Alt".to_string());
    }
    if shift {
        parts.push("Shift".to_string());
    }
    if super_ {
        parts.push("Super".to_string());
    }
    if !key.is_empty() {
        parts.push(key);
    }
    parts.join("+")
}

fn display_key(token: &str) -> String {
    if let Some(letter) = token.strip_prefix("Key") {
        return letter.to_string();
    }
    if let Some(digit) = token.strip_prefix("Digit") {
        return digit.to_string();
    }
    if let Some(arrow) = token.strip_prefix("Arrow") {
        return arrow.to_string();
    }
    match token {
        "BracketLeft" => "[".to_string(),
        "BracketRight" => "]".to_string(),
        "Backquote" => "`".to_string(),
        "Backslash" => "\\".to_string(),
        "Semicolon" => ";".to_string(),
        "Quote" => "'".to_string(),
        "Comma" => ",".to_string(),
        "Period" => ".".to_string(),
        "Slash" => "/".to_string(),
        "Minus" => "-".to_string(),
        "Equal" => "=".to_string(),
        other => other.to_string(),
    }
}

fn is_modifier_key_name(key: &str) -> bool {
    matches!(
        key,
        "ctrl"
            | "control"
            | "control_l"
            | "control_r"
            | "alt"
            | "alt_l"
            | "alt_r"
            | "meta_l"
            | "meta_r"
            | "shift"
            | "shift_l"
            | "shift_r"
            | "super"
            | "super_l"
            | "super_r"
            | "cmd"
            | "command"
            | "win"
            | "fn"
            | "function"
            | "iso_level3_shift"
            | "caps_lock"
            | "capslock"
            | "num_lock"
            | "numlock"
    )
}

/// GPUI key name → the accelerator `Code` token both adapters parse.
fn accelerator_code(key: &str) -> Option<String> {
    if key.len() == 1 {
        let ch = key.chars().next()?;
        if ch.is_ascii_alphabetic() {
            return Some(format!("Key{}", ch.to_ascii_uppercase()));
        }
        if ch.is_ascii_digit() {
            return Some(format!("Digit{ch}"));
        }
    }

    Some(
        match key {
            "f1" => "F1",
            "f2" => "F2",
            "f3" => "F3",
            "f4" => "F4",
            "f5" => "F5",
            "f6" => "F6",
            "f7" => "F7",
            "f8" => "F8",
            "f9" => "F9",
            "f10" => "F10",
            "f11" => "F11",
            "f12" => "F12",
            "space" => "Space",
            "enter" | "return" => "Enter",
            "tab" => "Tab",
            "backspace" => "Backspace",
            "delete" => "Delete",
            "insert" => "Insert",
            "home" => "Home",
            "end" => "End",
            "pageup" | "page_up" => "PageUp",
            "pagedown" | "page_down" => "PageDown",
            "up" => "ArrowUp",
            "down" => "ArrowDown",
            "left" => "ArrowLeft",
            "right" => "ArrowRight",
            "-" => "Minus",
            "=" => "Equal",
            "[" => "BracketLeft",
            "]" => "BracketRight",
            ";" => "Semicolon",
            "'" => "Quote",
            "`" => "Backquote",
            "\\" => "Backslash",
            "," => "Comma",
            "." => "Period",
            "/" => "Slash",
            _ => return None,
        }
        .to_string(),
    )
}

/// Why the Hotkey tab is showing a message. The kind is kept alongside the
/// text so tests (and future copy changes) can tell a conflict, a permission
/// problem and a generic failure apart without matching on strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HotkeyIssueKind {
    ModifierOnly,
    UnsupportedKey,
    Conflict,
    Permission,
    Other,
}

impl HotkeyIssueKind {
    /// Zero-visual-footprint marker id, so a UI test can assert *which*
    /// problem is on screen (this file's `Alert`s, like `voice_setup.rs`'s,
    /// don't register themselves for `find`).
    fn marker_id(self) -> &'static str {
        match self {
            HotkeyIssueKind::ModifierOnly => "hotkey-error-modifier-only",
            HotkeyIssueKind::UnsupportedKey => "hotkey-error-unsupported-key",
            HotkeyIssueKind::Conflict => "hotkey-error-conflict",
            HotkeyIssueKind::Permission => "hotkey-error-permission",
            HotkeyIssueKind::Other => "hotkey-error-other",
        }
    }
}

#[derive(Debug, Clone)]
struct HotkeyIssue {
    kind: HotkeyIssueKind,
    message: SharedString,
}

impl HotkeyIssue {
    fn new(kind: HotkeyIssueKind, message: impl Into<SharedString>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Map a port failure onto what the user is told. The permission case
    /// keeps `VoiceMeError`'s own wording, which names the `input` group —
    /// the one thing that makes the failure actionable.
    fn from_error(error: &VoiceMeError) -> Self {
        match error {
            VoiceMeError::HotkeyAlreadyInUse => {
                Self::new(HotkeyIssueKind::Conflict, CONFLICT_MESSAGE)
            }
            VoiceMeError::InputDevicePermissionDenied => {
                Self::new(HotkeyIssueKind::Permission, error.to_string())
            }
            other => Self::new(
                HotkeyIssueKind::Other,
                format!("Couldn't set that hotkey: {other}"),
            ),
        }
    }
}

/// The Hotkey tab.
pub struct HotkeyView {
    settings_store: Arc<dyn SettingsStore>,
    hotkey_port: Arc<dyn HotkeyPort>,
    focus_handle: FocusHandle,
    capture_mode: bool,
    pending_hotkey: Option<String>,
    saved_hotkey: Option<String>,
    hotkey_error: Option<HotkeyIssue>,
    /// Whether a modifier is currently held during capture. GPUI never
    /// dispatches a key-down for a modifier pressed on its own (Linux
    /// swallows modifier keysyms), so the "you pressed only a modifier" case
    /// is detected from the modifiers going back to none instead.
    modifiers_held: bool,
}

impl HotkeyView {
    /// `saved_hotkey` is the startup `AppState.hotkey`; `startup_error` is
    /// the message from a failed `start_listening` at launch (a Wayland
    /// session without `input`-group membership, most importantly), which is
    /// shown here rather than being left to stderr alone.
    pub fn new(
        settings_store: Arc<dyn SettingsStore>,
        hotkey_port: Arc<dyn HotkeyPort>,
        saved_hotkey: Option<String>,
        startup_error: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            settings_store,
            hotkey_port,
            focus_handle: cx.focus_handle(),
            capture_mode: false,
            pending_hotkey: None,
            saved_hotkey,
            hotkey_error: startup_error
                .map(|message| HotkeyIssue::new(HotkeyIssueKind::Permission, message)),
            modifiers_held: false,
        }
    }

    fn start_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.capture_mode = true;
        self.pending_hotkey = None;
        self.hotkey_error = None;
        self.modifiers_held = false;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn cancel_capture(&mut self, cx: &mut Context<Self>) {
        self.capture_mode = false;
        self.pending_hotkey = None;
        self.hotkey_error = None;
        self.modifiers_held = false;
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if !self.capture_mode {
            return;
        }

        match capture_keystroke(&event.keystroke.key, event.keystroke.modifiers) {
            CaptureOutcome::Cancelled => self.cancel_capture(cx),
            CaptureOutcome::ModifierOnly => {
                // Stay in capture — the user hasn't finished the
                // combination yet, they've only pressed part of it.
                self.hotkey_error = Some(HotkeyIssue::new(
                    HotkeyIssueKind::ModifierOnly,
                    MODIFIER_ONLY_MESSAGE,
                ));
                cx.notify();
            }
            CaptureOutcome::UnsupportedKey => {
                self.hotkey_error = Some(HotkeyIssue::new(
                    HotkeyIssueKind::UnsupportedKey,
                    UNSUPPORTED_KEY_MESSAGE,
                ));
                cx.notify();
            }
            CaptureOutcome::Captured(accelerator) => {
                self.pending_hotkey = Some(accelerator);
                self.capture_mode = false;
                self.hotkey_error = None;
                self.modifiers_held = false;
                cx.notify();
            }
        }
    }

    /// A modifier released back to nothing, with no key captured, is the
    /// only signal available that the user pressed a modifier on its own.
    fn on_modifiers_changed(&mut self, event: &ModifiersChangedEvent, cx: &mut Context<Self>) {
        if !self.capture_mode {
            return;
        }

        let any_held = event.modifiers.modified();
        if any_held {
            self.modifiers_held = true;
            return;
        }
        if self.modifiers_held {
            self.modifiers_held = false;
            self.hotkey_error = Some(HotkeyIssue::new(
                HotkeyIssueKind::ModifierOnly,
                MODIFIER_ONLY_MESSAGE,
            ));
            cx.notify();
        }
    }

    /// Bind first, persist second. A rejected combination never reaches
    /// `settings.toml`, and a persist failure puts the previous combination
    /// back so the two can't diverge.
    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_hotkey.clone() else {
            return;
        };

        if let Err(error) = self.hotkey_port.rebind(&pending) {
            self.hotkey_error = Some(HotkeyIssue::from_error(&error));
            cx.notify();
            return;
        }

        match self.settings_store.save_hotkey(Some(&pending)) {
            Ok(_state) => {
                self.saved_hotkey = Some(pending);
                self.pending_hotkey = None;
                self.hotkey_error = None;
            }
            Err(error) => {
                // The binding changed but the file didn't — restore the
                // previously saved combination so what fires matches what is
                // on disk. Best effort: if even that fails there is nothing
                // further this view can do beyond saying so.
                if let Some(previous) = self.saved_hotkey.clone() {
                    let _ = self.hotkey_port.rebind(&previous);
                }
                self.hotkey_error = Some(HotkeyIssue::new(
                    HotkeyIssueKind::Other,
                    format!("Couldn't save the hotkey: {error}"),
                ));
            }
        }
        cx.notify();
    }
}

impl Focusable for HotkeyView {
    fn focus_handle(&self, _cx: &gpui_kit::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for HotkeyView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let capture_mode = self.capture_mode;
        let has_pending = self.pending_hotkey.is_some();
        // The chip previews the unsaved combination while one is pending,
        // and otherwise shows whatever is actually bound.
        let chip_hotkey = self
            .pending_hotkey
            .as_deref()
            .or(self.saved_hotkey.as_deref())
            .map(display_hotkey);

        v_flex()
            .id("hotkey-capture-surface")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                this.on_key_down(event, cx)
            }))
            .on_modifiers_changed(cx.listener(
                |this, event: &ModifiersChangedEvent, _window, cx| {
                    this.on_modifiers_changed(event, cx)
                },
            ))
            .size_full()
            .p_6()
            .gap_4()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(div().text_lg().child("Global hotkey"))
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(if capture_mode {
                        CAPTURE_PROMPT
                    } else {
                        "The combination that summons voice-me from anywhere, including over a fullscreen game."
                    }),
            )
            // Test-only marker (zero visual footprint), matching
            // `voice_setup.rs`'s established pattern.
            .when(capture_mode, |el| {
                el.child(div().id("hotkey-capture-active").test_support())
            })
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child("Hotkey:")
                    .when_some(chip_hotkey, |el, hotkey| {
                        el.child(
                            div()
                                .id("hotkey-chip")
                                .test_support()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(cx.theme().muted)
                                .font_family("monospace")
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(hotkey),
                        )
                    })
                    .when(!has_pending && self.saved_hotkey.is_none(), |el| {
                        el.child(
                            div()
                                .id("hotkey-chip-empty")
                                .test_support()
                                .text_color(cx.theme().muted_foreground)
                                .child(NO_HOTKEY_LABEL),
                        )
                    }),
            )
            .when_some(self.hotkey_error.clone(), |el, issue| {
                el.child(Alert::error("hotkey-error", issue.message.clone()))
                    .child(div().id(issue.kind.marker_id()).test_support())
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("hotkey-change")
                            .primary()
                            .label(if capture_mode { "Listening…" } else { "Change" })
                            .disabled(capture_mode)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start_capture(window, cx)
                            })),
                    )
                    .when(capture_mode, |el| {
                        el.child(
                            Button::new("hotkey-cancel")
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_capture(cx))),
                        )
                    })
                    .child(
                        Button::new("hotkey-save")
                            .primary()
                            .label("Save")
                            .disabled(!has_pending)
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    //! The story's I/O & Edge-Case Matrix, driven through real clicks and
    //! keystrokes against a fake `HotkeyPort` and a fake `SettingsStore` —
    //! no X11 server, no `/dev/input`, no `settings.toml`.

    use std::sync::{Arc, Mutex};

    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{AppContext as _, TestAppContext, component::Root, px, size};
    use voice_me_core::{AppEventSender, AppState, SettingsStore, VoiceMeError};

    use super::*;

    #[derive(Default)]
    struct FakeHotkeyPort {
        /// Every combination `rebind` was asked for, in call order.
        rebinds: Mutex<Vec<String>>,
        /// Returned by the next `rebind` call instead of `Ok(())`.
        failure: Mutex<Option<VoiceMeError>>,
    }

    impl FakeHotkeyPort {
        fn failing(error: VoiceMeError) -> Self {
            Self {
                rebinds: Mutex::new(Vec::new()),
                failure: Mutex::new(Some(error)),
            }
        }

        fn rebinds(&self) -> Vec<String> {
            self.rebinds.lock().unwrap().clone()
        }
    }

    impl HotkeyPort for FakeHotkeyPort {
        fn start_listening(
            &self,
            hotkey: &str,
            _events: AppEventSender,
        ) -> Result<(), VoiceMeError> {
            self.rebind(hotkey)
        }

        fn rebind(&self, hotkey: &str) -> Result<(), VoiceMeError> {
            if let Some(error) = self.failure.lock().unwrap().take() {
                return Err(error);
            }
            self.rebinds.lock().unwrap().push(hotkey.to_string());
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeSettingsStore {
        saved_hotkeys: Mutex<Vec<Option<String>>>,
        save_fails: Mutex<bool>,
    }

    impl FakeSettingsStore {
        fn failing() -> Self {
            Self {
                saved_hotkeys: Mutex::new(Vec::new()),
                save_fails: Mutex::new(true),
            }
        }

        fn saved_hotkeys(&self) -> Vec<Option<String>> {
            self.saved_hotkeys.lock().unwrap().clone()
        }
    }

    impl SettingsStore for FakeSettingsStore {
        fn save_backend_selection(
            &self,
            _selection: &voice_me_core::BackendSelection,
        ) -> Result<AppState, VoiceMeError> {
            unimplemented!("not exercised by these tests")
        }

        fn save_local_runtimes(
            &self,
            _runtimes: &[voice_me_core::LocalRuntime],
        ) -> Result<AppState, VoiceMeError> {
            unimplemented!("not exercised by these tests")
        }

        fn save_api_key(
            &self,
            _provider: voice_me_core::RemoteProvider,
            _key: Option<&str>,
        ) -> Result<AppState, VoiceMeError> {
            unimplemented!("not exercised by these tests")
        }

        fn load(&self) -> Result<AppState, VoiceMeError> {
            Ok(AppState::default())
        }

        fn save_reference_voice_sample(&self, _wav: &[u8]) -> Result<AppState, VoiceMeError> {
            Ok(AppState::default())
        }

        fn save_selected_mic_device(
            &self,
            _device: Option<&str>,
        ) -> Result<AppState, VoiceMeError> {
            Ok(AppState::default())
        }

        fn save_hotkey(&self, hotkey: Option<&str>) -> Result<AppState, VoiceMeError> {
            if *self.save_fails.lock().unwrap() {
                return Err(VoiceMeError::Other("fake hotkey save failure".to_string()));
            }
            self.saved_hotkeys
                .lock()
                .unwrap()
                .push(hotkey.map(str::to_string));
            Ok(AppState::default())
        }
    }

    struct Harness {
        settings_store: Arc<FakeSettingsStore>,
        hotkey_port: Arc<FakeHotkeyPort>,
        handle: gpui_kit::WindowHandle<Root>,
    }

    fn open(
        cx: &mut TestAppContext,
        settings_store: Arc<FakeSettingsStore>,
        hotkey_port: Arc<FakeHotkeyPort>,
        saved_hotkey: Option<&str>,
        startup_error: Option<String>,
    ) -> Harness {
        cx.update(gpui_kit::init);
        let store_dyn: Arc<dyn SettingsStore> = settings_store.clone();
        let port_dyn: Arc<dyn HotkeyPort> = hotkey_port.clone();
        let saved = saved_hotkey.map(str::to_string);
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                HotkeyView::new(
                    store_dyn.clone(),
                    port_dyn.clone(),
                    saved.clone(),
                    startup_error.clone(),
                    cx,
                )
            });
            Root::new(view, window, cx)
        });
        Harness {
            settings_store,
            hotkey_port,
            handle,
        }
    }

    #[gpui_kit::test]
    fn capturing_a_combination_previews_it_on_a_chip_and_enables_save(cx: &mut TestAppContext) {
        let harness = open(
            cx,
            Arc::new(FakeSettingsStore::default()),
            Arc::new(FakeHotkeyPort::default()),
            None,
            None,
        );

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("hotkey-chip").is_none(),
                "nothing is bound yet, so there is no combination to show"
            );
            assert!(window.try_find("hotkey-chip-empty").is_some());
            // Save is disabled with nothing captured, and GPUI's real click
            // dispatch refuses disabled controls — clicking it must not
            // reach either collaborator (asserted after this block).
            window.click("hotkey-save", cx);

            window.click("hotkey-change", cx);
            window.render_frame(cx);
            assert!(window.try_find("hotkey-capture-active").is_some());

            window.press("ctrl-alt-v", cx);
            window.render_frame(cx);

            assert!(
                window.try_find("hotkey-capture-active").is_none(),
                "a complete combination ends capture"
            );
            assert!(window.try_find("hotkey-chip").is_some());
            assert!(
                window.try_find("hotkey-chip-empty").is_none(),
                "the empty-state label gives way to the captured combination"
            );
        })
        .unwrap();

        assert!(
            harness.hotkey_port.rebinds().is_empty(),
            "capturing alone must not touch the OS binding"
        );
        assert!(harness.settings_store.saved_hotkeys().is_empty());
    }

    #[gpui_kit::test]
    fn a_modifier_on_its_own_does_not_capture(cx: &mut TestAppContext) {
        let harness = open(
            cx,
            Arc::new(FakeSettingsStore::default()),
            Arc::new(FakeHotkeyPort::default()),
            None,
            None,
        );

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("hotkey-change", cx);
            window.press("control_l", cx);
            window.render_frame(cx);

            assert!(
                window.try_find("hotkey-capture-active").is_some(),
                "capture stays open, still waiting for a real combination"
            );
            assert!(
                window.try_find("hotkey-error-modifier-only").is_some(),
                "and says what's missing"
            );
            assert!(window.try_find("hotkey-chip").is_none());

            // The combination can still be completed from there.
            window.press("ctrl-alt-v", cx);
            window.render_frame(cx);
            assert!(window.try_find("hotkey-chip").is_some());
            assert!(window.try_find("hotkey-error-modifier-only").is_none());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn escape_cancels_capture_and_leaves_the_saved_binding_alone(cx: &mut TestAppContext) {
        let harness = open(
            cx,
            Arc::new(FakeSettingsStore::default()),
            Arc::new(FakeHotkeyPort::default()),
            Some("Ctrl+Alt+KeyV"),
            None,
        );

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("hotkey-change", cx);
            window.press("ctrl-alt-b", cx);
            window.render_frame(cx);

            // Re-entering capture discards the pending combination, and
            // Escape then leaves capture with nothing pending at all.
            window.click("hotkey-change", cx);
            window.press("escape", cx);
            window.render_frame(cx);

            assert!(window.try_find("hotkey-capture-active").is_none());
            assert!(
                window.try_find("hotkey-chip").is_some(),
                "the previously saved combination is still what's shown"
            );
            // Nothing is pending, so Save is disabled and this click is
            // refused by GPUI's dispatch (asserted after this block).
            window.click("hotkey-save", cx);
        })
        .unwrap();

        assert!(
            harness.hotkey_port.rebinds().is_empty(),
            "a cancelled capture must never re-bind anything"
        );
        assert!(harness.settings_store.saved_hotkeys().is_empty());
    }

    #[gpui_kit::test]
    fn an_unsaved_capture_never_reaches_the_os_or_the_settings_file(cx: &mut TestAppContext) {
        let harness = open(
            cx,
            Arc::new(FakeSettingsStore::default()),
            Arc::new(FakeHotkeyPort::default()),
            Some("Ctrl+Alt+KeyV"),
            None,
        );

        // Capture a new combination and then simply walk away (the window
        // closing without Save).
        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("hotkey-change", cx);
            window.press("ctrl-alt-b", cx);
            window.render_frame(cx);
        })
        .unwrap();

        assert!(harness.hotkey_port.rebinds().is_empty());
        assert!(harness.settings_store.saved_hotkeys().is_empty());
    }

    #[gpui_kit::test]
    fn saving_binds_then_persists_the_captured_combination(cx: &mut TestAppContext) {
        let harness = open(
            cx,
            Arc::new(FakeSettingsStore::default()),
            Arc::new(FakeHotkeyPort::default()),
            Some("Ctrl+Alt+KeyV"),
            None,
        );

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("hotkey-change", cx);
            window.press("ctrl-alt-b", cx);
            window.render_frame(cx);
            window.click("hotkey-save", cx);
            window.render_frame(cx);

            assert!(window.try_find("hotkey-error-conflict").is_none());
            // The pending combination is consumed, so Save is disabled
            // again and this second click is refused.
            window.click("hotkey-save", cx);
        })
        .unwrap();

        assert_eq!(
            harness.hotkey_port.rebinds(),
            vec!["Ctrl+Alt+KeyB"],
            "exactly one binding change, from the single effective Save"
        );
        assert_eq!(
            harness.settings_store.saved_hotkeys(),
            vec![Some("Ctrl+Alt+KeyB".to_string())]
        );
    }

    #[gpui_kit::test]
    fn a_conflicting_combination_is_rejected_before_it_is_persisted(cx: &mut TestAppContext) {
        let harness = open(
            cx,
            Arc::new(FakeSettingsStore::default()),
            Arc::new(FakeHotkeyPort::failing(VoiceMeError::HotkeyAlreadyInUse)),
            Some("Ctrl+Alt+KeyV"),
            None,
        );

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("hotkey-change", cx);
            window.press("ctrl-alt-t", cx);
            window.render_frame(cx);
            window.click("hotkey-save", cx);
            window.render_frame(cx);

            assert!(
                window.try_find("hotkey-error-conflict").is_some(),
                "the conflict is stated inline"
            );
            assert_eq!(
                window.find("hotkey-save").disabled(),
                None,
                "the combination stays pending so another one can be tried"
            );
        })
        .unwrap();

        assert!(
            harness.settings_store.saved_hotkeys().is_empty(),
            "a rejected combination must never reach settings.toml"
        );
    }

    #[gpui_kit::test]
    fn a_permission_failure_is_stated_in_words(cx: &mut TestAppContext) {
        let harness = open(
            cx,
            Arc::new(FakeSettingsStore::default()),
            Arc::new(FakeHotkeyPort::failing(
                VoiceMeError::InputDevicePermissionDenied,
            )),
            None,
            None,
        );

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("hotkey-change", cx);
            window.press("ctrl-alt-v", cx);
            window.render_frame(cx);
            window.click("hotkey-save", cx);
            window.render_frame(cx);

            assert!(window.try_find("hotkey-error-permission").is_some());
        })
        .unwrap();

        assert!(harness.settings_store.saved_hotkeys().is_empty());
    }

    #[gpui_kit::test]
    fn a_startup_permission_failure_is_shown_when_the_tab_opens(cx: &mut TestAppContext) {
        let harness = open(
            cx,
            Arc::new(FakeSettingsStore::default()),
            Arc::new(FakeHotkeyPort::default()),
            Some("Ctrl+Alt+KeyV"),
            Some(VoiceMeError::InputDevicePermissionDenied.to_string()),
        );

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("hotkey-error-permission").is_some(),
                "the app started fine, but the tab must say why the hotkey is inert"
            );
            assert!(
                window.try_find("hotkey-chip").is_some(),
                "the saved combination is still what's configured"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn a_failed_persist_restores_the_previously_bound_combination(cx: &mut TestAppContext) {
        let harness = open(
            cx,
            Arc::new(FakeSettingsStore::failing()),
            Arc::new(FakeHotkeyPort::default()),
            Some("Ctrl+Alt+KeyV"),
            None,
        );

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("hotkey-change", cx);
            window.press("ctrl-alt-b", cx);
            window.render_frame(cx);
            window.click("hotkey-save", cx);
            window.render_frame(cx);

            assert!(window.try_find("hotkey-error-other").is_some());
        })
        .unwrap();

        assert_eq!(
            harness.hotkey_port.rebinds(),
            vec!["Ctrl+Alt+KeyB", "Ctrl+Alt+KeyV"],
            "the new binding is rolled back to the saved one when the file write fails"
        );
        assert!(harness.settings_store.saved_hotkeys().is_empty());
    }

    #[test]
    fn capture_keystroke_covers_the_capture_matrix() {
        let mut ctrl_alt = Modifiers::none();
        ctrl_alt.control = true;
        ctrl_alt.alt = true;

        assert_eq!(
            capture_keystroke("v", ctrl_alt),
            CaptureOutcome::Captured("Ctrl+Alt+KeyV".to_string())
        );
        assert_eq!(
            capture_keystroke("escape", Modifiers::none()),
            CaptureOutcome::Cancelled
        );
        assert_eq!(
            capture_keystroke("control_l", Modifiers::none()),
            CaptureOutcome::ModifierOnly
        );
        assert_eq!(
            capture_keystroke("f5", Modifiers::none()),
            CaptureOutcome::Captured("F5".to_string())
        );
        assert_eq!(
            capture_keystroke("audiolowervolume", Modifiers::none()),
            CaptureOutcome::UnsupportedKey
        );
    }

    #[test]
    fn display_form_is_derived_not_persisted() {
        assert_eq!(display_hotkey("Ctrl+Alt+KeyV"), "Ctrl+Alt+V");
        assert_eq!(display_hotkey("shift+super+Digit1"), "Shift+Super+1");
        // `Meta` is shown as `Super`, the platform-native name on Linux.
        assert_eq!(display_hotkey("meta+F5"), "Super+F5");
        // Canonical modifier order regardless of how it was written.
        assert_eq!(display_hotkey("alt+ctrl+KeyB"), "Ctrl+Alt+B");
    }
}
