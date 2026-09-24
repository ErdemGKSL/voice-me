//! `voice-me-espeak` — the one place `espeak-ng` is run (Story 3.15), and
//! the one bounded runner every child process goes through: `espeak-ng`
//! and, since Story 3.17, `edge-tts`.
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
//! Story 3.17: `voice-me-tts-edge` runs the user-installed `edge-tts`
//! program through the same [`run`] (AD-12's one runner), and reuses the
//! os-release distro detection ([`distro_ids`], [`is_opensuse`],
//! [`read_os_release`]) for its pipx install steps. This crate itself still
//! opens no socket; the `edge-tts` process does the talking.
//!
//! Story 3.16: Piper runs on Windows too, so the crate builds there. On
//! Windows `espeak-ng.exe` is looked for in voice-me's cache (where the
//! Dependency Check's Install unpacks it), then under
//! `%ProgramFiles%\eSpeak NG\`, then on PATH; a program with an
//! `espeak-ng-data` directory beside it is always run with
//! `--path=<its directory>`, and every child is spawned with
//! `CREATE_NO_WINDOW`.
//!
//! Offline: this crate opens no socket (AD-8).

// Linux and Windows are the OSes with a Piper engine; elsewhere the whole
// body is gated rather than the workspace being made unbuildable there.
#![cfg(any(target_os = "linux", target_os = "windows"))]

#[cfg(target_os = "linux")]
mod os_release;
mod process;

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(target_os = "linux")]
pub use os_release::{
    GENERIC_INSTALL_STEP, distro_ids, install_command_for, install_step, is_opensuse,
    read_os_release,
};
#[cfg(target_os = "windows")]
pub use process::CREATE_NO_WINDOW;
pub use process::{RunError, run};

/// The program, by the fixed name it is looked up by on PATH.
pub const PROGRAM: &str = "espeak-ng";

/// How long one run of the program may take.
pub const DEADLINE: Duration = Duration::from_secs(10);

/// The program's file name on Windows, and the directory eSpeak NG's MSI
/// installs into (Story 3.16) — named once, in core.
pub use voice_me_core::assets::{
    ESPEAK_WINDOWS_INSTALL_DIR as WINDOWS_INSTALL_DIR, ESPEAK_WINDOWS_PROGRAM as WINDOWS_PROGRAM,
};

/// The data directory a self-contained eSpeak NG keeps beside its program.
pub const DATA_DIR: &str = "espeak-ng-data";

/// Where the program sits in an eSpeak NG unpacked into `espeak_dir`: `<espeak_dir>\eSpeak NG\espeak-ng.exe`.
pub fn unpacked_program(espeak_dir: &Path) -> PathBuf {
    voice_me_core::assets::espeak_program(espeak_dir)
}

/// The Windows lookup, over the directories it is given, so its order is
/// testable anywhere: the copy unpacked into `espeak_dir` (voice-me's
/// cache), then `<program_files>\eSpeak NG\espeak-ng.exe`, then
/// `espeak-ng.exe` in each of `path_dirs`. The first that is a file wins.
pub fn find_windows_program(
    espeak_dir: Option<&Path>,
    program_files: Option<&Path>,
    path_dirs: &[PathBuf],
) -> Option<PathBuf> {
    espeak_dir
        .map(unpacked_program)
        .into_iter()
        .chain(program_files.map(|dir| dir.join(WINDOWS_INSTALL_DIR).join(WINDOWS_PROGRAM)))
        .chain(path_dirs.iter().map(|dir| dir.join(WINDOWS_PROGRAM)))
        .find(|candidate| candidate.is_file())
}

/// Where `espeak-ng.exe` is, if anywhere: voice-me's cache, then
/// `%ProgramFiles%\eSpeak NG\`, then PATH (Story 3.16). What the
/// Dependency Check reports, and what Piper runs.
#[cfg(target_os = "windows")]
pub fn find_program() -> Option<PathBuf> {
    use voice_me_core::assets;
    let espeak_dir = assets::model_cache_root()
        .ok()
        .map(|root| assets::espeak_dir(&root));
    let program_files = std::env::var_os("ProgramFiles")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    find_windows_program(espeak_dir.as_deref(), program_files.as_deref(), &path_dirs)
}

/// Where `espeak-ng` is on PATH, if anywhere: the first executable file of
/// that name. What the Dependency Check reports; a run lets the OS resolve
/// the name the same way.
#[cfg(target_os = "linux")]
pub fn find_program() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(PROGRAM))
        .find(|candidate| is_executable(candidate))
}

#[cfg(target_os = "linux")]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// The arguments a phoneme run takes: quiet, the voice, IPA on stdout. The
/// text is not among them — it goes on stdin. Never `--ipa=3`: its U+200D
/// ties are not in any Piper voice's phoneme map.
///
/// Story 3.16: when `program`'s directory holds [`DATA_DIR`], the run
/// starts with `--path=<that directory>`, so an unpacked copy finds its
/// data without the registry. `/usr/bin` never holds it, so Linux runs are
/// unchanged.
pub fn ipa_args(program: &OsStr, voice: &str) -> Vec<OsString> {
    let mut args: Vec<OsString> = data_path_arg(program).into_iter().collect();
    args.extend(["-q", "-v", voice, "--ipa"].into_iter().map(OsString::from));
    args
}

/// `--path=<dir>` when `program` sits in `dir` next to [`DATA_DIR`].
fn data_path_arg(program: &OsStr) -> Option<OsString> {
    let dir = Path::new(program)
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())?;
    if !dir.join(DATA_DIR).is_dir() {
        return None;
    }
    let mut arg = OsString::from("--path=");
    arg.push(dir.as_os_str());
    Some(arg)
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
    let output = run(
        program,
        &ipa_args(program, voice),
        clause.as_bytes(),
        deadline,
    )?;
    Ok(String::from_utf8_lossy(&output).into_owned())
}

#[cfg(test)]
mod portable_tests {
    use super::*;

    #[test]
    fn the_phoneme_arguments_are_exactly_quiet_the_voice_and_ipa() {
        assert_eq!(
            ipa_args(OsStr::new(PROGRAM), "tr"),
            ["-q", "-v", "tr", "--ipa"].map(OsString::from).to_vec()
        );
    }

    #[test]
    fn a_program_with_no_data_directory_beside_it_gets_no_path() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join(WINDOWS_PROGRAM);
        std::fs::write(&program, b"").unwrap();

        assert_eq!(
            ipa_args(program.as_os_str(), "tr"),
            ["-q", "-v", "tr", "--ipa"].map(OsString::from).to_vec()
        );
    }

    #[test]
    fn a_program_next_to_its_data_directory_is_run_with_path() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join(WINDOWS_PROGRAM);
        std::fs::write(&program, b"").unwrap();
        std::fs::create_dir(dir.path().join(DATA_DIR)).unwrap();

        let mut path = OsString::from("--path=");
        path.push(dir.path().as_os_str());
        assert_eq!(
            ipa_args(program.as_os_str(), "tr"),
            vec![
                path,
                OsString::from("-q"),
                OsString::from("-v"),
                OsString::from("tr"),
                OsString::from("--ipa"),
            ]
        );
    }

    /// Lays out every Windows location in a temp dir and returns them:
    /// (voice-me's `espeak-ng` dir, Program Files, one PATH dir).
    fn windows_layout(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let cache = root.join("cache").join("espeak-ng");
        let program_files = root.join("Program Files");
        let on_path = root.join("bin");
        for program in [
            unpacked_program(&cache),
            program_files
                .join(WINDOWS_INSTALL_DIR)
                .join(WINDOWS_PROGRAM),
            on_path.join(WINDOWS_PROGRAM),
        ] {
            std::fs::create_dir_all(program.parent().unwrap()).unwrap();
            std::fs::write(&program, b"").unwrap();
        }
        (cache, program_files, on_path)
    }

    #[test]
    fn the_windows_lookup_prefers_the_cache_then_program_files_then_path() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, program_files, on_path) = windows_layout(dir.path());
        let path_dirs = [dir.path().join("empty"), on_path.clone()];

        assert_eq!(
            find_windows_program(Some(&cache), Some(&program_files), &path_dirs),
            Some(cache.join("eSpeak NG").join("espeak-ng.exe"))
        );

        std::fs::remove_dir_all(&cache).unwrap();
        assert_eq!(
            find_windows_program(Some(&cache), Some(&program_files), &path_dirs),
            Some(program_files.join("eSpeak NG").join("espeak-ng.exe"))
        );

        std::fs::remove_dir_all(&program_files).unwrap();
        assert_eq!(
            find_windows_program(Some(&cache), Some(&program_files), &path_dirs),
            Some(on_path.join("espeak-ng.exe"))
        );

        std::fs::remove_dir_all(&on_path).unwrap();
        assert_eq!(
            find_windows_program(Some(&cache), Some(&program_files), &path_dirs),
            None
        );
    }

    /// A directory by the program's name is not the program, and an unknown
    /// cache or Program Files is simply skipped.
    #[test]
    fn the_windows_lookup_wants_a_file_and_skips_unknown_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(WINDOWS_PROGRAM)).unwrap();

        assert_eq!(
            find_windows_program(None, None, &[dir.path().to_path_buf()]),
            None
        );
    }
}

// The fake programs are `/bin/sh` scripts.
#[cfg(all(test, unix))]
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

    /// Story 3.16: a program with `espeak-ng-data` beside it is run with
    /// `--path=<its directory>` first, then the usual arguments.
    #[test]
    fn a_program_next_to_its_data_is_run_with_path() {
        let _serial = serial();
        let dir = tempfile::tempdir().unwrap();
        let program = fake_program(dir.path(), "printf 'mˈɛrhaba\\n'");
        std::fs::create_dir(dir.path().join(DATA_DIR)).unwrap();

        ipa(program.as_os_str(), "tr", "Merhaba").unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.path().join("args")).unwrap(),
            format!("--path={}\n-q\n-v\ntr\n--ipa\n", dir.path().display())
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
        // Story 3.17: a program that is not there is `NotFound`, which
        // displays exactly as `Spawn` does.
        assert!(matches!(error, RunError::NotFound(_)), "{error:?}");
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
