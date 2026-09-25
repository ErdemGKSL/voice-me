//! Settings → Backend (Story 3.10): which backend speaks for the user, and
//! that backend's own options.
//!
//! The tab is two-step. The user first chooses **Local** or **Remote**, then
//! one backend of that kind from a `Select`. Below that it shows only the
//! *saved* backend's options: the provider's masked key and held voice
//! sample for a remote one, each below that backend's speech language
//! (Story 3.11), and for the System voice a voice picker when its language
//! has several voices (Story 3.12). Azure (Story 3.14) adds its region, and
//! a locale and voice picker fed from its live voice list. Last come the *Selected* and *Active* lines,
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
    ActiveBackend, ApiKeys, BackendSelection, CheckRequest, LanguageBackend, LocalEngine,
    LocalRuntime, RemoteProvider, RemoteSample, SpeechLanguages, SpeechVoices, StockVoice,
    device_choices, engine_choices, parse_azure_region, stock_voices_of, switch_engine,
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
    /// Save this backend's speech language (Story 3.11).
    SetSpeechLanguage(LanguageBackend, String),
    /// Save this backend's voice, or `None` for its language's top-priority
    /// one (Story 3.12).
    SetSpeechVoice(LanguageBackend, Option<String>),
    /// Save (`Some`, already `[a-z0-9]+`) or remove (`None`) the Azure
    /// region (Story 3.14).
    SaveAzureRegion(Option<String>),
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
            BackendAction::SetSpeechLanguage(backend, code) => f
                .debug_tuple("SetSpeechLanguage")
                .field(backend)
                .field(code)
                .finish(),
            BackendAction::SetSpeechVoice(backend, voice) => f
                .debug_tuple("SetSpeechVoice")
                .field(backend)
                .field(voice)
                .finish(),
            // Not a secret, but written out like the others.
            BackendAction::SaveAzureRegion(region) => {
                f.debug_tuple("SaveAzureRegion").field(region).finish()
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
    /// The saved backend's speech language (Story 3.11).
    SpeechLanguage,
    /// The saved backend's voice (Story 3.12).
    SpeechVoice,
    /// The Azure region (Story 3.14).
    AzureRegion,
}

/// Everything the Backend and Dependencies tabs show about the backend, as
/// the composition root last computed it.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendPanel {
    pub selection: BackendSelection,
    pub runtimes: Vec<LocalRuntime>,
    /// Whether the bundled runtime has every execution provider (Story
    /// 3.8), so its CUDA and WebGPU devices are listed
    /// (spec-backend-engine-and-device-selects).
    pub bundled_all_providers: bool,
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
    /// Each backend's saved speech language (Story 3.11).
    pub speech_languages: SpeechLanguages,
    /// Each backend's saved voice (Story 3.12).
    pub speech_voices: SpeechVoices,
    /// The voices the System voice's engine listed at the last check that
    /// had it selected (Story 3.12). Not persisted; held by the root.
    pub system_voices: Vec<StockVoice>,
    /// The saved Azure region (Story 3.14).
    pub azure_region: Option<String>,
    /// Azure's voice list as the root last fetched it (Story 3.14). Not
    /// persisted; cached by the root for the session.
    pub azure_voices: Vec<StockVoice>,
    /// The voices `edge-tts --list-voices` listed at the last check with
    /// Edge TTS selected (Story 3.17). Not persisted; held by the root.
    pub edge_tts_voices: Vec<StockVoice>,
    /// The Piper voices installed in the cache (Story 3.15): Piper's
    /// language and voice pickers list only these.
    pub piper_voices: Vec<StockVoice>,
}

impl BackendPanel {
    /// The run-time voice list `backend`'s languages and voices come from:
    /// the System voice's engine list, Azure's voice list, Edge TTS's
    /// voice list, or nothing.
    fn stock_voices(&self, backend: LanguageBackend) -> &[StockVoice] {
        match backend {
            LanguageBackend::SystemVoice => &self.system_voices,
            LanguageBackend::Remote(RemoteProvider::Azure) => &self.azure_voices,
            LanguageBackend::Remote(RemoteProvider::EdgeTts) => &self.edge_tts_voices,
            LanguageBackend::Piper => &self.piper_voices,
            _ => &[],
        }
    }
}

impl Default for BackendPanel {
    fn default() -> Self {
        Self {
            selection: BackendSelection::BUNDLED_CPU,
            runtimes: Vec::new(),
            bundled_all_providers: false,
            api_keys: ApiKeys::default(),
            active: ActiveBackend::NotStarted,
            restart_pending: false,
            probing: None,
            check_request: CheckRequest::cpu(),
            errors: HashMap::new(),
            remote_samples: Vec::new(),
            deleting_samples: Vec::new(),
            speech_languages: SpeechLanguages::default(),
            speech_voices: SpeechVoices::default(),
            system_voices: Vec::new(),
            azure_region: None,
            azure_voices: Vec::new(),
            edge_tts_voices: Vec::new(),
            piper_voices: Vec::new(),
        }
    }
}

/// Emitted by "Manage voices" (Story 3.15); the Settings shell switches to
/// the Piper voices tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenPiperVoicesTab;

/// What the Backend tab says while no Piper voice is installed.
pub const PIPER_NO_VOICE_NOTE: &str = "No Piper voice installed — Manage voices";

/// The line under Azure's options (Story 3.14): a stock voice, not the
/// user's.
pub const AZURE_STOCK_VOICE_NOTE: &str = "Speech will be in this Microsoft voice, not yours.";

/// The line under Edge TTS's options (Story 3.17): a stock voice, not the
/// user's.
pub const EDGE_TTS_STOCK_VOICE_NOTE: &str = "Speech will be in this Microsoft voice, not yours.";

/// What the Backend tab says while Azure has no voice saved (D4).
pub const AZURE_PICK_A_VOICE: &str = "Pick a voice for Azure";

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
            BackendSelection::Local { .. }
            | BackendSelection::SystemVoice
            | BackendSelection::Piper { .. } => BackendKind::Local,
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
            BackendKind::Remote => "Remote — online providers",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            BackendKind::Local => "Choose a local engine",
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

/// What the backend `Select` lists: a local engine, whose device is a
/// second `Select` (spec-backend-engine-and-device-selects), or a remote
/// provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendEntry {
    Engine(LocalEngine),
    Remote(RemoteProvider),
}

impl BackendEntry {
    /// The entry `selection` is listed under.
    fn of(selection: &BackendSelection) -> Self {
        match selection {
            BackendSelection::Remote(provider) => BackendEntry::Remote(*provider),
            BackendSelection::Local { .. } => BackendEntry::Engine(LocalEngine::Chatterbox),
            BackendSelection::Piper { .. } => BackendEntry::Engine(LocalEngine::Piper),
            BackendSelection::SystemVoice => BackendEntry::Engine(LocalEngine::SystemVoice),
        }
    }

    fn label(self) -> String {
        match self {
            BackendEntry::Engine(engine) => engine.label().to_string(),
            BackendEntry::Remote(provider) => BackendSelection::Remote(provider).label(),
        }
    }
}

/// One entry of the backend `Select`.
#[derive(Clone)]
struct BackendChoice {
    entry: BackendEntry,
    label: SharedString,
}

impl gpui_kit::component::searchable_list::SearchableListItem for BackendChoice {
    type Value = BackendEntry;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.entry
    }
}

/// One entry of the device `Select`: the engine on one device.
#[derive(Clone)]
struct DeviceChoice {
    selection: BackendSelection,
    label: SharedString,
}

impl gpui_kit::component::searchable_list::SearchableListItem for DeviceChoice {
    type Value = BackendSelection;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.selection
    }
}

/// The devices the saved selection's engine can run on — empty for the
/// System voice and a remote provider, which have none.
fn device_items(panel: &BackendPanel) -> Vec<DeviceChoice> {
    let Some(engine) = panel.selection.engine() else {
        return Vec::new();
    };
    device_choices(engine, &panel.runtimes, panel.bundled_all_providers)
        .into_iter()
        .map(|selection| DeviceChoice {
            label: selection.device_label().unwrap_or_default().into(),
            selection,
        })
        .collect()
}

/// One entry of the speech-language `Select`: the code the model gets, and
/// the English name the user reads.
#[derive(Clone)]
struct LanguageChoice {
    code: String,
    label: SharedString,
}

impl gpui_kit::component::searchable_list::SearchableListItem for LanguageChoice {
    type Value = String;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.code
    }
}

/// The speech languages `backend` lists, in its own order — for a backend
/// with a voice list, the languages of the voices it listed.
fn language_choices(backend: LanguageBackend, voices: &[StockVoice]) -> Vec<LanguageChoice> {
    backend
        .language_options(voices)
        .into_iter()
        .map(|language| LanguageChoice {
            code: language.code,
            label: language.label.into(),
        })
        .collect()
}

/// The code of `backend`'s saved language, if it is one `backend` speaks.
/// A value outside the set is `None` — the `Select` then shows its
/// placeholder — never a stand-in.
fn saved_language_code(panel: &BackendPanel, backend: LanguageBackend) -> Option<String> {
    panel
        .speech_languages
        .get(backend)
        .and_then(|saved| backend.resolve_language(saved, panel.stock_voices(backend)))
}

/// One entry of the voice `Select` (Story 3.12).
#[derive(Clone)]
struct VoiceChoice {
    id: String,
    label: SharedString,
}

impl gpui_kit::component::searchable_list::SearchableListItem for VoiceChoice {
    type Value = String;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.id
    }
}

/// What the voice picker shows for the saved backend's language, when that
/// backend has a voice choice (the System voice, Azure): its voices, top
/// priority first, and which one is in effect — the stored one if it is one
/// of them, with none stored the top-priority one (the System voice) or
/// none at all (Azure, D4), and `None` (the placeholder) for a stored voice
/// that is not the language's. `None` altogether when the saved language is
/// not one listed.
struct VoicePick {
    choices: Vec<VoiceChoice>,
    selected: Option<String>,
}

fn voice_pick(panel: &BackendPanel) -> Option<VoicePick> {
    let backend = panel.selection.language_backend();
    if !backend.has_voice_choice() {
        return None;
    }
    let code = saved_language_code(panel, backend)?;
    let voices = stock_voices_of(panel.stock_voices(backend), &code);
    let stored = panel.speech_voices.get(backend);
    let selected = match stored {
        None if backend.requires_voice() => None,
        None => voices.first().map(|voice| voice.id.clone()),
        Some(id) => voices
            .iter()
            .find(|voice| voice.id == id)
            .map(|voice| voice.id.clone()),
    };
    Some(VoicePick {
        choices: voices
            .into_iter()
            .map(|voice| VoiceChoice {
                id: voice.id.clone(),
                label: voice.name.clone().into(),
            })
            .collect(),
        selected,
    })
}

/// The entries of one kind: the local engines (without a device), or
/// every remote provider.
fn choices(kind: BackendKind) -> Vec<BackendChoice> {
    let entries: Vec<BackendEntry> = match kind {
        BackendKind::Local => engine_choices()
            .into_iter()
            .map(BackendEntry::Engine)
            .collect(),
        BackendKind::Remote => RemoteProvider::ALL
            .into_iter()
            .map(BackendEntry::Remote)
            .collect(),
    };
    entries
        .into_iter()
        .map(|entry| BackendChoice {
            label: entry.label().into(),
            entry,
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
    /// The saved engine's device (spec-backend-engine-and-device-selects),
    /// shown under Piper and Chatterbox.
    device_select: Entity<SelectState<Vec<DeviceChoice>>>,
    /// The saved backend's speech language (Story 3.11).
    language_select: Entity<SelectState<Vec<LanguageChoice>>>,
    /// Whose languages `language_select` currently lists.
    language_backend: LanguageBackend,
    /// The System voice's voice (Story 3.12), shown when its language has
    /// several voices, and Azure's (Story 3.14).
    voice_select: Entity<SelectState<Vec<VoiceChoice>>>,
    key_inputs: Vec<(RemoteProvider, Entity<InputState>)>,
    /// The Azure region field (Story 3.14).
    region_input: Entity<InputState>,
    /// A panel (or kind) change since the last render that still has to
    /// reach the `Select` and the inputs — which need the window, and so
    /// are synced at the next render.
    panel_stale: bool,
    keys_stale: bool,
    /// The saved Azure region changed, so the field is overwritten.
    region_stale: bool,
    items_stale: bool,
    /// The language `Select` needs its items or value brought in line with
    /// the panel at the next render.
    language_stale: bool,
    /// The System voice's list changed, so the language items are rebuilt
    /// even though the backend did not change.
    language_items_stale: bool,
    /// The voice `Select` needs its items and value brought in line.
    voice_stale: bool,
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
        let items = choices(kind);
        let saved_entry = BackendEntry::of(&panel.selection);
        let selected = items
            .iter()
            .position(|choice| choice.entry == saved_entry)
            .map(IndexPath::new);
        let backend_select = cx.new(|cx| SelectState::new(items, selected, window, cx));
        let subscription = cx.subscribe(&backend_select, |this, _select, event, cx| {
            let SelectEvent::Confirm(Some(entry)) = event else {
                return;
            };
            // Re-confirming the saved engine or provider is not a change.
            if *entry == BackendEntry::of(&this.panel.selection) {
                return;
            }
            let selection = match *entry {
                // Decision 4: the device is kept when the new engine offers
                // it, otherwise CPU.
                BackendEntry::Engine(engine) => switch_engine(
                    &this.panel.selection,
                    engine,
                    &this.panel.runtimes,
                    this.panel.bundled_all_providers,
                ),
                BackendEntry::Remote(provider) => BackendSelection::Remote(provider),
            };
            this.act(BackendAction::Select(selection), cx);
        });

        let device_items = device_items(&panel);
        let device_selected = device_items
            .iter()
            .position(|choice| choice.selection == panel.selection)
            .map(IndexPath::new);
        let device_select =
            cx.new(|cx| SelectState::new(device_items, device_selected, window, cx));
        let device_subscription = cx.subscribe(&device_select, |this, _select, event, cx| {
            let SelectEvent::Confirm(Some(selection)) = event else {
                return;
            };
            // Only a device of the saved engine is ever saved from here.
            if *selection != this.panel.selection
                && selection.engine() == this.panel.selection.engine()
            {
                this.act(BackendAction::Select(selection.clone()), cx);
            }
        });

        let language_backend = panel.selection.language_backend();
        let language_items =
            language_choices(language_backend, panel.stock_voices(language_backend));
        let language_selected = saved_language_code(&panel, language_backend).and_then(|code| {
            language_items
                .iter()
                .position(|choice| choice.code == code)
                .map(IndexPath::new)
        });
        let language_select =
            cx.new(|cx| SelectState::new(language_items, language_selected, window, cx));
        let language_subscription = cx.subscribe(&language_select, |this, _select, event, cx| {
            let SelectEvent::Confirm(Some(code)) = event else {
                return;
            };
            let backend = this.panel.selection.language_backend();
            // Items of a backend that is no longer the saved one are
            // never saved under the new one.
            if backend != this.language_backend {
                return;
            }
            if Some(code) != saved_language_code(&this.panel, backend).as_ref() {
                this.act(BackendAction::SetSpeechLanguage(backend, code.clone()), cx);
            }
        });

        let pick = voice_pick(&panel);
        let (voice_items, voice_selected) = match pick {
            Some(pick) => {
                let index = pick.selected.as_ref().and_then(|id| {
                    pick.choices
                        .iter()
                        .position(|choice| &choice.id == id)
                        .map(IndexPath::new)
                });
                (pick.choices, index)
            }
            None => (Vec::new(), None),
        };
        let voice_select = cx.new(|cx| SelectState::new(voice_items, voice_selected, window, cx));
        let voice_subscription = cx.subscribe(&voice_select, |this, _select, event, cx| {
            let SelectEvent::Confirm(Some(id)) = event else {
                return;
            };
            // The voice in effect — the stored one, or the top-priority one
            // with none stored — is not a change.
            let Some(pick) = voice_pick(&this.panel) else {
                return;
            };
            if !pick.choices.iter().any(|choice| &choice.id == id) {
                return;
            }
            if pick.selected.as_ref() != Some(id) {
                let backend = this.panel.selection.language_backend();
                this.act(BackendAction::SetSpeechVoice(backend, Some(id.clone())), cx);
            }
        });

        // Story 3.17: a keyless provider (Edge TTS) gets no key field.
        let key_inputs = RemoteProvider::ALL
            .into_iter()
            .filter(|provider| provider.needs_api_key())
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

        let region_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Azure region, like westeurope")
                .default_value(panel.azure_region.clone().unwrap_or_default())
        });

        Self {
            panel,
            actions,
            kind,
            backend_select,
            device_select,
            language_select,
            language_backend,
            voice_select,
            key_inputs,
            region_input,
            panel_stale: false,
            keys_stale: false,
            region_stale: false,
            items_stale: false,
            language_stale: false,
            language_items_stale: false,
            voice_stale: false,
            _subscriptions: vec![
                subscription,
                device_subscription,
                language_subscription,
                voice_subscription,
            ],
        }
    }

    /// Replace what the tab shows. Called by the composition root after
    /// every backend change, and when "Active" changes.
    pub fn set_backend_panel(&mut self, panel: BackendPanel, cx: &mut Context<Self>) {
        // A key field is only overwritten when the *saved* key changed (a
        // save, a removal) — never merely because some other part of the
        // panel did, which would wipe whatever the user is typing.
        let keys_changed = panel.api_keys != self.panel.api_keys;
        let runtimes_changed = panel.runtimes != self.panel.runtimes
            || panel.bundled_all_providers != self.panel.bundled_all_providers;
        let selection_changed = panel.selection != self.panel.selection;
        // A failed save leaves the selection unchanged, but the `Select`
        // still shows the entry the user picked: resync it to what is saved.
        let selection_failed = panel.errors.contains_key(&BackendArea::Selection);
        // The same for the language `Select`: a failed save leaves the
        // picked language showing until it is resynced to the saved one.
        let voices_changed = panel.system_voices != self.panel.system_voices
            || panel.azure_voices != self.panel.azure_voices
            || panel.edge_tts_voices != self.panel.edge_tts_voices
            || panel.piper_voices != self.panel.piper_voices;
        let region_changed = panel.azure_region != self.panel.azure_region;
        self.language_items_stale |= voices_changed;
        self.language_stale |= panel.speech_languages != self.panel.speech_languages
            || panel.selection.language_backend() != self.language_backend
            || voices_changed
            || panel.errors.contains_key(&BackendArea::SpeechLanguage);
        // Story 3.12: the same for the voice `Select`, which follows the
        // language as well as the stored voice.
        self.voice_stale |= panel.speech_voices != self.panel.speech_voices
            || panel.speech_languages != self.panel.speech_languages
            || panel.selection != self.panel.selection
            || voices_changed
            || panel.errors.contains_key(&BackendArea::SpeechVoice);

        // Decision 1: the kind follows the saved selection when it changes.
        if selection_changed {
            let kind = BackendKind::of(&panel.selection);
            self.items_stale |= kind != self.kind;
            self.kind = kind;
        }

        self.panel_stale |= keys_changed
            || region_changed
            || runtimes_changed
            || selection_failed
            || selection_changed;
        self.keys_stale |= keys_changed;
        self.region_stale |= region_changed;
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

    /// The saved backend's language backend, while its kind is shown and it
    /// has a speech language — the only case in which the language `Select`
    /// appears (3.10 Decision 1).
    fn shown_language_backend(&self) -> Option<LanguageBackend> {
        let backend = self.panel.selection.language_backend();
        (BackendKind::of(&self.panel.selection) == self.kind && backend.has_speech_language())
            .then_some(backend)
    }

    /// Bring the language `Select` in line with the panel: the saved
    /// backend's list, with its saved language chosen, or the placeholder.
    fn sync_language(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.language_stale) {
            return;
        }
        let backend = self.panel.selection.language_backend();
        let rebuild =
            backend != self.language_backend || std::mem::take(&mut self.language_items_stale);
        let items = rebuild.then(|| language_choices(backend, self.panel.stock_voices(backend)));
        self.language_backend = backend;
        let saved = saved_language_code(&self.panel, backend);
        self.language_select.update(cx, |select, cx| {
            if let Some(items) = items {
                select.set_items(items, window, cx);
            }
            match saved {
                Some(code) => select.set_selected_value(&code, window, cx),
                None => select.set_selected_index(None, window, cx),
            }
        });
    }

    /// Bring the voice `Select` in line with the panel: the saved backend's
    /// language's voices, with the one in effect chosen.
    fn sync_voice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.voice_stale) {
            return;
        }
        let (items, selected) = match voice_pick(&self.panel) {
            Some(pick) => (pick.choices, pick.selected),
            None => (Vec::new(), None),
        };
        self.voice_select.update(cx, |select, cx| {
            select.set_items(items, window, cx);
            match selected {
                Some(id) => select.set_selected_value(&id, window, cx),
                None => select.set_selected_index(None, window, cx),
            }
        });
    }

    /// Whether the voice picker is shown: a backend with a voice choice is
    /// the saved one, its kind is shown, and its language is listed. The
    /// System voice shows it only when the language has several voices —
    /// or a stored voice that is not the language's, which has to be
    /// fixable from here; Azure always does, since its voice must be
    /// chosen (D4).
    fn shows_voice_picker(&self) -> bool {
        let Some(backend) = self
            .shown_language_backend()
            .filter(|backend| backend.has_voice_choice())
        else {
            return false;
        };
        voice_pick(&self.panel).is_some_and(|pick| {
            backend.requires_voice() || pick.choices.len() > 1 || pick.selected.is_none()
        })
    }

    /// Bring the `Select` and the key inputs in line with the panel.
    fn sync_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_language(window, cx);
        self.sync_voice(window, cx);
        if !self.panel_stale {
            return;
        }
        self.panel_stale = false;

        let selection = self.panel.selection.clone();
        let items = std::mem::take(&mut self.items_stale).then(|| choices(self.kind));
        self.backend_select.update(cx, |select, cx| {
            if let Some(items) = items {
                select.set_items(items, window, cx);
            }
            // The saved entry is absent from the other kind's list, which
            // leaves the `Select` empty, on its placeholder.
            select.set_selected_value(&BackendEntry::of(&selection), window, cx);
        });
        // The saved engine's devices, with the saved one chosen.
        let devices = device_items(&self.panel);
        self.device_select.update(cx, |select, cx| {
            select.set_items(devices, window, cx);
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

        if std::mem::take(&mut self.region_stale) {
            let saved = self.panel.azure_region.clone().unwrap_or_default();
            self.region_input.update(cx, |input, cx| {
                if input.value().as_ref() != saved {
                    input.set_value(saved, window, cx);
                }
            });
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

    /// Story 3.14: the region is checked here first, so a bad one is an
    /// inline error and is never sent; the root checks it again.
    fn save_region(&mut self, cx: &mut Context<Self>) {
        let value = self.region_input.read(cx).value().to_string();
        match parse_azure_region(&value) {
            Ok(region) => {
                self.panel.errors.remove(&BackendArea::AzureRegion);
                self.act(BackendAction::SaveAzureRegion(region), cx);
            }
            Err(reason) => {
                self.panel.errors.insert(BackendArea::AzureRegion, reason);
                cx.notify();
            }
        }
    }

    /// Whether the device `Select` is shown: the Local kind with Piper or
    /// Chatterbox saved (spec-backend-engine-and-device-selects). Never for
    /// the System voice or a remote backend.
    fn shows_device_select(&self) -> bool {
        self.kind == BackendKind::Local
            && self
                .panel
                .selection
                .engine()
                .is_some_and(LocalEngine::has_device)
    }

    /// Step one and two: the kind, then a backend of that kind — and, for
    /// an ONNX engine, the device it runs on.
    fn choice_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let kind = self.kind;
        let shows_device = self.shows_device_select();
        // A failed save is shown under the last `Select` of the two.
        let error = self.panel.errors.get(&BackendArea::Selection).cloned();
        let error_line_for_selection = |cx: &mut Context<Self>| {
            error
                .clone()
                .map(|error| error_line("backend-error-selection", error, cx))
        };
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
                    .when(!shows_device, |el| {
                        el.children(error_line_for_selection(cx))
                    }),
            )
            .when(shows_device, |el| {
                el.child(
                    v_flex()
                        .id("backend-device")
                        .test_support()
                        .gap_1()
                        .child(div().font_weight(FontWeight::MEDIUM).child("Device"))
                        .child(
                            Select::new(&self.device_select)
                                .id("backend-device-select")
                                .accessibility_label("Device")
                                .placeholder("Choose a device")
                                .menu_width(px(360.))
                                .w(px(360.)),
                        )
                        .children(error_line_for_selection(cx)),
                )
            })
            .into_any_element()
    }

    /// The options of the kind being shown. Local: the added runtimes,
    /// whichever backend is saved (Decision 2). Remote: the saved
    /// provider's key and sample sections — none while the saved backend
    /// is of the other kind (Decision 1) — then one short line for every
    /// other provider holding a key or the voice sample (Decision 2).
    fn options_section(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        // Story 3.11: the saved backend's speech language sits above its
        // other options.
        let language = self
            .shown_language_backend()
            .map(|backend| self.speech_language_section(backend, cx));
        // Story 3.12: the System voice's voice, under its language.
        let voice = self
            .shows_voice_picker()
            .then(|| self.speech_voice_section(cx));
        // Story 3.15: Piper's voices are managed on their own tab.
        let manage_voices = (self.kind == BackendKind::Local && self.panel.selection.is_piper())
            .then(|| {
                h_flex()
                    .child(
                        Button::new("backend-manage-piper-voices")
                            .ghost()
                            .label("Manage voices")
                            .on_click(
                                cx.listener(|_this, _, _window, cx| cx.emit(OpenPiperVoicesTab)),
                            ),
                    )
                    .into_any_element()
            });
        match self.kind {
            BackendKind::Local => Some(
                v_flex()
                    .gap_6()
                    .children(language)
                    .children(voice)
                    .children(manage_voices)
                    .child(self.runtimes_section(cx))
                    .into_any_element(),
            ),
            BackendKind::Remote => {
                let saved = self.shown_saved_provider();
                let others: Vec<AnyElement> = RemoteProvider::ALL
                    .into_iter()
                    .filter(|provider| Some(*provider) != saved)
                    .filter_map(|provider| self.other_provider_line(provider, cx))
                    .collect();
                if saved.is_none() && others.is_empty() && language.is_none() {
                    return None;
                }
                // Story 3.14: Azure's key, then its region, then the locale
                // and voice its list offers, and what it speaks in.
                if saved == Some(RemoteProvider::Azure) {
                    return Some(
                        v_flex()
                            .gap_6()
                            .child(self.api_key_section(RemoteProvider::Azure, cx))
                            .child(self.azure_region_section(cx))
                            .children(language)
                            .children(voice)
                            .child(self.azure_voice_status(cx))
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
                    );
                }
                // Story 3.17: Edge TTS has no key and holds no sample — its
                // locale and voice, and what it speaks in.
                if saved == Some(RemoteProvider::EdgeTts) {
                    return Some(
                        v_flex()
                            .gap_6()
                            .children(language)
                            .children(voice)
                            .child(
                                div()
                                    .id("backend-edge-tts-stock-note")
                                    .test_support()
                                    .text_size(px(12.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(EDGE_TTS_STOCK_VOICE_NOTE),
                            )
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
                    );
                }
                Some(
                    v_flex()
                        .gap_6()
                        .children(language)
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

    /// Story 3.11: the saved backend's speech language — only the languages
    /// that backend speaks — with a note when the saved value is none of
    /// them, and a failed save inline.
    fn speech_language_section(
        &self,
        backend: LanguageBackend,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let unknown = saved_language_code(&self.panel, backend).is_none();
        let note = speech_language_note(&self.panel, backend);

        v_flex()
            .id("backend-speech-language")
            .test_support()
            .gap_1()
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child("Speech language"),
            )
            .child(
                Select::new(&self.language_select)
                    .id("backend-speech-language-select")
                    .accessibility_label("Speech language")
                    .placeholder("Choose a speech language")
                    .menu_width(px(360.))
                    .w(px(360.)),
            )
            .when(unknown, |el| {
                el.child(
                    div()
                        .id("backend-speech-language-note")
                        .test_support()
                        .text_size(px(12.))
                        .text_color(cx.theme().muted_foreground)
                        .child(note),
                )
            })
            .when_some(
                self.panel.errors.get(&BackendArea::SpeechLanguage).cloned(),
                |el, error| el.child(error_line("backend-speech-language-error", error, cx)),
            )
            .into_any_element()
    }

    /// Stories 3.12, 3.14: the saved backend's voice for its saved
    /// language, with a note when the stored voice is not one of that
    /// language's, and a failed save inline.
    fn speech_voice_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let stored = self
            .panel
            .speech_voices
            .get(self.panel.selection.language_backend())
            .map(str::to_string);
        // No stored voice is not a stale one: Azure's "Pick a voice" line
        // says that instead.
        let unknown =
            stored.is_some() && voice_pick(&self.panel).is_some_and(|pick| pick.selected.is_none());

        v_flex()
            .id("backend-speech-voice")
            .test_support()
            .gap_1()
            .child(div().font_weight(FontWeight::MEDIUM).child("Voice"))
            .child(
                Select::new(&self.voice_select)
                    .id("backend-speech-voice-select")
                    .accessibility_label("Voice")
                    .placeholder("Choose a voice")
                    .menu_width(px(360.))
                    .w(px(360.)),
            )
            .when(unknown, |el| {
                el.child(
                    div()
                        .id("backend-speech-voice-note")
                        .test_support()
                        .text_size(px(12.))
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "The saved voice {:?} is not one of this language's. Choose one to \
                             speak again.",
                            stored.unwrap_or_default()
                        )),
                )
            })
            .when_some(
                self.panel.errors.get(&BackendArea::SpeechVoice).cloned(),
                |el, error| el.child(error_line("backend-speech-voice-error", error, cx)),
            )
            .into_any_element()
    }

    /// Story 3.14: the Azure region field, with Save, and an inline error.
    fn azure_region_section(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .id("backend-azure-region")
            .test_support()
            .gap_1()
            .child(div().font_weight(FontWeight::MEDIUM).child("Azure region"))
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(360.)).child(Input::new(&self.region_input)))
                    .child(
                        Button::new("azure-region-save")
                            .label("Save")
                            .on_click(cx.listener(|this, _, _window, cx| this.save_region(cx))),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child("The region of your Azure Speech resource, like westeurope."),
            )
            .when_some(
                self.panel.errors.get(&BackendArea::AzureRegion).cloned(),
                |el, error| el.child(error_line("azure-region-error", error, cx)),
            )
            .into_any_element()
    }

    /// Story 3.14: "Pick a voice for Azure" while none is saved, and that
    /// the speech is a Microsoft voice, not the user's.
    fn azure_voice_status(&self, cx: &mut Context<Self>) -> AnyElement {
        let backend = LanguageBackend::Remote(RemoteProvider::Azure);
        let no_voice = self.panel.speech_voices.get(backend).is_none();
        v_flex()
            .gap_1()
            .when(no_voice, |el| {
                el.child(
                    div()
                        .id("backend-azure-pick-voice")
                        .test_support()
                        .child(AZURE_PICK_A_VOICE),
                )
            })
            .child(
                div()
                    .id("backend-azure-stock-note")
                    .test_support()
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child(AZURE_STOCK_VOICE_NOTE),
            )
            .into_any_element()
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
                    })
                    // Story 3.12: speech in a stock voice, not the user's.
                    .when(panel.selection.is_stock_voice(), |el| {
                        el.child(
                            div()
                                .id("backend-stock-voice")
                                .test_support()
                                .child(Tag::secondary().small().child("Stock voice")),
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

/// The note beside a speech language the saved backend cannot speak.
fn speech_language_note(panel: &BackendPanel, backend: LanguageBackend) -> String {
    // Nothing listed is not the same as a wrong value.
    if backend == LanguageBackend::SystemVoice && panel.system_voices.is_empty() {
        return "The System voice has not listed its voices yet. See Settings → Backend."
            .to_string();
    }
    if backend == LanguageBackend::Piper && panel.piper_voices.is_empty() {
        return PIPER_NO_VOICE_NOTE.to_string();
    }
    if backend == LanguageBackend::Remote(RemoteProvider::Azure) && panel.azure_voices.is_empty() {
        return "Azure's voices have not been listed yet. They are fetched once its key and \
                region are saved."
            .to_string();
    }
    // Story 3.17: a missing program is the Dependencies row's to say; a
    // failed listing is the error beside this language.
    if backend == LanguageBackend::Remote(RemoteProvider::EdgeTts)
        && panel.edge_tts_voices.is_empty()
    {
        return "Edge TTS's voices are not listed yet. They are listed by the edge-tts program \
             at the next check."
            .to_string();
    }
    let saved = panel.speech_languages.get(backend).unwrap_or_default();
    format!(
        "The saved speech language {saved:?} is not one {} speaks. Choose one to speak again.",
        backend.label()
    )
}

impl gpui_kit::EventEmitter<OpenPiperVoicesTab> for BackendView {}

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
        RemoteProvider::Azure => "azure",
        RemoteProvider::EdgeTts => "edge-tts",
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
                Some(&BackendEntry::Engine(LocalEngine::Chatterbox))
            );
            assert!(window.try_find("backend-device-select").is_some());
            assert_eq!(
                view.read(cx).device_select.read(cx).selected_value(),
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
            assert!(
                window.try_find("backend-device-select").is_none(),
                "a remote backend has no device"
            );
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
            assert!(window.try_find("backend-device-select").is_none());
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
                Some(&BackendEntry::Engine(LocalEngine::Chatterbox))
            );
            assert!(window.try_find("backend-device-select").is_some());
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
                Some(&BackendEntry::Remote(RemoteProvider::DeepInfra))
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

    /// Choosing another engine asks the root to select it, once, on the
    /// device the engine offers; re-confirming the current engine sends
    /// nothing. A remote provider is selected as itself.
    #[gpui_kit::test]
    fn choosing_an_engine_asks_the_root(cx: &mut TestAppContext) {
        let (window, view, recorded) = open_backend_tab(cx, cuda_panel());

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let select = view.read(cx).backend_select.clone();
            select.update(cx, |_, cx| {
                // Re-confirming the current engine is not a change.
                cx.emit(SelectEvent::Confirm(Some(BackendEntry::Engine(
                    LocalEngine::Chatterbox,
                ))));
                // The added runtime's CUDA is Chatterbox's only, and the
                // bundled runtime here offers CPU only: Piper on CPU.
                cx.emit(SelectEvent::Confirm(Some(BackendEntry::Engine(
                    LocalEngine::Piper,
                ))));
                cx.emit(SelectEvent::Confirm(Some(BackendEntry::Engine(
                    LocalEngine::SystemVoice,
                ))));
                cx.emit(SelectEvent::Confirm(Some(BackendEntry::Remote(
                    RemoteProvider::Azure,
                ))));
            });
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(
            *recorded.borrow(),
            vec![
                BackendAction::Select(BackendSelection::PIPER_CPU),
                BackendAction::Select(BackendSelection::SystemVoice),
                BackendAction::Select(BackendSelection::Remote(RemoteProvider::Azure)),
            ]
        );
    }

    /// The Engine switch row: Chatterbox on WebGPU → Piper keeps WebGPU
    /// when the bundled runtime has every provider.
    #[gpui_kit::test]
    fn switching_engine_keeps_the_device(cx: &mut TestAppContext) {
        let panel = BackendPanel {
            selection: BackendSelection::Local {
                runtime: None,
                target: voice_me_core::SpeechExecutionTarget::WebGpu,
            },
            bundled_all_providers: true,
            ..BackendPanel::default()
        };
        let (window, view, recorded) = open_backend_tab(cx, panel);

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let select = view.read(cx).backend_select.clone();
            select.update(cx, |_, cx| {
                cx.emit(SelectEvent::Confirm(Some(BackendEntry::Engine(
                    LocalEngine::Piper,
                ))));
            });
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(
            *recorded.borrow(),
            vec![BackendAction::Select(BackendSelection::Piper {
                target: voice_me_core::SpeechExecutionTarget::WebGpu
            })]
        );
    }

    /// The device `Select` lists the saved engine's devices; confirming
    /// another saves the engine on it, once.
    #[gpui_kit::test]
    fn the_device_select_lists_and_saves_the_engines_devices(cx: &mut TestAppContext) {
        let panel = BackendPanel {
            bundled_all_providers: true,
            ..cuda_panel()
        };
        let (window, view, recorded) = open_backend_tab(cx, panel);

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("backend-device-select").is_some());
            let devices: Vec<String> = device_items(&view.read(cx).panel)
                .into_iter()
                .map(|choice| choice.label.to_string())
                .collect();
            assert_eq!(
                devices,
                vec!["CPU", "CUDA", "WebGPU", "CUDA — libonnxruntime.so"]
            );
            let select = view.read(cx).device_select.clone();
            assert_eq!(
                select.read(cx).selected_value(),
                Some(&cuda_panel().selection)
            );
            select.update(cx, |_, cx| {
                // The saved device is not a change.
                cx.emit(SelectEvent::Confirm(Some(cuda_panel().selection)));
                cx.emit(SelectEvent::Confirm(Some(BackendSelection::Local {
                    runtime: None,
                    target: voice_me_core::SpeechExecutionTarget::WebGpu,
                })));
            });
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(
            *recorded.borrow(),
            vec![BackendAction::Select(BackendSelection::Local {
                runtime: None,
                target: voice_me_core::SpeechExecutionTarget::WebGpu,
            })]
        );
    }

    /// Piper's devices: CPU only until the bundled runtime has every
    /// provider, then CPU, CUDA and WebGPU — never an added runtime's. The
    /// System voice has no device `Select`.
    #[gpui_kit::test]
    fn the_device_select_follows_the_engine(cx: &mut TestAppContext) {
        let mut panel = piper_panel(&[], None);
        panel.runtimes = cuda_panel().runtimes;
        let (window, view, _recorded) = open_backend_tab(cx, panel.clone());

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                view.read(cx).backend_select.read(cx).selected_value(),
                Some(&BackendEntry::Engine(LocalEngine::Piper))
            );
            assert!(window.try_find("backend-device-select").is_some());
            let labels = |view: &Entity<BackendView>, cx: &App| -> Vec<String> {
                device_items(&view.read(cx).panel)
                    .into_iter()
                    .map(|choice| choice.label.to_string())
                    .collect()
            };
            assert_eq!(labels(&view, cx), vec!["CPU"]);

            view.update(cx, |view, cx| {
                view.set_backend_panel(
                    BackendPanel {
                        bundled_all_providers: true,
                        ..panel.clone()
                    },
                    cx,
                )
            });
            window.render_frame(cx);
            assert_eq!(labels(&view, cx), vec!["CPU", "CUDA", "WebGPU"]);
            assert_eq!(
                view.read(cx).device_select.read(cx).selected_value(),
                Some(&BackendSelection::PIPER_CPU)
            );

            view.update(cx, |view, cx| {
                view.set_backend_panel(
                    BackendPanel {
                        selection: BackendSelection::SystemVoice,
                        ..BackendPanel::default()
                    },
                    cx,
                )
            });
            window.render_frame(cx);
            assert!(window.try_find("backend-device-select").is_none());
        })
        .unwrap();
    }

    /// The Engine list row: the Local `Select` lists the engines without a
    /// device; Remote lists every provider.
    #[test]
    fn each_kind_lists_only_its_own_backends() {
        let local: Vec<_> = choices(BackendKind::Local)
            .into_iter()
            .map(|choice| (choice.entry, choice.label.to_string()))
            .collect();
        assert_eq!(
            local,
            vec![
                (
                    BackendEntry::Engine(LocalEngine::Piper),
                    "Piper — natural, instant (stock voice)".to_string()
                ),
                (
                    BackendEntry::Engine(LocalEngine::Chatterbox),
                    "Chatterbox — your voice".to_string()
                ),
                (
                    BackendEntry::Engine(LocalEngine::SystemVoice),
                    "System voice — instant (stock voice)".to_string()
                ),
            ]
        );
        let remote: Vec<_> = choices(BackendKind::Remote)
            .into_iter()
            .map(|choice| choice.entry)
            .collect();
        assert_eq!(
            remote,
            RemoteProvider::ALL
                .into_iter()
                .map(BackendEntry::Remote)
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
            let select = view.read(cx).device_select.clone();
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

    /// A runtime added while the Local kind is shown rebuilds the device
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

            let select = view.read(cx).device_select.clone();
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

    fn with_languages(panel: BackendPanel, local: &str, deepinfra: &str) -> BackendPanel {
        BackendPanel {
            speech_languages: SpeechLanguages {
                local: local.to_string(),
                deepinfra: deepinfra.to_string(),
                ..SpeechLanguages::default()
            },
            ..panel
        }
    }

    fn codes(backend: LanguageBackend) -> Vec<String> {
        language_choices(backend, &[])
            .into_iter()
            .map(|choice| choice.code)
            .collect()
    }

    /// Story 3.11, Local saved: the language `Select` lists Turkish then
    /// English, with the saved one chosen, above the runtimes.
    #[gpui_kit::test]
    fn a_local_backend_lists_its_two_languages_with_the_saved_one_selected(
        cx: &mut TestAppContext,
    ) {
        let (window, view, _recorded) = open_backend_tab(cx, BackendPanel::default());

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("backend-speech-language").is_some());
            assert!(window.try_find("backend-speech-language-note").is_none());
            assert_eq!(view.read(cx).language_backend, LanguageBackend::Local);
            assert_eq!(codes(LanguageBackend::Local), vec!["tr", "en"]);
            let select = view.read(cx).language_select.clone();
            assert_eq!(select.read(cx).selected_value(), Some(&"tr".to_string()));

            // Only Local's languages are among the items.
            select.update(cx, |select, cx| {
                select.set_selected_value(&"es".to_string(), window, cx)
            });
            assert_eq!(select.read(cx).selected_value(), None);
        })
        .unwrap();

        // Every local entry shares the Local language.
        let (window, view, _recorded) =
            open_backend_tab(cx, with_languages(cuda_panel(), "en", "tr"));
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                view.read(cx).language_select.read(cx).selected_value(),
                Some(&"en".to_string())
            );
        })
        .unwrap();
    }

    /// Story 3.11, DeepInfra saved: 23 languages, the saved one chosen, and
    /// the section sits with DeepInfra's key field.
    #[gpui_kit::test]
    fn deepinfra_lists_its_23_languages_with_the_saved_one_selected(cx: &mut TestAppContext) {
        let panel = with_languages(remote_panel(RemoteProvider::DeepInfra), "tr", "en");
        let (window, view, _recorded) = open_backend_tab(cx, panel);

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("backend-speech-language").is_some());
            assert!(window.try_find("api-key-save-deepinfra").is_some());
            let backend = LanguageBackend::Remote(RemoteProvider::DeepInfra);
            assert_eq!(view.read(cx).language_backend, backend);
            assert_eq!(codes(backend).len(), 23);
            let select = view.read(cx).language_select.clone();
            assert_eq!(select.read(cx).selected_value(), Some(&"en".to_string()));
            select.update(cx, |select, cx| {
                select.set_selected_value(&"es".to_string(), window, cx)
            });
            assert_eq!(select.read(cx).selected_value(), Some(&"es".to_string()));
        })
        .unwrap();
    }

    /// Switching the saved backend swaps the list and the chosen language.
    #[gpui_kit::test]
    fn a_new_saved_backend_brings_its_own_languages(cx: &mut TestAppContext) {
        let (window, view, _recorded) =
            open_backend_tab(cx, with_languages(BackendPanel::default(), "en", "es"));

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            view.update(cx, |view, cx| {
                view.set_backend_panel(
                    with_languages(remote_panel(RemoteProvider::DeepInfra), "en", "es"),
                    cx,
                )
            });
            window.render_frame(cx);
            assert_eq!(
                view.read(cx).language_backend,
                LanguageBackend::Remote(RemoteProvider::DeepInfra)
            );
            assert_eq!(
                view.read(cx).language_select.read(cx).selected_value(),
                Some(&"es".to_string())
            );

            view.update(cx, |view, cx| {
                view.set_backend_panel(with_languages(BackendPanel::default(), "en", "es"), cx)
            });
            window.render_frame(cx);
            assert_eq!(
                view.read(cx).language_select.read(cx).selected_value(),
                Some(&"en".to_string())
            );
        })
        .unwrap();
    }

    /// Picking a language asks the root once; re-confirming the saved one
    /// sends nothing.
    #[gpui_kit::test]
    fn picking_a_language_asks_the_root_once(cx: &mut TestAppContext) {
        let panel = with_languages(remote_panel(RemoteProvider::DeepInfra), "tr", "en");
        let (window, view, recorded) = open_backend_tab(cx, panel);

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let select = view.read(cx).language_select.clone();
            select.update(cx, |_, cx| {
                cx.emit(SelectEvent::Confirm(Some("en".to_string())));
                cx.emit(SelectEvent::Confirm(Some("es".to_string())));
            });
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(
            *recorded.borrow(),
            vec![BackendAction::SetSpeechLanguage(
                LanguageBackend::Remote(RemoteProvider::DeepInfra),
                "es".to_string()
            )]
        );
    }

    /// 3.10 Decision 1: after a flip the saved backend's options, its
    /// language included, are not shown. fal.ai has no language at all.
    #[gpui_kit::test]
    fn no_language_select_after_a_flip_or_for_fal_ai(cx: &mut TestAppContext) {
        let (window, _view, _recorded) = open_backend_tab(cx, BackendPanel::default());
        cx.update_window(window.into(), |_, window, cx| {
            flip_to(window, BackendKind::Remote, cx);
            assert!(window.try_find("backend-speech-language").is_none());
            flip_to(window, BackendKind::Local, cx);
            assert!(window.try_find("backend-speech-language").is_some());
        })
        .unwrap();

        let (window, _view, _recorded) =
            open_backend_tab(cx, remote_panel(RemoteProvider::DeepInfra));
        cx.update_window(window.into(), |_, window, cx| {
            flip_to(window, BackendKind::Local, cx);
            assert!(window.try_find("backend-speech-language").is_none());
            assert!(window.try_find("backend-runtimes").is_some());
        })
        .unwrap();

        let (window, _view, _recorded) = open_backend_tab(cx, remote_panel(RemoteProvider::FalAi));
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("backend-speech-language").is_none());
            assert!(window.try_find("api-key-save-fal-ai").is_some());
        })
        .unwrap();
    }

    /// The Out of set row: a hand-edited value outside the set leaves the
    /// `Select` on its placeholder, with a note naming the value.
    #[gpui_kit::test]
    fn a_saved_language_outside_the_set_shows_the_placeholder_and_a_note(cx: &mut TestAppContext) {
        let (window, view, recorded) =
            open_backend_tab(cx, with_languages(BackendPanel::default(), "es", "tr"));

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                view.read(cx).language_select.read(cx).selected_value(),
                None
            );
            assert!(window.try_find("backend-speech-language-note").is_some());

            // Picking a real one is a change, and is sent.
            let select = view.read(cx).language_select.clone();
            select.update(cx, |_, cx| {
                cx.emit(SelectEvent::Confirm(Some("tr".to_string())))
            });
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(
            *recorded.borrow(),
            vec![BackendAction::SetSpeechLanguage(
                LanguageBackend::Local,
                "tr".to_string()
            )]
        );
    }

    /// A failed save is shown under the `Select`, which resyncs to the
    /// saved language.
    #[gpui_kit::test]
    fn a_failed_language_save_is_inline_and_resyncs_the_select(cx: &mut TestAppContext) {
        let (window, view, _recorded) = open_backend_tab(cx, BackendPanel::default());

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let select = view.read(cx).language_select.clone();
            select.update(cx, |select, cx| {
                select.set_selected_value(&"en".to_string(), window, cx)
            });
            let mut panel = BackendPanel::default();
            panel
                .errors
                .insert(BackendArea::SpeechLanguage, "Could not save.".to_string());
            view.update(cx, |view, cx| view.set_backend_panel(panel, cx));
            window.render_frame(cx);
            assert!(window.try_find("backend-speech-language-error").is_some());
            assert_eq!(select.read(cx).selected_value(), Some(&"tr".to_string()));
        })
        .unwrap();
    }

    fn espeak_voice(id: &str, language: &str, name: &str) -> StockVoice {
        StockVoice {
            id: id.to_string(),
            language: language.to_string(),
            language_label: name.to_string(),
            name: name.to_string(),
            priority: 5,
        }
    }

    /// The System voice saved, speaking `language` with `voice` stored,
    /// against a small eSpeak-shaped list.
    fn system_voice_panel(language: &str, voice: Option<&str>) -> BackendPanel {
        BackendPanel {
            selection: BackendSelection::SystemVoice,
            speech_languages: SpeechLanguages {
                system_voice: language.to_string(),
                ..SpeechLanguages::default()
            },
            speech_voices: SpeechVoices {
                system_voice: voice.map(str::to_string),
                ..SpeechVoices::default()
            },
            system_voices: vec![
                espeak_voice("gmw/en-US", "en-us", "English (America)"),
                espeak_voice("trk/tr", "tr", "Turkish"),
                espeak_voice("sit/yue", "yue", "Chinese (Cantonese)"),
                espeak_voice(
                    "sit/yue-Latn-jyutping",
                    "yue",
                    "Chinese (Cantonese, latin as Jyutping)",
                ),
            ],
            ..BackendPanel::default()
        }
    }

    const JYUTPING: &str = "sit/yue-Latn-jyutping";

    /// Story 3.12, Several voices: the language `Select` lists the
    /// engine's languages, the voice picker lists yue's two voices with
    /// the top-priority one selected, and Selected carries the Stock voice
    /// tag instead of CPU mode.
    #[gpui_kit::test]
    fn several_voices_show_the_picker_with_the_default_selected(cx: &mut TestAppContext) {
        let (window, view, recorded) = open_backend_tab(cx, system_voice_panel("yue", None));

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(view.read(cx).kind, BackendKind::Local);
            assert_eq!(
                view.read(cx).backend_select.read(cx).selected_value(),
                Some(&BackendEntry::Engine(LocalEngine::SystemVoice))
            );
            assert!(window.try_find("backend-device-select").is_none());
            assert!(window.try_find("backend-stock-voice").is_some());
            assert!(window.try_find("backend-cpu-mode").is_none());
            assert!(window.try_find("backend-speech-language").is_some());
            assert!(window.try_find("backend-speech-language-note").is_none());
            assert_eq!(
                view.read(cx).language_select.read(cx).selected_value(),
                Some(&"yue".to_string())
            );
            assert!(window.try_find("backend-speech-voice").is_some());
            assert!(window.try_find("backend-speech-voice-select").is_some());
            assert!(window.try_find("backend-speech-voice-note").is_none());
            let voice = view.read(cx).voice_select.clone();
            assert_eq!(
                voice.read(cx).selected_value(),
                Some(&"sit/yue".to_string())
            );
            // Both of yue's voices are among the items, and nothing else.
            voice.update(cx, |select, cx| {
                select.set_selected_value(&JYUTPING.to_string(), window, cx);
            });
            assert_eq!(voice.read(cx).selected_value(), Some(&JYUTPING.to_string()));
            voice.update(cx, |select, cx| {
                select.set_selected_value(&"trk/tr".to_string(), window, cx);
            });
            assert_eq!(voice.read(cx).selected_value(), None);
        })
        .unwrap();
        cx.run_until_parked();

        assert!(
            recorded.borrow().is_empty(),
            "showing the picker saves nothing"
        );
    }

    /// A language with one voice has no picker.
    #[gpui_kit::test]
    fn a_single_voice_hides_the_picker(cx: &mut TestAppContext) {
        let (window, view, _recorded) = open_backend_tab(cx, system_voice_panel("tr", None));

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("backend-speech-language").is_some());
            assert_eq!(
                view.read(cx).language_select.read(cx).selected_value(),
                Some(&"tr".to_string())
            );
            assert!(window.try_find("backend-speech-voice").is_none());
            assert!(window.try_find("backend-stock-voice").is_some());
        })
        .unwrap();
    }

    /// The Pick a voice row: another voice is sent once; re-confirming the
    /// one in effect (the unset default) sends nothing.
    #[gpui_kit::test]
    fn picking_a_voice_asks_the_root_once(cx: &mut TestAppContext) {
        let (window, view, recorded) = open_backend_tab(cx, system_voice_panel("yue", None));

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let voice = view.read(cx).voice_select.clone();
            voice.update(cx, |_, cx| {
                cx.emit(SelectEvent::Confirm(Some("sit/yue".to_string())));
                cx.emit(SelectEvent::Confirm(Some(JYUTPING.to_string())));
            });
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(
            *recorded.borrow(),
            vec![BackendAction::SetSpeechVoice(
                LanguageBackend::SystemVoice,
                Some(JYUTPING.to_string())
            )]
        );

        // The root saved it: the picker follows, and re-picking it sends
        // nothing more.
        cx.update_window(window.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.set_backend_panel(system_voice_panel("yue", Some(JYUTPING)), cx)
            });
            window.render_frame(cx);
            let voice = view.read(cx).voice_select.clone();
            assert_eq!(voice.read(cx).selected_value(), Some(&JYUTPING.to_string()));
            voice.update(cx, |_, cx| {
                cx.emit(SelectEvent::Confirm(Some(JYUTPING.to_string())))
            });
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(recorded.borrow().len(), 1);
    }

    /// The Change language row: picking a language sends only the language
    /// action — the root clears the voice — and once saved, a one-voice
    /// language hides the picker.
    #[gpui_kit::test]
    fn a_language_change_sends_only_the_language_action(cx: &mut TestAppContext) {
        let (window, view, recorded) =
            open_backend_tab(cx, system_voice_panel("yue", Some(JYUTPING)));

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let language = view.read(cx).language_select.clone();
            language.update(cx, |_, cx| {
                cx.emit(SelectEvent::Confirm(Some("tr".to_string())))
            });
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(
            *recorded.borrow(),
            vec![BackendAction::SetSpeechLanguage(
                LanguageBackend::SystemVoice,
                "tr".to_string()
            )]
        );

        cx.update_window(window.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.set_backend_panel(system_voice_panel("tr", None), cx)
            });
            window.render_frame(cx);
            assert!(window.try_find("backend-speech-voice").is_none());
            assert_eq!(
                view.read(cx).language_select.read(cx).selected_value(),
                Some(&"tr".to_string())
            );
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(recorded.borrow().len(), 1, "the resync sends nothing");
    }

    /// The Out of set row in the tab: a stored voice that is not the
    /// language's leaves the picker on its placeholder with a note, even
    /// for a one-voice language; an unlisted language gets the language
    /// note; and a failed voice save is shown inline.
    #[gpui_kit::test]
    fn an_unlisted_voice_or_language_shows_the_placeholder_and_a_note(cx: &mut TestAppContext) {
        let (window, view, _recorded) =
            open_backend_tab(cx, system_voice_panel("tr", Some("sit/yue")));
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("backend-speech-voice").is_some());
            assert!(window.try_find("backend-speech-voice-note").is_some());
            assert_eq!(view.read(cx).voice_select.read(cx).selected_value(), None);

            let mut panel = system_voice_panel("yue", None);
            panel
                .errors
                .insert(BackendArea::SpeechVoice, "Could not save.".to_string());
            view.update(cx, |view, cx| view.set_backend_panel(panel, cx));
            window.render_frame(cx);
            assert!(window.try_find("backend-speech-voice-error").is_some());
            assert!(window.try_find("backend-speech-voice-note").is_none());
        })
        .unwrap();

        let (window, view, _recorded) = open_backend_tab(cx, system_voice_panel("xx", None));
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                view.read(cx).language_select.read(cx).selected_value(),
                None
            );
            assert!(window.try_find("backend-speech-language-note").is_some());
            assert!(window.try_find("backend-speech-voice").is_none());
            let note = speech_language_note(&view.read(cx).panel, LanguageBackend::SystemVoice);
            assert!(note.contains("\"xx\""), "{note}");
        })
        .unwrap();

        // Nothing listed yet (the check has not run): the language note,
        // no picker, and the list arriving later fills the `Select`.
        let empty = BackendPanel {
            system_voices: Vec::new(),
            ..system_voice_panel("tr", None)
        };
        let (window, view, _recorded) = open_backend_tab(cx, empty);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("backend-speech-language-note").is_some());
            let note = speech_language_note(&view.read(cx).panel, LanguageBackend::SystemVoice);
            assert!(note.contains("Settings → Backend"), "{note}");
            assert!(!note.contains("eSpeak"), "{note}");
            view.update(cx, |view, cx| {
                view.set_backend_panel(system_voice_panel("tr", None), cx)
            });
            window.render_frame(cx);
            assert!(window.try_find("backend-speech-language-note").is_none());
            assert_eq!(
                view.read(cx).language_select.read(cx).selected_value(),
                Some(&"tr".to_string())
            );
        })
        .unwrap();
    }

    fn azure_voice(short_name: &str, locale: &str, locale_name: &str, local: &str) -> StockVoice {
        StockVoice {
            id: short_name.to_string(),
            language: locale.to_string(),
            language_label: locale_name.to_string(),
            name: format!("{local} (Female)"),
            priority: 0,
        }
    }

    const EMEL: &str = "tr-TR-EmelNeural";
    const AZURE: LanguageBackend = LanguageBackend::Remote(RemoteProvider::Azure);

    /// Azure saved with a key and `westeurope`, speaking `locale` with
    /// `voice` stored, against a small fetched list.
    fn azure_panel(locale: &str, voice: Option<&str>) -> BackendPanel {
        let mut api_keys = ApiKeys::default();
        api_keys.set(RemoteProvider::Azure, Some("az-key".to_string()));
        BackendPanel {
            selection: BackendSelection::Remote(RemoteProvider::Azure),
            api_keys,
            azure_region: Some("westeurope".to_string()),
            speech_languages: SpeechLanguages {
                azure: locale.to_string(),
                ..SpeechLanguages::default()
            },
            speech_voices: SpeechVoices {
                azure: voice.map(str::to_string),
                ..SpeechVoices::default()
            },
            azure_voices: vec![
                azure_voice(EMEL, "tr-TR", "Turkish (Türkiye)", "Emel"),
                azure_voice("tr-TR-AhmetNeural", "tr-TR", "Turkish (Türkiye)", "Ahmet"),
                azure_voice(
                    "en-US-JennyNeural",
                    "en-US",
                    "English (United States)",
                    "Jenny",
                ),
            ],
            ..BackendPanel::default()
        }
    }

    /// Story 3.14: Azure's options — key, region, locale, voice, the
    /// Microsoft-voice line and the Stock voice tag — and no sample line.
    #[gpui_kit::test]
    fn azure_shows_its_region_locale_and_voice_and_no_sample_line(cx: &mut TestAppContext) {
        let (window, view, recorded) = open_backend_tab(cx, azure_panel("tr-TR", Some(EMEL)));

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(view.read(cx).kind, BackendKind::Remote);
            assert!(window.try_find("api-key-save-azure").is_some());
            assert!(window.try_find("backend-azure-region").is_some());
            assert!(window.try_find("azure-region-save").is_some());
            assert!(window.try_find("backend-speech-language").is_some());
            assert_eq!(
                view.read(cx).language_select.read(cx).selected_value(),
                Some(&"tr-TR".to_string())
            );
            assert!(window.try_find("backend-speech-voice").is_some());
            assert_eq!(
                view.read(cx).voice_select.read(cx).selected_value(),
                Some(&EMEL.to_string())
            );
            assert!(window.try_find("backend-azure-stock-note").is_some());
            assert!(window.try_find("backend-azure-pick-voice").is_none());
            assert!(window.try_find("backend-stock-voice").is_some());
            assert!(window.try_find("remote-sample-state-azure").is_none());
            assert!(window.try_find("backend-remote-samples").is_none());
            assert_eq!(
                view.read(cx).region_input.read(cx).value().as_ref(),
                "westeurope"
            );
        })
        .unwrap();
        cx.run_until_parked();
        assert!(recorded.borrow().is_empty());
    }

    /// The No voice row in the tab: the picker is on its placeholder, with
    /// "Pick a voice for Azure" and no stale-voice note.
    #[gpui_kit::test]
    fn azure_with_no_voice_says_pick_a_voice(cx: &mut TestAppContext) {
        let (window, view, _recorded) = open_backend_tab(cx, azure_panel("tr-TR", None));

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("backend-speech-voice").is_some());
            assert_eq!(view.read(cx).voice_select.read(cx).selected_value(), None);
            assert!(window.try_find("backend-azure-pick-voice").is_some());
            assert!(window.try_find("backend-speech-voice-note").is_none());
        })
        .unwrap();
    }

    /// Picking a voice asks the root once, under Azure.
    #[gpui_kit::test]
    fn picking_an_azure_voice_asks_the_root(cx: &mut TestAppContext) {
        let (window, view, recorded) = open_backend_tab(cx, azure_panel("tr-TR", None));

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let voice = view.read(cx).voice_select.clone();
            voice.update(cx, |_, cx| {
                cx.emit(SelectEvent::Confirm(Some(EMEL.to_string())))
            });
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(
            *recorded.borrow(),
            vec![BackendAction::SetSpeechVoice(AZURE, Some(EMEL.to_string()))]
        );
    }

    /// The Change locale row: only the language action is sent; once the
    /// root saved it with the voice cleared, the tab says to pick a voice.
    #[gpui_kit::test]
    fn an_azure_locale_change_sends_only_the_language_action(cx: &mut TestAppContext) {
        let (window, view, recorded) = open_backend_tab(cx, azure_panel("tr-TR", Some(EMEL)));

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let language = view.read(cx).language_select.clone();
            language.update(cx, |_, cx| {
                cx.emit(SelectEvent::Confirm(Some("en-US".to_string())))
            });
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(
            *recorded.borrow(),
            vec![BackendAction::SetSpeechLanguage(AZURE, "en-US".to_string())]
        );

        cx.update_window(window.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.set_backend_panel(azure_panel("en-US", None), cx)
            });
            window.render_frame(cx);
            assert!(window.try_find("backend-azure-pick-voice").is_some());
            assert_eq!(view.read(cx).voice_select.read(cx).selected_value(), None);
            assert_eq!(
                view.read(cx).language_select.read(cx).selected_value(),
                Some(&"en-US".to_string())
            );
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(recorded.borrow().len(), 1, "the resync sends nothing");
    }

    /// The Stale voice row in the tab: placeholder and note.
    #[gpui_kit::test]
    fn a_stale_azure_voice_shows_the_placeholder_and_a_note(cx: &mut TestAppContext) {
        let (window, view, _recorded) =
            open_backend_tab(cx, azure_panel("tr-TR", Some("tr-TR-GoneNeural")));

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(view.read(cx).voice_select.read(cx).selected_value(), None);
            assert!(window.try_find("backend-speech-voice-note").is_some());
        })
        .unwrap();
    }

    /// The Bad region row: an invalid region is an inline error and is
    /// never sent; a valid one is sent trimmed and lowercased.
    #[gpui_kit::test]
    fn the_azure_region_is_checked_before_it_is_saved(cx: &mut TestAppContext) {
        let (window, view, recorded) = open_backend_tab(cx, azure_panel("tr-TR", Some(EMEL)));

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let input = view.read(cx).region_input.clone();
            input.update(cx, |input, cx| input.set_value("west europe!", window, cx));
            window.render_frame(cx);
            window.click("azure-region-save", cx);
            window.render_frame(cx);
            assert!(window.try_find("azure-region-error").is_some());

            input.update(cx, |input, cx| input.set_value(" NorthEurope ", window, cx));
            window.render_frame(cx);
            window.click("azure-region-save", cx);
            window.render_frame(cx);
            assert!(window.try_find("azure-region-error").is_none());
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(
            *recorded.borrow(),
            vec![BackendAction::SaveAzureRegion(Some(
                "northeurope".to_string()
            ))]
        );
    }

    /// No list yet: the language note says so, and there is no picker.
    #[gpui_kit::test]
    fn azure_with_no_list_yet_says_so(cx: &mut TestAppContext) {
        let panel = BackendPanel {
            azure_voices: Vec::new(),
            ..azure_panel("tr-TR", None)
        };
        let (window, view, _recorded) = open_backend_tab(cx, panel);

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("backend-speech-language-note").is_some());
            assert!(window.try_find("backend-speech-voice").is_none());
            assert!(window.try_find("backend-azure-pick-voice").is_some());
            let note = speech_language_note(&view.read(cx).panel, AZURE);
            assert!(note.contains("Azure"), "{note}");
        })
        .unwrap();
    }

    fn edge_voice(id: &str, locale: &str, name: &str, priority: u32) -> StockVoice {
        StockVoice {
            id: id.to_string(),
            language: locale.to_string(),
            language_label: locale.to_string(),
            name: name.to_string(),
            priority,
        }
    }

    /// Edge TTS saved, speaking `locale` with `voice` stored, against a
    /// small list as `edge-tts --list-voices` gives it.
    fn edge_tts_panel(locale: &str, voice: Option<&str>) -> BackendPanel {
        BackendPanel {
            selection: BackendSelection::Remote(RemoteProvider::EdgeTts),
            speech_languages: SpeechLanguages {
                edge_tts: locale.to_string(),
                ..SpeechLanguages::default()
            },
            speech_voices: SpeechVoices {
                edge_tts: voice.map(str::to_string),
                ..SpeechVoices::default()
            },
            edge_tts_voices: vec![
                edge_voice("en-US-AvaMultilingualNeural", "en-US", "Ava (Female)", 0),
                edge_voice("tr-TR-AhmetNeural", "tr-TR", "Ahmet (Male)", 1),
                edge_voice(EMEL, "tr-TR", "Emel (Female)", 2),
            ],
            ..BackendPanel::default()
        }
    }

    /// Story 3.17: Edge TTS has no key field and no sample line; its
    /// locale and voice come from its list, with the first listed voice in
    /// effect, and it says it speaks in a Microsoft voice.
    #[gpui_kit::test]
    fn edge_tts_shows_its_locale_and_voice_and_no_key(cx: &mut TestAppContext) {
        let (window, view, recorded) = open_backend_tab(cx, edge_tts_panel("tr-TR", None));

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(view.read(cx).kind, BackendKind::Remote);
            assert!(
                !view
                    .read(cx)
                    .key_inputs
                    .iter()
                    .any(|(provider, _)| *provider == RemoteProvider::EdgeTts)
            );
            assert!(window.try_find("api-key-save-edge-tts").is_none());
            assert!(window.try_find("backend-api-keys").is_none());
            assert!(window.try_find("api-key-plaintext-notice").is_none());
            assert!(window.try_find("backend-remote-samples").is_none());
            assert!(window.try_find("backend-speech-language").is_some());
            assert_eq!(
                view.read(cx).language_select.read(cx).selected_value(),
                Some(&"tr-TR".to_string())
            );
            assert!(window.try_find("backend-speech-voice").is_some());
            assert_eq!(
                view.read(cx).voice_select.read(cx).selected_value(),
                Some(&"tr-TR-AhmetNeural".to_string()),
                "the language's first listed voice"
            );
            assert!(window.try_find("backend-edge-tts-stock-note").is_some());
            assert!(window.try_find("backend-stock-voice").is_some());
        })
        .unwrap();
        cx.run_until_parked();
        assert!(recorded.borrow().is_empty());
    }

    /// Picking Emel asks the root once, under Edge TTS.
    #[gpui_kit::test]
    fn picking_an_edge_tts_voice_asks_the_root(cx: &mut TestAppContext) {
        let (window, view, recorded) = open_backend_tab(cx, edge_tts_panel("tr-TR", None));

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let select = view.read(cx).voice_select.clone();
            select.update(cx, |_, cx| {
                // The voice in effect is not a change.
                cx.emit(SelectEvent::Confirm(Some("tr-TR-AhmetNeural".to_string())));
                cx.emit(SelectEvent::Confirm(Some(EMEL.to_string())));
            });
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(
            *recorded.borrow(),
            vec![BackendAction::SetSpeechVoice(
                LanguageBackend::Remote(RemoteProvider::EdgeTts),
                Some(EMEL.to_string())
            )]
        );
    }

    /// Edge TTS is listed after Azure, under the Remote kind, which no
    /// longer promises a key.
    #[test]
    fn edge_tts_is_the_last_remote_entry() {
        let remote = choices(BackendKind::Remote);
        let last = remote.last().unwrap();
        assert_eq!(last.entry, BackendEntry::Remote(RemoteProvider::EdgeTts));
        assert_eq!(last.label.as_ref(), "Edge TTS — free, online (stock voice)");
        assert_eq!(
            remote[remote.len() - 2].entry,
            BackendEntry::Remote(RemoteProvider::Azure)
        );
        assert_eq!(BackendKind::Remote.label(), "Remote — online providers");
        assert_eq!(provider_slug(RemoteProvider::EdgeTts), "edge-tts");
    }

    /// Nothing listed yet: the note says so, and never that the program is
    /// missing — the Dependencies row says that.
    #[test]
    fn edge_tts_with_no_list_yet_says_so() {
        let panel = BackendPanel {
            edge_tts_voices: Vec::new(),
            ..edge_tts_panel("tr-TR", None)
        };
        let note = speech_language_note(&panel, LanguageBackend::Remote(RemoteProvider::EdgeTts));
        assert!(note.contains("Edge TTS"), "{note}");
        assert!(note.contains("not listed yet"), "{note}");
        assert!(!note.contains("  "), "no stray spaces: {note}");
    }

    fn piper_panel(installed: &[(&str, &str)], voice: Option<&str>) -> BackendPanel {
        BackendPanel {
            selection: BackendSelection::PIPER_CPU,
            speech_voices: SpeechVoices {
                piper: voice.map(str::to_string),
                ..SpeechVoices::default()
            },
            piper_voices: installed
                .iter()
                .map(|(id, locale)| StockVoice {
                    id: id.to_string(),
                    language: locale.to_string(),
                    language_label: if locale.starts_with("tr") {
                        "Turkish (Turkey)".to_string()
                    } else {
                        "English (United States)".to_string()
                    },
                    name: id.to_string(),
                    priority: 0,
                })
                .collect(),
            ..BackendPanel::default()
        }
    }

    /// Story 3.15: Piper's pickers list only installed voices, and it is a
    /// Local stock voice with "Manage voices".
    #[gpui_kit::test]
    fn piper_pickers_list_installed_voices_only(cx: &mut TestAppContext) {
        let panel = piper_panel(
            &[
                ("tr_TR-dfki-medium", "tr_TR"),
                ("tr_TR-fahrettin-medium", "tr_TR"),
                ("en_US-lessac-medium", "en_US"),
            ],
            Some("tr_TR-fahrettin-medium"),
        );
        let (window, view, _recorded) = open_backend_tab(cx, panel);

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(view.read(cx).kind, BackendKind::Local);
            assert!(window.try_find("backend-stock-voice").is_some());
            assert!(window.try_find("backend-manage-piper-voices").is_some());
            assert_eq!(
                view.read(cx).language_select.read(cx).selected_value(),
                Some(&"tr_TR".to_string())
            );
            let voice = view.read(cx).voice_select.clone();
            assert_eq!(
                voice.read(cx).selected_value(),
                Some(&"tr_TR-fahrettin-medium".to_string())
            );
            // An English voice is not among Turkish's.
            voice.update(cx, |select, cx| {
                select.set_selected_value(&"en_US-lessac-medium".to_string(), window, cx);
            });
            assert_eq!(voice.read(cx).selected_value(), None);
            voice.update(cx, |select, cx| {
                select.set_selected_value(&"tr_TR-dfki-medium".to_string(), window, cx);
            });
            assert_eq!(
                voice.read(cx).selected_value(),
                Some(&"tr_TR-dfki-medium".to_string())
            );
        })
        .unwrap();
    }

    /// With no Piper voice installed the language note says so and points
    /// at "Manage voices", which asks the shell for the Piper voices tab.
    #[gpui_kit::test]
    fn no_piper_voice_says_so_and_manage_voices_opens_the_tab(cx: &mut TestAppContext) {
        let (window, view, _recorded) =
            open_backend_tab(cx, piper_panel(&[], Some("tr_TR-fahrettin-medium")));
        let opened = Rc::new(std::cell::Cell::new(false));
        let _subscription = cx.update({
            let opened = opened.clone();
            let view = view.clone();
            move |cx| cx.subscribe(&view, move |_, _: &OpenPiperVoicesTab, _| opened.set(true))
        });

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("backend-speech-language-note").is_some());
            assert_eq!(
                speech_language_note(&view.read(cx).panel, LanguageBackend::Piper),
                PIPER_NO_VOICE_NOTE
            );
            window.click("backend-manage-piper-voices", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(opened.get());
    }
}
