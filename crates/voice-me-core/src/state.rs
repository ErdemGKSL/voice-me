use std::path::PathBuf;

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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SpeechExecutionTarget {
    /// The floor, and the only target the shipped `cpu` build can reach.
    #[default]
    Cpu,
    /// A Vulkan/D3D12 GPU through the WebGPU execution provider. Present as
    /// vocabulary so the resolved-backend value can express it; only a build
    /// compiled with that provider can actually honour it.
    WebGpu,
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

/// Whether one dependency is on this machine right now.
///
/// Two states only, and both are *words*: UX-DR-wise "missing" has to be
/// readable, not inferred from a colour. There is deliberately no
/// `Installing` here — Story 3.2 owns provisioning, and a report is a
/// snapshot of the filesystem, not a progress channel.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyKind {
    /// The ONNX Runtime shared library.
    OnnxRuntime,
    /// The model files the selected weight variant needs.
    ModelWeights,
    /// The Virtual Microphone device. Reported, never blocking: a user with
    /// no device can still type, and playback fails afterwards through the
    /// existing `VirtualMicUnavailable` notification.
    VirtualMicrophone,
}

impl DependencyKind {
    /// Whether a gap here stops the Speak Action before the user types
    /// (Decision 3).
    pub fn blocks_speech(self) -> bool {
        matches!(
            self,
            DependencyKind::OnnxRuntime | DependencyKind::ModelWeights
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
    /// Whether Story 3.2 will be able to fetch this with one click. `false`
    /// means the row gets short manual steps instead of an Install button.
    pub automatable: bool,
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
        }
    }

    /// Mark a row as one the app cannot fix for the user.
    pub fn manual(mut self) -> Self {
        self.automatable = false;
        self
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
    /// The AD-9 resolved speech backend. Not persisted: it describes this
    /// machine and this binary, not the user's preferences.
    pub speech_backend: SpeechBackend,
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
            dependencies: DependencyOutcome::default(),
        }
    }
}
