//! `voice-me-espeak` — the one place `espeak-ng` is run (Story 3.15).
//!
//! eSpeak NG is GPL-3.0, so it is never linked: the `espeak-ng` program runs
//! as a child process, AD-12's one exception to "no child process". It is
//! found by its fixed name on PATH and run with no shell; the text goes on
//! stdin, never in argv; every pipe is read on its own thread; and a run
//! still going after [`DEADLINE`] is killed and reaped.
//!
//! Two callers: the System voice (Story 3.12) speaks through it, and Piper
//! (Story 3.15) asks it for each clause's IPA phonemes. Both go through
//! [`run`], so neither can reach the program another way.
//!
//! Offline: this crate opens no socket (AD-8).

// It uses `std::os::unix`, which does not exist on Windows, so the whole
// body is gated rather than the workspace being made unbuildable there.
#![cfg(target_os = "linux")]

mod os_release;
mod process;

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use os_release::{GENERIC_INSTALL_STEP, install_command_for, install_step};
pub use process::{RunError, run};

/// The program, by the fixed name it is looked up by on PATH.
pub const PROGRAM: &str = "espeak-ng";

/// How long one run of the program may take.
pub const DEADLINE: Duration = Duration::from_secs(10);

/// Where `espeak-ng` is on PATH, if anywhere: the first executable file of
/// that name. What the Dependency Check reports; a run lets the OS resolve
/// the name the same way.
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

/// The arguments a phoneme run takes: quiet, the voice, IPA on stdout. The
/// text is not among them — it goes on stdin. Never `--ipa=3`: its U+200D
/// ties are not in any Piper voice's phoneme map.
pub fn ipa_args(voice: &str) -> Vec<OsString> {
    ["-q", "-v", voice, "--ipa"]
        .into_iter()
        .map(OsString::from)
        .collect()
}

/// The IPA phonemes `espeak-ng` gives for one `clause` in `voice`, exactly
/// as it printed them (lines and all), within [`DEADLINE`].
pub fn ipa(program: &OsStr, voice: &str, clause: &str) -> Result<String, RunError> {
    ipa_within(program, voice, clause, DEADLINE)
}

/// [`ipa`] with another deadline — how the tests make a hang short.
pub fn ipa_within(
    program: &OsStr,
    voice: &str,
    clause: &str,
    deadline: Duration,
) -> Result<String, RunError> {
    if clause.trim().is_empty() {
        return Ok(String::new());
    }
    let output = run(program, &ipa_args(voice), clause.as_bytes(), deadline)?;
    Ok(String::from_utf8_lossy(&output).into_owned())
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
    fn the_phoneme_arguments_are_exactly_quiet_the_voice_and_ipa() {
        assert_eq!(
            ipa_args("tr"),
            ["-q", "-v", "tr", "--ipa"].map(OsString::from).to_vec()
        );
    }

    /// Whatever the clause is, it reaches the program on stdin, byte for
    /// byte, and never in argv; the output comes back as printed.
    #[test]
    fn a_clause_goes_on_stdin_and_its_ipa_comes_back_as_printed() {
        let _serial = serial();
        let dir = tempfile::tempdir().unwrap();
        let program = fake_program(dir.path(), "printf 'mˈɛrhaba ɛrdˈæm\\n'");

        for clause in ["Merhaba Erdem", "--ipa=3 $(touch pwned) `id`"] {
            let ipa = ipa(program.as_os_str(), "tr", clause).unwrap();
            assert_eq!(ipa, "mˈɛrhaba ɛrdˈæm\n");
            assert_eq!(
                std::fs::read_to_string(dir.path().join("args")).unwrap(),
                "-q\n-v\ntr\n--ipa\n"
            );
            assert_eq!(
                std::fs::read_to_string(dir.path().join("stdin")).unwrap(),
                clause
            );
        }
        assert!(!dir.path().join("pwned").exists());
    }

    #[test]
    fn an_empty_clause_runs_nothing() {
        assert_eq!(
            ipa(OsStr::new("/nonexistent/voice-me/espeak-ng"), "tr", "  "),
            Ok(String::new())
        );
    }

    /// A non-zero exit carries the trimmed stderr.
    #[test]
    fn a_non_zero_exit_carries_its_stderr() {
        let _serial = serial();
        let dir = tempfile::tempdir().unwrap();
        let program = fake_program(dir.path(), "echo '  no such voice: xx  ' >&2\nexit 1");

        let error = ipa(program.as_os_str(), "xx", "merhaba").unwrap_err();

        assert!(
            matches!(&error, RunError::Failed { stderr, .. } if stderr == "no such voice: xx"),
            "{error:?}"
        );
        assert!(error.to_string().contains(": no such voice: xx"), "{error}");
    }

    /// The deadline: a program that never finishes is killed at it, and
    /// reaped.
    #[test]
    fn a_program_past_its_deadline_is_killed_and_reported() {
        let _serial = serial();
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let program = fake_program(
            dir.path(),
            &format!("echo $$ > '{}'\nexec sleep 30", pid_file.display()),
        );

        let started = std::time::Instant::now();
        let error =
            ipa_within(program.as_os_str(), "tr", "merhaba", Duration::from_secs(2)).unwrap_err();

        assert!(
            started.elapsed() < Duration::from_secs(8),
            "not killed in time"
        );
        assert_eq!(error, RunError::TimedOut(Duration::from_secs(2)));
        assert!(error.to_string().contains("did not finish"), "{error}");
        if let Ok(pid) = std::fs::read_to_string(&pid_file) {
            assert!(
                !Path::new("/proc").join(pid.trim()).exists(),
                "the child is still around"
            );
        }
    }

    #[test]
    fn a_missing_program_cannot_be_started() {
        let error = ipa(
            OsStr::new("/nonexistent/voice-me/espeak-ng"),
            "tr",
            "merhaba",
        )
        .unwrap_err();
        assert!(matches!(error, RunError::Spawn(_)), "{error:?}");
    }

    /// No input (a voice listing) is no stdin at all.
    #[test]
    fn a_run_with_no_input_gets_no_stdin() {
        let _serial = serial();
        let dir = tempfile::tempdir().unwrap();
        let program = fake_program(dir.path(), "echo listed");

        let output = run(
            program.as_os_str(),
            &[OsString::from("--voices")],
            &[],
            DEADLINE,
        )
        .unwrap();

        assert_eq!(output, b"listed\n");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("stdin")).unwrap(),
            ""
        );
    }

    /// Against the real program, when this machine has it.
    #[test]
    fn the_real_program_gives_turkish_ipa() {
        let Some(program) = find_program() else {
            eprintln!("skipped: espeak-ng is not on PATH");
            return;
        };
        let ipa = ipa(program.as_os_str(), "tr", "Merhaba Erdem").unwrap();
        assert!(ipa.contains('ˈ'), "{ipa:?}");
    }
}
