//! `voice-me-tts-system-linux` — the System voice on Linux (Story 3.12).
//!
//! A [`TtsPort`] over eSpeak NG. eSpeak NG is GPL-3.0, so it is never
//! linked: the `espeak-ng` program runs as a child process, AD-12's one
//! exception to "no child process". It is found by name on PATH and run
//! with no shell; the text goes on stdin, never in argv; every pipe is read
//! on its own thread; and a run still going after 10 seconds is killed and
//! reaped. Its WAV is decoded here and resampled to AD-11's 24 kHz.
//!
//! Offline: this crate opens no socket (AD-8).

// This crate *is* the Linux capability (AD-2): it uses `std::os::unix`,
// which does not exist on Windows, so the whole body is gated rather than
// the workspace being made unbuildable there. Windows is Story 3.13.
#![cfg(target_os = "linux")]

mod os_release;
mod process;
mod voices;
mod wav;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use voice_me_core::{AudioBuffer, SystemVoice, TtsPort, VoiceMeError};

pub use os_release::{GENERIC_INSTALL_STEP, install_command_for, install_step};
pub use voices::parse_voices;
pub use wav::decode_wav;

/// The program, by the fixed name it is looked up by on PATH.
pub const PROGRAM: &str = "espeak-ng";

/// The engine's name as the user reads it.
pub const ENGINE_LABEL: &str = "eSpeak NG";

/// How long one run of the program may take.
pub const DEADLINE: Duration = Duration::from_secs(10);

/// The arguments one utterance runs with: the voice, UTF-8 input (`-b 1`),
/// and the WAV on stdout. The text is not among them — it goes on stdin,
/// so no text can ever be read as an option.
pub fn speak_args(voice_id: &str) -> Vec<OsString> {
    ["-v", voice_id, "-b", "1", "--stdout"]
        .into_iter()
        .map(OsString::from)
        .collect()
}

/// Where `espeak-ng` is on PATH, if anywhere: the first executable file of
/// that name. What the Dependency Check reports; the adapter itself lets
/// the OS resolve the name the same way.
pub fn find_program() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(PROGRAM))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// The voices `espeak-ng --voices` lists, run the same bounded way.
pub fn list_voices() -> Result<Vec<SystemVoice>, VoiceMeError> {
    SystemVoiceLinux::new().voices()
}

/// The System voice's [`TtsPort`] adapter.
#[derive(Debug, Clone)]
pub struct SystemVoiceLinux {
    program: OsString,
    deadline: Duration,
}

impl Default for SystemVoiceLinux {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemVoiceLinux {
    /// The adapter over `espeak-ng` on PATH.
    pub fn new() -> Self {
        Self {
            program: OsString::from(PROGRAM),
            deadline: DEADLINE,
        }
    }

    /// The adapter over another program, with another deadline — how the
    /// tests stand a fake binary in for eSpeak NG.
    #[cfg(test)]
    fn with_program(program: impl Into<OsString>, deadline: Duration) -> Self {
        Self {
            program: program.into(),
            deadline,
        }
    }

    /// The voices the program lists.
    pub fn voices(&self) -> Result<Vec<SystemVoice>, VoiceMeError> {
        let output = process::run(
            &self.program,
            &[OsString::from("--voices")],
            &[],
            self.deadline,
        )
        .map_err(|error| failure(format!("could not list its voices: {PROGRAM} {error}")))?;
        let voices = parse_voices(&String::from_utf8_lossy(&output));
        if voices.is_empty() {
            return Err(failure(format!(
                "listed no voices: {PROGRAM} --voices printed nothing voice-me could read"
            )));
        }
        Ok(voices)
    }
}

/// A failure, naming the engine and the reason.
fn failure(reason: String) -> VoiceMeError {
    VoiceMeError::SpeechEngine(format!("{ENGINE_LABEL} {reason}"))
}

impl TtsPort for SystemVoiceLinux {
    /// Nothing to build: each utterance is one short run of the program.
    fn warm_up(&self) -> Result<(), VoiceMeError> {
        Ok(())
    }

    fn is_ready(&self) -> bool {
        true
    }

    /// Speak `text` in `voice`. A stock voice: `reference_clip` is never
    /// read, and `language` is already implied by the voice.
    fn generate(
        &self,
        text: &str,
        _reference_clip: Option<&Path>,
        _language: &str,
        voice: Option<&str>,
    ) -> Result<AudioBuffer, VoiceMeError> {
        if text.trim().is_empty() {
            return Err(VoiceMeError::EmptyText);
        }
        let Some(voice) = voice else {
            return Err(failure("was given no voice to speak in".to_string()));
        };
        let output = process::run(
            &self.program,
            &speak_args(voice),
            text.as_bytes(),
            self.deadline,
        )
        .map_err(|error| failure(format!("failed: {PROGRAM} {error}")))?;
        decode_wav(&output).map_err(|reason| failure(format!("failed: {reason}")))
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    /// Held by every test that writes and runs a fake program. Writing an
    /// executable while another test thread forks lets the fork inherit the
    /// write handle, and exec then fails with "Text file busy" (ETXTBSY).
    static FAKE_PROGRAMS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        FAKE_PROGRAMS
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// A fake `espeak-ng`: a shell script in a temp dir. It records its
    /// arguments one per line and its stdin, then runs `body`.
    fn fake_program(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fake-espeak-ng");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{args}'\ncat > '{stdin}'\n{body}\n",
            args = dir.join("args").display(),
            stdin = dir.join("stdin").display(),
        );
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn the_arguments_are_exactly_the_voice_and_the_output_flags() {
        assert_eq!(
            speak_args("trk/tr"),
            ["-v", "trk/tr", "-b", "1", "--stdout"]
                .map(OsString::from)
                .to_vec()
        );
    }

    /// The Hostile text row: whatever the text is, it reaches the program
    /// on stdin, byte for byte, and never in argv.
    #[test]
    fn hostile_text_goes_on_stdin_and_never_in_argv() {
        let _serial = serial();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("out.wav"),
            wav::streaming_wav(22_050, 2_205),
        )
        .unwrap();
        let program = fake_program(
            dir.path(),
            &format!("cat '{}'", dir.path().join("out.wav").display()),
        );
        let adapter = SystemVoiceLinux::with_program(&program, DEADLINE);

        for text in ["-v en --stdout", "hello; rm -rf ~ $(touch pwned) `id`"] {
            let audio = adapter.generate(text, None, "tr", Some("trk/tr")).unwrap();
            assert!(!audio.is_empty());

            let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
            assert_eq!(args, "-v\ntrk/tr\n-b\n1\n--stdout\n");
            assert_eq!(
                std::fs::read_to_string(dir.path().join("stdin")).unwrap(),
                text
            );
        }
        assert!(!dir.path().join("pwned").exists());
    }

    /// The Engine failure row: a non-zero exit names eSpeak NG and carries
    /// the trimmed stderr.
    #[test]
    fn a_non_zero_exit_names_the_engine_and_its_stderr() {
        let _serial = serial();
        let dir = tempfile::tempdir().unwrap();
        let program = fake_program(dir.path(), "echo '  no such voice: xx  ' >&2\nexit 1");
        let adapter = SystemVoiceLinux::with_program(&program, DEADLINE);

        let error = adapter
            .generate("merhaba", None, "tr", Some("xx"))
            .unwrap_err()
            .to_string();

        assert!(error.contains("eSpeak NG"), "{error}");
        assert!(error.contains(": no such voice: xx"), "{error}");
    }

    #[test]
    fn unreadable_output_names_the_engine() {
        let _serial = serial();
        let dir = tempfile::tempdir().unwrap();
        let program = fake_program(dir.path(), "echo 'this is not a wav'");
        let adapter = SystemVoiceLinux::with_program(&program, DEADLINE);

        let error = adapter
            .generate("merhaba", None, "tr", Some("trk/tr"))
            .unwrap_err()
            .to_string();

        assert!(
            error.contains("eSpeak NG") && error.contains("not a WAV"),
            "{error}"
        );
    }

    /// The deadline: a program that never finishes is killed at it, and
    /// the failure says so.
    #[test]
    fn a_program_past_its_deadline_is_killed_and_reported() {
        let _serial = serial();
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let program = fake_program(
            dir.path(),
            &format!("echo $$ > '{}'\nexec sleep 30", pid_file.display()),
        );
        let adapter = SystemVoiceLinux::with_program(&program, Duration::from_secs(2));

        let started = std::time::Instant::now();
        let error = adapter
            .generate("merhaba", None, "tr", Some("trk/tr"))
            .unwrap_err()
            .to_string();

        assert!(
            started.elapsed() < Duration::from_secs(8),
            "not killed in time"
        );
        assert!(
            error.contains("eSpeak NG") && error.contains("did not finish"),
            "{error}"
        );
        // Killed and reaped: the process is gone, not a zombie. (On a
        // loaded machine the script may be killed before it wrote its pid,
        // which is the same outcome.)
        if let Ok(pid) = std::fs::read_to_string(&pid_file) {
            assert!(
                !Path::new("/proc").join(pid.trim()).exists(),
                "the child is still around"
            );
        }
    }

    #[test]
    fn a_missing_program_is_a_failure_naming_the_engine() {
        let adapter = SystemVoiceLinux::with_program("/nonexistent/voice-me/espeak-ng", DEADLINE);
        let error = adapter
            .generate("merhaba", None, "tr", Some("trk/tr"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("eSpeak NG"), "{error}");
        assert!(adapter.voices().is_err());
    }

    #[test]
    fn no_voice_and_no_text_are_refused_before_running_anything() {
        let adapter = SystemVoiceLinux::with_program("/nonexistent/voice-me/espeak-ng", DEADLINE);
        assert!(matches!(
            adapter.generate("  ", None, "tr", Some("trk/tr")),
            Err(VoiceMeError::EmptyText)
        ));
        assert!(adapter.generate("merhaba", None, "tr", None).is_err());
    }

    #[test]
    fn the_voice_list_comes_from_the_program() {
        let _serial = serial();
        let dir = tempfile::tempdir().unwrap();
        let program = fake_program(
            dir.path(),
            "echo 'Pty Language Age/Gender VoiceName File Other Languages'\n\
             echo ' 5  tr  --/M  Turkish  trk/tr'",
        );
        let voices = SystemVoiceLinux::with_program(&program, DEADLINE)
            .voices()
            .unwrap();
        assert_eq!(voices.len(), 1);
        assert_eq!(voices[0].id, "trk/tr");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("args")).unwrap(),
            "--voices\n"
        );
    }
}
