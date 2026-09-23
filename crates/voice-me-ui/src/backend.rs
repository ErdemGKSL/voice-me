//! Settings → Backend (Story 3.10): which backend speaks for the user, and
//! that backend's own options.
//!
//! The tab is two-step. The user first chooses **Local** or **Remote**, then
//! one backend of that kind from a `Select`. Below that it shows only the
//! *saved* backend's options: the provider's masked key and held voice
//! sample for a remote one. Last come the *Selected* and *Active* lines,
//! kept as two separate facts, the "CPU mode" tag and the restart line.
//!
//! Flipping Local ↔ Remote only changes what the tab shows (Decision 1):
//! nothing is saved until an entry of the new kind is picked, so the
//! `Select` then shows a placeholder, the saved backend's options are not
//! shown, and *Selected* still names the saved backend. When the saved
//! selection changes, the kind follows it.
//!
//! Two things stay reachable whichever backend is saved (Decision 2): the
//! Local kind always shows the added runtimes and **Add runtime…**, and the
//! Remote kind gives every other provider that holds the voice sample or a
//! saved key one short line with **Remove key** / **Delete from …**.
//!
//! The view never touches `SettingsStore`: every change goes to the
//! composition root as a [`BackendAction`], and the root pushes the result
//! back in as a [`BackendPanel`] — the same panel the Dependencies tab gets.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IndexPath, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    radio::{Radio, RadioGroup},
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
    ActiveBackend, ApiKeys, BackendSelection, CheckRequest, LocalRuntime, RemoteProvider,
    RemoteSample, backend_choices,
};

/// Something the user asked of the backend. The views only *ask*; the
/// composition root does it (through `SettingsStore`, the probe helper, or
/// a relaunch) and pushes the outcome back as a [`BackendPanel`].
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

/// Written by hand so a key typed into a key field can never reach a log
/// line through `{:?}`.
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

/// Which part of the backend UI an error belongs to, so it is shown next
/// to the control that caused it.
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

/// Everything the Backend and Dependencies tabs show about the backend, as
/// the composition root last computed it.
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

/// The plaintext notice shown beside a key field (Decision 4).
pub const API_KEY_STORAGE_NOTICE: &str = "Keys are stored in plain text in voice-me's settings file, readable by anyone with access \
     to your account.";

/// The providers that can hold an uploaded Reference Voice Sample in this
/// release. fal.ai joins with Story 3.7.
const SAMPLE_HOLDING_PROVIDERS: [RemoteProvider; 1] = [RemoteProvider::DeepInfra];

/// The first step of the tab: where speech is generated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendKind {
    Local,
    Remote,
}

impl BackendKind {
    const ALL: [BackendKind; 2] = [BackendKind::Local, BackendKind::Remote];

    fn of(selection: &BackendSelection) -> Self {
        match selection {
            BackendSelection::Local { .. } => BackendKind::Local,
            BackendSelection::Remote(_) => BackendKind::Remote,
        }
    }

    fn index(self) -> usize {
        match self {
            BackendKind::Local => 0,
            BackendKind::Remote => 1,
        }
    }

    fn label(self) -> &'static str {
        match self {
            BackendKind::Local => "Local — on this computer",
            BackendKind::Remote => "Remote — with your own API key",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            BackendKind::Local => "Choose a local backend",
            BackendKind::Remote => "Choose a remote backend",
        }
    }

    /// The heading above the kind choice.
    const HEADING: &'static str = "Where speech is generated";

    fn select_label(self) -> &'static str {
        match self {
            BackendKind::Local => "Local backend",
            BackendKind::Remote => "Remote backend",
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

/// The existing backends of one kind: the bundled CPU entry and each added
/// runtime's entries, or every remote provider.
fn choices(kind: BackendKind, runtimes: &[LocalRuntime]) -> Vec<BackendChoice> {
    backend_choices(runtimes)
        .into_iter()
        .filter(|selection| BackendKind::of(selection) == kind)
        .map(|selection| BackendChoice {
            label: selection.label().into(),
            selection,
        })
        .collect()
}

/// The Backend tab.
pub struct BackendView {
    panel: BackendPanel,
    actions: BackendActions,
    /// The kind being shown — the saved selection's, unless the user
    /// flipped it and has not picked an entry yet (Decision 1).
    kind: BackendKind,
    backend_select: Entity<SelectState<Vec<BackendChoice>>>,
    key_inputs: Vec<(RemoteProvider, Entity<InputState>)>,
    /// A panel (or kind) change since the last render that still has to
    /// reach the `Select` and the inputs — which need the window, and so
    /// are synced at the next render.
    panel_stale: bool,
    keys_stale: bool,
    items_stale: bool,
    _subscriptions: Vec<Subscription>,
}

impl BackendView {
    pub fn new(
        panel: BackendPanel,
        actions: BackendActions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let kind = BackendKind::of(&panel.selection);
        let items = choices(kind, &panel.runtimes);
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
                this.act(BackendAction::Select(selection.clone()), cx);
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
            panel,
            actions,
            kind,
            backend_select,
            key_inputs,
            panel_stale: false,
            keys_stale: false,
            items_stale: false,
            _subscriptions: vec![subscription],
        }
    }

    /// Replace what the tab shows. Called by the composition root after
    /// every backend change, and when "Active" changes.
    pub fn set_backend_panel(&mut self, panel: BackendPanel, cx: &mut Context<Self>) {
        // A key field is only overwritten when the *saved* key changed (a
        // save, a removal) — never merely because some other part of the
        // panel did, which would wipe whatever the user is typing.
        let keys_changed = panel.api_keys != self.panel.api_keys;
        let runtimes_changed = panel.runtimes != self.panel.runtimes;
        let selection_changed = panel.selection != self.panel.selection;
        // A failed save leaves the selection unchanged, but the `Select`
        // still shows the entry the user picked: resync it to what is saved.
        let selection_failed = panel.errors.contains_key(&BackendArea::Selection);

        // Decision 1: the kind follows the saved selection when it changes.
        if selection_changed {
            let kind = BackendKind::of(&panel.selection);
            self.items_stale |= kind != self.kind;
            self.kind = kind;
        }

        self.panel_stale |=
            keys_changed || runtimes_changed || selection_failed || selection_changed;
        self.keys_stale |= keys_changed;
        self.items_stale |= runtimes_changed;
        self.panel = panel;
        cx.notify();
    }

    /// Show the other kind's backends. Saves nothing (Decision 1).
    fn set_kind(&mut self, kind: BackendKind, cx: &mut Context<Self>) {
        if kind == self.kind {
            return;
        }
        self.kind = kind;
        self.items_stale = true;
        self.panel_stale = true;
        cx.notify();
    }

    /// The saved remote provider, while the Remote kind is shown — the only
    /// case in which a provider's key field and sample section appear.
    fn shown_saved_provider(&self) -> Option<RemoteProvider> {
        match (&self.panel.selection, self.kind) {
            (BackendSelection::Remote(provider), BackendKind::Remote) => Some(*provider),
            _ => None,
        }
    }

    /// Bring the `Select` and the key inputs in line with the panel.
    fn sync_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.panel_stale {
            return;
        }
        self.panel_stale = false;

        let selection = self.panel.selection.clone();
        let items =
            std::mem::take(&mut self.items_stale).then(|| choices(self.kind, &self.panel.runtimes));
        self.backend_select.update(cx, |select, cx| {
            if let Some(items) = items {
                select.set_items(items, window, cx);
            }
            // The saved entry is absent from the other kind's list, which
            // leaves the `Select` empty, on its placeholder.
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

    /// Step one and two: the kind, then a backend of that kind.
    fn choice_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let kind = self.kind;
        v_flex()
            .gap_4()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .id("backend-kind-heading")
                            .test_support()
                            .font_weight(FontWeight::MEDIUM)
                            .child(BackendKind::HEADING),
                    )
                    .child(
                        RadioGroup::horizontal("backend-kind")
                            .selected_index(Some(kind.index()))
                            .children(
                                BackendKind::ALL
                                    .into_iter()
                                    .map(|kind| Radio::new(kind.index()).label(kind.label())),
                            )
                            .on_change(cx.listener(|this, index: &usize, _window, cx| {
                                if let Some(kind) = BackendKind::ALL.get(*index) {
                                    this.set_kind(*kind, cx);
                                }
                            })),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(kind.select_label()),
                    )
                    .child(
                        Select::new(&self.backend_select)
                            .id("backend-select")
                            .accessibility_label(kind.select_label())
                            .placeholder(kind.placeholder())
                            .menu_width(px(360.))
                            .w(px(360.)),
                    )
                    .when_some(
                        self.panel.errors.get(&BackendArea::Selection).cloned(),
                        |el, error| el.child(error_line("backend-error-selection", error, cx)),
                    ),
            )
            .into_any_element()
    }

    /// The options of the kind being shown. Local: the added runtimes,
    /// whichever backend is saved (Decision 2). Remote: the saved
    /// provider's key and sample sections — none while the saved backend
    /// is of the other kind (Decision 1) — then one short line for every
    /// other provider holding a key or the voice sample (Decision 2).
    fn options_section(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        match self.kind {
            BackendKind::Local => Some(self.runtimes_section(cx)),
            BackendKind::Remote => {
                let saved = self.shown_saved_provider();
                let others: Vec<AnyElement> = RemoteProvider::ALL
                    .into_iter()
                    .filter(|provider| Some(*provider) != saved)
                    .filter_map(|provider| self.other_provider_line(provider, cx))
                    .collect();
                if saved.is_none() && others.is_empty() {
                    return None;
                }
                Some(
                    v_flex()
                        .gap_6()
                        .when_some(saved, |el, provider| {
                            el.child(self.api_key_section(provider, cx))
                                .when(SAMPLE_HOLDING_PROVIDERS.contains(&provider), |el| {
                                    el.child(self.remote_sample_section(provider, cx))
                                })
                        })
                        .when(!others.is_empty(), |el| {
                            el.child(
                                v_flex()
                                    .id("backend-other-providers")
                                    .test_support()
                                    .gap_2()
                                    .children(others),
                            )
                        })
                        .into_any_element(),
                )
            }
        }
    }

    /// Decision 2: a provider that is not the saved one but holds a saved
    /// key or the voice sample — one line naming what applies, with the
    /// way to take each back. `None` when it holds neither.
    fn other_provider_line(
        &self,
        provider: RemoteProvider,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let panel = &self.panel;
        let has_key = panel.api_keys.has(provider);
        let held = panel
            .remote_samples
            .iter()
            .any(|sample| sample.provider == provider);
        if !has_key && !held {
            return None;
        }
        let slug = provider_slug(provider);
        let label = provider.label();
        let deleting = panel.deleting_samples.contains(&provider);
        let facts = [
            has_key.then_some("key saved"),
            held.then_some("voice sample held"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let errors = [
            BackendArea::ApiKey(provider),
            BackendArea::RemoteSample(provider),
        ]
        .into_iter()
        .filter_map(|area| panel.errors.get(&area).cloned())
        .collect::<Vec<_>>();

        Some(
            v_flex()
                .id(SharedString::from(format!("backend-other-{slug}")))
                .test_support()
                .gap_1()
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .id(SharedString::from(format!("backend-other-state-{slug}")))
                                .test_support()
                                .child(format!("{label}: {facts}")),
                        )
                        .when(has_key, |el| {
                            el.child(
                                Button::new(SharedString::from(format!(
                                    "backend-other-remove-key-{slug}"
                                )))
                                .small()
                                .ghost()
                                .label("Remove key")
                                .on_click(cx.listener(
                                    move |this, _, _window, cx| {
                                        this.act(BackendAction::SaveApiKey(provider, None), cx)
                                    },
                                )),
                            )
                        })
                        .when(held, |el| {
                            el.child(
                                Button::new(SharedString::from(format!(
                                    "backend-other-delete-sample-{slug}"
                                )))
                                .small()
                                .label(if deleting {
                                    "Deleting…".to_string()
                                } else {
                                    format!("Delete from {label}")
                                })
                                .disabled(deleting)
                                .on_click(cx.listener(
                                    move |this, _, _window, cx| {
                                        this.act(BackendAction::DeleteRemoteSample(provider), cx)
                                    },
                                )),
                            )
                        }),
                )
                .when(!errors.is_empty(), |el| {
                    el.child(error_line(
                        format!("backend-other-error-{slug}"),
                        errors.join(" "),
                        cx,
                    ))
                })
                .into_any_element(),
        )
    }

    /// What is selected and what is actually active, the CPU-mode tag, and
    /// a restart prompt when a switch needs one.
    fn status_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let panel = &self.panel;
        v_flex()
            .id("backend-section")
            .gap_2()
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
                    .test_support()
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
            .test_support()
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

    /// The selected provider's masked key field, with the plaintext notice.
    fn api_key_section(&self, provider: RemoteProvider, cx: &mut Context<Self>) -> AnyElement {
        let slug = provider_slug(provider);
        let saved = self.panel.api_keys.has(provider);
        let input = self
            .key_inputs
            .iter()
            .find(|(p, _)| *p == provider)
            .map(|(_, input)| input.clone());

        v_flex()
            .id("backend-api-keys")
            .test_support()
            .gap_1()
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child(format!("{} API key", provider.label())),
            )
            .when_some(input, |el, input| {
                el.child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(div().w(px(360.)).child(Input::new(&input).mask_toggle()))
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
            })
            .child(
                div()
                    .id("api-key-plaintext-notice")
                    .test_support()
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child(API_KEY_STORAGE_NOTICE),
            )
            .when_some(
                self.panel
                    .errors
                    .get(&BackendArea::ApiKey(provider))
                    .cloned(),
                |el, error| el.child(error_line(format!("api-key-error-{slug}"), error, cx)),
            )
            .into_any_element()
    }

    /// Story 3.6: whether the provider holds the Reference Voice Sample,
    /// and the way to take it back.
    fn remote_sample_section(
        &self,
        provider: RemoteProvider,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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

        v_flex()
            .id("backend-remote-samples")
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
                            Button::new(SharedString::from(format!("remote-sample-delete-{slug}")))
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
                |el, error| el.child(error_line(format!("remote-sample-error-{slug}"), error, cx)),
            )
            .into_any_element()
    }
}

impl Render for BackendView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_controls(window, cx);
        let options = self.options_section(cx);

        v_flex()
            .id("backend-surface")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .p_6()
            .gap_6()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(div().text_lg().child("Backend"))
            .child(self.choice_section(cx))
            .children(options)
            .child(self.status_section(cx))
    }
}

/// An inline error next to the control it belongs to.
pub(crate) fn error_line(id: impl Into<SharedString>, error: String, cx: &App) -> AnyElement {
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

#[cfg(test)]
mod tests {
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{TestAppContext, component::Root, px, size};

    use super::*;

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

    fn remote_panel(provider: RemoteProvider) -> BackendPanel {
        BackendPanel {
            selection: BackendSelection::Remote(provider),
            ..BackendPanel::default()
        }
    }

    /// Opens the tab with `panel`, recording every action.
    fn open_backend_tab(
        cx: &mut TestAppContext,
        panel: BackendPanel,
    ) -> (
        gpui_kit::WindowHandle<Root>,
        gpui_kit::Entity<BackendView>,
        Recorded,
    ) {
        cx.update(gpui_kit::init);
        let (actions, recorded) = recording_actions();
        let mut slot = None;
        let window = cx.open_window(size(px(900.), px(1400.)), |window, cx| {
            let view = cx.new(|cx| BackendView::new(panel, actions, window, cx));
            slot = Some(view.clone());
            Root::new(view, window, cx)
        });
        (window, slot.unwrap(), recorded)
    }

    /// Local CPU saved: the Local kind, CPU in the `Select`, the runtimes,
    /// no key field, and the CPU-mode tag.
    #[gpui_kit::test]
    fn a_local_selection_shows_runtimes_and_no_key_field(cx: &mut TestAppContext) {
        let (window, view, _recorded) = open_backend_tab(cx, BackendPanel::default());

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(view.read(cx).kind, BackendKind::Local);
            assert_eq!(
                view.read(cx).backend_select.read(cx).selected_value(),
                Some(&BackendSelection::BUNDLED_CPU)
            );
            assert!(window.try_find("backend-runtimes").is_some());
            assert!(window.try_find("backend-add-runtime").is_some());
            assert!(window.try_find("backend-api-keys").is_none());
            assert!(window.try_find("api-key-save-deepinfra").is_none());
            assert!(window.try_find("api-key-save-fal-ai").is_none());
            assert!(window.try_find("remote-sample-state-deepinfra").is_none());
            assert!(window.try_find("backend-cpu-mode").is_some());
        })
        .unwrap();
    }

    /// A remote selection: the Remote kind, only that provider's key field
    /// with its notice and sample line, and no runtimes.
    #[gpui_kit::test]
    fn a_remote_selection_shows_only_that_providers_options(cx: &mut TestAppContext) {
        let (window, view, _recorded) =
            open_backend_tab(cx, remote_panel(RemoteProvider::DeepInfra));

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(view.read(cx).kind, BackendKind::Remote);
            assert!(window.try_find("api-key-save-deepinfra").is_some());
            assert!(window.try_find("api-key-plaintext-notice").is_some());
            assert!(window.try_find("remote-sample-state-deepinfra").is_some());
            assert!(
                window.try_find("api-key-save-fal-ai").is_none(),
                "only the selected provider's key field"
            );
            assert!(window.try_find("backend-runtimes").is_none());
            assert!(window.try_find("backend-add-runtime").is_none());
            assert!(window.try_find("backend-cpu-mode").is_none());
        })
        .unwrap();

        // fal.ai holds no samples in this release: no sample line.
        let (window, _view, _recorded) = open_backend_tab(cx, remote_panel(RemoteProvider::FalAi));
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("api-key-save-fal-ai").is_some());
            assert!(window.try_find("remote-sample-state-fal-ai").is_none());
            assert!(window.try_find("api-key-save-deepinfra").is_none());
        })
        .unwrap();
    }

    /// Decision 1: flipping the kind saves nothing and shows no options;
    /// the `Select` is empty and Selected still names the saved backend.
    #[gpui_kit::test]
    fn flipping_the_kind_sends_nothing_and_shows_no_options(cx: &mut TestAppContext) {
        let (window, view, recorded) = open_backend_tab(cx, BackendPanel::default());

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window
                .within("backend-kind")
                .click(BackendKind::Remote.index(), cx);
            window.render_frame(cx);

            assert_eq!(view.read(cx).kind, BackendKind::Remote);
            assert_eq!(
                view.read(cx).backend_select.read(cx).selected_value(),
                None,
                "none of the remote backends is chosen"
            );
            assert!(window.try_find("backend-runtimes").is_none());
            assert!(window.try_find("backend-api-keys").is_none());
            assert!(window.try_find("api-key-save-deepinfra").is_none());
            assert!(window.try_find("remote-sample-state-deepinfra").is_none());
            assert!(window.try_find("backend-selected").is_some());
            assert_eq!(
                view.read(cx).panel.selection,
                BackendSelection::BUNDLED_CPU,
                "Selected still names the saved backend"
            );
            assert!(window.try_find("backend-cpu-mode").is_some());

            // Back to Local: the saved entry is chosen again.
            window
                .within("backend-kind")
                .click(BackendKind::Local.index(), cx);
            window.render_frame(cx);
            assert_eq!(view.read(cx).kind, BackendKind::Local);
            assert_eq!(
                view.read(cx).backend_select.read(cx).selected_value(),
                Some(&BackendSelection::BUNDLED_CPU)
            );
            assert!(window.try_find("backend-runtimes").is_some());
        })
        .unwrap();
        cx.run_until_parked();

        assert!(recorded.borrow().is_empty(), "flipping saves nothing");
    }

    /// When the saved selection changes, the kind follows it.
    #[gpui_kit::test]
    fn the_kind_follows_a_new_saved_selection(cx: &mut TestAppContext) {
        let (window, view, _recorded) = open_backend_tab(cx, BackendPanel::default());

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            view.update(cx, |view, cx| {
                view.set_backend_panel(remote_panel(RemoteProvider::DeepInfra), cx)
            });
            window.render_frame(cx);
            assert_eq!(view.read(cx).kind, BackendKind::Remote);
            assert_eq!(
                view.read(cx).backend_select.read(cx).selected_value(),
                Some(&BackendSelection::Remote(RemoteProvider::DeepInfra))
            );
            assert!(window.try_find("api-key-save-deepinfra").is_some());
        })
        .unwrap();
    }

    /// UX-DR18: CPU is a normal choice with an informational tag; a CUDA
    /// selection has none. "Selected" and "Active" are separate lines.
    #[gpui_kit::test]
    fn the_cpu_mode_tag_follows_the_selection(cx: &mut TestAppContext) {
        let (window, view, _recorded) = open_backend_tab(cx, cuda_panel());

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
        let (window, view, recorded) = open_backend_tab(cx, cuda_panel());

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
            open_backend_tab(cx, remote_panel(RemoteProvider::DeepInfra));

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
        let mut panel = remote_panel(RemoteProvider::FalAi);
        panel
            .api_keys
            .set(RemoteProvider::FalAi, Some("fal-secret".to_string()));
        let (window, view, recorded) = open_backend_tab(cx, panel);

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let input = view.read(cx).key_inputs[1].1.clone();
            assert_eq!(input.read(cx).value().as_ref(), "fal-secret");
            assert!(input.read(cx).presentation().is_masked());
            window.click("api-key-remove-fal-ai", cx);

            view.update(cx, |view, cx| {
                view.set_backend_panel(remote_panel(RemoteProvider::FalAi), cx)
            });
            window.render_frame(cx);
            assert_eq!(input.read(cx).value().as_ref(), "");
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(
            *recorded.borrow(),
            vec![BackendAction::SaveApiKey(RemoteProvider::FalAi, None)]
        );
    }

    /// Story 3.6: a held sample offers Delete, which asks the root exactly
    /// once; once nothing is held the state line stays and the button goes.
    #[gpui_kit::test]
    fn a_held_sample_offers_delete_through_the_root(cx: &mut TestAppContext) {
        let panel = BackendPanel {
            remote_samples: vec![RemoteSample {
                provider: RemoteProvider::DeepInfra,
                sample_sha256: "aa".to_string(),
                voice_id: "v1".to_string(),
            }],
            ..remote_panel(RemoteProvider::DeepInfra)
        };
        let (window, view, recorded) = open_backend_tab(cx, panel);

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("remote-sample-state-deepinfra").is_some());
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
                view.set_backend_panel(remote_panel(RemoteProvider::DeepInfra), cx)
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
            ..remote_panel(RemoteProvider::DeepInfra)
        };
        panel.errors.insert(
            BackendArea::RemoteSample(RemoteProvider::DeepInfra),
            "DeepInfra: internal".to_string(),
        );
        let (window, _view, _recorded) = open_backend_tab(cx, panel);

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("remote-sample-error-deepinfra").is_some());
            assert!(window.try_find("remote-sample-delete-deepinfra").is_some());
        })
        .unwrap();
    }

    /// Choosing another entry in the `Select` asks the root to select it,
    /// once; re-confirming the current entry sends nothing.
    #[gpui_kit::test]
    fn choosing_a_backend_asks_the_root(cx: &mut TestAppContext) {
        let (window, view, recorded) = open_backend_tab(cx, cuda_panel());

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let select = view.read(cx).backend_select.clone();
            select.update(cx, |_, cx| {
                // Re-confirming the current selection is not a change.
                cx.emit(SelectEvent::Confirm(Some(cuda_panel().selection)));
                cx.emit(SelectEvent::Confirm(Some(BackendSelection::BUNDLED_CPU)));
            });
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(
            *recorded.borrow(),
            vec![BackendAction::Select(BackendSelection::BUNDLED_CPU)]
        );
    }

    /// The `Select` lists only existing backends of the kind shown.
    #[test]
    fn each_kind_lists_only_its_own_backends() {
        let runtimes = cuda_panel().runtimes;
        let local: Vec<_> = choices(BackendKind::Local, &runtimes)
            .into_iter()
            .map(|choice| choice.selection)
            .collect();
        assert_eq!(local[0], BackendSelection::BUNDLED_CPU);
        assert_eq!(local[1..], runtimes[0].entries()[..]);
        let remote: Vec<_> = choices(BackendKind::Remote, &runtimes)
            .into_iter()
            .map(|choice| choice.selection)
            .collect();
        assert_eq!(
            remote,
            RemoteProvider::ALL
                .into_iter()
                .map(BackendSelection::Remote)
                .collect::<Vec<_>>()
        );
    }

    /// A failed save resyncs the `Select` to the saved entry, and the error
    /// is shown next to it.
    #[gpui_kit::test]
    fn a_failed_selection_resyncs_the_select(cx: &mut TestAppContext) {
        let (window, view, _recorded) = open_backend_tab(cx, cuda_panel());

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let select = view.read(cx).backend_select.clone();
            select.update(cx, |select, cx| {
                select.set_selected_value(&BackendSelection::BUNDLED_CPU, window, cx)
            });
            let mut panel = cuda_panel();
            panel
                .errors
                .insert(BackendArea::Selection, "Could not save.".to_string());
            view.update(cx, |view, cx| view.set_backend_panel(panel, cx));
            window.render_frame(cx);
            assert!(window.try_find("backend-error-selection").is_some());
            assert_eq!(
                select.read(cx).selected_value(),
                Some(&cuda_panel().selection)
            );
        })
        .unwrap();
    }

    fn deepinfra_sample() -> RemoteSample {
        RemoteSample {
            provider: RemoteProvider::DeepInfra,
            sample_sha256: "aa".to_string(),
            voice_id: "v1".to_string(),
        }
    }

    /// Flip the shown kind by clicking the kind `RadioGroup`.
    fn flip_to(window: &mut Window, kind: BackendKind, cx: &mut App) {
        window.render_frame(cx);
        window.within("backend-kind").click(kind.index(), cx);
        window.render_frame(cx);
    }

    /// The kind choice has a visible heading, as the second step does.
    #[gpui_kit::test]
    fn the_kind_choice_has_a_heading(cx: &mut TestAppContext) {
        let (window, _view, _recorded) = open_backend_tab(cx, BackendPanel::default());

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("backend-kind-heading").is_some());
        })
        .unwrap();
    }

    /// A flipped kind survives a later push that leaves the saved
    /// selection as it was (here, "Active" changing).
    #[gpui_kit::test]
    fn a_flipped_kind_survives_an_unrelated_panel_push(cx: &mut TestAppContext) {
        let (window, view, recorded) = open_backend_tab(cx, BackendPanel::default());

        cx.update_window(window.into(), |_, window, cx| {
            flip_to(window, BackendKind::Remote, cx);
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
            assert_eq!(view.read(cx).kind, BackendKind::Remote);
            assert_eq!(
                view.read(cx).backend_select.read(cx).selected_value(),
                None,
                "the Select stays on the Remote placeholder"
            );
            assert!(window.try_find("backend-runtimes").is_none());
        })
        .unwrap();
        cx.run_until_parked();

        assert!(recorded.borrow().is_empty());
    }

    /// A runtime added while the Local kind is shown rebuilds the Local
    /// `Select`, so its entries can be chosen.
    #[gpui_kit::test]
    fn an_added_runtime_appears_in_the_local_select(cx: &mut TestAppContext) {
        let (window, view, _recorded) = open_backend_tab(cx, BackendPanel::default());
        let cuda = cuda_panel();
        let cuda_entry = cuda.selection.clone();

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            // Same saved selection (CPU), one more runtime.
            view.update(cx, |view, cx| {
                view.set_backend_panel(
                    BackendPanel {
                        runtimes: cuda.runtimes.clone(),
                        ..BackendPanel::default()
                    },
                    cx,
                )
            });
            window.render_frame(cx);
            assert!(window.try_find("backend-runtime-0").is_some());

            let select = view.read(cx).backend_select.clone();
            select.update(cx, |select, cx| {
                select.set_selected_value(&cuda_entry, window, cx)
            });
            assert_eq!(
                select.read(cx).selected_value(),
                Some(&cuda_entry),
                "the runtime's entry is among the Select's items"
            );
        })
        .unwrap();
    }

    /// Decision 2: with a remote backend saved, the Local kind still shows
    /// the added runtimes and Add runtime…, with probe errors inline, and
    /// no key field.
    #[gpui_kit::test]
    fn the_local_kind_shows_runtimes_while_a_remote_backend_is_saved(cx: &mut TestAppContext) {
        let mut panel = remote_panel(RemoteProvider::DeepInfra);
        panel.runtimes = cuda_panel().runtimes;
        panel.errors.insert(
            BackendArea::Runtimes,
            "Not an ONNX Runtime library.".to_string(),
        );
        let (window, view, recorded) = open_backend_tab(cx, panel);

        cx.update_window(window.into(), |_, window, cx| {
            flip_to(window, BackendKind::Local, cx);
            assert_eq!(view.read(cx).kind, BackendKind::Local);
            assert_eq!(view.read(cx).backend_select.read(cx).selected_value(), None);
            assert!(window.try_find("backend-runtimes").is_some());
            assert!(window.try_find("backend-runtime-0").is_some());
            assert!(window.try_find("backend-add-runtime").is_some());
            assert!(window.try_find("backend-error-runtimes").is_some());
            assert!(window.try_find("backend-api-keys").is_none());
            assert!(window.try_find("api-key-save-deepinfra").is_none());
            assert!(window.try_find("remote-sample-state-deepinfra").is_none());
        })
        .unwrap();
        cx.run_until_parked();

        assert!(recorded.borrow().is_empty(), "flipping saves nothing");
    }

    /// Decision 2: CPU saved, DeepInfra holding the sample — the Remote
    /// kind shows a DeepInfra line whose delete asks the root once.
    #[gpui_kit::test]
    fn a_sample_held_by_an_unsaved_provider_can_be_deleted(cx: &mut TestAppContext) {
        let panel = BackendPanel {
            remote_samples: vec![deepinfra_sample()],
            ..BackendPanel::default()
        };
        let (window, view, recorded) = open_backend_tab(cx, panel);

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("backend-other-deepinfra").is_none(),
                "the line belongs to the Remote kind"
            );
            flip_to(window, BackendKind::Remote, cx);
            assert!(window.try_find("backend-other-deepinfra").is_some());
            assert!(
                window
                    .try_find("backend-other-remove-key-deepinfra")
                    .is_none()
            );
            assert!(window.try_find("backend-other-fal-ai").is_none());
            assert!(window.try_find("api-key-save-deepinfra").is_none());
            window.click("backend-other-delete-sample-deepinfra", cx);
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(
            *recorded.borrow(),
            vec![BackendAction::DeleteRemoteSample(RemoteProvider::DeepInfra)]
        );

        // In flight, then failed: both shown on that line.
        cx.update_window(window.into(), |_, window, cx| {
            let mut panel = BackendPanel {
                remote_samples: vec![deepinfra_sample()],
                deleting_samples: vec![RemoteProvider::DeepInfra],
                ..BackendPanel::default()
            };
            view.update(cx, |view, cx| view.set_backend_panel(panel.clone(), cx));
            window.render_frame(cx);
            assert_eq!(view.read(cx).kind, BackendKind::Remote);
            assert_eq!(
                window.find("backend-other-delete-sample-deepinfra").label(),
                Some("Deleting…")
            );
            // Disabled while it runs: a click is refused. (The harness does
            // not report `disabled` for this Button, so the refusal is what
            // is checked.)
            window.click("backend-other-delete-sample-deepinfra", cx);

            panel.deleting_samples.clear();
            panel.errors.insert(
                BackendArea::RemoteSample(RemoteProvider::DeepInfra),
                "DeepInfra: internal".to_string(),
            );
            view.update(cx, |view, cx| view.set_backend_panel(panel, cx));
            window.render_frame(cx);
            assert!(window.try_find("backend-other-error-deepinfra").is_some());
            assert_eq!(
                window.find("backend-other-delete-sample-deepinfra").label(),
                Some("Delete from DeepInfra"),
                "the delete is offered again once it has finished"
            );
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(
            recorded.borrow().len(),
            1,
            "a click while deleting sends nothing"
        );
    }

    /// Decision 2: DeepInfra saved, fal.ai holding a key — fal.ai gets a
    /// line (not a key field) whose Remove key asks the root once.
    #[gpui_kit::test]
    fn a_key_saved_for_an_unsaved_provider_can_be_removed(cx: &mut TestAppContext) {
        let mut panel = remote_panel(RemoteProvider::DeepInfra);
        panel
            .api_keys
            .set(RemoteProvider::FalAi, Some("fal-secret".to_string()));
        panel.errors.insert(
            BackendArea::ApiKey(RemoteProvider::FalAi),
            "Could not save.".to_string(),
        );
        let (window, _view, recorded) = open_backend_tab(cx, panel);

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("api-key-save-deepinfra").is_some());
            assert!(window.try_find("api-key-save-fal-ai").is_none());
            assert!(window.try_find("backend-other-fal-ai").is_some());
            assert!(
                window.try_find("backend-other-deepinfra").is_none(),
                "the saved provider has its full options, not a line"
            );
            assert!(
                window
                    .try_find("backend-other-delete-sample-fal-ai")
                    .is_none()
            );
            assert!(window.try_find("backend-other-error-fal-ai").is_some());
            window.click("backend-other-remove-key-fal-ai", cx);
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(
            *recorded.borrow(),
            vec![BackendAction::SaveApiKey(RemoteProvider::FalAi, None)]
        );
    }

    /// Decision 2: a provider that is not saved but holds both a key and
    /// the sample gets one line offering both ways back.
    #[gpui_kit::test]
    fn a_provider_holding_a_key_and_the_sample_offers_both(cx: &mut TestAppContext) {
        let mut panel = BackendPanel {
            remote_samples: vec![deepinfra_sample()],
            ..BackendPanel::default()
        };
        panel
            .api_keys
            .set(RemoteProvider::DeepInfra, Some("di-secret".to_string()));
        let (window, _view, _recorded) = open_backend_tab(cx, panel);

        cx.update_window(window.into(), |_, window, cx| {
            flip_to(window, BackendKind::Remote, cx);
            assert!(window.try_find("backend-other-deepinfra").is_some());
            assert!(window.try_find("backend-other-state-deepinfra").is_some());
            // The state line is a plain `div`, whose text the harness
            // cannot read; the two buttons' labels it can.
            assert_eq!(
                window.find("backend-other-remove-key-deepinfra").label(),
                Some("Remove key")
            );
            assert_eq!(
                window.find("backend-other-delete-sample-deepinfra").label(),
                Some("Delete from DeepInfra")
            );
            assert!(window.try_find("backend-other-fal-ai").is_none());
        })
        .unwrap();
    }
}
