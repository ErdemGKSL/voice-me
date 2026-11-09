//! The folder recordings are kept in: `<dir>/wavs/<id>.wav`, one file per
//! sentence, written the moment a take is accepted so a crash or a closed
//! window loses nothing and a later run picks up where this one stopped.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Where recordings live unless `--dir` says otherwise:
/// `Documents/voice-me-recordings`.
pub fn default_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let documents = home.join("Documents");
    if documents.is_dir() { documents } else { home }.join("voice-me-recordings")
}

#[derive(Debug, Clone)]
pub struct Session {
    dir: PathBuf,
}

impl Session {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, String> {
        let dir = dir.into();
        std::fs::create_dir_all(dir.join("wavs"))
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn wav_path(&self, id: &str) -> PathBuf {
        self.dir.join("wavs").join(format!("{id}.wav"))
    }

    /// The ids that have a recording.
    pub fn recorded(&self) -> HashSet<String> {
        std::fs::read_dir(self.dir.join("wavs"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                (path.extension()? == "wav").then(|| path.file_stem()?.to_str().map(String::from))?
            })
            .collect()
    }

    /// Save `wav` as `id`'s recording: written aside, then renamed over, so
    /// a half-written file never counts as a take.
    pub fn save(&self, id: &str, wav: &[u8]) -> Result<(), String> {
        let path = self.wav_path(id);
        let partial = path.with_extension("wav.part");
        std::fs::write(&partial, wav).map_err(|e| format!("could not save {id}: {e}"))?;
        std::fs::rename(&partial, &path).map_err(|e| format!("could not save {id}: {e}"))
    }

    pub fn load(&self, id: &str) -> Option<Vec<u8>> {
        std::fs::read(self.wav_path(id)).ok()
    }

    pub fn delete(&self, id: &str) {
        let _ = std::fs::remove_file(self.wav_path(id));
    }

    fn speaker_path(&self) -> PathBuf {
        self.dir.join("speaker.txt")
    }

    /// The speaker's name the dataset is exported under.
    pub fn speaker(&self) -> String {
        std::fs::read_to_string(self.speaker_path())
            .map(|s| s.trim().to_string())
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "speaker".to_string())
    }

    /// The microphone chosen last time, by name.
    pub fn mic(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.join("mic.txt"))
            .map(|s| s.trim().to_string())
            .ok()
            .filter(|s| !s.is_empty())
    }

    pub fn set_mic(&self, name: Option<&str>) -> Result<(), String> {
        let path = self.dir.join("mic.txt");
        match name {
            Some(name) => std::fs::write(path, name.trim()).map_err(|e| e.to_string()),
            None => std::fs::remove_file(path).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) }).map_err(|e| e.to_string()),
        }
    }

    pub fn has_speaker(&self) -> bool {
        self.speaker_path().is_file()
    }

    pub fn set_speaker(&self, name: &str) -> Result<(), String> {
        std::fs::write(self.speaker_path(), name.trim()).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_takes_are_found_again_and_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::open(dir.path()).unwrap();
        assert!(session.recorded().is_empty());
        session.save("00001", b"RIFFdata").unwrap();
        session.save("00007", b"RIFFdata").unwrap();
        std::fs::write(dir.path().join("wavs/00009.wav.part"), b"x").unwrap();
        assert_eq!(
            session.recorded(),
            ["00001", "00007"].map(String::from).into_iter().collect()
        );
        assert_eq!(session.load("00001").unwrap(), b"RIFFdata");
        session.delete("00001");
        assert!(session.load("00001").is_none());
    }

    #[test]
    fn the_speaker_name_defaults_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::open(dir.path()).unwrap();
        assert_eq!(session.speaker(), "speaker");
        session.set_speaker("  erdem ").unwrap();
        assert_eq!(session.speaker(), "erdem");
    }
}
