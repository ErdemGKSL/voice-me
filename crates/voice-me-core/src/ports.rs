use std::path::Path;

use crate::AppEventSender;
use crate::audio::AudioBuffer;
use crate::error::VoiceMeError;
use crate::state::LocalRuntime;
use crate::state::{
    AppState, BackendSelection, DependencyKind, LanguageBackend, RemoteProvider, RemoteSample,
    SpeechBackend,
};

/// What one Dependency Check is asked about (Story 3.3).
///
/// The resolved backend alone is not enough any more: whether the selection
/// can run here depends on which library the user picked and — for a remote
/// provider — on whether a key is saved. Only whether a key exists travels
/// here, never the key itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRequest {
    /// The backend the selection resolves to: the target and the weights
    /// it implies.
    pub backend: SpeechBackend,
    /// What the user selected.
    pub selection: BackendSelection,
    /// Whether the selected remote provider has an API key. Meaningless
    /// for a local selection.
    pub has_api_key: bool,
    /// Whether the selected remote provider has a region saved (Story
    /// 3.14: Azure). Meaningless for any other selection.
    pub has_region: bool,
    /// Whether the selected backend has a voice saved (Story 3.14: Azure,
    /// which has no default voice). Meaningless for any other selection.
    pub has_voice: bool,
    /// The Piper voice the selection speaks in (Story 3.15): the saved one,
    /// or the language's top installed one. What the Piper voice row checks
    /// for, and what its Install downloads. `None` for any other selection,
    /// or when no voice can be named.
    pub piper_voice: Option<String>,
}

impl CheckRequest {
    /// The request for the bundled CPU backend — the default selection.
    pub fn cpu() -> Self {
        Self {
            backend: SpeechBackend::CPU,
            selection: BackendSelection::BUNDLED_CPU,
            has_api_key: false,
            has_region: false,
            has_voice: false,
            piper_voice: None,
        }
    }
}

/// Driving adapter port: captures the configured global hotkey per OS.
pub trait HotkeyPort {
    /// Start listening for `hotkey`, signaling each press through `events`.
    ///
    /// `events` is the sender half of the shared `AppEvent` channel (AD-3),
    /// threaded in the same way as `TrayPort::show`: the adapter signals
    /// presses only by sending `AppEvent::HotkeyPressed`, never by depending
    /// on `voice-me-ui` or touching windows itself.
    ///
    /// Called once at startup by the composition root, and only when a
    /// hotkey was actually saved in a previous run — an app that has never
    /// had one configured must start with no hotkey active and no error.
    fn start_listening(&self, hotkey: &str, events: AppEventSender) -> Result<(), VoiceMeError>;

    /// Make `hotkey` the live combination, replacing whichever one is
    /// currently bound.
    ///
    /// This is the "confirm it works before persisting it" step the Settings
    /// UI calls on Save: on a backend that can detect a conflict it returns
    /// [`VoiceMeError::HotkeyAlreadyInUse`] *without* disturbing the
    /// previously bound combination, so the caller can surface the conflict
    /// and leave both the old binding and `settings.toml` untouched. Any
    /// other `Err` leaves the old binding in place too.
    fn rebind(&self, hotkey: &str) -> Result<(), VoiceMeError>;
}

/// Driven adapter port: plays generated audio through the OS virtual microphone.
///
/// `Send + Sync` for the same reason [`TtsPort`] and [`NotificationPort`]
/// are: the Speak Action hands the generated buffer straight to `play`, and
/// the whole action runs on Tokio's blocking pool through
/// [`crate::tokio_bridge::spawn_blocking`] (AD-5). The adapter is therefore
/// held as an `Arc<dyn VirtualMicPort>` shared between GPUI's main thread
/// and that pool, and the port says so rather than leaving each composition
/// root to discover it.
pub trait VirtualMicPort: Send + Sync {
    /// Play a buffer of generated speech through the virtual microphone device.
    ///
    /// Takes the core-owned [`AudioBuffer`] (AD-11), not bytes: the format
    /// is fixed at 24 kHz mono f32 by what Chatterbox's decoder emits, so
    /// there is nothing for this side to detect, negotiate, or guess wrong.
    fn play(&self, audio: &AudioBuffer) -> Result<(), VoiceMeError>;
}

/// Driven adapter port: manages the OS system tray presence.
pub trait TrayPort {
    /// Show the application's tray icon and menu.
    ///
    /// Takes a `gpui_kit::App` context because tray-backed implementations
    /// (e.g. `gpui-tray`, see spec-2-1) build their native tray item and
    /// dispatch menu-item clicks as ordinary GPUI actions through `cx` —
    /// there is no way to stand up or drive a tray without it. `voice-me-core`
    /// depends on `gpui-kit` only for this context type, never on the tray
    /// crate itself, which stays confined to the `voice-me-tray-*` adapters.
    ///
    /// `events` is the sender half of the shared `AppEvent` channel (AD-3):
    /// adapters use it to signal actions like "Settings…" clicked back to
    /// the composition root, without depending on `voice-me-ui` or calling
    /// window APIs themselves.
    fn show(&self, cx: &mut gpui_kit::App, events: AppEventSender) -> Result<(), VoiceMeError>;
}

/// Driven adapter port: generates speech with Chatterbox-Multilingual,
/// in-process, on ONNX Runtime (AD-12 — there is no sidecar process and no
/// Python anywhere in the pipeline; spec-2-5 verified this).
///
/// `Send + Sync` because every call is driven through
/// [`crate::tokio_bridge::spawn_blocking`] (AD-5): the adapter is shared
/// across GPUI's main thread and Tokio's blocking pool, so the port itself
/// has to say so rather than each composition root discovering it.
pub trait TtsPort: Send + Sync {
    /// Build and hold whatever generation needs, ahead of the first Speak
    /// Action.
    ///
    /// Session construction measured 86–110 s in spec-2-5, against a ~20 s
    /// utterance — far too much to hide inside the first call and call it a
    /// lazy cost (AD-10). The composition root calls this once at startup,
    /// on the blocking pool, and only when there is an active Reference
    /// Voice Sample: a first-run user who has not set one up pays nothing.
    ///
    /// Idempotent, and safe to race with [`TtsPort::generate`] — whichever
    /// arrives second waits for the first to finish rather than building a
    /// second set of sessions.
    fn warm_up(&self) -> Result<(), VoiceMeError>;

    /// Whether [`TtsPort::generate`] can start generating immediately, i.e.
    /// whether warm-up has already completed.
    ///
    /// This is what distinguishes "this will take the usual ~20 s" from
    /// "this will take a minute and a half first", which is the only wait
    /// unusual enough to be worth a notification (spec-2-6 Decision 4). It
    /// must never block — in particular it must not wait on whatever lock
    /// serializes generation, or a perfectly normal in-flight utterance
    /// would report "not ready" and notify for nothing.
    fn is_ready(&self) -> bool;

    /// Generate speech for `text` in `language`, cloning the voice in the
    /// Reference Voice Sample at `reference_clip` — or, for a stock-voice
    /// backend (Story 3.12), in the engine's `voice`.
    ///
    /// A cloning adapter needs `reference_clip` and returns
    /// [`VoiceMeError::NoReferenceVoiceSample`] on `None`; it ignores
    /// `voice`. A stock-voice adapter is given `None` and the voice id.
    ///
    /// `language` is a lowercase ISO code (`"tr"`, `"en"`) — the model
    /// takes it as a literal bracketed tag prepended to the text, so it is
    /// part of the prompt rather than a separate knob. `reference_clip` is
    /// a path, not decoded audio: whichever container the user's clip is in
    /// has to be decoded and resampled to exactly 24 kHz before the speech
    /// encoder sees it, and that belongs to the adapter that knows the
    /// model's requirements.
    ///
    /// This call blocks for as long as generation takes (seconds), so it is
    /// always driven through [`crate::tokio_bridge::spawn_blocking`] (AD-5)
    /// and never on GPUI's main thread.
    ///
    /// Exactly one generation runs at a time (AD-10). A second call arriving
    /// while one is in flight waits for it and then runs on the same
    /// sessions; it never builds a second set and never runs concurrently.
    fn generate(
        &self,
        text: &str,
        reference_clip: Option<&Path>,
        language: &str,
        voice: Option<&str>,
    ) -> Result<AudioBuffer, VoiceMeError>;
}

/// Driven adapter port: delivers an OS-native desktop notification.
///
/// The same driven shape as [`TrayPort`] — one trait in the hexagon, one
/// per-OS adapter crate behind it — but with no `gpui_kit::App` context:
/// every notification this application sends originates from a background
/// job that has already left the main thread (a failed generation, a slow
/// session build), and requiring a context would mean hopping back just to
/// say something went wrong.
///
/// It is the *only* surface the Speak Action has for failure or unusual
/// slowness in this story (UX-DR14/15). The Prompt Overlay is fire-and-forget
/// — it is already closed by the time generation starts — and its inline
/// notice belongs to Story 3.4.
///
/// `Send + Sync` for the same reason [`TtsPort`] is: the adapter is shared
/// with Tokio's blocking pool.
pub trait NotificationPort: Send + Sync {
    /// Show one notification with `summary` as its title and `body` beneath.
    ///
    /// Returning `Err` means the notification could not be *delivered* —
    /// there is no notification daemon, or the bus call failed. Callers
    /// treat that as unreportable rather than as a second failure to report:
    /// notifying about a failed notification has nowhere to go.
    fn notify(&self, summary: &str, body: &str) -> Result<(), VoiceMeError>;
}

/// Driven adapter port: detects and provisions runtime dependencies.
///
/// `Send + Sync` for the same reason [`TtsPort`] is: every call is driven
/// off the thread that asked for it — the startup check runs on GPUI's
/// background executor, and so does the one Settings' "Check again" button
/// triggers, because a detection call can block on an unresponsive audio
/// server and freezing the Settings window is not an acceptable way to
/// find that out.
pub trait DependencyProvisioningPort: Send + Sync {
    /// Run the Dependency Check for `request`, sending
    /// [`crate::AppEvent::DependencyCheckCompleted`] on `events`.
    ///
    /// Backend-relative by construction: there is no fixed dependency list
    /// to ask for, because the CPU backend never needs a GPU provider
    /// library and a GPU backend never needs the CPU's weights. `events` is
    /// threaded the same way [`HotkeyPort::start_listening`] threads it —
    /// the adapter reports only by sending, never by calling into
    /// `voice-me-core` or `voice-me-ui`, and holds nothing afterwards.
    ///
    /// Detection only: this reads the filesystem and the environment and
    /// changes neither. `Err` means the check itself could not run at all
    /// (no cache directory to resolve, say), which is a different thing
    /// from a check that ran and found something missing — the latter is a
    /// perfectly successful call carrying a report full of `missing` rows.
    ///
    /// Story 3.3: the first row is whether the selected backend can run on
    /// this machine at all (a driver, a GPU, an API key), probed afresh on
    /// every call.
    fn check(&self, request: CheckRequest, events: AppEventSender) -> Result<(), VoiceMeError>;

    /// Fetch or install whatever `backend` still lacks for the `kind` row
    /// (Story 3.2), blocking until it is done.
    ///
    /// Blocking and sync like [`Self::check`]: the caller dispatches it
    /// through [`crate::tokio_bridge::spawn_blocking`] (AD-5), and an
    /// adapter that needs async I/O drives it on the Tokio runtime it finds
    /// itself on. It must never run on GPUI's main thread — a model
    /// download is minutes long.
    ///
    /// Reports on `events` only: [`crate::AppEvent::ProvisioningProgress`]
    /// while bytes arrive, and exactly one
    /// [`crate::AppEvent::ProvisioningFinished`] at the end whatever the
    /// outcome. The returned `Result` mirrors that event and exists for the
    /// caller that has to tell "the adapter reported" from "the adapter
    /// never ran" (a panicked job sends nothing). It does not re-run the
    /// check; the composition root does that on `ProvisioningFinished`.
    ///
    /// Relative to `request` exactly as [`Self::check`] is: it fetches the
    /// selected backend's assets and nothing else — for the Piper voice row
    /// (Story 3.15), the voice `request.piper_voice` names. A second call
    /// for a `kind` already being provisioned returns at once, sending
    /// nothing, so the run already in flight stays the only one reporting.
    fn provision(
        &self,
        kind: DependencyKind,
        request: CheckRequest,
        events: AppEventSender,
    ) -> Result<(), VoiceMeError>;
}

/// Where a Piper voice in a catalog comes from (Story 3.15), in the order
/// a duplicate is resolved: the first source wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PiperSource {
    /// voice-me's own `piper-voices/catalog.json`, fetched live.
    VoiceMe,
    /// `rhasspy/piper-voices`' `voices.json`, at a pinned revision.
    Official,
    /// The `speaches-ai` repositories on Hugging Face.
    Speaches,
}

impl PiperSource {
    /// Every source, in precedence order.
    pub const ALL: [PiperSource; 3] = [
        PiperSource::VoiceMe,
        PiperSource::Official,
        PiperSource::Speaches,
    ];

    /// How the source is named to the user, and in `voice.toml`.
    pub fn label(self) -> &'static str {
        match self {
            PiperSource::VoiceMe => "voice-me",
            PiperSource::Official => "rhasspy/piper-voices",
            PiperSource::Speaches => "speaches-ai",
        }
    }
}

/// One downloadable Piper voice, as the Piper voices tab shows it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PiperCatalogEntry {
    /// `tr_TR-dfki-medium`: the voice's id, and its directory name.
    pub key: String,
    /// The voice's own name (`dfki`).
    pub name: String,
    /// `tr_TR`: Piper's speech language.
    pub locale: String,
    /// How the language is named to the user (`Turkish (Turkey)`).
    pub language_label: String,
    pub quality: String,
    /// `None` when the source declares none ("see model card").
    pub licence: Option<String>,
    pub source: PiperSource,
    /// The graph and config together; `None` when the source's list does
    /// not say (speaches-ai, until the download resolves it).
    pub size_bytes: Option<u64>,
}

/// What one catalog source gave: its voices (without any an earlier source
/// already listed), or why it could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogResult {
    pub source: PiperSource,
    pub result: Result<Vec<PiperCatalogEntry>, String>,
}

/// Driven adapter port: the Piper voice catalogs, and installing and
/// deleting voices (Story 3.15). Implemented by `voice-me-deps`, the only
/// crate that touches the network or writes the cache.
///
/// Every call blocks (network, disk); the caller runs it off the main
/// thread.
pub trait PiperCatalogPort: Send + Sync {
    /// Fetch every catalog, in precedence order, deduplicated: a voice an
    /// earlier source lists is not listed again. A source that fails is
    /// its own `Err`; the others are still returned.
    fn fetch_catalog(&self) -> Vec<CatalogResult>;

    /// Download `entry` into the cache: `.part` files, verified with the
    /// digest its source publishes, moved into place, then its `voice.toml`.
    /// Reports [`crate::AppEvent::PiperVoiceProgress`] while bytes arrive
    /// and exactly one [`crate::AppEvent::PiperVoiceFinished`].
    fn install(
        &self,
        entry: &PiperCatalogEntry,
        events: AppEventSender,
    ) -> Result<(), VoiceMeError>;

    /// Delete the installed voice `key` from the cache.
    fn delete(&self, key: &str) -> Result<(), VoiceMeError>;
}

/// Port for reading/writing persisted settings (implemented inside
/// `voice-me-core` itself, per AD-6 — not a separate adapter crate).
pub trait SettingsStore {
    /// Load settings into an `AppState`.
    fn load(&self) -> Result<AppState, VoiceMeError>;

    /// Persist `wav_bytes` as the active Reference Voice Sample (AD-6):
    /// writes the audio file to the OS data directory and updates the
    /// settings, replacing whichever clip was previously active. Returns the
    /// resulting `AppState`. Adapters (e.g. `voice-me-ui`'s recorder) call
    /// this instead of writing to the data directory themselves.
    fn save_reference_voice_sample(&self, wav_bytes: &[u8]) -> Result<AppState, VoiceMeError>;

    /// Persist the selected input (microphone) device name, or `None` to
    /// clear the selection back to the OS default. Returns the resulting
    /// `AppState`.
    fn save_selected_mic_device(&self, device: Option<&str>) -> Result<AppState, VoiceMeError>;

    /// Persist the configured global hotkey, or `None` to clear it. The
    /// string is stored in `global-hotkey`'s accelerator syntax (e.g.
    /// `Ctrl+Alt+KeyV`) so it round-trips through `HotKey::from_str` with no
    /// bespoke parser; the display form shown on a chip is derived for
    /// rendering only and never persisted. Returns the resulting `AppState`.
    fn save_hotkey(&self, hotkey: Option<&str>) -> Result<AppState, VoiceMeError>;

    /// Persist the selected backend (Story 3.5). Returns the resulting
    /// `AppState`.
    fn save_backend_selection(
        &self,
        selection: &BackendSelection,
    ) -> Result<AppState, VoiceMeError>;

    /// Persist `backend`'s speech language (Story 3.11), leaving every
    /// other backend's untouched. An error for a backend that has no
    /// speech language. Returns the resulting `AppState`.
    fn save_speech_language(
        &self,
        backend: LanguageBackend,
        code: &str,
    ) -> Result<AppState, VoiceMeError>;

    /// Persist `backend`'s voice (Story 3.12), or `None` for the language's
    /// top-priority one. An error for a backend with no voice choice.
    /// Returns the resulting `AppState`.
    fn save_speech_voice(
        &self,
        backend: LanguageBackend,
        voice: Option<&str>,
    ) -> Result<AppState, VoiceMeError>;

    /// Persist the list of ONNX Runtime libraries the user added, replacing
    /// the previous list. Returns the resulting `AppState`.
    fn save_local_runtimes(&self, runtimes: &[LocalRuntime]) -> Result<AppState, VoiceMeError>;

    /// Persist `provider`'s API key, or `None` to remove it. Each provider's
    /// key is stored independently. Returns the resulting `AppState`.
    fn save_api_key(
        &self,
        provider: RemoteProvider,
        key: Option<&str>,
    ) -> Result<AppState, VoiceMeError>;

    /// Persist the Azure region (Story 3.14), or `None` to remove it. The
    /// region is trimmed and lowercased; anything but `[a-z0-9]+` is an
    /// error and nothing is saved. Returns the resulting `AppState`.
    fn save_azure_region(&self, region: Option<&str>) -> Result<AppState, VoiceMeError>;

    /// Persist where the Prompt Overlay opens vertically, in percent of the
    /// free space (0 top, 100 bottom). Above 100 is saved as 100. Returns
    /// the resulting `AppState`.
    fn save_overlay_position(&self, percent: u8) -> Result<AppState, VoiceMeError>;

    /// Record that the user confirmed `provider`'s disclosure — what is
    /// sent, and to whom (Story 3.6). Idempotent. Returns the resulting
    /// `AppState`.
    fn save_disclosure_confirmed(&self, provider: RemoteProvider)
    -> Result<AppState, VoiceMeError>;

    /// Replace (or with `None`, forget) the Reference Voice Sample
    /// `provider` holds. `sample.provider` is ignored in favour of
    /// `provider`. Returns the resulting `AppState`.
    fn save_remote_sample(
        &self,
        provider: RemoteProvider,
        sample: Option<RemoteSample>,
    ) -> Result<AppState, VoiceMeError>;

    /// The Reference Voice Sample `provider` holds, if any.
    fn load_remote_sample(
        &self,
        provider: RemoteProvider,
    ) -> Result<Option<RemoteSample>, VoiceMeError> {
        Ok(self.load()?.remote_sample(provider).cloned())
    }
}
