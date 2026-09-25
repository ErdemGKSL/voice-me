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
    AppEvent, DependencyKind, DependencyProvisioningPort, DependencyStatus, SpeechWeights, assets,
};

use crate::DepsAdapter;
use crate::provision::part_path;
use crate::sources::{Asset, LibraryEntry, RuntimeArchive, Sources};
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

pub(crate) struct TestServer {
    pub(crate) base: String,
    state: Arc<Mutex<ServerState>>,
}

impl TestServer {
    pub(crate) fn start(files: HashMap<String, Vec<u8>>) -> Self {
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
            digest: crate::sources::Digest::Sha256(sha256(bytes)),
        };
        let model_files = models
            .iter()
            .map(|(rel, bytes)| asset(rel, bytes))
            .collect();
        let runtime = runtime_entry.map(|library_entry| {
            let (rel, bytes) = extra.first().expect("a runtime archive to serve");
            RuntimeArchive {
                archive: asset(rel, bytes),
                library_entries: vec![LibraryEntry::new(
                    library_entry,
                    assets::runtime_dylib_file_name(),
                )],
            }
        });

        Self {
            server,
            files,
            sources: Sources {
                model_files,
                runtime,
                runtime_all_providers: false,
                cuda_runtime: None,
                nvidia_wheels: Vec::new(),
                espeak: None,
                virtual_mic: None,
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
            .provision(kind, voice_me_core::CheckRequest::cpu(), tx)
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
            asset.digest = crate::sources::Digest::Sha256("0".repeat(64));
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

/// Every file in a runtime directory but the record of where the runtime
/// came from.
fn runtime_libraries(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = walk(dir)
        .into_iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .filter(|name| name != assets::RUNTIME_SOURCE_FILE)
        .collect();
    names.sort();
    names
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
    let runtime_dir = runtime_libraries(installed.parent().unwrap());
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

/// A small `.zip` shaped like the Windows release: the real library, the
/// provider bridge the CPU path never loads, a header, and the directory
/// entries a real zip carries.
fn fake_runtime_zip(library: &[u8]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    writer.add_directory("ort-9.9.9/", options).unwrap();
    writer.add_directory("ort-9.9.9/lib/", options).unwrap();
    let mut add_file = |path: &str, bytes: &[u8]| {
        writer.start_file(path, options).unwrap();
        writer.write_all(bytes).unwrap();
    };
    add_file("ort-9.9.9/include/onnxruntime_c_api.h", b"/* header */");
    add_file(
        "ort-9.9.9/lib/onnxruntime_providers_shared.dll",
        b"provider bridge",
    );
    add_file("ort-9.9.9/lib/onnxruntime.dll", library);
    writer.finish().unwrap().into_inner()
}

#[test]
fn a_zip_runtime_install_extracts_only_the_real_library_into_the_cache() {
    let library = b"MZ pretend this is onnxruntime".to_vec();
    let archive = fake_runtime_zip(&library);
    let fixture = Fixture::with_extra_files(
        vec![("runtime/ort-9.9.9.zip".to_string(), archive)],
        Some("ort-9.9.9/lib/onnxruntime.dll".to_string()),
    );

    let (result, events) = fixture.provision(DependencyKind::OnnxRuntime);

    assert_eq!(result, Ok(()));
    let installed = assets::bundled_runtime_dylib(fixture.root());
    assert_eq!(std::fs::read(&installed).unwrap(), library);
    let runtime_dir = runtime_libraries(installed.parent().unwrap());
    assert_eq!(
        runtime_dir,
        vec![assets::runtime_dylib_file_name().to_string()],
        "no provider library, no header, no archive, no .part"
    );
    assert_eq!(finished(&events), vec![Ok(())]);
}

#[test]
fn a_zip_without_the_library_names_the_extraction_failure() {
    let archive = fake_runtime_zip(b"lib");
    let fixture = Fixture::with_extra_files(
        vec![("runtime/ort-9.9.9.zip".to_string(), archive)],
        Some("ort-9.9.9/lib/not-here.dll".to_string()),
    );

    let (result, _) = fixture.provision(DependencyKind::OnnxRuntime);

    let message = result.expect_err("nothing to extract");
    assert!(
        message.starts_with("Extraction of ")
            && message.contains(
                "from ort-9.9.9.zip failed: the archive has no ort-9.9.9/lib/not-here.dll"
            ),
        "{message}"
    );
    assert!(!assets::bundled_runtime_dylib(fixture.root()).exists());
}

/// A directory entry with the library's name is not the library.
#[test]
fn a_zip_directory_entry_is_not_taken_for_the_library() {
    let archive = fake_runtime_zip(b"lib");
    let fixture = Fixture::with_extra_files(
        vec![("runtime/ort-9.9.9.zip".to_string(), archive)],
        Some("ort-9.9.9/lib/".to_string()),
    );

    let (result, _) = fixture.provision(DependencyKind::OnnxRuntime);

    let message = result.expect_err("a directory is not a library");
    assert!(
        message.contains("the archive has no ort-9.9.9/lib/"),
        "{message}"
    );
    assert!(!assets::bundled_runtime_dylib(fixture.root()).exists());
}

/// A symlink entry with the library's name is not the library: extracting
/// it would leave the engine loading whatever it points at.
#[test]
fn a_zip_symlink_entry_is_not_taken_for_the_library() {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    writer
        .start_file("ort-9.9.9/lib/onnxruntime_real.dll", options)
        .unwrap();
    writer.write_all(b"lib").unwrap();
    writer
        .add_symlink(
            "ort-9.9.9/lib/onnxruntime.dll",
            "onnxruntime_real.dll",
            options,
        )
        .unwrap();
    let archive = writer.finish().unwrap().into_inner();
    let fixture = Fixture::with_extra_files(
        vec![("runtime/ort-9.9.9.zip".to_string(), archive)],
        Some("ort-9.9.9/lib/onnxruntime.dll".to_string()),
    );

    let (result, _) = fixture.provision(DependencyKind::OnnxRuntime);

    let message = result.expect_err("a symlink is not a library");
    assert!(message.contains("the archive has no"), "{message}");
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
    let result = adapter.provision(
        DependencyKind::ModelWeights,
        voice_me_core::CheckRequest::cpu(),
        tx,
    );

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
    // No driver pack to fetch: the Linux shape, on every OS.
    let no_pack = || Sources {
        virtual_mic: None,
        ..Sources::pinned()
    };
    let adapter = DepsAdapter::with_sources(no_pack()).with_virtual_mic_installer(move |dir| {
        assert_eq!(dir, None, "nothing was unpacked");
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let (tx, mut rx) = futures::channel::mpsc::unbounded();

    let result = adapter.provision(
        DependencyKind::VirtualMicrophone,
        voice_me_core::CheckRequest::cpu(),
        tx,
    );

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

    let failing = DepsAdapter::with_sources(no_pack()).with_virtual_mic_installer(|_| {
        Err(voice_me_core::VoiceMeError::Other(
            "Could not install the Virtual Microphone: connection refused".to_string(),
        ))
    });
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    assert!(
        failing
            .provision(
                DependencyKind::VirtualMicrophone,
                voice_me_core::CheckRequest::cpu(),
                tx
            )
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

/// Where the fake VB-CABLE pack is served from and lands, relative to the
/// cache root — the real pin's path.
const VB_PACK: &str = "vb-cable/VBCABLE_Driver_Pack45.zip";

/// A small `.zip` shaped like VB's driver pack: the 64-bit setup the
/// installer runs, decoys beside it (the 32-bit setup, the driver files it
/// needs) and a directory entry.
fn fake_vb_cable_pack() -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let add_file = |writer: &mut zip::ZipWriter<_>, path: &str, bytes: &[u8]| {
        writer.start_file(path, options).unwrap();
        writer.write_all(bytes).unwrap();
    };
    add_file(
        &mut writer,
        "VBCABLE_Setup_x64.exe",
        b"MZ pretend 64-bit setup",
    );
    add_file(&mut writer, "VBCABLE_Setup.exe", b"MZ pretend 32-bit setup");
    add_file(&mut writer, "vbMmeCable64_win10.sys", b"driver");
    writer.add_directory("win10/", options).unwrap();
    add_file(&mut writer, "win10/vbMmeCable64_win10.inf", b"[Version]");
    writer.finish().unwrap().into_inner()
}

impl Fixture {
    /// A fixture whose source table pins `served` as the VB-CABLE pack,
    /// checked against `pinned` (the same bytes, unless a test wants a
    /// mismatch).
    fn with_vb_cable_pack(served: Vec<u8>, pinned: &[u8]) -> Self {
        let mut fixture = Self::with_extra_files(vec![(VB_PACK.to_string(), served)], None);
        fixture.sources.virtual_mic = Some(Asset {
            relative_path: VB_PACK.to_string(),
            url: format!("{}/{VB_PACK}", fixture.server.base),
            size: pinned.len() as u64,
            digest: crate::sources::Digest::Sha256(sha256(pinned)),
        });
        fixture
    }

    fn vb_pack(&self) -> PathBuf {
        self.root()
            .join("vb-cable")
            .join("VBCABLE_Driver_Pack45.zip")
    }

    fn vb_unpacked(&self) -> PathBuf {
        self.root().join("vb-cable").join("VBCABLE_Driver_Pack45")
    }

    /// Install on the Virtual Microphone row with a fake installer that
    /// records the directory it was handed and answers `answer`.
    fn install_vb_cable(
        &self,
        answer: Result<(), String>,
    ) -> (Result<(), String>, Vec<AppEvent>, Vec<Option<PathBuf>>) {
        let calls: Arc<Mutex<Vec<Option<PathBuf>>>> = Arc::default();
        let recorded = calls.clone();
        let adapter = self.adapter().with_virtual_mic_installer(move |dir| {
            let setup_there = dir.is_some_and(|dir| dir.join("VBCABLE_Setup_x64.exe").is_file());
            recorded.lock().unwrap().push(dir.map(Path::to_path_buf));
            assert!(setup_there, "the setup is unpacked before it is run");
            answer.clone().map_err(voice_me_core::VoiceMeError::Other)
        });
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let result = adapter
            .provision(
                DependencyKind::VirtualMicrophone,
                voice_me_core::CheckRequest::cpu(),
                tx,
            )
            .map_err(|error| error.to_string());
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        let calls = calls.lock().unwrap().clone();
        (result, events, calls)
    }
}

/// Story 2.8, matrix row "Install": download with progress → verify →
/// extract → the installer runs with the unpacked pack → the zip is gone.
#[test]
fn installing_vb_cable_downloads_unpacks_and_runs_its_setup() {
    let pack = fake_vb_cable_pack();
    let fixture = Fixture::with_vb_cable_pack(pack.clone(), &pack);

    let (result, events, calls) = fixture.install_vb_cable(Ok(()));

    assert_eq!(result, Ok(()));
    assert_eq!(calls, vec![Some(fixture.vb_unpacked())]);
    assert_eq!(
        std::fs::read(fixture.vb_unpacked().join("VBCABLE_Setup_x64.exe")).unwrap(),
        b"MZ pretend 64-bit setup"
    );
    assert_eq!(
        std::fs::read(
            fixture
                .vb_unpacked()
                .join("win10")
                .join("vbMmeCable64_win10.inf")
        )
        .unwrap(),
        b"[Version]"
    );
    assert!(
        !fixture.vb_pack().exists(),
        "the zip is deleted once unpacked"
    );
    assert!(
        walk(fixture.root())
            .iter()
            .all(|path| path.extension().is_none_or(|ext| ext != "part")),
        "no .part left behind"
    );
    assert!(
        fixture.root().join("vb-cable").join("setup-ran").exists(),
        "a finished setup is remembered for the restart hint"
    );

    let size = pack.len() as u64;
    let figures = progress(&events);
    assert_eq!(figures.first(), Some(&(0, size)), "{figures:?}");
    assert!(figures.contains(&(size, size)), "{figures:?}");
    assert_eq!(
        figures.last(),
        Some(&(0, 0)),
        "then \"installing\" while VB's setup runs"
    );
    assert_eq!(finished(&events), vec![Ok(())]);
}

/// Matrix row "Admin prompt declined": the installer's refusal is the row's
/// error, and no "setup ran" marker is left.
#[test]
fn a_declined_admin_prompt_is_the_rows_error() {
    let pack = fake_vb_cable_pack();
    let fixture = Fixture::with_vb_cable_pack(pack.clone(), &pack);

    let (result, events, calls) = fixture.install_vb_cable(Err(
        "Windows did not allow the installer to run.".to_string(),
    ));

    assert_eq!(
        result,
        Err("Windows did not allow the installer to run.".to_string())
    );
    assert_eq!(calls.len(), 1);
    assert_eq!(
        finished(&events),
        vec![Err(
            "Windows did not allow the installer to run.".to_string()
        )]
    );
    assert!(!fixture.root().join("vb-cable").join("setup-ran").exists());
    assert!(
        fixture.vb_pack().exists(),
        "the verified pack is kept for a retry"
    );
}

/// A retry after a failed setup re-extracts the kept pack instead of
/// downloading it again, and only a setup that succeeded deletes the pack
/// and leaves the marker.
#[test]
fn a_retry_after_a_failed_setup_does_not_download_again() {
    let pack = fake_vb_cable_pack();
    let fixture = Fixture::with_vb_cable_pack(pack.clone(), &pack);

    let (result, _, _) = fixture.install_vb_cable(Err(
        "VB-CABLE's setup did not finish (exit code 1).".to_string(),
    ));
    assert!(result.is_err());
    assert!(fixture.vb_pack().exists(), "kept after the failure");
    assert!(!fixture.root().join("vb-cable").join("setup-ran").exists());
    let fetched = requests_for(&fixture, VB_PACK).len();
    assert!(fetched >= 1, "the first attempt downloaded the pack");

    let (result, events, calls) = fixture.install_vb_cable(Ok(()));
    assert_eq!(result, Ok(()));
    assert_eq!(calls, vec![Some(fixture.vb_unpacked())]);
    assert_eq!(
        requests_for(&fixture, VB_PACK).len(),
        fetched,
        "the retry fetched nothing"
    );
    assert!(progress(&events).iter().all(|figure| *figure == (0, 0)));
    assert!(
        !fixture.vb_pack().exists(),
        "deleted once the setup succeeded"
    );
    assert!(fixture.root().join("vb-cable").join("setup-ran").exists());
}

/// A marker left by an earlier successful setup does not survive a new
/// attempt that fails.
#[test]
fn a_failed_setup_clears_an_earlier_marker() {
    let pack = fake_vb_cable_pack();
    let fixture = Fixture::with_vb_cable_pack(pack.clone(), &pack);
    let marker = fixture.root().join("vb-cable").join("setup-ran");
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    std::fs::write(&marker, b"").unwrap();

    let (result, _, _) = fixture.install_vb_cable(Err(
        "Windows did not allow the installer to run.".to_string(),
    ));

    assert!(result.is_err());
    assert!(!marker.exists());
}

/// Matrix row "Install", hash mismatch: nothing keeps the pack's name, no
/// `.part` is left, nothing is unpacked and no installer runs.
#[test]
fn a_vb_cable_pack_that_fails_its_checksum_keeps_no_pack() {
    let pack = fake_vb_cable_pack();
    // Pinned: bytes of the same size that differ in one place.
    let mut expected = pack.clone();
    expected[0] ^= 0xff;
    let fixture = Fixture::with_vb_cable_pack(pack, &expected);

    let (result, events, calls) = fixture.install_vb_cable(Ok(()));

    let message = result.expect_err("a mismatched pack is refused");
    assert!(
        message.contains("VBCABLE_Driver_Pack45.zip did not match its expected checksum"),
        "{message}"
    );
    assert!(calls.is_empty(), "no installer runs");
    assert!(!fixture.vb_pack().exists());
    assert!(!part_path(&fixture.vb_pack()).exists());
    assert!(!fixture.vb_unpacked().exists());
    assert_eq!(finished(&events).len(), 1);
}

/// Zip entries are extracted only by their enclosed names: a pack holding
/// `../` is refused whole, before anything is written.
#[test]
fn a_vb_cable_pack_with_an_escaping_entry_is_refused() {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    writer.start_file("VBCABLE_Setup_x64.exe", options).unwrap();
    writer.write_all(b"MZ setup").unwrap();
    writer.start_file("../escaped.txt", options).unwrap();
    writer.write_all(b"outside").unwrap();
    let pack = writer.finish().unwrap().into_inner();
    let fixture = Fixture::with_vb_cable_pack(pack.clone(), &pack);

    let (result, _, calls) = fixture.install_vb_cable(Ok(()));

    let message = result.expect_err("an escaping entry is refused");
    assert!(message.contains("unsafe name"), "{message}");
    assert!(message.contains("../escaped.txt"), "{message}");
    assert!(calls.is_empty(), "no installer runs");
    assert!(!fixture.root().join("vb-cable").join("escaped.txt").exists());
    assert!(!fixture.root().join("escaped.txt").exists());
    assert!(
        !fixture.vb_unpacked().join("VBCABLE_Setup_x64.exe").exists(),
        "nothing is written from a refused archive"
    );
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
        move || {
            adapter.provision(
                DependencyKind::ModelWeights,
                voice_me_core::CheckRequest::cpu(),
                tx,
            )
        },
    ));

    assert!(result.is_ok(), "{result:?}");
    for path in fixture.q4_files() {
        assert_eq!(std::fs::read(&path).unwrap(), fixture.served(&path));
    }
}

// Story 3.16: Install on the Windows eSpeak NG row, driven on any OS
// through an injected MSI source and a fake unpacker.

const ESPEAK_MSI: &str = "espeak-ng.msi";

/// Fake MSI bytes, a few KB so progress has figures to report.
fn fake_espeak_msi() -> Vec<u8> {
    (0..6000).map(|n| (n * 13 % 251) as u8).collect()
}

/// A fixture serving `msi` as eSpeak NG's package, pinned to `pinned`'s
/// SHA-256.
fn espeak_fixture(msi: Vec<u8>, pinned: &[u8]) -> Fixture {
    let mut fixture = Fixture::with_extra_files(vec![(ESPEAK_MSI.to_string(), msi)], None);
    fixture.sources.espeak = Some(Asset {
        relative_path: ESPEAK_MSI.to_string(),
        url: format!("{}/{ESPEAK_MSI}", fixture.server.base),
        size: pinned.len() as u64,
        digest: crate::sources::Digest::Sha256(sha256(pinned)),
    });
    fixture
}

/// Install on the eSpeak NG row through `adapter`.
fn provision_espeak_with(adapter: DepsAdapter) -> (Result<(), String>, Vec<AppEvent>) {
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let result = adapter
        .provision(
            DependencyKind::SystemVoiceEngine,
            voice_me_core::CheckRequest::cpu(),
            tx,
        )
        .map_err(|error| error.to_string());
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    (result, events)
}

/// Lays out what unpacking the MSI leaves in `target`: the program, its data
/// directory and its library.
fn fake_unpacked_image(target: &Path) {
    let dir = target.join("eSpeak NG");
    std::fs::create_dir_all(dir.join("espeak-ng-data")).unwrap();
    std::fs::write(dir.join("espeak-ng.exe"), b"MZ").unwrap();
    std::fs::write(dir.join("libespeak-ng.dll"), b"MZ").unwrap();
}

fn espeak_dirs(fixture: &Fixture) -> (PathBuf, PathBuf, PathBuf) {
    (
        assets::espeak_dir(fixture.root()),
        fixture.root().join("espeak-ng.tmp"),
        fixture.root().join(ESPEAK_MSI),
    )
}

/// The Install row of the matrix: download with progress → verify →
/// unpack → Ready, and the `.msi` is gone afterwards.
#[test]
fn an_espeak_install_unpacks_the_verified_msi_and_deletes_it() {
    let msi = fake_espeak_msi();
    let fixture = espeak_fixture(msi.clone(), &msi);
    let (espeak_dir, staging, msi_path) = espeak_dirs(&fixture);
    let unpacked_from = Arc::new(Mutex::new(Vec::new()));
    let seen = unpacked_from.clone();
    let adapter = fixture
        .adapter()
        .with_espeak_unpacker(move |package: &Path, target: &Path| {
            assert!(!target.exists(), "the unpack goes into a fresh directory");
            seen.lock()
                .unwrap()
                .push((std::fs::read(package).unwrap(), target.to_path_buf()));
            fake_unpacked_image(target);
            Ok(())
        });

    let (result, events) = provision_espeak_with(adapter);

    assert_eq!(result, Ok(()));
    let calls = unpacked_from.lock().unwrap().clone();
    assert_eq!(calls, vec![(msi.clone(), staging.clone())]);
    let program = espeak_dir.join("eSpeak NG").join("espeak-ng.exe");
    assert!(program.is_file());
    assert!(espeak_dir.join("eSpeak NG").join("espeak-ng-data").is_dir());
    assert!(!staging.exists());
    assert!(!msi_path.exists(), "the .msi is deleted afterwards");
    assert!(!part_path(&msi_path).exists());

    let total = msi.len() as u64;
    let figures = progress(&events);
    assert_eq!(figures.first(), Some(&(0, total)));
    assert_eq!(figures.last(), Some(&(total, total)));
    assert_eq!(finished(&events), vec![Ok(())]);
    assert!(events.iter().all(|event| match event {
        AppEvent::ProvisioningProgress { kind, .. }
        | AppEvent::ProvisioningFinished { kind, .. } => *kind == DependencyKind::SystemVoiceEngine,
        _ => true,
    }));

    // The row then reads Ready, found where voice-me's Windows lookup
    // looks first.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        let found = voice_me_espeak::find_windows_program(Some(&espeak_dir), None, &[]);
        assert_eq!(found.as_deref(), Some(program.as_path()));
        let row = crate::capability::windows_espeak_row(found.as_deref(), true);
        assert_eq!(row.status, DependencyStatus::Ready);
        assert!(row.detail.contains("espeak-ng.exe"), "{}", row.detail);
    }

    // A second Install finds it there and fetches nothing.
    let requests = fixture.server.requests().len();
    let (again, _) =
        provision_espeak_with(fixture.adapter().with_espeak_unpacker(|_, _| {
            panic!("nothing to unpack when eSpeak NG is already there")
        }));
    assert_eq!(again, Ok(()));
    assert_eq!(fixture.server.requests().len(), requests);
}

/// A stale `espeak-ng.msi` already in the cache (an older pin, a truncated
/// copy) is not unpacked: it is replaced by the verified download, which
/// is.
#[test]
fn a_stale_espeak_msi_in_the_cache_is_fetched_again_before_unpacking() {
    let msi = fake_espeak_msi();
    let fixture = espeak_fixture(msi.clone(), &msi);
    let (espeak_dir, _, msi_path) = espeak_dirs(&fixture);
    std::fs::write(&msi_path, b"an older espeak-ng.msi").unwrap();
    let unpacked = Arc::new(Mutex::new(Vec::new()));
    let seen = unpacked.clone();

    let (result, _) = provision_espeak_with(fixture.adapter().with_espeak_unpacker(
        move |package: &Path, target: &Path| {
            seen.lock().unwrap().push(std::fs::read(package).unwrap());
            fake_unpacked_image(target);
            Ok(())
        },
    ));

    assert_eq!(result, Ok(()));
    assert_eq!(fixture.server.requests().len(), 1, "fetched again");
    assert_eq!(*unpacked.lock().unwrap(), vec![msi], "the verified one");
    assert!(assets::espeak_program(&espeak_dir).is_file());
    assert!(!msi_path.exists());
}

/// Already unpacked, Install deletes a leftover package and fetches
/// nothing.
#[test]
fn an_unpacked_espeak_ng_takes_a_leftover_msi_with_it() {
    let msi = fake_espeak_msi();
    let fixture = espeak_fixture(msi.clone(), &msi);
    let (espeak_dir, _, msi_path) = espeak_dirs(&fixture);
    fake_unpacked_image(&espeak_dir);
    std::fs::write(&msi_path, &msi).unwrap();

    let (result, _) = provision_espeak_with(
        fixture
            .adapter()
            .with_espeak_unpacker(|_, _| panic!("nothing to unpack")),
    );

    assert_eq!(result, Ok(()));
    assert!(!msi_path.exists());
    assert!(fixture.server.requests().is_empty());
}

/// A package that does not match its pin is never unpacked, and nothing
/// of it stays behind.
#[test]
fn an_espeak_msi_with_the_wrong_hash_installs_nothing() {
    let msi = fake_espeak_msi();
    let mut other = msi.clone();
    other[10] ^= 0xff;
    let fixture = espeak_fixture(msi, &other);
    let (espeak_dir, staging, msi_path) = espeak_dirs(&fixture);

    let (result, events) =
        provision_espeak_with(fixture.adapter().with_espeak_unpacker(|_, _| {
            panic!("a package that failed its check is never unpacked")
        }));

    let error = result.unwrap_err();
    assert!(error.contains(ESPEAK_MSI), "{error}");
    assert!(!espeak_dir.exists());
    assert!(!staging.exists());
    assert!(!msi_path.exists());
    assert!(!part_path(&msi_path).exists());
    assert_eq!(finished(&events).len(), 1);
    assert!(finished(&events)[0].is_err());
}

/// An unpacker that fails part-way leaves no `espeak-ng` directory and no
/// staging directory, and its reason reaches the row.
#[test]
fn a_failed_espeak_unpack_leaves_no_install_behind() {
    let msi = fake_espeak_msi();
    let fixture = espeak_fixture(msi.clone(), &msi);
    let (espeak_dir, staging, _) = espeak_dirs(&fixture);

    let (result, events) =
        provision_espeak_with(fixture.adapter().with_espeak_unpacker(|_, target: &Path| {
            std::fs::create_dir_all(target.join("eSpeak NG")).unwrap();
            std::fs::write(target.join("eSpeak NG").join("espeak-ng.exe"), b"half").unwrap();
            Err(voice_me_core::VoiceMeError::Other(
                "Could not unpack eSpeak NG: the package is damaged.".to_string(),
            ))
        }));

    let error = result.unwrap_err();
    assert!(error.contains("the package is damaged"), "{error}");
    assert!(!espeak_dir.exists(), "no half-installed program");
    assert!(!staging.exists());
    assert!(matches!(finished(&events).as_slice(), [Err(reason)] if reason.contains("damaged")));
}

/// An unpack that "succeeds" without `espeak-ng.exe` is refused, and
/// nothing is installed.
#[test]
fn an_espeak_unpack_without_the_program_is_refused() {
    let msi = fake_espeak_msi();
    let fixture = espeak_fixture(msi.clone(), &msi);
    let (espeak_dir, staging, _) = espeak_dirs(&fixture);

    let (result, _) =
        provision_espeak_with(fixture.adapter().with_espeak_unpacker(|_, target: &Path| {
            std::fs::create_dir_all(target.join("eSpeak NG").join("espeak-ng-data")).unwrap();
            Ok(())
        }));

    let error = result.unwrap_err();
    assert!(error.contains("espeak-ng.exe"), "{error}");
    assert!(!espeak_dir.exists());
    assert!(!staging.exists());
}

/// Linux has no eSpeak NG download: its row keeps manual steps, and
/// Install there is still refused.
#[cfg(target_os = "linux")]
#[test]
fn the_linux_espeak_ng_provision_is_still_refused() {
    let fixture = Fixture::new();
    assert!(fixture.sources.espeak.is_none());
    assert!(crate::sources::Sources::pinned().espeak.is_none());

    let (result, events) = provision_espeak_with(fixture.adapter());

    let error = result.unwrap_err();
    assert!(error.contains("cannot install eSpeak NG"), "{error}");
    assert!(fixture.server.requests().is_empty());
    assert!(!assets::espeak_dir(fixture.root()).exists());
    assert!(matches!(finished(&events).as_slice(), [Err(_)]));
}

// ---------------------------------------------------------------------------
// Story 3.8: voice-me's all-provider runtime, its CUDA provider, and the
// NVIDIA wheels.
// ---------------------------------------------------------------------------

/// A `.tgz` holding `<dir>/lib/<name>` for each file, plus a licence that
/// is never extracted.
fn fake_release_tgz(dir: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    let mut add_file = |path: &str, bytes: &[u8]| {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, path, bytes).unwrap();
    };
    add_file(&format!("{dir}/LICENSE"), b"MIT");
    for (name, bytes) in files {
        add_file(&format!("{dir}/lib/{name}"), bytes);
    }
    builder.into_inner().unwrap().finish().unwrap()
}

/// A wheel is a zip: the libraries under `nvidia/<pkg>/lib/`, plus the
/// Python files and headers voice-me never extracts.
fn fake_wheel(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut add_file = |path: &str, bytes: &[u8]| {
        writer.start_file(path, options).unwrap();
        writer.write_all(bytes).unwrap();
    };
    add_file("nvidia/__init__.py", b"");
    add_file("nvidia/cudnn/include/cudnn.h", b"/* header */");
    for (path, bytes) in files {
        add_file(path, bytes);
    }
    writer.finish().unwrap().into_inner()
}

const CORE_LIB: &[u8] = b"\x7fELF voice-me onnxruntime";
const SHARED_LIB: &[u8] = b"\x7fELF providers_shared";
const CUDA_LIB: &[u8] = b"\x7fELF providers_cuda";
const CUDNN_LIB: &[u8] = b"\x7fELF libcudnn.so.9";
const CUDART_LIB: &[u8] = b"\x7fELF libcudart.so.12";

/// The fixture with voice-me's release pinned: the core, the CUDA
/// provider, a cuDNN wheel and a CUDA runtime wheel, all served locally.
fn all_provider_fixture() -> Fixture {
    let lib = assets::runtime_dylib_file_name();
    let provider = assets::cuda_provider_file_name();
    let core = fake_release_tgz(
        "core-9.9.9",
        &[
            ("libonnxruntime.so.9.9.9", CORE_LIB),
            ("libonnxruntime_providers_shared.so", SHARED_LIB),
        ],
    );
    let cuda = fake_release_tgz(
        "cuda-9.9.9",
        &[("libonnxruntime_providers_cuda.so", CUDA_LIB)],
    );
    let cudnn = fake_wheel(&[
        ("nvidia/cudnn/lib/libcudnn.so.9", CUDNN_LIB),
        ("nvidia/cudnn/lib/libcudnn_ops.so.9", b"not asked for"),
    ]);
    let cudart = fake_wheel(&[("nvidia/cuda_runtime/lib/libcudart.so.12", CUDART_LIB)]);
    let mut fixture = Fixture::with_extra_files(
        vec![
            ("runtime/core-9.9.9.tgz".to_string(), core),
            ("runtime/cuda-9.9.9.tgz".to_string(), cuda),
            ("runtime/cuda/nvidia-cudnn.whl".to_string(), cudnn),
            ("runtime/cuda/nvidia-cudart.whl".to_string(), cudart),
        ],
        None,
    );
    let archive = |rel: &str, entries: Vec<LibraryEntry>| {
        let bytes = &fixture.files[rel];
        RuntimeArchive {
            archive: Asset {
                relative_path: rel.to_string(),
                url: format!("{}/{rel}", fixture.server.base),
                size: bytes.len() as u64,
                digest: crate::sources::Digest::Sha256(sha256(bytes)),
            },
            library_entries: entries,
        }
    };
    let runtime = archive(
        "runtime/core-9.9.9.tgz",
        vec![
            LibraryEntry::new("core-9.9.9/lib/libonnxruntime.so.9.9.9", lib),
            LibraryEntry::new(
                "core-9.9.9/lib/libonnxruntime_providers_shared.so",
                "libonnxruntime_providers_shared.so",
            ),
        ],
    );
    let cuda_runtime = archive(
        "runtime/cuda-9.9.9.tgz",
        vec![LibraryEntry::new(
            "cuda-9.9.9/lib/libonnxruntime_providers_cuda.so",
            provider,
        )],
    );
    let wheels = vec![
        archive(
            "runtime/cuda/nvidia-cudart.whl",
            vec![LibraryEntry::new(
                "nvidia/cuda_runtime/lib/libcudart.so.12",
                "libcudart.so.12",
            )],
        ),
        archive(
            "runtime/cuda/nvidia-cudnn.whl",
            vec![LibraryEntry::new(
                "nvidia/cudnn/lib/libcudnn.so.9",
                "libcudnn.so.9",
            )],
        ),
    ];
    fixture.sources.runtime = Some(runtime);
    fixture.sources.runtime_all_providers = true;
    fixture.sources.cuda_runtime = Some(cuda_runtime);
    fixture.sources.nvidia_wheels = wheels;
    fixture
}

fn local_request(target: voice_me_core::SpeechExecutionTarget) -> voice_me_core::CheckRequest {
    voice_me_core::CheckRequest {
        backend: voice_me_core::SpeechBackend::for_target(target),
        selection: voice_me_core::BackendSelection::Local {
            runtime: None,
            target,
        },
        ..voice_me_core::CheckRequest::cpu()
    }
}

impl Fixture {
    fn provision_for(
        &self,
        kind: DependencyKind,
        request: voice_me_core::CheckRequest,
    ) -> (Result<(), String>, Vec<AppEvent>) {
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let result = self
            .adapter()
            .provision(kind, request, tx)
            .map_err(|error| error.to_string());
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        (result, events)
    }

    fn rows_for(&self, request: voice_me_core::CheckRequest) -> Vec<voice_me_core::Dependency> {
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        self.adapter().check(request, tx).unwrap();
        let Ok(AppEvent::DependencyCheckCompleted { report }) = rx.try_recv() else {
            panic!("the check reports by event");
        };
        report.dependencies
    }
}

/// Matrix row "CUDA": one Install fetches the core, the CUDA provider and
/// every NVIDIA wheel in one download with one progress figure, and
/// extracts only their shared libraries — the provider beside the runtime,
/// NVIDIA's into `runtime/cuda/`.
#[test]
fn one_cuda_install_fetches_the_runtime_the_provider_and_the_nvidia_libraries() {
    let fixture = all_provider_fixture();
    let cuda = local_request(voice_me_core::SpeechExecutionTarget::Cuda);

    let (result, events) = fixture.provision_for(DependencyKind::NvidiaLibraries, cuda.clone());

    assert_eq!(result, Ok(()));
    let root = fixture.root();
    assert_eq!(
        std::fs::read(assets::bundled_runtime_dylib(root)).unwrap(),
        CORE_LIB
    );
    assert_eq!(
        std::fs::read(assets::bundled_cuda_provider(root)).unwrap(),
        CUDA_LIB
    );
    let cuda_dir = assets::cuda_libraries_dir(root);
    assert_eq!(
        runtime_libraries(&cuda_dir),
        vec!["libcudart.so.12".to_string(), "libcudnn.so.9".to_string()],
        "only the listed libraries: no wheel, no header, no decoy"
    );
    assert_eq!(
        std::fs::read(cuda_dir.join("libcudnn.so.9")).unwrap(),
        CUDNN_LIB
    );
    let mut top: Vec<_> = std::fs::read_dir(assets::runtime_dir(root))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    top.sort();
    let mut expected = vec![
        assets::RUNTIME_SOURCE_FILE.to_string(),
        assets::CUDA_LIBRARIES_DIR.to_string(),
        assets::cuda_provider_file_name().to_string(),
        assets::runtime_dylib_file_name().to_string(),
        "libonnxruntime_providers_shared.so".to_string(),
    ];
    expected.sort();
    assert_eq!(top, expected, "no archive and no .part left behind");
    let core = fixture.sources.runtime.as_ref().unwrap();
    assert_eq!(
        std::fs::read_to_string(assets::runtime_source_file(root)).unwrap(),
        format!(
            "{} sha256:{}",
            core.archive.url,
            core.archive.digest.expected()
        )
    );

    // One figure, on the row that was clicked, over all four archives.
    let total: u64 = fixture
        .files
        .iter()
        .filter(|(rel, _)| rel.starts_with("runtime/"))
        .map(|(_, bytes)| size_of(bytes))
        .sum();
    let figures = progress(&events);
    assert_eq!(figures.last(), Some(&(total, total)), "{figures:?}");
    assert!(events.iter().all(|event| !matches!(
        event,
        AppEvent::ProvisioningProgress { kind, .. } if *kind != DependencyKind::NvidiaLibraries
    )));
    assert_eq!(finished(&events), vec![Ok(())]);

    // Every CUDA row now reads ready, and Install on another of them has
    // nothing left to fetch.
    let rows = fixture.rows_for(cuda.clone());
    for kind in [
        DependencyKind::OnnxRuntime,
        DependencyKind::CudaProvider,
        DependencyKind::NvidiaLibraries,
    ] {
        let row = rows.iter().find(|row| row.kind == kind).unwrap();
        assert!(!row.status.is_missing(), "{row:?}");
    }
    let requests = fixture.server.requests().len();
    let (again, _) = fixture.provision_for(DependencyKind::CudaProvider, cuda);
    assert_eq!(again, Ok(()));
    assert_eq!(fixture.server.requests().len(), requests);
}

/// Matrix row "CPU user": with the release pinned, a CPU install fetches
/// only the core archive — never the CUDA provider or a wheel.
#[test]
fn a_cpu_install_fetches_only_the_core_runtime() {
    let fixture = all_provider_fixture();

    let (result, _) = fixture.provision(DependencyKind::OnnxRuntime);

    assert_eq!(result, Ok(()));
    let requested: Vec<_> = fixture
        .server
        .requests()
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    assert_eq!(requested, vec!["runtime/core-9.9.9.tgz".to_string()]);
    assert!(!assets::bundled_cuda_provider(fixture.root()).exists());
    assert!(!assets::cuda_libraries_dir(fixture.root()).exists());
}

/// Matrix row "A missing piece is its own row; Install fetches the rest":
/// with the runtime already in place, only the missing wheel is fetched.
#[test]
fn a_cuda_install_fetches_only_the_missing_pieces() {
    let fixture = all_provider_fixture();
    let cuda = local_request(voice_me_core::SpeechExecutionTarget::Cuda);
    let (first, _) = fixture.provision_for(DependencyKind::OnnxRuntime, cuda.clone());
    assert_eq!(first, Ok(()));
    let cudnn = assets::cuda_libraries_dir(fixture.root()).join("libcudnn.so.9");
    std::fs::remove_file(&cudnn).unwrap();
    let before = fixture.server.requests().len();

    let (result, _) = fixture.provision_for(DependencyKind::NvidiaLibraries, cuda);

    assert_eq!(result, Ok(()));
    let requests = fixture.server.requests();
    let later: Vec<_> = requests[before..]
        .iter()
        .map(|(path, _)| path.as_str())
        .collect();
    assert_eq!(later, ["runtime/cuda/nvidia-cudnn.whl"]);
    assert_eq!(std::fs::read(&cudnn).unwrap(), CUDNN_LIB);
}

/// Matrix row "WebGPU" after pinning: the CPU-only runtime a user already
/// has is replaced by voice-me's build, and a CUDA provider left from an
/// older build goes with it.
#[test]
fn a_webgpu_install_replaces_a_cpu_only_runtime() {
    let fixture = all_provider_fixture();
    let root = fixture.root().to_path_buf();
    std::fs::create_dir_all(assets::runtime_dir(&root)).unwrap();
    std::fs::write(assets::bundled_runtime_dylib(&root), b"microsoft cpu build").unwrap();
    std::fs::write(assets::bundled_cuda_provider(&root), b"old provider").unwrap();

    let (result, _) = fixture.provision_for(
        DependencyKind::OnnxRuntime,
        local_request(voice_me_core::SpeechExecutionTarget::WebGpu),
    );

    assert_eq!(result, Ok(()));
    assert_eq!(
        std::fs::read(assets::bundled_runtime_dylib(&root)).unwrap(),
        CORE_LIB
    );
    assert!(!assets::bundled_cuda_provider(&root).exists());
    assert!(
        !assets::cuda_libraries_dir(&root).exists(),
        "WebGPU needs no NVIDIA library"
    );
}

/// A core archive that lacks one of its libraries installs none of them:
/// half a runtime would read as installed.
#[test]
fn a_core_archive_missing_a_library_installs_nothing() {
    let mut fixture = all_provider_fixture();
    let runtime = fixture.sources.runtime.as_mut().unwrap();
    runtime.library_entries.push(LibraryEntry::new(
        "core-9.9.9/lib/libwebgpu_dawn.so",
        "libwebgpu_dawn.so",
    ));

    let (result, _) = fixture.provision(DependencyKind::OnnxRuntime);

    let message = result.expect_err("the archive has no Dawn library");
    assert!(
        message.contains("Extraction of libwebgpu_dawn.so from core-9.9.9.tgz failed")
            && message.contains("the archive has no core-9.9.9/lib/libwebgpu_dawn.so"),
        "{message}"
    );
    assert!(!assets::bundled_runtime_dylib(fixture.root()).exists());
    assert!(
        runtime_libraries(&assets::runtime_dir(fixture.root()))
            .iter()
            .all(|name| name.ends_with(".tgz")),
        "only the verified archive is kept"
    );
}

/// Before the release is pinned (decision 7), Install for CUDA is refused
/// with today's sentence, and nothing is fetched.
#[test]
fn before_pinning_a_cuda_install_is_refused_as_before() {
    let fixture = Fixture::with_extra_files(
        vec![(
            "runtime/ort-9.9.9.tgz".to_string(),
            fake_runtime_archive(b"lib"),
        )],
        Some("ort-9.9.9/lib/libonnxruntime.so.9.9.9".to_string()),
    );

    let (result, _) = fixture.provision_for(
        DependencyKind::OnnxRuntime,
        local_request(voice_me_core::SpeechExecutionTarget::Cuda),
    );

    let message = result.expect_err("no CUDA runtime source yet");
    assert!(message.contains("not yet available"), "{message}");
    assert!(fixture.server.requests().is_empty());
}

/// Piper on CPU installs the CPU runtime: the request's backend is Piper's
/// own device (spec-backend-engine-and-device-selects), not Chatterbox's.
#[test]
fn a_piper_runtime_install_on_cpu_installs_the_cpu_runtime() {
    let fixture = Fixture::with_extra_files(
        vec![(
            "runtime/ort-9.9.9.tgz".to_string(),
            fake_runtime_archive(b"lib"),
        )],
        Some("ort-9.9.9/lib/libonnxruntime.so.9.9.9".to_string()),
    );
    let request = voice_me_core::CheckRequest {
        selection: voice_me_core::BackendSelection::PIPER_CPU,
        ..voice_me_core::CheckRequest::cpu()
    };

    let (result, _) = fixture.provision_for(DependencyKind::OnnxRuntime, request);

    assert_eq!(result, Ok(()));
    assert!(assets::bundled_runtime_dylib(fixture.root()).exists());
}

/// The libraries of a failed rename go back as they were: the directory
/// holds the old set or the new one, never a mix.
#[test]
fn a_failed_rename_puts_the_old_libraries_back() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("core.tgz");
    std::fs::write(
        &archive,
        fake_release_tgz("core", &[("liba.so", b"new a"), ("libb.so", b"new b")]),
    )
    .unwrap();
    let out = dir.path().join("runtime");
    std::fs::create_dir_all(out.join("libb.so")).unwrap();
    std::fs::write(out.join("libb.so").join("keep"), b"x").unwrap();
    std::fs::write(out.join("liba.so"), b"old a").unwrap();
    let entries = [
        LibraryEntry::new("core/lib/liba.so", "liba.so"),
        LibraryEntry::new("core/lib/libb.so", "libb.so"),
    ];

    let error = crate::provision::extract_libraries(&archive, &entries, &out)
        .expect_err("libb.so cannot replace a directory");

    assert!(error.to_string().contains("libb.so"), "{error}");
    assert_eq!(std::fs::read(out.join("liba.so")).unwrap(), b"old a");
    assert!(out.join("libb.so").join("keep").exists());
    let mut left: Vec<_> = std::fs::read_dir(&out)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["liba.so", "libb.so"], "no .old, no .part");

    // With the obstacle gone the new set lands whole, and the backups go.
    std::fs::remove_dir_all(out.join("libb.so")).unwrap();
    crate::provision::extract_libraries(&archive, &entries, &out).unwrap();
    assert_eq!(std::fs::read(out.join("liba.so")).unwrap(), b"new a");
    assert_eq!(std::fs::read(out.join("libb.so")).unwrap(), b"new b");
    assert!(!out.join("liba.so.old").exists());
}

/// The old CUDA provider goes only once the new core is in place: a core
/// that fails to extract leaves the provider where it was.
#[test]
fn a_failed_core_extraction_keeps_the_old_provider() {
    let mut fixture = all_provider_fixture();
    fixture
        .sources
        .runtime
        .as_mut()
        .unwrap()
        .library_entries
        .push(LibraryEntry::new("core-9.9.9/lib/missing.so", "missing.so"));
    let provider = assets::bundled_cuda_provider(fixture.root());
    std::fs::create_dir_all(provider.parent().unwrap()).unwrap();
    std::fs::write(&provider, b"old provider").unwrap();

    let (result, _) = fixture.provision_for(
        DependencyKind::OnnxRuntime,
        local_request(voice_me_core::SpeechExecutionTarget::WebGpu),
    );

    assert!(result.is_err());
    assert_eq!(std::fs::read(&provider).unwrap(), b"old provider");
}

/// An archive on disk that no longer matches its pin (the pin changed
/// since it was fetched) is fetched again rather than extracted.
#[test]
fn an_archive_on_disk_that_fails_its_pin_is_fetched_again() {
    let fixture = all_provider_fixture();
    let placed = fixture.root().join("runtime").join("core-9.9.9.tgz");
    std::fs::create_dir_all(placed.parent().unwrap()).unwrap();
    std::fs::write(&placed, b"an archive from an older pin").unwrap();

    let (result, _) = fixture.provision(DependencyKind::OnnxRuntime);

    assert_eq!(result, Ok(()));
    let requested: Vec<_> = fixture
        .server
        .requests()
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    assert_eq!(requested, ["runtime/core-9.9.9.tgz"]);
    assert_eq!(
        std::fs::read(assets::bundled_runtime_dylib(fixture.root())).unwrap(),
        CORE_LIB
    );
}

/// NVIDIA libraries from an older wheel pin are cleared before the new
/// wheels are extracted, and the record names only the new ones.
#[test]
fn a_repinned_wheel_clears_the_nvidia_folder_first() {
    let fixture = all_provider_fixture();
    let cuda = local_request(voice_me_core::SpeechExecutionTarget::Cuda);
    assert_eq!(
        fixture
            .provision_for(DependencyKind::NvidiaLibraries, cuda.clone())
            .0,
        Ok(())
    );
    let cuda_dir = assets::cuda_libraries_dir(fixture.root());
    std::fs::write(cuda_dir.join("libcudnn_old.so.9"), b"from the old wheel").unwrap();
    let marker = assets::cuda_libraries_source_file(fixture.root());
    let recorded = std::fs::read_to_string(&marker).unwrap();
    std::fs::write(&marker, recorded.replacen("sha256:", "sha256:00", 1)).unwrap();

    let (result, _) = fixture.provision_for(DependencyKind::NvidiaLibraries, cuda);

    assert_eq!(result, Ok(()));
    assert!(!cuda_dir.join("libcudnn_old.so.9").exists());
    assert_eq!(
        assets::installed_cuda_libraries(fixture.root()),
        ["libcudart.so.12", "libcudnn.so.9"]
    );
}

/// With no runtime source for this system, Install on a runtime that is
/// already there has nothing to do — and is not an error.
#[test]
fn a_cpu_install_with_nothing_missing_is_fine_without_a_source() {
    let fixture = Fixture::new();
    let installed = assets::bundled_runtime_dylib(fixture.root());
    std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
    std::fs::write(&installed, b"lib").unwrap();

    let (result, _) = fixture.provision(DependencyKind::OnnxRuntime);

    assert_eq!(result, Ok(()));
    std::fs::remove_file(&installed).unwrap();
    let (result, _) = fixture.provision(DependencyKind::OnnxRuntime);
    assert!(result.unwrap_err().contains("cannot install"));
}

/// Story 3.8 (Windows): with the runtime loaded in this process the new
/// core and provider are staged for the next start, the loaded files are
/// left alone, and the NVIDIA libraries go straight into `runtime/cuda/`.
#[test]
fn an_update_to_a_loaded_runtime_is_staged() {
    let fixture = all_provider_fixture();
    let root = fixture.root().to_path_buf();
    std::fs::create_dir_all(assets::runtime_dir(&root)).unwrap();
    std::fs::write(assets::bundled_runtime_dylib(&root), b"loaded cpu build").unwrap();
    let adapter = fixture
        .adapter()
        .with_runtime_in_use(|| true)
        .staging_when_in_use();
    let (tx, _rx) = futures::channel::mpsc::unbounded();

    let result = adapter.provision(
        DependencyKind::OnnxRuntime,
        local_request(voice_me_core::SpeechExecutionTarget::Cuda),
        tx,
    );

    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        std::fs::read(assets::bundled_runtime_dylib(&root)).unwrap(),
        b"loaded cpu build"
    );
    let staged = assets::staged_runtime_dir(&root);
    assert_eq!(
        std::fs::read(staged.join(assets::runtime_dylib_file_name())).unwrap(),
        CORE_LIB
    );
    assert_eq!(
        std::fs::read(staged.join(assets::cuda_provider_file_name())).unwrap(),
        CUDA_LIB
    );
    assert!(crate::staged_runtime_pending(&root));
    assert!(
        assets::cuda_libraries_dir(&root)
            .join("libcudnn.so.9")
            .exists()
    );

    // A second Install waits for the restart instead of staging again.
    let requests = fixture.server.requests().len();
    let (again, _) = fixture.provision_for(
        DependencyKind::CudaProvider,
        local_request(voice_me_core::SpeechExecutionTarget::Cuda),
    );
    assert_eq!(again, Ok(()));
    assert_eq!(fixture.server.requests().len(), requests);

    // At the next start the staged set is put in place.
    assert!(crate::apply_staged_runtime(&root).unwrap());
    assert_eq!(
        std::fs::read(assets::bundled_runtime_dylib(&root)).unwrap(),
        CORE_LIB
    );
    assert_eq!(
        std::fs::read(assets::bundled_cuda_provider(&root)).unwrap(),
        CUDA_LIB
    );
    let rows = fixture.rows_for(local_request(voice_me_core::SpeechExecutionTarget::Cuda));
    assert!(
        rows.iter()
            .filter(|row| row.kind != DependencyKind::ModelWeights
                && row.kind != DependencyKind::BackendCapability
                && row.kind != DependencyKind::VirtualMicrophone)
            .all(|row| !row.status.is_missing()),
        "{rows:?}"
    );
}

/// Matrix row "Piper on CUDA": Install on Piper's CUDA provider row fetches
/// the core, the provider and the NVIDIA libraries, exactly as for
/// Chatterbox — and never the model files.
#[test]
fn a_piper_cuda_install_fetches_the_cuda_runtime_pieces() {
    let fixture = all_provider_fixture();
    let target = voice_me_core::SpeechExecutionTarget::Cuda;
    let request = voice_me_core::CheckRequest {
        backend: voice_me_core::SpeechBackend::for_target(target),
        selection: voice_me_core::BackendSelection::Piper { target },
        ..voice_me_core::CheckRequest::cpu()
    };

    let (result, _) = fixture.provision_for(DependencyKind::CudaProvider, request);

    assert_eq!(result, Ok(()));
    let root = fixture.root();
    assert_eq!(
        std::fs::read(assets::bundled_runtime_dylib(root)).unwrap(),
        CORE_LIB
    );
    assert_eq!(
        std::fs::read(assets::bundled_cuda_provider(root)).unwrap(),
        CUDA_LIB
    );
    assert!(
        assets::cuda_libraries_dir(root)
            .join("libcudnn.so.9")
            .exists()
    );
    for path in assets::required_model_files(root, voice_me_core::SpeechWeights::Fp16) {
        assert!(!path.exists(), "{}", path.display());
    }
}
