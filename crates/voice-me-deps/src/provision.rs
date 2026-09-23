//! The downloading half of `voice-me-deps` (Story 3.2).
//!
//! One file at a time: a ranged GET appending to `<name>.part`, a SHA-256
//! of the finished `.part` against the pinned value, and only then an
//! atomic rename to the final name. A half-written or corrupt file
//! therefore never carries the name the Dependency Check and the engine
//! look for, which is what makes "present" and "verified" the same test.
//!
//! An interrupted download keeps its `.part`; the next Install sends
//! `Range: bytes=<len>-` and appends to it. A server that ignores the range
//! and answers `200` restarts that one file from zero. Every failure is a
//! sentence naming the file and the reason — and, where bytes were kept,
//! how many, since that is what tells the user a retry is cheap.

use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use reqwest::header::{CONTENT_RANGE, RANGE};
use sha2::{Digest as _, Sha256};
use voice_me_core::{AppEvent, AppEventSender, DependencyKind, VoiceMeError, format_bytes};

use crate::sources::{Asset, PlannedDownload};

/// The fastest a row's progress is reported: about ten times a second.
/// Faster only floods the one channel every other event shares.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// A connection that produces nothing for this long is dead, not slow.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Sends `ProvisioningProgress` for one row, throttled.
pub struct ProgressReporter {
    kind: DependencyKind,
    events: AppEventSender,
    done: u64,
    total: u64,
    last_sent: Option<Instant>,
}

impl ProgressReporter {
    /// Start reporting on `kind` for `plan`: the total is what the plan
    /// fetches, and whatever its `.part` files already hold counts as done
    /// from the very first event — a resumed download starts at 300 MB, not
    /// at zero.
    pub fn for_plan(
        kind: DependencyKind,
        events: AppEventSender,
        plan: &[PlannedDownload],
    ) -> Self {
        let total = plan.iter().map(|planned| planned.asset.size).sum();
        let done = plan
            .iter()
            .map(|planned| part_len(&planned.destination).min(planned.asset.size))
            .sum();
        let mut reporter = Self {
            kind,
            events,
            done,
            total,
            last_sent: None,
        };
        reporter.send();
        reporter
    }

    fn advance(&mut self, bytes: u64) {
        self.done = self.done.saturating_add(bytes);
        let due = self
            .last_sent
            .is_none_or(|sent| sent.elapsed() >= PROGRESS_INTERVAL);
        if due {
            self.send();
        }
    }

    /// Take back bytes counted as done that are no longer on disk — a
    /// server that ignored the range, a `.part` longer than the file.
    fn rewind(&mut self, bytes: u64) {
        self.done = self.done.saturating_sub(bytes);
        self.send();
    }

    /// The last figure, unthrottled, so the row does not end on a stale
    /// one.
    pub fn finish(&mut self) {
        self.send();
    }

    fn send(&mut self) {
        self.last_sent = Some(Instant::now());
        // A closed receiver means the app is quitting; the download carries
        // on to its next safe point regardless.
        let _ = self.events.unbounded_send(AppEvent::ProvisioningProgress {
            kind: self.kind,
            done_bytes: self.done,
            total_bytes: self.total,
        });
    }
}

/// The one HTTP client shape this crate uses: plain HTTPS GETs, following
/// the redirects both origins use to reach their CDNs.
pub fn client() -> Result<reqwest::Client, VoiceMeError> {
    reqwest::Client::builder()
        .user_agent(concat!("voice-me/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .build()
        .map_err(|error| {
            VoiceMeError::Other(format!("could not start a download: {}", reason(&error)))
        })
}

/// `<destination>.part`: where bytes land until they are verified.
pub fn part_path(destination: &Path) -> PathBuf {
    let mut name = destination
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".part");
    destination.with_file_name(name)
}

fn part_len(destination: &Path) -> u64 {
    std::fs::metadata(part_path(destination))
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

/// Fetch one planned file, resuming its `.part` if there is one, verify it
/// and give it its final name.
pub async fn download(
    client: &reqwest::Client,
    planned: &PlannedDownload,
    progress: &mut ProgressReporter,
) -> Result<(), VoiceMeError> {
    let asset = &planned.asset;
    let destination = &planned.destination;
    let name = asset.file_name();
    let dir = destination
        .parent()
        .ok_or_else(|| VoiceMeError::Other(format!("{name} has no directory to go into")))?;
    std::fs::create_dir_all(dir).map_err(|error| {
        VoiceMeError::Other(format!("Could not create {}: {error}", dir.display()))
    })?;

    let part = part_path(destination);
    let mut kept = part_len(destination);
    if kept > asset.size {
        // Longer than the file can be: not a prefix of it. The reporter
        // counted it as `size`, so that is what comes back off.
        remove_part(&part, dir)?;
        progress.rewind(asset.size);
        kept = 0;
    }

    if kept < asset.size {
        kept = fetch_into_part(client, asset, &part, dir, kept, progress).await?;
    }

    if kept < asset.size {
        return Err(VoiceMeError::Other(format!(
            "Download of {name} failed: the connection closed before the file was complete ({})",
            kept_sentence(kept, asset.size)
        )));
    }

    verify(asset, &part, dir)?;
    std::fs::rename(&part, destination).map_err(|error| {
        VoiceMeError::Other(format!(
            "Could not move {name} into place in {}: {error}",
            dir.display()
        ))
    })
}

/// Stream the body into `part`, returning how many bytes it now holds.
async fn fetch_into_part(
    client: &reqwest::Client,
    asset: &Asset,
    part: &Path,
    dir: &Path,
    mut kept: u64,
    progress: &mut ProgressReporter,
) -> Result<u64, VoiceMeError> {
    let name = asset.file_name();
    let mut request = client.get(&asset.url);
    if kept > 0 {
        request = request.header(RANGE, format!("bytes={kept}-"));
    }
    let mut response = request
        .send()
        .await
        .map_err(|error| network_error(name, &error, kept, asset.size))?;

    let status = response.status();
    let mut file = if status == StatusCode::PARTIAL_CONTENT && kept > 0 {
        // A range answered from anywhere but where the `.part` ends would
        // append the wrong bytes; better to start that file over.
        if content_range_start(&response) != Some(kept) {
            remove_part(part, dir)?;
            progress.rewind(kept);
            return Err(VoiceMeError::Other(format!(
                "Download of {name} failed: the server resumed from the wrong place; Install will \
                 fetch it again from the start"
            )));
        }
        OpenOptions::new()
            .append(true)
            .open(part)
            .map_err(|error| write_error(name, dir, &error, kept, asset.size))?
    } else if status == StatusCode::RANGE_NOT_SATISFIABLE {
        // The `.part` does not describe a prefix the server recognises.
        // Keeping it would fail the same way on every retry.
        remove_part(part, dir)?;
        progress.rewind(kept);
        return Err(VoiceMeError::Other(format!(
            "Download of {name} failed: the server could not resume it; Install will fetch it \
             again from the start"
        )));
    } else if status.is_success() {
        // The whole file, from the first byte: the server ignored the
        // range (or none was sent). Whatever the `.part` held goes.
        progress.rewind(kept);
        kept = 0;
        File::create(part).map_err(|error| write_error(name, dir, &error, kept, asset.size))?
    } else {
        return Err(VoiceMeError::Other(format!(
            "Download of {name} failed: the server answered {status}{}",
            if kept > 0 {
                format!(" ({})", kept_sentence(kept, asset.size))
            } else {
                String::new()
            }
        )));
    };

    loop {
        match response.chunk().await {
            Ok(Some(bytes)) => {
                // More than the pinned size can only be the wrong file;
                // writing on would be unbounded and push the figure past
                // its total.
                if kept + bytes.len() as u64 > asset.size {
                    drop(file);
                    remove_part(part, dir)?;
                    progress.rewind(kept);
                    return Err(VoiceMeError::Other(format!(
                        "Download of {name} failed: the server sent more than the expected {}; \
                         Install will fetch it again",
                        format_bytes(asset.size)
                    )));
                }
                file.write_all(&bytes)
                    .map_err(|error| write_error(name, dir, &error, kept, asset.size))?;
                kept += bytes.len() as u64;
                progress.advance(bytes.len() as u64);
            }
            Ok(None) => break,
            Err(error) => {
                // Whatever reached the file stays there for the next run.
                let _ = file.flush();
                return Err(network_error(name, &error, kept, asset.size));
            }
        }
    }
    file.sync_data()
        .map_err(|error| write_error(name, dir, &error, kept, asset.size))?;
    Ok(kept)
}

/// The start offset of a `Content-Range: bytes <start>-<end>/<total>`.
fn content_range_start(response: &reqwest::Response) -> Option<u64> {
    let value = response.headers().get(CONTENT_RANGE)?.to_str().ok()?;
    let range = value.trim().strip_prefix("bytes")?.trim_start();
    range.split('-').next()?.trim().parse().ok()
}

/// Hash the finished `.part` and compare. A mismatch deletes it: those
/// bytes are wrong, and resuming onto them would only be wrong again.
fn verify(asset: &Asset, part: &Path, dir: &Path) -> Result<(), VoiceMeError> {
    let name = asset.file_name();
    let actual = sha256_file(part).map_err(|error| {
        VoiceMeError::Other(format!(
            "Could not read {name} back from {} to check it: {error}",
            dir.display()
        ))
    })?;
    if actual.eq_ignore_ascii_case(&asset.sha256) {
        return Ok(());
    }
    remove_part(part, dir)?;
    Err(VoiceMeError::Other(format!(
        "{name} did not match its expected checksum; Install will fetch it again"
    )))
}

/// Lowercase hex SHA-256 of a file, read in 1 MiB pieces.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn remove_part(part: &Path, dir: &Path) -> Result<(), VoiceMeError> {
    match std::fs::remove_file(part) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(VoiceMeError::Other(format!(
            "Could not remove a bad partial download from {}: {error}",
            dir.display()
        ))),
    }
}

/// Pull the one real runtime library out of the verified archive and give
/// it its final name — `.part` then rename, like every download.
///
/// Only a regular-file entry counts: the archive's `libonnxruntime.so` is a
/// symlink chain, and extracting a symlink into the cache would leave the
/// engine loading whatever it points at.
pub fn extract_runtime_library(
    archive: &Path,
    entry_name: &str,
    destination: &Path,
) -> Result<(), VoiceMeError> {
    let library = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let archive_name = archive
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let failed = |reason: String| {
        VoiceMeError::Other(format!(
            "Extraction of {library} from {archive_name} failed: {reason}"
        ))
    };

    let dir = destination
        .parent()
        .ok_or_else(|| failed("no directory to extract into".to_string()))?;
    std::fs::create_dir_all(dir)
        .map_err(|error| failed(format!("could not create {}: {error}", dir.display())))?;

    let file = File::open(archive).map_err(|error| failed(error.to_string()))?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let entries = tar.entries().map_err(|error| failed(error.to_string()))?;
    for entry in entries {
        let mut entry = entry.map_err(|error| failed(error.to_string()))?;
        let is_library = entry
            .path()
            .map(|path| path == Path::new(entry_name))
            .unwrap_or(false);
        if !is_library || !entry.header().entry_type().is_file() {
            continue;
        }

        let part = part_path(destination);
        let mut out = File::create(&part)
            .map_err(|error| failed(format!("could not write to {}: {error}", dir.display())))?;
        std::io::copy(&mut entry, &mut out)
            .and_then(|_| out.sync_all())
            .map_err(|error| {
                let _ = std::fs::remove_file(&part);
                failed(format!("could not write to {}: {error}", dir.display()))
            })?;
        drop(out);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o755));
        }
        return std::fs::rename(&part, destination).map_err(|error| {
            failed(format!(
                "could not move it into place in {}: {error}",
                dir.display()
            ))
        });
    }

    Err(failed(format!("the archive has no {entry_name}")))
}

fn kept_sentence(kept: u64, size: u64) -> String {
    format!(
        "{} of {} kept, Install resumes from there",
        format_bytes(kept),
        format_bytes(size)
    )
}

fn network_error(name: &str, error: &reqwest::Error, kept: u64, size: u64) -> VoiceMeError {
    let inner = reason(error);
    let why = if error.is_timeout() {
        "the connection timed out".to_string()
    } else if inner.contains("end of file before message length reached") {
        // hyper's words for a body cut short; the user's words are these.
        "the connection closed before the file was complete".to_string()
    } else if error.is_connect() {
        format!("could not connect ({inner})")
    } else {
        inner
    };
    if kept > 0 {
        VoiceMeError::Other(format!(
            "Download of {name} failed: {why} ({})",
            kept_sentence(kept, size)
        ))
    } else {
        VoiceMeError::Other(format!("Download of {name} failed: {why}"))
    }
}

fn write_error(
    name: &str,
    dir: &Path,
    error: &std::io::Error,
    kept: u64,
    size: u64,
) -> VoiceMeError {
    if kept == 0 {
        return VoiceMeError::Other(format!(
            "Could not write {name} to {}: {error}",
            dir.display()
        ));
    }
    VoiceMeError::Other(format!(
        "Could not write {name} to {}: {error} ({})",
        dir.display(),
        kept_sentence(kept, size)
    ))
}

/// The innermost cause, which is where the words a person can act on live
/// ("Connection reset by peer"), rather than reqwest's outer "error sending
/// request for url (…)" wrapper.
fn reason(error: &(dyn std::error::Error + 'static)) -> String {
    let mut innermost = error;
    while let Some(source) = innermost.source() {
        innermost = source;
    }
    innermost.to_string()
}
