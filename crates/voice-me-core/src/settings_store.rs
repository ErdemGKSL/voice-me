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
use crate::state::{AppState, SpeechBackend};

const SETTINGS_FILE_NAME: &str = "settings.toml";
const REFERENCE_VOICE_SAMPLE_FILE_NAME: &str = "reference_voice_sample.wav";

fn default_ui_language() -> String {
    crate::state::DEFAULT_UI_LANGUAGE.to_string()
}

/// The language generated speech is produced in, when the settings file
/// does not say (spec-2-6 Decision 2).
///
/// Turkish, not the UI-language default of English: this is the language the
/// user actually speaks into their voice chats, and FR5 wants a *selected*
/// language rather than a constant. There is no Settings control for it yet
/// — Epic 4 builds that alongside the UI-language selector — so until then
/// the file is the only way to change it, and it round-trips through every
/// other save.
fn default_speech_language() -> String {
    crate::state::DEFAULT_SPEECH_LANGUAGE.to_string()
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
    #[serde(default = "default_speech_language")]
    speech_language: String,
    #[serde(default)]
    selected_mic_device: Option<String>,
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
            speech_language: default_speech_language(),
            selected_mic_device: None,
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
        toml::from_str(&contents)
            .map_err(|err| VoiceMeError::Other(format!("failed to parse settings file: {err}")))
    }

    fn write_settings_file(&self, settings: &SettingsFile) -> Result<(), VoiceMeError> {
        fs::create_dir_all(&self.config_dir)?;
        let contents = toml::to_string_pretty(settings)
            .map_err(|err| VoiceMeError::Other(format!("failed to serialize settings: {err}")))?;
        fs::write(self.settings_path(), contents)?;
        Ok(())
    }

    fn build_state(&self, settings: SettingsFile) -> AppState {
        let sample_path = self.reference_voice_sample_path();
        AppState {
            hotkey: settings.hotkey,
            reference_voice_sample: existing_path(&sample_path),
            ui_language: settings.ui_language,
            speech_language: settings.speech_language,
            selected_mic_device: settings.selected_mic_device,
            // Not persisted: the composition root overwrites it with the
            // backend this build and this machine resolved to (AD-9).
            speech_backend: SpeechBackend::default(),
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
    fn the_speech_language_defaults_to_turkish_and_survives_an_unrelated_save() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let store = FileSettingsStore::with_dirs(
            config_dir.path().to_path_buf(),
            data_dir.path().to_path_buf(),
        );

        // No settings file at all yet (first run).
        assert_eq!(store.load().unwrap().speech_language, "tr");

        // Any save writes the whole file, so the default becomes explicit
        // and stays put — this is how the setting is "persisted" with no UI
        // control to set it (spec-2-6 Decision 2).
        store.save_hotkey(Some("Ctrl+Alt+KeyV")).unwrap();
        let written = fs::read_to_string(config_dir.path().join(SETTINGS_FILE_NAME)).unwrap();
        assert!(
            written.contains("speech_language = \"tr\""),
            "the setting has to reach the file, or it cannot be edited: {written}"
        );
    }

    #[test]
    fn a_speech_language_in_the_file_is_what_generation_gets() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        fs::write(
            config_dir.path().join(SETTINGS_FILE_NAME),
            "speech_language = \"en\"\n",
        )
        .unwrap();

        let store = FileSettingsStore::with_dirs(
            config_dir.path().to_path_buf(),
            data_dir.path().to_path_buf(),
        );

        assert_eq!(store.load().unwrap().speech_language, "en");
        // A file that omits the other fields still gets their defaults
        // rather than empty strings.
        assert_eq!(store.load().unwrap().ui_language, "en");
    }

    #[test]
    fn save_hotkey_leaves_the_other_settings_intact() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let store = FileSettingsStore::with_dirs(
            config_dir.path().to_path_buf(),
            data_dir.path().to_path_buf(),
        );

        // A speech language the user chose by hand — the only way to set it
        // until Epic 4 builds the selector, so an unrelated save silently
        // resetting it to the default would be invisible until they next
        // heard the wrong language.
        fs::write(
            config_dir.path().join(SETTINGS_FILE_NAME),
            "speech_language = \"en\"\n",
        )
        .unwrap();

        store.save_selected_mic_device(Some("USB Mic")).unwrap();
        let state = store.save_hotkey(Some("Ctrl+Alt+KeyV")).unwrap();

        assert_eq!(state.selected_mic_device, Some("USB Mic".to_string()));
        assert_eq!(state.hotkey, Some("Ctrl+Alt+KeyV".to_string()));
        assert_eq!(state.speech_language, "en");
    }
}
