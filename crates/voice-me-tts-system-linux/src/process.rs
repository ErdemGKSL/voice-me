//! One bounded run of a fixed program: no shell, input on stdin, every
//! pipe drained on its own thread, killed and reaped at the deadline.
//!
//! Program-agnostic (Story 3.17): the one runner both `espeak-ng` and
//! `edge-tts` go through (AD-12).

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How often a running child is asked whether it has exited.
const POLL: Duration = Duration::from_millis(5);

/// Why a run produced no usable output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// The program could not be started at all (not executable, say).
    Spawn(String),
    /// The program does not exist: spawning it failed with
    /// [`std::io::ErrorKind::NotFound`] (Story 3.17 — it can vanish after
    /// the Dependency Check).
    NotFound(String),
    /// It was still running at the deadline, and was killed.
    TimedOut(Duration),
    /// It exited unsuccessfully; carries the trimmed stderr.
    Failed { status: String, stderr: String },
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Spawn(reason) | RunError::NotFound(reason) => {
                write!(f, "could not be started: {reason}")
            }
            RunError::TimedOut(deadline) => {
                write!(f, "did not finish within {} seconds", deadline.as_secs())
            }
            RunError::Failed { status, stderr } if stderr.is_empty() => {
                write!(f, "exited with {status}")
            }
            RunError::Failed { status, stderr } => write!(f, "exited with {status}: {stderr}"),
        }
    }
}

/// Run `program` with `args`, writing `input` to its stdin, and return its
/// stdout — or why not, within `deadline`.
pub fn run(
    program: &OsStr,
    args: &[OsString],
    input: &[u8],
    deadline: Duration,
) -> Result<Vec<u8>, RunError> {
    // No input (the voice list) is no stdin at all, rather than an empty
    // pipe.
    let stdin = if input.is_empty() {
        Stdio::null()
    } else {
        Stdio::piped()
    };
    let mut child = Command::new(program)
        .args(args)
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => RunError::NotFound(error.to_string()),
            _ => RunError::Spawn(error.to_string()),
        })?;

    // Each pipe on its own thread: a child blocked writing a full stdout
    // pipe while we block writing its stdin would otherwise deadlock.
    let stdin = child.stdin.take().map(|mut stdin| {
        let input = input.to_vec();
        std::thread::spawn(move || {
            // A child that exits without reading is reported by its status.
            let _ = stdin.write_all(&input);
        })
    });
    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() >= deadline => break Err(RunError::TimedOut(deadline)),
            Ok(None) => std::thread::sleep(POLL),
            Err(error) => break Err(RunError::Spawn(error.to_string())),
        }
    };
    if status.is_err() {
        // Kill and reap, so no zombie is left behind; the pipes then close
        // and the reader threads finish.
        let _ = child.kill();
        let _ = child.wait();
    }

    let join = |handle: Option<JoinHandle<Vec<u8>>>| {
        handle
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default()
    };
    if let Some(handle) = stdin {
        let _ = handle.join();
    }
    let stdout = join(stdout);
    let stderr = join(stderr);

    let status = status?;
    if !status.success() {
        return Err(RunError::Failed {
            status: status.to_string(),
            stderr: String::from_utf8_lossy(&stderr).trim().to_string(),
        });
    }
    Ok(stdout)
}

fn drain(mut pipe: impl Read + Send + 'static) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        bytes
    })
}
