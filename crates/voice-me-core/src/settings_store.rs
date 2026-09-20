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
use crate::state::AppState;

const SETTINGS_FILE_NAME: &str = "settings.toml";
const REFERENCE_VOICE_SAMPLE_FILE_NAME: &str = "reference_voice_sample.wav";

fn default_ui_language() -> String {
    "en".to_string()
}

/// The subset of `AppState` that is actually serialized to TOML. The active
/// Reference Voice Sample is intentionally excluded — it's derived from file
/// presence in `data_dir`, per AD-6.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SettingsFile {
    #[serde(default)]
    hotkey: Option<String>,
    #[serde(default = "default_ui_language")]
    ui_language: String,
    #[serde(default)]
    selected_mic_device: Option<String>,
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
            selected_mic_device: settings.selected_mic_device,
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
}
