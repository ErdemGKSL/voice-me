//! `voice-me-tts-edge` — Edge TTS on Linux (Story 3.17).
//!
//! A [`TtsPort`] over the user-installed `edge-tts` program, which speaks
//! in Microsoft's Edge Read Aloud neural voices with no key. It runs as a
//! child process through the one bounded runner (AD-12): no shell, the
//! typed text on stdin and never in argv, every pipe on its own thread, and
//! killed and reaped at the deadline. Its MP3 is decoded here to AD-11's
//! 24 kHz mono f32.
//!
//! This crate opens no socket and has no HTTP client (AD-8): the `edge-tts`
//! process is what talks to Microsoft.

// Linux only (E1): on other OSes Edge TTS is listed but gets a cannot-run
// row, and no engine is built.
#![cfg(target_os = "linux")]

mod install;
mod mp3;
mod voices;

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use voice_me_core::{AudioBuffer, StockVoice, TtsPort, VoiceMeError};
use voice_me_tts_system_linux::process::{self, RunError};

pub use install::{
    CHECK_AGAIN_STEP, PIP_FALLBACK_STEP, install_steps, install_steps_for, pipx_command_for,
};
pub use mp3::decode_mp3;
pub use voices::parse_voices;

/// The program, by the fixed name it is looked up by.
pub const PROGRAM: &str = "edge-tts";

/// The engine's name as the user reads it.
pub const ENGINE_LABEL: &str = "Edge TTS";

/// How long one utterance may take: it is network-bound, like Azure's.
pub const SPEECH_DEADLINE: Duration = Duration::from_secs(30);

/// How long listing the voices may take.
pub const VOICES_DEADLINE: Duration = Duration::from_secs(15);

/// What the user is told when the program is not there.
pub const NOT_INSTALLED: &str =
    "edge-tts is not installed. Please install it: pipx install edge-tts";

/// The longest stderr line a failure carries, in characters.
const STDERR_LIMIT: usize = 200;

/// The arguments one utterance runs with: exactly `--voice=<id> -f -
/// --write-media -`. The `=` form keeps a voice id from being read as a
/// flag, and the text is not among them — it goes on stdin.
pub fn speak_args(voice_id: &str) -> Vec<OsString> {
    let mut args = vec![OsString::from(format!("--voice={voice_id}"))];
    args.extend(["-f", "-", "--write-media", "-"].map(OsString::from));
    args
}

/// Where `edge-tts` is: the first executable of that name on `PATH`, then
/// `$HOME/.local/bin/edge-tts`, where `pipx` and `pip install --user` put it
/// and which a desktop-launched app often lacks on its `PATH`. The
/// Dependency Check's row and the adapter both use this.
pub fn find_program() -> Option<PathBuf> {
    find_program_in(
        std::env::var_os("PATH").as_deref(),
        std::env::var_os("HOME").as_deref().map(Path::new),
    )
}

/// [`find_program`] with the `PATH` and `HOME` given.
pub fn find_program_in(path: Option<&OsStr>, home: Option<&Path>) -> Option<PathBuf> {
    let on_path = path
        .into_iter()
        .flat_map(std::env::split_paths)
        // An empty or relative entry ("", ".", "bin") would resolve against
        // the app's working directory: only absolute directories count.
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(PROGRAM));
    let local_bin = home
        .filter(|home| !home.as_os_str().is_empty())
        .map(|home| home.join(".local").join("bin").join(PROGRAM));
    on_path
        .chain(local_bin)
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// The voices `edge-tts --list-voices` lists.
pub fn list_voices() -> Result<Vec<StockVoice>, VoiceMeError> {
    EdgeTts::new().voices()
}

/// Edge TTS's [`TtsPort`] adapter.
#[derive(Debug, Clone)]
pub struct EdgeTts {
    /// `None`: [`find_program`] at each run, so an install after the
    /// adapter was built is picked up. `Some`: a fixed program (tests).
    program: Option<OsString>,
    speech_deadline: Duration,
    voices_deadline: Duration,
}

impl Default for EdgeTts {
    fn default() -> Self {
        Self::new()
    }
}

impl EdgeTts {
    /// The adapter over the `edge-tts` [`find_program`] finds.
    pub fn new() -> Self {
        Self {
            program: None,
            speech_deadline: SPEECH_DEADLINE,
            voices_deadline: VOICES_DEADLINE,
        }
    }

    /// The adapter over another program, with another deadline — how the
    /// tests stand a fake binary in for `edge-tts`.
    #[cfg(test)]
    fn with_program(program: impl Into<OsString>, deadline: Duration) -> Self {
        Self {
            program: Some(program.into()),
            speech_deadline: deadline,
            voices_deadline: deadline,
        }
    }

    /// The program to run: the fixed one, or where `edge-tts` is now. With
    /// none found, the bare name, whose spawn then fails as not found.
    fn program(&self) -> OsString {
        self.program.clone().unwrap_or_else(|| {
            find_program().map_or_else(|| OsString::from(PROGRAM), PathBuf::into_os_string)
        })
    }

    /// The voices the program lists. Zero voices is an error.
    pub fn voices(&self) -> Result<Vec<StockVoice>, VoiceMeError> {
        let output = process::run(
            &self.program(),
            &[OsString::from("--list-voices")],
            &[],
            self.voices_deadline,
        )
        .map_err(run_failure)?;
        let voices = parse_voices(&String::from_utf8_lossy(&output));
        if voices.is_empty() {
            // An older edge-tts prints a list this parser cannot read.
            return Err(failure(
                "listed no voices — update it: pipx upgrade edge-tts".to_string(),
            ));
        }
        Ok(voices)
    }
}

/// A failure, naming the engine and the reason.
fn failure(reason: String) -> VoiceMeError {
    VoiceMeError::SpeechEngine(format!("{ENGINE_LABEL} {reason}"))
}

/// A failed run, as the one reason the user reads.
fn run_failure(error: RunError) -> VoiceMeError {
    match error {
        RunError::NotFound(_) => failure(format!("failed: {NOT_INSTALLED}")),
        RunError::TimedOut(deadline) => {
            failure(format!("took longer than {} s", deadline.as_secs()))
        }
        RunError::Failed { status, stderr } => {
            // A Python traceback: the reason is its last line.
            match stderr.lines().map(str::trim).rfind(|line| !line.is_empty()) {
                Some(line) => failure(format!(
                    "failed: {}",
                    line.chars().take(STDERR_LIMIT).collect::<String>()
                )),
                None => failure(format!("failed: {PROGRAM} exited with {status}")),
            }
        }
        RunError::Spawn(reason) => {
            failure(format!("failed: {PROGRAM} could not be started: {reason}"))
        }
    }
}

impl TtsPort for EdgeTts {
    /// Nothing to build: each utterance is one run of the program.
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
            &self.program(),
            &speak_args(voice),
            text.as_bytes(),
            self.speech_deadline,
        )
        .map_err(run_failure)?;
        decode_mp3(&output).map_err(|_| failure("returned no audio".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    /// Held by every test that writes or runs a program. Writing an
    /// executable while another test thread forks lets the fork inherit the
    /// write handle, and exec then fails with "Text file busy" (ETXTBSY).
    static PROGRAMS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        PROGRAMS.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    const TONE: &[u8] = include_bytes!("../tests/fixtures/tone.mp3");

    fn write_executable(path: &Path, script: &str) {
        std::fs::write(path, script).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A fake `edge-tts`: a shell script in a temp dir. It records its
    /// arguments one per line and its stdin, then runs `body`.
    fn fake_program(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fake-edge-tts");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{args}'\ncat > '{stdin}'\n{body}\n",
            args = dir.join("args").display(),
            stdin = dir.join("stdin").display(),
        );
        write_executable(&path, &script);
        path
    }

    /// A fake that writes the tone fixture to stdout.
    fn speaking_program(dir: &Path) -> PathBuf {
        std::fs::write(dir.join("tone.mp3"), TONE).unwrap();
        fake_program(dir, &format!("cat '{}'", dir.join("tone.mp3").display()))
    }

    #[test]
    fn the_arguments_are_exactly_the_voice_and_the_stdin_stdout_flags() {
        assert_eq!(
            speak_args("tr-TR-AhmetNeural"),
            ["--voice=tr-TR-AhmetNeural", "-f", "-", "--write-media", "-"]
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
        let adapter = EdgeTts::with_program(speaking_program(dir.path()), SPEECH_DEADLINE);

        for text in [
            "--voice=x; $(rm -rf ~)",
            "hello; rm -rf ~ $(touch pwned) `id`",
        ] {
            let audio = adapter
                .generate(text, None, "tr-TR", Some("tr-TR-AhmetNeural"))
                .unwrap();
            assert!(!audio.is_empty());

            let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
            assert_eq!(args, "--voice=tr-TR-AhmetNeural\n-f\n-\n--write-media\n-\n");
            assert_eq!(
                std::fs::read_to_string(dir.path().join("stdin")).unwrap(),
                text
            );
        }
        assert!(!dir.path().join("pwned").exists());
    }

    /// The Speak row at this level: the MP3 comes back as a 24 kHz buffer.
    #[test]
    fn the_programs_mp3_comes_back_as_the_24_khz_buffer() {
        let _serial = serial();
        let dir = tempfile::tempdir().unwrap();
        let adapter = EdgeTts::with_program(speaking_program(dir.path()), SPEECH_DEADLINE);

        let audio = adapter
            .generate("Merhaba", None, "tr-TR", Some("tr-TR-AhmetNeural"))
            .unwrap();

        assert!((11_000..=13_500).contains(&audio.len()), "{}", audio.len());
    }

    /// The Offline row: exit 1 with a traceback is the traceback's last
    /// line, capped.
    #[test]
    fn a_non_zero_exit_is_the_last_stderr_line() {
        let _serial = serial();
        let dir = tempfile::tempdir().unwrap();
        let program = fake_program(
            dir.path(),
            "echo 'Traceback (most recent call last):' >&2\n\
             echo '  File \"edge_tts/communicate.py\", line 1' >&2\n\
             echo 'aiohttp.client_exceptions.ClientConnectorError: Cannot connect to host speech.platform.bing.com:443' >&2\n\
             echo '' >&2\n\
             exit 1",
        );
        let adapter = EdgeTts::with_program(&program, SPEECH_DEADLINE);

        let error = adapter
            .generate("merhaba", None, "tr-TR", Some("tr-TR-AhmetNeural"))
            .unwrap_err();

        let VoiceMeError::SpeechEngine(message) = error else {
            panic!("{error:?}");
        };
        assert_eq!(
            message,
            "Edge TTS failed: aiohttp.client_exceptions.ClientConnectorError: Cannot connect to \
             host speech.platform.bing.com:443"
        );

        let long = fake_program(
            dir.path(),
            &format!("echo '{}' >&2\nexit 1", "x".repeat(500)),
        );
        let error = EdgeTts::with_program(&long, SPEECH_DEADLINE)
            .generate("merhaba", None, "tr-TR", Some("tr-TR-AhmetNeural"))
            .unwrap_err()
            .to_string();
        assert!(error.ends_with(&"x".repeat(200)), "{error}");
        assert!(!error.contains(&"x".repeat(201)), "{error}");
    }

    #[test]
    fn empty_or_undecodable_stdout_is_no_audio() {
        let _serial = serial();
        let dir = tempfile::tempdir().unwrap();
        for body in ["true", "echo 'this is not an mp3'"] {
            let program = fake_program(dir.path(), body);
            let error = EdgeTts::with_program(&program, SPEECH_DEADLINE)
                .generate("merhaba", None, "tr-TR", Some("tr-TR-AhmetNeural"))
                .unwrap_err();
            assert!(
                matches!(&error, VoiceMeError::SpeechEngine(message)
                    if message == "Edge TTS returned no audio"),
                "{error:?}"
            );
        }
    }

    /// The Timeout row: a program that never finishes is killed and reaped
    /// at the deadline, and the failure says so.
    #[test]
    fn a_program_past_its_deadline_is_killed_and_reported() {
        let _serial = serial();
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let program = fake_program(
            dir.path(),
            &format!("echo $$ > '{}'\nexec sleep 30", pid_file.display()),
        );
        let adapter = EdgeTts::with_program(&program, Duration::from_secs(2));

        let started = std::time::Instant::now();
        let error = adapter
            .generate("merhaba", None, "tr-TR", Some("tr-TR-AhmetNeural"))
            .unwrap_err()
            .to_string();

        assert!(
            started.elapsed() < Duration::from_secs(8),
            "not killed in time"
        );
        assert!(error.contains("Edge TTS took longer than 2 s"), "{error}");
        if let Ok(pid) = std::fs::read_to_string(&pid_file) {
            assert!(
                !Path::new("/proc").join(pid.trim()).exists(),
                "the child is still around"
            );
        }
    }

    /// The program removed after the check: the install message.
    #[test]
    fn a_missing_program_is_the_install_message() {
        let _serial = serial();
        let adapter = EdgeTts::with_program("/nonexistent/voice-me/edge-tts", SPEECH_DEADLINE);

        for error in [
            adapter
                .generate("merhaba", None, "tr-TR", Some("tr-TR-AhmetNeural"))
                .unwrap_err(),
            adapter.voices().unwrap_err(),
        ] {
            let VoiceMeError::SpeechEngine(message) = &error else {
                panic!("{error:?}");
            };
            assert_eq!(
                message,
                "Edge TTS failed: edge-tts is not installed. Please install it: pipx install \
                 edge-tts"
            );
        }
    }

    #[test]
    fn no_voice_and_no_text_are_refused_before_running_anything() {
        let adapter = EdgeTts::with_program("/nonexistent/voice-me/edge-tts", SPEECH_DEADLINE);
        assert!(matches!(
            adapter.generate("  ", None, "tr-TR", Some("tr-TR-AhmetNeural")),
            Err(VoiceMeError::EmptyText)
        ));
        let error = adapter
            .generate("merhaba", None, "tr-TR", None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no voice"), "{error}");
    }

    #[test]
    fn the_voice_list_comes_from_the_program_with_no_stdin() {
        let _serial = serial();
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path().join("list.txt");
        std::fs::write(
            &fixture,
            include_str!("../tests/fixtures/list-voices-7.2.8.txt"),
        )
        .unwrap();
        let program = fake_program(dir.path(), &format!("cat '{}'", fixture.display()));

        let voices = EdgeTts::with_program(&program, VOICES_DEADLINE)
            .voices()
            .unwrap();

        assert!(voices.iter().any(|voice| voice.id == "tr-TR-EmelNeural"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("args")).unwrap(),
            "--list-voices\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("stdin")).unwrap(),
            ""
        );
    }

    #[test]
    fn zero_voices_is_an_error() {
        let _serial = serial();
        let dir = tempfile::tempdir().unwrap();
        let program = fake_program(
            dir.path(),
            "echo 'Name  Gender  ContentCategories  VoicePersonalities'\necho '----  ------'",
        );

        let error = EdgeTts::with_program(&program, VOICES_DEADLINE)
            .voices()
            .unwrap_err()
            .to_string();

        assert!(
            error.contains("Edge TTS listed no voices — update it: pipx upgrade edge-tts"),
            "{error}"
        );
    }

    /// The pipx install row: with an empty `PATH`, the program is found in
    /// `$HOME/.local/bin` — and only when it is executable.
    #[test]
    fn the_program_is_found_in_home_local_bin_with_an_empty_path() {
        let _serial = serial();
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".local").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let program = bin.join(PROGRAM);

        assert_eq!(
            find_program_in(Some(OsStr::new("")), Some(home.path())),
            None
        );

        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        assert_eq!(
            find_program_in(Some(OsStr::new("")), Some(home.path())),
            None,
            "not executable"
        );

        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            find_program_in(Some(OsStr::new("")), Some(home.path())),
            Some(program.clone())
        );
        assert_eq!(find_program_in(None, Some(home.path())), Some(program));
        assert_eq!(find_program_in(Some(OsStr::new("")), None), None);

        // Relative entries never resolve against the working directory.
        let cwd = std::env::current_dir().unwrap();
        let relative = tempfile::tempdir_in(&cwd).unwrap();
        write_executable(&relative.path().join(PROGRAM), "#!/bin/sh\n");
        let name = relative.path().file_name().unwrap().to_os_string();
        let mut dot = OsString::from("./");
        dot.push(&name);
        for entry in [name.as_os_str(), dot.as_os_str()] {
            assert_eq!(find_program_in(Some(entry), None), None, "{entry:?}");
        }
    }

    #[test]
    fn path_comes_before_home_local_bin() {
        let _serial = serial();
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".local").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        write_executable(&bin.join(PROGRAM), "#!/bin/sh\n");
        let on_path = tempfile::tempdir().unwrap();
        write_executable(&on_path.path().join(PROGRAM), "#!/bin/sh\n");

        assert_eq!(
            find_program_in(Some(on_path.path().as_os_str()), Some(home.path())),
            Some(on_path.path().join(PROGRAM))
        );
    }
}
