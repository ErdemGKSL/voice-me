//! The Settings window shell (Story 2.3): a tab bar over the individual
//! settings sections.
//!
//! Story 2.2 deliberately opened `VoiceSetupView` directly, with no shell
//! around it. Epics 3 and 4 both add sections, so the shell arrives here
//! rather than as a later rewrite — this view owns nothing but which tab is
//! showing; each section keeps its own state in its own entity.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window, div,
};
use voice_me_core::{
    AppEventSender, DependencyKind, DependencyOutcome, DependencyProvisioningPort, HotkeyPort,
    SettingsStore,
};

use crate::backend::{BackendActions, BackendPanel, BackendView};
use crate::dependencies::{DependenciesView, OpenBackendTab, RowProvisioning};
use crate::hotkey::HotkeyView;
use crate::voice_setup::VoiceSetupView;

const VOICE_TAB: usize = 0;
const HOTKEY_TAB: usize = 1;
const BACKEND_TAB: usize = 2;
const DEPENDENCIES_TAB: usize = 3;

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
}

/// The tabbed Settings window.
pub struct SettingsView {
    voice: Entity<VoiceSetupView>,
    hotkey: Entity<HotkeyView>,
    backend: Entity<BackendView>,
    dependencies: Entity<DependenciesView>,
    active_tab: usize,
    _subscriptions: Vec<Subscription>,
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
                cx,
            )
        });

        let backend = cx.new(|cx| {
            BackendView::new(
                dependencies.backend.clone(),
                dependencies.actions.clone(),
                window,
                cx,
            )
        });

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
        // The capability row's "Open Backend tab".
        let open_backend = cx.subscribe(&dependencies, |this, _, _: &OpenBackendTab, cx| {
            this.active_tab = BACKEND_TAB;
            cx.notify();
        });

        Self {
            voice,
            hotkey,
            backend,
            dependencies,
            active_tab: VOICE_TAB,
            _subscriptions: vec![open_backend],
        }
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
        self.backend
            .update(cx, |view, cx| view.set_backend_panel(panel.clone(), cx));
        self.dependencies
            .update(cx, |view, cx| view.set_backend_panel(panel, cx));
    }

    /// Push a fresh Dependency Check outcome into the Dependencies tab.
    pub fn set_dependency_outcome(&mut self, outcome: DependencyOutcome, cx: &mut Context<Self>) {
        self.dependencies
            .update(cx, |view, cx| view.set_outcome(outcome, cx));
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
                    .child(Tab::new().label("Backend"))
                    .child(Tab::new().label("Dependencies"))
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
                        BACKEND_TAB => el.child(self.backend.clone()),
                        DEPENDENCIES_TAB => el.child(self.dependencies.clone()),
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

        fn save_api_key(
            &self,
            _provider: voice_me_core::RemoteProvider,
            _key: Option<&str>,
        ) -> Result<AppState, VoiceMeError> {
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
            _backend: voice_me_core::SpeechBackend,
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
                    DependenciesTab {
                        deps_port: Arc::new(StubDepsPort),
                        events: event_tx.clone(),
                        backend: BackendPanel::default(),
                        actions: std::rc::Rc::new(|_, _| {}),
                        outcome: DependencyOutcome::Pending,
                        provisioning: HashMap::new(),
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

            window.click(BACKEND_TAB, cx);
            window.render_frame(cx);
            assert!(
                window.try_find("backend-surface").is_some(),
                "the third tab must route to the Backend section"
            );
            assert!(window.try_find("dependencies-surface").is_none());

            window.click(DEPENDENCIES_TAB, cx);
            window.render_frame(cx);
            assert!(
                window.try_find("dependencies-surface").is_some(),
                "the fourth tab must route to the Dependencies section"
            );
            assert!(window.try_find("backend-surface").is_none());
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
                    DependenciesTab {
                        deps_port: Arc::new(StubDepsPort),
                        events: event_tx.clone(),
                        backend: BackendPanel::default(),
                        actions: std::rc::Rc::new(|_, _| {}),
                        outcome: DependencyOutcome::Pending,
                        provisioning: HashMap::new(),
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
        let handle = cx.open_window(size(px(900.), px(900.)), |window, cx| {
            let view = cx.new(|cx| {
                SettingsView::new(
                    settings_store.clone(),
                    hotkey_port.clone(),
                    true,
                    None,
                    None,
                    None,
                    DependenciesTab {
                        deps_port: Arc::new(StubDepsPort),
                        events: event_tx.clone(),
                        backend,
                        actions: std::rc::Rc::new(|_, _| {}),
                        outcome,
                        provisioning: HashMap::new(),
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

    /// Story 3.10: the capability row's "Open Backend tab" switches the
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

            window.click(BACKEND_TAB, cx);
            window.render_frame(cx);
            assert!(
                window.try_find("api-key-save-deepinfra").is_some(),
                "the Backend tab shows the new selection's options"
            );
        })
        .unwrap();
    }
}
