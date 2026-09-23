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
//! Story 3.3 adds the backend section above the rows: a `Select` of every
//! backend (the bundled CPU runtime, each runtime the user added, the
//! remote providers), the *Selected* and *Active* lines kept as two
//! separate facts, the runtimes the user added, and the API keys. The view
//! never touches `SettingsStore`: every change goes to the composition root
//! as a [`BackendAction`], and the root pushes the result back in as a
//! [`BackendPanel`]. A capability row's action is **Use CPU backend**.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IndexPath, Sizable as _,
    alert::Alert,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    progress::Progress,
    select::{Select, SelectEvent, SelectState},
    tag::Tag,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, TestSupportExt as _, Window, div,
    px,
};
use voice_me_core::{
    ActiveBackend, ApiKeys, AppEvent, AppEventSender, BackendSelection, CheckRequest, Dependency,
    DependencyKind, DependencyOutcome, DependencyProvisioningPort, LocalRuntime, RemoteProvider,
    RemoteSample, backend_choices, format_bytes,
    tokio_bridge::{self, TokioRuntime},
};

/// Something the user asked of the backend section. The view only *asks*;
/// the composition root does it (through `SettingsStore`, the probe helper,
/// or a relaunch) and pushes the outcome back as a [`BackendPanel`].
#[derive(Clone, PartialEq, Eq)]
pub enum BackendAction {
    /// Make this the selected backend.
    Select(BackendSelection),
    /// Probe the ONNX Runtime library at this path and, if it is one, add
    /// its backends.
    AddRuntime(PathBuf),
    /// Forget an added runtime library.
    RemoveRuntime(PathBuf),
    /// Save (`Some`) or remove (`None`) a provider's API key.
    SaveApiKey(RemoteProvider, Option<String>),
    /// Select the bundled CPU backend — a capability row's one action.
    UseCpu,
    /// Relaunch voice-me so a selection that needs another runtime library
    /// takes effect.
    Restart,
    /// Delete the Reference Voice Sample this provider holds (Story 3.6).
    DeleteRemoteSample(RemoteProvider),
}

/// Written by hand so a key typed into the API keys section can never reach
/// a log line through `{:?}`.
impl std::fmt::Debug for BackendAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendAction::Select(selection) => f.debug_tuple("Select").field(selection).finish(),
            BackendAction::AddRuntime(path) => f.debug_tuple("AddRuntime").field(path).finish(),
            BackendAction::RemoveRuntime(path) => {
                f.debug_tuple("RemoveRuntime").field(path).finish()
            }
            BackendAction::SaveApiKey(provider, key) => f
                .debug_tuple("SaveApiKey")
                .field(provider)
                .field(&key.as_ref().map(|_| "<redacted>"))
                .finish(),
            BackendAction::UseCpu => f.write_str("UseCpu"),
            BackendAction::Restart => f.write_str("Restart"),
            BackendAction::DeleteRemoteSample(provider) => {
                f.debug_tuple("DeleteRemoteSample").field(provider).finish()
            }
        }
    }
}

/// Where the root sends each [`BackendAction`].
pub type BackendActions = Rc<dyn Fn(BackendAction, &mut App)>;

/// Which part of the backend section an error belongs to, so it is shown
/// next to the control that caused it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendArea {
    Selection,
    Runtimes,
    ApiKey(RemoteProvider),
    Capability,
    Restart,
    /// The voice sample a provider holds (Story 3.6).
    RemoteSample(RemoteProvider),
}

/// Everything the backend section shows, as the composition root last
/// computed it.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendPanel {
    pub selection: BackendSelection,
    pub runtimes: Vec<LocalRuntime>,
    pub api_keys: ApiKeys,
    /// What the engine actually acquired — only ever from its own report.
    pub active: ActiveBackend,
    /// The selection was saved but needs another runtime library than the
    /// one this process committed (Decision 2).
    pub restart_pending: bool,
    /// The runtime library being probed right now, if any.
    pub probing: Option<PathBuf>,
    /// What "Check again" and Install ask about.
    pub check_request: CheckRequest,
    pub errors: HashMap<BackendArea, String>,
    /// The Reference Voice Sample as each provider holds it (Story 3.6).
    pub remote_samples: Vec<RemoteSample>,
    /// Providers whose held sample is being deleted right now.
    pub deleting_samples: Vec<RemoteProvider>,
}

impl Default for BackendPanel {
    fn default() -> Self {
        Self {
            selection: BackendSelection::BUNDLED_CPU,
            runtimes: Vec::new(),
            api_keys: ApiKeys::default(),
            active: ActiveBackend::NotStarted,
            restart_pending: false,
            probing: None,
            check_request: CheckRequest::cpu(),
            errors: HashMap::new(),
            remote_samples: Vec::new(),
            deleting_samples: Vec::new(),
        }
    }
}

/// One entry of the backend `Select`.
#[derive(Clone)]
struct BackendChoice {
    selection: BackendSelection,
    label: SharedString,
}

impl gpui_kit::component::searchable_list::SearchableListItem for BackendChoice {
    type Value = BackendSelection;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.selection
    }
}

fn choices(runtimes: &[LocalRuntime]) -> Vec<BackendChoice> {
    backend_choices(runtimes)
        .into_iter()
        .map(|selection| BackendChoice {
            label: selection.label().into(),
            selection,
        })
        .collect()
}

/// The plaintext notice the API keys section shows (Decision 4).
pub const API_KEY_STORAGE_NOTICE: &str = "Keys are stored in plain text in voice-me's settings file, readable by anyone with access \
     to your account.";

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
    panel: BackendPanel,
    actions: BackendActions,
    backend_select: Entity<SelectState<Vec<BackendChoice>>>,
    key_inputs: Vec<(RemoteProvider, Entity<InputState>)>,
    /// A panel pushed in since the last render, whose selection and keys
    /// still have to reach the `Select` and the inputs — which need the
    /// window, and so are synced at the next render.
    panel_stale: bool,
    keys_stale: bool,
    runtimes_stale: bool,
    _subscriptions: Vec<Subscription>,
}

impl DependenciesView {
    pub fn new(
        deps: Arc<dyn DependencyProvisioningPort>,
        events: AppEventSender,
        panel: BackendPanel,
        outcome: DependencyOutcome,
        actions: BackendActions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let items = choices(&panel.runtimes);
        let selected = items
            .iter()
            .position(|choice| choice.selection == panel.selection)
            .map(IndexPath::new);
        let backend_select = cx.new(|cx| SelectState::new(items, selected, window, cx));
        let subscription = cx.subscribe(&backend_select, |this, _select, event, cx| {
            let SelectEvent::Confirm(Some(selection)) = event else {
                return;
            };
            if *selection != this.panel.selection {
                (this.actions.clone())(BackendAction::Select(selection.clone()), cx);
            }
        });

        let key_inputs = RemoteProvider::ALL
            .into_iter()
            .map(|provider| {
                let saved = panel.api_keys.get(provider).unwrap_or_default().to_string();
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .masked(true)
                        .placeholder(format!("{} API key", provider.label()))
                        .default_value(saved)
                });
                (provider, input)
            })
            .collect();

        Self {
            deps,
            events,
            outcome,
            provisioning: HashMap::new(),
            steps_shown: HashSet::new(),
            panel,
            actions,
            backend_select,
            key_inputs,
            panel_stale: false,
            keys_stale: false,
            runtimes_stale: false,
            _subscriptions: vec![subscription],
        }
    }

    /// Replace what the backend section shows. Called by the composition
    /// root after every backend change, and when "Active" changes.
    pub fn set_backend_panel(&mut self, panel: BackendPanel, cx: &mut Context<Self>) {
        // A key field is only overwritten when the *saved* key changed (a
        // save, a removal) — never merely because some other part of the
        // panel did, which would wipe whatever the user is typing.
        let keys_changed = panel.api_keys != self.panel.api_keys;
        let runtimes_changed = panel.runtimes != self.panel.runtimes;
        // A failed save leaves the selection unchanged, but the `Select`
        // still shows the entry the user picked: resync it to what is saved.
        let selection_failed = panel.errors.contains_key(&BackendArea::Selection);
        self.panel_stale |= keys_changed
            || runtimes_changed
            || selection_failed
            || panel.selection != self.panel.selection;
        self.keys_stale |= keys_changed;
        self.runtimes_stale |= runtimes_changed;
        self.panel = panel;
        cx.notify();
    }

    /// Bring the `Select` and the key inputs in line with the panel.
    fn sync_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.panel_stale {
            return;
        }
        self.panel_stale = false;

        let selection = self.panel.selection.clone();
        let items = self.runtimes_stale.then(|| choices(&self.panel.runtimes));
        self.runtimes_stale = false;
        self.backend_select.update(cx, |select, cx| {
            if let Some(items) = items {
                select.set_items(items, window, cx);
            }
            select.set_selected_value(&selection, window, cx);
        });

        if std::mem::take(&mut self.keys_stale) {
            for (provider, input) in &self.key_inputs {
                let saved = self
                    .panel
                    .api_keys
                    .get(*provider)
                    .unwrap_or_default()
                    .to_string();
                input.update(cx, |input, cx| {
                    if input.value().as_ref() != saved {
                        input.set_value(saved, window, cx);
                    }
                });
            }
        }
    }

    fn act(&mut self, action: BackendAction, cx: &mut Context<Self>) {
        (self.actions.clone())(action, cx);
    }

    /// "Add runtime…": a native file picker, then the root probes the file.
    fn add_runtime(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Add runtime".into()),
        });
        cx.spawn(async move |this, cx| {
            let path = match picked.await {
                Ok(Ok(Some(mut paths))) => paths.pop(),
                Ok(Ok(None)) | Err(_) => None,
                Ok(Err(error)) => {
                    let _ = this.update(cx, |this, cx| {
                        this.panel.errors.insert(
                            BackendArea::Runtimes,
                            format!("The file picker could not open: {error}"),
                        );
                        cx.notify();
                    });
                    return;
                }
            };
            if let Some(path) = path {
                let _ = this.update(cx, |this, cx| this.act(BackendAction::AddRuntime(path), cx));
            }
        })
        .detach();
    }

    fn save_key(&mut self, provider: RemoteProvider, cx: &mut Context<Self>) {
        let Some((_, input)) = self.key_inputs.iter().find(|(p, _)| *p == provider) else {
            return;
        };
        let value = input.read(cx).value().trim().to_string();
        let key = (!value.is_empty()).then_some(value);
        self.act(BackendAction::SaveApiKey(provider, key), cx);
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
        let backend = self.panel.check_request.backend;
        let work =
            tokio_bridge::spawn_blocking_on(&handle, move || deps.provision(kind, backend, events));
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
                            h_flex().child(
                                Button::new("backend-use-cpu")
                                    .primary()
                                    .label("Use CPU backend")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.act(BackendAction::UseCpu, cx)
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

    /// The backend section: the selector, what is selected and what is
    /// actually active, and a restart prompt when a switch needs one.
    fn backend_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let panel = &self.panel;
        v_flex()
            .id("backend-section")
            .gap_2()
            .child(div().font_weight(FontWeight::MEDIUM).child("Backend"))
            .child(
                Select::new(&self.backend_select)
                    .id("backend-select")
                    .accessibility_label("Backend")
                    .menu_width(px(360.))
                    .w(px(360.)),
            )
            .when_some(
                panel.errors.get(&BackendArea::Selection).cloned(),
                |el, error| el.child(error_line("backend-error-selection", error, cx)),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("backend-selected")
                            .test_support()
                            .child(format!("Selected: {}", panel.selection.label())),
                    )
                    // UX-DR18: CPU is a normal choice — an informational
                    // tag, never a warning, however good the GPU is.
                    .when(panel.selection.is_cpu(), |el| {
                        el.child(
                            div()
                                .id("backend-cpu-mode")
                                .test_support()
                                .child(Tag::secondary().small().child("CPU mode")),
                        )
                    }),
            )
            .child(
                div()
                    .id("backend-active")
                    .test_support()
                    .text_color(cx.theme().muted_foreground)
                    .child(panel.active.summary()),
            )
            .when(panel.restart_pending, |el| {
                el.child(
                    h_flex()
                        .id("backend-restart-pending")
                        .test_support()
                        .items_center()
                        .gap_2()
                        .child("Takes effect after restart")
                        .child(
                            Button::new("backend-restart-now")
                                .label("Restart now")
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.act(BackendAction::Restart, cx)
                                })),
                        ),
                )
                .when_some(
                    panel.errors.get(&BackendArea::Restart).cloned(),
                    |el, error| el.child(error_line("backend-error-restart", error, cx)),
                )
            })
            .into_any_element()
    }

    /// The runtimes the user added, and "Add runtime…".
    fn runtimes_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let panel = &self.panel;
        let mut rows = Vec::with_capacity(panel.runtimes.len());
        for (index, runtime) in panel.runtimes.iter().enumerate() {
            let path = runtime.path.clone();
            let provides = runtime
                .targets
                .iter()
                .map(|target| target.label())
                .collect::<Vec<_>>()
                .join(", ");
            rows.push(
                h_flex()
                    .id(SharedString::from(format!("backend-runtime-{index}")))
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        v_flex().min_w_0().child(runtime.file_name()).child(
                            div()
                                .text_size(px(12.))
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{} — provides {provides}", runtime.path.display())),
                        ),
                    )
                    .child(
                        Button::new(SharedString::from(format!(
                            "backend-runtime-remove-{index}"
                        )))
                        .ghost()
                        .label("Remove")
                        .on_click(cx.listener(
                            move |this, _, _window, cx| {
                                this.act(BackendAction::RemoveRuntime(path.clone()), cx)
                            },
                        )),
                    )
                    .into_any_element(),
            );
        }

        v_flex()
            .id("backend-runtimes")
            .gap_2()
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child("Local runtimes"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        "Add an ONNX Runtime library to use the execution providers it was \
                         built with.",
                    ),
            )
            .children(rows)
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("backend-add-runtime")
                            .label("Add runtime…")
                            .disabled(panel.probing.is_some())
                            .on_click(cx.listener(|this, _, _window, cx| this.add_runtime(cx))),
                    )
                    .when_some(panel.probing.as_ref(), |el, path| {
                        el.child(
                            div()
                                .id("backend-probing")
                                .test_support()
                                .text_size(px(12.))
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("Probing {}…", path.display())),
                        )
                    }),
            )
            .when_some(
                panel.errors.get(&BackendArea::Runtimes).cloned(),
                |el, error| el.child(error_line("backend-error-runtimes", error, cx)),
            )
            .into_any_element()
    }

    /// One masked field per provider, each saved independently.
    fn api_keys_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut fields = Vec::with_capacity(self.key_inputs.len());
        for (provider, input) in &self.key_inputs {
            let provider = *provider;
            let slug = provider_slug(provider);
            let saved = self.panel.api_keys.has(provider);
            fields.push(
                v_flex()
                    .gap_1()
                    .child(provider.label())
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(div().w(px(360.)).child(Input::new(input).mask_toggle()))
                            .child(
                                Button::new(SharedString::from(format!("api-key-save-{slug}")))
                                    .label("Save")
                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                        this.save_key(provider, cx)
                                    })),
                            )
                            .child(
                                Button::new(SharedString::from(format!("api-key-remove-{slug}")))
                                    .ghost()
                                    .label("Remove")
                                    .disabled(!saved)
                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                        this.act(BackendAction::SaveApiKey(provider, None), cx)
                                    })),
                            ),
                    )
                    .when_some(
                        self.panel
                            .errors
                            .get(&BackendArea::ApiKey(provider))
                            .cloned(),
                        |el, error| {
                            el.child(error_line(format!("api-key-error-{slug}"), error, cx))
                        },
                    )
                    .into_any_element(),
            );
        }

        v_flex()
            .id("backend-api-keys")
            .gap_2()
            .child(div().font_weight(FontWeight::MEDIUM).child("API keys"))
            .child(
                div()
                    .id("api-key-plaintext-notice")
                    .test_support()
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child(API_KEY_STORAGE_NOTICE),
            )
            .children(fields)
            .into_any_element()
    }

    /// Story 3.6: whether a provider holds the Reference Voice Sample, and
    /// the way to take it back. Only providers that can hold one are
    /// listed.
    fn remote_samples_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut lines = Vec::new();
        for provider in SAMPLE_HOLDING_PROVIDERS {
            let slug = provider_slug(provider);
            let label = provider.label();
            let held = self
                .panel
                .remote_samples
                .iter()
                .any(|sample| sample.provider == provider);
            let deleting = self.panel.deleting_samples.contains(&provider);
            let state = if held {
                format!("Held on {label}'s servers")
            } else {
                format!("Not held on {label}")
            };
            lines.push(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(format!("Voice sample on {label}")),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .id(SharedString::from(format!("remote-sample-state-{slug}")))
                                    .test_support()
                                    .child(state),
                            )
                            .when(held, |el| {
                                el.child(
                                    Button::new(SharedString::from(format!(
                                        "remote-sample-delete-{slug}"
                                    )))
                                    .small()
                                    .label(if deleting {
                                        "Deleting…".to_string()
                                    } else {
                                        format!("Delete from {label}")
                                    })
                                    .disabled(deleting)
                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                        this.act(BackendAction::DeleteRemoteSample(provider), cx)
                                    })),
                                )
                            }),
                    )
                    .when_some(
                        self.panel
                            .errors
                            .get(&BackendArea::RemoteSample(provider))
                            .cloned(),
                        |el, error| {
                            el.child(error_line(format!("remote-sample-error-{slug}"), error, cx))
                        },
                    )
                    .into_any_element(),
            );
        }

        v_flex()
            .id("backend-remote-samples")
            .gap_2()
            .children(lines)
            .into_any_element()
    }
}

/// The providers that can hold an uploaded Reference Voice Sample in this
/// release. fal.ai joins with Story 3.7.
const SAMPLE_HOLDING_PROVIDERS: [RemoteProvider; 1] = [RemoteProvider::DeepInfra];

/// An inline error next to the control it belongs to.
fn error_line(id: impl Into<SharedString>, error: String, cx: &App) -> AnyElement {
    div()
        .id(id.into())
        .test_support()
        .text_size(px(12.))
        .text_color(cx.theme().danger)
        .child(error)
        .into_any_element()
}

fn provider_slug(provider: RemoteProvider) -> &'static str {
    match provider {
        RemoteProvider::DeepInfra => "deepinfra",
        RemoteProvider::FalAi => "fal-ai",
    }
}

impl Render for DependenciesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_controls(window, cx);
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
            .child(self.backend_section(cx))
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
            .child(self.runtimes_section(cx))
            .child(self.api_keys_section(cx))
            .child(self.remote_samples_section(cx))
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

    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{TestAppContext, component::Root, px, size};
    use voice_me_core::{AppEvent, VoiceMeError};

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
        last_backend: std::sync::Mutex<Option<voice_me_core::SpeechBackend>>,
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
            backend: voice_me_core::SpeechBackend,
            _events: AppEventSender,
        ) -> Result<(), VoiceMeError> {
            *self.last_backend.lock().unwrap() = Some(backend);
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
            let view = cx.new(|cx| {
                DependenciesView::new(
                    deps.clone(),
                    event_tx.clone(),
                    panel,
                    outcome,
                    no_actions(),
                    window,
                    cx,
                )
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
            let view = cx.new(|cx| {
                DependenciesView::new(
                    deps.clone(),
                    event_tx.clone(),
                    BackendPanel::default(),
                    DependencyOutcome::Pending,
                    no_actions(),
                    window,
                    cx,
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
                _backend: voice_me_core::SpeechBackend,
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
            let view = cx.new(|cx| {
                DependenciesView::new(
                    Arc::new(FailingDeps),
                    event_tx.clone(),
                    BackendPanel::default(),
                    DependencyOutcome::Failed("could not resolve a cache directory".to_string()),
                    no_actions(),
                    window,
                    cx,
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
            let view = cx.new(|cx| {
                DependenciesView::new(
                    deps.clone(),
                    event_tx.clone(),
                    BackendPanel::default(),
                    report_with_an_installable_and_a_manual_row(),
                    no_actions(),
                    window,
                    cx,
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
            let view = cx.new(|cx| {
                DependenciesView::new(
                    deps.clone(),
                    event_tx.clone(),
                    BackendPanel::default(),
                    report_with_an_installable_and_a_manual_row(),
                    no_actions(),
                    window,
                    cx,
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

    // ---- Story 3.3: the backend section ----------------------------------

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
    fn open_backend_tab(
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
            let view = cx.new(|cx| {
                DependenciesView::new(
                    deps.clone(),
                    event_tx.clone(),
                    panel,
                    outcome,
                    actions,
                    window,
                    cx,
                )
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
        let (window, _view, recorded) = open_backend_tab(cx, cuda_panel(), cuda_cannot_run());

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

    /// UX-DR18: CPU is a normal choice with an informational tag; a CUDA
    /// selection has none. "Selected" and "Active" are separate lines.
    #[gpui_kit::test]
    fn the_cpu_mode_tag_follows_the_selection(cx: &mut TestAppContext) {
        let (window, view, _recorded) =
            open_backend_tab(cx, cuda_panel(), DependencyOutcome::Pending);

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("backend-cpu-mode").is_none());
            assert!(window.try_find("backend-selected").is_some());
            assert!(window.try_find("backend-active").is_some());
            assert_eq!(
                view.read(cx).panel.active.summary(),
                "Active: not started yet"
            );

            view.update(cx, |view, cx| {
                view.set_backend_panel(
                    BackendPanel {
                        active: ActiveBackend::Acquired(voice_me_core::SpeechBackend::CPU),
                        ..BackendPanel::default()
                    },
                    cx,
                )
            });
            window.render_frame(cx);
            assert!(window.try_find("backend-cpu-mode").is_some());
            assert_eq!(
                view.read(cx).panel.active.summary(),
                "Active: CPU — Q4 weights"
            );
        })
        .unwrap();
    }

    /// Decision 2: "Takes effect after restart" appears only when the root
    /// says the switch needs one, and Restart now asks the root.
    #[gpui_kit::test]
    fn the_restart_line_appears_only_when_needed(cx: &mut TestAppContext) {
        let (window, view, recorded) =
            open_backend_tab(cx, cuda_panel(), DependencyOutcome::Pending);

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("backend-restart-pending").is_none());
            assert!(window.try_find("backend-restart-now").is_none());

            view.update(cx, |view, cx| {
                let panel = BackendPanel {
                    restart_pending: true,
                    ..view.panel.clone()
                };
                view.set_backend_panel(panel, cx)
            });
            window.render_frame(cx);
            assert!(window.try_find("backend-restart-pending").is_some());
            window.click("backend-restart-now", cx);
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(*recorded.borrow(), vec![BackendAction::Restart]);
    }

    /// A key typed into the masked field is saved through the root's
    /// callback — never by the view itself — and stays masked.
    #[gpui_kit::test]
    fn a_masked_key_is_saved_through_the_callback(cx: &mut TestAppContext) {
        let (window, view, recorded) =
            open_backend_tab(cx, BackendPanel::default(), DependencyOutcome::Pending);

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("api-key-plaintext-notice").is_some());
            let input = view.read(cx).key_inputs[0].1.clone();
            assert_eq!(view.read(cx).key_inputs[0].0, RemoteProvider::DeepInfra);
            assert!(
                input.read(cx).presentation().is_masked(),
                "the key is masked by default"
            );
            input.update(cx, |input, cx| {
                input.set_value("  sk-test-123  ", window, cx)
            });
            window.render_frame(cx);
            window.click("api-key-save-deepinfra", cx);
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(
            *recorded.borrow(),
            vec![BackendAction::SaveApiKey(
                RemoteProvider::DeepInfra,
                Some("sk-test-123".to_string())
            )]
        );
        assert!(
            !format!("{:?}", recorded.borrow()).contains("sk-test"),
            "a key never reaches Debug output"
        );
    }

    /// A saved key comes back in the field, masked; removing it clears the
    /// field once the root says the key is gone.
    #[gpui_kit::test]
    fn a_saved_key_is_shown_masked_and_cleared_on_removal(cx: &mut TestAppContext) {
        let mut panel = BackendPanel::default();
        panel
            .api_keys
            .set(RemoteProvider::FalAi, Some("fal-secret".to_string()));
        let (window, view, _recorded) = open_backend_tab(cx, panel, DependencyOutcome::Pending);

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let input = view.read(cx).key_inputs[1].1.clone();
            assert_eq!(input.read(cx).value().as_ref(), "fal-secret");
            assert!(input.read(cx).presentation().is_masked());

            view.update(cx, |view, cx| {
                view.set_backend_panel(BackendPanel::default(), cx)
            });
            window.render_frame(cx);
            assert_eq!(input.read(cx).value().as_ref(), "");
        })
        .unwrap();
    }

    /// Story 3.6: a held sample says so and offers Delete, which asks the
    /// root exactly once; nothing held says that instead, with no button.
    #[gpui_kit::test]
    fn a_held_sample_offers_delete_through_the_root(cx: &mut TestAppContext) {
        let panel = BackendPanel {
            remote_samples: vec![RemoteSample {
                provider: RemoteProvider::DeepInfra,
                sample_sha256: "aa".to_string(),
                voice_id: "v1".to_string(),
            }],
            ..BackendPanel::default()
        };
        let (window, view, recorded) = open_backend_tab(cx, panel, DependencyOutcome::Pending);

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("remote-sample-state-deepinfra").is_some());
            assert!(
                window.try_find("remote-sample-state-fal-ai").is_none(),
                "fal.ai holds no samples in this release"
            );
            window.click("remote-sample-delete-deepinfra", cx);
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(
            *recorded.borrow(),
            vec![BackendAction::DeleteRemoteSample(RemoteProvider::DeepInfra)]
        );

        // The root says it is gone: the button goes with it.
        cx.update_window(window.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.set_backend_panel(BackendPanel::default(), cx)
            });
            window.render_frame(cx);
            assert!(window.try_find("remote-sample-delete-deepinfra").is_none());
            assert!(window.try_find("remote-sample-state-deepinfra").is_some());
        })
        .unwrap();
    }

    /// A failed delete is shown inline, next to the sample line.
    #[gpui_kit::test]
    fn a_failed_delete_is_shown_inline(cx: &mut TestAppContext) {
        let mut panel = BackendPanel {
            remote_samples: vec![RemoteSample {
                provider: RemoteProvider::DeepInfra,
                sample_sha256: "aa".to_string(),
                voice_id: "v1".to_string(),
            }],
            ..BackendPanel::default()
        };
        panel.errors.insert(
            BackendArea::RemoteSample(RemoteProvider::DeepInfra),
            "DeepInfra: internal".to_string(),
        );
        let (window, _view, _recorded) = open_backend_tab(cx, panel, DependencyOutcome::Pending);

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("remote-sample-error-deepinfra").is_some());
            assert!(window.try_find("remote-sample-delete-deepinfra").is_some());
        })
        .unwrap();
    }

    /// Choosing another entry in the `Select` asks the root to select it.
    #[gpui_kit::test]
    fn choosing_a_backend_asks_the_root(cx: &mut TestAppContext) {
        let (window, view, recorded) =
            open_backend_tab(cx, cuda_panel(), DependencyOutcome::Pending);

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let select = view.read(cx).backend_select.clone();
            select.update(cx, |_, cx| {
                // Re-confirming the current selection is not a change.
                cx.emit(SelectEvent::Confirm(Some(cuda_panel().selection)));
                cx.emit(SelectEvent::Confirm(Some(BackendSelection::Remote(
                    RemoteProvider::DeepInfra,
                ))));
            });
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(
            *recorded.borrow(),
            vec![BackendAction::Select(BackendSelection::Remote(
                RemoteProvider::DeepInfra
            ))]
        );
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

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("dependencies-check-again", cx);
            window.click("dependency-install-model-weights", cx);
        })
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while (port.checks.load(Ordering::SeqCst) == 0
            || port.provisions.load(Ordering::SeqCst) == 0)
            && std::time::Instant::now() < deadline
        {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert_eq!(*port.last_request.lock().unwrap(), Some(request.clone()));
        assert_eq!(*port.last_backend.lock().unwrap(), Some(request.backend));
    }
}
