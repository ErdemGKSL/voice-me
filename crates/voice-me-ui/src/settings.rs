//! The Settings window shell (Story 2.3): a tab bar over the individual
//! settings sections.
//!
//! Story 2.2 deliberately opened `VoiceSetupView` directly, with no shell
//! around it. Epics 3 and 4 both add sections, so the shell arrives here
//! rather than as a later rewrite — this view owns nothing but which tab is
//! showing; each section keeps its own state in its own entity.

use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Window,
    div,
};
use voice_me_core::{HotkeyPort, SettingsStore};

use crate::hotkey::HotkeyView;
use crate::voice_setup::VoiceSetupView;

const VOICE_TAB: usize = 0;
const HOTKEY_TAB: usize = 1;

/// The tabbed Settings window.
pub struct SettingsView {
    voice: Entity<VoiceSetupView>,
    hotkey: Entity<HotkeyView>,
    active_tab: usize,
}

impl SettingsView {
    /// `has_active_sample`/`selected_mic_device` are the Voice tab's startup
    /// state (Story 1.5); `saved_hotkey`/`hotkey_startup_error` are the
    /// Hotkey tab's. All of them come from the composition root's single
    /// `settings_store.load()` — no view reads the store at render time.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        settings_store: Arc<dyn SettingsStore>,
        hotkey_port: Arc<dyn HotkeyPort>,
        has_active_sample: bool,
        selected_mic_device: Option<String>,
        saved_hotkey: Option<String>,
        hotkey_startup_error: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let voice = cx.new(|cx| {
            VoiceSetupView::new(
                settings_store.clone(),
                has_active_sample,
                selected_mic_device,
                window,
                cx,
            )
        });
        let hotkey = cx.new(|cx| {
            HotkeyView::new(
                settings_store,
                hotkey_port,
                saved_hotkey,
                hotkey_startup_error,
                cx,
            )
        });

        Self {
            voice,
            hotkey,
            active_tab: VOICE_TAB,
        }
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                TabBar::new("settings-tabs")
                    .selected_index(self.active_tab)
                    .child(Tab::new().label("Voice"))
                    .child(Tab::new().label("Hotkey"))
                    .on_click(cx.listener(|this, index: &usize, _window, cx| {
                        this.active_tab = *index;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .map(|el| match self.active_tab {
                        HOTKEY_TAB => el.child(self.hotkey.clone()),
                        _ => el.child(self.voice.clone()),
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    //! Tab routing is the only path to the Hotkey tab in the shipped app,
    //! so it is exercised through real clicks: each section is identified by
    //! a control only that section renders.

    use std::sync::Arc;

    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{TestAppContext, component::Root, px, size};
    use voice_me_core::{AppEventSender, AppState, VoiceMeError};

    use super::*;

    struct StubSettingsStore;

    impl SettingsStore for StubSettingsStore {
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

        fn save_hotkey(&self, _hotkey: Option<&str>) -> Result<AppState, VoiceMeError> {
            Ok(AppState::default())
        }
    }

    struct StubHotkeyPort;

    impl HotkeyPort for StubHotkeyPort {
        fn start_listening(
            &self,
            _hotkey: &str,
            _events: AppEventSender,
        ) -> Result<(), VoiceMeError> {
            Ok(())
        }

        fn rebind(&self, _hotkey: &str) -> Result<(), VoiceMeError> {
            Ok(())
        }
    }

    #[gpui_kit::test]
    fn the_voice_section_opens_first_and_the_hotkey_tab_switches_to_it(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let settings_store: Arc<dyn SettingsStore> = Arc::new(StubSettingsStore);
        let hotkey_port: Arc<dyn HotkeyPort> = Arc::new(StubHotkeyPort);
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                SettingsView::new(
                    settings_store.clone(),
                    hotkey_port.clone(),
                    true,
                    None,
                    None,
                    None,
                    window,
                    cx,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("voice-setup-record").is_some(),
                "the Voice section is what the window opens on"
            );
            assert!(window.try_find("hotkey-change").is_none());

            // `TabBar` identifies each tab by its index.
            window.click(HOTKEY_TAB, cx);
            window.render_frame(cx);

            assert!(
                window.try_find("hotkey-change").is_some(),
                "the second tab must route to the Hotkey section"
            );
            assert!(window.try_find("voice-setup-record").is_none());

            window.click(VOICE_TAB, cx);
            window.render_frame(cx);
            assert!(window.try_find("voice-setup-record").is_some());
        })
        .unwrap();
    }
}
