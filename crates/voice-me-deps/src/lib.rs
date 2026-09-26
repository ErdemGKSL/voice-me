//! `voice-me-deps` — the `DependencyProvisioningPort` adapter.
//!
//! Two halves. **Detection** (Story 3.1) reads the filesystem and the
//! environment, builds one [`DependencyReport`] for the selected backend,
//! sends it on the shared `AppEvent` channel (AD-3) and holds nothing
//! afterwards. **Provisioning** (Story 3.2) fetches what a row lacks into
//! the deps-owned cache: resumable, checked against pinned SHA-256s, and
//! reported on the same channel. This is the only crate in the workspace
//! that opens a network connection (AD-8), and it does so with plain HTTPS
//! GETs — no Hugging Face Hub client.
//!
//! Two rules shape everything below:
//!
//! * **Backend-relative.** The row list — and the download plan — is
//!   derived from the selected [`SpeechBackend`], never from a fixed list:
//!   a CPU selection reports and fetches no GPU provider library and no
//!   FP16 weights, and FP16 files sitting in the cache do not satisfy a Q4
//!   selection.
//! * **One list.** The model filenames come from `voice-me-core::assets`,
//!   the same module `voice-me-tts`'s `ModelCache` reads, so the check, the
//!   provisioner and the engine cannot disagree about what "provisioned"
//!   means. `deps` never depends on `tts`.

pub mod capability;
mod msi_unpack;
pub mod piper;
pub mod provision;
#[cfg(test)]
mod provision_tests;
pub mod sources;
#[cfg(test)]
mod test_support;

use std::collections::HashSet;
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex};

use voice_me_core::{
    AppEvent, AppEventSender, BackendSelection, CatalogResult, CheckRequest, Dependency,
    DependencyKind, DependencyProvisioningPort, DependencyReport, PiperCatalogEntry,
    PiperCatalogPort, RemoteProvider, SpeechBackend, SpeechExecutionTarget, SpeechWeights,
    VoiceMeError, assets,
};

use crate::capability::{GpuProbe, SystemGpuProbe, SystemVoiceProbe};

use crate::piper::{CatalogVoice, PiperSources};
use crate::provision::{ProgressReporter, ProgressTarget};
use crate::sources::{PlannedDownload, RuntimeArchive, Sources};

/// `DependencyProvisioningPort` adapter.
///
/// Detection is stateless: every check re-reads the world, because the
/// whole point of "Check again" is that the answer changed since last time.
/// The only thing held is which rows are being provisioned right now, so a
/// second Install on the same row cannot start a second writer on the same
/// `.part` file. Clones share that set.
#[derive(Clone)]
pub struct DepsAdapter {
    sources: Arc<Sources>,
    in_flight: Arc<Mutex<HashSet<DependencyKind>>>,
    /// Held for the whole of a runtime install (Story 3.8). Install on the
    /// runtime, CUDA provider and NVIDIA rows fetches the same set of
    /// files, so a second one waits for the first and then finds nothing
    /// left to do, rather than writing to the same `.part` files.
    runtime_install: Arc<Mutex<()>>,
    /// Whether this process has the bundled runtime loaded (Story 3.8),
    /// asked of the speech engine by the composition root.
    runtime_in_use: Arc<dyn Fn() -> bool + Send + Sync>,
    /// Whether a runtime update waits in `runtime/staged/` while the
    /// runtime is in use: Windows, which cannot replace a loaded library.
    stage_when_in_use: bool,
    /// What Install on the Virtual Microphone row runs: the Linux audio
    /// crate's own `install()`, or on Windows VB-CABLE's setup from the
    /// unpacked driver pack (Story 2.8). Injectable so the row's
    /// provisioning path is testable without an audio server or a UAC
    /// prompt.
    virtual_mic_installer: Arc<VirtualMicInstaller>,
    /// What the capability row asks the hardware (Story 3.3). The real
    /// driver and Vulkan probes in the app; injectable so every capability
    /// row is testable with no GPU.
    gpu_probe: Arc<dyn GpuProbe>,
    /// Where the Piper catalogs are fetched from (Story 3.15).
    piper_sources: Arc<PiperSources>,
    /// The voices the last catalog fetch listed: what makes a voice other
    /// than the built-in default one voice-me knows how to install. Held
    /// only for this process, and only filled when the tab asked.
    piper_catalog: Arc<Mutex<Vec<CatalogVoice>>>,
    /// The voices being downloaded from the Piper voices tab right now.
    piper_in_flight: Arc<Mutex<HashSet<String>>>,
    /// What unpacks eSpeak NG's MSI (Story 3.16).
    espeak_unpacker: Arc<EspeakUnpacker>,
    /// Whether Windows' speech engine answers, and with how many voices
    /// (Story 3.13): what the Windows System voice row reports. Injected
    /// by the composition root, which asks the System voice's own crate.
    system_voice_probe: Arc<SystemVoiceProbe>,
}

/// Install on the Virtual Microphone row. Given the directory the driver
/// pack was unpacked into when one was downloaded (Windows: VB-CABLE's
/// pack, Story 2.8); `None` when the row downloads nothing (Linux, which
/// ignores it).
type VirtualMicInstaller = dyn Fn(Option<&Path>) -> Result<(), VoiceMeError> + Send + Sync;

/// What Install on the Windows eSpeak NG row runs once the MSI is verified
/// (Story 3.16): unpack `msi` into `target`, a directory that does not
/// exist yet. voice-me's own MSI reader ([`msi_unpack`]); injectable so the
/// whole flow is testable without a package.
type EspeakUnpacker = dyn Fn(&Path, &Path) -> Result<(), VoiceMeError> + Send + Sync;

impl std::fmt::Debug for DepsAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DepsAdapter")
            .field("sources", &self.sources)
            .field("in_flight", &self.in_flight)
            .finish_non_exhaustive()
    }
}

impl Default for DepsAdapter {
    fn default() -> Self {
        Self::with_sources(Sources::default())
    }
}

impl DepsAdapter {
    /// The adapter over the real, pinned sources.
    pub fn new() -> Self {
        Self::default()
    }

    /// The adapter over an injected source table — how the tests point
    /// every download at an in-process server.
    pub fn with_sources(sources: Sources) -> Self {
        Self {
            sources: Arc::new(sources),
            in_flight: Arc::default(),
            runtime_install: Arc::default(),
            runtime_in_use: Arc::new(|| false),
            stage_when_in_use: cfg!(target_os = "windows"),
            virtual_mic_installer: Arc::new(install_virtual_microphone),
            gpu_probe: Arc::new(SystemGpuProbe),
            piper_sources: Arc::new(PiperSources::pinned()),
            piper_catalog: Arc::default(),
            piper_in_flight: Arc::default(),
            espeak_unpacker: Arc::new(msi_unpack::unpack),
            system_voice_probe: Arc::new(|| Err(capability::NO_SYSTEM_VOICE_PROBE.to_string())),
        }
    }

    /// Replace where the Piper catalogs are fetched from.
    pub fn with_piper_sources(mut self, sources: PiperSources) -> Self {
        self.piper_sources = Arc::new(sources);
        self
    }

    /// The voice `key`, if voice-me knows where to download it: the
    /// built-in default, or one the last catalog fetch listed.
    fn known_piper_voice(&self, key: &str) -> Option<CatalogVoice> {
        let default = piper::default_voice();
        if key == default.entry.key {
            return Some(default);
        }
        self.piper_catalog
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find(|voice| voice.entry.key == key)
            .cloned()
    }

    /// Story 3.15: Install on the Piper voice row. With nothing installed
    /// that is the default voice; otherwise the voice the request names.
    fn provision_piper_voice(
        &self,
        request: &CheckRequest,
        events: &AppEventSender,
    ) -> Result<(), VoiceMeError> {
        let root = assets::model_cache_root()?;
        let key = request
            .piper_voice
            .clone()
            .unwrap_or_else(|| assets::PIPER_DEFAULT_VOICE.key.to_string());
        let voice = self.known_piper_voice(&key).ok_or_else(|| {
            VoiceMeError::Other(format!(
                "voice-me does not know where to download {key}; open Settings → Piper voices \
                 to find it again."
            ))
        })?;
        piper::install_voice(
            &root,
            &voice,
            &self.piper_sources,
            ProgressTarget::Row(DependencyKind::PiperVoice),
            events,
        )
    }

    /// Replace what unpacks eSpeak NG's MSI on Install (Story 3.16).
    pub fn with_espeak_unpacker(
        mut self,
        unpacker: impl Fn(&Path, &Path) -> Result<(), VoiceMeError> + Send + Sync + 'static,
    ) -> Self {
        self.espeak_unpacker = Arc::new(unpacker);
        self
    }

    /// Replace what the Windows System voice row asks Windows' speech
    /// engine (Story 3.13): how many voices it lists, or why it cannot be
    /// reached. Only Windows asks it.
    pub fn with_system_voice_probe(
        mut self,
        probe: impl Fn() -> Result<usize, String> + Send + Sync + 'static,
    ) -> Self {
        self.system_voice_probe = Arc::new(probe);
        self
    }

    /// Replace what the capability row asks the hardware.
    pub fn with_gpu_probe(mut self, probe: impl GpuProbe + 'static) -> Self {
        self.gpu_probe = Arc::new(probe);
        self
    }

    /// Replace what Install on the Virtual Microphone row runs.
    pub fn with_virtual_mic_installer(
        mut self,
        installer: impl Fn(Option<&Path>) -> Result<(), VoiceMeError> + Send + Sync + 'static,
    ) -> Self {
        self.virtual_mic_installer = Arc::new(installer);
        self
    }

    fn provision_row(
        &self,
        kind: DependencyKind,
        request: &CheckRequest,
        events: &AppEventSender,
    ) -> Result<(), VoiceMeError> {
        let backend = request.backend;
        match kind {
            DependencyKind::ModelWeights => {
                let root = assets::model_cache_root()?;
                let plan = self.sources.model_plan(&root, backend.weights)?;
                fetch(kind, &plan, events)
            }
            // Story 3.8: one Install on any of the runtime's rows fetches
            // everything the selected target still needs — Piper's too, on
            // its own device (spec-backend-engine-and-device-selects).
            DependencyKind::OnnxRuntime
            | DependencyKind::CudaProvider
            | DependencyKind::NvidiaLibraries => self.provision_runtime(kind, backend, events),
            DependencyKind::VirtualMicrophone => self.provision_virtual_mic(events),
            DependencyKind::PiperVoice => self.provision_piper_voice(request, events),
            // Story 3.16: where eSpeak NG has a pinned download (Windows
            // x64), Install unpacks the official MSI into the cache.
            DependencyKind::SystemVoiceEngine if self.sources.espeak.is_some() => {
                self.provision_espeak(events)
            }
            // A system package: its row has manual steps, never Install.
            DependencyKind::SystemVoiceEngine => Err(VoiceMeError::Other(
                "voice-me cannot install eSpeak NG; follow the steps on the row.".to_string(),
            )),
            // Story 3.17: voice-me never runs pip; the row has steps only.
            DependencyKind::EdgeTtsProgram => Err(VoiceMeError::Other(
                "voice-me cannot install edge-tts; follow the steps on the row.".to_string(),
            )),
            // Nothing to fetch: a backend that cannot run here is fixed by
            // choosing another one, which the row's own action does.
            DependencyKind::BackendCapability => Err(VoiceMeError::Other(
                "Nothing can be installed to make this backend run here; choose the CPU \
                 backend or another one instead."
                    .to_string(),
            )),
        }
    }

    /// Story 3.16: Install on the Windows eSpeak NG row. The pinned MSI is
    /// fetched with progress on the row (a verified one already on disk is
    /// reused), unpacked into `<cache>/espeak-ng.tmp`, checked for
    /// `eSpeak NG/espeak-ng.exe`, and only then swapped into
    /// `<cache>/espeak-ng`. On any failure nothing is left in either
    /// directory; on success the MSI is deleted.
    fn provision_espeak(&self, events: &AppEventSender) -> Result<(), VoiceMeError> {
        let Some(msi_asset) = self.sources.espeak.as_ref() else {
            return Err(VoiceMeError::Other(
                "voice-me cannot install eSpeak NG on this system; follow the steps on the row."
                    .to_string(),
            ));
        };
        let root = assets::model_cache_root()?;
        let target = assets::espeak_dir(&root);
        let msi = msi_asset.destination(&root);
        if assets::espeak_program(&target).is_file() {
            // A package left by an earlier run is of no use any more.
            let _ = std::fs::remove_file(&msi);
            return Ok(());
        }

        // A package already on disk is reused only when it is the pinned
        // one; anything else (an older pin, a truncated copy) goes.
        if msi.exists() && !provision::is_verified(msi_asset, &msi) {
            let _ = std::fs::remove_file(&msi);
        }
        if !msi.exists() {
            let plan = [PlannedDownload {
                asset: msi_asset.clone(),
                destination: msi.clone(),
            }];
            fetch(DependencyKind::SystemVoiceEngine, &plan, events)?;
        }

        let staging = root.join(ESPEAK_STAGING_DIR);
        remove_dir_if_present(&staging)?;
        let unpacked = (self.espeak_unpacker)(&msi, &staging).and_then(|()| {
            if assets::espeak_program(&staging).is_file() {
                Ok(())
            } else {
                Err(VoiceMeError::Other(format!(
                    "The eSpeak NG package unpacked without {}; nothing was installed.",
                    assets::espeak_program(Path::new("")).display()
                )))
            }
        });
        if let Err(error) = unpacked {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
        // A directory without the program is a stale leftover, not an
        // install: it gives way to the complete one.
        let swapped = remove_dir_if_present(&target).and_then(|()| {
            std::fs::rename(&staging, &target).map_err(|error| {
                VoiceMeError::Other(format!(
                    "Could not move eSpeak NG into {}: {error}",
                    target.display()
                ))
            })
        });
        if let Err(error) = swapped {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
        // Our own download, of no further use once eSpeak NG is out of it.
        let _ = std::fs::remove_file(&msi);
        Ok(())
    }

    /// Install on the Virtual Microphone row.
    ///
    /// With no driver pack to fetch (Linux) the installer runs straight
    /// away. On Windows (Story 2.8) VB-CABLE's pinned pack is downloaded
    /// with progress and verified like every download, unpacked into
    /// `<cache>/vb-cable/<pack>/`, the zip deleted, and then VB's own setup
    /// runs behind Windows' administrator prompt. A setup that finished
    /// leaves a marker, so the row can tell "not installed" from "installed,
    /// waiting for a restart".
    fn provision_virtual_mic(&self, events: &AppEventSender) -> Result<(), VoiceMeError> {
        let kind = DependencyKind::VirtualMicrophone;
        // No bytes to count while the installer runs; the row says
        // "installing" with no figure until it finishes.
        let installing = || {
            let _ = events.unbounded_send(AppEvent::ProvisioningProgress {
                kind,
                done_bytes: 0,
                total_bytes: 0,
            });
        };
        let Some(pack) = self.sources.virtual_mic.as_ref() else {
            installing();
            return (self.virtual_mic_installer)(None);
        };

        let root = assets::model_cache_root()?;
        let archive = pack.destination(&root);
        // A verified pack kept by a run whose extraction or setup failed
        // is not fetched again.
        if !archive.exists() {
            let plan = [PlannedDownload {
                asset: pack.clone(),
                destination: archive.clone(),
            }];
            fetch(kind, &plan, events)?;
        }
        let unpacked = archive.with_extension("");
        provision::extract_zip_into_dir(&archive, &unpacked)?;
        // A marker from an earlier attempt says nothing about this one.
        let marker = unpacked.parent().map(|dir| dir.join(VB_CABLE_SETUP_RAN));
        if let Some(marker) = &marker {
            let _ = std::fs::remove_file(marker);
        }

        installing();
        (self.virtual_mic_installer)(Some(unpacked.as_path()))?;
        // Our own download, kept until the setup succeeded so a retry
        // after a declined prompt re-extracts it instead of fetching.
        let _ = std::fs::remove_file(&archive);
        if let Some(marker) = &marker {
            let _ = std::fs::write(marker, b"");
        }
        Ok(())
    }

    /// Decision 1: the runtime installs automatically on Linux x64 and
    /// Windows x64 only, and only into the cache — never over a path
    /// `ORT_DYLIB_PATH` names.
    ///
    /// Story 3.8: everything `backend` still needs, in one download with
    /// one progress figure on `kind`'s row — the core runtime (also when an
    /// older or CPU-only one is in the way of a GPU backend), and for CUDA
    /// the CUDA provider and NVIDIA's libraries.
    fn provision_runtime(
        &self,
        kind: DependencyKind,
        backend: SpeechBackend,
        events: &AppEventSender,
    ) -> Result<(), VoiceMeError> {
        let root = assets::model_cache_root()?;
        // Only the bundled runtime is ever installed: a library the user
        // added is theirs, and its row is manual when it goes missing.
        let resolved = assets::resolve_runtime_dylib(&root, None);
        if resolved.configured {
            return Err(VoiceMeError::Other(format!(
                "{} is set to {}; voice-me does not replace a runtime you configured. Unset it to \
                 let Install place one in the cache.",
                assets::RUNTIME_DYLIB_ENV,
                resolved.path.display()
            )));
        }
        if !runtime_installable(&self.sources, backend.target) {
            // Nothing Install could fetch — which is fine when nothing is
            // missing.
            if resolved.path.exists() {
                return Ok(());
            }
            return Err(VoiceMeError::Other(
                if backend.target != SpeechExecutionTarget::Cpu {
                    gpu_runtime_unavailable(backend)
                } else {
                    "voice-me cannot install ONNX Runtime on this system; follow the steps on \
                     the row."
                        .to_string()
                },
            ));
        }

        let _one_at_a_time = self
            .runtime_install
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let pieces = runtime_pieces(&root, &self.sources, backend.target);
        if pieces.is_empty() {
            return Ok(());
        }

        // Libraries from a wheel that is no longer pinned go before any new
        // one arrives (the wheels themselves download into that folder).
        let cuda_dir = assets::cuda_libraries_dir(&root);
        if pieces
            .iter()
            .any(|piece| piece.role == PieceRole::NvidiaWheel)
            && nvidia_libraries_are_stale(&root, &self.sources)
        {
            remove_dir_if_present(&cuda_dir)?;
        }

        // A verified archive left by a run whose extraction failed is not
        // fetched again; one that no longer matches its pin (the pin
        // changed since) is.
        let mut plan = Vec::new();
        for piece in &pieces {
            let destination = piece.source.archive.destination(&root);
            if destination.exists() && !provision::is_verified(&piece.source.archive, &destination)
            {
                let _ = std::fs::remove_file(&destination);
            }
            if !destination.exists() {
                plan.push(PlannedDownload {
                    asset: piece.source.archive.clone(),
                    destination,
                });
            }
        }
        fetch(kind, &plan, events)?;

        // Windows cannot replace a library this process has loaded: the
        // new core and provider wait in `runtime/staged/` for the next start.
        let stage = self.stage_when_in_use && (self.runtime_in_use)();
        let staged = assets::staged_runtime_dir(&root);
        if stage {
            remove_dir_if_present(&staged)?;
        }
        // The core's record is written after the last runtime library of
        // the set (the core, or its CUDA provider): in `staged/` its
        // presence marks a complete set.
        let core_installed = pieces.iter().any(|piece| piece.role == PieceRole::Core);
        let last_runtime = pieces
            .iter()
            .rposition(|piece| piece.role != PieceRole::NvidiaWheel);
        for (index, piece) in pieces.iter().enumerate() {
            let archive = piece.source.archive.destination(&root);
            if piece.role == PieceRole::NvidiaWheel {
                provision::extract_libraries(&archive, &piece.source.library_entries, &cuda_dir)?;
                let files: Vec<&str> = piece
                    .source
                    .library_entries
                    .iter()
                    .map(|entry| entry.file_name.as_str())
                    .collect();
                append_line(
                    &assets::cuda_libraries_source_file(&root),
                    &assets::cuda_marker_line(&archive_stamp(piece.source), &files),
                )?;
            } else {
                let dir = if stage {
                    staged.clone()
                } else {
                    assets::runtime_dir(&root)
                };
                provision::extract_libraries(&archive, &piece.source.library_entries, &dir)?;
                if piece.role == PieceRole::Core && !stage {
                    // A CUDA provider belongs to the core it was built
                    // with; it goes only once the new core is in place.
                    let _ = std::fs::remove_file(assets::bundled_cuda_provider(&root));
                }
                if core_installed
                    && Some(index) == last_runtime
                    && let Some(core) = self.sources.runtime.as_ref()
                {
                    let marker = dir.join(assets::RUNTIME_SOURCE_FILE);
                    std::fs::write(&marker, archive_stamp(core)).map_err(|error| {
                        VoiceMeError::Other(format!(
                            "Could not write {}: {error}",
                            marker.display()
                        ))
                    })?;
                }
            }
            // Our own download, of no further use once the libraries are
            // out of it.
            let _ = std::fs::remove_file(&archive);
        }
        Ok(())
    }

    /// Replace whether this process has the bundled runtime loaded (Story
    /// 3.8): the composition root asks the speech engine.
    pub fn with_runtime_in_use(
        mut self,
        in_use: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        self.runtime_in_use = Arc::new(in_use);
        self
    }

    /// Stage runtime updates whenever the runtime is in use, whatever the
    /// OS — how the Windows path is tested anywhere.
    #[cfg(test)]
    fn staging_when_in_use(mut self) -> Self {
        self.stage_when_in_use = true;
        self
    }
}

/// Append `line` to `file`, creating it.
fn append_line(file: &Path, line: &str) -> Result<(), VoiceMeError> {
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .and_then(|mut out| out.write_all(line.as_bytes()))
        .map_err(|error| {
            VoiceMeError::Other(format!("Could not write {}: {error}", file.display()))
        })
}

/// What an archive is, for the records Install keeps: its release URL
/// (which names the tag) and its pinned SHA-256. A re-pin under the same
/// file names still reads as a different runtime.
fn archive_stamp(archive: &RuntimeArchive) -> String {
    format!(
        "{} sha256:{}",
        archive.archive.url,
        archive.archive.digest.expected()
    )
}

/// What a runtime install extracts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PieceRole {
    Core,
    CudaProvider,
    NvidiaWheel,
}

/// One archive a runtime install extracts.
struct RuntimePiece<'a> {
    source: &'a RuntimeArchive,
    role: PieceRole,
}

/// Whether Install can provide the bundled runtime `target` needs: any
/// pinned runtime for CPU, voice-me's own build for WebGPU, and that build
/// with its CUDA provider and the NVIDIA wheels for CUDA (Story 3.8).
fn runtime_installable(sources: &Sources, target: SpeechExecutionTarget) -> bool {
    match target {
        SpeechExecutionTarget::Cpu => sources.runtime.is_some(),
        SpeechExecutionTarget::WebGpu => sources.runtime.is_some() && sources.runtime_all_providers,
        SpeechExecutionTarget::Cuda => sources.runtime.is_some() && sources.cuda_installable(),
    }
}

/// Whether the bundled runtime in `root` is not the one `sources` pins
/// (Story 3.8): a runtime installed before voice-me's own build was pinned
/// — Microsoft's CPU-only one, which records nothing — or from an older
/// release or pin. Only voice-me's own build is ever judged stale; until
/// it is pinned, every runtime is the right one.
fn runtime_is_stale(root: &Path, sources: &Sources) -> bool {
    let Some(runtime) = sources
        .runtime
        .as_ref()
        .filter(|_| sources.runtime_all_providers)
    else {
        return false;
    };
    let installed = std::fs::read_to_string(assets::runtime_source_file(root)).unwrap_or_default();
    installed.trim() != archive_stamp(runtime)
}

/// Whether the bundled runtime is missing a library its source ships. A
/// stale runtime is judged on its main library alone — it is replaced
/// whole, and the CPU backend keeps using it meanwhile.
fn core_incomplete(root: &Path, sources: &Sources) -> bool {
    if !assets::bundled_runtime_dylib(root).exists() {
        return true;
    }
    match sources.runtime.as_ref() {
        Some(core) if !runtime_is_stale(root, sources) => core
            .library_entries
            .iter()
            .any(|entry| !assets::runtime_dir(root).join(&entry.file_name).exists()),
        _ => false,
    }
}

/// Whether `runtime/cuda/` holds libraries from a wheel that is not pinned
/// any more (the sonames do not change between versions, so only the
/// record tells).
fn nvidia_libraries_are_stale(root: &Path, sources: &Sources) -> bool {
    let pinned: Vec<String> = sources.nvidia_wheels.iter().map(archive_stamp).collect();
    assets::installed_cuda_wheels(root)
        .iter()
        .any(|(stamp, _)| !pinned.contains(stamp))
}

/// Whether one pinned NVIDIA wheel is installed: recorded under its
/// current pin, with every library it names present.
fn nvidia_wheel_installed(root: &Path, sources: &Sources, wheel: &RuntimeArchive) -> bool {
    if nvidia_libraries_are_stale(root, sources) {
        return false;
    }
    let stamp = archive_stamp(wheel);
    let dir = assets::cuda_libraries_dir(root);
    assets::installed_cuda_wheels(root)
        .iter()
        .any(|(recorded, _)| *recorded == stamp)
        && wheel
            .library_entries
            .iter()
            .all(|entry| dir.join(&entry.file_name).exists())
}

/// Everything the bundled runtime still lacks for `target`, in install
/// order. Only what [`runtime_installable`] allows is ever listed; a core
/// and provider already staged for the next start are not listed again.
fn runtime_pieces<'a>(
    root: &Path,
    sources: &'a Sources,
    target: SpeechExecutionTarget,
) -> Vec<RuntimePiece<'a>> {
    let mut pieces = Vec::new();
    let Some(core) = sources.runtime.as_ref() else {
        return pieces;
    };
    let staged = staged_runtime_pending(root);
    let gpu = target != SpeechExecutionTarget::Cpu;
    let core_needed =
        !staged && (core_incomplete(root, sources) || (gpu && runtime_is_stale(root, sources)));
    if core_needed {
        pieces.push(RuntimePiece {
            source: core,
            role: PieceRole::Core,
        });
    }
    if target != SpeechExecutionTarget::Cuda || !runtime_installable(sources, target) {
        return pieces;
    }
    if let Some(provider) = sources.cuda_runtime.as_ref()
        && !staged
        && (core_needed || !assets::bundled_cuda_provider(root).exists())
    {
        pieces.push(RuntimePiece {
            source: provider,
            role: PieceRole::CudaProvider,
        });
    }
    for wheel in &sources.nvidia_wheels {
        if !nvidia_wheel_installed(root, sources, wheel) {
            pieces.push(RuntimePiece {
                source: wheel,
                role: PieceRole::NvidiaWheel,
            });
        }
    }
    pieces
}

/// Story 3.8 (Windows): whether a complete runtime update waits in
/// `runtime/staged/` for the next start. Its record is written last, so it
/// marks a complete set.
pub fn staged_runtime_pending(root: &Path) -> bool {
    assets::staged_runtime_dir(root)
        .join(assets::RUNTIME_SOURCE_FILE)
        .is_file()
}

/// Story 3.8 (Windows): move a runtime update staged by an earlier run into
/// `runtime/`. Called first thing at startup, before anything resolves or
/// loads the runtime. `Ok(true)` when one was applied.
///
/// Only a complete set moves (its record last); an incomplete one is
/// removed, and Install stages it again. A staged core without a CUDA
/// provider takes the old provider away — it belongs to the old core.
pub fn apply_staged_runtime(root: &Path) -> Result<bool, VoiceMeError> {
    let staged = assets::staged_runtime_dir(root);
    if !staged.is_dir() {
        return Ok(false);
    }
    if !staged_runtime_pending(root) {
        remove_dir_if_present(&staged)?;
        return Ok(false);
    }
    let runtime = assets::runtime_dir(root);
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&staged)
        .map_err(|error| {
            VoiceMeError::Other(format!("Could not read {}: {error}", staged.display()))
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    // The record goes last, so an interrupted move is applied again.
    files.sort_by_key(|path| {
        path.file_name()
            .is_some_and(|name| name == assets::RUNTIME_SOURCE_FILE)
    });
    if !files.iter().any(|path| {
        path.file_name() == Some(std::ffi::OsStr::new(assets::cuda_provider_file_name()))
    }) {
        let _ = std::fs::remove_file(assets::bundled_cuda_provider(root));
    }
    for file in files {
        let Some(name) = file.file_name() else {
            continue;
        };
        let destination = runtime.join(name);
        std::fs::rename(&file, &destination).map_err(|error| {
            VoiceMeError::Other(format!(
                "Could not move {} into {}: {error}",
                file.display(),
                runtime.display()
            ))
        })?;
    }
    remove_dir_if_present(&staged)?;
    Ok(true)
}

impl DependencyProvisioningPort for DepsAdapter {
    fn check(&self, request: CheckRequest, events: AppEventSender) -> Result<(), VoiceMeError> {
        // The one failure that is not a report: with no cache directory to
        // resolve there is no path to state anything about, so the check
        // itself failed and the Dependencies tab says so with this reason.
        let root = assets::model_cache_root()?;

        // Story 3.3: whether the selection can run here at all comes first
        // — it is the row that explains every other one. A remote
        // selection has no file list (its readiness is a key and a
        // provider), so it reports no engine rows.
        let capability = capability::capability_row(&request, self.gpu_probe.as_ref());
        let engine_rows = match &request.selection {
            BackendSelection::Local { runtime, .. } => {
                speech_engine_rows(&root, request.backend, runtime.as_deref(), &self.sources)
            }
            // Story 3.12: the System voice's one engine row. It is not an
            // ONNX target, so it has no runtime or model rows.
            BackendSelection::SystemVoice => self.system_voice_engine_rows(),
            // Story 3.15: Piper's shared runtime (with its device's CUDA
            // pieces), its voice, and the eSpeak NG it reads text through.
            BackendSelection::Piper { .. } => self.piper_rows(&root, &request),
            // Story 3.17: Edge TTS's one program row. Its key-free
            // readiness is whether `edge-tts` is found.
            BackendSelection::Remote(RemoteProvider::EdgeTts) => edge_tts_rows(),
            BackendSelection::Remote(_) => Vec::new(),
        };

        let report = DependencyReport::new(
            request.backend,
            capability
                .into_iter()
                .chain(engine_rows)
                .chain(virtual_microphone_row(&root))
                .collect(),
        );

        // A closed receiver means the app is shutting down; there is
        // nowhere to report that to, and nothing this adapter could do
        // about it.
        let _ = events.unbounded_send(AppEvent::DependencyCheckCompleted { report });
        Ok(())
    }

    fn provision(
        &self,
        kind: DependencyKind,
        request: CheckRequest,
        events: AppEventSender,
    ) -> Result<(), VoiceMeError> {
        // A second Install on a row already being installed: the first run
        // stays the only one reporting, and nothing is sent for this one.
        let Some(claim) = InFlight::claim(&self.in_flight, kind) else {
            return Ok(());
        };

        let result = self.provision_row(kind, &request, &events);
        // Released before the event goes out: once the row reads "missing"
        // again, the next Install must not be swallowed as a duplicate.
        drop(claim);
        let _ = events.unbounded_send(AppEvent::ProvisioningFinished {
            kind,
            result: result.as_ref().map(|_| ()).map_err(ToString::to_string),
        });
        result
    }
}

impl DepsAdapter {
    /// The System voice's one row: eSpeak NG on Linux (Story 3.12).
    #[cfg(not(target_os = "windows"))]
    fn system_voice_engine_rows(&self) -> Vec<Dependency> {
        system_voice_rows("The System voice speaks through it.")
    }

    /// The System voice's one row on Windows (Story 3.13): Windows speech,
    /// asked through the injected probe — ready, or decision 3's manual,
    /// blocking capability row. Never the eSpeak NG row.
    #[cfg(target_os = "windows")]
    fn system_voice_engine_rows(&self) -> Vec<Dependency> {
        vec![capability::windows_system_voice_row((self
            .system_voice_probe)(
        ))]
    }

    /// Story 3.15: Piper's rows — the shared bundled runtime (the one
    /// Chatterbox uses; Piper never downloads its own) with, on CUDA, the
    /// CUDA provider and NVIDIA libraries (spec-backend-engine-and-device-
    /// selects), then the voice and eSpeak NG. Never the model files.
    /// Linux and (Story 3.16) Windows only: elsewhere the capability row
    /// says Piper arrives later.
    fn piper_rows(&self, root: &Path, request: &CheckRequest) -> Vec<Dependency> {
        if !cfg!(any(target_os = "linux", target_os = "windows")) {
            return Vec::new();
        }
        let known = |key: &str| self.known_piper_voice(key).is_some();
        let mut rows = runtime_rows(root, request.backend, None, &self.sources);
        rows.push(piper::piper_voice_row(
            root,
            request.piper_voice.as_deref(),
            &known,
        ));
        rows.extend(self.piper_espeak_rows());
        rows
    }

    /// Piper's eSpeak NG row: the System voice's row on Linux.
    #[cfg(not(target_os = "windows"))]
    fn piper_espeak_rows(&self) -> Vec<Dependency> {
        system_voice_rows("Piper reads text through it.")
    }

    /// Piper's eSpeak NG row on Windows (Story 3.16): found in the cache,
    /// under Program Files or on PATH, or missing with Install where the
    /// MSI is pinned. Used for Piper only — the Windows System voice speaks
    /// through Windows' own engine and has no eSpeak NG row.
    #[cfg(target_os = "windows")]
    fn piper_espeak_rows(&self) -> Vec<Dependency> {
        vec![capability::windows_espeak_row(
            voice_me_espeak::find_program().as_deref(),
            self.sources.espeak.is_some(),
        )]
    }
}

/// Story 3.15: the Piper voices tab's catalogs, downloads and deletes.
impl PiperCatalogPort for DepsAdapter {
    fn fetch_catalog(&self) -> Vec<CatalogResult> {
        let results = piper::fetch_catalogs(&self.piper_sources);
        let voices: Vec<CatalogVoice> = results
            .iter()
            .filter_map(|(_, result)| result.as_ref().ok())
            .flatten()
            .cloned()
            .collect();
        // A refresh that failed entirely keeps what an earlier one listed:
        // those voices are still where they were.
        if !voices.is_empty() {
            *self
                .piper_catalog
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = voices;
        }
        piper::catalog_results(&results)
    }

    fn install(
        &self,
        entry: &PiperCatalogEntry,
        events: AppEventSender,
    ) -> Result<(), VoiceMeError> {
        let key = entry.key.clone();
        // A second Download of a voice already downloading: nothing sent.
        let Some(_claim) = InFlight::claim(&self.piper_in_flight, key.clone()) else {
            return Ok(());
        };
        let result = (|| {
            let root = assets::model_cache_root()?;
            let listed = self
                .piper_catalog
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            let voice = piper::voice_for_entry(&listed, entry).ok_or_else(|| {
                VoiceMeError::Other(format!(
                    "{key} is not in the catalog any more; refresh the list."
                ))
            })?;
            piper::install_voice(
                &root,
                &voice,
                &self.piper_sources,
                ProgressTarget::PiperVoice(key.clone()),
                &events,
            )
        })();
        drop(_claim);
        let _ = events.unbounded_send(AppEvent::PiperVoiceFinished {
            key,
            result: result.as_ref().map(|_| ()).map_err(ToString::to_string),
        });
        result
    }

    fn delete(&self, key: &str) -> Result<(), VoiceMeError> {
        piper::delete_voice(&assets::model_cache_root()?, key)
    }

    fn update_custom_voices(&self, events: AppEventSender) -> Result<Vec<String>, VoiceMeError> {
        let root = assets::model_cache_root()?;
        let listed =
            piper::fetch_voice_me_catalog(&self.piper_sources).map_err(VoiceMeError::Other)?;
        let mut updated = Vec::new();
        for voice in piper::outdated_custom_voices(&root, &listed) {
            let key = voice.entry.key.clone();
            // A voice the tab is downloading right now is being replaced
            // already.
            let Some(_claim) = InFlight::claim(&self.piper_in_flight, key.clone()) else {
                continue;
            };
            let result = piper::install_voice(
                &root,
                &voice,
                &self.piper_sources,
                ProgressTarget::PiperVoice(key.clone()),
                &events,
            );
            drop(_claim);
            if result.is_ok() {
                updated.push(key.clone());
            }
            let _ = events.unbounded_send(AppEvent::PiperVoiceFinished {
                key,
                result: result.map_err(|error| error.to_string()),
            });
        }
        Ok(updated)
    }
}

/// Where eSpeak NG is unpacked first, inside the cache root (Story 3.16);
/// only a complete unpack is renamed to [`assets::ESPEAK_DIR`].
const ESPEAK_STAGING_DIR: &str = "espeak-ng.tmp";

/// Remove `dir` and everything in it, if it is there.
fn remove_dir_if_present(dir: &Path) -> Result<(), VoiceMeError> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(VoiceMeError::Other(format!(
            "Could not remove {}: {error}",
            dir.display()
        ))),
    }
}

/// Removes its row from the in-flight set when dropped — including when the
/// provisioning job panics, so a crashed run never leaves Install dead.
struct InFlight<K: std::hash::Hash + Eq + Clone = DependencyKind> {
    set: Arc<Mutex<HashSet<K>>>,
    kind: K,
}

impl<K: std::hash::Hash + Eq + Clone> InFlight<K> {
    fn claim(set: &Arc<Mutex<HashSet<K>>>, kind: K) -> Option<Self> {
        let mut rows = set.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        rows.insert(kind.clone()).then(|| Self {
            set: set.clone(),
            kind,
        })
    }
}

impl<K: std::hash::Hash + Eq + Clone> Drop for InFlight<K> {
    fn drop(&mut self) {
        self.set
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.kind);
    }
}

/// Download every file in `plan`, one after another, reporting progress
/// for `kind`.
fn fetch(
    kind: DependencyKind,
    plan: &[PlannedDownload],
    events: &AppEventSender,
) -> Result<(), VoiceMeError> {
    fetch_to(ProgressTarget::Row(kind), plan, events)
}

/// [`fetch`], reporting on any target — a Piper voice from its tab, too.
fn fetch_to(
    target: ProgressTarget,
    plan: &[PlannedDownload],
    events: &AppEventSender,
) -> Result<(), VoiceMeError> {
    // Nothing to fetch, nothing to report: the reporter's constructor
    // would otherwise send a meaningless 0-of-0 figure.
    if plan.is_empty() {
        return Ok(());
    }
    let mut progress = ProgressReporter::for_target(target, events.clone(), plan);
    block_on(async {
        let client = provision::client()?;
        for planned in plan {
            provision::download(&client, planned, &mut progress).await?;
        }
        progress.finish();
        Ok(())
    })
}

/// Drive `future` to completion from this blocking thread.
///
/// In the app this runs on Tokio's blocking pool (the AD-5 bridge), so the
/// multi-threaded runtime it belongs to is reused — no second runtime.
/// Anywhere else (tests, or a current-thread runtime whose I/O driver only
/// its own `block_on` can turn) a small runtime is built for the one call.
fn block_on<T>(future: impl Future<Output = Result<T, VoiceMeError>>) -> Result<T, VoiceMeError> {
    use tokio::runtime::{Builder, Handle, RuntimeFlavor};

    match Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == RuntimeFlavor::MultiThread => {
            handle.block_on(future)
        }
        _ => Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(future),
    }
}

/// The rows a missing one of which blocks the Prompt Overlay (Decision 3):
/// the ONNX Runtime library, and the model files the selected weight
/// variant needs.
///
/// Pure apart from `ORT_DYLIB_PATH` and `Path::exists`, and takes the cache
/// root rather than resolving it, so the whole I/O matrix can be driven
/// against temporary directories. `sources` decides only whether the
/// runtime row can offer Install on this target.
///
/// `added` is the runtime library the user added and selected, if any
/// (Story 3.3): it is reported on like any other path.
pub fn speech_engine_rows(
    root: &Path,
    backend: SpeechBackend,
    added: Option<&Path>,
    sources: &Sources,
) -> Vec<Dependency> {
    let mut rows = runtime_rows(root, backend, added, sources);
    rows.push(model_weights_row(root, backend.weights));
    rows
}

/// The runtime rows every ONNX engine on `backend`'s target needs: the
/// runtime library and, for CUDA on the bundled runtime, its CUDA provider
/// and NVIDIA libraries. Chatterbox adds its model files after them; Piper
/// its voice (spec-backend-engine-and-device-selects).
fn runtime_rows(
    root: &Path,
    backend: SpeechBackend,
    added: Option<&Path>,
    sources: &Sources,
) -> Vec<Dependency> {
    let mut rows = vec![runtime_row(root, backend, added, sources)];
    // Story 3.8: the bundled runtime's CUDA pieces, each its own row, once
    // they can be installed. A library the user added or configured is
    // theirs, and is reported exactly as before.
    let bundled = !assets::resolve_runtime_dylib(root, added).configured && added.is_none();
    if bundled
        && backend.target == SpeechExecutionTarget::Cuda
        && runtime_installable(sources, backend.target)
    {
        rows.push(cuda_provider_row(root, sources));
        rows.push(nvidia_libraries_row(root, sources));
    }
    rows
}

const CUDA_PROVIDER_LABEL: &str = "ONNX Runtime CUDA provider";
const NVIDIA_LIBRARIES_LABEL: &str = "NVIDIA CUDA libraries";

/// Story 3.8: the CUDA provider beside the bundled runtime.
fn cuda_provider_row(root: &Path, sources: &Sources) -> Dependency {
    let path = assets::bundled_cuda_provider(root);
    if staged_runtime_pending(root) {
        return Dependency::missing(
            DependencyKind::CudaProvider,
            CUDA_PROVIDER_LABEL,
            "Installed; restart voice-me to finish installing it.".to_string(),
        )
        .manual([
            "Restart voice-me — the CUDA provider is put in place as it starts.",
            "The CPU backend keeps working until then.",
        ]);
    }
    // A provider beside an older core belongs to that core: Install
    // replaces both.
    if path.exists() && !runtime_is_stale(root, sources) {
        return Dependency::ready(
            DependencyKind::CudaProvider,
            CUDA_PROVIDER_LABEL,
            format!("Found at {}.", path.display()),
        );
    }
    Dependency::missing(
        DependencyKind::CudaProvider,
        CUDA_PROVIDER_LABEL,
        format!(
            "Not found at {}. Install fetches it together with everything else the CUDA \
             backend needs.",
            path.display()
        ),
    )
}

/// Story 3.8: NVIDIA's CUDA 12 runtime, cuBLAS, cuFFT and cuDNN 9, in
/// `<cache>/runtime/cuda/`. The detail names the first missing library and
/// how much Install would download.
fn nvidia_libraries_row(root: &Path, sources: &Sources) -> Dependency {
    let dir = assets::cuda_libraries_dir(root);
    let expected: Vec<_> = sources
        .nvidia_wheels
        .iter()
        .flat_map(|wheel| {
            let dir = &dir;
            wheel
                .library_entries
                .iter()
                .map(move |entry| (wheel, dir.join(&entry.file_name)))
        })
        .collect();
    // Libraries recorded from a wheel that is no longer pinned are all
    // replaced, whatever their names say.
    if nvidia_libraries_are_stale(root, sources) {
        let download: u64 = sources
            .nvidia_wheels
            .iter()
            .map(|wheel| wheel.archive.size)
            .sum();
        return Dependency::missing(
            DependencyKind::NvidiaLibraries,
            NVIDIA_LIBRARIES_LABEL,
            format!(
                "The libraries in {} are from an older version. Install replaces them with \
                 NVIDIA's CUDA 12 and cuDNN 9 libraries ({}).",
                dir.display(),
                voice_me_core::format_bytes(download)
            ),
        );
    }
    let missing: Vec<_> = expected
        .iter()
        .filter(|(wheel, path)| !path.exists() || !nvidia_wheel_installed(root, sources, wheel))
        .collect();
    let Some((_, first)) = missing.first() else {
        return Dependency::ready(
            DependencyKind::NvidiaLibraries,
            NVIDIA_LIBRARIES_LABEL,
            format!(
                "All {} libraries present in {}.",
                expected.len(),
                dir.display()
            ),
        );
    };
    let mut wheels: Vec<&RuntimeArchive> = Vec::new();
    for (wheel, _) in &missing {
        if !wheels.contains(wheel) {
            wheels.push(wheel);
        }
    }
    let download: u64 = wheels.iter().map(|wheel| wheel.archive.size).sum();
    Dependency::missing(
        DependencyKind::NvidiaLibraries,
        NVIDIA_LIBRARIES_LABEL,
        format!(
            "Missing {} of {} libraries, starting with {}. Install downloads NVIDIA's CUDA 12 \
             and cuDNN 9 libraries ({}).",
            missing.len(),
            expected.len(),
            first.display(),
            voice_me_core::format_bytes(download)
        ),
    )
}

/// The ONNX Runtime row, reporting on exactly the rule the engine will
/// apply when it loads the library: `ORT_DYLIB_PATH` if set, otherwise the
/// cache root's own copy.
///
/// The missing cases are deliberately different sentences. "You configured
/// a path and it is not there" is a stale setting the user has to fix
/// themselves; "nothing is configured and nothing is in the cache" is a
/// provisioning gap Install fills — where a runtime for this target and
/// backend exists to be installed at all.
fn runtime_row(
    root: &Path,
    backend: SpeechBackend,
    added: Option<&Path>,
    sources: &Sources,
) -> Dependency {
    let resolved = assets::resolve_runtime_dylib(root, added);
    let path = resolved.path.display();
    let bundled = !resolved.added && !resolved.configured;
    let gpu = backend.target != SpeechExecutionTarget::Cpu;

    // Story 3.8 (Windows): the new runtime is installed but waits for the
    // next start, since this process has the old one loaded.
    if bundled && gpu && staged_runtime_pending(root) {
        return Dependency::missing(
            DependencyKind::OnnxRuntime,
            RUNTIME_LABEL,
            "voice-me's build of ONNX Runtime is installed; restart voice-me to finish \
             installing it."
                .to_string(),
        )
        .manual([
            "Restart voice-me — the new runtime is put in place as it starts.",
            "The CPU backend keeps working until then.",
        ]);
    }

    // Story 3.8: an older or CPU-only runtime in the cache cannot run a
    // GPU backend; Install puts voice-me's build in its place. The CPU
    // backend keeps using it — it works.
    if bundled
        && gpu
        && resolved.path.exists()
        && runtime_installable(sources, backend.target)
        && runtime_is_stale(root, sources)
    {
        return Dependency::missing(
            DependencyKind::OnnxRuntime,
            RUNTIME_LABEL,
            format!(
                "Found at {path}, but it is an older or CPU-only build. Install replaces it with \
                 voice-me's build of ONNX Runtime {}, which runs CPU, WebGPU and CUDA.",
                sources::RUNTIME_VERSION
            ),
        );
    }

    // A runtime missing one of the libraries it ships with (the provider
    // bridge, Dawn) is fetched again.
    if bundled && resolved.path.exists() && core_incomplete(root, sources) {
        return Dependency::missing(
            DependencyKind::OnnxRuntime,
            RUNTIME_LABEL,
            format!(
                "Found at {path}, but some of the libraries that come with it are missing. \
                 Install fetches it again."
            ),
        );
    }

    if resolved.path.exists() {
        return Dependency::ready(
            DependencyKind::OnnxRuntime,
            RUNTIME_LABEL,
            // What was actually established is that a file is there — the
            // library has not been loaded, and saying so would be a claim
            // this check never made.
            format!("Found at {path}."),
        );
    }

    if resolved.added {
        return Dependency::missing(
            DependencyKind::OnnxRuntime,
            RUNTIME_LABEL,
            format!("The runtime you added is no longer at {path}."),
        )
        // Nothing can be downloaded to replace a library the user chose.
        .manual([
            "Put the library back at that path, or remove it under Local runtimes and add it \
             again from where it is now."
                .to_string(),
            "Or choose another backend above.".to_string(),
        ]);
    }

    if resolved.configured {
        return Dependency::missing(
            DependencyKind::OnnxRuntime,
            RUNTIME_LABEL,
            format!(
                "{} is set to {path}, and nothing is there. Point it at an ONNX Runtime library, \
                 or unset it to use the one in the cache.",
                assets::RUNTIME_DYLIB_ENV
            ),
        )
        // Nothing can be downloaded to satisfy a path the user chose: the
        // fix is to correct or clear the variable.
        .manual([
            format!(
                "Find where {} is set — your shell profile, or the launcher that starts voice-me.",
                assets::RUNTIME_DYLIB_ENV
            ),
            format!(
                "Point it at an ONNX Runtime library that exists, or remove it so voice-me can \
                 install one into {}.",
                assets::bundled_runtime_dylib(root).display()
            ),
            "Start voice-me again so it reads the change.".to_string(),
        ]);
    }

    // Decision 3 (spec 3-2) and decision 7 (Story 3.8): until voice-me's
    // own build is pinned, a GPU backend's runtime has no source.
    if gpu && !runtime_installable(sources, backend.target) {
        return Dependency::missing(
            DependencyKind::OnnxRuntime,
            RUNTIME_LABEL,
            format!("Not found at {path}. {}", gpu_runtime_unavailable(backend)),
        )
        .manual([
            "The GPU build of ONNX Runtime ships with a later voice-me release.",
            "Until then, the CPU backend works on this machine and voice-me installs its runtime \
             for you.",
        ]);
    }

    let detail = format!(
        "Not found at {path}. {} is unset, so that is where voice-me looks.",
        assets::RUNTIME_DYLIB_ENV
    );
    if runtime_installable(sources, backend.target) {
        Dependency::missing(DependencyKind::OnnxRuntime, RUNTIME_LABEL, detail)
    } else {
        // Decision 1: only Linux x64 and Windows x64 have an automatic
        // runtime install.
        Dependency::missing(DependencyKind::OnnxRuntime, RUNTIME_LABEL, detail).manual([
            format!(
                "Download ONNX Runtime {} for this system from Microsoft's onnxruntime releases.",
                sources::RUNTIME_VERSION
            ),
            format!(
                "Copy its {} into {}.",
                assets::runtime_dylib_file_name(),
                path
            ),
            "Press Check again.".to_string(),
        ])
    }
}

fn gpu_runtime_unavailable(backend: SpeechBackend) -> String {
    format!(
        "The {} backend's runtime is not yet available to install. Add a runtime library \
         that provides it under Local runtimes.",
        backend.target.label()
    )
}

/// The model-files row for the *selected* weight variant.
///
/// One row rather than nine, because the nine files are one artefact set as
/// far as the user is concerned — but the detail still names an exact
/// absolute path, since "some model files are missing" is unactionable and
/// a path is not.
fn model_weights_row(root: &Path, weights: SpeechWeights) -> Dependency {
    let required = assets::required_model_files(root, weights);
    let missing: Vec<_> = required
        .iter()
        .filter(|path| !path.exists())
        .cloned()
        .collect();

    let label = format!("Speech model files ({})", weights_label(weights));

    match missing.first() {
        None => Dependency::ready(
            DependencyKind::ModelWeights,
            label,
            format!(
                "All {} files present in {}.",
                required.len(),
                root.display()
            ),
        ),
        Some(first) => {
            let detail = if missing.len() == 1 {
                format!("Missing: {}", first.display())
            } else {
                format!(
                    "Missing {} of {} files, starting with {}",
                    missing.len(),
                    required.len(),
                    first.display()
                )
            };
            Dependency::missing(DependencyKind::ModelWeights, label, detail)
        }
    }
}

/// How the selected weight variant is named to the user.
fn weights_label(weights: SpeechWeights) -> &'static str {
    weights.label()
}

const RUNTIME_LABEL: &str = "ONNX Runtime";
const VIRTUAL_MIC_LABEL: &str = "Virtual Microphone";

/// Left beside the unpacked VB-CABLE pack once VB's setup has run and
/// returned successfully (Story 2.8).
const VB_CABLE_SETUP_RAN: &str = "setup-ran";

/// The Virtual Microphone row on Linux (Decision 1).
///
/// The status is *asked of* `voice-me-audio-linux` rather than detected
/// again here: that crate already owns every line of PipeWire knowledge in
/// this workspace, and a second copy of it would be a second thing to be
/// wrong.
#[cfg(target_os = "linux")]
fn virtual_microphone_row(_root: &Path) -> Option<Dependency> {
    let row = match voice_me_audio_linux::virtual_microphone_available() {
        Ok(true) => Dependency::ready(
            DependencyKind::VirtualMicrophone,
            VIRTUAL_MIC_LABEL,
            format!(
                "Other applications can select \"{}\" as their microphone.",
                voice_me_audio_linux::DEVICE_DESCRIPTION
            ),
        ),
        // The audio server answered and the device is not there: Install
        // loads it again, no network involved.
        Ok(false) => Dependency::missing(
            DependencyKind::VirtualMicrophone,
            VIRTUAL_MIC_LABEL,
            "The device is not loaded. voice-me installs it at startup; if it keeps disappearing, \
             the audio server may have been restarted."
                .to_string(),
        ),
        // Not a failed check: the audio server being unreachable *is* the
        // answer, and the reason is the useful half of it.
        Err(error) => Dependency::missing(
            DependencyKind::VirtualMicrophone,
            VIRTUAL_MIC_LABEL,
            format!("Could not ask the audio server: {error}"),
        )
        .manual([
            "Make sure PipeWire (with pipewire-pulse) or PulseAudio is running in your session.",
            "Log out and back in if it was just installed or restarted.",
            "Press Check again — Install appears here once the audio server answers.",
        ]),
    };
    Some(row)
}

/// The Virtual Microphone row on Windows (Story 2.8): whether VB-CABLE's
/// "CABLE Input" is present, asked of `voice-me-audio-windows` — the crate
/// that plays to it — rather than detected again here.
#[cfg(target_os = "windows")]
fn virtual_microphone_row(root: &Path) -> Option<Dependency> {
    let marker = root.join(sources::VB_CABLE_DIR).join(VB_CABLE_SETUP_RAN);
    let available = voice_me_audio_windows::virtual_microphone_available();
    // Once the cable is found, the restart hint has done its job: a later
    // uninstall must read as "not installed", not "restart".
    if matches!(available, Ok(true)) {
        let _ = std::fs::remove_file(&marker);
    }
    Some(vb_cable_row(available, marker.exists()))
}

/// Neither Linux nor Windows: no Virtual Microphone voice-me can install.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn virtual_microphone_row(_root: &Path) -> Option<Dependency> {
    None
}

/// VB-CABLE's credit, as its licence asks: it is VB-Audio's donationware.
const VB_CABLE_CREDIT: &str = "VB-CABLE is VB-Audio's donationware (www.vb-cable.com).";

/// The Windows row from what was found: whether "CABLE Input" is present
/// (or why Windows could not be asked), and whether VB's setup has already
/// run from this cache. Pure, so every state is tested on any OS.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn vb_cable_row(available: Result<bool, VoiceMeError>, setup_ran: bool) -> Dependency {
    match available {
        Ok(true) => Dependency::ready(
            DependencyKind::VirtualMicrophone,
            VIRTUAL_MIC_LABEL,
            "Other applications can select CABLE Output as their microphone.",
        ),
        // Setup finished but the driver is not running yet: Windows starts
        // a new audio driver only after a restart.
        Ok(false) if setup_ran => Dependency::missing(
            DependencyKind::VirtualMicrophone,
            VIRTUAL_MIC_LABEL,
            format!(
                "VB-CABLE's setup has run, but Windows has not started the device yet. Restart \
                 Windows, then press Check again. {VB_CABLE_CREDIT}"
            ),
        ),
        Ok(false) => Dependency::missing(
            DependencyKind::VirtualMicrophone,
            VIRTUAL_MIC_LABEL,
            format!(
                "VB-CABLE is not installed. Install downloads it from VB-Audio and runs its setup, \
                 which Windows asks you to allow; voice chat then selects CABLE Output as its \
                 microphone. Or download VB-CABLE yourself from www.vb-cable.com and run \
                 VBCABLE_Setup_x64.exe as administrator. {VB_CABLE_CREDIT}"
            ),
        ),
        Err(error) => Dependency::missing(
            DependencyKind::VirtualMicrophone,
            VIRTUAL_MIC_LABEL,
            format!("Could not list Windows' playback devices: {error}. {VB_CABLE_CREDIT}"),
        ),
    }
}

/// The System voice's engine row (Story 3.12): whether `espeak-ng` is on
/// PATH, asked of the crate that runs it, with the distribution's install
/// command when it is not.
///
/// `used_by` finishes the missing row's sentence: who needs it (Story 3.15:
/// the System voice speaks through it, Piper reads text through it).
#[cfg(target_os = "linux")]
fn system_voice_rows(used_by: &str) -> Vec<Dependency> {
    let found = voice_me_espeak::find_program();
    let install = if found.is_some() {
        String::new()
    } else {
        voice_me_espeak::install_step()
    };
    vec![capability::system_voice_engine_row(
        found.as_deref(),
        &install,
        used_by,
    )]
}

/// Off Linux there is no eSpeak NG row for the System voice: Windows has
/// its own ([`DepsAdapter::system_voice_engine_rows`]), and elsewhere the
/// capability row says it cannot run yet.
#[cfg(not(target_os = "linux"))]
#[cfg_attr(target_os = "windows", allow(dead_code))]
fn system_voice_rows(_used_by: &str) -> Vec<Dependency> {
    Vec::new()
}

/// Edge TTS's program row (Story 3.17): where `edge-tts` is, asked of the
/// crate that runs it (PATH, then `~/.local/bin`), with the distribution's
/// pipx command when it is not found.
#[cfg(target_os = "linux")]
fn edge_tts_rows() -> Vec<Dependency> {
    edge_tts_rows_with(
        voice_me_tts_edge::find_program(),
        &voice_me_espeak::read_os_release(),
    )
}

/// [`edge_tts_rows`] with where the program was found and the contents of
/// `/etc/os-release` given, so both branches are testable on any host.
#[cfg(target_os = "linux")]
fn edge_tts_rows_with(found: Option<std::path::PathBuf>, os_release: &str) -> Vec<Dependency> {
    let steps = if found.is_some() {
        Vec::new()
    } else {
        voice_me_tts_edge::install_steps_for(os_release)
    };
    vec![capability::edge_tts_program_row(found.as_deref(), steps)]
}

/// Off Linux the capability row says Edge TTS cannot run here; there is no
/// program to report on.
#[cfg(not(target_os = "linux"))]
fn edge_tts_rows() -> Vec<Dependency> {
    Vec::new()
}

/// Install reuses the audio crate's own idempotent `install()`: it ends
/// with exactly one device, whatever it started with. Nothing is
/// downloaded on Linux, so there is no unpacked directory to use.
#[cfg(target_os = "linux")]
fn install_virtual_microphone(_unpacked: Option<&Path>) -> Result<(), VoiceMeError> {
    voice_me_audio_linux::LinuxVirtualMicAdapter::new()
        .and_then(|adapter| adapter.install())
        .map(|_| ())
        .map_err(|error| {
            VoiceMeError::Other(format!("Could not install the Virtual Microphone: {error}"))
        })
}

/// Story 2.8: run VB-CABLE's own 64-bit setup from the unpacked pack,
/// behind Windows' administrator prompt. Never silent — the user confirms
/// the prompt and VB's installer both.
#[cfg(target_os = "windows")]
fn install_virtual_microphone(unpacked: Option<&Path>) -> Result<(), VoiceMeError> {
    let unpacked = unpacked.ok_or_else(|| {
        VoiceMeError::Other("VB-CABLE's driver pack was not downloaded.".to_string())
    })?;
    voice_me_audio_windows::run_installer_elevated(
        &unpacked.join(voice_me_audio_windows::SETUP_PROGRAM),
    )
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn install_virtual_microphone(_unpacked: Option<&Path>) -> Result<(), VoiceMeError> {
    Err(VoiceMeError::Other(
        "voice-me cannot install a Virtual Microphone on this system yet.".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    //! The story's I/O & Edge-Case Matrix, driven against temporary
    //! directories.
    //!
    //! `ORT_DYLIB_PATH` is process-global, so the rows that depend on it are
    //! serialised behind one mutex and always restore what they found — a
    //! stray variable left behind would silently change what every later
    //! test in this process sees.

    use std::path::Path;
    use voice_me_core::DependencyStatus;

    use super::*;
    use crate::test_support::EnvGuard;

    /// The common starting point: no configured runtime path, so the check
    /// falls back to the cache root's own copy.
    fn no_configured_runtime() -> EnvGuard {
        EnvGuard::new().unset(assets::RUNTIME_DYLIB_ENV)
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"not really a model, but it is on disk").unwrap();
    }

    /// A cache root with every file the given variant needs, plus the
    /// runtime library where the check looks for it with no env var set.
    fn provisioned(weights: SpeechWeights) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for path in assets::required_model_files(dir.path(), weights) {
            touch(&path);
        }
        touch(&assets::bundled_runtime_dylib(dir.path()));
        dir
    }

    fn row(rows: &[Dependency], kind: DependencyKind) -> &Dependency {
        rows.iter()
            .find(|dependency| dependency.kind == kind)
            .unwrap_or_else(|| panic!("no {kind:?} row in {rows:?}"))
    }

    /// The sources as they are before voice-me's release is pinned
    /// (decision 7), whatever `Sources::pinned` says today.
    fn unpinned_sources() -> Sources {
        Sources {
            runtime_all_providers: false,
            cuda_runtime: None,
            nvidia_wheels: Vec::new(),
            ..Sources::pinned()
        }
    }

    fn fake_archive(relative_path: &str, size: u64, entries: &[(&str, &str)]) -> RuntimeArchive {
        RuntimeArchive {
            archive: sources::Asset {
                relative_path: relative_path.to_string(),
                url: format!("https://example.invalid/{relative_path}"),
                size,
                digest: sources::Digest::Sha256("00".repeat(32)),
            },
            library_entries: entries
                .iter()
                .map(|(entry, file_name)| sources::LibraryEntry::new(*entry, *file_name))
                .collect(),
        }
    }

    /// Story 3.8 once pinned: voice-me's core, its CUDA provider, and two
    /// NVIDIA wheels (the rows only read their file names and sizes).
    fn all_provider_sources() -> Sources {
        let lib = assets::runtime_dylib_file_name();
        let provider = assets::cuda_provider_file_name();
        Sources {
            runtime: Some(fake_archive("runtime/core.tgz", 10, &[("core/lib/x", lib)])),
            runtime_all_providers: true,
            cuda_runtime: Some(fake_archive(
                "runtime/cuda.tgz",
                20,
                &[("cuda/lib/x", provider)],
            )),
            nvidia_wheels: vec![
                fake_archive(
                    "runtime/cuda/cudart.whl",
                    1_000_000,
                    &[("nvidia/cuda_runtime/lib/libcudart.so.12", "libcudart.so.12")],
                ),
                fake_archive(
                    "runtime/cuda/cudnn.whl",
                    2_000_000,
                    &[
                        (
                            "nvidia/cudnn/lib/libcudnn_graph.so.9",
                            "libcudnn_graph.so.9",
                        ),
                        ("nvidia/cudnn/lib/libcudnn.so.9", "libcudnn.so.9"),
                    ],
                ),
            ],
            ..Sources::pinned()
        }
    }

    fn mark_runtime_from(root: &Path, sources: &Sources) {
        let marker = assets::runtime_source_file(root);
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(marker, archive_stamp(sources.runtime.as_ref().unwrap())).unwrap();
    }

    /// Install's record of NVIDIA wheels, as if `wheels` were installed.
    fn mark_wheels_installed(root: &Path, wheels: &[RuntimeArchive]) {
        let marker = assets::cuda_libraries_source_file(root);
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        let text: String = wheels
            .iter()
            .map(|wheel| {
                let files: Vec<&str> = wheel
                    .library_entries
                    .iter()
                    .map(|entry| entry.file_name.as_str())
                    .collect();
                assets::cuda_marker_line(&archive_stamp(wheel), &files)
            })
            .collect();
        std::fs::write(marker, text).unwrap();
    }

    /// Story 3.8, matrix row "CUDA": on an empty cache every missing piece
    /// is its own row, and every one of them offers Install.
    #[test]
    fn a_cuda_backend_on_the_bundled_runtime_reports_each_missing_piece() {
        let _env = no_configured_runtime();
        let dir = tempfile::tempdir().unwrap();
        let cuda = SpeechBackend::for_target(SpeechExecutionTarget::Cuda);

        let rows = speech_engine_rows(dir.path(), cuda, None, &all_provider_sources());

        let kinds: Vec<_> = rows.iter().map(|row| row.kind).collect();
        assert_eq!(
            kinds,
            [
                DependencyKind::OnnxRuntime,
                DependencyKind::CudaProvider,
                DependencyKind::NvidiaLibraries,
                DependencyKind::ModelWeights
            ]
        );
        for kind in &kinds[..3] {
            let row = row(&rows, *kind);
            assert!(row.status.is_missing(), "{row:?}");
            assert!(row.automatable, "{row:?}");
            assert!(row.kind.blocks_speech());
        }
        let nvidia = row(&rows, DependencyKind::NvidiaLibraries);
        assert!(
            nvidia.detail.contains("Missing 3 of 3 libraries"),
            "{}",
            nvidia.detail
        );
        assert!(
            nvidia.detail.contains("libcudart.so.12"),
            "{}",
            nvidia.detail
        );
        assert!(nvidia.detail.contains("(3 MB)"), "{}", nvidia.detail);
    }

    #[test]
    fn a_complete_cuda_install_reports_every_piece_ready() {
        let _env = no_configured_runtime();
        let dir = provisioned(SpeechWeights::Fp16);
        let sources = all_provider_sources();
        mark_runtime_from(dir.path(), &sources);
        touch(&assets::bundled_cuda_provider(dir.path()));
        let cuda_dir = assets::cuda_libraries_dir(dir.path());
        for name in ["libcudart.so.12", "libcudnn_graph.so.9", "libcudnn.so.9"] {
            touch(&cuda_dir.join(name));
        }
        mark_wheels_installed(dir.path(), &sources.nvidia_wheels);

        let rows = speech_engine_rows(
            dir.path(),
            SpeechBackend::for_target(SpeechExecutionTarget::Cuda),
            None,
            &sources,
        );

        assert_eq!(rows.len(), 4, "{rows:?}");
        assert!(rows.iter().all(|row| !row.status.is_missing()), "{rows:?}");
    }

    /// Only the NVIDIA wheel still missing a library counts toward what
    /// Install would download.
    #[test]
    fn the_nvidia_row_counts_only_what_is_still_missing() {
        let _env = no_configured_runtime();
        let dir = tempfile::tempdir().unwrap();
        let sources = all_provider_sources();
        touch(&assets::cuda_libraries_dir(dir.path()).join("libcudart.so.12"));
        mark_wheels_installed(dir.path(), &sources.nvidia_wheels[..1]);

        let row = nvidia_libraries_row(dir.path(), &sources);

        assert!(row.detail.contains("Missing 2 of 3"), "{}", row.detail);
        assert!(row.detail.contains("(2 MB)"), "{}", row.detail);
    }

    /// Matrix row "WebGPU" after pinning: the CPU-only runtime already in
    /// the cache (no record of its source) cannot run WebGPU, and Install
    /// replaces it. The CPU backend keeps using it.
    #[test]
    fn a_cpu_only_runtime_in_the_cache_is_replaced_for_a_gpu_backend_only() {
        let _env = no_configured_runtime();
        let dir = provisioned(SpeechWeights::Fp16);
        let sources = all_provider_sources();
        let webgpu = SpeechBackend::for_target(SpeechExecutionTarget::WebGpu);

        let stale = row(
            &speech_engine_rows(dir.path(), webgpu, None, &sources),
            DependencyKind::OnnxRuntime,
        )
        .clone();
        assert!(stale.status.is_missing());
        assert!(stale.automatable);
        assert!(
            stale.detail.contains("an older or CPU-only build"),
            "{}",
            stale.detail
        );

        let cpu = speech_engine_rows(dir.path(), SpeechBackend::CPU, None, &sources);
        assert!(!row(&cpu, DependencyKind::OnnxRuntime).status.is_missing());

        mark_runtime_from(dir.path(), &sources);
        let current = speech_engine_rows(dir.path(), webgpu, None, &sources);
        assert!(
            !row(&current, DependencyKind::OnnxRuntime)
                .status
                .is_missing()
        );
        assert_eq!(current.len(), 2, "WebGPU needs no CUDA rows: {current:?}");
    }

    /// Matrix rows "Before pinning" and "Added runtime": no CUDA rows, and
    /// the runtime row behaves exactly as before Story 3.8.
    #[test]
    fn cuda_rows_appear_only_for_the_bundled_runtime_once_pinned() {
        let _env = no_configured_runtime();
        let dir = tempfile::tempdir().unwrap();
        let cuda = SpeechBackend::for_target(SpeechExecutionTarget::Cuda);

        let unpinned = speech_engine_rows(dir.path(), cuda, None, &unpinned_sources());
        assert_eq!(unpinned.len(), 2, "{unpinned:?}");
        assert!(!row(&unpinned, DependencyKind::OnnxRuntime).automatable);

        let added = dir.path().join("mine").join("libonnxruntime.so");
        touch(&added);
        let rows = speech_engine_rows(dir.path(), cuda, Some(&added), &all_provider_sources());
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(!row(&rows, DependencyKind::OnnxRuntime).status.is_missing());
    }

    /// A re-pin under the same file names (voiceme.2) is still a
    /// different runtime: the record holds the URL and SHA-256.
    #[test]
    fn a_repinned_core_reads_stale_and_its_provider_missing() {
        let _env = no_configured_runtime();
        let dir = provisioned(SpeechWeights::Fp16);
        let old = all_provider_sources();
        mark_runtime_from(dir.path(), &old);
        touch(&assets::bundled_cuda_provider(dir.path()));
        let mut repinned = all_provider_sources();
        repinned.runtime.as_mut().unwrap().archive.digest =
            sources::Digest::Sha256("11".repeat(32));
        let cuda = SpeechBackend::for_target(SpeechExecutionTarget::Cuda);

        assert!(!runtime_is_stale(dir.path(), &old));
        assert!(runtime_is_stale(dir.path(), &repinned));
        let rows = speech_engine_rows(dir.path(), cuda, None, &repinned);
        assert!(row(&rows, DependencyKind::OnnxRuntime).status.is_missing());
        let provider = row(&rows, DependencyKind::CudaProvider);
        assert!(provider.status.is_missing(), "{provider:?}");
        assert!(provider.automatable);
    }

    /// A pinned runtime missing one of the libraries it ships with is
    /// fetched again.
    #[test]
    fn a_runtime_missing_a_shipped_library_is_missing() {
        let _env = no_configured_runtime();
        let dir = provisioned(SpeechWeights::Q4);
        let mut sources = all_provider_sources();
        sources
            .runtime
            .as_mut()
            .unwrap()
            .library_entries
            .push(sources::LibraryEntry::new(
                "core/lib/y",
                "libonnxruntime_providers_shared.so",
            ));
        mark_runtime_from(dir.path(), &sources);

        let rows = speech_engine_rows(dir.path(), SpeechBackend::CPU, None, &sources);
        let runtime = row(&rows, DependencyKind::OnnxRuntime);

        assert!(runtime.status.is_missing(), "{runtime:?}");
        assert!(runtime.automatable);
        touch(&assets::runtime_dir(dir.path()).join("libonnxruntime_providers_shared.so"));
        let rows = speech_engine_rows(dir.path(), SpeechBackend::CPU, None, &sources);
        assert!(!row(&rows, DependencyKind::OnnxRuntime).status.is_missing());
    }

    /// NVIDIA libraries recorded under an older wheel pin are all replaced.
    #[test]
    fn nvidia_libraries_from_an_older_pin_are_missing() {
        let _env = no_configured_runtime();
        let dir = tempfile::tempdir().unwrap();
        let sources = all_provider_sources();
        let cuda_dir = assets::cuda_libraries_dir(dir.path());
        for name in ["libcudart.so.12", "libcudnn_graph.so.9", "libcudnn.so.9"] {
            touch(&cuda_dir.join(name));
        }
        let mut older = sources.nvidia_wheels.clone();
        older[1].archive.digest = sources::Digest::Sha256("22".repeat(32));
        mark_wheels_installed(dir.path(), &older);

        let row = nvidia_libraries_row(dir.path(), &sources);

        assert!(row.status.is_missing());
        assert!(row.detail.contains("older version"), "{}", row.detail);
        mark_wheels_installed(dir.path(), &sources.nvidia_wheels);
        assert!(
            !nvidia_libraries_row(dir.path(), &sources)
                .status
                .is_missing()
        );
    }

    /// Story 3.8 (Windows): a staged update is the GPU runtime rows'
    /// reason until restart; the CPU backend keeps its runtime.
    #[test]
    fn a_staged_runtime_asks_for_a_restart_on_gpu_rows() {
        let _env = no_configured_runtime();
        let dir = provisioned(SpeechWeights::Fp16);
        let sources = all_provider_sources();
        touch(&assets::staged_runtime_dir(dir.path()).join(assets::RUNTIME_SOURCE_FILE));

        let rows = speech_engine_rows(
            dir.path(),
            SpeechBackend::for_target(SpeechExecutionTarget::Cuda),
            None,
            &sources,
        );
        for kind in [DependencyKind::OnnxRuntime, DependencyKind::CudaProvider] {
            let row = row(&rows, kind);
            assert!(row.status.is_missing());
            assert!(!row.automatable, "{row:?}");
            assert!(row.detail.contains("restart voice-me"), "{row:?}");
        }
        let cpu = speech_engine_rows(dir.path(), SpeechBackend::CPU, None, &sources);
        assert!(!row(&cpu, DependencyKind::OnnxRuntime).status.is_missing());
    }

    /// A complete staged set moves into `runtime/` (its record too, the
    /// old provider gone); an incomplete one is thrown away.
    #[test]
    fn a_staged_runtime_is_applied_at_start() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(assets::runtime_dir(root)).unwrap();
        std::fs::write(assets::bundled_runtime_dylib(root), b"old").unwrap();
        std::fs::write(assets::bundled_cuda_provider(root), b"old provider").unwrap();
        let staged = assets::staged_runtime_dir(root);
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join(assets::runtime_dylib_file_name()), b"new").unwrap();

        assert!(
            !apply_staged_runtime(root).unwrap(),
            "no record: incomplete"
        );
        assert!(!staged.exists());
        assert_eq!(
            std::fs::read(assets::bundled_runtime_dylib(root)).unwrap(),
            b"old"
        );

        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join(assets::runtime_dylib_file_name()), b"new").unwrap();
        std::fs::write(staged.join(assets::RUNTIME_SOURCE_FILE), b"stamp").unwrap();
        assert!(staged_runtime_pending(root));

        assert!(apply_staged_runtime(root).unwrap());
        assert_eq!(
            std::fs::read(assets::bundled_runtime_dylib(root)).unwrap(),
            b"new"
        );
        assert_eq!(
            std::fs::read(assets::runtime_source_file(root)).unwrap(),
            b"stamp"
        );
        assert!(!assets::bundled_cuda_provider(root).exists());
        assert!(!staged.exists());
        assert!(!staged_runtime_pending(root));
        assert!(!apply_staged_runtime(root).unwrap());
    }

    /// A configured `ORT_DYLIB_PATH` is the user's too: no CUDA rows.
    #[test]
    fn a_configured_runtime_gets_no_cuda_rows() {
        let dir = tempfile::tempdir().unwrap();
        let configured = dir.path().join("mine").join("libonnxruntime.so");
        touch(&configured);
        let _env = EnvGuard::new().set(assets::RUNTIME_DYLIB_ENV, &configured);

        let rows = speech_engine_rows(
            dir.path(),
            SpeechBackend::for_target(SpeechExecutionTarget::Cuda),
            None,
            &all_provider_sources(),
        );

        assert_eq!(rows.len(), 2, "{rows:?}");
    }

    #[test]
    fn a_fully_provisioned_cpu_cache_reports_every_row_ready() {
        let _env = no_configured_runtime();
        let dir = provisioned(SpeechWeights::Q4);

        let rows = speech_engine_rows(dir.path(), SpeechBackend::CPU, None, &Sources::pinned());

        assert!(
            rows.iter().all(|row| row.status == DependencyStatus::Ready),
            "{rows:?}"
        );
    }

    #[test]
    fn an_absent_language_model_names_the_exact_path() {
        let _env = no_configured_runtime();
        let dir = provisioned(SpeechWeights::Q4);
        let weights = assets::graph_file(
            dir.path(),
            assets::language_model_file_name(SpeechWeights::Q4),
        );
        std::fs::remove_file(&weights).unwrap();

        let rows = speech_engine_rows(dir.path(), SpeechBackend::CPU, None, &Sources::pinned());
        let model = row(&rows, DependencyKind::ModelWeights);

        assert_eq!(model.status, DependencyStatus::Missing);
        assert!(
            model.detail.contains(&weights.display().to_string()),
            "the absent path is the only actionable thing to say: {}",
            model.detail
        );
    }

    /// The external-data sibling is as required as the graph, and ONNX
    /// Runtime's own message for an absent one names a bare filename that
    /// is useless to whoever has to go fetch it.
    #[test]
    fn an_absent_external_data_file_is_missing_too() {
        let _env = no_configured_runtime();
        let dir = provisioned(SpeechWeights::Q4);
        let data =
            assets::graph_file(dir.path(), "speech_encoder.onnx").with_extension("onnx_data");
        std::fs::remove_file(&data).unwrap();

        let rows = speech_engine_rows(dir.path(), SpeechBackend::CPU, None, &Sources::pinned());

        assert_eq!(
            row(&rows, DependencyKind::ModelWeights).status,
            DependencyStatus::Missing
        );
    }

    #[test]
    fn an_absent_tokenizer_is_missing_too() {
        let _env = no_configured_runtime();
        let dir = provisioned(SpeechWeights::Q4);
        std::fs::remove_file(assets::tokenizer_file(dir.path())).unwrap();

        let rows = speech_engine_rows(dir.path(), SpeechBackend::CPU, None, &Sources::pinned());

        assert_eq!(
            row(&rows, DependencyKind::ModelWeights).status,
            DependencyStatus::Missing
        );
    }

    #[test]
    fn an_unresolvable_runtime_says_where_it_looked() {
        let _env = no_configured_runtime();
        let dir = provisioned(SpeechWeights::Q4);
        std::fs::remove_file(assets::bundled_runtime_dylib(dir.path())).unwrap();

        let rows = speech_engine_rows(dir.path(), SpeechBackend::CPU, None, &Sources::pinned());
        let runtime = row(&rows, DependencyKind::OnnxRuntime);

        assert_eq!(runtime.status, DependencyStatus::Missing);
        assert!(
            runtime.detail.contains(
                &assets::bundled_runtime_dylib(dir.path())
                    .display()
                    .to_string()
            ),
            "{}",
            runtime.detail
        );
        assert!(
            runtime.detail.contains(assets::RUNTIME_DYLIB_ENV),
            "the rule itself has to be stated, or the path looks arbitrary: {}",
            runtime.detail
        );
    }

    #[test]
    fn a_stale_configured_runtime_path_names_the_configured_path() {
        let dir = provisioned(SpeechWeights::Q4);
        let stale = dir.path().join("moved-away").join("libonnxruntime.so");
        let _env = EnvGuard::new().set(assets::RUNTIME_DYLIB_ENV, &stale);

        let rows = speech_engine_rows(dir.path(), SpeechBackend::CPU, None, &Sources::pinned());
        let runtime = row(&rows, DependencyKind::OnnxRuntime);

        assert_eq!(runtime.status, DependencyStatus::Missing);
        assert!(
            runtime.detail.contains(&stale.display().to_string()),
            "the path the user configured is the one that is wrong: {}",
            runtime.detail
        );
        assert!(
            !runtime.automatable,
            "nothing can be downloaded to satisfy a path the user chose"
        );
        assert!(
            (2..=4).contains(&runtime.manual_steps.len()),
            "a manual row carries two to four short steps instead: {:?}",
            runtime.manual_steps
        );
    }

    /// Decision 3 (spec 3-2), and decision 7 of Story 3.8: until voice-me's
    /// own build is pinned, a GPU backend's runtime has no source, so its
    /// row is manual and says so rather than offering an Install that
    /// would fetch the CPU runtime.
    #[test]
    fn a_gpu_backends_missing_runtime_is_manual() {
        let _env = no_configured_runtime();
        let dir = tempfile::tempdir().unwrap();
        let backend = SpeechBackend {
            target: SpeechExecutionTarget::WebGpu,
            device: None,
            weights: SpeechWeights::Fp16,
        };

        let rows = speech_engine_rows(dir.path(), backend, None, &unpinned_sources());
        let runtime = row(&rows, DependencyKind::OnnxRuntime);

        assert!(!runtime.automatable);
        assert!(
            runtime.detail.contains("not yet available"),
            "{}",
            runtime.detail
        );
        assert!(!runtime.manual_steps.is_empty());
    }

    /// Decision 1: the runtime row offers Install only where a runtime
    /// source exists for this target.
    #[test]
    fn a_runtime_with_no_source_for_this_target_is_manual() {
        let _env = no_configured_runtime();
        let dir = tempfile::tempdir().unwrap();
        let no_runtime = Sources {
            runtime: None,
            ..Sources::pinned()
        };

        let rows = speech_engine_rows(dir.path(), SpeechBackend::CPU, None, &no_runtime);
        let runtime = row(&rows, DependencyKind::OnnxRuntime);

        assert!(!runtime.automatable);
        assert!((2..=4).contains(&runtime.manual_steps.len()));
        assert!(
            !runtime
                .manual_steps
                .iter()
                .any(|step| step.contains("http")),
            "no external link: {:?}",
            runtime.manual_steps
        );
    }

    #[test]
    fn a_configured_runtime_that_exists_is_ready() {
        let dir = provisioned(SpeechWeights::Q4);
        let elsewhere = dir.path().join("elsewhere").join("libonnxruntime.so");
        touch(&elsewhere);
        let _env = EnvGuard::new().set(assets::RUNTIME_DYLIB_ENV, &elsewhere);

        let rows = speech_engine_rows(dir.path(), SpeechBackend::CPU, None, &Sources::pinned());

        assert_eq!(
            row(&rows, DependencyKind::OnnxRuntime).status,
            DependencyStatus::Ready
        );
    }

    /// The backend-relative rule, at its sharpest: FP16 files on disk do
    /// not make a Q4 selection ready, and no FP16 row is reported at all.
    #[test]
    fn another_variants_weights_do_not_satisfy_the_selected_one() {
        let _env = no_configured_runtime();
        let dir = provisioned(SpeechWeights::Fp16);

        let rows = speech_engine_rows(dir.path(), SpeechBackend::CPU, None, &Sources::pinned());
        let model = row(&rows, DependencyKind::ModelWeights);

        assert_eq!(model.status, DependencyStatus::Missing);
        assert!(
            model.detail.contains("language_model_q4.onnx"),
            "{}",
            model.detail
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row.kind == DependencyKind::ModelWeights)
                .count(),
            1,
            "the unselected variant is not a dependency, so it is not a row: {rows:?}"
        );
        assert!(
            !rows.iter().any(|row| row.label.contains("FP16")),
            "{rows:?}"
        );
    }

    /// Decision 3: a machine with no speech engine blocks; the same machine
    /// with only its Virtual Microphone gone does not.
    #[test]
    fn only_speech_engine_rows_block_the_overlay() {
        let blocked = DependencyReport::new(
            SpeechBackend::CPU,
            vec![Dependency::missing(
                DependencyKind::ModelWeights,
                "Speech model files (Q4)",
                "Missing: /cache/onnx/language_model_q4.onnx",
            )],
        );
        assert_eq!(
            blocked.speech_engine_blocker().map(|row| row.kind),
            Some(DependencyKind::ModelWeights)
        );

        let mic_only = DependencyReport::new(
            SpeechBackend::CPU,
            vec![
                Dependency::ready(DependencyKind::OnnxRuntime, "ONNX Runtime", "ok"),
                Dependency::ready(
                    DependencyKind::ModelWeights,
                    "Speech model files (Q4)",
                    "ok",
                ),
                Dependency::missing(
                    DependencyKind::VirtualMicrophone,
                    "Virtual Microphone",
                    "not loaded",
                ),
            ],
        );
        assert!(mic_only.has_missing(), "it is still reported as missing");
        assert!(
            mic_only.speech_engine_blocker().is_none(),
            "but the user can still type — playback is what fails, later"
        );
    }

    /// With no `HOME`, no `XDG_CACHE_HOME` and no override there is no path
    /// to say anything about, so the check itself fails — as a domain
    /// error the tab can print, never as a panic.
    #[test]
    #[cfg(not(target_os = "windows"))]
    fn an_unresolvable_cache_root_fails_the_check_rather_than_panicking() {
        let _env = EnvGuard::new()
            .unset(assets::CACHE_ROOT_ENV)
            .unset("XDG_CACHE_HOME")
            .unset("HOME");

        let error = assets::model_cache_root().expect_err("nothing to resolve a cache root from");

        assert!(
            error.to_string().contains("cache directory"),
            "the reason has to reach the Dependencies tab: {error}"
        );
    }

    /// Story 2.8, matrix row "Row, installed".
    #[test]
    fn a_present_cable_is_ready_and_names_cable_output() {
        let row = vb_cable_row(Ok(true), false);
        assert_eq!(row.status, DependencyStatus::Ready);
        assert_eq!(
            row.detail,
            "Other applications can select CABLE Output as their microphone."
        );
    }

    /// Story 2.8, matrix row "Row, missing": Install (automatable, no
    /// manual steps) and VB-CABLE's credit.
    #[test]
    fn a_missing_cable_offers_install_and_credits_vb_cable() {
        let row = vb_cable_row(Ok(false), false);
        assert_eq!(row.status, DependencyStatus::Missing);
        assert!(row.automatable, "Install, not manual steps");
        assert!(row.manual_steps.is_empty());
        assert!(row.detail.contains("donationware"), "{}", row.detail);
        assert!(row.detail.contains("www.vb-cable.com"), "{}", row.detail);
        assert!(!row.detail.contains("Restart"), "{}", row.detail);
        assert!(
            row.detail.contains(
                "Or download VB-CABLE yourself from www.vb-cable.com and run \
                 VBCABLE_Setup_x64.exe as administrator."
            ),
            "{}",
            row.detail
        );
    }

    /// Story 2.8, matrix row "Installed, not yet active".
    #[test]
    fn a_cable_installed_but_not_yet_active_asks_for_a_restart() {
        let row = vb_cable_row(Ok(false), true);
        assert_eq!(row.status, DependencyStatus::Missing);
        assert!(
            row.detail
                .contains("Restart Windows, then press Check again"),
            "{}",
            row.detail
        );
    }

    #[test]
    fn a_device_list_that_failed_is_a_missing_row_with_the_reason() {
        let row = vb_cable_row(
            Err(VoiceMeError::VirtualMicUnavailable(
                "no audio service".to_string(),
            )),
            false,
        );
        assert_eq!(row.status, DependencyStatus::Missing);
        assert!(row.detail.contains("no audio service"), "{}", row.detail);
    }

    /// The Windows row is reported on Windows: present or missing, never
    /// absent. Runs on the Windows CI job.
    #[test]
    #[cfg(target_os = "windows")]
    fn windows_reports_a_virtual_microphone_row() {
        let dir = tempfile::tempdir().unwrap();
        let row = virtual_microphone_row(dir.path()).expect("a row on Windows");
        assert_eq!(row.kind, DependencyKind::VirtualMicrophone);
    }

    /// The marker `provision_virtual_mic` writes (`<cache>/vb-cable/
    /// setup-ran`) is the one the row reads. Skipped when this machine
    /// already has the cable, since the row is then Ready.
    #[test]
    #[cfg(target_os = "windows")]
    fn the_setup_ran_marker_is_read_where_install_writes_it() {
        let dir = tempfile::tempdir().unwrap();
        let unpacked = crate::sources::vb_cable_pack()
            .destination(dir.path())
            .with_extension("");
        let marker = unpacked.parent().unwrap().join(VB_CABLE_SETUP_RAN);
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(&marker, b"").unwrap();
        assert_eq!(
            marker,
            dir.path().join("vb-cable").join("setup-ran"),
            "the writer's path"
        );

        let row = virtual_microphone_row(dir.path()).expect("a row on Windows");
        if row.status != DependencyStatus::Ready {
            assert!(row.detail.contains("Restart Windows"), "{}", row.detail);
        }
    }

    /// The real Windows installer hook refuses a directory with no setup in
    /// it before PowerShell is ever started.
    #[test]
    #[cfg(target_os = "windows")]
    fn the_windows_installer_needs_the_unpacked_setup() {
        let empty = tempfile::tempdir().unwrap();
        let message = install_virtual_microphone(Some(empty.path()))
            .unwrap_err()
            .to_string();
        assert!(message.contains("VBCABLE_Setup_x64.exe"), "{message}");

        assert!(install_virtual_microphone(None).is_err());
    }

    #[test]
    fn the_report_carries_the_backend_it_was_computed_for() {
        let dir = provisioned(SpeechWeights::Q4);
        let _env = EnvGuard::new()
            .unset(assets::RUNTIME_DYLIB_ENV)
            .set(assets::CACHE_ROOT_ENV, dir.path());
        let (tx, mut rx) = futures::channel::mpsc::unbounded();

        DepsAdapter::new().check(CheckRequest::cpu(), tx).unwrap();

        let AppEvent::DependencyCheckCompleted { report } =
            rx.try_recv().expect("the check reports by event")
        else {
            panic!("the check sends exactly one kind of event");
        };
        assert_eq!(report.backend, SpeechBackend::CPU);
        assert!(
            report
                .dependencies
                .iter()
                .any(|row| row.kind == DependencyKind::ModelWeights)
        );
    }

    /// A probe that answers "nothing here" to everything.
    struct NoGpu;

    impl GpuProbe for NoGpu {
        fn cuda(&self) -> capability::CudaProbe {
            capability::CudaProbe::NoDriver
        }

        fn vulkan(&self) -> capability::VulkanProbe {
            capability::VulkanProbe::NoLoader
        }
    }

    fn check_with(request: CheckRequest, root: &Path) -> DependencyReport {
        let _env = EnvGuard::new()
            .unset(assets::RUNTIME_DYLIB_ENV)
            .set(assets::CACHE_ROOT_ENV, root);
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        DepsAdapter::new()
            .with_gpu_probe(NoGpu)
            .check(request, tx)
            .unwrap();
        let AppEvent::DependencyCheckCompleted { report } = rx.try_recv().unwrap() else {
            panic!("the check sends exactly one kind of event");
        };
        report
    }

    /// The acceptance criterion's machine: CUDA selected, no NVIDIA driver.
    /// The capability row comes first, blocks, and is the overlay's reason.
    #[test]
    fn a_cuda_selection_with_no_driver_leads_with_a_blocking_capability_row() {
        let dir = provisioned(SpeechWeights::Fp16);
        let runtime = dir.path().join("gpu").join("libonnxruntime.so");
        touch(&runtime);
        let selection = voice_me_core::BackendSelection::Local {
            runtime: Some(runtime.clone()),
            target: SpeechExecutionTarget::Cuda,
        };

        let report = check_with(
            CheckRequest {
                backend: SpeechBackend::for_target(SpeechExecutionTarget::Cuda),
                selection,
                has_api_key: false,
                has_region: false,
                has_voice: false,
                piper_voice: None,
            },
            dir.path(),
        );

        let first = &report.dependencies[0];
        assert_eq!(first.kind, DependencyKind::BackendCapability);
        assert!(
            first.detail.contains("No NVIDIA driver found."),
            "{}",
            first.detail
        );
        assert_eq!(
            report.speech_engine_blocker().map(|row| row.kind),
            Some(DependencyKind::BackendCapability)
        );
        // The added library is reported on like any other path.
        let runtime_row = row(&report.dependencies, DependencyKind::OnnxRuntime);
        assert_eq!(runtime_row.status, DependencyStatus::Ready);
        assert!(runtime_row.detail.contains(&runtime.display().to_string()));
        // Weights follow the target: FP16 for CUDA.
        assert!(
            row(&report.dependencies, DependencyKind::ModelWeights)
                .label
                .contains("FP16")
        );
    }

    /// A CPU selection has no capability row — and still no FP16 row.
    #[test]
    fn a_cpu_selection_reports_no_capability_row() {
        let dir = provisioned(SpeechWeights::Q4);

        let report = check_with(CheckRequest::cpu(), dir.path());

        assert!(
            !report
                .dependencies
                .iter()
                .any(|row| row.kind == DependencyKind::BackendCapability),
            "{:?}",
            report.dependencies
        );
        assert!(report.speech_engine_blocker().is_none());
    }

    /// A remote selection's readiness is a key, not a file list.
    #[test]
    fn a_remote_selection_reports_no_engine_rows() {
        let dir = tempfile::tempdir().unwrap();

        let report = check_with(
            CheckRequest {
                backend: SpeechBackend::CPU,
                selection: voice_me_core::BackendSelection::Remote(
                    voice_me_core::RemoteProvider::DeepInfra,
                ),
                has_api_key: false,
                has_region: false,
                has_voice: false,
                piper_voice: None,
            },
            dir.path(),
        );

        assert_eq!(
            report.dependencies[0].kind,
            DependencyKind::BackendCapability
        );
        assert!(
            !report.dependencies.iter().any(|row| matches!(
                row.kind,
                DependencyKind::OnnxRuntime | DependencyKind::ModelWeights
            )),
            "{:?}",
            report.dependencies
        );
    }

    /// Story 3.12: the System voice reports its engine row and no ONNX
    /// rows — on Linux, whatever this machine has; elsewhere, the
    /// capability row instead.
    #[test]
    fn a_system_voice_selection_reports_its_engine_and_no_onnx_rows() {
        let dir = tempfile::tempdir().unwrap();

        let report = check_with(
            CheckRequest {
                backend: SpeechBackend::CPU,
                selection: voice_me_core::BackendSelection::SystemVoice,
                has_api_key: false,
                has_region: false,
                has_voice: false,
                piper_voice: None,
            },
            dir.path(),
        );

        assert!(
            !report.dependencies.iter().any(|row| matches!(
                row.kind,
                DependencyKind::OnnxRuntime | DependencyKind::ModelWeights
            )),
            "{:?}",
            report.dependencies
        );
        #[cfg(target_os = "linux")]
        {
            let engine = row(&report.dependencies, DependencyKind::SystemVoiceEngine);
            assert_eq!(
                engine.status.is_missing(),
                voice_me_espeak::find_program().is_none()
            );
        }
        #[cfg(not(target_os = "linux"))]
        assert_eq!(
            report.dependencies[0].kind,
            DependencyKind::BackendCapability
        );
        // Story 3.13: Windows never reports the eSpeak NG row for the
        // System voice — that kind's Install downloads eSpeak NG.
        #[cfg(target_os = "windows")]
        assert!(
            !report
                .dependencies
                .iter()
                .any(|row| row.kind == DependencyKind::SystemVoiceEngine),
            "{:?}",
            report.dependencies
        );
    }

    /// Story 3.13: on Windows the System voice's row is what the injected
    /// probe says — ready with voices, blocking with Windows' steps without.
    #[test]
    #[cfg(target_os = "windows")]
    fn the_windows_system_voice_row_follows_the_injected_probe() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::new()
            .unset(assets::RUNTIME_DYLIB_ENV)
            .set(assets::CACHE_ROOT_ENV, dir.path());
        let request = CheckRequest {
            backend: SpeechBackend::CPU,
            selection: voice_me_core::BackendSelection::SystemVoice,
            has_api_key: false,
            has_region: false,
            has_voice: false,
            piper_voice: None,
        };
        let check = |probe: fn() -> Result<usize, String>| {
            let (tx, mut rx) = futures::channel::mpsc::unbounded();
            DepsAdapter::new()
                .with_gpu_probe(NoGpu)
                .with_system_voice_probe(probe)
                .check(request.clone(), tx)
                .unwrap();
            let AppEvent::DependencyCheckCompleted { report } = rx.try_recv().unwrap() else {
                panic!("the check sends exactly one kind of event");
            };
            row(&report.dependencies, DependencyKind::BackendCapability).clone()
        };

        assert!(!check(|| Ok(3)).status.is_missing());
        let blocked = check(|| Ok(0));
        assert!(blocked.status.is_missing());
        assert!(!blocked.automatable);
        assert!(blocked.detail.contains("Add voices"), "{}", blocked.detail);
        assert!(check(|| Err("no engine".to_string())).status.is_missing());
    }

    #[test]
    fn an_added_runtime_that_moved_is_a_manual_row_naming_its_path() {
        let _env = no_configured_runtime();
        let dir = provisioned(SpeechWeights::Fp16);
        let gone = dir.path().join("moved").join("libonnxruntime.so");

        let rows = speech_engine_rows(
            dir.path(),
            SpeechBackend::for_target(SpeechExecutionTarget::Cuda),
            Some(&gone),
            &Sources::pinned(),
        );
        let runtime = row(&rows, DependencyKind::OnnxRuntime);

        assert_eq!(runtime.status, DependencyStatus::Missing);
        assert!(!runtime.automatable);
        assert!(runtime.detail.contains(&gone.display().to_string()));
    }

    #[test]
    fn the_gpu_runtime_sentence_names_cuda() {
        assert!(
            gpu_runtime_unavailable(SpeechBackend::for_target(SpeechExecutionTarget::Cuda))
                .contains("CUDA")
        );
    }

    /// Story 3.14: Azure is a stock voice, but gets no eSpeak row and no
    /// ONNX rows — only its capability row.
    #[test]
    fn an_azure_selection_reports_no_espeak_or_engine_rows() {
        let dir = tempfile::tempdir().unwrap();

        let report = check_with(
            CheckRequest {
                backend: SpeechBackend::CPU,
                selection: voice_me_core::BackendSelection::Remote(
                    voice_me_core::RemoteProvider::Azure,
                ),
                has_api_key: true,
                has_region: true,
                has_voice: false,
                piper_voice: None,
            },
            dir.path(),
        );

        assert!(
            !report.dependencies.iter().any(|row| matches!(
                row.kind,
                DependencyKind::OnnxRuntime
                    | DependencyKind::ModelWeights
                    | DependencyKind::SystemVoiceEngine
            )),
            "{:?}",
            report.dependencies
        );
        assert_eq!(
            report.speech_engine_blocker().map(|row| row.kind),
            Some(DependencyKind::BackendCapability)
        );
        assert!(
            report.dependencies[0].detail.contains("no voice selected"),
            "{}",
            report.dependencies[0].detail
        );
    }

    /// Story 3.17: Edge TTS gets its program row — blocking when
    /// `edge-tts` is missing — and no key, ONNX or eSpeak row.
    #[test]
    fn an_edge_tts_selection_reports_only_its_program_row() {
        let dir = tempfile::tempdir().unwrap();

        let report = check_with(
            CheckRequest {
                backend: SpeechBackend::CPU,
                selection: BackendSelection::Remote(RemoteProvider::EdgeTts),
                has_api_key: false,
                has_region: false,
                has_voice: false,
                piper_voice: None,
            },
            dir.path(),
        );

        assert!(
            !report.dependencies.iter().any(|row| matches!(
                row.kind,
                DependencyKind::OnnxRuntime
                    | DependencyKind::ModelWeights
                    | DependencyKind::SystemVoiceEngine
            )),
            "{:?}",
            report.dependencies
        );
        #[cfg(target_os = "linux")]
        {
            assert!(
                !report
                    .dependencies
                    .iter()
                    .any(|row| row.kind == DependencyKind::BackendCapability),
                "no key row: {:?}",
                report.dependencies
            );
            let program = row(&report.dependencies, DependencyKind::EdgeTtsProgram);
            let missing = voice_me_tts_edge::find_program().is_none();
            assert_eq!(program.status.is_missing(), missing);
            if missing {
                assert!(!program.automatable);
                assert_eq!(
                    report.speech_engine_blocker().map(|row| row.kind),
                    Some(DependencyKind::EdgeTtsProgram)
                );
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            assert_eq!(
                report.speech_engine_blocker().map(|row| row.kind),
                Some(DependencyKind::BackendCapability)
            );
            assert!(
                report.dependencies[0]
                    .detail
                    .contains("Edge TTS isn't available on this OS yet."),
                "{}",
                report.dependencies[0].detail
            );
        }
    }

    /// Story 3.17: both branches of the program row, whatever this host
    /// has installed.
    #[test]
    #[cfg(target_os = "linux")]
    fn the_edge_tts_row_is_blocking_with_pipx_steps_or_ready() {
        let rows = edge_tts_rows_with(None, "ID=ubuntu\nID_LIKE=debian\n");
        assert_eq!(rows.len(), 1);
        let missing = &rows[0];
        assert_eq!(missing.kind, DependencyKind::EdgeTtsProgram);
        assert!(missing.status.is_missing());
        assert!(missing.kind.blocks_speech());
        assert!(!missing.automatable);
        assert_eq!(
            missing.detail,
            "edge-tts is not installed. Please install it: pipx install edge-tts"
        );
        assert_eq!(
            missing.manual_steps,
            vec![
                "If you don't have pipx: sudo apt install pipx".to_string(),
                "Press Check again.".to_string(),
            ]
        );

        let path = std::path::PathBuf::from("/home/erdem/.local/bin/edge-tts");
        let rows = edge_tts_rows_with(Some(path), "ID=ubuntu\n");
        let ready = &rows[0];
        assert!(!ready.status.is_missing());
        assert_eq!(ready.detail, "Found at /home/erdem/.local/bin/edge-tts.");
        assert!(ready.manual_steps.is_empty());
    }

    /// Story 3.17: there is no Install for `edge-tts`.
    #[test]
    fn provisioning_edge_tts_is_refused() {
        let (tx, mut rx) = futures::channel::mpsc::unbounded();

        let error = DepsAdapter::new()
            .provision(
                DependencyKind::EdgeTtsProgram,
                CheckRequest {
                    backend: SpeechBackend::CPU,
                    selection: BackendSelection::Remote(RemoteProvider::EdgeTts),
                    has_api_key: false,
                    has_region: false,
                    has_voice: false,
                    piper_voice: None,
                },
                tx,
            )
            .unwrap_err()
            .to_string();

        assert!(error.contains("cannot install edge-tts"), "{error}");
        assert!(matches!(
            rx.try_recv(),
            Ok(AppEvent::ProvisioningFinished {
                kind: DependencyKind::EdgeTtsProgram,
                result: Err(_),
            })
        ));
    }

    fn piper_request(voice: Option<&str>) -> CheckRequest {
        CheckRequest {
            backend: SpeechBackend::CPU,
            selection: voice_me_core::BackendSelection::PIPER_CPU,
            has_api_key: false,
            has_region: false,
            has_voice: voice.is_some(),
            piper_voice: voice.map(str::to_string),
        }
    }

    /// Story 3.15's First run row: Piper reports the shared runtime, "No
    /// Piper voice installed" (Install) and eSpeak NG — no Chatterbox
    /// model rows, and on Linux (and Windows, Story 3.16) no capability
    /// row.
    #[test]
    fn a_piper_selection_reports_runtime_voice_and_espeak_rows() {
        let dir = tempfile::tempdir().unwrap();

        let report = check_with(
            piper_request(Some(assets::PIPER_DEFAULT_VOICE.key)),
            dir.path(),
        );

        assert!(
            !report
                .dependencies
                .iter()
                .any(|row| row.kind == DependencyKind::ModelWeights),
            "{:?}",
            report.dependencies
        );
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        {
            let kinds: Vec<_> = report
                .dependencies
                .iter()
                .map(|row| row.kind)
                .filter(|kind| *kind != DependencyKind::VirtualMicrophone)
                .collect();
            assert_eq!(
                kinds,
                vec![
                    DependencyKind::OnnxRuntime,
                    DependencyKind::PiperVoice,
                    DependencyKind::SystemVoiceEngine
                ]
            );
            let voice = row(&report.dependencies, DependencyKind::PiperVoice);
            assert!(voice.status.is_missing() && voice.automatable);
            assert!(voice.detail.contains("No Piper voice installed"));
            assert_eq!(
                report.speech_engine_blocker().map(|row| row.kind),
                Some(DependencyKind::OnnxRuntime),
                "the runtime row comes first and blocks"
            );
            let espeak = row(&report.dependencies, DependencyKind::SystemVoiceEngine);
            assert_eq!(
                espeak.status.is_missing(),
                voice_me_espeak::find_program().is_none()
            );
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        assert_eq!(
            report.dependencies[0].kind,
            DependencyKind::BackendCapability
        );
    }

    /// spec-backend-engine-and-device-selects: Piper on CUDA gets the same
    /// runtime, CUDA provider and NVIDIA rows as Chatterbox, then its voice
    /// — never the model files — and a capability row for its device. Piper
    /// on CPU is unchanged.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[test]
    fn piper_on_a_gpu_reports_its_device_rows_and_no_model_files() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::new()
            .unset(assets::RUNTIME_DYLIB_ENV)
            .set(assets::CACHE_ROOT_ENV, dir.path());
        let check = |target: SpeechExecutionTarget| {
            let (tx, mut rx) = futures::channel::mpsc::unbounded();
            let request = CheckRequest {
                backend: SpeechBackend::for_target(target),
                selection: BackendSelection::Piper { target },
                ..piper_request(Some(assets::PIPER_DEFAULT_VOICE.key))
            };
            DepsAdapter::with_sources(all_provider_sources())
                .with_gpu_probe(NoGpu)
                .check(request, tx)
                .unwrap();
            let AppEvent::DependencyCheckCompleted { report } = rx.try_recv().unwrap() else {
                panic!("the check sends exactly one kind of event");
            };
            report.dependencies
        };
        let kinds = |rows: &[Dependency]| -> Vec<DependencyKind> {
            rows.iter()
                .map(|row| row.kind)
                .filter(|kind| *kind != DependencyKind::VirtualMicrophone)
                .collect()
        };

        let cuda = check(SpeechExecutionTarget::Cuda);
        assert_eq!(
            kinds(&cuda),
            vec![
                DependencyKind::BackendCapability,
                DependencyKind::OnnxRuntime,
                DependencyKind::CudaProvider,
                DependencyKind::NvidiaLibraries,
                DependencyKind::PiperVoice,
                DependencyKind::SystemVoiceEngine,
            ]
        );
        let capability = row(&cuda, DependencyKind::BackendCapability);
        assert!(capability.status.is_missing(), "{capability:?}");
        let provider = row(&cuda, DependencyKind::CudaProvider);
        assert!(provider.status.is_missing() && provider.automatable);
        let nvidia = row(&cuda, DependencyKind::NvidiaLibraries);
        assert!(nvidia.status.is_missing() && nvidia.automatable);

        // WebGPU with no GPU: a blocking capability row, and no CUDA pieces.
        let webgpu = check(SpeechExecutionTarget::WebGpu);
        assert_eq!(
            kinds(&webgpu),
            vec![
                DependencyKind::BackendCapability,
                DependencyKind::OnnxRuntime,
                DependencyKind::PiperVoice,
                DependencyKind::SystemVoiceEngine,
            ]
        );
        let capability = row(&webgpu, DependencyKind::BackendCapability);
        assert!(capability.status.is_missing(), "{capability:?}");

        let cpu = check(SpeechExecutionTarget::Cpu);
        assert_eq!(
            kinds(&cpu),
            vec![
                DependencyKind::OnnxRuntime,
                DependencyKind::PiperVoice,
                DependencyKind::SystemVoiceEngine,
            ]
        );
    }

    /// Story 3.16: the Windows Piper check reports the eSpeak NG row from
    /// voice-me's cache — Missing with Install while nothing is found, then
    /// Ready naming the unpacked program.
    #[cfg(target_os = "windows")]
    #[test]
    fn a_windows_piper_check_finds_espeak_ng_in_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let _env = crate::test_support::EnvGuard::new().set(assets::CACHE_ROOT_ENV, dir.path());
        let espeak_row = || {
            let (tx, mut rx) = futures::channel::mpsc::unbounded();
            DepsAdapter::new()
                .check(piper_request(Some(assets::PIPER_DEFAULT_VOICE.key)), tx)
                .unwrap();
            let Ok(AppEvent::DependencyCheckCompleted { report }) = rx.try_recv() else {
                panic!("the check reports by event");
            };
            row(&report.dependencies, DependencyKind::SystemVoiceEngine).clone()
        };

        if voice_me_espeak::find_program().is_none() {
            let missing = espeak_row();
            assert!(missing.status.is_missing());
            if cfg!(target_arch = "x86_64") {
                assert!(missing.automatable, "Install fetches the pinned MSI");
            }
        }

        let program = assets::espeak_program(&assets::espeak_dir(dir.path()));
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::write(&program, b"MZ").unwrap();

        let ready = espeak_row();
        assert_eq!(ready.status, voice_me_core::DependencyStatus::Ready);
        assert!(
            ready.detail.contains(&program.display().to_string()),
            "{}",
            ready.detail
        );
    }

    /// Install on the voice row for a voice voice-me knows nothing about
    /// fails with one sentence, reported once — nothing is fetched.
    #[test]
    fn installing_an_unknown_piper_voice_is_refused_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let files = assets::piper_voice_files(dir.path(), "tr_TR-dfki-medium").unwrap();
        std::fs::create_dir_all(&files.dir).unwrap();
        std::fs::write(&files.model, b"m").unwrap();
        std::fs::write(&files.config, b"c").unwrap();
        std::fs::write(
            &files.manifest,
            "name = \"dfki\"\nlocale = \"tr_TR\"\nlabel = \"Turkish\"\nquality = \"medium\"\n\
             source = \"voice-me\"\n",
        )
        .unwrap();
        let _env = EnvGuard::new().set(assets::CACHE_ROOT_ENV, dir.path());
        let (tx, mut rx) = futures::channel::mpsc::unbounded();

        let error = DepsAdapter::new()
            .provision(
                DependencyKind::PiperVoice,
                piper_request(Some("xx_XX-unknown-low")),
                tx,
            )
            .unwrap_err()
            .to_string();

        assert!(error.contains("xx_XX-unknown-low"), "{error}");
        let mut finished = 0;
        while let Ok(event) = rx.try_recv() {
            if matches!(event, AppEvent::ProvisioningFinished { .. }) {
                finished += 1;
            }
        }
        assert_eq!(finished, 1);
    }

    /// The Piper voices tab through the port: the catalogs are fetched,
    /// then Download of a listed, non-default official voice installs it
    /// and reports exactly one successful finish.
    #[test]
    fn the_catalog_port_lists_and_installs_an_official_voice() {
        use md5::Digest as _;
        use std::collections::HashMap;

        use crate::provision_tests::TestServer;

        let md5 = |bytes: &[u8]| -> String {
            md5::Md5::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        };
        let model = vec![3_u8; 4_000];
        let config = b"{\"audio\":{\"sample_rate\":22050}}".to_vec();
        let base = "tr/tr_TR/dfki/medium/tr_TR-dfki-medium";
        let voices_json = serde_json::json!({
            "tr_TR-dfki-medium": {
                "key": "tr_TR-dfki-medium",
                "name": "dfki",
                "language": {
                    "code": "tr_TR",
                    "family": "tr",
                    "region": "TR",
                    "name_native": "Türkçe",
                    "name_english": "Turkish",
                    "country_english": "Turkey"
                },
                "quality": "medium",
                "num_speakers": 1,
                "speaker_id_map": {},
                "files": {
                    format!("{base}.onnx"): {
                        "size_bytes": model.len(), "md5_digest": md5(&model)
                    },
                    format!("{base}.onnx.json"): {
                        "size_bytes": config.len(), "md5_digest": md5(&config)
                    }
                },
                "aliases": []
            }
        })
        .to_string();
        let server = TestServer::start(HashMap::from([
            ("voices.json".to_string(), voices_json.into_bytes()),
            (format!("files/{base}.onnx"), model.clone()),
            (format!("files/{base}.onnx.json"), config.clone()),
        ]));
        let cache = tempfile::tempdir().unwrap();
        let _env = EnvGuard::new()
            .unset("http_proxy")
            .unset("HTTP_PROXY")
            .unset("all_proxy")
            .unset("ALL_PROXY")
            .set(assets::CACHE_ROOT_ENV, cache.path());
        let adapter = DepsAdapter::new().with_piper_sources(PiperSources {
            voice_me_catalog: format!("{}/missing-catalog.json", server.base),
            official_voices: format!("{}/voices.json", server.base),
            official_files: format!("{}/files", server.base),
            speaches_list: format!("{}/missing-list", server.base),
            ..PiperSources::pinned()
        });

        let catalogs = adapter.fetch_catalog();
        let entry = catalogs
            .iter()
            .filter_map(|catalog| catalog.result.as_ref().ok())
            .flatten()
            .find(|entry| entry.key == "tr_TR-dfki-medium")
            .cloned()
            .expect("the official catalog lists dfki");
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        adapter.install(&entry, tx).unwrap();

        let installed = assets::installed_piper_voices(cache.path());
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].key, "tr_TR-dfki-medium");
        let mut finished = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let AppEvent::PiperVoiceFinished { key, result } = event {
                finished.push((key, result));
            }
        }
        assert_eq!(finished, vec![("tr_TR-dfki-medium".to_string(), Ok(()))]);
    }
}
