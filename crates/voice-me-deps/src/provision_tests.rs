//! Story 3.2's I/O & Edge-Case Matrix, driven against an in-process HTTP
//! server with small fake assets and an injected source table. Nothing
//! here reaches the network: every URL points at `127.0.0.1`.

use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sha2::{Digest as _, Sha256};
use voice_me_core::{
    AppEvent, DependencyKind, DependencyProvisioningPort, DependencyStatus, SpeechBackend,
    SpeechWeights, assets,
};

use crate::DepsAdapter;
use crate::provision::part_path;
use crate::sources::{Asset, RuntimeArchive, Sources};
use crate::test_support::EnvGuard;

/// How the server answers one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    /// `Range` honoured with a `206`.
    Honour,
    /// `Range` ignored: always the whole file with a `200`.
    IgnoreRange,
    /// Promise the whole body, send this many bytes of it, hang up. Once:
    /// the next request for the same path is honoured normally.
    DropAfter(usize),
    /// Answer a range with a `206` whose `Content-Range` starts one byte
    /// later than asked.
    WrongContentRange,
    /// Answer a range with `416 Range Not Satisfiable`.
    RangeNotSatisfiable,
}

#[derive(Default)]
struct ServerState {
    files: HashMap<String, Vec<u8>>,
    behaviour: HashMap<String, Behaviour>,
    /// Every request as `(path, Range header)`, in arrival order.
    requests: Vec<(String, Option<String>)>,
}

struct TestServer {
    base: String,
    state: Arc<Mutex<ServerState>>,
}

impl TestServer {
    fn start(files: HashMap<String, Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(ServerState {
            files,
            ..ServerState::default()
        }));
        let shared = state.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let shared = shared.clone();
                std::thread::spawn(move || serve(stream, &shared));
            }
        });
        Self { base, state }
    }

    fn set(&self, path: &str, behaviour: Behaviour) {
        self.state
            .lock()
            .unwrap()
            .behaviour
            .insert(path.to_string(), behaviour);
    }

    fn requests(&self) -> Vec<(String, Option<String>)> {
        self.state.lock().unwrap().requests.clone()
    }
}

fn serve(mut stream: TcpStream, state: &Mutex<ServerState>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .trim_start_matches('/')
        .to_string();
    let mut range = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("range")
        {
            range = Some(value.trim().to_string());
        }
    }

    let (body, behaviour) = {
        let mut state = state.lock().unwrap();
        state.requests.push((path.clone(), range.clone()));
        let behaviour = state
            .behaviour
            .get(&path)
            .copied()
            .unwrap_or(Behaviour::Honour);
        // A drop happens once; the retry is served normally.
        if matches!(behaviour, Behaviour::DropAfter(_)) {
            state.behaviour.insert(path.clone(), Behaviour::Honour);
        }
        (state.files.get(&path).cloned(), behaviour)
    };

    let Some(body) = body else {
        let _ = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    };

    let start = range
        .as_deref()
        .and_then(|value| value.strip_prefix("bytes="))
        .and_then(|value| value.strip_suffix('-'))
        .and_then(|value| value.parse::<usize>().ok());

    match (behaviour, start) {
        (Behaviour::WrongContentRange, Some(start)) if start + 1 < body.len() => {
            let rest = &body[start + 1..];
            let head = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                rest.len(),
                start + 1,
                body.len() - 1,
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(rest);
        }
        (Behaviour::RangeNotSatisfiable, Some(_)) => {
            let _ = stream.write_all(
                b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
        (Behaviour::Honour, Some(start)) if start < body.len() => {
            let rest = &body[start..];
            let head = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                rest.len(),
                start,
                body.len() - 1,
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(rest);
        }
        (Behaviour::DropAfter(sent), _) => {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body[..sent.min(body.len())]);
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        _ => {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
        }
    }
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Fake contents for every model file of every variant, distinct per file
/// and a few KB each so the ranged paths are exercised for real.
fn fake_model_files() -> Vec<(String, Vec<u8>)> {
    let root = Path::new("/r");
    let mut relative: Vec<String> = Vec::new();
    for weights in [SpeechWeights::Q4, SpeechWeights::Fp16] {
        for path in assets::required_model_files(root, weights) {
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if !relative.contains(&rel) {
                relative.push(rel);
            }
        }
    }
    relative
        .into_iter()
        .enumerate()
        .map(|(index, rel)| {
            let bytes = (0..4096 + index * 97)
                .map(|n| ((n * 31 + index * 7) % 251) as u8)
                .collect();
            (rel, bytes)
        })
        .collect()
}

struct Fixture {
    server: TestServer,
    files: HashMap<String, Vec<u8>>,
    sources: Sources,
    cache: tempfile::TempDir,
    env: EnvGuard,
}

impl Fixture {
    fn new() -> Self {
        Self::with_extra_files(Vec::new(), None)
    }

    fn with_extra_files(extra: Vec<(String, Vec<u8>)>, runtime_entry: Option<String>) -> Self {
        let cache = tempfile::tempdir().unwrap();
        let env = EnvGuard::new()
            .unset(assets::RUNTIME_DYLIB_ENV)
            .unset("http_proxy")
            .unset("HTTP_PROXY")
            .unset("all_proxy")
            .unset("ALL_PROXY")
            .set(assets::CACHE_ROOT_ENV, cache.path());

        let models = fake_model_files();
        let mut files: HashMap<String, Vec<u8>> = models.iter().cloned().collect();
        files.extend(extra.iter().cloned());
        let server = TestServer::start(files.clone());

        let asset = |rel: &str, bytes: &[u8]| Asset {
            relative_path: rel.to_string(),
            url: format!("{}/{rel}", server.base),
            size: bytes.len() as u64,
            sha256: sha256(bytes),
        };
        let model_files = models
            .iter()
            .map(|(rel, bytes)| asset(rel, bytes))
            .collect();
        let runtime = runtime_entry.map(|library_entry| {
            let (rel, bytes) = extra.first().expect("a runtime archive to serve");
            RuntimeArchive {
                archive: asset(rel, bytes),
                library_entry,
            }
        });

        Self {
            server,
            files,
            sources: Sources {
                model_files,
                runtime,
            },
            cache,
            env,
        }
    }

    fn root(&self) -> &Path {
        self.cache.path()
    }

    fn adapter(&self) -> DepsAdapter {
        DepsAdapter::with_sources(self.sources.clone())
    }

    fn provision(&self, kind: DependencyKind) -> (Result<(), String>, Vec<AppEvent>) {
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let result = self
            .adapter()
            .provision(kind, SpeechBackend::CPU, tx)
            .map_err(|error| error.to_string());
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        (result, events)
    }

    fn q4_files(&self) -> Vec<PathBuf> {
        assets::required_model_files(self.root(), SpeechWeights::Q4)
    }

    fn served(&self, path: &Path) -> &[u8] {
        let rel = path
            .strip_prefix(self.root())
            .unwrap()
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        &self.files[&rel]
    }

    fn model_row_status(&self) -> DependencyStatus {
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        self.adapter()
            .check(voice_me_core::CheckRequest::cpu(), tx)
            .unwrap();
        let Ok(AppEvent::DependencyCheckCompleted { report }) = rx.try_recv() else {
            panic!("the check reports by event");
        };
        report
            .dependencies
            .iter()
            .find(|row| row.kind == DependencyKind::ModelWeights)
            .unwrap()
            .status
    }
}

fn progress(events: &[AppEvent]) -> Vec<(u64, u64)> {
    events
        .iter()
        .filter_map(|event| match event {
            AppEvent::ProvisioningProgress {
                done_bytes,
                total_bytes,
                ..
            } => Some((*done_bytes, *total_bytes)),
            _ => None,
        })
        .collect()
}

fn finished(events: &[AppEvent]) -> Vec<Result<(), String>> {
    events
        .iter()
        .filter_map(|event| match event {
            AppEvent::ProvisioningFinished { result, .. } => Some(result.clone()),
            _ => None,
        })
        .collect()
}

fn size_of(bytes: &[u8]) -> u64 {
    bytes.len() as u64
}

#[test]
fn a_fresh_cpu_install_fetches_the_nine_q4_files_and_the_row_turns_ready() {
    let fixture = Fixture::new();
    assert_eq!(fixture.model_row_status(), DependencyStatus::Missing);

    let (result, events) = fixture.provision(DependencyKind::ModelWeights);

    assert_eq!(result, Ok(()));
    for path in fixture.q4_files() {
        assert_eq!(
            std::fs::read(&path).unwrap(),
            fixture.served(&path),
            "{}",
            path.display()
        );
        assert!(!part_path(&path).exists(), "no .part left behind");
    }
    assert_eq!(fixture.server.requests().len(), 9);

    let expected_total: u64 = fixture
        .q4_files()
        .iter()
        .map(|p| size_of(fixture.served(p)))
        .sum();
    let figures = progress(&events);
    assert_eq!(figures.first(), Some(&(0, expected_total)));
    assert_eq!(figures.last(), Some(&(expected_total, expected_total)));
    assert!(
        figures.windows(2).all(|pair| pair[0].0 <= pair[1].0),
        "the figure only grows: {figures:?}"
    );
    assert_eq!(
        finished(&events),
        vec![Ok(())],
        "exactly one Finished, last"
    );
    assert!(matches!(
        events.last(),
        Some(AppEvent::ProvisioningFinished { .. })
    ));

    assert_eq!(fixture.model_row_status(), DependencyStatus::Ready);
}

#[test]
fn a_partial_set_fetches_only_the_missing_files() {
    let fixture = Fixture::new();
    let files = fixture.q4_files();
    for path in &files[..7] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, fixture.served(path)).unwrap();
    }

    let (result, events) = fixture.provision(DependencyKind::ModelWeights);

    assert_eq!(result, Ok(()));
    assert_eq!(fixture.server.requests().len(), 2);
    let expected_total: u64 = files[7..].iter().map(|p| size_of(fixture.served(p))).sum();
    assert_eq!(progress(&events).first(), Some(&(0, expected_total)));
}

#[test]
fn an_interrupted_part_resumes_with_a_range_request() {
    let fixture = Fixture::new();
    let target =
        assets::graph_file(fixture.root(), "speech_encoder.onnx").with_extension("onnx_data");
    let full = fixture.served(&target).to_vec();
    let kept = 1000;
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(part_path(&target), &full[..kept]).unwrap();

    let (result, events) = fixture.provision(DependencyKind::ModelWeights);

    assert_eq!(result, Ok(()));
    assert_eq!(std::fs::read(&target).unwrap(), full);
    let requests = fixture.server.requests();
    let (_, range) = requests
        .iter()
        .find(|(path, _)| path.ends_with("speech_encoder.onnx_data"))
        .unwrap();
    assert_eq!(range.as_deref(), Some("bytes=1000-"));
    assert_eq!(
        progress(&events).first().map(|(done, _)| *done),
        Some(kept as u64),
        "progress starts at what was already on disk"
    );
}

#[test]
fn a_server_that_ignores_the_range_restarts_the_file_from_zero() {
    let fixture = Fixture::new();
    let target =
        assets::graph_file(fixture.root(), "speech_encoder.onnx").with_extension("onnx_data");
    let full = fixture.served(&target).to_vec();
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(part_path(&target), &full[..1000]).unwrap();
    fixture
        .server
        .set("onnx/speech_encoder.onnx_data", Behaviour::IgnoreRange);

    let (result, events) = fixture.provision(DependencyKind::ModelWeights);

    assert_eq!(result, Ok(()));
    assert_eq!(
        std::fs::read(&target).unwrap(),
        full,
        "the whole body replaced the .part rather than being appended to it"
    );
    let figures = progress(&events);
    let total = figures[0].1;
    assert_eq!(
        figures.last(),
        Some(&(total, total)),
        "the rewind is accounted for"
    );
}

#[test]
fn a_corrupt_download_never_takes_its_final_name() {
    let mut fixture = Fixture::new();
    let target =
        assets::graph_file(fixture.root(), "speech_encoder.onnx").with_extension("onnx_data");
    for asset in &mut fixture.sources.model_files {
        if asset.relative_path == "onnx/speech_encoder.onnx_data" {
            asset.sha256 = "0".repeat(64);
        }
    }

    let (result, events) = fixture.provision(DependencyKind::ModelWeights);

    let expected =
        "speech_encoder.onnx_data did not match its expected checksum; Install will fetch it again";
    assert_eq!(result, Err(expected.to_string()));
    assert_eq!(finished(&events), vec![Err(expected.to_string())]);
    assert!(!target.exists(), "a bad file is never reported as ready");
    assert!(
        !part_path(&target).exists(),
        "and its bytes are not resumed onto"
    );
    assert_eq!(fixture.model_row_status(), DependencyStatus::Missing);
}

#[test]
fn a_dropped_connection_keeps_the_part_and_the_next_install_resumes() {
    let fixture = Fixture::new();
    let target =
        assets::graph_file(fixture.root(), "speech_encoder.onnx").with_extension("onnx_data");
    let full = fixture.served(&target).to_vec();
    fixture
        .server
        .set("onnx/speech_encoder.onnx_data", Behaviour::DropAfter(1500));

    let (result, _) = fixture.provision(DependencyKind::ModelWeights);

    let message = result.expect_err("the connection dropped");
    assert!(
        message.starts_with("Download of speech_encoder.onnx_data failed: "),
        "{message}"
    );
    assert!(
        message.contains("kept, Install resumes from there"),
        "{message}"
    );
    assert!(!target.exists());
    let kept = std::fs::metadata(part_path(&target)).unwrap().len();
    assert_eq!(kept, 1500, "every byte that arrived is kept");
    assert_eq!(fixture.model_row_status(), DependencyStatus::Missing);

    let (result, _) = fixture.provision(DependencyKind::ModelWeights);

    assert_eq!(result, Ok(()));
    assert_eq!(std::fs::read(&target).unwrap(), full);
    let last = fixture
        .server
        .requests()
        .into_iter()
        .rfind(|(path, _)| path.ends_with("speech_encoder.onnx_data"))
        .unwrap();
    assert_eq!(last.1.as_deref(), Some("bytes=1500-"));
}

#[test]
fn an_unwritable_directory_names_the_directory_and_the_reason() {
    let fixture = Fixture::new();
    // A file where the graph directory has to be: no permissions trick that
    // a root-run test would sail through.
    let blocked = fixture.root().join(assets::GRAPH_DIR);
    std::fs::write(&blocked, b"not a directory").unwrap();

    let (result, _) = fixture.provision(DependencyKind::ModelWeights);

    let message = result.expect_err("nothing can be written there");
    assert!(
        message.contains(&blocked.display().to_string()),
        "{message}"
    );
}

/// Backend relativity, end to end: a Q4 provision never requests, and never
/// leaves in the cache, an FP16 file or a GPU provider library.
#[test]
fn a_cpu_provision_never_fetches_fp16_files_or_gpu_libraries() {
    let fixture = Fixture::new();

    let (result, _) = fixture.provision(DependencyKind::ModelWeights);

    assert_eq!(result, Ok(()));
    assert!(
        fixture
            .server
            .requests()
            .iter()
            .all(|(path, _)| !path.contains("fp16")),
        "{:?}",
        fixture.server.requests()
    );
    let mut on_disk = Vec::new();
    for entry in walk(fixture.root()) {
        on_disk.push(entry.file_name().unwrap().to_string_lossy().into_owned());
    }
    assert!(
        !on_disk.iter().any(|name| name.contains("fp16")),
        "{on_disk:?}"
    );
    assert!(
        !on_disk.iter().any(|name| name.contains("providers")),
        "{on_disk:?}"
    );
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(walk(&path));
        } else {
            found.push(path);
        }
    }
    found
}

/// A small `.tgz` shaped like the real one: the real library, a symlink
/// chain onto it, and a provider bridge the CPU path never loads.
fn fake_runtime_archive(library: &[u8]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);

    let mut add_file = |path: &str, bytes: &[u8]| {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append_data(&mut header, path, bytes).unwrap();
    };
    add_file("ort-9.9.9/lib/libonnxruntime.so.9.9.9", library);
    add_file(
        "ort-9.9.9/lib/libonnxruntime_providers_shared.so",
        b"provider bridge",
    );

    let mut link = tar::Header::new_gnu();
    link.set_entry_type(tar::EntryType::Symlink);
    link.set_size(0);
    link.set_mode(0o777);
    link.set_cksum();
    builder
        .append_link(
            &mut link,
            "ort-9.9.9/lib/libonnxruntime.so",
            "libonnxruntime.so.9.9.9",
        )
        .unwrap();

    builder.into_inner().unwrap().finish().unwrap()
}

#[test]
fn a_runtime_install_extracts_only_the_real_library_into_the_cache() {
    let library = b"\x7fELF pretend this is onnxruntime".to_vec();
    let archive = fake_runtime_archive(&library);
    let fixture = Fixture::with_extra_files(
        vec![("runtime/ort-9.9.9.tgz".to_string(), archive)],
        Some("ort-9.9.9/lib/libonnxruntime.so.9.9.9".to_string()),
    );

    let (result, events) = fixture.provision(DependencyKind::OnnxRuntime);

    assert_eq!(result, Ok(()));
    let installed = assets::bundled_runtime_dylib(fixture.root());
    assert_eq!(std::fs::read(&installed).unwrap(), library);
    let runtime_dir: Vec<_> = walk(installed.parent().unwrap())
        .into_iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        runtime_dir,
        vec![assets::runtime_dylib_file_name().to_string()],
        "no provider library, no archive, no .part"
    );
    assert_eq!(finished(&events), vec![Ok(())]);
}

#[test]
fn an_archive_without_the_library_names_the_extraction_failure() {
    let archive = fake_runtime_archive(b"lib");
    let fixture = Fixture::with_extra_files(
        vec![("runtime/ort-9.9.9.tgz".to_string(), archive)],
        Some("ort-9.9.9/lib/not-here.so".to_string()),
    );

    let (result, _) = fixture.provision(DependencyKind::OnnxRuntime);

    let message = result.expect_err("nothing to extract");
    assert!(message.starts_with("Extraction of "), "{message}");
    assert!(message.contains("not-here.so"), "{message}");
    assert!(!assets::bundled_runtime_dylib(fixture.root()).exists());
}

/// Never delete — or write over — a runtime the user configured.
#[test]
fn a_configured_runtime_path_is_never_provisioned_over() {
    let archive = fake_runtime_archive(b"lib");
    let fixture = Fixture::with_extra_files(
        vec![("runtime/ort-9.9.9.tgz".to_string(), archive)],
        Some("ort-9.9.9/lib/libonnxruntime.so.9.9.9".to_string()),
    );
    let mut fixture = fixture;
    let configured = fixture.root().join("mine").join("libonnxruntime.so");
    fixture.env.set_mut(assets::RUNTIME_DYLIB_ENV, &configured);

    let (result, _) = fixture.provision(DependencyKind::OnnxRuntime);

    let message = result.expect_err("a configured path is the user's to fix");
    assert!(message.contains(assets::RUNTIME_DYLIB_ENV), "{message}");
    assert!(!configured.exists());
    assert!(
        fixture.server.requests().is_empty(),
        "nothing was downloaded"
    );
}

/// A second Install on a row already installing is ignored rather than
/// starting a second writer on the same `.part`.
#[test]
fn a_second_install_on_a_row_in_flight_is_ignored() {
    let fixture = Fixture::new();
    let adapter = fixture.adapter();
    let claim = crate::InFlight::claim(&adapter.in_flight, DependencyKind::ModelWeights)
        .expect("the first claim succeeds");

    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let result = adapter.provision(DependencyKind::ModelWeights, SpeechBackend::CPU, tx);

    assert!(result.is_ok());
    assert!(rx.try_recv().is_err(), "the duplicate sends nothing");
    assert!(fixture.server.requests().is_empty());
    drop(claim);
}

/// Matrix row "Virtual mic missing": Install runs the audio crate's
/// idempotent install — no network — and the row's end travels on the
/// channel like any other, failure text included.
#[test]
fn installing_the_virtual_microphone_runs_the_installer_and_reports_its_end() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let adapter = DepsAdapter::new().with_virtual_mic_installer(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let (tx, mut rx) = futures::channel::mpsc::unbounded();

    let result = adapter.provision(DependencyKind::VirtualMicrophone, SpeechBackend::CPU, tx);

    assert!(result.is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        rx.try_recv().unwrap(),
        AppEvent::ProvisioningProgress {
            kind: DependencyKind::VirtualMicrophone,
            done_bytes: 0,
            total_bytes: 0,
        }
    );
    assert_eq!(
        rx.try_recv().unwrap(),
        AppEvent::ProvisioningFinished {
            kind: DependencyKind::VirtualMicrophone,
            result: Ok(()),
        }
    );

    let failing = DepsAdapter::new().with_virtual_mic_installer(|| {
        Err(voice_me_core::VoiceMeError::Other(
            "Could not install the Virtual Microphone: connection refused".to_string(),
        ))
    });
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    assert!(
        failing
            .provision(DependencyKind::VirtualMicrophone, SpeechBackend::CPU, tx)
            .is_err()
    );
    let _progress = rx.try_recv().unwrap();
    match rx.try_recv().unwrap() {
        AppEvent::ProvisioningFinished {
            result: Err(reason),
            ..
        } => {
            assert!(reason.contains("connection refused"), "{reason}")
        }
        other => panic!("expected a failed finish, got {other:?}"),
    }
}

fn speech_encoder_data(fixture: &Fixture) -> PathBuf {
    assets::graph_file(fixture.root(), "speech_encoder.onnx").with_extension("onnx_data")
}

fn requests_for(fixture: &Fixture, suffix: &str) -> Vec<Option<String>> {
    fixture
        .server
        .requests()
        .into_iter()
        .filter(|(path, _)| path.ends_with(suffix))
        .map(|(_, range)| range)
        .collect()
}

/// The wrong file served whole: more bytes than the pin. Writing stops at
/// the pinned size, the `.part` goes, and the figure never passes 100 %.
#[test]
fn an_oversize_body_is_stopped_and_named() {
    let mut fixture = Fixture::new();
    let target = speech_encoder_data(&fixture);
    for asset in &mut fixture.sources.model_files {
        if asset.relative_path == "onnx/speech_encoder.onnx_data" {
            asset.size -= 100;
        }
    }

    let (result, events) = fixture.provision(DependencyKind::ModelWeights);

    let message = result.expect_err("more bytes than the pin");
    assert!(
        message.starts_with("Download of speech_encoder.onnx_data failed: the server sent more"),
        "{message}"
    );
    assert!(!target.exists());
    assert!(!part_path(&target).exists());
    assert!(
        progress(&events).iter().all(|(done, total)| done <= total),
        "{:?}",
        progress(&events)
    );
}

#[test]
fn a_part_longer_than_the_file_restarts_from_zero() {
    let fixture = Fixture::new();
    let target = speech_encoder_data(&fixture);
    let full = fixture.served(&target).to_vec();
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(part_path(&target), vec![7_u8; full.len() + 10]).unwrap();

    let (result, _) = fixture.provision(DependencyKind::ModelWeights);

    assert_eq!(result, Ok(()));
    assert_eq!(std::fs::read(&target).unwrap(), full);
    assert_eq!(
        requests_for(&fixture, "speech_encoder.onnx_data"),
        vec![None],
        "fetched whole, with no range"
    );
}

#[test]
fn a_complete_correct_part_is_renamed_without_a_request() {
    let fixture = Fixture::new();
    let files = fixture.q4_files();
    let (last, rest) = files.split_last().unwrap();
    for path in rest {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, fixture.served(path)).unwrap();
    }
    std::fs::create_dir_all(last.parent().unwrap()).unwrap();
    std::fs::write(part_path(last), fixture.served(last)).unwrap();

    let (result, _) = fixture.provision(DependencyKind::ModelWeights);

    assert_eq!(result, Ok(()));
    assert!(fixture.server.requests().is_empty());
    assert_eq!(std::fs::read(last).unwrap(), fixture.served(last));
    assert!(!part_path(last).exists());
}

#[test]
fn a_resume_from_the_wrong_place_deletes_the_part() {
    let fixture = Fixture::new();
    let target = speech_encoder_data(&fixture);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(part_path(&target), &fixture.served(&target)[..1000]).unwrap();
    fixture.server.set(
        "onnx/speech_encoder.onnx_data",
        Behaviour::WrongContentRange,
    );

    let (result, _) = fixture.provision(DependencyKind::ModelWeights);

    assert_eq!(
        result,
        Err(
            "Download of speech_encoder.onnx_data failed: the server resumed from the wrong \
             place; Install will fetch it again from the start"
                .to_string()
        )
    );
    assert!(!part_path(&target).exists());
    assert!(!target.exists());
}

#[test]
fn a_range_the_server_cannot_satisfy_deletes_the_part() {
    let fixture = Fixture::new();
    let target = speech_encoder_data(&fixture);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(part_path(&target), &fixture.served(&target)[..1000]).unwrap();
    fixture.server.set(
        "onnx/speech_encoder.onnx_data",
        Behaviour::RangeNotSatisfiable,
    );

    let (result, _) = fixture.provision(DependencyKind::ModelWeights);

    assert_eq!(
        result,
        Err(
            "Download of speech_encoder.onnx_data failed: the server could not resume it; \
             Install will fetch it again from the start"
                .to_string()
        )
    );
    assert!(!part_path(&target).exists());
    assert!(!target.exists());
}

/// A verified archive left by a run whose extraction failed is extracted
/// without being fetched again.
#[test]
fn a_verified_archive_already_on_disk_is_extracted_without_a_request() {
    let library = b"\x7fELF already here".to_vec();
    let archive = fake_runtime_archive(&library);
    let fixture = Fixture::with_extra_files(
        vec![("runtime/ort-9.9.9.tgz".to_string(), archive.clone())],
        Some("ort-9.9.9/lib/libonnxruntime.so.9.9.9".to_string()),
    );
    let placed = fixture.root().join("runtime").join("ort-9.9.9.tgz");
    std::fs::create_dir_all(placed.parent().unwrap()).unwrap();
    std::fs::write(&placed, &archive).unwrap();

    let (result, _) = fixture.provision(DependencyKind::OnnxRuntime);

    assert_eq!(result, Ok(()));
    assert!(fixture.server.requests().is_empty());
    assert_eq!(
        std::fs::read(assets::bundled_runtime_dylib(fixture.root())).unwrap(),
        library
    );
    assert!(!placed.exists(), "the archive is removed once extracted");
}

/// The production path: `provision` dispatched through the AD-5 bridge on
/// a multi-threaded runtime, so the adapter reuses that runtime's handle
/// rather than building its own.
#[test]
fn a_q4_install_through_the_tokio_bridge_lands_every_file() {
    let fixture = Fixture::new();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let adapter = fixture.adapter();
    let (tx, _rx) = futures::channel::mpsc::unbounded();

    let result = futures::executor::block_on(voice_me_core::tokio_bridge::spawn_blocking_on(
        runtime.handle(),
        move || adapter.provision(DependencyKind::ModelWeights, SpeechBackend::CPU, tx),
    ));

    assert!(result.is_ok(), "{result:?}");
    for path in fixture.q4_files() {
        assert_eq!(std::fs::read(&path).unwrap(), fixture.served(&path));
    }
}
