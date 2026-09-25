use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The interface language when nothing has been saved yet.
pub const DEFAULT_UI_LANGUAGE: &str = "en";

/// The Prompt Overlay's default vertical position, in percent of the free
/// vertical space: centred in the free area of the primary display's visible
/// area (taskbar and panels excluded).
pub const DEFAULT_OVERLAY_POSITION: u8 = 50;

/// The top edge of a window placed `pct` percent of the way down the free
/// vertical space of a visible area: `0` puts the window's top on the
/// area's top, `100` its bottom on the area's bottom, `50` centres it.
/// `pct` above 100 counts as 100. A window taller than the area gets the
/// area's top, never a position above it.
pub fn overlay_origin_y(visible_top: f32, visible_height: f32, window_height: f32, pct: u8) -> f32 {
    let free = (visible_height - window_height).max(0.0);
    visible_top + free * f32::from(pct.min(100)) / 100.0
}

/// The language generated speech is produced in when nothing has been saved
/// yet (spec-2-6 Decision 2).
///
/// Turkish, not the UI default of English: this is the language the user
/// actually speaks into their voice chats. Story 3.11 gives every backend
/// with a speech language its own saved value, and each of them starts here.
pub const DEFAULT_SPEECH_LANGUAGE: &str = "tr";

/// Azure's speech language when nothing has been saved yet (Story 3.14):
/// Azure names its languages by locale, and Turkish is the default here as
/// everywhere else.
pub const DEFAULT_AZURE_LOCALE: &str = "tr-TR";

/// Edge TTS's speech language when nothing has been saved yet (Story
/// 3.17): its voices are named by locale, like Azure's.
pub const DEFAULT_EDGE_TTS_LOCALE: &str = "tr-TR";

/// Why an Azure region was not saved (Story 3.14).
pub const AZURE_REGION_INVALID: &str =
    "An Azure region is letters and digits only, like westeurope.";

/// The Azure region `input` names, trimmed and lowercased (Story 3.14):
/// `Ok(None)` for a blank input (no region), `Err` with the reason for
/// anything but `[a-z0-9]+` — so a region can never change the host it
/// is put into.
pub fn parse_azure_region(input: &str) -> Result<Option<String>, String> {
    let region = input.trim().to_ascii_lowercase();
    if region.is_empty() {
        return Ok(None);
    }
    if region
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        Ok(Some(region))
    } else {
        Err(AZURE_REGION_INVALID.to_string())
    }
}

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
/// with Story 3.7. Azure (Story 3.14) speaks in a stock Microsoft voice.
/// Edge TTS (Story 3.17) speaks in the same voices through the separately
/// installed `edge-tts` program, with no key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteProvider {
    #[serde(rename = "deepinfra")]
    DeepInfra,
    FalAi,
    Azure,
    EdgeTts,
}

impl RemoteProvider {
    /// Every provider, in the order the UI lists them.
    pub const ALL: [RemoteProvider; 4] = [
        RemoteProvider::DeepInfra,
        RemoteProvider::FalAi,
        RemoteProvider::Azure,
        RemoteProvider::EdgeTts,
    ];

    /// How the provider is named to the user.
    pub fn label(self) -> &'static str {
        match self {
            RemoteProvider::DeepInfra => "DeepInfra",
            RemoteProvider::FalAi => "fal.ai",
            RemoteProvider::Azure => "Azure",
            RemoteProvider::EdgeTts => "Edge TTS",
        }
    }

    /// Whether this provider needs the user's API key (Story 3.17): every
    /// provider but Edge TTS, which runs a program that needs no account.
    /// Every key check — the capability row, the Backend tab's key field,
    /// the disclosure's key guard — asks this first.
    pub fn needs_api_key(self) -> bool {
        self != RemoteProvider::EdgeTts
    }

    /// Whether this provider speaks in a stock voice rather than cloning
    /// the user's (Story 3.14): it never receives the Reference Voice
    /// Sample.
    pub fn is_stock_voice(self) -> bool {
        matches!(self, RemoteProvider::Azure | RemoteProvider::EdgeTts)
    }
}

/// Which *kind* of backend a speech language belongs to (Story 3.11).
///
/// Every local Chatterbox selection — the bundled CPU runtime and each added
/// runtime's CPU/CUDA/WebGPU entry — is the same model, so they share one
/// [`LanguageBackend::Local`] language. Each remote provider has its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LanguageBackend {
    Local,
    Remote(RemoteProvider),
    /// The OS's own speech engine (Story 3.12: eSpeak NG on Linux). Its
    /// set is whatever the engine lists on this machine, so it has no
    /// static table: see [`LanguageBackend::language_options`].
    SystemVoice,
    /// Piper (Story 3.15): its languages are the locales of the voices
    /// installed in the cache, so it has no static table either.
    Piper,
}

impl LanguageBackend {
    /// How the backend is named in a refusal: "local Chatterbox",
    /// "DeepInfra".
    pub fn label(self) -> &'static str {
        match self {
            LanguageBackend::Local => "local Chatterbox",
            LanguageBackend::Remote(provider) => provider.label(),
            LanguageBackend::SystemVoice => "System voice",
            LanguageBackend::Piper => "Piper",
        }
    }

    /// The speech languages this backend generates in, in the order the UI
    /// lists them. Empty for a backend whose set is not decided yet
    /// (fal.ai, Story 3.7), and for the System voice, whose set is not
    /// static — [`Self::language_options`] takes its voice list.
    pub fn speech_languages(self) -> &'static [SpeechLanguage] {
        match self {
            LanguageBackend::Local => LOCAL_SPEECH_LANGUAGES,
            LanguageBackend::Remote(RemoteProvider::DeepInfra) => DEEPINFRA_SPEECH_LANGUAGES,
            LanguageBackend::Remote(
                RemoteProvider::FalAi | RemoteProvider::Azure | RemoteProvider::EdgeTts,
            )
            | LanguageBackend::SystemVoice
            | LanguageBackend::Piper => &[],
        }
    }

    /// Whether this backend's languages and voices come from a list read
    /// at run time rather than a static table: the System voice's engine
    /// (Story 3.12), Azure's voice list (Story 3.14), the installed Piper
    /// voices (Story 3.15) and `edge-tts --list-voices` (Story 3.17).
    pub fn has_voice_list(self) -> bool {
        matches!(
            self,
            LanguageBackend::SystemVoice
                | LanguageBackend::Remote(RemoteProvider::Azure | RemoteProvider::EdgeTts)
                | LanguageBackend::Piper
        )
    }

    /// Whether this backend has a speech language at all. A backend with a
    /// voice list always does, even while its list is empty or not loaded
    /// yet.
    pub fn has_speech_language(self) -> bool {
        self.has_voice_list() || !self.speech_languages().is_empty()
    }

    /// Whether this backend speaks in a stock voice chosen from a list
    /// rather than the user's cloned one (Stories 3.12, 3.14).
    pub fn has_voice_choice(self) -> bool {
        self.has_voice_list()
    }

    /// Whether this backend's voice must be chosen before it can speak
    /// (Story 3.14, D4): Azure has no "top-priority" default voice, while
    /// the System voice falls back to its language's top-priority one.
    pub fn requires_voice(self) -> bool {
        self == LanguageBackend::Remote(RemoteProvider::Azure)
    }

    /// The sibling of [`Self::speech_languages`] that also covers the
    /// backends with a voice list: their languages come from `voices`, the
    /// list read at run time (see [`stock_voice_languages`]). Every other
    /// backend ignores `voices` and lists its static set.
    pub fn language_options(self, voices: &[StockVoice]) -> Vec<LanguageOption> {
        match self {
            backend if backend.has_voice_list() => stock_voice_languages(voices),
            _ => self
                .speech_languages()
                .iter()
                .map(|language| LanguageOption {
                    code: language.code.to_string(),
                    label: language.label.to_string(),
                })
                .collect(),
        }
    }

    /// The canonical code of the language `saved` names, if this backend
    /// speaks it — [`Self::speech_language`] for a static set, and for a
    /// backend with a voice list the listed language it matches (trimmed,
    /// compared without regard to ASCII case). `None` outside the set,
    /// never a default.
    ///
    /// Story 3.13 (decision 1): with no exact match, the fallback treats
    /// `_` and `-` alike (`tr_TR` finds `tr-TR`), then a bare primary subtag
    /// matches a listed tag with that primary subtag, and the other way
    /// round — the saved `tr` finds Windows' `tr-TR`, the first listed one
    /// when several share it. Two tags that both carry a region never
    /// match each other (`en-gb` never finds `en-us`), so every code that
    /// resolved before still resolves to the same language.
    pub fn resolve_language(self, saved: &str, voices: &[StockVoice]) -> Option<String> {
        match self {
            backend if backend.has_voice_list() => {
                let saved = saved.trim();
                voices
                    .iter()
                    .find(|voice| voice.language.eq_ignore_ascii_case(saved))
                    .or_else(|| {
                        voices
                            .iter()
                            .find(|voice| same_tag_any_separator(saved, &voice.language))
                    })
                    .or_else(|| {
                        voices
                            .iter()
                            .find(|voice| same_primary_language(saved, &voice.language))
                    })
                    .map(|voice| voice.language.clone())
            }
            _ => self
                .speech_language(saved)
                .map(|language| language.code.to_string()),
        }
    }

    /// The entry a saved value names, if this backend speaks it. The value
    /// is trimmed and lowercased first, so a hand-edited `"  EN \n"` still
    /// names English; anything else outside the set is `None`, never a
    /// default.
    pub fn speech_language(self, saved: &str) -> Option<&'static SpeechLanguage> {
        let code = saved.trim().to_lowercase();
        self.speech_languages()
            .iter()
            .find(|language| language.code == code)
    }
}

/// One entry of a backend's language list, owned, so a list read from the
/// System voice's engine and a static table look the same to the UI.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LanguageOption {
    pub code: String,
    pub label: String,
}

/// One stock voice a backend lists at run time: a voice the System
/// voice's engine lists (Story 3.12, parsed from `espeak-ng --voices`) or
/// one of Azure's (Story 3.14, from its voice list).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StockVoice {
    /// What the engine or provider is told to use (`trk/tr`,
    /// `tr-TR-EmelNeural`). Unique in a list.
    pub id: String,
    /// The language the voice speaks (`tr`, `en-gb`, `tr-TR`). Each
    /// distinct value, compared without regard to ASCII case, is its own
    /// speech language.
    pub language: String,
    /// How the voice's language is named to the user, when this voice is
    /// its language's top-priority one: eSpeak's voice name, Azure's
    /// `LocaleName` ("Turkish (Türkiye)").
    pub language_label: String,
    /// The name the user reads ("Chinese (Cantonese)", "Emel (Female)").
    pub name: String,
    /// The priority within the language: lower is preferred.
    pub priority: u32,
}

/// Whether `a` and `b` are the same tag once `_` and `-` are read alike,
/// without regard to ASCII case: `tr_TR` and `tr-TR` (decision 1's
/// fallback, Story 3.13).
fn same_tag_any_separator(a: &str, b: &str) -> bool {
    let separator = |byte: u8| if byte == b'_' { b'-' } else { byte };
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .all(|(x, y)| separator(x).eq_ignore_ascii_case(&separator(y)))
}

/// A language tag's primary subtag: `tr` of `tr-TR`, `en` of `en_gb` (the
/// fallback reads `_` and `-` alike).
fn primary_subtag(tag: &str) -> &str {
    tag.split(['-', '_']).next().unwrap_or(tag)
}

/// Decision 1's fallback (Story 3.13): `a` and `b` share a primary subtag
/// (without regard to ASCII case) and at least one of them is nothing but
/// that subtag — `tr` ↔ `tr-TR`, never `en-gb` ↔ `en-us`.
fn same_primary_language(a: &str, b: &str) -> bool {
    let (primary_a, primary_b) = (primary_subtag(a), primary_subtag(b));
    !primary_a.is_empty()
        && primary_a.eq_ignore_ascii_case(primary_b)
        && (primary_a.len() == a.len() || primary_b.len() == b.len())
}

/// The distinct languages `voices` speak (compared without regard to ASCII
/// case), each labelled with its top-priority voice's language label,
/// sorted by label.
pub fn stock_voice_languages(voices: &[StockVoice]) -> Vec<LanguageOption> {
    let mut languages: Vec<LanguageOption> = Vec::new();
    for voice in voices {
        if languages
            .iter()
            .any(|language| language.code.eq_ignore_ascii_case(&voice.language))
        {
            continue;
        }
        let top = stock_voices_of(voices, &voice.language)[0];
        languages.push(LanguageOption {
            code: voice.language.clone(),
            label: top.language_label.clone(),
        });
    }
    languages.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.code.cmp(&b.code)));
    languages
}

/// The voices that speak `language` (compared without regard to ASCII
/// case), top priority first: lowest priority number, then list order.
pub fn stock_voices_of<'a>(voices: &'a [StockVoice], language: &str) -> Vec<&'a StockVoice> {
    let mut of: Vec<&StockVoice> = voices
        .iter()
        .filter(|voice| voice.language.eq_ignore_ascii_case(language))
        .collect();
    // A stable sort keeps list order among equal priorities.
    of.sort_by_key(|voice| voice.priority);
    of
}

/// Why a stored stock-voice language or voice cannot be used (Stories
/// 3.12, 3.14). Each names the backend it is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StockVoiceRefusal {
    /// No voice list has been read yet.
    NoVoices(LanguageBackend),
    /// No listed voice speaks the stored language.
    Language {
        backend: LanguageBackend,
        language: String,
    },
    /// The stored voice is not one of the language's voices.
    Voice {
        backend: LanguageBackend,
        language: String,
        voice: String,
    },
    /// The backend needs a voice and none is stored (Story 3.14).
    NoVoice(LanguageBackend),
}

impl std::fmt::Display for StockVoiceRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StockVoiceRefusal::NoVoices(backend) => write!(
                f,
                "{} has no voices listed yet — check Settings → Backend",
                backend.label()
            ),
            StockVoiceRefusal::Language { backend, language } => write!(
                f,
                "{} can't speak the speech language {language:?} — choose one in Settings → \
                 Backend",
                backend.label()
            ),
            StockVoiceRefusal::Voice {
                backend,
                language,
                voice,
            } => write!(
                f,
                "{} has no voice {voice:?} for {language:?} — choose one in Settings → Backend",
                backend.label()
            ),
            StockVoiceRefusal::NoVoice(backend) => write!(
                f,
                "{} has no voice selected — pick one in Settings → Backend",
                backend.label()
            ),
        }
    }
}

/// The voice a stock-voice Speak Action uses: the stored `voice` if it is
/// one of `language`'s, or with none stored, the language's top-priority
/// voice — unless `backend` requires a stored voice (Azure, D4). Refused
/// by name otherwise — never a substitute.
pub fn resolve_stock_voice<'a>(
    backend: LanguageBackend,
    voices: &'a [StockVoice],
    language: &str,
    voice: Option<&str>,
) -> Result<&'a StockVoice, StockVoiceRefusal> {
    if voice.is_none() && backend.requires_voice() {
        return Err(StockVoiceRefusal::NoVoice(backend));
    }
    if voices.is_empty() {
        return Err(StockVoiceRefusal::NoVoices(backend));
    }
    let Some(code) = backend.resolve_language(language, voices) else {
        return Err(StockVoiceRefusal::Language {
            backend,
            language: language.trim().to_string(),
        });
    };
    let of = stock_voices_of(voices, &code);
    match voice {
        None => Ok(of[0]),
        Some(id) => of
            .into_iter()
            .find(|candidate| candidate.id == id)
            .ok_or_else(|| StockVoiceRefusal::Voice {
                backend,
                language: code,
                voice: id.to_string(),
            }),
    }
}

/// One speech language a backend generates in: the tag the model gets and
/// the English name the UI shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpeechLanguage {
    pub code: &'static str,
    pub label: &'static str,
}

const fn language(code: &'static str, label: &'static str) -> SpeechLanguage {
    SpeechLanguage { code, label }
}

/// Local Chatterbox (AD-12): the languages that need no Python-only text
/// normalization. Turkish first — the default.
const LOCAL_SPEECH_LANGUAGES: &[SpeechLanguage] =
    &[language("tr", "Turkish"), language("en", "English")];

/// DeepInfra's `ResembleAI/chatterbox-multilingual`: its 23 languages,
/// alphabetical by name.
const DEEPINFRA_SPEECH_LANGUAGES: &[SpeechLanguage] = &[
    language("ar", "Arabic"),
    language("zh", "Chinese"),
    language("da", "Danish"),
    language("nl", "Dutch"),
    language("en", "English"),
    language("fi", "Finnish"),
    language("fr", "French"),
    language("de", "German"),
    language("el", "Greek"),
    language("he", "Hebrew"),
    language("hi", "Hindi"),
    language("it", "Italian"),
    language("ja", "Japanese"),
    language("ko", "Korean"),
    language("ms", "Malay"),
    language("no", "Norwegian"),
    language("pl", "Polish"),
    language("pt", "Portuguese"),
    language("ru", "Russian"),
    language("es", "Spanish"),
    language("sw", "Swahili"),
    language("sv", "Swedish"),
    language("tr", "Turkish"),
];

/// Piper's speech language when nothing has been saved yet (Story 3.15):
/// the default voice's locale.
pub const DEFAULT_PIPER_LOCALE: &str = crate::assets::PIPER_DEFAULT_VOICE.locale;

/// The saved speech language of each backend that has one (Story 3.11).
/// Persisted. Values are stored as written — a hand-edited value outside the
/// backend's set is kept, and refused by name at the next Speak Action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechLanguages {
    pub local: String,
    pub deepinfra: String,
    /// Story 3.12. Defaults to `tr`; the legacy key never seeds it.
    pub system_voice: String,
    /// Story 3.14: an Azure locale. Defaults to `tr-TR`; the legacy key
    /// never seeds it.
    pub azure: String,
    /// Story 3.17: an Edge TTS locale. Defaults to `tr-TR`; the legacy key
    /// never seeds it.
    pub edge_tts: String,
    /// Story 3.15: a Piper locale (`tr_TR`). Defaults to the default
    /// voice's; the legacy key never seeds it.
    pub piper: String,
}

impl Default for SpeechLanguages {
    fn default() -> Self {
        Self {
            local: DEFAULT_SPEECH_LANGUAGE.to_string(),
            deepinfra: DEFAULT_SPEECH_LANGUAGE.to_string(),
            system_voice: DEFAULT_SPEECH_LANGUAGE.to_string(),
            azure: DEFAULT_AZURE_LOCALE.to_string(),
            edge_tts: DEFAULT_EDGE_TTS_LOCALE.to_string(),
            piper: DEFAULT_PIPER_LOCALE.to_string(),
        }
    }
}

/// The saved voice of each backend that has a voice choice (Stories 3.12,
/// 3.14, 3.15). Persisted. For the System voice and Piper `None` is the
/// language's top-priority voice; for Azure it is no voice at all (D4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechVoices {
    pub system_voice: Option<String>,
    /// Azure's `ShortName` (`tr-TR-EmelNeural`).
    pub azure: Option<String>,
    /// Story 3.17: an Edge TTS voice id (`tr-TR-EmelNeural`). `None` is the
    /// language's first listed voice.
    pub edge_tts: Option<String>,
    /// Story 3.15: a Piper voice key. Starts at the default voice.
    pub piper: Option<String>,
}

impl Default for SpeechVoices {
    fn default() -> Self {
        Self {
            system_voice: None,
            azure: None,
            edge_tts: None,
            piper: Some(crate::assets::PIPER_DEFAULT_VOICE.key.to_string()),
        }
    }
}

impl SpeechVoices {
    /// `backend`'s saved voice, if it has a voice choice and one is saved.
    pub fn get(&self, backend: LanguageBackend) -> Option<&str> {
        match backend {
            LanguageBackend::SystemVoice => self.system_voice.as_deref(),
            LanguageBackend::Remote(RemoteProvider::Azure) => self.azure.as_deref(),
            LanguageBackend::Remote(RemoteProvider::EdgeTts) => self.edge_tts.as_deref(),
            LanguageBackend::Piper => self.piper.as_deref(),
            _ => None,
        }
    }
}

impl SpeechLanguages {
    /// `backend`'s saved language, or `None` for a backend that has none.
    pub fn get(&self, backend: LanguageBackend) -> Option<&str> {
        match backend {
            LanguageBackend::Local => Some(&self.local),
            LanguageBackend::Remote(RemoteProvider::DeepInfra) => Some(&self.deepinfra),
            LanguageBackend::Remote(RemoteProvider::FalAi) => None,
            LanguageBackend::Remote(RemoteProvider::Azure) => Some(&self.azure),
            LanguageBackend::Remote(RemoteProvider::EdgeTts) => Some(&self.edge_tts),
            LanguageBackend::SystemVoice => Some(&self.system_voice),
            LanguageBackend::Piper => Some(&self.piper),
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
    /// The OS's own speech engine in a stock voice (Story 3.12): eSpeak NG
    /// on Linux. Local, but never an ONNX target.
    SystemVoice,
    /// A Piper neural stock voice (Story 3.15), run on the bundled runtime
    /// — on CPU, or on CUDA or WebGPU once the bundled runtime has every
    /// provider (spec-backend-engine-and-device-selects). Local, but not a
    /// Chatterbox target.
    Piper { target: SpeechExecutionTarget },
}

/// What an unsaved selection is (Story 3.15): Piper where this OS's build
/// has it (Linux; Windows since Story 3.16), the bundled CPU runtime
/// everywhere else. Only ever the answer for a profile with *no* saved
/// selection — an explicit one is never rewritten.
impl Default for BackendSelection {
    fn default() -> Self {
        if cfg!(any(target_os = "linux", target_os = "windows")) {
            Self::PIPER_CPU
        } else {
            Self::BUNDLED_CPU
        }
    }
}

impl BackendSelection {
    /// The built-in "CPU (bundled runtime)" entry — the default, and what
    /// "Use CPU backend" selects.
    pub const BUNDLED_CPU: Self = Self::Local {
        runtime: None,
        target: SpeechExecutionTarget::Cpu,
    };

    /// Piper on the bundled CPU runtime — the first-run default on Linux
    /// and Windows.
    pub const PIPER_CPU: Self = Self::Piper {
        target: SpeechExecutionTarget::Cpu,
    };

    /// The ONNX execution target this selection runs on — Chatterbox's or
    /// Piper's — or `None` for a remote provider and for the System voice,
    /// which runs no ONNX session.
    pub fn local_target(&self) -> Option<SpeechExecutionTarget> {
        match self {
            BackendSelection::Local { target, .. } | BackendSelection::Piper { target } => {
                Some(*target)
            }
            BackendSelection::Remote(_) | BackendSelection::SystemVoice => None,
        }
    }

    /// Whether this is Chatterbox (a [`BackendSelection::Local`] entry): the
    /// one engine that needs the speech model files.
    pub fn is_chatterbox(&self) -> bool {
        matches!(self, BackendSelection::Local { .. })
    }

    /// The local engine this selection runs, or `None` for a remote one.
    pub fn engine(&self) -> Option<LocalEngine> {
        match self {
            BackendSelection::Piper { .. } => Some(LocalEngine::Piper),
            BackendSelection::Local { .. } => Some(LocalEngine::Chatterbox),
            BackendSelection::SystemVoice => Some(LocalEngine::SystemVoice),
            BackendSelection::Remote(_) => None,
        }
    }

    /// The same engine on the CPU — what "Use CPU backend" selects: Piper
    /// on CPU for Piper, the bundled CPU runtime for anything else.
    pub fn on_cpu(&self) -> Self {
        match self {
            BackendSelection::Piper { .. } => Self::PIPER_CPU,
            _ => Self::BUNDLED_CPU,
        }
    }

    /// How the device reads in the device `Select`: "CPU", "CUDA",
    /// "WebGPU", or "CUDA — libonnxruntime.so" for an added runtime's.
    /// `None` for a selection with no device.
    pub fn device_label(&self) -> Option<String> {
        match self {
            BackendSelection::Local {
                runtime: Some(path),
                target,
            } => Some(format!("{} — {}", target.label(), file_name(path))),
            BackendSelection::Local {
                runtime: None,
                target,
            }
            | BackendSelection::Piper { target } => Some(target.label().to_string()),
            BackendSelection::Remote(_) | BackendSelection::SystemVoice => None,
        }
    }

    /// Whether this backend speaks in a stock voice rather than the user's
    /// cloned one — and so needs no Reference Voice Sample (Stories 3.12,
    /// 3.14): the System voice and Azure.
    pub fn is_stock_voice(&self) -> bool {
        match self {
            BackendSelection::SystemVoice | BackendSelection::Piper { .. } => true,
            BackendSelection::Remote(provider) => provider.is_stock_voice(),
            BackendSelection::Local { .. } => false,
        }
    }

    /// Whether this is the OS's own speech engine (Story 3.12) — what the
    /// eSpeak NG row and voice listing are about. Azure is a stock voice,
    /// but not this.
    pub fn is_system_voice(&self) -> bool {
        matches!(self, BackendSelection::SystemVoice)
    }

    /// Whether this is Piper (Story 3.15).
    pub fn is_piper(&self) -> bool {
        matches!(self, BackendSelection::Piper { .. })
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

    /// Which backend's speech language this selection uses (Story 3.11):
    /// every local entry shares the Local one.
    pub fn language_backend(&self) -> LanguageBackend {
        match self {
            BackendSelection::Local { .. } => LanguageBackend::Local,
            BackendSelection::Remote(provider) => LanguageBackend::Remote(*provider),
            BackendSelection::SystemVoice => LanguageBackend::SystemVoice,
            BackendSelection::Piper { .. } => LanguageBackend::Piper,
        }
    }

    /// Whether this is a CPU selection — the normal, never-warned-about
    /// state (UX-DR18), whichever library provides it.
    pub fn is_cpu(&self) -> bool {
        self.local_target() == Some(SpeechExecutionTarget::Cpu)
    }

    /// How the selection reads on the *Selected* line: the engine, then
    /// its device — "Chatterbox — your voice · CUDA — libonnxruntime.so",
    /// "Piper — natural, instant (stock voice) · WebGPU" — or the remote
    /// provider, "DeepInfra (remote)".
    pub fn label(&self) -> String {
        match self {
            // Story 3.17: the UX's own entry for the keyless one.
            BackendSelection::Remote(RemoteProvider::EdgeTts) => {
                "Edge TTS — free, online (stock voice)".to_string()
            }
            BackendSelection::Remote(provider) => format!("{} (remote)", provider.label()),
            local => {
                let engine = local.engine().map(LocalEngine::label).unwrap_or_default();
                match local.device_label() {
                    Some(device) => format!("{engine} · {device}"),
                    None => engine.to_string(),
                }
            }
        }
    }
}

/// A local engine — what the Local backend `Select` lists
/// (spec-backend-engine-and-device-selects). Piper and Chatterbox run on
/// ONNX Runtime and so also have a device; the System voice has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocalEngine {
    Piper,
    Chatterbox,
    SystemVoice,
}

impl LocalEngine {
    /// Every local engine, in the order the `Select` lists them.
    pub const ALL: [LocalEngine; 3] = [
        LocalEngine::Piper,
        LocalEngine::Chatterbox,
        LocalEngine::SystemVoice,
    ];

    /// How the engine reads in the backend `Select`, without a device.
    pub fn label(self) -> &'static str {
        match self {
            LocalEngine::Piper => "Piper — natural, instant (stock voice)",
            LocalEngine::Chatterbox => "Chatterbox — your voice",
            LocalEngine::SystemVoice => "System voice — instant (stock voice)",
        }
    }

    /// Whether the engine runs on an ONNX device, and so has a device
    /// `Select`.
    pub fn has_device(self) -> bool {
        !matches!(self, LocalEngine::SystemVoice)
    }

    /// The engine on the CPU: its default device.
    pub fn on_cpu(self) -> BackendSelection {
        match self {
            LocalEngine::Piper => BackendSelection::PIPER_CPU,
            LocalEngine::Chatterbox => BackendSelection::BUNDLED_CPU,
            LocalEngine::SystemVoice => BackendSelection::SystemVoice,
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

/// The local engines the backend `Select` lists under Local, in order:
/// Piper, Chatterbox, the System voice.
pub fn engine_choices() -> Vec<LocalEngine> {
    LocalEngine::ALL.to_vec()
}

/// The devices `engine` can run on, in the order the device `Select` lists
/// them: CPU on the bundled runtime, then CUDA and WebGPU on it when it
/// has every provider (`all_providers`, Story 3.8), then — for Chatterbox
/// only, since Piper runs on the bundled runtime — each added runtime's
/// entries. Empty for the System voice.
pub fn device_choices(
    engine: LocalEngine,
    runtimes: &[LocalRuntime],
    all_providers: bool,
) -> Vec<BackendSelection> {
    let bundled_targets: &[SpeechExecutionTarget] = if all_providers {
        &[
            SpeechExecutionTarget::Cpu,
            SpeechExecutionTarget::Cuda,
            SpeechExecutionTarget::WebGpu,
        ]
    } else {
        &[SpeechExecutionTarget::Cpu]
    };
    match engine {
        LocalEngine::SystemVoice => Vec::new(),
        LocalEngine::Piper => bundled_targets
            .iter()
            .map(|&target| BackendSelection::Piper { target })
            .collect(),
        LocalEngine::Chatterbox => bundled_targets
            .iter()
            .map(|&target| BackendSelection::Local {
                runtime: None,
                target,
            })
            .chain(runtimes.iter().flat_map(LocalRuntime::entries))
            .collect(),
    }
}

/// The selection a switch to `engine` saves: the same device when `engine`
/// offers it, otherwise CPU. An added runtime's device is only Chatterbox's,
/// so Piper keeps its target on the bundled runtime when that has it.
pub fn switch_engine(
    current: &BackendSelection,
    engine: LocalEngine,
    runtimes: &[LocalRuntime],
    all_providers: bool,
) -> BackendSelection {
    let devices = device_choices(engine, runtimes, all_providers);
    if devices.contains(current) {
        return current.clone();
    }
    let wanted = match (engine, current) {
        (LocalEngine::Piper, BackendSelection::Local { target, .. }) => {
            Some(BackendSelection::Piper { target: *target })
        }
        (LocalEngine::Chatterbox, BackendSelection::Piper { target }) => {
            Some(BackendSelection::Local {
                runtime: None,
                target: *target,
            })
        }
        _ => None,
    };
    wanted
        .filter(|wanted| devices.contains(wanted))
        .unwrap_or_else(|| engine.on_cpu())
}

/// Every entry a backend can be, grouped as the tab lists them: each local
/// engine's devices (or the System voice itself), then the remote
/// providers. Only the tests enumerate them all.
#[cfg(test)]
fn backend_choices(runtimes: &[LocalRuntime], all_providers: bool) -> Vec<BackendSelection> {
    LocalEngine::ALL
        .into_iter()
        .flat_map(|engine| {
            let devices = device_choices(engine, runtimes, all_providers);
            if devices.is_empty() {
                vec![engine.on_cpu()]
            } else {
                devices
            }
        })
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
    /// Story 3.14.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azure: Option<String>,
}

impl ApiKeys {
    /// The key for `provider`, if one is saved. A blank string is no key.
    pub fn get(&self, provider: RemoteProvider) -> Option<&str> {
        match provider {
            RemoteProvider::DeepInfra => self.deepinfra.as_deref(),
            RemoteProvider::FalAi => self.fal_ai.as_deref(),
            RemoteProvider::Azure => self.azure.as_deref(),
            // Story 3.17: Edge TTS has no key.
            RemoteProvider::EdgeTts => None,
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
            RemoteProvider::Azure => self.azure = key,
            // Story 3.17: Edge TTS has no key, so there is nothing to set.
            RemoteProvider::EdgeTts => {}
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
            .field("azure", &redact(&self.azure))
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
    /// The OS speech engine the System voice runs (Story 3.12: the
    /// `espeak-ng` program). Blocking, with manual steps on Linux: it is a
    /// system package voice-me cannot install. Piper phonemizes through it
    /// too; on Windows (Story 3.16) Piper's row installs it with one click.
    SystemVoiceEngine,
    /// The `edge-tts` program Edge TTS runs (Story 3.17). Blocking, with
    /// manual steps only: voice-me never runs pip for the user.
    EdgeTtsProgram,
    /// The Piper voice the selection speaks in (Story 3.15). Blocking; its
    /// Install downloads the voice named in the request.
    PiperVoice,
    /// ONNX Runtime's CUDA execution provider, beside the bundled runtime
    /// (Story 3.8). Reported for a CUDA backend on the bundled runtime
    /// only; blocking.
    CudaProvider,
    /// NVIDIA's CUDA libraries — cudart, cuBLAS, cuFFT, cuDNN — the CUDA
    /// provider loads (Story 3.8). Reported with [`Self::CudaProvider`];
    /// blocking.
    NvidiaLibraries,
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
                | DependencyKind::SystemVoiceEngine
                | DependencyKind::EdgeTtsProgram
                | DependencyKind::PiperVoice
                | DependencyKind::CudaProvider
                | DependencyKind::NvidiaLibraries
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
    /// The language generated speech is produced in (FR5), per backend
    /// (Story 3.11). Persisted, and set from Settings → Backend. Read on
    /// every Speak Action through [`Self::speech_language`].
    pub speech_languages: SpeechLanguages,
    /// The saved voice of each backend with a voice choice (Story 3.12).
    /// Persisted.
    pub speech_voices: SpeechVoices,
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
    /// The voices the System voice's engine listed at the last Dependency
    /// Check that had it selected (Story 3.12). Not persisted: like
    /// [`Self::dependencies`], it describes this machine as it was a moment
    /// ago, and the composition root merges it in.
    pub system_voices: Vec<StockVoice>,
    /// The Azure region (`westeurope`), saved next to the key (Story 3.14).
    /// Persisted; not a secret. Always `[a-z0-9]+` when set.
    pub azure_region: Option<String>,
    /// Azure's voice list as fetched at the last Dependency Check with
    /// Azure selected (Story 3.14, D1). Cached by the composition root for
    /// the session and never persisted.
    pub azure_voices: Vec<StockVoice>,
    /// The voices `edge-tts --list-voices` listed at the last Dependency
    /// Check with Edge TTS selected and the program found (Story 3.17).
    /// Merged in by the composition root; never persisted.
    pub edge_tts_voices: Vec<StockVoice>,
    /// The Piper voices installed in the cache (Story 3.15), read from disk
    /// by the composition root. Not persisted.
    pub piper_voices: Vec<StockVoice>,
    /// Where the Prompt Overlay opens vertically, 0 (top) to 100 (bottom)
    /// percent of the free space in the primary display's visible area.
    /// Persisted; set from Settings → Hotkey. Always `0..=100`.
    pub overlay_position: u8,
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
            speech_languages: SpeechLanguages::default(),
            speech_voices: SpeechVoices::default(),
            selected_mic_device: None,
            speech_backend: SpeechBackend::default(),
            backend_selection: BackendSelection::default(),
            local_runtimes: Vec::new(),
            api_keys: ApiKeys::default(),
            confirmed_disclosures: Vec::new(),
            remote_samples: Vec::new(),
            active_backend: ActiveBackend::default(),
            dependencies: DependencyOutcome::default(),
            system_voices: Vec::new(),
            azure_region: None,
            azure_voices: Vec::new(),
            edge_tts_voices: Vec::new(),
            piper_voices: Vec::new(),
            overlay_position: DEFAULT_OVERLAY_POSITION,
        }
    }
}

impl AppState {
    /// The selected backend's saved speech language, as stored (not yet
    /// normalised), or `None` when that backend has no speech language.
    pub fn speech_language(&self) -> Option<&str> {
        self.speech_languages
            .get(self.backend_selection.language_backend())
    }

    /// The run-time voice list `backend` resolves its languages and voices
    /// against: the System voice's engine list, Azure's voice list, and
    /// nothing for a backend with a static set.
    pub fn stock_voices(&self, backend: LanguageBackend) -> &[StockVoice] {
        match backend {
            LanguageBackend::SystemVoice => &self.system_voices,
            LanguageBackend::Remote(RemoteProvider::Azure) => &self.azure_voices,
            LanguageBackend::Remote(RemoteProvider::EdgeTts) => &self.edge_tts_voices,
            LanguageBackend::Piper => &self.piper_voices,
            _ => &[],
        }
    }

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
    fn the_overlay_sits_its_percentage_down_the_free_space() {
        // A 1080 px visible area starting under a 40 px panel, 200 px window.
        assert_eq!(overlay_origin_y(40.0, 1000.0, 200.0, 0), 40.0);
        assert_eq!(overlay_origin_y(40.0, 1000.0, 200.0, 100), 840.0);
        assert_eq!(overlay_origin_y(40.0, 1000.0, 200.0, 50), 440.0);
        assert_eq!(overlay_origin_y(40.0, 1000.0, 200.0, 20), 200.0);
        // Out of range counts as the bottom; a too-tall window, the top.
        assert_eq!(overlay_origin_y(40.0, 1000.0, 200.0, 250), 840.0);
        assert_eq!(overlay_origin_y(40.0, 100.0, 200.0, 70), 40.0);
    }

    #[test]
    fn the_overlay_defaults_to_centred() {
        assert_eq!(AppState::default().overlay_position, 50);
    }

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
        keys.set(RemoteProvider::Azure, Some("sk-secret-azure".to_string()));
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

    /// Story 3.17: only Edge TTS goes without a key; it is a stock voice
    /// listed after Azure, and its program blocks speech.
    #[test]
    fn edge_tts_needs_no_key_and_its_program_blocks_speech() {
        for provider in RemoteProvider::ALL {
            assert_eq!(
                provider.needs_api_key(),
                provider != RemoteProvider::EdgeTts,
                "{provider:?}"
            );
        }
        assert_eq!(RemoteProvider::ALL[3], RemoteProvider::EdgeTts);
        assert_eq!(RemoteProvider::EdgeTts.label(), "Edge TTS");
        assert!(RemoteProvider::EdgeTts.is_stock_voice());
        assert!(DependencyKind::EdgeTtsProgram.blocks_speech());

        let edge = LanguageBackend::Remote(RemoteProvider::EdgeTts);
        assert!(edge.has_voice_list());
        assert!(!edge.requires_voice());
        assert_eq!(SpeechLanguages::default().get(edge), Some("tr-TR"));

        let mut keys = ApiKeys::default();
        keys.set(RemoteProvider::EdgeTts, Some("anything".to_string()));
        assert_eq!(keys, ApiKeys::default());
        assert!(!keys.has(RemoteProvider::EdgeTts));

        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            provider: RemoteProvider,
        }
        let written = toml::to_string(&Wrapper {
            provider: RemoteProvider::EdgeTts,
        })
        .unwrap();
        assert_eq!(written.trim(), "provider = \"edge_tts\"");
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
        assert_eq!(
            cuda.entries()[0].device_label().as_deref(),
            Some("CUDA — libonnxruntime.so")
        );

        let cpu_only = LocalRuntime {
            path: PathBuf::from("/opt/cpu/libonnxruntime.so"),
            targets: vec![SpeechExecutionTarget::Cpu],
        };
        assert_eq!(
            cpu_only.entries()[0].device_label().as_deref(),
            Some("CPU — libonnxruntime.so")
        );

        let choices = backend_choices(&[cuda], false);
        // Piper first under Local, then Chatterbox's devices.
        assert_eq!(choices.first(), Some(&BackendSelection::PIPER_CPU));
        assert_eq!(choices[1], BackendSelection::BUNDLED_CPU);
        assert_eq!(choices[1].label(), "Chatterbox — your voice · CPU");
        assert_eq!(
            choices.last(),
            Some(&BackendSelection::Remote(RemoteProvider::EdgeTts))
        );
        assert_eq!(choices.len(), 8);
        // The System voice comes after the local runtimes, before remote.
        assert_eq!(choices[3], BackendSelection::SystemVoice);
    }

    /// The Engine list row: Piper, Chatterbox, the System voice, without a
    /// device in any label.
    #[test]
    fn the_engine_list_is_piper_chatterbox_then_the_system_voice() {
        assert_eq!(
            engine_choices(),
            vec![
                LocalEngine::Piper,
                LocalEngine::Chatterbox,
                LocalEngine::SystemVoice
            ]
        );
        for engine in engine_choices() {
            let label = engine.label();
            for device in ["CPU", "CUDA", "WebGPU", "runtime"] {
                assert!(!label.contains(device), "{label}");
            }
        }
        assert!(LocalEngine::Piper.has_device());
        assert!(LocalEngine::Chatterbox.has_device());
        assert!(!LocalEngine::SystemVoice.has_device());
        assert_eq!(
            BackendSelection::Piper {
                target: SpeechExecutionTarget::Cuda
            }
            .engine(),
            Some(LocalEngine::Piper)
        );
        assert_eq!(
            BackendSelection::BUNDLED_CPU.engine(),
            Some(LocalEngine::Chatterbox)
        );
        assert_eq!(
            BackendSelection::Remote(RemoteProvider::Azure).engine(),
            None
        );
    }

    /// The Device list and No device rows.
    #[test]
    fn the_device_list_follows_the_engine_and_the_bundled_runtime() {
        let added = LocalRuntime {
            path: PathBuf::from("/opt/ort/libonnxruntime.so"),
            targets: vec![SpeechExecutionTarget::Cpu, SpeechExecutionTarget::Cuda],
        };
        let runtimes = [added];

        let chatterbox = device_choices(LocalEngine::Chatterbox, &runtimes, true);
        let labels: Vec<_> = chatterbox
            .iter()
            .map(|selection| selection.device_label().unwrap())
            .collect();
        assert_eq!(
            labels,
            vec!["CPU", "CUDA", "WebGPU", "CUDA — libonnxruntime.so"]
        );

        // Before the all-provider runtime: the bundled runtime offers CPU
        // only.
        let labels: Vec<_> = device_choices(LocalEngine::Chatterbox, &runtimes, false)
            .iter()
            .map(|selection| selection.device_label().unwrap())
            .collect();
        assert_eq!(labels, vec!["CPU", "CUDA — libonnxruntime.so"]);

        // Piper runs on the bundled runtime only.
        assert_eq!(
            device_choices(LocalEngine::Piper, &runtimes, true),
            vec![
                BackendSelection::PIPER_CPU,
                BackendSelection::Piper {
                    target: SpeechExecutionTarget::Cuda
                },
                BackendSelection::Piper {
                    target: SpeechExecutionTarget::WebGpu
                },
            ]
        );
        assert_eq!(
            device_choices(LocalEngine::Piper, &runtimes, false),
            vec![BackendSelection::PIPER_CPU]
        );
        assert!(device_choices(LocalEngine::SystemVoice, &runtimes, true).is_empty());
        assert_eq!(BackendSelection::SystemVoice.device_label(), None);
        assert_eq!(
            BackendSelection::Remote(RemoteProvider::DeepInfra).device_label(),
            None
        );
    }

    /// The Engine switch row: the device is kept when the new engine offers
    /// it, otherwise CPU.
    #[test]
    fn switching_engine_keeps_the_device_when_the_new_engine_offers_it() {
        let webgpu = BackendSelection::Local {
            runtime: None,
            target: SpeechExecutionTarget::WebGpu,
        };
        assert_eq!(
            switch_engine(&webgpu, LocalEngine::Piper, &[], true),
            BackendSelection::Piper {
                target: SpeechExecutionTarget::WebGpu
            }
        );
        assert_eq!(
            switch_engine(
                &BackendSelection::Piper {
                    target: SpeechExecutionTarget::Cuda
                },
                LocalEngine::Chatterbox,
                &[],
                true
            ),
            BackendSelection::Local {
                runtime: None,
                target: SpeechExecutionTarget::Cuda
            }
        );
        // Not offered → CPU.
        assert_eq!(
            switch_engine(&webgpu, LocalEngine::Piper, &[], false),
            BackendSelection::PIPER_CPU
        );
        // An added runtime's device is Chatterbox's only; Piper takes the
        // same target on the bundled runtime.
        let added = BackendSelection::Local {
            runtime: Some(PathBuf::from("/opt/ort/libonnxruntime.so")),
            target: SpeechExecutionTarget::Cuda,
        };
        assert_eq!(
            switch_engine(&added, LocalEngine::Piper, &[], true),
            BackendSelection::Piper {
                target: SpeechExecutionTarget::Cuda
            }
        );
        assert_eq!(
            switch_engine(&added, LocalEngine::Piper, &[], false),
            BackendSelection::PIPER_CPU
        );
        assert_eq!(
            switch_engine(
                &BackendSelection::SystemVoice,
                LocalEngine::Chatterbox,
                &[],
                true
            ),
            BackendSelection::BUNDLED_CPU
        );
        assert_eq!(
            switch_engine(&webgpu, LocalEngine::SystemVoice, &[], true),
            BackendSelection::SystemVoice
        );
        assert_eq!(
            BackendSelection::Piper {
                target: SpeechExecutionTarget::Cuda
            }
            .on_cpu(),
            BackendSelection::PIPER_CPU
        );
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
    fn the_local_set_is_exactly_turkish_then_english() {
        let codes: Vec<_> = LanguageBackend::Local
            .speech_languages()
            .iter()
            .map(|language| language.code)
            .collect();
        assert_eq!(codes, vec!["tr", "en"]);
        assert_eq!(
            LanguageBackend::Local.speech_languages()[0].label,
            "Turkish"
        );
    }

    #[test]
    fn deepinfra_has_23_unique_languages_alphabetical_by_name() {
        let set = LanguageBackend::Remote(RemoteProvider::DeepInfra).speech_languages();
        assert_eq!(set.len(), 23);
        let codes: std::collections::HashSet<_> = set.iter().map(|l| l.code).collect();
        assert_eq!(codes.len(), 23);
        for code in
            "ar da de el en es fi fr he hi it ja ko ms nl no pl pt ru sv sw tr zh".split(' ')
        {
            assert!(codes.contains(code), "{code}");
        }
        let labels: Vec<_> = set.iter().map(|l| l.label).collect();
        let mut sorted = labels.clone();
        sorted.sort();
        assert_eq!(labels, sorted);
    }

    #[test]
    fn fal_ai_has_no_speech_language() {
        let fal = LanguageBackend::Remote(RemoteProvider::FalAi);
        assert!(fal.speech_languages().is_empty());
        assert!(!fal.has_speech_language());
        assert_eq!(SpeechLanguages::default().get(fal), None);
    }

    #[test]
    fn every_local_entry_shares_the_local_language() {
        let runtime = LocalRuntime {
            path: PathBuf::from("/opt/ort/libonnxruntime.so"),
            targets: vec![SpeechExecutionTarget::Cuda, SpeechExecutionTarget::WebGpu],
        };
        let cpu_only = LocalRuntime {
            path: PathBuf::from("/opt/cpu/libonnxruntime.so"),
            targets: vec![SpeechExecutionTarget::Cpu],
        };
        for selection in backend_choices(&[runtime, cpu_only], true) {
            let expected = match &selection {
                BackendSelection::Local { .. } => LanguageBackend::Local,
                BackendSelection::Remote(provider) => LanguageBackend::Remote(*provider),
                BackendSelection::SystemVoice => LanguageBackend::SystemVoice,
                BackendSelection::Piper { .. } => LanguageBackend::Piper,
            };
            assert_eq!(selection.language_backend(), expected);
        }

        let state = AppState {
            speech_languages: SpeechLanguages {
                local: "en".to_string(),
                deepinfra: "es".to_string(),
                ..SpeechLanguages::default()
            },
            backend_selection: BackendSelection::Local {
                runtime: Some(PathBuf::from("/opt/ort/libonnxruntime.so")),
                target: SpeechExecutionTarget::Cuda,
            },
            ..AppState::default()
        };
        assert_eq!(state.speech_language(), Some("en"));
    }

    #[test]
    fn a_capability_miss_blocks_speech() {
        assert!(DependencyKind::BackendCapability.blocks_speech());
    }

    fn voice(id: &str, language: &str, name: &str, priority: u32) -> StockVoice {
        StockVoice {
            id: id.to_string(),
            language: language.to_string(),
            language_label: name.to_string(),
            name: name.to_string(),
            priority,
        }
    }

    fn some_voices() -> Vec<StockVoice> {
        vec![
            voice("gmw/en", "en-gb", "English (Great Britain)", 2),
            voice("gmw/en-US", "en-us", "English (America)", 5),
            voice("trk/tr", "tr", "Turkish", 5),
            voice("sit/yue", "yue", "Chinese (Cantonese)", 5),
            voice(
                "sit/yue-Latn-jyutping",
                "yue",
                "Chinese (Cantonese, latin as Jyutping)",
                5,
            ),
        ]
    }

    /// Windows' voices report BCP-47 tags (Story 3.13).
    fn windows_voices() -> Vec<StockVoice> {
        vec![
            voice("tolga", "tr-TR", "Microsoft Tolga", 0),
            voice("david", "en-US", "Microsoft David", 1),
            voice("zira", "en-US", "Microsoft Zira", 2),
            voice("hazel", "en-GB", "Microsoft Hazel", 3),
        ]
    }

    /// Decision 1: the saved `tr` finds `tr-TR` — and `tr-TR` finds a bare
    /// `tr` the other way round.
    #[test]
    fn a_bare_primary_subtag_falls_back_to_a_listed_regional_tag() {
        let backend = LanguageBackend::SystemVoice;
        assert_eq!(
            backend.resolve_language("tr", &windows_voices()).as_deref(),
            Some("tr-TR")
        );
        assert_eq!(
            backend
                .resolve_language(" TR ", &windows_voices())
                .as_deref(),
            Some("tr-TR")
        );
        // Several share the subtag: the first listed.
        assert_eq!(
            backend.resolve_language("en", &windows_voices()).as_deref(),
            Some("en-US")
        );
        assert_eq!(
            resolve_stock_voice(backend, &windows_voices(), "tr", None)
                .unwrap()
                .id,
            "tolga"
        );
        assert_eq!(
            backend.resolve_language("tr-TR", &some_voices()).as_deref(),
            Some("tr")
        );
        // No language with that subtag at all.
        assert_eq!(backend.resolve_language("de", &windows_voices()), None);
        assert_eq!(backend.resolve_language("", &windows_voices()), None);
        // Azure's and Edge TTS's lists get the same fallback.
        assert_eq!(
            LanguageBackend::Remote(RemoteProvider::Azure)
                .resolve_language("tr", &windows_voices())
                .as_deref(),
            Some("tr-TR")
        );
    }

    /// Decision 1's fallback reads `_` and `-` alike.
    #[test]
    fn an_underscore_tag_finds_the_same_tag_with_a_hyphen() {
        let backend = LanguageBackend::SystemVoice;
        assert_eq!(
            backend
                .resolve_language("tr_TR", &windows_voices())
                .as_deref(),
            Some("tr-TR")
        );
        assert_eq!(
            backend
                .resolve_language("en_gb", &windows_voices())
                .as_deref(),
            Some("en-GB")
        );
        // Still never another region.
        assert_eq!(backend.resolve_language("en_AU", &windows_voices()), None);
    }

    #[test]
    fn an_exact_match_wins_over_the_primary_subtag() {
        let mut voices = windows_voices();
        voices.push(voice("bare", "en", "Bare English", 9));
        assert_eq!(
            LanguageBackend::SystemVoice
                .resolve_language("en", &voices)
                .as_deref(),
            Some("en")
        );
        assert_eq!(
            LanguageBackend::SystemVoice
                .resolve_language("en-gb", &voices)
                .as_deref(),
            Some("en-GB")
        );
    }

    /// eSpeak's codes resolve exactly as before: `en-gb` stays `en-gb`,
    /// and two regional tags never match each other.
    #[test]
    fn espeak_regional_codes_are_unchanged() {
        let backend = LanguageBackend::SystemVoice;
        assert_eq!(
            backend.resolve_language("en-gb", &some_voices()).as_deref(),
            Some("en-gb")
        );
        assert_eq!(
            backend.resolve_language("en-us", &some_voices()).as_deref(),
            Some("en-us")
        );
        assert_eq!(backend.resolve_language("en-029", &some_voices()), None);
        assert_eq!(
            backend.resolve_language("yue", &some_voices()).as_deref(),
            Some("yue")
        );
    }

    #[test]
    fn each_system_voice_language_code_is_its_own_language_sorted_by_label() {
        let languages = stock_voice_languages(&some_voices());
        let codes: Vec<_> = languages.iter().map(|l| l.code.as_str()).collect();
        assert_eq!(codes, vec!["yue", "en-us", "en-gb", "tr"]);
        assert_eq!(languages[0].label, "Chinese (Cantonese)");
        assert_eq!(
            LanguageBackend::SystemVoice.language_options(&some_voices()),
            languages
        );
        assert!(LanguageBackend::SystemVoice.has_speech_language());
        assert!(
            LanguageBackend::SystemVoice
                .language_options(&[])
                .is_empty()
        );
    }

    #[test]
    fn a_language_is_labelled_by_its_top_priority_voice() {
        let voices = vec![
            voice("a/low", "xx", "Second choice", 5),
            voice("a/high", "xx", "First choice", 2),
        ];
        assert_eq!(stock_voice_languages(&voices)[0].label, "First choice");
        assert_eq!(stock_voices_of(&voices, "xx")[0].id, "a/high");
    }

    #[test]
    fn the_system_voice_resolves_the_stored_voice_or_the_top_priority_one() {
        let voices = some_voices();
        assert_eq!(
            resolve_stock_voice(LanguageBackend::SystemVoice, &voices, "yue", None)
                .unwrap()
                .id,
            "sit/yue"
        );
        assert_eq!(
            resolve_stock_voice(
                LanguageBackend::SystemVoice,
                &voices,
                " YUE ",
                Some("sit/yue-Latn-jyutping")
            )
            .unwrap()
            .id,
            "sit/yue-Latn-jyutping"
        );
        assert_eq!(
            resolve_stock_voice(LanguageBackend::SystemVoice, &voices, "xx", None),
            Err(StockVoiceRefusal::Language {
                backend: LanguageBackend::SystemVoice,
                language: "xx".to_string()
            })
        );
        assert_eq!(
            resolve_stock_voice(LanguageBackend::SystemVoice, &voices, "tr", Some("sit/yue")),
            Err(StockVoiceRefusal::Voice {
                backend: LanguageBackend::SystemVoice,
                language: "tr".to_string(),
                voice: "sit/yue".to_string()
            })
        );
        assert_eq!(
            resolve_stock_voice(LanguageBackend::SystemVoice, &[], "tr", None),
            Err(StockVoiceRefusal::NoVoices(LanguageBackend::SystemVoice))
        );
    }

    #[test]
    fn the_system_voice_is_a_stock_voice_and_never_an_onnx_target() {
        let selection = BackendSelection::SystemVoice;
        assert_eq!(selection.local_target(), None);
        assert!(!selection.is_cpu());
        assert!(selection.is_stock_voice());
        assert!(!BackendSelection::BUNDLED_CPU.is_stock_voice());
        assert!(selection.label().contains("stock voice"));
        assert_eq!(selection.language_backend(), LanguageBackend::SystemVoice);
        assert!(DependencyKind::SystemVoiceEngine.blocks_speech());
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

    fn azure_voices() -> Vec<StockVoice> {
        vec![
            azure_voice("tr-TR-EmelNeural", "tr-TR", "Turkish (Türkiye)", "Emel"),
            azure_voice(
                "en-US-JennyNeural",
                "en-US",
                "English (United States)",
                "Jenny",
            ),
            azure_voice(
                "en-US-AriaNeural",
                "en-us",
                "English (United States)",
                "Aria",
            ),
        ]
    }

    #[test]
    fn azure_is_a_stock_voice_with_a_voice_list_and_a_required_voice() {
        let azure = BackendSelection::Remote(RemoteProvider::Azure);
        assert!(azure.is_stock_voice());
        assert!(!azure.is_system_voice());
        assert!(BackendSelection::SystemVoice.is_system_voice());
        assert!(!BackendSelection::Remote(RemoteProvider::DeepInfra).is_stock_voice());
        let backend = azure.language_backend();
        assert!(backend.has_voice_list() && backend.has_voice_choice());
        assert!(backend.requires_voice());
        assert!(!LanguageBackend::SystemVoice.requires_voice());
        assert!(backend.has_speech_language(), "even with no list yet");
        assert_eq!(backend.label(), "Azure");
        assert_eq!(SpeechLanguages::default().get(backend), Some("tr-TR"));
        assert_eq!(SpeechVoices::default().get(backend), None);
    }

    #[test]
    fn azure_locales_are_distinct_case_insensitively_and_labelled_by_locale_name() {
        let languages =
            LanguageBackend::Remote(RemoteProvider::Azure).language_options(&azure_voices());
        let codes: Vec<_> = languages.iter().map(|l| l.code.as_str()).collect();
        assert_eq!(codes, vec!["en-US", "tr-TR"]);
        assert_eq!(languages[1].label, "Turkish (Türkiye)");
        assert_eq!(stock_voices_of(&azure_voices(), "EN-us").len(), 2);
    }

    #[test]
    fn an_azure_voice_is_required_and_never_substituted() {
        let backend = LanguageBackend::Remote(RemoteProvider::Azure);
        let voices = azure_voices();
        assert_eq!(
            resolve_stock_voice(backend, &voices, "tr-tr", Some("tr-TR-EmelNeural"))
                .unwrap()
                .id,
            "tr-TR-EmelNeural"
        );
        assert_eq!(
            resolve_stock_voice(backend, &voices, "tr-TR", None),
            Err(StockVoiceRefusal::NoVoice(backend))
        );
        let stale =
            resolve_stock_voice(backend, &voices, "tr-TR", Some("tr-TR-AhmetNeural")).unwrap_err();
        let message = stale.to_string();
        assert!(
            message.contains("Azure") && message.contains("tr-TR-AhmetNeural"),
            "{message}"
        );
    }

    #[test]
    fn an_azure_region_is_trimmed_lowercased_and_letters_and_digits_only() {
        assert_eq!(
            parse_azure_region("  WestEurope\n"),
            Ok(Some("westeurope".to_string()))
        );
        assert_eq!(
            parse_azure_region("eastus2"),
            Ok(Some("eastus2".to_string()))
        );
        assert_eq!(parse_azure_region("   "), Ok(None));
        for bad in ["west europe!", "evil.com/", "a-b", "westeurope.attacker"] {
            assert!(parse_azure_region(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn piper_is_a_local_stock_voice_listed_first_with_a_voice_list() {
        let piper = BackendSelection::PIPER_CPU;
        assert_eq!(piper.local_target(), Some(SpeechExecutionTarget::Cpu));
        assert!(piper.is_cpu());
        assert!(!piper.is_chatterbox());
        let piper_cuda = BackendSelection::Piper {
            target: SpeechExecutionTarget::Cuda,
        };
        assert_eq!(piper_cuda.local_target(), Some(SpeechExecutionTarget::Cuda));
        assert!(!piper_cuda.is_cpu());
        assert!(piper_cuda.is_piper());
        assert!(piper.is_stock_voice());
        assert!(!piper.is_system_voice());
        assert!(piper.is_piper());
        assert_eq!(
            piper.label(),
            "Piper — natural, instant (stock voice) · CPU"
        );
        assert_eq!(backend_choices(&[], false)[0], piper);
        let backend = piper.language_backend();
        assert_eq!(backend, LanguageBackend::Piper);
        assert!(backend.has_voice_list() && backend.has_voice_choice());
        assert!(!backend.requires_voice());
        assert!(backend.has_speech_language(), "even with nothing installed");
        assert_eq!(SpeechLanguages::default().get(backend), Some("tr_TR"));
        assert_eq!(
            SpeechVoices::default().get(backend),
            Some("tr_TR-fahrettin-medium")
        );
        assert!(DependencyKind::PiperVoice.blocks_speech());
        assert!(DependencyKind::CudaProvider.blocks_speech());
        assert!(DependencyKind::NvidiaLibraries.blocks_speech());
    }

    /// The First run row at core level: with nothing saved, Linux and
    /// Windows (Story 3.16) speak with Piper and fahrettin; every other OS
    /// keeps the bundled CPU.
    #[test]
    fn an_unsaved_selection_is_piper_on_linux_and_the_bundled_cpu_elsewhere() {
        let state = AppState::default();
        if cfg!(any(target_os = "linux", target_os = "windows")) {
            assert_eq!(state.backend_selection, BackendSelection::PIPER_CPU);
            assert_eq!(state.speech_language(), Some("tr_TR"));
            assert_eq!(
                state.speech_voices.get(LanguageBackend::Piper),
                Some("tr_TR-fahrettin-medium")
            );
        } else {
            assert_eq!(state.backend_selection, BackendSelection::BUNDLED_CPU);
        }
    }

    #[test]
    fn piper_languages_are_the_installed_voices_locales() {
        let voices = vec![
            voice("tr_TR-fahrettin-medium", "tr_TR", "Turkish", 0),
            voice("tr_TR-dfki-medium", "tr_TR", "Turkish", 0),
            voice("en_US-lessac-medium", "en_US", "English", 0),
        ];
        let state = AppState {
            piper_voices: voices.clone(),
            ..AppState::default()
        };
        assert_eq!(state.stock_voices(LanguageBackend::Piper), &voices[..]);
        let codes: Vec<_> = LanguageBackend::Piper
            .language_options(&voices)
            .into_iter()
            .map(|language| language.code)
            .collect();
        assert_eq!(codes, vec!["en_US", "tr_TR"]);
        assert_eq!(
            resolve_stock_voice(LanguageBackend::Piper, &voices, "tr_TR", None)
                .unwrap()
                .id,
            "tr_TR-fahrettin-medium"
        );
    }
}
