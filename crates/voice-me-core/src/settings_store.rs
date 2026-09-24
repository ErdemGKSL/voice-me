//! `FileSettingsStore` — the concrete `SettingsStore` adapter (AD-6).
//!
//! Lives inside `voice-me-core` itself, not a separate adapter crate: one
//! TOML settings file at the OS config directory, and the Reference Voice
//! Sample audio file at the OS data directory, both resolved via the
//! `directories` crate. `AppState`'s active Reference Voice Sample field is
//! derived from whether that fixed-name file exists on disk — never a
//! separate ID/manifest.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::VoiceMeError;
use crate::ports::SettingsStore;
use crate::state::{
    ActiveBackend, ApiKeys, AppState, BackendSelection, DEFAULT_AZURE_LOCALE,
    DEFAULT_EDGE_TTS_LOCALE, DEFAULT_OVERLAY_POSITION, DEFAULT_PIPER_LOCALE,
    DEFAULT_SPEECH_LANGUAGE, DependencyOutcome, LanguageBackend, LocalRuntime, RemoteProvider,
    RemoteSample, SpeechBackend, SpeechExecutionTarget, SpeechLanguages, SpeechVoices,
    parse_azure_region,
};

const SETTINGS_FILE_NAME: &str = "settings.toml";
const REFERENCE_VOICE_SAMPLE_FILE_NAME: &str = "reference_voice_sample.wav";

fn default_ui_language() -> String {
    crate::state::DEFAULT_UI_LANGUAGE.to_string()
}

/// The subset of `AppState` that is actually serialized to TOML. The active
/// Reference Voice Sample is intentionally excluded — it's derived from file
/// presence in `data_dir`, per AD-6.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SettingsFile {
    #[serde(default)]
    hotkey: Option<String>,
    #[serde(default = "default_ui_language")]
    ui_language: String,
    /// The single speech language written before Story 3.11. Read only: it
    /// seeds whichever `[speech_languages]` entries the file lacks, and is
    /// never written back.
    #[serde(default, deserialize_with = "lenient", skip_serializing)]
    speech_language: Option<String>,
    /// Story 3.11: each backend's own speech language. Lenient per entry:
    /// an unreadable one falls back to its default, not the whole file.
    #[serde(default, deserialize_with = "lenient_languages")]
    speech_languages: SpeechLanguagesFile,
    /// Story 3.12: the voice of each backend with a voice choice. Lenient
    /// like the languages; an absent entry is the top-priority voice.
    #[serde(default, deserialize_with = "lenient_voices")]
    speech_voices: SpeechVoicesFile,
    #[serde(default)]
    selected_mic_device: Option<String>,
    /// Story 3.5. Absent in files written before it — which load as the
    /// bundled CPU runtime, the only backend those builds had.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    backend_selection: Option<SelectionFile>,
    /// Story 3.3: the ONNX Runtime libraries the user added.
    #[serde(
        default,
        deserialize_with = "lenient_list",
        skip_serializing_if = "Vec::is_empty"
    )]
    local_runtimes: Vec<LocalRuntime>,
    /// Stories 3.5/3.6: plaintext, as the UI says next to the fields.
    /// `ApiKeys`' own `Debug` keeps them out of this struct's. Lenient like
    /// the fields above: a malformed table loses the keys, not the file.
    #[serde(default, deserialize_with = "lenient_keys")]
    api_keys: ApiKeys,
    /// Story 3.14: the Azure region, next to the key. Not a secret. Lenient:
    /// an unreadable value is no region, and one that is not `[a-z0-9]+`
    /// is dropped on load, so it can never change the host.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    azure_region: Option<String>,
    /// Story 3.6: the providers whose disclosure the user confirmed. An
    /// unreadable entry drops out — at worst the user is asked again.
    #[serde(
        default,
        deserialize_with = "lenient_list",
        skip_serializing_if = "Vec::is_empty"
    )]
    confirmed_disclosures: Vec<RemoteProvider>,
    /// Story 3.6: the Reference Voice Sample as each provider holds it. An
    /// unreadable entry drops out — at worst the sample is uploaded again.
    #[serde(
        default,
        deserialize_with = "lenient_list",
        skip_serializing_if = "Vec::is_empty"
    )]
    remote_samples: Vec<RemoteSample>,
    /// Where the Prompt Overlay opens vertically, in percent. Lenient: an
    /// unreadable value is the default (centred in the free area), and one
    /// outside 0–100 is clamped on load. Read as `i64` so `250` or `-3`
    /// clamps rather than failing to fit; clamped, it is held as a `u8`.
    #[serde(
        default,
        deserialize_with = "lenient_percent",
        skip_serializing_if = "Option::is_none"
    )]
    overlay_position: Option<u8>,
}

/// How [`SpeechLanguages`] is written to TOML, keyed by backend slug:
///
/// ```toml
/// [speech_languages]
/// local = "tr"
/// deepinfra = "es"
/// ```
///
/// fal.ai has no speech language yet (Story 3.7 decides its set), so it has
/// no key here. `None` is an entry the file lacks, filled in by
/// [`FileSettingsStore::read_settings_file`] before anything reads it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SpeechLanguagesFile {
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    local: Option<String>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    deepinfra: Option<String>,
    /// Story 3.12. Never seeded by the legacy key.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    system_voice: Option<String>,
    /// Story 3.14: an Azure locale. Never seeded by the legacy key.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    azure: Option<String>,
    /// Story 3.17: an Edge TTS locale. Never seeded by the legacy key.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    edge_tts: Option<String>,
    /// Story 3.15: a Piper locale. Never seeded by the legacy key.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    piper: Option<String>,
}

impl SpeechLanguagesFile {
    fn entry(&mut self, backend: LanguageBackend) -> Option<&mut Option<String>> {
        match backend {
            LanguageBackend::Local => Some(&mut self.local),
            LanguageBackend::Remote(RemoteProvider::DeepInfra) => Some(&mut self.deepinfra),
            LanguageBackend::Remote(RemoteProvider::FalAi) => None,
            LanguageBackend::Remote(RemoteProvider::Azure) => Some(&mut self.azure),
            LanguageBackend::Remote(RemoteProvider::EdgeTts) => Some(&mut self.edge_tts),
            LanguageBackend::SystemVoice => Some(&mut self.system_voice),
            LanguageBackend::Piper => Some(&mut self.piper),
        }
    }

    fn to_state(&self) -> SpeechLanguages {
        let or_default =
            |entry: &Option<String>| entry.clone().unwrap_or(DEFAULT_SPEECH_LANGUAGE.to_string());
        SpeechLanguages {
            local: or_default(&self.local),
            deepinfra: or_default(&self.deepinfra),
            system_voice: or_default(&self.system_voice),
            azure: self
                .azure
                .clone()
                .unwrap_or_else(|| DEFAULT_AZURE_LOCALE.to_string()),
            edge_tts: self
                .edge_tts
                .clone()
                .unwrap_or_else(|| DEFAULT_EDGE_TTS_LOCALE.to_string()),
            piper: self
                .piper
                .clone()
                .unwrap_or_else(|| DEFAULT_PIPER_LOCALE.to_string()),
        }
    }
}

/// How [`SpeechVoices`] is written to TOML (Story 3.12):
///
/// ```toml
/// [speech_voices]
/// system_voice = "sit/yue-Latn-jyutping"
/// azure = "tr-TR-EmelNeural"
/// edge_tts = "tr-TR-EmelNeural"
/// ```
///
/// An absent key is the language's top-priority voice — for Azure, no
/// voice at all (D4).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SpeechVoicesFile {
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    system_voice: Option<String>,
    /// Story 3.14.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    azure: Option<String>,
    /// Story 3.17.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    edge_tts: Option<String>,
    /// Story 3.15: a Piper voice key. Absent is the language's
    /// top-priority installed voice; a profile that never saved a Piper
    /// language starts at the default voice (see `read_settings_file`).
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    piper: Option<String>,
}

impl SpeechVoicesFile {
    fn entry(&mut self, backend: LanguageBackend) -> Option<&mut Option<String>> {
        match backend {
            LanguageBackend::SystemVoice => Some(&mut self.system_voice),
            LanguageBackend::Remote(RemoteProvider::Azure) => Some(&mut self.azure),
            LanguageBackend::Remote(RemoteProvider::EdgeTts) => Some(&mut self.edge_tts),
            LanguageBackend::Piper => Some(&mut self.piper),
            _ => None,
        }
    }

    fn to_state(&self) -> SpeechVoices {
        SpeechVoices {
            system_voice: self.system_voice.clone(),
            azure: self.azure.clone(),
            edge_tts: self.edge_tts.clone(),
            piper: self.piper.clone(),
        }
    }
}

/// How a [`BackendSelection`] is written to TOML:
///
/// ```toml
/// [backend_selection]
/// kind = "local"
/// runtime = "/opt/onnxruntime/lib/libonnxruntime.so"
/// target = "cuda"
/// ```
///
/// A mirror rather than serde on the core type, so the in-memory shape
/// (`Remote(RemoteProvider)`) and the file format can each stay natural.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SelectionFile {
    Local {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runtime: Option<PathBuf>,
        target: SpeechExecutionTarget,
    },
    Remote {
        provider: RemoteProvider,
    },
    /// Story 3.12.
    SystemVoice,
    /// Story 3.15.
    Piper,
}

impl From<&BackendSelection> for SelectionFile {
    fn from(selection: &BackendSelection) -> Self {
        match selection {
            BackendSelection::Local { runtime, target } => SelectionFile::Local {
                runtime: runtime.clone(),
                target: *target,
            },
            BackendSelection::Remote(provider) => SelectionFile::Remote {
                provider: *provider,
            },
            BackendSelection::SystemVoice => SelectionFile::SystemVoice,
            BackendSelection::Piper => SelectionFile::Piper,
        }
    }
}

impl From<SelectionFile> for BackendSelection {
    fn from(file: SelectionFile) -> Self {
        match file {
            SelectionFile::Local { runtime, target } => BackendSelection::Local { runtime, target },
            SelectionFile::Remote { provider } => BackendSelection::Remote(provider),
            SelectionFile::SystemVoice => BackendSelection::SystemVoice,
            SelectionFile::Piper => BackendSelection::Piper,
        }
    }
}

/// A selection this build cannot read — a target a newer version wrote,
/// say — falls back to the default rather than making the whole settings
/// file unreadable: losing the hotkey over a backend name would be far
/// worse than landing on the CPU backend, which the Dependencies tab then
/// states plainly.
fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = toml::Value::deserialize(deserializer)?;
    Ok(T::deserialize(value).ok())
}

/// The same leniency for a percentage, clamped to 0–100 as it loads — the
/// one clamp on the way in.
fn lenient_percent<'de, D>(deserializer: D) -> Result<Option<u8>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(lenient::<_, i64>(deserializer)?.map(|percent| percent.clamp(0, 100) as u8))
}

/// The same leniency for the API keys: an unreadable table reads as no keys.
fn lenient_keys<'de, D>(deserializer: D) -> Result<ApiKeys, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(lenient(deserializer)?.unwrap_or_default())
}

/// The same leniency for the speech-language table: an unreadable table
/// reads as one with no entries, which the defaults then fill.
fn lenient_languages<'de, D>(deserializer: D) -> Result<SpeechLanguagesFile, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(lenient(deserializer)?.unwrap_or_default())
}

/// The same leniency for the voice table.
fn lenient_voices<'de, D>(deserializer: D) -> Result<SpeechVoicesFile, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(lenient(deserializer)?.unwrap_or_default())
}

/// The same leniency per entry: one unreadable runtime drops out of the
/// list without taking the others with it.
fn lenient_list<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let values = Vec::<toml::Value>::deserialize(deserializer).unwrap_or_default();
    Ok(values
        .into_iter()
        .filter_map(|value| T::deserialize(value).ok())
        .collect())
}

/// Hand-written rather than derived: `Default` is what a *missing* settings
/// file loads as, and a derived one would hand back empty strings for the
/// two language fields — which is not what `#[serde(default = ..)]` gives a
/// file that merely omits them. An empty speech language is not a harmless
/// blank either: it becomes an empty `[]` tag in the model prompt.
impl Default for SettingsFile {
    fn default() -> Self {
        Self {
            hotkey: None,
            ui_language: default_ui_language(),
            speech_language: None,
            speech_languages: SpeechLanguagesFile {
                local: Some(DEFAULT_SPEECH_LANGUAGE.to_string()),
                deepinfra: Some(DEFAULT_SPEECH_LANGUAGE.to_string()),
                system_voice: Some(DEFAULT_SPEECH_LANGUAGE.to_string()),
                azure: Some(DEFAULT_AZURE_LOCALE.to_string()),
                edge_tts: Some(DEFAULT_EDGE_TTS_LOCALE.to_string()),
                piper: Some(DEFAULT_PIPER_LOCALE.to_string()),
            },
            speech_voices: SpeechVoicesFile {
                piper: Some(crate::assets::PIPER_DEFAULT_VOICE.key.to_string()),
                ..SpeechVoicesFile::default()
            },
            selected_mic_device: None,
            backend_selection: None,
            local_runtimes: Vec::new(),
            api_keys: ApiKeys::default(),
            azure_region: None,
            confirmed_disclosures: Vec::new(),
            remote_samples: Vec::new(),
            overlay_position: None,
        }
    }
}

/// Concrete `SettingsStore` adapter backed by the local filesystem.
pub struct FileSettingsStore {
    config_dir: PathBuf,
    data_dir: PathBuf,
}

impl FileSettingsStore {
    /// Resolve the OS config/data directories via `directories::ProjectDirs`.
    pub fn new() -> Result<Self, VoiceMeError> {
        let dirs =
            directories::ProjectDirs::from("dev", "voice-me", "voice-me").ok_or_else(|| {
                VoiceMeError::Other("could not resolve platform directories".to_string())
            })?;
        Ok(Self::with_dirs(
            dirs.config_dir().to_path_buf(),
            dirs.data_dir().to_path_buf(),
        ))
    }

    /// Build a store against explicit directories — used in tests so no real
    /// OS config/data directory is touched.
    pub fn with_dirs(config_dir: PathBuf, data_dir: PathBuf) -> Self {
        Self {
            config_dir,
            data_dir,
        }
    }

    fn settings_path(&self) -> PathBuf {
        self.config_dir.join(SETTINGS_FILE_NAME)
    }

    fn reference_voice_sample_path(&self) -> PathBuf {
        self.data_dir.join(REFERENCE_VOICE_SAMPLE_FILE_NAME)
    }

    fn read_settings_file(&self) -> Result<SettingsFile, VoiceMeError> {
        let path = self.settings_path();
        if !path.exists() {
            return Ok(SettingsFile::default());
        }
        let contents = fs::read_to_string(&path)?;
        let mut settings: SettingsFile = toml::from_str(&contents)
            .map_err(|err| VoiceMeError::Other(format!("failed to parse settings file: {err}")))?;
        // Story 3.11 migration: a legacy single `speech_language` seeds the
        // Local and DeepInfra entries the table lacks — the table wins where
        // both exist — and is then dropped, so the next write carries the
        // table alone. Whatever is still missing gets the default, so every
        // write makes the languages explicit in the file.
        let legacy = settings.speech_language.take();
        for backend in [
            LanguageBackend::Local,
            LanguageBackend::Remote(RemoteProvider::DeepInfra),
        ] {
            if let Some(entry) = settings.speech_languages.entry(backend)
                && entry.is_none()
            {
                *entry = Some(
                    legacy
                        .clone()
                        .unwrap_or_else(|| DEFAULT_SPEECH_LANGUAGE.to_string()),
                );
            }
        }
        // Story 3.12: the System voice starts at the default, whatever the
        // legacy key said.
        settings
            .speech_languages
            .system_voice
            .get_or_insert_with(|| DEFAULT_SPEECH_LANGUAGE.to_string());
        // Story 3.14: Azure starts at its default locale, whatever the
        // legacy key said.
        settings
            .speech_languages
            .azure
            .get_or_insert_with(|| DEFAULT_AZURE_LOCALE.to_string());
        // Story 3.17: so does Edge TTS.
        settings
            .speech_languages
            .edge_tts
            .get_or_insert_with(|| DEFAULT_EDGE_TTS_LOCALE.to_string());
        // Story 3.15: a profile that never saved a Piper language starts at
        // the default voice and its locale. Once a language is saved, an
        // absent voice is that language's top-priority one — so a new
        // language (which clears the voice) is never pulled back here.
        if settings.speech_languages.piper.is_none() {
            settings.speech_languages.piper = Some(DEFAULT_PIPER_LOCALE.to_string());
            settings
                .speech_voices
                .piper
                .get_or_insert_with(|| crate::assets::PIPER_DEFAULT_VOICE.key.to_string());
        }
        // A hand-edited region that is not `[a-z0-9]+` is no region: it
        // must never reach the host name.
        settings.azure_region = settings
            .azure_region
            .as_deref()
            .and_then(|region| parse_azure_region(region).ok().flatten());
        Ok(settings)
    }

    fn write_settings_file(&self, settings: &SettingsFile) -> Result<(), VoiceMeError> {
        fs::create_dir_all(&self.config_dir)?;
        let contents = toml::to_string_pretty(settings)
            .map_err(|err| VoiceMeError::Other(format!("failed to serialize settings: {err}")))?;
        fs::write(self.settings_path(), contents)?;
        // The file holds API keys in plaintext (Stories 3.5/3.6), so on Unix
        // it is readable by its owner only — which is what the UI's notice
        // promises — including a file an older build left at 0644.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(self.settings_path(), fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    fn build_state(&self, settings: SettingsFile) -> AppState {
        let sample_path = self.reference_voice_sample_path();
        AppState {
            hotkey: settings.hotkey,
            reference_voice_sample: existing_path(&sample_path),
            ui_language: settings.ui_language,
            speech_languages: settings.speech_languages.to_state(),
            speech_voices: settings.speech_voices.to_state(),
            selected_mic_device: settings.selected_mic_device,
            // Not persisted: the composition root overwrites it with the
            // backend the selection below resolves to (AD-9).
            speech_backend: SpeechBackend::default(),
            backend_selection: settings
                .backend_selection
                .map(BackendSelection::from)
                .unwrap_or_default(),
            local_runtimes: settings.local_runtimes,
            api_keys: settings.api_keys,
            confirmed_disclosures: settings.confirmed_disclosures,
            remote_samples: settings.remote_samples,
            // A fact about this process, never read from a file.
            active_backend: ActiveBackend::default(),
            // Not persisted either, and for a stronger reason: a dependency
            // report describes the filesystem as it was a moment ago, so
            // one read back from a file would be a claim about a machine
            // that may have changed since. The composition root merges in
            // whatever the latest live check found.
            dependencies: DependencyOutcome::default(),
            // Read from the engine by the composition root, never a file.
            system_voices: Vec::new(),
            azure_region: settings.azure_region,
            // Fetched from Azure and cached by the composition root for
            // the session, never a file.
            azure_voices: Vec::new(),
            // Listed by the `edge-tts` program at a check, never a file.
            edge_tts_voices: Vec::new(),
            // Read from the cache by the composition root, never a file.
            piper_voices: Vec::new(),
            // Clamped to 0–100 as it loads.
            overlay_position: settings
                .overlay_position
                .unwrap_or(DEFAULT_OVERLAY_POSITION),
        }
    }
}

fn existing_path(path: &Path) -> Option<PathBuf> {
    path.exists().then(|| path.to_path_buf())
}

impl SettingsStore for FileSettingsStore {
    fn load(&self) -> Result<AppState, VoiceMeError> {
        let settings = self.read_settings_file()?;
        Ok(self.build_state(settings))
    }

    fn save_reference_voice_sample(&self, wav_bytes: &[u8]) -> Result<AppState, VoiceMeError> {
        fs::create_dir_all(&self.data_dir)?;

        // Do every fallible step that does NOT touch the previously active
        // clip first, so that if any of it fails, the function returns `Err`
        // with nothing changed on disk (the UI re-offers the clip for
        // retry). Persist the settings file (AD-6: one call writes both the
        // audio file and the updated settings, even though this story
        // doesn't change any of its fields) before the wav rename below.
        let settings = self.read_settings_file()?;
        self.write_settings_file(&settings)?;

        // Write to a temp file, then rename into place, so the previous
        // clip is only ever replaced by a complete new one (never left
        // half-written if this process is interrupted mid-write). This
        // rename is the last step and the only truly irreversible one.
        let final_path = self.reference_voice_sample_path();
        let tmp_path = self.data_dir.join(format!(
            "{REFERENCE_VOICE_SAMPLE_FILE_NAME}.tmp-{}",
            std::process::id()
        ));
        fs::write(&tmp_path, wav_bytes)?;
        if let Err(err) = fs::rename(&tmp_path, &final_path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(err.into());
        }

        Ok(self.build_state(settings))
    }

    fn save_selected_mic_device(&self, device: Option<&str>) -> Result<AppState, VoiceMeError> {
        let mut settings = self.read_settings_file()?;
        settings.selected_mic_device = device.map(str::to_string);
        self.write_settings_file(&settings)?;
        Ok(self.build_state(settings))
    }

    fn save_hotkey(&self, hotkey: Option<&str>) -> Result<AppState, VoiceMeError> {
        let mut settings = self.read_settings_file()?;
        settings.hotkey = hotkey.map(str::to_string);
        self.write_settings_file(&settings)?;
        Ok(self.build_state(settings))
    }

    fn save_backend_selection(
        &self,
        selection: &BackendSelection,
    ) -> Result<AppState, VoiceMeError> {
        let mut settings = self.read_settings_file()?;
        settings.backend_selection = Some(SelectionFile::from(selection));
        self.write_settings_file(&settings)?;
        Ok(self.build_state(settings))
    }

    fn save_speech_language(
        &self,
        backend: LanguageBackend,
        code: &str,
    ) -> Result<AppState, VoiceMeError> {
        let mut settings = self.read_settings_file()?;
        let Some(entry) = settings.speech_languages.entry(backend) else {
            return Err(VoiceMeError::Other(format!(
                "{} has no speech language to save",
                backend.label()
            )));
        };
        // Story 3.14: Azure's locales compare case-insensitively, so
        // `tr-tr` → `tr-TR` keeps the voice.
        let changed = !entry
            .as_deref()
            .is_some_and(|saved| saved.eq_ignore_ascii_case(code));
        *entry = Some(code.to_string());
        // Story 3.12: a voice belongs to one language, so a new System
        // voice language clears the stored voice.
        if changed && let Some(voice) = settings.speech_voices.entry(backend) {
            *voice = None;
        }
        self.write_settings_file(&settings)?;
        Ok(self.build_state(settings))
    }

    fn save_speech_voice(
        &self,
        backend: LanguageBackend,
        voice: Option<&str>,
    ) -> Result<AppState, VoiceMeError> {
        let mut settings = self.read_settings_file()?;
        let Some(entry) = settings.speech_voices.entry(backend) else {
            return Err(VoiceMeError::Other(format!(
                "{} has no voice to save",
                backend.label()
            )));
        };
        *entry = voice.map(str::to_string);
        self.write_settings_file(&settings)?;
        Ok(self.build_state(settings))
    }

    fn save_local_runtimes(&self, runtimes: &[LocalRuntime]) -> Result<AppState, VoiceMeError> {
        let mut settings = self.read_settings_file()?;
        settings.local_runtimes = runtimes.to_vec();
        self.write_settings_file(&settings)?;
        Ok(self.build_state(settings))
    }

    fn save_api_key(
        &self,
        provider: RemoteProvider,
        key: Option<&str>,
    ) -> Result<AppState, VoiceMeError> {
        let mut settings = self.read_settings_file()?;
        settings.api_keys.set(provider, key.map(str::to_string));
        self.write_settings_file(&settings)?;
        Ok(self.build_state(settings))
    }

    fn save_azure_region(&self, region: Option<&str>) -> Result<AppState, VoiceMeError> {
        let region = match region {
            Some(region) => parse_azure_region(region).map_err(VoiceMeError::Other)?,
            None => None,
        };
        let mut settings = self.read_settings_file()?;
        settings.azure_region = region;
        self.write_settings_file(&settings)?;
        Ok(self.build_state(settings))
    }

    fn save_overlay_position(&self, percent: u8) -> Result<AppState, VoiceMeError> {
        let mut settings = self.read_settings_file()?;
        settings.overlay_position = Some(percent.min(100));
        self.write_settings_file(&settings)?;
        Ok(self.build_state(settings))
    }

    fn save_disclosure_confirmed(
        &self,
        provider: RemoteProvider,
    ) -> Result<AppState, VoiceMeError> {
        let mut settings = self.read_settings_file()?;
        if !settings.confirmed_disclosures.contains(&provider) {
            settings.confirmed_disclosures.push(provider);
        }
        self.write_settings_file(&settings)?;
        Ok(self.build_state(settings))
    }

    fn save_remote_sample(
        &self,
        provider: RemoteProvider,
        sample: Option<RemoteSample>,
    ) -> Result<AppState, VoiceMeError> {
        let mut settings = self.read_settings_file()?;
        settings
            .remote_samples
            .retain(|held| held.provider != provider);
        if let Some(sample) = sample {
            settings
                .remote_samples
                .push(RemoteSample { provider, ..sample });
        }
        self.write_settings_file(&settings)?;
        Ok(self.build_state(settings))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_with_no_files_yet_reports_no_active_sample() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let store = FileSettingsStore::with_dirs(
            config_dir.path().to_path_buf(),
            data_dir.path().to_path_buf(),
        );

        let state = store.load().unwrap();

        assert_eq!(state.reference_voice_sample, None);
    }

    #[test]
    fn save_reference_voice_sample_round_trips() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let store = FileSettingsStore::with_dirs(
            config_dir.path().to_path_buf(),
            data_dir.path().to_path_buf(),
        );

        let wav_bytes = b"fake wav bytes".to_vec();
        let state = store.save_reference_voice_sample(&wav_bytes).unwrap();

        let saved_path = state.reference_voice_sample.clone().unwrap();
        assert_eq!(fs::read(&saved_path).unwrap(), wav_bytes);

        // A fresh store instance (simulating a new process) still reports it.
        let reloaded_store = FileSettingsStore::with_dirs(
            config_dir.path().to_path_buf(),
            data_dir.path().to_path_buf(),
        );
        let reloaded_state = reloaded_store.load().unwrap();
        assert_eq!(reloaded_state.reference_voice_sample, Some(saved_path));
    }

    #[test]
    fn re_recording_replaces_the_previously_active_sample() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let store = FileSettingsStore::with_dirs(
            config_dir.path().to_path_buf(),
            data_dir.path().to_path_buf(),
        );

        let first = b"first clip".to_vec();
        let state1 = store.save_reference_voice_sample(&first).unwrap();
        let path = state1.reference_voice_sample.unwrap();
        assert_eq!(fs::read(&path).unwrap(), first);

        let second = b"second clip, replaces the first".to_vec();
        let state2 = store.save_reference_voice_sample(&second).unwrap();
        let path2 = state2.reference_voice_sample.unwrap();

        assert_eq!(path, path2, "same fixed filename, not a new id/manifest");
        assert_eq!(fs::read(&path2).unwrap(), second);
    }

    #[test]
    fn save_selected_mic_device_round_trips_across_a_fresh_load() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let store = FileSettingsStore::with_dirs(
            config_dir.path().to_path_buf(),
            data_dir.path().to_path_buf(),
        );
        assert_eq!(store.load().unwrap().selected_mic_device, None);

        let state = store.save_selected_mic_device(Some("USB Mic")).unwrap();
        assert_eq!(state.selected_mic_device, Some("USB Mic".to_string()));

        let reloaded_store = FileSettingsStore::with_dirs(
            config_dir.path().to_path_buf(),
            data_dir.path().to_path_buf(),
        );
        assert_eq!(
            reloaded_store.load().unwrap().selected_mic_device,
            Some("USB Mic".to_string())
        );

        // `None` clears the selection back to the OS default.
        let cleared = store.save_selected_mic_device(None).unwrap();
        assert_eq!(cleared.selected_mic_device, None);
    }

    #[test]
    fn save_hotkey_round_trips_across_a_fresh_load() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let store = FileSettingsStore::with_dirs(
            config_dir.path().to_path_buf(),
            data_dir.path().to_path_buf(),
        );
        assert_eq!(store.load().unwrap().hotkey, None);

        let state = store.save_hotkey(Some("Ctrl+Alt+KeyV")).unwrap();
        assert_eq!(state.hotkey, Some("Ctrl+Alt+KeyV".to_string()));

        // A brand new store (simulating the next run of the process) still
        // reports it — this is what re-activates the hotkey at next startup.
        let reloaded_store = FileSettingsStore::with_dirs(
            config_dir.path().to_path_buf(),
            data_dir.path().to_path_buf(),
        );
        assert_eq!(
            reloaded_store.load().unwrap().hotkey,
            Some("Ctrl+Alt+KeyV".to_string())
        );

        // Saving a different combination replaces the previous one rather
        // than accumulating bindings.
        let replaced = store.save_hotkey(Some("Ctrl+Alt+KeyB")).unwrap();
        assert_eq!(replaced.hotkey, Some("Ctrl+Alt+KeyB".to_string()));

        // `None` clears the configured hotkey back to "none configured".
        let cleared = store.save_hotkey(None).unwrap();
        assert_eq!(cleared.hotkey, None);
        let reloaded_store = FileSettingsStore::with_dirs(
            config_dir.path().to_path_buf(),
            data_dir.path().to_path_buf(),
        );
        assert_eq!(reloaded_store.load().unwrap().hotkey, None);
    }

    #[test]
    fn the_speech_languages_default_to_turkish_and_survive_an_unrelated_save() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let store = store_in(config_dir.path(), data_dir.path());

        // No settings file at all yet (first run).
        let state = store.load().unwrap();
        assert_eq!(state.speech_languages, SpeechLanguages::default());
        assert_eq!(state.speech_languages.local, "tr");
        // Story 3.15: the unsaved selection (Piper on Linux, and on Windows
        // since Story 3.16) reads its own.
        assert_eq!(
            state.speech_language(),
            SpeechLanguages::default().get(BackendSelection::default().language_backend())
        );

        // Any save writes the whole file, so the defaults become explicit.
        store.save_hotkey(Some("Ctrl+Alt+KeyV")).unwrap();
        let written = fs::read_to_string(config_dir.path().join(SETTINGS_FILE_NAME)).unwrap();
        assert!(written.contains("[speech_languages]"), "{written}");
        assert!(written.contains("local = \"tr\""), "{written}");
        assert!(written.contains("deepinfra = \"tr\""), "{written}");
    }

    /// The I/O matrix's Migration row, across a fresh store.
    #[test]
    fn a_legacy_speech_language_seeds_local_and_deepinfra_and_is_not_written_back() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        fs::write(
            config_dir.path().join(SETTINGS_FILE_NAME),
            "speech_language = \"en\"\n",
        )
        .unwrap();

        let state = store_in(config_dir.path(), data_dir.path()).load().unwrap();
        assert_eq!(state.speech_languages.local, "en");
        assert_eq!(state.speech_languages.deepinfra, "en");
        // A file that omits the other fields still gets their defaults.
        assert_eq!(state.ui_language, "en");

        store_in(config_dir.path(), data_dir.path())
            .save_hotkey(Some("Ctrl+Alt+KeyV"))
            .unwrap();
        let written = fs::read_to_string(config_dir.path().join(SETTINGS_FILE_NAME)).unwrap();
        assert!(written.contains("[speech_languages]"), "{written}");
        assert!(
            !written.contains("speech_language ="),
            "the legacy key is not written back: {written}"
        );

        let reloaded = store_in(config_dir.path(), data_dir.path()).load().unwrap();
        assert_eq!(reloaded.speech_languages.local, "en");
        assert_eq!(reloaded.speech_languages.deepinfra, "en");
        assert_eq!(reloaded.hotkey.as_deref(), Some("Ctrl+Alt+KeyV"));
    }

    #[test]
    fn the_table_wins_over_the_legacy_key() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        fs::write(
            config_dir.path().join(SETTINGS_FILE_NAME),
            "speech_language = \"en\"\n\n[speech_languages]\ndeepinfra = \"es\"\n",
        )
        .unwrap();

        let state = store_in(config_dir.path(), data_dir.path()).load().unwrap();

        assert_eq!(state.speech_languages.deepinfra, "es", "the table wins");
        assert_eq!(
            state.speech_languages.local, "en",
            "the legacy key fills only the entry the table lacks"
        );
    }

    #[test]
    fn saving_one_backends_language_leaves_the_others_alone() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let store = store_in(config_dir.path(), data_dir.path());
        store
            .save_speech_language(LanguageBackend::Local, "en")
            .unwrap();

        let state = store
            .save_speech_language(LanguageBackend::Remote(RemoteProvider::DeepInfra), "es")
            .unwrap();

        assert_eq!(state.speech_languages.deepinfra, "es");
        assert_eq!(state.speech_languages.local, "en");
        assert_eq!(state.ui_language, "en", "the UI language is untouched");
        let reloaded = store_in(config_dir.path(), data_dir.path()).load().unwrap();
        assert_eq!(reloaded.speech_languages, state.speech_languages);
        assert_eq!(reloaded.ui_language, "en");

        // fal.ai has no speech language to save.
        assert!(
            store
                .save_speech_language(LanguageBackend::Remote(RemoteProvider::FalAi), "en")
                .is_err()
        );
    }

    #[test]
    fn an_unreadable_speech_language_falls_back_without_losing_the_file() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        fs::write(
            config_dir.path().join(SETTINGS_FILE_NAME),
            "hotkey = \"Ctrl+Alt+KeyV\"\n\n[speech_languages]\nlocal = 42\ndeepinfra = \"es\"\n",
        )
        .unwrap();

        let state = store_in(config_dir.path(), data_dir.path()).load().unwrap();

        assert_eq!(state.speech_languages.local, "tr");
        assert_eq!(state.speech_languages.deepinfra, "es");
        assert_eq!(state.hotkey.as_deref(), Some("Ctrl+Alt+KeyV"));

        // A table that is not a table at all loses the entries, not the file.
        fs::write(
            config_dir.path().join(SETTINGS_FILE_NAME),
            "hotkey = \"Ctrl+Alt+KeyV\"\nspeech_languages = 7\n",
        )
        .unwrap();
        let state = store_in(config_dir.path(), data_dir.path()).load().unwrap();
        assert_eq!(state.speech_languages, SpeechLanguages::default());
        assert_eq!(state.hotkey.as_deref(), Some("Ctrl+Alt+KeyV"));
    }

    #[test]
    fn the_system_voice_language_and_voice_round_trip_and_a_new_language_clears_the_voice() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let store = store_in(config_dir.path(), data_dir.path());
        let state = store.load().unwrap();
        assert_eq!(state.speech_languages.system_voice, "tr");
        assert_eq!(state.speech_voices.system_voice, None);

        store
            .save_backend_selection(&BackendSelection::SystemVoice)
            .unwrap();
        store
            .save_speech_language(LanguageBackend::SystemVoice, "yue")
            .unwrap();
        store
            .save_speech_voice(LanguageBackend::SystemVoice, Some("sit/yue-Latn-jyutping"))
            .unwrap();

        let reloaded = store_in(config_dir.path(), data_dir.path()).load().unwrap();
        assert_eq!(reloaded.backend_selection, BackendSelection::SystemVoice);
        assert_eq!(reloaded.speech_languages.system_voice, "yue");
        assert_eq!(
            reloaded.speech_voices.system_voice.as_deref(),
            Some("sit/yue-Latn-jyutping")
        );
        assert_eq!(reloaded.speech_languages.local, "tr", "Local is untouched");
        let written = fs::read_to_string(config_dir.path().join(SETTINGS_FILE_NAME)).unwrap();
        assert!(written.contains("[speech_voices]"), "{written}");
        assert!(written.contains("system_voice"), "{written}");

        // Re-saving the same language keeps the voice; a new one clears it.
        let same = store
            .save_speech_language(LanguageBackend::SystemVoice, "yue")
            .unwrap();
        assert!(same.speech_voices.system_voice.is_some());
        let state = store
            .save_speech_language(LanguageBackend::SystemVoice, "tr")
            .unwrap();
        assert_eq!(state.speech_languages.system_voice, "tr");
        assert_eq!(state.speech_voices.system_voice, None);
        assert_eq!(
            store_in(config_dir.path(), data_dir.path())
                .load()
                .unwrap()
                .speech_voices
                .system_voice,
            None
        );

        // Only the System voice has a voice to save.
        assert!(
            store
                .save_speech_voice(LanguageBackend::Local, Some("x"))
                .is_err()
        );
    }

    #[test]
    fn the_legacy_key_does_not_seed_the_system_voice() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        fs::write(
            config_dir.path().join(SETTINGS_FILE_NAME),
            "speech_language = \"en\"\n",
        )
        .unwrap();

        let state = store_in(config_dir.path(), data_dir.path()).load().unwrap();

        assert_eq!(state.speech_languages.local, "en");
        assert_eq!(state.speech_languages.system_voice, "tr");
    }

    #[test]
    fn save_hotkey_leaves_the_other_settings_intact() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let store = store_in(config_dir.path(), data_dir.path());
        store
            .save_speech_language(LanguageBackend::Local, "en")
            .unwrap();

        store.save_selected_mic_device(Some("USB Mic")).unwrap();
        let state = store.save_hotkey(Some("Ctrl+Alt+KeyV")).unwrap();

        assert_eq!(state.selected_mic_device, Some("USB Mic".to_string()));
        assert_eq!(state.hotkey, Some("Ctrl+Alt+KeyV".to_string()));
        assert_eq!(state.speech_languages.local, "en");
    }

    fn store_in(config_dir: &Path, data_dir: &Path) -> FileSettingsStore {
        FileSettingsStore::with_dirs(config_dir.to_path_buf(), data_dir.to_path_buf())
    }

    /// Story 3.15: a file with no saved selection loads as the default —
    /// Piper on Linux and Windows (Story 3.16), the bundled CPU runtime
    /// elsewhere.
    #[test]
    fn a_file_from_before_story_3_5_loads_as_the_default_backend() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        fs::write(
            config_dir.path().join(SETTINGS_FILE_NAME),
            "hotkey = \"Ctrl+Alt+KeyV\"\nspeech_language = \"en\"\n",
        )
        .unwrap();

        let state = store_in(config_dir.path(), data_dir.path()).load().unwrap();

        assert_eq!(state.backend_selection, BackendSelection::default());
        if cfg!(any(target_os = "linux", target_os = "windows")) {
            assert_eq!(state.backend_selection, BackendSelection::Piper);
        } else {
            assert_eq!(state.backend_selection, BackendSelection::BUNDLED_CPU);
        }
        assert!(state.local_runtimes.is_empty());
        assert_eq!(state.api_keys, ApiKeys::default());
        assert_eq!(state.hotkey.as_deref(), Some("Ctrl+Alt+KeyV"));
    }

    #[test]
    fn the_backend_selection_round_trips_across_a_fresh_load() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let store = store_in(config_dir.path(), data_dir.path());

        let cuda = BackendSelection::Local {
            runtime: Some(PathBuf::from("/opt/ort/libonnxruntime.so")),
            target: SpeechExecutionTarget::Cuda,
        };
        store.save_backend_selection(&cuda).unwrap();
        assert_eq!(
            store_in(config_dir.path(), data_dir.path())
                .load()
                .unwrap()
                .backend_selection,
            cuda
        );

        let remote = BackendSelection::Remote(RemoteProvider::FalAi);
        store.save_backend_selection(&remote).unwrap();
        assert_eq!(store.load().unwrap().backend_selection, remote);

        let written = fs::read_to_string(config_dir.path().join(SETTINGS_FILE_NAME)).unwrap();
        assert!(written.contains("fal_ai"), "{written}");
    }

    #[test]
    fn added_runtimes_round_trip_and_leave_the_other_settings_alone() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let store = store_in(config_dir.path(), data_dir.path());
        store.save_hotkey(Some("Ctrl+Alt+KeyV")).unwrap();

        let runtimes = vec![LocalRuntime {
            path: PathBuf::from("/opt/ort/libonnxruntime.so"),
            targets: vec![SpeechExecutionTarget::Cpu, SpeechExecutionTarget::WebGpu],
        }];
        store.save_local_runtimes(&runtimes).unwrap();

        let reloaded = store_in(config_dir.path(), data_dir.path()).load().unwrap();
        assert_eq!(reloaded.local_runtimes, runtimes);
        assert_eq!(reloaded.hotkey.as_deref(), Some("Ctrl+Alt+KeyV"));

        let cleared = store.save_local_runtimes(&[]).unwrap();
        assert!(cleared.local_runtimes.is_empty());
    }

    #[test]
    fn api_keys_are_stored_independently_and_can_be_removed() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let store = store_in(config_dir.path(), data_dir.path());

        store
            .save_api_key(RemoteProvider::DeepInfra, Some("di-key"))
            .unwrap();
        let state = store
            .save_api_key(RemoteProvider::FalAi, Some("fal-key"))
            .unwrap();
        assert_eq!(
            state.api_keys.get(RemoteProvider::DeepInfra),
            Some("di-key")
        );
        assert_eq!(state.api_keys.get(RemoteProvider::FalAi), Some("fal-key"));

        // Decision: plaintext in the settings file, which the UI states.
        let written = fs::read_to_string(config_dir.path().join(SETTINGS_FILE_NAME)).unwrap();
        assert!(written.contains("di-key"), "{written}");

        let state = store.save_api_key(RemoteProvider::DeepInfra, None).unwrap();
        assert_eq!(state.api_keys.get(RemoteProvider::DeepInfra), None);
        assert_eq!(
            store_in(config_dir.path(), data_dir.path())
                .load()
                .unwrap()
                .api_keys
                .get(RemoteProvider::FalAi),
            Some("fal-key")
        );
    }

    #[test]
    fn an_unreadable_selection_falls_back_without_losing_the_rest_of_the_file() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        fs::write(
            config_dir.path().join(SETTINGS_FILE_NAME),
            "hotkey = \"Ctrl+Alt+KeyV\"\n\n[backend_selection]\nkind = \"local\"\ntarget = \"tpu\"\n",
        )
        .unwrap();

        let state = store_in(config_dir.path(), data_dir.path()).load().unwrap();

        // The default — Piper on Linux since Story 3.15, and on Windows
        // since Story 3.16.
        assert_eq!(state.backend_selection, BackendSelection::default());
        assert_eq!(state.hotkey.as_deref(), Some("Ctrl+Alt+KeyV"));
    }

    #[test]
    fn the_settings_files_debug_output_never_contains_a_key() {
        let settings = SettingsFile {
            api_keys: {
                let mut keys = ApiKeys::default();
                keys.set(
                    RemoteProvider::DeepInfra,
                    Some("sk-very-secret".to_string()),
                );
                keys
            },
            ..SettingsFile::default()
        };

        assert!(!format!("{settings:?}").contains("sk-very-secret"));
    }

    #[cfg(unix)]
    #[test]
    fn the_settings_file_is_readable_by_its_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let path = config_dir.path().join(SETTINGS_FILE_NAME);
        fs::write(&path, "hotkey = \"Ctrl+Alt+KeyV\"\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        store_in(config_dir.path(), data_dir.path())
            .save_api_key(RemoteProvider::DeepInfra, Some("di-key"))
            .unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{mode:o}");
    }

    #[test]
    fn a_confirmed_disclosure_round_trips_once_per_provider() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let store = store_in(config_dir.path(), data_dir.path());
        assert!(store.load().unwrap().confirmed_disclosures.is_empty());

        store
            .save_disclosure_confirmed(RemoteProvider::DeepInfra)
            .unwrap();
        let state = store
            .save_disclosure_confirmed(RemoteProvider::DeepInfra)
            .unwrap();
        assert_eq!(state.confirmed_disclosures, vec![RemoteProvider::DeepInfra]);

        let reloaded = store_in(config_dir.path(), data_dir.path()).load().unwrap();
        assert!(reloaded.disclosure_confirmed(RemoteProvider::DeepInfra));
        assert!(!reloaded.disclosure_confirmed(RemoteProvider::FalAi));
    }

    #[test]
    fn a_remote_sample_round_trips_is_replaced_and_is_forgotten() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let store = store_in(config_dir.path(), data_dir.path());
        store
            .save_api_key(RemoteProvider::DeepInfra, Some("di-key"))
            .unwrap();
        let sample = |hash: &str, id: &str| RemoteSample {
            provider: RemoteProvider::DeepInfra,
            sample_sha256: hash.to_string(),
            voice_id: id.to_string(),
        };

        store
            .save_remote_sample(RemoteProvider::DeepInfra, Some(sample("aa", "v1")))
            .unwrap();
        store
            .save_remote_sample(RemoteProvider::DeepInfra, Some(sample("bb", "v2")))
            .unwrap();
        store
            .save_disclosure_confirmed(RemoteProvider::DeepInfra)
            .unwrap();

        let reloaded = store_in(config_dir.path(), data_dir.path());
        assert_eq!(
            reloaded
                .load_remote_sample(RemoteProvider::DeepInfra)
                .unwrap(),
            Some(sample("bb", "v2")),
            "one sample per provider: the newer one replaces the older"
        );
        assert_eq!(reloaded.load().unwrap().remote_samples.len(), 1);
        assert_eq!(
            reloaded
                .load()
                .unwrap()
                .api_keys
                .get(RemoteProvider::DeepInfra),
            Some("di-key")
        );
        let written = fs::read_to_string(config_dir.path().join(SETTINGS_FILE_NAME)).unwrap();
        assert!(written.contains("voice_id = \"v2\""), "{written}");

        let state = store
            .save_remote_sample(RemoteProvider::DeepInfra, None)
            .unwrap();
        assert!(state.remote_samples.is_empty());
        assert!(state.disclosure_confirmed(RemoteProvider::DeepInfra));
    }

    #[test]
    fn unreadable_remote_entries_drop_out_without_losing_the_file() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        fs::write(
            config_dir.path().join(SETTINGS_FILE_NAME),
            "hotkey = \"Ctrl+Alt+KeyV\"\nconfirmed_disclosures = [\"deepinfra\", \"nowhere\"]\n\n\
             [[remote_samples]]\nprovider = \"deepinfra\"\nsample_sha256 = \"aa\"\nvoice_id = \"v1\"\n\n\
             [[remote_samples]]\nprovider = \"deepinfra\"\n",
        )
        .unwrap();

        let state = store_in(config_dir.path(), data_dir.path()).load().unwrap();

        assert_eq!(state.confirmed_disclosures, vec![RemoteProvider::DeepInfra]);
        assert_eq!(state.remote_samples.len(), 1);
        assert_eq!(state.hotkey.as_deref(), Some("Ctrl+Alt+KeyV"));
    }

    #[test]
    fn a_malformed_api_keys_table_loses_the_keys_not_the_file() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        fs::write(
            config_dir.path().join(SETTINGS_FILE_NAME),
            "hotkey = \"Ctrl+Alt+KeyV\"\napi_keys = 42\n",
        )
        .unwrap();

        let state = store_in(config_dir.path(), data_dir.path()).load().unwrap();

        assert_eq!(state.api_keys, ApiKeys::default());
        assert_eq!(state.hotkey.as_deref(), Some("Ctrl+Alt+KeyV"));
    }

    #[test]
    fn overlay_position_defaults_to_centred_and_round_trips() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let store = store_in(config_dir.path(), data_dir.path());
        store.save_hotkey(Some("Ctrl+Alt+KeyV")).unwrap();
        let written = fs::read_to_string(config_dir.path().join(SETTINGS_FILE_NAME)).unwrap();
        assert!(!written.contains("overlay_position"), "{written}");
        assert_eq!(store.load().unwrap().overlay_position, 50);

        let state = store.save_overlay_position(30).unwrap();
        assert_eq!(state.overlay_position, 30);
        assert_eq!(state.hotkey.as_deref(), Some("Ctrl+Alt+KeyV"));
        let written = fs::read_to_string(config_dir.path().join(SETTINGS_FILE_NAME)).unwrap();
        assert!(written.contains("overlay_position = 30"), "{written}");

        let reloaded = store_in(config_dir.path(), data_dir.path()).load().unwrap();
        assert_eq!(reloaded.overlay_position, 30);
        assert_eq!(reloaded.hotkey.as_deref(), Some("Ctrl+Alt+KeyV"));

        assert_eq!(store.save_overlay_position(0).unwrap().overlay_position, 0);
        assert_eq!(
            store.save_overlay_position(200).unwrap().overlay_position,
            100
        );
    }

    #[test]
    fn a_bad_overlay_position_loads_as_the_default_or_clamped() {
        for (value, expected) in [("\"x\"", 50), ("250", 100), ("-3", 0), ("12.5", 50)] {
            let config_dir = tempfile::tempdir().unwrap();
            let data_dir = tempfile::tempdir().unwrap();
            fs::write(
                config_dir.path().join(SETTINGS_FILE_NAME),
                format!("hotkey = \"Ctrl+Alt+KeyV\"\noverlay_position = {value}\n"),
            )
            .unwrap();

            let state = store_in(config_dir.path(), data_dir.path()).load().unwrap();

            assert_eq!(state.overlay_position, expected, "{value}");
            assert_eq!(state.hotkey.as_deref(), Some("Ctrl+Alt+KeyV"), "{value}");
        }
    }

    /// Story 3.14: the region, locale and voice round trip; a new locale
    /// clears the voice; a bad region is refused and nothing is saved.
    #[test]
    fn azure_region_locale_and_voice_round_trip() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let store = store_in(config_dir.path(), data_dir.path());
        let azure = LanguageBackend::Remote(RemoteProvider::Azure);
        let state = store.load().unwrap();
        assert_eq!(state.speech_languages.azure, "tr-TR");
        assert_eq!(state.speech_voices.azure, None);
        assert_eq!(state.azure_region, None);

        let state = store.save_azure_region(Some("  WestEurope ")).unwrap();
        assert_eq!(state.azure_region.as_deref(), Some("westeurope"));
        assert!(store.save_azure_region(Some("west europe!")).is_err());
        store
            .save_api_key(RemoteProvider::Azure, Some("az-key"))
            .unwrap();
        store
            .save_speech_voice(azure, Some("tr-TR-EmelNeural"))
            .unwrap();

        let reloaded = store_in(config_dir.path(), data_dir.path()).load().unwrap();
        assert_eq!(reloaded.azure_region.as_deref(), Some("westeurope"));
        assert_eq!(reloaded.api_keys.get(RemoteProvider::Azure), Some("az-key"));
        assert_eq!(
            reloaded.speech_voices.azure.as_deref(),
            Some("tr-TR-EmelNeural")
        );
        assert!(!format!("{reloaded:?}").contains("az-key"));
        let written = fs::read_to_string(config_dir.path().join(SETTINGS_FILE_NAME)).unwrap();
        assert!(
            written.contains("azure_region = \"westeurope\""),
            "{written}"
        );

        // A case-only change is the same locale, and keeps the voice.
        let state = store.save_speech_language(azure, "tr-tr").unwrap();
        assert_eq!(
            state.speech_voices.azure.as_deref(),
            Some("tr-TR-EmelNeural")
        );

        // The Change locale row: a new locale clears the voice.
        let state = store.save_speech_language(azure, "en-US").unwrap();
        assert_eq!(state.speech_languages.azure, "en-US");
        assert_eq!(state.speech_voices.azure, None);
        assert_eq!(state.speech_languages.system_voice, "tr", "untouched");

        // `None` removes the region.
        assert_eq!(store.save_azure_region(None).unwrap().azure_region, None);
    }

    /// Story 3.17: Edge TTS's locale and voice round trip under
    /// `edge_tts`, a new locale clears the voice, and it has no key.
    #[test]
    fn edge_tts_locale_and_voice_round_trip() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let store = store_in(config_dir.path(), data_dir.path());
        let edge = LanguageBackend::Remote(RemoteProvider::EdgeTts);
        let state = store.load().unwrap();
        assert_eq!(state.speech_languages.edge_tts, "tr-TR");
        assert_eq!(state.speech_voices.edge_tts, None);

        store
            .save_backend_selection(&BackendSelection::Remote(RemoteProvider::EdgeTts))
            .unwrap();
        store.save_speech_language(edge, "en-US").unwrap();
        store
            .save_speech_voice(edge, Some("en-US-AvaMultilingualNeural"))
            .unwrap();
        let state = store
            .save_api_key(RemoteProvider::EdgeTts, Some("ignored"))
            .unwrap();
        assert_eq!(state.api_keys.get(RemoteProvider::EdgeTts), None);

        let reloaded = store_in(config_dir.path(), data_dir.path()).load().unwrap();
        assert_eq!(
            reloaded.backend_selection,
            BackendSelection::Remote(RemoteProvider::EdgeTts)
        );
        assert_eq!(reloaded.speech_languages.edge_tts, "en-US");
        assert_eq!(
            reloaded.speech_voices.edge_tts.as_deref(),
            Some("en-US-AvaMultilingualNeural")
        );
        assert_eq!(reloaded.speech_languages.azure, "tr-TR", "untouched");
        let written = fs::read_to_string(config_dir.path().join(SETTINGS_FILE_NAME)).unwrap();
        assert!(written.contains("provider = \"edge_tts\""), "{written}");
        assert!(written.contains("edge_tts = \"en-US\""), "{written}");
        assert!(!written.contains("ignored"), "{written}");

        let state = store.save_speech_language(edge, "tr-TR").unwrap();
        assert_eq!(state.speech_voices.edge_tts, None);
    }

    #[test]
    fn a_hand_edited_bad_region_loads_as_no_region() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        fs::write(
            config_dir.path().join(SETTINGS_FILE_NAME),
            "hotkey = \"Ctrl+Alt+KeyV\"\nazure_region = \"evil.example/\"\nspeech_language = \"en\"\n",
        )
        .unwrap();

        let state = store_in(config_dir.path(), data_dir.path()).load().unwrap();

        assert_eq!(state.azure_region, None);
        assert_eq!(state.hotkey.as_deref(), Some("Ctrl+Alt+KeyV"));
        assert_eq!(
            state.speech_languages.azure, "tr-TR",
            "the legacy key never seeds Azure"
        );
    }

    /// Story 3.15: Piper's selection, language and voice round-trip; an
    /// explicit selection is never rewritten; a new language clears the
    /// voice and it stays cleared.
    #[test]
    fn piper_selection_language_and_voice_round_trip() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let store = store_in(config_dir.path(), data_dir.path());

        let fresh = store.load().unwrap();
        assert_eq!(fresh.speech_languages.piper, "tr_TR");
        assert_eq!(
            fresh.speech_voices.piper.as_deref(),
            Some("tr_TR-fahrettin-medium")
        );

        store
            .save_backend_selection(&BackendSelection::Piper)
            .unwrap();
        store
            .save_speech_voice(LanguageBackend::Piper, Some("tr_TR-dfki-medium"))
            .unwrap();
        let state = store_in(config_dir.path(), data_dir.path()).load().unwrap();
        assert_eq!(state.backend_selection, BackendSelection::Piper);
        assert_eq!(
            state.speech_voices.piper.as_deref(),
            Some("tr_TR-dfki-medium")
        );

        store
            .save_speech_language(LanguageBackend::Piper, "en_US")
            .unwrap();
        let state = store_in(config_dir.path(), data_dir.path()).load().unwrap();
        assert_eq!(state.speech_languages.piper, "en_US");
        assert_eq!(
            state.speech_voices.piper, None,
            "a new language clears the voice"
        );
        assert_eq!(
            state.speech_languages.system_voice, "tr",
            "every other backend's language is untouched"
        );

        // An explicit selection is never rewritten by the default.
        store
            .save_backend_selection(&BackendSelection::BUNDLED_CPU)
            .unwrap();
        assert_eq!(
            store.load().unwrap().backend_selection,
            BackendSelection::BUNDLED_CPU
        );
        let text = fs::read_to_string(config_dir.path().join(SETTINGS_FILE_NAME)).unwrap();
        assert!(text.contains("kind = \"local\""), "{text}");
    }
}
