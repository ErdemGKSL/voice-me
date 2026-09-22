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
        }
    }
}
