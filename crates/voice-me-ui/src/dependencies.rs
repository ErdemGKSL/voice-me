//! Settings → Dependencies (Stories 3.1/3.2): the one place the user learns
//! what the app needs, what it cannot find, and fixes it.
//!
//! The view owns no detection of its own. It renders the latest
//! [`DependencyReport`] the composition root handed it, and "Check again"
//! asks `DependencyProvisioningPort` for a fresh one — which comes back the
//! same way every other report does, on the `AppEvent` channel (AD-3), and
//! is pushed in through [`DependenciesView::set_outcome`]. There is one
//! report in the process and the overlay gate reads the same one.
//!
//! Story 3.2 adds the fixing half. A missing row the app can fix gets an
//! Install button that runs `provision` on Tokio's blocking pool (AD-5);
//! its progress and its end arrive back on the `AppEvent` channel and are
//! pushed in through [`DependenciesView::set_provisioning`]. Install
//! progress lives here, beside the report, not in it: the report keeps
//! saying "missing" until the re-run check says otherwise, so the overlay
//! gate never believes a half-installed row. A row the app cannot fix gets
//! a "Show steps" toggle with short inline steps instead — never a link.
//!
//! Status is stated in words. The `missing` next to a row is text, never a
//! colour with a meaning attached to it.
//!
//! Story 3.3 added the backend's capability row: when the selected backend
//! can't run here the row says so in words, and its action is **Use CPU
//! backend**. Story 3.10 moved everything else about the backend — the
//! `Select`, the runtimes, the keys, the Selected/Active lines — to its own
//! tab ([`crate::backend::BackendView`]); the capability row links there
//! with **Open Backend tab**. The view still receives the same
//! [`BackendPanel`] for what "Check again" and Install ask about and for the
//! capability row's error, and still sends [`BackendAction::UseCpu`] to the
//! composition root rather than touching `SettingsStore`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _,
    alert::Alert,
    button::{Button, ButtonVariants as _},
    h_flex,
    progress::Progress,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _,
    TestSupportExt as _, Window, div, px,
};
use voice_me_core::{
    AppEvent, AppEventSender, Dependency, DependencyKind, DependencyOutcome,
    DependencyProvisioningPort, format_bytes,
    tokio_bridge::{self, TokioRuntime},
};

use crate::backend::{BackendAction, BackendActions, BackendArea, BackendPanel, error_line};

/// Emitted when the capability row's **Open Backend tab** is clicked; the
/// Settings shell switches to the Backend tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenBackendTab;

/// Where one row's Install stands, beside what the report says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowProvisioning {
    /// Running. `total == 0` means there is no byte count to show (the
    /// Virtual Microphone, or the moment between the click and the first
    /// progress event).
    Installing { done: u64, total: u64 },
    /// The last Install on this row failed, and this is the sentence
    /// saying which file and why.
    Failed(String),
}

impl RowProvisioning {
    fn is_installing(&self) -> bool {
        matches!(self, RowProvisioning::Installing { .. })
    }
}

/// The Dependencies tab.
pub struct DependenciesView {
    deps: Arc<dyn DependencyProvisioningPort>,
    events: AppEventSender,
    outcome: DependencyOutcome,
    provisioning: HashMap<DependencyKind, RowProvisioning>,
    /// Manual rows whose steps are expanded.
    steps_shown: HashSet<DependencyKind>,
    /// Kept for what "Check again" and Install ask about
    /// (`check_request`) and for the capability row's error.
    panel: BackendPanel,
    actions: BackendActions,
}

impl EventEmitter<OpenBackendTab> for DependenciesView {}

impl DependenciesView {
    pub fn new(
        deps: Arc<dyn DependencyProvisioningPort>,
        events: AppEventSender,
        panel: BackendPanel,
        outcome: DependencyOutcome,
        actions: BackendActions,
    ) -> Self {
        Self {
            deps,
            events,
            outcome,
            provisioning: HashMap::new(),
            steps_shown: HashSet::new(),
            panel,
            actions,
        }
    }

    /// Replace the backend state the tab reads. Called by the composition
    /// root, through the Settings shell, after every backend change.
    pub fn set_backend_panel(&mut self, panel: BackendPanel, cx: &mut Context<Self>) {
        self.panel = panel;
        cx.notify();
    }

    fn act(&mut self, action: BackendAction, cx: &mut Context<Self>) {
        (self.actions.clone())(action, cx);
    }

    /// Start with what the composition root already knows about installs
    /// in flight — a Settings window reopened mid-download has to show the
    /// row still installing, not offer a second Install.
    pub fn with_provisioning(
        mut self,
        provisioning: HashMap<DependencyKind, RowProvisioning>,
    ) -> Self {
        self.provisioning = provisioning;
        self
    }

    /// Replace what the tab shows. Called by the composition root when a
    /// `DependencyCheckCompleted` arrives — including the one the button
    /// below asked for, which is why the rows re-render without the user
    /// leaving the window.
    ///
    /// A row the new report calls ready has nothing left to install, so
    /// any failure still shown against it goes.
    pub fn set_outcome(&mut self, outcome: DependencyOutcome, cx: &mut Context<Self>) {
        if let DependencyOutcome::Ready(report) = &outcome {
            for dependency in &report.dependencies {
                if !dependency.status.is_missing() {
                    self.provisioning.remove(&dependency.kind);
                }
            }
        }
        self.outcome = outcome;
        cx.notify();
    }

    /// Update one row's install state; `None` clears it. Called by the
    /// composition root for every `ProvisioningProgress` and
    /// `ProvisioningFinished`.
    pub fn set_provisioning(
        &mut self,
        kind: DependencyKind,
        state: Option<RowProvisioning>,
        cx: &mut Context<Self>,
    ) {
        match state {
            Some(state) => self.provisioning.insert(kind, state),
            None => self.provisioning.remove(&kind),
        };
        cx.notify();
    }

    /// Replace every row's install state with the composition root's copy.
    ///
    /// Sent alongside each new report: the root is the one place that saw
    /// every `ProvisioningFinished`, so after a check lands its map — not
    /// whatever this view last guessed — is what the rows show.
    pub fn replace_provisioning(
        &mut self,
        provisioning: HashMap<DependencyKind, RowProvisioning>,
        cx: &mut Context<Self>,
    ) {
        self.provisioning = provisioning;
        cx.notify();
    }

    /// Run the check again, off the UI thread.
    ///
    /// Not a synchronous call, even though it is only a handful of `stat`s
    /// and one audio-server query: that query connects to PipeWire, and an
    /// unresponsive audio server would freeze the Settings window on the
    /// click. The startup check runs on the same executor for the same
    /// reason.
    ///
    /// A check that *ran* reports itself by event, and reaches this view
    /// through [`Self::set_outcome`]. Only a check that could not run at
    /// all is handled here.
    fn check_again(&mut self, cx: &mut Context<Self>) {
        let deps = self.deps.clone();
        let events = self.events.clone();
        let request = self.panel.check_request.clone();
        let check = cx.background_spawn(async move { deps.check(request, events) });
        cx.spawn(async move |this, cx| {
            let Err(error) = check.await else { return };
            let _ = this.update(cx, |this, cx| {
                this.set_outcome(DependencyOutcome::Failed(error.to_string()), cx);
            });
        })
        .detach();
    }

    /// Install one row, on Tokio's blocking pool (AD-5).
    ///
    /// The row flips to "installing" immediately, which is also what
    /// disables its button: a second click on a row already installing
    /// never reaches the port. Progress and the end arrive by event; only
    /// a job that never reported (no runtime to run on, or a panic) is
    /// handled here.
    fn install(&mut self, kind: DependencyKind, cx: &mut Context<Self>) {
        if self
            .provisioning
            .get(&kind)
            .is_some_and(RowProvisioning::is_installing)
        {
            return;
        }

        // The composition root owns every row's install state; the view
        // tells it through the same channel the adapter reports on, so a
        // report landing before the adapter's first progress event cannot
        // wipe "installing", and a job that never reports cannot leave the
        // root believing it is still running.
        let Some(handle) = cx
            .try_global::<TokioRuntime>()
            .map(|runtime| runtime.handle().clone())
        else {
            let reason = "voice-me's background runtime did not start this session, so nothing \
                          can be installed until it is restarted."
                .to_string();
            let _ = self.events.unbounded_send(AppEvent::ProvisioningFinished {
                kind,
                result: Err(reason.clone()),
            });
            self.set_provisioning(kind, Some(RowProvisioning::Failed(reason)), cx);
            return;
        };
        let _ = self.events.unbounded_send(AppEvent::ProvisioningProgress {
            kind,
            done_bytes: 0,
            total_bytes: 0,
        });
        self.set_provisioning(
            kind,
            Some(RowProvisioning::Installing { done: 0, total: 0 }),
            cx,
        );

        let deps = self.deps.clone();
        let events = self.events.clone();
        let request = self.panel.check_request.clone();
        let work =
            tokio_bridge::spawn_blocking_on(&handle, move || deps.provision(kind, request, events));
        let events = self.events.clone();
        cx.spawn(async move |this, cx| {
            let Err(error) = work.await else { return };
            // A job that panicked never sent `ProvisioningFinished`; this
            // does, so the root stops holding the row as installing. When
            // the adapter did report its own failure this repeats the same
            // sentence, which the root handles idempotently.
            let reason = error.to_string();
            let _ = events.unbounded_send(AppEvent::ProvisioningFinished {
                kind,
                result: Err(reason.clone()),
            });
            let _ = this.update(cx, |this, cx| {
                this.set_provisioning(kind, Some(RowProvisioning::Failed(reason)), cx);
            });
        })
        .detach();
    }

    fn toggle_steps(&mut self, kind: DependencyKind, cx: &mut Context<Self>) {
        if !self.steps_shown.remove(&kind) {
            self.steps_shown.insert(kind);
        }
        cx.notify();
    }

    fn row(&self, dependency: &Dependency, cx: &mut Context<Self>) -> AnyElement {
        let kind = dependency.kind;
        let missing = dependency.status.is_missing();
        let provisioning = self.provisioning.get(&kind).cloned();
        let installing = match provisioning {
            Some(RowProvisioning::Installing { done, total }) => Some((done, total)),
            _ => None,
        };
        let failure = match provisioning {
            Some(RowProvisioning::Failed(reason)) => Some(reason),
            _ => None,
        };
        let steps_shown = self.steps_shown.contains(&kind);

        // The word, not a colour: "missing" and "installing" have to be
        // readable.
        let status_word = if installing.is_some() {
            "installing"
        } else {
            dependency.status.label()
        };

        v_flex()
            .id(row_marker(kind))
            .test_support()
            .gap_1()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(dependency.label.clone()),
                    )
                    .child(
                        div()
                            .id(status_marker(kind, missing, installing.is_some()))
                            .test_support()
                            .px_2()
                            .py(px(1.))
                            .rounded_md()
                            .bg(cx.theme().muted)
                            .text_size(px(12.))
                            .text_color(if missing && installing.is_none() {
                                cx.theme().danger
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(status_word),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child(dependency.detail.clone()),
            )
            .when_some(installing, |el, (done, total)| {
                let figure = if total > 0 {
                    format!("{} of {}", format_bytes(done), format_bytes(total))
                } else {
                    "Working…".to_string()
                };
                el.child(
                    v_flex()
                        .id(installing_marker(kind))
                        .test_support()
                        .gap_1()
                        .child(
                            Progress::new(progress_marker(kind))
                                .loading(total == 0)
                                .value(if total > 0 {
                                    (done as f64 / total as f64 * 100.) as f32
                                } else {
                                    0.
                                })
                                .accessibility_label(format!(
                                    "Installing {}: {figure}",
                                    dependency.label
                                )),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(cx.theme().muted_foreground)
                                .child(figure),
                        ),
                )
            })
            .when_some(failure.filter(|_| missing), |el, reason| {
                el.child(
                    div()
                        .id(failed_marker(kind))
                        .test_support()
                        .text_size(px(12.))
                        .text_color(cx.theme().danger)
                        .child(reason),
                )
            })
            .when(missing && dependency.automatable, |el| {
                el.child(
                    h_flex().child(
                        Button::new(install_marker(kind))
                            .primary()
                            .label("Install")
                            .disabled(installing.is_some())
                            .on_click(
                                cx.listener(move |this, _, _window, cx| this.install(kind, cx)),
                            ),
                    ),
                )
            })
            .when(missing && kind == DependencyKind::BackendCapability, |el| {
                el.child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("backend-use-cpu")
                                        .primary()
                                        .label("Use CPU backend")
                                        .on_click(cx.listener(|this, _, _window, cx| {
                                            this.act(BackendAction::UseCpu, cx)
                                        })),
                                )
                                .child(
                                    Button::new("backend-open-tab")
                                        .ghost()
                                        .label("Open Backend tab")
                                        .on_click(cx.listener(|_this, _, _window, cx| {
                                            cx.emit(OpenBackendTab)
                                        })),
                                ),
                        )
                        .when_some(
                            self.panel.errors.get(&BackendArea::Capability).cloned(),
                            |el, error| el.child(error_line("backend-error-capability", error, cx)),
                        ),
                )
            })
            .when(
                missing && !dependency.automatable && kind != DependencyKind::BackendCapability,
                |el| {
                    el.child(
                        v_flex()
                            .id(manual_marker(kind))
                            .test_support()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child("voice-me cannot fix this one for you."),
                            )
                            .child(
                                h_flex().child(
                                    Button::new(steps_toggle_marker(kind))
                                        .ghost()
                                        .label(if steps_shown {
                                            "Hide steps"
                                        } else {
                                            "Show steps"
                                        })
                                        .on_click(cx.listener(move |this, _, _window, cx| {
                                            this.toggle_steps(kind, cx)
                                        })),
                                ),
                            )
                            .when(steps_shown, |el| {
                                el.child(
                                    v_flex()
                                        .id(steps_marker(kind))
                                        .test_support()
                                        .gap_1()
                                        .pl_2()
                                        .text_size(px(12.))
                                        .children(dependency.manual_steps.iter().enumerate().map(
                                            |(index, step)| {
                                                div().child(format!("{}. {step}", index + 1))
                                            },
                                        )),
                                )
                            }),
                    )
                },
            )
            .into_any_element()
    }
}

impl Render for DependenciesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let outcome = self.outcome.clone();

        v_flex()
            .id("dependencies-surface")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .p_6()
            .gap_6()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(div().text_lg().child("Dependencies"))
            .child(
                v_flex()
                    .gap_4()
                    .map(|el| match &outcome {
                        DependencyOutcome::Pending => el.child(
                            div()
                                .id("dependencies-pending")
                                .test_support()
                                .text_color(cx.theme().muted_foreground)
                                .child("Checking…"),
                        ),
                        DependencyOutcome::Failed(reason) => el
                            .child(Alert::error(
                                "dependencies-check-failed-alert",
                                format!("The dependency check could not run: {reason}"),
                            ))
                            .child(div().id("dependencies-check-failed").test_support()),
                        DependencyOutcome::Ready(report) => {
                            // A plain loop rather than `map`: the rows borrow
                            // `cx` for the theme, which a closure cannot hand
                            // back out.
                            let mut rows = Vec::with_capacity(report.dependencies.len());
                            for dependency in &report.dependencies {
                                rows.push(self.row(dependency, cx));
                            }
                            el.child(v_flex().gap_4().children(rows))
                        }
                    })
                    .child(
                        h_flex().child(
                            Button::new("dependencies-check-again")
                                .primary()
                                .label("Check again")
                                .on_click(cx.listener(|this, _, _window, cx| this.check_again(cx))),
                        ),
                    ),
            )
    }
}

/// The sentence the Prompt Overlay shows when `dependency` is what stops
/// the Speak Action.
///
/// Lives here, beside the tab that shows the same row, so the overlay and
/// Settings never name the same blocker two different ways.
///
/// The row's own detail already says what is wrong ("Missing: <path>",
/// "Not found at <path>"), so this only puts the row's name in front of it
/// — restating "missing" here would make the notice say it twice.
pub fn blocker_notice(dependency: &Dependency) -> String {
    format!("{} — {}", dependency.label, dependency.detail)
}

/// The per-row slug every element id below is built from.
fn slug(kind: DependencyKind) -> &'static str {
    match kind {
        DependencyKind::OnnxRuntime => "onnx-runtime",
        DependencyKind::ModelWeights => "model-weights",
        DependencyKind::VirtualMicrophone => "virtual-microphone",
        DependencyKind::BackendCapability => "backend-capability",
        DependencyKind::SystemVoiceEngine => "system-voice-engine",
        DependencyKind::EdgeTtsProgram => "edge-tts-program",
        DependencyKind::PiperVoice => "piper-voice",
        DependencyKind::CudaProvider => "cuda-provider",
        DependencyKind::NvidiaLibraries => "nvidia-libraries",
    }
}

fn marker(prefix: &str, kind: DependencyKind) -> String {
    format!("{prefix}-{}", slug(kind))
}

fn row_marker(kind: DependencyKind) -> String {
    marker("dependency-row", kind)
}

fn status_marker(kind: DependencyKind, missing: bool, installing: bool) -> String {
    let state = match (installing, missing) {
        (true, _) => "installing",
        (false, true) => "missing",
        (false, false) => "ready",
    };
    marker(&format!("dependency-{state}"), kind)
}

fn manual_marker(kind: DependencyKind) -> String {
    marker("dependency-manual", kind)
}

fn install_marker(kind: DependencyKind) -> String {
    marker("dependency-install", kind)
}

fn installing_marker(kind: DependencyKind) -> String {
    marker("dependency-progress", kind)
}

fn progress_marker(kind: DependencyKind) -> String {
    marker("dependency-progress-bar", kind)
}

fn failed_marker(kind: DependencyKind) -> String {
    marker("dependency-failed", kind)
}

fn steps_toggle_marker(kind: DependencyKind) -> String {
    marker("dependency-steps-toggle", kind)
}

fn steps_marker(kind: DependencyKind) -> String {
    marker("dependency-steps", kind)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use std::path::PathBuf;
    use std::rc::Rc;

    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{TestAppContext, component::Root, px, size};
    use voice_me_core::{AppEvent, CheckRequest, LocalRuntime, VoiceMeError};

    use super::*;

    /// A handler that ignores every request.
    fn no_actions() -> BackendActions {
        Rc::new(|_, _| {})
    }

    /// Counts the checks and installs it was asked for, and can be told to
    /// fail the way an unresolvable cache root does.
    #[derive(Default)]
    struct CountingDepsPort {
        checks: AtomicUsize,
        provisions: AtomicUsize,
        fails_with: Option<String>,
        /// What the last check and install were asked about.
        last_request: std::sync::Mutex<Option<CheckRequest>>,
        last_provision: std::sync::Mutex<Option<CheckRequest>>,
    }

    impl DependencyProvisioningPort for CountingDepsPort {
        fn check(
            &self,
            request: CheckRequest,
            _events: AppEventSender,
        ) -> Result<(), VoiceMeError> {
            *self.last_request.lock().unwrap() = Some(request);
            self.checks.fetch_add(1, Ordering::SeqCst);
            match self.fails_with.as_ref() {
                Some(reason) => Err(VoiceMeError::Other(reason.clone())),
                None => Ok(()),
            }
        }

        fn provision(
            &self,
            _kind: DependencyKind,
            request: voice_me_core::CheckRequest,
            _events: AppEventSender,
        ) -> Result<(), VoiceMeError> {
            *self.last_provision.lock().unwrap() = Some(request);
            self.provisions.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// A report with one automatable missing row and one manual one.
    fn report_with_an_installable_and_a_manual_row() -> DependencyOutcome {
        DependencyOutcome::Ready(voice_me_core::DependencyReport::new(
            voice_me_core::SpeechBackend::CPU,
            vec![
                Dependency::missing(
                    DependencyKind::OnnxRuntime,
                    "ONNX Runtime",
                    "ORT_DYLIB_PATH is set to /gone, and nothing is there.",
                )
                .manual([
                    "Point ORT_DYLIB_PATH at a library.",
                    "Start voice-me again.",
                ]),
                Dependency::missing(
                    DependencyKind::ModelWeights,
                    "Speech model files (Q4)",
                    "Missing 9 of 9 files",
                ),
            ],
        ))
    }

    /// Opens the tab over `port` showing `outcome`, with the AD-5 runtime
    /// installed the way the composition root installs it.
    fn open_tab(
        cx: &mut TestAppContext,
        port: Arc<CountingDepsPort>,
        outcome: DependencyOutcome,
    ) -> (gpui_kit::WindowHandle<Root>, tokio::runtime::Runtime) {
        open_tab_with_panel(cx, port, outcome, BackendPanel::default())
    }

    fn open_tab_with_panel(
        cx: &mut TestAppContext,
        port: Arc<CountingDepsPort>,
        outcome: DependencyOutcome,
        panel: BackendPanel,
    ) -> (gpui_kit::WindowHandle<Root>, tokio::runtime::Runtime) {
        cx.update(gpui_kit::init);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let handle = runtime.handle().clone();
        cx.update(|cx| cx.set_global(TokioRuntime::from_handle(handle)));
        let (event_tx, _event_rx) = futures::channel::mpsc::unbounded::<AppEvent>();
        let deps: Arc<dyn DependencyProvisioningPort> = port;
        let window = cx.open_window(size(px(640.), px(640.)), |window, cx| {
            let view = cx.new(|_| {
                DependenciesView::new(deps.clone(), event_tx.clone(), panel, outcome, no_actions())
            });
            Root::new(view, window, cx)
        });
        (window, runtime)
    }

    /// Opens the tab over `port` and clicks "Check again".
    fn click_check_again(cx: &mut TestAppContext, port: Arc<CountingDepsPort>) -> bool {
        cx.update(gpui_kit::init);
        let (event_tx, _event_rx) = futures::channel::mpsc::unbounded::<AppEvent>();
        let deps: Arc<dyn DependencyProvisioningPort> = port.clone();

        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|_| {
                DependenciesView::new(
                    deps.clone(),
                    event_tx.clone(),
                    BackendPanel::default(),
                    DependencyOutcome::Pending,
                    no_actions(),
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("dependencies-check-again", cx);
        })
        .unwrap();
        // The check runs off the UI thread; let it land.
        cx.run_until_parked();

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.try_find("dependencies-check-failed").is_some()
        })
        .unwrap()
    }

    /// Without this, deleting the button's `on_click` leaves every other
    /// test green: nothing else exercises the asking half of the tab.
    #[gpui_kit::test]
    fn check_again_asks_the_port_for_a_fresh_report(cx: &mut TestAppContext) {
        let port = Arc::new(CountingDepsPort::default());

        let failed = click_check_again(cx, port.clone());

        assert_eq!(
            port.checks.load(Ordering::SeqCst),
            1,
            "the button's whole job is to ask for a new check"
        );
        assert!(
            !failed,
            "a check that ran reports by event — the tab keeps waiting, it does not claim failure"
        );
    }

    /// The other half: a check that could not run at all is the one result
    /// that never arrives by event, so the tab has to show it itself.
    #[gpui_kit::test]
    fn a_check_that_cannot_run_is_shown_in_the_tab(cx: &mut TestAppContext) {
        let port = Arc::new(CountingDepsPort {
            fails_with: Some("could not resolve a cache directory".to_string()),
            ..CountingDepsPort::default()
        });

        assert!(
            click_check_again(cx, port.clone()),
            "the reason has nowhere else to surface"
        );
        assert_eq!(port.checks.load(Ordering::SeqCst), 1);
    }

    /// A check that could not run at all has to *say so* in the tab —
    /// silently showing "Checking…" forever would hide the one thing the
    /// user needs to read.
    #[gpui_kit::test]
    fn a_check_that_could_not_run_says_so_with_its_reason(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::test::TestWindowExt as _;
        use gpui_kit::{AppContext as _, component::Root, px, size};
        use voice_me_core::VoiceMeError;

        struct FailingDeps;

        impl DependencyProvisioningPort for FailingDeps {
            fn check(
                &self,
                _request: CheckRequest,
                _events: AppEventSender,
            ) -> Result<(), VoiceMeError> {
                Err(VoiceMeError::Other(
                    "could not resolve a cache directory".to_string(),
                ))
            }

            fn provision(
                &self,
                _kind: DependencyKind,
                _request: voice_me_core::CheckRequest,
                _events: AppEventSender,
            ) -> Result<(), VoiceMeError> {
                Err(VoiceMeError::Other(
                    "could not resolve a cache directory".to_string(),
                ))
            }
        }

        cx.update(gpui_kit::init);
        let (event_tx, _event_rx) = futures::channel::mpsc::unbounded();
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|_| {
                DependenciesView::new(
                    Arc::new(FailingDeps),
                    event_tx.clone(),
                    BackendPanel::default(),
                    DependencyOutcome::Failed("could not resolve a cache directory".to_string()),
                    no_actions(),
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("dependencies-check-failed").is_some(),
                "the tab states that the check itself failed"
            );
            assert!(
                window.try_find("dependencies-pending").is_none(),
                "and stops claiming it is still checking"
            );
        })
        .unwrap();
    }

    #[test]
    fn the_blocker_notice_names_the_row_and_its_detail() {
        let notice = blocker_notice(&Dependency::missing(
            DependencyKind::ModelWeights,
            "Speech model files (Q4)",
            "Missing: /cache/onnx/language_model_q4.onnx",
        ));

        assert!(notice.contains("Speech model files (Q4)"), "{notice}");
        assert!(notice.contains("language_model_q4.onnx"), "{notice}");
        assert_eq!(
            notice.matches("Missing").count(),
            1,
            "the notice says what is wrong once, not once per half: {notice}"
        );
    }

    /// Install calls the port — once, however many times it is clicked
    /// while the row is installing — and the row says "installing".
    #[gpui_kit::test]
    fn clicking_install_calls_the_port_once(cx: &mut TestAppContext) {
        let port = Arc::new(CountingDepsPort::default());
        let (window, _runtime) = open_tab(
            cx,
            port.clone(),
            report_with_an_installable_and_a_manual_row(),
        );

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("dependency-install-model-weights", cx);
            window.render_frame(cx);
            assert!(
                window
                    .try_find("dependency-installing-model-weights")
                    .is_some(),
                "the row says installing, in words, straight away"
            );
            // A second click while installing: the row's own state is
            // what stops it reaching the port (asserted below).
            window.click("dependency-install-model-weights", cx);
        })
        .unwrap();

        // The job runs on Tokio's blocking pool, not GPUI's executor.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while port.provisions.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        cx.run_until_parked();

        assert_eq!(port.provisions.load(Ordering::SeqCst), 1);
    }

    /// A manual row has no Install; it has steps behind a toggle, inline.
    #[gpui_kit::test]
    fn a_manual_row_shows_steps_instead_of_install(cx: &mut TestAppContext) {
        let port = Arc::new(CountingDepsPort::default());
        let (window, _runtime) = open_tab(
            cx,
            port.clone(),
            report_with_an_installable_and_a_manual_row(),
        );

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("dependency-install-onnx-runtime").is_none());
            assert!(
                window.try_find("dependency-steps-onnx-runtime").is_none(),
                "collapsed until asked for"
            );

            window.click("dependency-steps-toggle-onnx-runtime", cx);
            window.render_frame(cx);

            assert!(window.try_find("dependency-steps-onnx-runtime").is_some());
        })
        .unwrap();
        assert_eq!(port.provisions.load(Ordering::SeqCst), 0);
    }

    /// Progress and failure pushed in by the composition root land on the
    /// right row, and a report that turns the row ready clears them.
    #[gpui_kit::test]
    fn progress_and_failure_are_shown_on_the_row(cx: &mut TestAppContext) {
        let port = Arc::new(CountingDepsPort::default());
        cx.update(gpui_kit::init);
        let (event_tx, _event_rx) = futures::channel::mpsc::unbounded::<AppEvent>();
        let deps: Arc<dyn DependencyProvisioningPort> = port;
        let mut view_slot = None;
        let window = cx.open_window(size(px(640.), px(640.)), |window, cx| {
            let view = cx.new(|_| {
                DependenciesView::new(
                    deps.clone(),
                    event_tx.clone(),
                    BackendPanel::default(),
                    report_with_an_installable_and_a_manual_row(),
                    no_actions(),
                )
            });
            view_slot = Some(view.clone());
            Root::new(view, window, cx)
        });
        let view = view_slot.unwrap();

        cx.update_window(window.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.set_provisioning(
                    DependencyKind::ModelWeights,
                    Some(RowProvisioning::Installing {
                        done: 412_000_000,
                        total: 1_555_000_000,
                    }),
                    cx,
                )
            });
            window.render_frame(cx);
            assert!(
                window
                    .try_find("dependency-progress-model-weights")
                    .is_some()
            );

            view.update(cx, |view, cx| {
                view.set_provisioning(
                    DependencyKind::ModelWeights,
                    Some(RowProvisioning::Failed(
                        "Download of speech_encoder.onnx_data failed: connection reset".into(),
                    )),
                    cx,
                )
            });
            window.render_frame(cx);
            assert!(
                window
                    .try_find("dependency-progress-model-weights")
                    .is_none()
            );
            assert!(window.try_find("dependency-failed-model-weights").is_some());
            assert!(
                window
                    .try_find("dependency-installing-model-weights")
                    .is_none(),
                "a failed row reads missing again, with Install offered for a retry"
            );
            assert!(
                window
                    .try_find("dependency-install-model-weights")
                    .is_some()
            );

            view.update(cx, |view, cx| {
                view.set_outcome(
                    DependencyOutcome::Ready(voice_me_core::DependencyReport::new(
                        voice_me_core::SpeechBackend::CPU,
                        vec![Dependency::ready(
                            DependencyKind::ModelWeights,
                            "Speech model files (Q4)",
                            "All 9 files present",
                        )],
                    )),
                    cx,
                )
            });
            window.render_frame(cx);
            assert!(window.try_find("dependency-failed-model-weights").is_none());
            assert!(
                window
                    .try_find("dependency-install-model-weights")
                    .is_none()
            );
            assert!(window.try_find("dependency-ready-model-weights").is_some());
        })
        .unwrap();
    }

    /// A window reopened mid-download is seeded with the root's map: the
    /// row renders installing, and clicking Install does not reach the
    /// port a second time.
    #[gpui_kit::test]
    fn a_seeded_install_in_flight_renders_installing_and_is_not_restarted(cx: &mut TestAppContext) {
        let port = Arc::new(CountingDepsPort::default());
        cx.update(gpui_kit::init);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let tokio_handle = runtime.handle().clone();
        cx.update(|cx| cx.set_global(TokioRuntime::from_handle(tokio_handle)));
        let (event_tx, _event_rx) = futures::channel::mpsc::unbounded::<AppEvent>();
        let deps: Arc<dyn DependencyProvisioningPort> = port.clone();
        let window = cx.open_window(size(px(640.), px(640.)), |window, cx| {
            let view = cx.new(|_| {
                DependenciesView::new(
                    deps.clone(),
                    event_tx.clone(),
                    BackendPanel::default(),
                    report_with_an_installable_and_a_manual_row(),
                    no_actions(),
                )
                .with_provisioning(HashMap::from([(
                    DependencyKind::ModelWeights,
                    RowProvisioning::Installing {
                        done: 300_000_000,
                        total: 1_555_000_000,
                    },
                )]))
            });
            Root::new(view, window, cx)
        });

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .try_find("dependency-installing-model-weights")
                    .is_some()
            );
            assert!(
                window
                    .try_find("dependency-progress-model-weights")
                    .is_some()
            );
            window.click("dependency-install-model-weights", cx);
        })
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        cx.run_until_parked();

        assert_eq!(port.provisions.load(Ordering::SeqCst), 0);
    }

    // ---- Stories 3.3/3.10: the capability row --------------------------------

    type Recorded = Rc<std::cell::RefCell<Vec<BackendAction>>>;

    fn recording_actions() -> (BackendActions, Recorded) {
        let recorded: Recorded = Rc::default();
        let sink = recorded.clone();
        (
            Rc::new(move |action, _cx| sink.borrow_mut().push(action)),
            recorded,
        )
    }

    fn cuda_panel() -> BackendPanel {
        let runtime = LocalRuntime {
            path: PathBuf::from("/opt/ort/libonnxruntime.so"),
            targets: vec![
                voice_me_core::SpeechExecutionTarget::Cpu,
                voice_me_core::SpeechExecutionTarget::Cuda,
            ],
        };
        BackendPanel {
            selection: runtime.entries()[0].clone(),
            runtimes: vec![runtime],
            ..BackendPanel::default()
        }
    }

    fn cuda_cannot_run() -> DependencyOutcome {
        let mut row = Dependency::missing(
            DependencyKind::BackendCapability,
            "Selected backend",
            "CUDA (libonnxruntime.so) can't run here: No NVIDIA driver found.",
        );
        row.automatable = false;
        DependencyOutcome::Ready(voice_me_core::DependencyReport::new(
            voice_me_core::SpeechBackend::for_target(voice_me_core::SpeechExecutionTarget::Cuda),
            vec![row],
        ))
    }

    /// Opens the tab with `panel` and `outcome`, recording every action.
    fn open_capability_tab(
        cx: &mut TestAppContext,
        panel: BackendPanel,
        outcome: DependencyOutcome,
    ) -> (
        gpui_kit::WindowHandle<Root>,
        gpui_kit::Entity<DependenciesView>,
        Recorded,
    ) {
        cx.update(gpui_kit::init);
        let (actions, recorded) = recording_actions();
        let (event_tx, _event_rx) = futures::channel::mpsc::unbounded::<AppEvent>();
        let deps: Arc<dyn DependencyProvisioningPort> = Arc::new(CountingDepsPort::default());
        let mut slot = None;
        let window = cx.open_window(size(px(900.), px(1400.)), |window, cx| {
            let view = cx.new(|_| {
                DependenciesView::new(deps.clone(), event_tx.clone(), panel, outcome, actions)
            });
            slot = Some(view.clone());
            Root::new(view, window, cx)
        });
        (window, slot.unwrap(), recorded)
    }

    /// The acceptance criterion's row: the reason in words, and one button
    /// whose click reaches the root exactly once.
    #[gpui_kit::test]
    fn a_capability_row_offers_use_cpu_and_asks_the_root_once(cx: &mut TestAppContext) {
        let (window, _view, recorded) = open_capability_tab(cx, cuda_panel(), cuda_cannot_run());

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .try_find("dependency-missing-backend-capability")
                    .is_some(),
                "the row says `missing`, in words"
            );
            assert!(
                window
                    .try_find("dependency-install-backend-capability")
                    .is_none(),
                "nothing to install: the way out is choosing CPU"
            );
            assert!(
                window
                    .try_find("dependency-steps-toggle-backend-capability")
                    .is_none()
            );
            window.click("backend-use-cpu", cx);
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(*recorded.borrow(), vec![BackendAction::UseCpu]);
    }

    /// Story 3.10: the capability row links to the Backend tab. The click
    /// emits one event for the Settings shell and asks the root nothing.
    #[gpui_kit::test]
    fn open_backend_tab_emits_once(cx: &mut TestAppContext) {
        let (window, view, recorded) = open_capability_tab(cx, cuda_panel(), cuda_cannot_run());
        let opened = Rc::new(std::cell::Cell::new(0));
        let counter = opened.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&view, move |_, _: &OpenBackendTab, _| {
                counter.set(counter.get() + 1)
            })
        });

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("backend-open-tab", cx);
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(opened.get(), 1);
        assert!(recorded.borrow().is_empty());
    }

    /// Story 3.10: the backend's own controls live on the Backend tab, not
    /// here.
    #[gpui_kit::test]
    fn the_dependencies_tab_shows_no_backend_controls(cx: &mut TestAppContext) {
        let (window, _view, _recorded) = open_capability_tab(cx, cuda_panel(), cuda_cannot_run());

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            for id in [
                "backend-select",
                "backend-device-select",
                "backend-runtimes",
                "backend-add-runtime",
                "backend-api-keys",
                "api-key-save-deepinfra",
                "api-key-plaintext-notice",
                "remote-sample-state-deepinfra",
                "backend-selected",
            ] {
                assert!(
                    window.try_find(id).is_none(),
                    "{id} belongs on the Backend tab"
                );
            }
            assert!(window.try_find("dependencies-check-again").is_some());
        })
        .unwrap();
    }

    /// "Check again" and Install ask about the *selected* backend — the
    /// panel's request — never a fixed CPU one, which would hide a
    /// capability blocker or fetch the wrong weight variant.
    #[gpui_kit::test]
    fn check_again_and_install_forward_the_selected_backend(cx: &mut TestAppContext) {
        let panel = cuda_panel();
        let request = CheckRequest {
            backend: voice_me_core::SpeechBackend::for_target(
                voice_me_core::SpeechExecutionTarget::Cuda,
            ),
            selection: panel.selection.clone(),
            has_api_key: false,
            has_region: false,
            has_voice: false,
            piper_voice: None,
        };
        let panel = BackendPanel {
            check_request: request.clone(),
            ..panel
        };
        let port = Arc::new(CountingDepsPort::default());
        let (window, _runtime) = open_tab_with_panel(
            cx,
            port.clone(),
            report_with_an_installable_and_a_manual_row(),
            panel,
        );

        // Install runs on Tokio's blocking pool: wait for it without
        // driving GPUI's scheduler, since its completion wakes the view from
        // a Tokio thread, which the test scheduler rejects while running.
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("dependency-install-model-weights", cx);
        })
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while port.provisions.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // The count rises inside `provision`; let it return and wake the
        // view before the scheduler runs.
        std::thread::sleep(std::time::Duration::from_millis(50));
        cx.run_until_parked();

        // "Check again" runs on GPUI's own background executor, which the
        // scheduler drives.
        cx.update_window(window.into(), |_, window, cx| {
            window.click("dependencies-check-again", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(port.checks.load(Ordering::SeqCst), 1);

        assert_eq!(*port.last_request.lock().unwrap(), Some(request.clone()));
        assert_eq!(*port.last_provision.lock().unwrap(), Some(request));
    }

    /// Install on the Piper voice row passes the selected voice through, so
    /// the adapter downloads that voice, not the default.
    #[gpui_kit::test]
    fn install_on_the_piper_voice_row_forwards_the_selected_voice(cx: &mut TestAppContext) {
        let request = CheckRequest {
            backend: voice_me_core::SpeechBackend::CPU,
            selection: voice_me_core::BackendSelection::PIPER_CPU,
            has_api_key: false,
            has_region: false,
            has_voice: false,
            piper_voice: Some("tr_TR-dfki-medium".to_string()),
        };
        let panel = BackendPanel {
            selection: voice_me_core::BackendSelection::PIPER_CPU,
            check_request: request.clone(),
            ..BackendPanel::default()
        };
        let outcome = DependencyOutcome::Ready(voice_me_core::DependencyReport::new(
            voice_me_core::SpeechBackend::CPU,
            vec![Dependency::missing(
                DependencyKind::PiperVoice,
                "Piper voice",
                "The selected voice tr_TR-dfki-medium is not installed.",
            )],
        ));
        let port = Arc::new(CountingDepsPort::default());
        let (window, _runtime) = open_tab_with_panel(cx, port.clone(), outcome, panel);

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("dependency-install-piper-voice", cx);
        })
        .unwrap();
        // The job runs on Tokio's blocking pool. Wait without driving
        // GPUI's scheduler: its completion wakes the view from a Tokio
        // thread, which the test scheduler rejects while it is running.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while port.provisions.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        cx.run_until_parked();

        let forwarded = port.last_provision.lock().unwrap().clone().unwrap();
        assert_eq!(forwarded.piper_voice.as_deref(), Some("tr_TR-dfki-medium"));
        assert_eq!(forwarded, request);
    }

    /// Story 3.17: the `edge-tts` row's slug, and what the overlay says.
    #[test]
    fn the_edge_tts_row_has_its_own_slug_and_notice() {
        assert_eq!(slug(DependencyKind::EdgeTtsProgram), "edge-tts-program");
        assert_eq!(
            row_marker(DependencyKind::EdgeTtsProgram),
            "dependency-row-edge-tts-program"
        );
        let row = Dependency::missing(
            DependencyKind::EdgeTtsProgram,
            "edge-tts",
            "edge-tts is not installed. Please install it: pipx install edge-tts",
        )
        .manual(["Or: pip install --user edge-tts", "Press Check again."]);
        assert_eq!(
            blocker_notice(&row),
            "edge-tts — edge-tts is not installed. Please install it: pipx install edge-tts"
        );
    }
}
