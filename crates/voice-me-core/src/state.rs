use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The interface language when nothing has been saved yet.
pub const DEFAULT_UI_LANGUAGE: &str = "en";

/// The language generated speech is produced in when nothing has been saved
/// yet (spec-2-6 Decision 2).
///
/// Turkish, not the UI default of English: this is the language the user
/// actually speaks into their voice chats. FR5 wants a *selected* language
/// rather than a constant, and Epic 4 builds the Settings → Voice selector
/// that sets it; until then the settings file is the only way to change it.
pub const DEFAULT_SPEECH_LANGUAGE: &str = "tr";

/// Which execution provider the speech engine is placed on (AD-9).
///
/// Core-side vocabulary on purpose: `voice-me-core` must never name an `ort`
/// type, and `voice-me-tts` must never read anything but `AppState`. The two
/// enums here are the shared words the two sides map between.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeechExecutionTarget {
    /// The floor: every ONNX Runtime build has it.
    #[default]
    Cpu,
    /// An NVIDIA GPU through the CUDA execution provider (Story 3.3). Only a
    /// runtime library built with that provider can honour it, and only on
    /// a machine whose driver and GPU pass the capability check.
    Cuda,
    /// A Vulkan/D3D12 GPU through the WebGPU execution provider. Only a
    /// runtime library compiled with that provider can honour it.
    #[serde(rename = "webgpu")]
    WebGpu,
}

impl SpeechExecutionTarget {
    /// How the target is named to the user.
    pub fn label(self) -> &'static str {
        match self {
            SpeechExecutionTarget::Cpu => "CPU",
            SpeechExecutionTarget::Cuda => "CUDA",
            SpeechExecutionTarget::WebGpu => "WebGPU",
        }
    }

    /// Whether this target runs on a GPU.
    pub fn is_gpu(self) -> bool {
        !matches!(self, SpeechExecutionTarget::Cpu)
    }
}

/// Which `language_model` weight variant the backend implies (AD-12).
///
/// Not a free choice: the variant is a consequence of the execution target
/// (FP16 is a GPU kernel path and is the *worst* CPU option), so it travels
/// with the target in [`SpeechBackend`] rather than as its own setting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SpeechWeights {
    /// 4-bit, 354 MB. The shipped CPU default — see spec-2-6 Decision 1.
    #[default]
    Q4,
    /// 1.04 GB. GPU only in practice.
    Fp16,
    /// 2.08 GB. The quality baseline.
    Fp32,
}

impl SpeechWeights {
    /// How the variant is named to the user.
    pub fn label(self) -> &'static str {
        match self {
            SpeechWeights::Q4 => "Q4",
            SpeechWeights::Fp16 => "FP16",
            SpeechWeights::Fp32 => "FP32",
        }
    }
}

/// The AD-9 resolved backend: one value holding the active execution target,
/// the device within it where that applies, and the weight variant it
/// implies.
///
/// `voice-me-app` writes this (today from the build's compile-time variant
/// alone; Story 3.1 will write the Dependency Check's outcome here instead)
/// and `voice-me-tts` reads it. `voice-me-tts` never calls `voice-me-deps`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SpeechBackend {
    pub target: SpeechExecutionTarget,
    /// Which device within the target, where the target has more than one.
    /// `None` means "whatever the provider picks".
    pub device: Option<u32>,
    pub weights: SpeechWeights,
}

impl SpeechBackend {
    /// The backend `target` implies: the target decides the weights (AD-12
    /// — CPU runs Q4, a GPU target runs FP16), and no device is chosen
    /// (Story 3.9 adds the device picker).
    pub fn for_target(target: SpeechExecutionTarget) -> Self {
        Self {
            target,
            device: None,
            weights: match target {
                SpeechExecutionTarget::Cpu => SpeechWeights::Q4,
                SpeechExecutionTarget::Cuda | SpeechExecutionTarget::WebGpu => SpeechWeights::Fp16,
            },
        }
    }

    /// "CUDA — FP16 weights": what the backend line says a session runs on.
    pub fn summary(self) -> String {
        format!("{} — {} weights", self.target.label(), self.weights.label())
    }

    /// The shipped `cpu` variant's backend: CPU, no device selection, Q4.
    ///
    /// Q4 rather than FP32 (spec-2-6 Decision 1): 354 MB against 2.08 GB, and
    /// ~18 % faster end to end, against a quality difference the user
    /// described as not large.
    pub const CPU: Self = Self {
        target: SpeechExecutionTarget::Cpu,
        device: None,
        weights: SpeechWeights::Q4,
    };
}

/// A remote speech provider the user can select (Stories 3.5–3.7).
///
/// DeepInfra generates since Story 3.6; fal.ai is selectable, and generates
/// with Story 3.7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteProvider {
    #[serde(rename = "deepinfra")]
    DeepInfra,
    FalAi,
}

impl RemoteProvider {
    /// Every provider, in the order the UI lists them.
    pub const ALL: [RemoteProvider; 2] = [RemoteProvider::DeepInfra, RemoteProvider::FalAi];

    /// How the provider is named to the user.
    pub fn label(self) -> &'static str {
        match self {
            RemoteProvider::DeepInfra => "DeepInfra",
            RemoteProvider::FalAi => "fal.ai",
        }
    }
}

/// Which backend the user chose to generate speech with (Story 3.5).
///
/// Persisted. It is a *wish*, not a fact: whether it can run here is the
/// Dependency Check's capability row, and what actually ran is
/// [`ActiveBackend`]. Keeping the three apart is what lets the UI say
/// "Selected" and "Active" as two separate things (AD-9).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BackendSelection {
    /// An ONNX Runtime execution provider on this machine.
    Local {
        /// The runtime library that provides `target`. `None` is the
        /// bundled runtime (`ORT_DYLIB_PATH`, or the cache copy); `Some` is
        /// one the user added through "Add runtime…".
        runtime: Option<PathBuf>,
        target: SpeechExecutionTarget,
    },
    /// A remote provider, with the user's own key.
    Remote(RemoteProvider),
}

impl Default for BackendSelection {
    fn default() -> Self {
        Self::BUNDLED_CPU
    }
}

impl BackendSelection {
    /// The built-in "CPU (bundled runtime)" entry — the default, and what
    /// "Use CPU backend" selects.
    pub const BUNDLED_CPU: Self = Self::Local {
        runtime: None,
        target: SpeechExecutionTarget::Cpu,
    };

    /// The local execution target, or `None` for a remote provider.
    pub fn local_target(&self) -> Option<SpeechExecutionTarget> {
        match self {
            BackendSelection::Local { target, .. } => Some(*target),
            BackendSelection::Remote(_) => None,
        }
    }

    /// The added runtime library this selection loads, if it is one the
    /// user added rather than the bundled one.
    pub fn added_runtime(&self) -> Option<&Path> {
        match self {
            BackendSelection::Local {
                runtime: Some(path),
                ..
            } => Some(path),
            _ => None,
        }
    }

    /// Whether this is a CPU selection — the normal, never-warned-about
    /// state (UX-DR18), whichever library provides it.
    pub fn is_cpu(&self) -> bool {
        self.local_target() == Some(SpeechExecutionTarget::Cpu)
    }

    /// How the entry reads in the backend `Select`: "CPU (bundled
    /// runtime)", "CUDA (libonnxruntime.so)", "DeepInfra (remote)".
    pub fn label(&self) -> String {
        match self {
            BackendSelection::Local {
                runtime: None,
                target,
            } => format!("{} (bundled runtime)", target.label()),
            BackendSelection::Local {
                runtime: Some(path),
                target,
            } => format!("{} ({})", target.label(), file_name(path)),
            BackendSelection::Remote(provider) => format!("{} (remote)", provider.label()),
        }
    }
}

/// A path's file name for display, falling back to the whole path.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// An ONNX Runtime library the user added through "Add runtime…", with
/// what its probe reported (Decision 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalRuntime {
    pub path: PathBuf,
    /// Every execution provider the library reported as available, CPU
    /// included.
    pub targets: Vec<SpeechExecutionTarget>,
}

impl LocalRuntime {
    /// The backend entries this library contributes: one per GPU provider
    /// it has, or a single CPU entry when it has none (Decision 1). A GPU
    /// build's CPU provider is not listed again — the bundled runtime
    /// already covers CPU.
    pub fn entries(&self) -> Vec<BackendSelection> {
        let mut gpu: Vec<_> = self
            .targets
            .iter()
            .copied()
            .filter(|target| target.is_gpu())
            .collect();
        gpu.dedup();
        let targets = if gpu.is_empty() {
            vec![SpeechExecutionTarget::Cpu]
        } else {
            gpu
        };
        targets
            .into_iter()
            .map(|target| BackendSelection::Local {
                runtime: Some(self.path.clone()),
                target,
            })
            .collect()
    }

    /// The library's file name, for display.
    pub fn file_name(&self) -> String {
        file_name(&self.path)
    }
}

/// Every entry the backend `Select` lists, in order: the bundled CPU
/// runtime, each added runtime's entries, then the remote providers.
pub fn backend_choices(runtimes: &[LocalRuntime]) -> Vec<BackendSelection> {
    std::iter::once(BackendSelection::BUNDLED_CPU)
        .chain(runtimes.iter().flat_map(LocalRuntime::entries))
        .chain(
            RemoteProvider::ALL
                .into_iter()
                .map(BackendSelection::Remote),
        )
        .collect()
}

/// The user's provider API keys (Stories 3.5/3.6).
///
/// Stored in plaintext in the settings file — the UI says so where they are
/// entered. `Debug` is written by hand so a key can never reach a log line
/// through `{:?}` on this, on `AppState`, or on the settings file.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiKeys {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deepinfra: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fal_ai: Option<String>,
}

impl ApiKeys {
    /// The key for `provider`, if one is saved. A blank string is no key.
    pub fn get(&self, provider: RemoteProvider) -> Option<&str> {
        match provider {
            RemoteProvider::DeepInfra => self.deepinfra.as_deref(),
            RemoteProvider::FalAi => self.fal_ai.as_deref(),
        }
        .filter(|key| !key.trim().is_empty())
    }

    /// Whether a key is saved for `provider`.
    pub fn has(&self, provider: RemoteProvider) -> bool {
        self.get(provider).is_some()
    }

    /// Replace (or with `None`, remove) the key for `provider`. Surrounding
    /// whitespace — a pasted newline, say — is not part of a key.
    pub fn set(&mut self, provider: RemoteProvider, key: Option<String>) {
        let key = key
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty());
        match provider {
            RemoteProvider::DeepInfra => self.deepinfra = key,
            RemoteProvider::FalAi => self.fal_ai = key,
        }
    }
}

impl std::fmt::Debug for ApiKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redact = |key: &Option<String>| {
            if key.is_some() {
                "<redacted>"
            } else {
                "<none>"
            }
        };
        f.debug_struct("ApiKeys")
            .field("deepinfra", &redact(&self.deepinfra))
            .field("fal_ai", &redact(&self.fal_ai))
            .finish()
    }
}

/// The Reference Voice Sample as a remote provider holds it (Story 3.6):
/// uploaded once, then referenced by `voice_id`.
///
/// Keyed by the SHA-256 of the sample file, so a re-recorded sample no
/// longer matches and is uploaded afresh. Persisted; the id is not a
/// secret — it is useless without the key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteSample {
    pub provider: RemoteProvider,
    /// Lowercase hex SHA-256 of the sample file that was uploaded.
    pub sample_sha256: String,
    /// The provider's id for the uploaded voice.
    pub voice_id: String,
}

/// What the speech engine actually acquired when it built its session —
/// the "Active" half of the backend line (AD-9).
///
/// Written only from the TTS adapter's own report
/// ([`crate::AppEvent::SpeechSessionBuilt`]), never from the selection:
/// "Active" must never name a target no session was built on. Not
/// persisted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ActiveBackend {
    /// No session has been built this launch (or since the last switch).
    #[default]
    NotStarted,
    /// The engine built its sessions on this backend.
    Acquired(SpeechBackend),
    /// The last build failed, and this is the engine's error.
    Failed(String),
}

impl ActiveBackend {
    /// "Active: CUDA — FP16 weights", "Active: not started yet",
    /// "Active: none — <engine error>".
    pub fn summary(&self) -> String {
        match self {
            ActiveBackend::NotStarted => "Active: not started yet".to_string(),
            ActiveBackend::Acquired(backend) => format!("Active: {}", backend.summary()),
            ActiveBackend::Failed(reason) => format!("Active: none — {reason}"),
        }
    }
}

/// Whether one dependency is on this machine right now.
///
/// Two states only, and both are *words*: UX-DR-wise "missing" has to be
/// readable, not inferred from a colour. There is deliberately no
/// `Installing` here: a report is a snapshot of the filesystem, not a
/// progress channel. Story 3.2 keeps install progress beside the report, in
/// the Dependencies tab, so a row being installed is still `Missing` here —
/// and the overlay gate, which reads only this, keeps treating it as a
/// blocker until the re-run check says otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyStatus {
    /// Present and usable.
    Ready,
    /// Not on this machine.
    Missing,
}

impl DependencyStatus {
    /// The word the UI shows. Defined here rather than in the view so the
    /// overlay's blocker sentence and the Dependencies tab cannot disagree.
    pub fn label(self) -> &'static str {
        match self {
            DependencyStatus::Ready => "ready",
            DependencyStatus::Missing => "missing",
        }
    }

    /// Whether this row is a blocker.
    pub fn is_missing(self) -> bool {
        matches!(self, DependencyStatus::Missing)
    }
}

/// Which dependency a row is about.
///
/// Carried alongside the label because two different questions are asked of
/// a report — "what do I show the user" (the label) and "does this stop the
/// Speak Action" (this) — and matching on prose would be a bug waiting to
/// happen. Decision 3: only the speech-engine kinds block the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DependencyKind {
    /// The ONNX Runtime shared library.
    OnnxRuntime,
    /// The model files the selected weight variant needs.
    ModelWeights,
    /// The Virtual Microphone device. Reported, never blocking: a user with
    /// no device can still type, and playback fails afterwards through the
    /// existing `VirtualMicUnavailable` notification.
    VirtualMicrophone,
    /// Whether the *selected* backend can run on this machine at all
    /// (Story 3.3): a driver, a capable GPU, an API key. Blocking — a
    /// selection that cannot run is never quietly run on CPU instead.
    BackendCapability,
}

impl DependencyKind {
    /// Whether a gap here stops the Speak Action before the user types
    /// (Decision 3).
    pub fn blocks_speech(self) -> bool {
        matches!(
            self,
            DependencyKind::OnnxRuntime
                | DependencyKind::ModelWeights
                | DependencyKind::BackendCapability
        )
    }
}

/// One row of the Dependency Check's report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    pub kind: DependencyKind,
    /// What the user reads as the row's name.
    pub label: String,
    pub status: DependencyStatus,
    /// The exact path that is missing, or where the thing was found — the
    /// difference between "Dependency check failed (code 3)" and something
    /// a person can act on.
    pub detail: String,
    /// Whether Story 3.2 can fetch this with one click. `false` means the
    /// row gets short manual steps instead of an Install button.
    pub automatable: bool,
    /// What the user does by hand when [`Self::automatable`] is `false`:
    /// two to four short steps, shown inline under a "Show steps" toggle.
    /// Never a link — the epic promises no external docs. Empty on an
    /// automatable row.
    pub manual_steps: Vec<String>,
}

impl Dependency {
    /// A ready row.
    pub fn ready(
        kind: DependencyKind,
        label: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            label: label.into(),
            status: DependencyStatus::Ready,
            detail: detail.into(),
            automatable: true,
            manual_steps: Vec::new(),
        }
    }

    /// A missing row.
    pub fn missing(
        kind: DependencyKind,
        label: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            label: label.into(),
            status: DependencyStatus::Missing,
            detail: detail.into(),
            automatable: true,
            manual_steps: Vec::new(),
        }
    }

    /// Mark a row as one the app cannot fix for the user, with the short
    /// steps the user follows instead.
    pub fn manual<S: Into<String>>(mut self, steps: impl IntoIterator<Item = S>) -> Self {
        self.automatable = false;
        self.manual_steps = steps.into_iter().map(Into::into).collect();
        self
    }
}

/// A byte count the way the Dependencies tab and provisioning errors state
/// it: decimal units, so "1.56 GB" matches what a file manager says.
///
/// Below a gigabyte, whole megabytes ("412 MB"); from a gigabyte up, two
/// decimals ("1.56 GB"), because a whole-number figure would sit still for
/// a hundred megabytes at a time and look frozen.
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1_000;
    const MB: u64 = 1_000_000;
    const GB: u64 = 1_000_000_000;
    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{} KB", bytes / KB)
    } else {
        format!("{bytes} B")
    }
}

/// What one Dependency Check found, for one backend.
///
/// A value, not a service: `voice-me-deps` computes it, sends it once on
/// the `AppEvent` channel and holds nothing. The backend it was computed
/// for travels with it because every row in it is relative to that choice —
/// a report is meaningless without knowing which selection produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyReport {
    pub backend: SpeechBackend,
    pub dependencies: Vec<Dependency>,
}

impl DependencyReport {
    pub fn new(backend: SpeechBackend, dependencies: Vec<Dependency>) -> Self {
        Self {
            backend,
            dependencies,
        }
    }

    /// Whether anything at all is missing — what decides whether Settings →
    /// Dependencies opens itself at startup (Decision 2).
    pub fn has_missing(&self) -> bool {
        self.dependencies
            .iter()
            .any(|dependency| dependency.status.is_missing())
    }

    /// The first missing dependency that stops the Speak Action, if any.
    ///
    /// The single gate for Story 3.4: the composition root asks this, and
    /// the overlay names whatever comes back. A missing Virtual Microphone
    /// is never it.
    pub fn speech_engine_blocker(&self) -> Option<&Dependency> {
        self.dependencies
            .iter()
            .find(|dependency| dependency.status.is_missing() && dependency.kind.blocks_speech())
    }
}

/// What the Dependency Check has said so far.
///
/// Three states rather than an `Option<DependencyReport>`, because "the
/// check itself could not run" is a real, reportable answer — an
/// unresolvable cache root means there is no path to say anything about —
/// and flattening it into "nothing yet" would leave both readers silent
/// about the one thing the user needs to know.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DependencyOutcome {
    /// No check has completed yet this launch.
    #[default]
    Pending,
    /// The check could not run at all; this is why.
    Failed(String),
    /// The check ran. The report may still be full of missing rows.
    Ready(DependencyReport),
}

impl DependencyOutcome {
    /// The report, if one has arrived.
    pub fn report(&self) -> Option<&DependencyReport> {
        match self {
            DependencyOutcome::Ready(report) => Some(report),
            _ => None,
        }
    }
}

/// The single, canonical application state, owned by `voice-me-core`.
///
/// Mutation only happens through `voice-me-core` use-case functions (AD-3) —
/// adapters never write to this struct directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    pub hotkey: Option<String>,
    pub reference_voice_sample: Option<PathBuf>,
    pub ui_language: String,
    /// The language generated speech is produced in (FR5). Persisted, but
    /// not yet settable from the UI — spec-2-6 Decision 2 defers the
    /// Settings → Voice selector to Epic 4, where the UI-language selector
    /// is built.
    pub speech_language: String,
    pub selected_mic_device: Option<String>,
    /// The AD-9 resolved speech backend. Not persisted: the composition
    /// root derives it from [`Self::backend_selection`] on every read.
    pub speech_backend: SpeechBackend,
    /// Which backend the user selected (Story 3.5). Persisted.
    pub backend_selection: BackendSelection,
    /// The ONNX Runtime libraries the user added. Persisted.
    pub local_runtimes: Vec<LocalRuntime>,
    /// The remote providers' API keys. Persisted, in plaintext; redacted
    /// from `Debug`.
    pub api_keys: ApiKeys,
    /// The remote providers whose disclosure — what is sent, and to whom —
    /// the user has confirmed (Story 3.6). Persisted. Core refuses to call
    /// a remote provider that is not in here.
    pub confirmed_disclosures: Vec<RemoteProvider>,
    /// The Reference Voice Sample as each remote provider holds it, at
    /// most one per provider. Persisted.
    pub remote_samples: Vec<RemoteSample>,
    /// What the engine actually acquired at its last session build. Not
    /// persisted — it is a fact about this process.
    pub active_backend: ActiveBackend,
    /// The latest Dependency Check's outcome — including a check that could
    /// not run at all, which is a different thing from one that has not run
    /// yet. Not persisted either, and deliberately not a `SettingsFile`
    /// field: it describes the filesystem as it was a moment ago, which is
    /// exactly the kind of thing that must never be believed from a file
    /// written on a previous run.
    pub dependencies: DependencyOutcome,
}

/// Hand-written rather than derived so the two language fields default to
/// real languages instead of empty strings. An empty speech language is not
/// a harmless blank: it becomes an empty `[]` tag in the model prompt.
impl Default for AppState {
    fn default() -> Self {
        Self {
            hotkey: None,
            reference_voice_sample: None,
            ui_language: DEFAULT_UI_LANGUAGE.to_string(),
            speech_language: DEFAULT_SPEECH_LANGUAGE.to_string(),
            selected_mic_device: None,
            speech_backend: SpeechBackend::default(),
            backend_selection: BackendSelection::default(),
            local_runtimes: Vec::new(),
            api_keys: ApiKeys::default(),
            confirmed_disclosures: Vec::new(),
            remote_samples: Vec::new(),
            active_backend: ActiveBackend::default(),
            dependencies: DependencyOutcome::default(),
        }
    }
}

impl AppState {
    /// Whether `provider`'s disclosure has been confirmed.
    pub fn disclosure_confirmed(&self, provider: RemoteProvider) -> bool {
        self.confirmed_disclosures.contains(&provider)
    }

    /// The sample `provider` holds, if any.
    pub fn remote_sample(&self, provider: RemoteProvider) -> Option<&RemoteSample> {
        self.remote_samples
            .iter()
            .find(|sample| sample.provider == provider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_figures_read_the_way_the_spec_writes_them() {
        assert_eq!(format_bytes(412_000_000), "412 MB");
        assert_eq!(format_bytes(1_555_123_000), "1.56 GB");
        assert_eq!(format_bytes(71_798), "71 KB");
        assert_eq!(format_bytes(0), "0 B");
    }

    #[test]
    fn a_manual_row_carries_its_steps_and_no_install() {
        let row = Dependency::missing(DependencyKind::OnnxRuntime, "ONNX Runtime", "gone")
            .manual(["one", "two"]);

        assert!(!row.automatable);
        assert_eq!(row.manual_steps, vec!["one".to_string(), "two".to_string()]);
    }

    #[test]
    fn debug_output_never_contains_an_api_key() {
        let mut keys = ApiKeys::default();
        keys.set(
            RemoteProvider::DeepInfra,
            Some("sk-secret-deepinfra".to_string()),
        );
        let state = AppState {
            api_keys: keys.clone(),
            ..AppState::default()
        };

        for printed in [
            format!("{keys:?}"),
            format!("{state:?}"),
            format!("{state:#?}"),
        ] {
            assert!(!printed.contains("sk-secret"), "{printed}");
            assert!(printed.contains("<redacted>"), "{printed}");
        }
    }

    #[test]
    fn a_blank_key_is_no_key() {
        let mut keys = ApiKeys::default();
        keys.set(RemoteProvider::FalAi, Some("   ".to_string()));
        assert!(!keys.has(RemoteProvider::FalAi));
        keys.set(RemoteProvider::FalAi, Some(" key\n".to_string()));
        assert_eq!(keys.get(RemoteProvider::FalAi), Some("key"));
    }

    #[test]
    fn the_target_implies_the_weights() {
        assert_eq!(
            SpeechBackend::for_target(SpeechExecutionTarget::Cpu),
            SpeechBackend::CPU
        );
        assert_eq!(
            SpeechBackend::for_target(SpeechExecutionTarget::Cuda).weights,
            SpeechWeights::Fp16
        );
        assert_eq!(
            SpeechBackend::for_target(SpeechExecutionTarget::WebGpu).weights,
            SpeechWeights::Fp16
        );
    }

    #[test]
    fn a_gpu_runtime_lists_its_gpu_providers_and_a_cpu_only_one_lists_cpu() {
        let cuda = LocalRuntime {
            path: PathBuf::from("/opt/ort/libonnxruntime.so"),
            targets: vec![SpeechExecutionTarget::Cpu, SpeechExecutionTarget::Cuda],
        };
        assert_eq!(
            cuda.entries(),
            vec![BackendSelection::Local {
                runtime: Some(PathBuf::from("/opt/ort/libonnxruntime.so")),
                target: SpeechExecutionTarget::Cuda,
            }]
        );
        assert_eq!(cuda.entries()[0].label(), "CUDA (libonnxruntime.so)");

        let cpu_only = LocalRuntime {
            path: PathBuf::from("/opt/cpu/libonnxruntime.so"),
            targets: vec![SpeechExecutionTarget::Cpu],
        };
        assert_eq!(cpu_only.entries()[0].label(), "CPU (libonnxruntime.so)");

        let choices = backend_choices(&[cuda]);
        assert_eq!(choices.first(), Some(&BackendSelection::BUNDLED_CPU));
        assert_eq!(choices[0].label(), "CPU (bundled runtime)");
        assert_eq!(
            choices.last(),
            Some(&BackendSelection::Remote(RemoteProvider::FalAi))
        );
        assert_eq!(choices.len(), 4);
    }

    #[test]
    fn the_active_line_reads_the_three_ways_the_spec_writes_it() {
        assert_eq!(
            ActiveBackend::NotStarted.summary(),
            "Active: not started yet"
        );
        assert_eq!(
            ActiveBackend::Acquired(SpeechBackend::for_target(SpeechExecutionTarget::Cuda))
                .summary(),
            "Active: CUDA — FP16 weights"
        );
        assert_eq!(
            ActiveBackend::Failed("no driver".to_string()).summary(),
            "Active: none — no driver"
        );
    }

    #[test]
    fn a_capability_miss_blocks_speech() {
        assert!(DependencyKind::BackendCapability.blocks_speech());
    }
}
