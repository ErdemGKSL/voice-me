//! The Settings window shell: a sidebar beside the individual
//! settings sections.
//!
//! Story 2.2 deliberately opened `VoiceSetupView` directly, with no shell
//! around it. Epics 3 and 4 both add sections, so the shell arrives here
//! rather than as a later rewrite — this view owns which section is
//! showing; each section keeps its own state in its own entity.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, KeyDownEvent, ParentElement as _, Render, Styled as _, Subscription,
    TestSupportExt as _, Window, WindowOptions, div, px, size,
};
use voice_me_core::{
    AppEventSender, BackendSelection, DependencyKind, DependencyOutcome,
    DependencyProvisioningPort, HotkeyPort, SettingsStore,
};

use crate::backend::{BackendActions, BackendPanel, BackendView, OpenPiperVoicesTab};
use crate::dependencies::{DependenciesView, OpenBackendTab, RowProvisioning};
use crate::hotkey::{HotkeyDesktop, HotkeyView};
use crate::piper_voices::{PiperVoicesActions, PiperVoicesPanel, PiperVoicesView};
use crate::voice_setup::VoiceSetupView;

const VOICE_TAB: usize = 0;
const HOTKEY_TAB: usize = 1;
const BACKEND_TAB: usize = 2;
const DEPENDENCIES_TAB: usize = 3;
const PIPER_VOICES_TAB: usize = 4;

/// The Piper voices tab's startup state and where its requests go (Story
/// 3.15).
#[derive(Clone)]
pub struct PiperVoicesTab {
    pub panel: PiperVoicesPanel,
    pub actions: PiperVoicesActions,
}

impl Default for PiperVoicesTab {
    fn default() -> Self {
        Self {
            panel: PiperVoicesPanel::default(),
            actions: std::rc::Rc::new(|_, _| {}),
        }
    }
}

/// Everything the Backend and Dependencies tabs need, as one argument.
///
/// Both tabs are fed the same [`BackendPanel`] and ask through the same
/// [`BackendActions`] (Story 3.10): the Backend tab shows and changes the
/// selection, and the Dependencies tab reads `check_request` and the
/// capability row's error from it.
///
/// Grouped rather than spread across four more parameters: `SettingsView`
/// already takes the startup state of two other tabs, and a twelve-argument
/// constructor is a place for two of them to be swapped by accident.
pub struct DependenciesTab {
    pub deps_port: Arc<dyn DependencyProvisioningPort>,
    pub events: AppEventSender,
    /// The backend's state (Story 3.3), shown by both tabs.
    pub backend: BackendPanel,
    /// Where both tabs' backend requests go — the composition root.
    pub actions: BackendActions,
    pub outcome: DependencyOutcome,
    /// Installs already running or already failed when the window opens
    /// (Story 3.2), so a reopened window never offers a second Install on
    /// a row that is still downloading.
    pub provisioning: HashMap<DependencyKind, RowProvisioning>,
    /// Story 3.15: the Piper voices tab.
    pub piper: PiperVoicesTab,
}

/// The Settings window with contextual sidebar navigation.
pub struct SettingsView {
    voice: Entity<VoiceSetupView>,
    hotkey: Entity<HotkeyView>,
    backend: Entity<BackendView>,
    dependencies: Entity<DependenciesView>,
    piper_voices: Entity<PiperVoicesView>,
    active_tab: usize,
    selection: BackendSelection,
    speech_recorder_open: bool,
    focus_handle: FocusHandle,
    /// What the title bar's own close button does (Linux only; Windows and
    /// macOS close through the platform, which runs the window's
    /// should-close handler itself).
    on_close: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    /// The window options the branded title bar needs: the platform
    /// title bar gives way to it, so dragging its background moves the
    /// window and it draws the window controls where the OS expects them.
    ///
    /// The minimum leaves useful width for form controls beside the sidebar.
    pub fn window_options() -> WindowOptions {
        WindowOptions {
            window_min_size: Some(size(px(720.), px(480.))),
            ..TitleBar::window_options()
        }
    }

    /// What the desktop adds to the Hotkey tab — the GNOME/KDE settings
    /// button, or the compositor steps (spec-native-gnome-kde-hotkey).
    pub fn set_hotkey_desktop(&mut self, desktop: HotkeyDesktop, cx: &mut Context<Self>) {
        self.hotkey
            .update(cx, |hotkey, cx| hotkey.set_desktop(desktop, cx));
    }

    /// What the title bar's close button runs on Linux, where the button is
    /// ours and closing through it skips the window's should-close handler.
    /// It has to leave the composition root in the same state that handler
    /// does, and then remove the window.
    pub fn set_on_close(&mut self, on_close: impl Fn(&mut Window, &mut App) + 'static) {
        self.on_close = Some(Rc::new(on_close));
    }

    /// `has_active_sample`/`selected_mic_device` are the Voice tab's startup
    /// state (Story 1.5); `saved_hotkey`/`hotkey_startup_error` are the
    /// Hotkey tab's, with `overlay_position` (the saved
    /// `AppState.overlay_position`) and `wayland` (whether this is a Wayland
    /// session). All of them come from the composition root's single
    /// `settings_store.load()` — no view reads the store at render time.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        settings_store: Arc<dyn SettingsStore>,
        hotkey_port: Arc<dyn HotkeyPort>,
        has_active_sample: bool,
        selected_mic_device: Option<String>,
        saved_hotkey: Option<String>,
        hotkey_startup_error: Option<String>,
        overlay_position: u8,
        wayland: bool,
        dependencies: DependenciesTab,
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
                overlay_position,
                wayland,
                window,
                cx,
            )
        });

        let selection = dependencies.backend.selection.clone();
        let backend = cx.new(|cx| {
            BackendView::new(
                dependencies.backend.clone(),
                dependencies.actions.clone(),
                window,
                cx,
            )
        });

        let piper = dependencies.piper.clone();
        let piper_voices =
            cx.new(|cx| PiperVoicesView::new(piper.panel, piper.actions, window, cx));

        let dependencies = cx.new(|_| {
            DependenciesView::new(
                dependencies.deps_port,
                dependencies.events,
                dependencies.backend,
                dependencies.outcome,
                dependencies.actions,
            )
            .with_provisioning(dependencies.provisioning)
        });
        // The capability row's "Open Speech".
        let open_backend = cx.subscribe(&dependencies, |this, _, _: &OpenBackendTab, cx| {
            this.active_tab = BACKEND_TAB;
            cx.notify();
        });
        // Story 3.15: the Backend tab's "Manage voices".
        let open_piper_voices = cx.subscribe(&backend, |this, _, _: &OpenPiperVoicesTab, cx| {
            this.show_piper_voices(cx);
        });

        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        Self {
            voice,
            hotkey,
            backend,
            dependencies,
            piper_voices,
            active_tab: if selection.is_chatterbox() {
                VOICE_TAB
            } else {
                BACKEND_TAB
            },
            selection,
            speech_recorder_open: false,
            focus_handle,
            on_close: None,
            _subscriptions: vec![open_backend, open_piper_voices],
        }
    }

    /// Show the Piper voices tab; the catalogs are fetched the first time.
    pub fn show_piper_voices(&mut self, cx: &mut Context<Self>) {
        self.active_tab = PIPER_VOICES_TAB;
        self.piper_voices.update(cx, |view, cx| view.opened(cx));
        cx.notify();
    }

    /// Push the Piper voices tab's new state.
    pub fn set_piper_voices_panel(&mut self, panel: PiperVoicesPanel, cx: &mut Context<Self>) {
        self.piper_voices
            .update(cx, |view, cx| view.set_panel(panel, cx));
    }

    /// Open the window on the Dependencies tab.
    ///
    /// Story 3.4 needs *exactly* this tab: a blocked overlay that sent the
    /// user to Settings and left them on Voice would have told them nothing.
    pub fn show_dependencies(&mut self, cx: &mut Context<Self>) {
        self.active_tab = DEPENDENCIES_TAB;
        cx.notify();
    }

    /// Push one row's install state into the Dependencies tab.
    pub fn set_provisioning(
        &mut self,
        kind: DependencyKind,
        state: Option<RowProvisioning>,
        cx: &mut Context<Self>,
    ) {
        self.dependencies
            .update(cx, |view, cx| view.set_provisioning(kind, state, cx));
    }

    /// Replace every row's install state in the Dependencies tab.
    pub fn replace_provisioning(
        &mut self,
        provisioning: HashMap<DependencyKind, RowProvisioning>,
        cx: &mut Context<Self>,
    ) {
        self.dependencies
            .update(cx, |view, cx| view.replace_provisioning(provisioning, cx));
    }

    /// Push the backend's new state into both the Backend tab and the
    /// Dependencies tab, which still reads `check_request` and the
    /// capability row's error from it.
    pub fn set_backend_panel(&mut self, panel: BackendPanel, cx: &mut Context<Self>) {
        let selection = panel.selection.clone();
        if self.active_tab == VOICE_TAB && !selection.is_chatterbox() {
            self.active_tab = BACKEND_TAB;
        }
        if !selection.is_piper() && self.active_tab == PIPER_VOICES_TAB {
            self.active_tab = BACKEND_TAB;
        }
        if self.selection != selection {
            self.speech_recorder_open = false;
        }
        self.selection = selection;
        self.backend
            .update(cx, |view, cx| view.set_backend_panel(panel.clone(), cx));
        self.dependencies
            .update(cx, |view, cx| view.set_backend_panel(panel, cx));
        cx.notify();
    }

    /// Push a fresh Dependency Check outcome into the Dependencies tab.
    pub fn set_dependency_outcome(&mut self, outcome: DependencyOutcome, cx: &mut Context<Self>) {
        self.dependencies
            .update(cx, |view, cx| view.set_outcome(outcome, cx));
    }
}

impl Focusable for SettingsView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let on_close = self.on_close.clone();
        let cloning_remote = matches!(
            self.selection,
            BackendSelection::Remote(provider) if !provider.is_stock_voice()
        );
        let nav = |index, label: &'static str, cx: &mut Context<Self>| {
            let button = Button::new(format!("settings-nav-{index}"))
                .small()
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.active_tab = index;
                    cx.notify();
                }));
            if self.active_tab == index
                || (index == BACKEND_TAB && self.active_tab == PIPER_VOICES_TAB)
            {
                button.outline()
            } else {
                button.ghost()
            }
        };
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if !event.keystroke.modifiers.control {
                    return;
                }
                let tab = match event.keystroke.key.as_str() {
                    "1" if this.selection.is_chatterbox() => VOICE_TAB,
                    "2" => BACKEND_TAB,
                    "3" => HOTKEY_TAB,
                    "4" => DEPENDENCIES_TAB,
                    _ => return,
                };
                this.active_tab = tab;
                cx.notify();
                cx.stop_propagation();
            }))
            .bg(cx.theme().background)
            .child(
                TitleBar::new()
                    .when_some(on_close, |bar, on_close| {
                        bar.on_close_window(move |_, window, cx| on_close(window, cx))
                    })
                    .child(
                        div()
                            .id("settings-branded-title")
                            .test_support()
                            // A narrow absolute title stays centered in the
                            // whole window without covering native controls.
                            // TitleBar begins its content 12px from the edge.
                            .absolute()
                            .left(window.bounds().size.width / 2. - px(122.))
                            .w(px(220.))
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .child(
                                div().id("settings-waveform-mark").test_support().child(
                                    Icon::default()
                                        .data(include_bytes!(
                                            "../../../assets/voice-me-waveform.svg"
                                        ))
                                        .text_color(cx.theme().primary),
                                ),
                            )
                            .child("Voice Me"),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(
                        v_flex()
                            .id("settings-sidebar")
                            .test_support()
                            .w(px(168.))
                            .h_full()
                            .p_3()
                            .gap_1()
                            .bg(cx.theme().sidebar)
                            .border_r_1()
                            .border_color(cx.theme().border)
                            .when(self.selection.is_chatterbox(), |el| {
                                el.child(nav(VOICE_TAB, "Voice", cx))
                            })
                            .child(nav(BACKEND_TAB, "Speech", cx))
                            .child(nav(HOTKEY_TAB, "Hotkey", cx))
                            .child(nav(DEPENDENCIES_TAB, "System", cx)),
                    )
                    .child(
                        div()
                            .id("settings-content")
                            .test_support()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .overflow_hidden()
                            .map(|el| match self.active_tab {
                                HOTKEY_TAB => el.child(self.hotkey.clone()),
                                BACKEND_TAB if cloning_remote => el.child(
                                    v_flex()
                                        .size_full()
                                        .child(
                                            h_flex().p_2().bg(cx.theme().background).child(
                                                Button::new("settings-speech-recorder")
                                                    .small()
                                                    .outline()
                                                    .label(if self.speech_recorder_open {
                                                        "Back to speech settings"
                                                    } else {
                                                        "Record reference sample"
                                                    })
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.speech_recorder_open =
                                                            !this.speech_recorder_open;
                                                        cx.notify();
                                                    })),
                                            ),
                                        )
                                        .child(div().flex_1().min_h_0().overflow_hidden().child(
                                            if self.speech_recorder_open {
                                                self.voice.clone().into_any_element()
                                            } else {
                                                self.backend.clone().into_any_element()
                                            },
                                        )),
                                ),
                                BACKEND_TAB => el.child(self.backend.clone()),
                                DEPENDENCIES_TAB => el.child(self.dependencies.clone()),
                                PIPER_VOICES_TAB => el.child(self.piper_voices.clone()),
                                _ => el.child(self.voice.clone()),
                            }),
                    ),
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
    use voice_me_core::{AppEvent, AppEventSender, AppState, VoiceMeError};

    use super::*;

    struct StubSettingsStore;

    impl SettingsStore for StubSettingsStore {
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

        fn save_speech_language(
            &self,
            _backend: voice_me_core::LanguageBackend,
            _code: &str,
        ) -> Result<AppState, VoiceMeError> {
            unimplemented!("not exercised by these tests")
        }

        fn save_speech_voice(
            &self,
            _backend: voice_me_core::LanguageBackend,
            _voice: Option<&str>,
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

        fn save_azure_region(&self, _region: Option<&str>) -> Result<AppState, VoiceMeError> {
            unimplemented!("not exercised by these tests")
        }

        fn save_overlay_position(&self, _percent: u8) -> Result<AppState, VoiceMeError> {
            unimplemented!("not exercised by these tests")
        }

        fn save_disclosure_confirmed(
            &self,
            _provider: voice_me_core::RemoteProvider,
        ) -> Result<AppState, VoiceMeError> {
            unimplemented!("not exercised by these tests")
        }

        fn save_remote_sample(
            &self,
            _provider: voice_me_core::RemoteProvider,
            _sample: Option<voice_me_core::RemoteSample>,
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

        fn save_hotkey(&self, _hotkey: Option<&str>) -> Result<AppState, VoiceMeError> {
            Ok(AppState::default())
        }
    }

    struct StubDepsPort;

    impl DependencyProvisioningPort for StubDepsPort {
        fn check(
            &self,
            _request: voice_me_core::CheckRequest,
            _events: AppEventSender,
        ) -> Result<(), VoiceMeError> {
            Ok(())
        }

        fn provision(
            &self,
            _kind: DependencyKind,
            _request: voice_me_core::CheckRequest,
            _events: AppEventSender,
        ) -> Result<(), VoiceMeError> {
            Ok(())
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
    fn the_sidebar_and_branded_title_fit_the_window(cx: &mut TestAppContext) {
        let (handle, _view) =
            open_settings(cx, BackendPanel::default(), DependencyOutcome::Pending);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let title = window.find("settings-branded-title").bounds();
            assert!(
                (title.center().x - window.bounds().center().x).abs() <= px(1.),
                "title={title:?} window={:?}",
                window.bounds()
            );
            if let Some(controls) = window.try_find("window-controls").map(|item| item.bounds()) {
                assert!(title.origin.x + title.size.width <= controls.origin.x);
            }
            assert!(window.try_find("settings-waveform-mark").is_some());
            assert!(window.try_find("settings-sidebar").is_some());
            assert!(window.find("settings-content").bounds().size.width >= px(480.));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn sidebar_is_keyboard_usable_at_minimum_width(cx: &mut TestAppContext) {
        let (handle, _view) =
            open_settings(cx, BackendPanel::default(), DependencyOutcome::Pending);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.bounds().size.width, px(720.));
            window.activate_window();
            window.press("tab", cx);
            assert!(window.focused(cx).is_some());
            window.press("tab", cx);
            window.render_frame(cx);
            assert!(window.focused(cx).is_some());
            window.press("tab", cx);
            window.press("ctrl-2", cx);
            window.render_frame(cx);
            assert!(window.try_find("backend-surface").is_some());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn the_voice_section_opens_first_and_the_hotkey_tab_switches_to_it(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let settings_store: Arc<dyn SettingsStore> = Arc::new(StubSettingsStore);
        let hotkey_port: Arc<dyn HotkeyPort> = Arc::new(StubHotkeyPort);
        let (event_tx, _event_rx) = futures::channel::mpsc::unbounded::<AppEvent>();
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                SettingsView::new(
                    settings_store.clone(),
                    hotkey_port.clone(),
                    true,
                    None,
                    None,
                    None,
                    50,
                    false,
                    DependenciesTab {
                        deps_port: Arc::new(StubDepsPort),
                        events: event_tx.clone(),
                        backend: BackendPanel::default(),
                        actions: std::rc::Rc::new(|_, _| {}),
                        outcome: DependencyOutcome::Pending,
                        provisioning: HashMap::new(),
                        piper: PiperVoicesTab::default(),
                    },
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
            window.click("settings-nav-1", cx);
            window.render_frame(cx);

            assert!(
                window.try_find("hotkey-change").is_some(),
                "the second tab must route to the Hotkey section"
            );
            assert!(window.try_find("voice-setup-record").is_none());

            window.click("settings-nav-0", cx);
            window.render_frame(cx);
            assert!(window.try_find("voice-setup-record").is_some());

            window.click("settings-nav-2", cx);
            window.render_frame(cx);
            assert!(
                window.try_find("backend-surface").is_some(),
                "the third tab must route to the Backend section"
            );
            assert!(window.try_find("dependencies-surface").is_none());

            window.click("settings-nav-3", cx);
            window.render_frame(cx);
            assert!(
                window.try_find("dependencies-surface").is_some(),
                "the fourth tab must route to the Dependencies section"
            );
            assert!(window.try_find("backend-surface").is_none());

            assert!(
                window.try_find("settings-nav-4").is_none(),
                "Piper management is contextual"
            );
        })
        .unwrap();
    }

    /// Story 3.15: "Manage voices" under a Piper selection on the Backend
    /// tab switches Settings to the Piper voices tab.
    #[gpui_kit::test]
    fn manage_voices_switches_to_the_piper_voices_tab(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let settings_store: Arc<dyn SettingsStore> = Arc::new(StubSettingsStore);
        let hotkey_port: Arc<dyn HotkeyPort> = Arc::new(StubHotkeyPort);
        let (event_tx, _event_rx) = futures::channel::mpsc::unbounded::<AppEvent>();
        let handle = cx.open_window(size(px(640.), px(640.)), |window, cx| {
            let view = cx.new(|cx| {
                SettingsView::new(
                    settings_store.clone(),
                    hotkey_port.clone(),
                    true,
                    None,
                    None,
                    None,
                    50,
                    false,
                    DependenciesTab {
                        deps_port: Arc::new(StubDepsPort),
                        events: event_tx.clone(),
                        backend: BackendPanel {
                            selection: voice_me_core::BackendSelection::PIPER_CPU,
                            ..BackendPanel::default()
                        },
                        actions: std::rc::Rc::new(|_, _| {}),
                        outcome: DependencyOutcome::Pending,
                        provisioning: HashMap::new(),
                        piper: PiperVoicesTab::default(),
                    },
                    window,
                    cx,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("settings-nav-2", cx);
            window.render_frame(cx);
            assert!(window.try_find("settings-nav-4").is_none());
            assert!(window.try_find("piper-voices-surface").is_none());

            window.click("backend-manage-piper-voices", cx);
        })
        .unwrap();
        // The tab switch arrives as an event, delivered once the click's
        // update has finished.
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("piper-voices-surface").is_some(),
                "Manage voices must open the Piper voices tab"
            );
            assert!(window.try_find("backend-surface").is_none());
            assert!(window.try_find("settings-nav-4").is_none());
        })
        .unwrap();
    }

    /// Story 3.4 sends the user here from a blocked overlay, so "open
    /// Settings" has to mean this tab specifically — landing on Voice would
    /// have told them nothing about what is missing.
    #[gpui_kit::test]
    fn settings_can_be_opened_focused_on_the_dependencies_tab(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let settings_store: Arc<dyn SettingsStore> = Arc::new(StubSettingsStore);
        let hotkey_port: Arc<dyn HotkeyPort> = Arc::new(StubHotkeyPort);
        let (event_tx, _event_rx) = futures::channel::mpsc::unbounded::<AppEvent>();
        let view_slot: Arc<std::sync::Mutex<Option<Entity<SettingsView>>>> =
            Arc::new(std::sync::Mutex::new(None));

        let slot = view_slot.clone();
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                SettingsView::new(
                    settings_store.clone(),
                    hotkey_port.clone(),
                    true,
                    None,
                    None,
                    None,
                    50,
                    false,
                    DependenciesTab {
                        deps_port: Arc::new(StubDepsPort),
                        events: event_tx.clone(),
                        backend: BackendPanel::default(),
                        actions: std::rc::Rc::new(|_, _| {}),
                        outcome: DependencyOutcome::Pending,
                        provisioning: HashMap::new(),
                        piper: PiperVoicesTab::default(),
                    },
                    window,
                    cx,
                )
            });
            *slot.lock().unwrap() = Some(view.clone());
            Root::new(view, window, cx)
        });
        let view = view_slot.lock().unwrap().clone().unwrap();

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("dependencies-surface").is_none());

            view.update(cx, |view, cx| view.show_dependencies(cx));
            window.render_frame(cx);
            assert!(
                window.try_find("dependencies-surface").is_some(),
                "opening Settings on the Dependencies tab is what Story 3.4 needs"
            );

            // And a report arriving afterwards re-renders the rows without
            // the user leaving the window.
            view.update(cx, |view, cx| {
                view.set_dependency_outcome(
                    DependencyOutcome::Ready(voice_me_core::DependencyReport::new(
                        voice_me_core::SpeechBackend::CPU,
                        vec![voice_me_core::Dependency::missing(
                            voice_me_core::DependencyKind::ModelWeights,
                            "Speech model files (Q4)",
                            "Missing: /cache/onnx/language_model_q4.onnx",
                        )],
                    )),
                    cx,
                )
            });
            window.render_frame(cx);
            assert!(
                window
                    .try_find("dependency-missing-model-weights")
                    .is_some(),
                "the row says the word `missing`, in text"
            );
            assert!(window.try_find("dependencies-pending").is_none());
        })
        .unwrap();
    }

    /// Opens Settings over `backend` and `outcome`, returning the shell.
    fn open_settings(
        cx: &mut TestAppContext,
        backend: BackendPanel,
        outcome: DependencyOutcome,
    ) -> (gpui_kit::WindowHandle<Root>, Entity<SettingsView>) {
        cx.update(gpui_kit::init);
        let settings_store: Arc<dyn SettingsStore> = Arc::new(StubSettingsStore);
        let hotkey_port: Arc<dyn HotkeyPort> = Arc::new(StubHotkeyPort);
        let (event_tx, _event_rx) = futures::channel::mpsc::unbounded::<AppEvent>();
        let mut slot = None;
        let handle = cx.open_window(size(px(720.), px(900.)), |window, cx| {
            let view = cx.new(|cx| {
                SettingsView::new(
                    settings_store.clone(),
                    hotkey_port.clone(),
                    true,
                    None,
                    None,
                    None,
                    50,
                    false,
                    DependenciesTab {
                        deps_port: Arc::new(StubDepsPort),
                        events: event_tx.clone(),
                        backend,
                        actions: std::rc::Rc::new(|_, _| {}),
                        outcome,
                        provisioning: HashMap::new(),
                        piper: PiperVoicesTab::default(),
                    },
                    window,
                    cx,
                )
            });
            slot = Some(view.clone());
            Root::new(view, window, cx)
        });
        (handle, slot.unwrap())
    }

    #[gpui_kit::test]
    fn cloning_remote_opens_speech_with_recorder_access(cx: &mut TestAppContext) {
        let (handle, view) = open_settings(
            cx,
            BackendPanel {
                selection: BackendSelection::Remote(voice_me_core::RemoteProvider::DeepInfra),
                ..BackendPanel::default()
            },
            DependencyOutcome::Pending,
        );
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("settings-nav-0").is_none());
            assert!(window.try_find("backend-surface").is_some());
            window.click("settings-speech-recorder", cx);
            window.render_frame(cx);
            assert!(window.try_find("voice-setup-record").is_some());
            window.click("settings-nav-3", cx);
            window.click("settings-nav-2", cx);
            view.update(cx, |view, cx| {
                view.set_backend_panel(
                    BackendPanel {
                        selection: BackendSelection::Remote(
                            voice_me_core::RemoteProvider::DeepInfra,
                        ),
                        ..BackendPanel::default()
                    },
                    cx,
                );
            });
            window.render_frame(cx);
            assert!(window.try_find("voice-setup-record").is_some());
            view.update(cx, |view, cx| {
                view.set_backend_panel(
                    BackendPanel {
                        selection: BackendSelection::PIPER_CPU,
                        ..BackendPanel::default()
                    },
                    cx,
                );
            });
            window.render_frame(cx);
            assert!(window.try_find("settings-speech-recorder").is_none());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn switching_away_from_chatterbox_moves_voice_to_speech(cx: &mut TestAppContext) {
        let (handle, view) = open_settings(cx, BackendPanel::default(), DependencyOutcome::Pending);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("settings-nav-0").is_some());
            view.update(cx, |view, cx| {
                view.set_backend_panel(
                    BackendPanel {
                        selection: BackendSelection::PIPER_CPU,
                        ..BackendPanel::default()
                    },
                    cx,
                )
            });
            window.render_frame(cx);
            assert!(window.try_find("settings-nav-0").is_none());
            assert!(window.try_find("backend-surface").is_some());
            assert!(window.try_find("settings-nav-4").is_none());
        })
        .unwrap();
    }

    /// Story 3.10: the capability row's "Open Speech" switches the
    /// shell to the Backend tab.
    #[gpui_kit::test]
    fn the_capability_link_switches_to_the_backend_tab(cx: &mut TestAppContext) {
        let mut row = voice_me_core::Dependency::missing(
            DependencyKind::BackendCapability,
            "Selected backend",
            "CUDA can't run here: No NVIDIA driver found.",
        );
        row.automatable = false;
        let outcome = DependencyOutcome::Ready(voice_me_core::DependencyReport::new(
            voice_me_core::SpeechBackend::CPU,
            vec![row],
        ));
        let (handle, view) = open_settings(cx, BackendPanel::default(), outcome);

        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.show_dependencies(cx));
            window.render_frame(cx);
            assert!(window.try_find("dependencies-surface").is_some());
            window.click("backend-open-tab", cx);
        })
        .unwrap();
        // The tab switch is a subscription, delivered once the click's
        // update has finished.
        cx.run_until_parked();

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("backend-surface").is_some(),
                "the link lands on the Backend tab"
            );
            assert!(window.try_find("dependencies-surface").is_none());
        })
        .unwrap();
    }

    /// A new panel reaches both tabs: the Backend tab shows the new
    /// selection, and Dependencies reads the capability error from it.
    #[gpui_kit::test]
    fn a_backend_panel_reaches_both_tabs(cx: &mut TestAppContext) {
        let mut row = voice_me_core::Dependency::missing(
            DependencyKind::BackendCapability,
            "Selected backend",
            "CUDA can't run here: No NVIDIA driver found.",
        );
        row.automatable = false;
        let outcome = DependencyOutcome::Ready(voice_me_core::DependencyReport::new(
            voice_me_core::SpeechBackend::CPU,
            vec![row],
        ));
        let (handle, view) = open_settings(cx, BackendPanel::default(), outcome);

        cx.update_window(handle.into(), |_, window, cx| {
            let mut panel = BackendPanel {
                selection: voice_me_core::BackendSelection::Remote(
                    voice_me_core::RemoteProvider::DeepInfra,
                ),
                ..BackendPanel::default()
            };
            panel.errors.insert(
                crate::backend::BackendArea::Capability,
                "Could not save.".to_string(),
            );
            view.update(cx, |view, cx| view.set_backend_panel(panel, cx));

            view.update(cx, |view, cx| view.show_dependencies(cx));
            window.render_frame(cx);
            assert!(window.try_find("backend-error-capability").is_some());

            window.click("settings-nav-2", cx);
            window.render_frame(cx);
            assert!(
                window.try_find("api-key-save-deepinfra").is_some(),
                "the Backend tab shows the new selection's options"
            );
        })
        .unwrap();
    }
}
