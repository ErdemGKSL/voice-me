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
    AppEvent, AppEventSender, CheckRequest, Dependency, DependencyKind, DependencyProvisioningPort,
    DependencyReport, SpeechBackend, SpeechExecutionTarget, SpeechWeights, VoiceMeError, assets,
};

use crate::capability::{GpuProbe, SystemGpuProbe};

use crate::provision::ProgressReporter;
use crate::sources::{PlannedDownload, Sources};

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
    /// What Install on the Virtual Microphone row runs. The audio crate's
    /// own `install()` in the app; injectable so the row's provisioning
    /// path is testable without an audio server.
    virtual_mic_installer: Arc<VirtualMicInstaller>,
    /// What the capability row asks the hardware (Story 3.3). The real
    /// driver and Vulkan probes in the app; injectable so every capability
    /// row is testable with no GPU.
    gpu_probe: Arc<dyn GpuProbe>,
}

type VirtualMicInstaller = dyn Fn() -> Result<(), VoiceMeError> + Send + Sync;

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
            virtual_mic_installer: Arc::new(install_virtual_microphone),
            gpu_probe: Arc::new(SystemGpuProbe),
        }
    }

    /// Replace what the capability row asks the hardware.
    pub fn with_gpu_probe(mut self, probe: impl GpuProbe + 'static) -> Self {
        self.gpu_probe = Arc::new(probe);
        self
    }

    /// Replace what Install on the Virtual Microphone row runs.
    pub fn with_virtual_mic_installer(
        mut self,
        installer: impl Fn() -> Result<(), VoiceMeError> + Send + Sync + 'static,
    ) -> Self {
        self.virtual_mic_installer = Arc::new(installer);
        self
    }

    fn provision_row(
        &self,
        kind: DependencyKind,
        backend: SpeechBackend,
        events: &AppEventSender,
    ) -> Result<(), VoiceMeError> {
        match kind {
            DependencyKind::ModelWeights => {
                let root = assets::model_cache_root()?;
                let plan = self.sources.model_plan(&root, backend.weights)?;
                fetch(kind, &plan, events)
            }
            DependencyKind::OnnxRuntime => self.provision_runtime(backend, events),
            DependencyKind::VirtualMicrophone => {
                // No bytes to count; the row says "installing" with no
                // figure until it finishes.
                let _ = events.unbounded_send(AppEvent::ProvisioningProgress {
                    kind,
                    done_bytes: 0,
                    total_bytes: 0,
                });
                (self.virtual_mic_installer)()
            }
            // A system package: its row has manual steps, never Install.
            DependencyKind::SystemVoiceEngine => Err(VoiceMeError::Other(
                "voice-me cannot install eSpeak NG; follow the steps on the row.".to_string(),
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

    /// Decision 1: the runtime installs automatically on Linux x64 only,
    /// and only into the cache — never over a path `ORT_DYLIB_PATH` names.
    fn provision_runtime(
        &self,
        backend: SpeechBackend,
        events: &AppEventSender,
    ) -> Result<(), VoiceMeError> {
        let root = assets::model_cache_root()?;
        if backend.target != SpeechExecutionTarget::Cpu {
            return Err(VoiceMeError::Other(gpu_runtime_unavailable(backend)));
        }
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
        if resolved.path.exists() {
            return Ok(());
        }
        let Some(runtime) = self.sources.runtime.as_ref() else {
            return Err(VoiceMeError::Other(
                "voice-me cannot install ONNX Runtime on this system; follow the steps on the row."
                    .to_string(),
            ));
        };

        // A verified archive left by a run whose extraction failed is not
        // fetched again.
        let archive = runtime.archive.destination(&root);
        if !archive.exists() {
            let plan = [PlannedDownload {
                asset: runtime.archive.clone(),
                destination: archive.clone(),
            }];
            fetch(DependencyKind::OnnxRuntime, &plan, events)?;
        }
        provision::extract_runtime_library(&archive, &runtime.library_entry, &resolved.path)?;
        // Our own download, of no further use once the library is out of it.
        let _ = std::fs::remove_file(&archive);
        Ok(())
    }
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
        let engine_rows = match request.selection.local_target() {
            Some(_) => speech_engine_rows(
                &root,
                request.backend,
                request.selection.added_runtime(),
                &self.sources,
            ),
            // Story 3.12: the System voice's one engine row. It is not an
            // ONNX target, so it has no runtime or model rows.
            None if request.selection.is_stock_voice() => system_voice_rows(),
            None => Vec::new(),
        };

        let report = DependencyReport::new(
            request.backend,
            capability
                .into_iter()
                .chain(engine_rows)
                .chain(virtual_microphone_row())
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
        backend: SpeechBackend,
        events: AppEventSender,
    ) -> Result<(), VoiceMeError> {
        // A second Install on a row already being installed: the first run
        // stays the only one reporting, and nothing is sent for this one.
        let Some(claim) = InFlight::claim(&self.in_flight, kind) else {
            return Ok(());
        };

        let result = self.provision_row(kind, backend, &events);
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

/// Removes its row from the in-flight set when dropped — including when the
/// provisioning job panics, so a crashed run never leaves Install dead.
struct InFlight {
    set: Arc<Mutex<HashSet<DependencyKind>>>,
    kind: DependencyKind,
}

impl InFlight {
    fn claim(set: &Arc<Mutex<HashSet<DependencyKind>>>, kind: DependencyKind) -> Option<Self> {
        let mut rows = set.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        rows.insert(kind).then(|| Self {
            set: set.clone(),
            kind,
        })
    }
}

impl Drop for InFlight {
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
    // Nothing to fetch, nothing to report: the reporter's constructor
    // would otherwise send a meaningless 0-of-0 figure.
    if plan.is_empty() {
        return Ok(());
    }
    let mut progress = ProgressReporter::for_plan(kind, events.clone(), plan);
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
    vec![
        runtime_row(root, backend, added, sources.runtime.is_some()),
        model_weights_row(root, backend.weights),
    ]
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
    installable: bool,
) -> Dependency {
    let resolved = assets::resolve_runtime_dylib(root, added);
    let path = resolved.path.display();

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

    // Decision 3: a GPU backend's runtime has no source until Story 3.8.
    if backend.target != SpeechExecutionTarget::Cpu {
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
    if installable {
        Dependency::missing(DependencyKind::OnnxRuntime, RUNTIME_LABEL, detail)
    } else {
        // Decision 1: only Linux x64 has an automatic runtime install.
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

/// The Virtual Microphone row — Linux only (Decision 1).
///
/// The status is *asked of* `voice-me-audio-linux` rather than detected
/// again here: that crate already owns every line of PipeWire knowledge in
/// this workspace, and a second copy of it would be a second thing to be
/// wrong. Windows gets no row at all until Story 2.8 gives the Windows
/// driver a control surface worth reporting on.
#[cfg(target_os = "linux")]
fn virtual_microphone_row() -> Option<Dependency> {
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

#[cfg(not(target_os = "linux"))]
fn virtual_microphone_row() -> Option<Dependency> {
    None
}

/// The System voice's engine row (Story 3.12): whether `espeak-ng` is on
/// PATH, asked of the crate that runs it, with the distribution's install
/// command when it is not.
#[cfg(target_os = "linux")]
fn system_voice_rows() -> Vec<Dependency> {
    let found = voice_me_tts_system_linux::find_program();
    let install = if found.is_some() {
        String::new()
    } else {
        voice_me_tts_system_linux::install_step()
    };
    vec![capability::system_voice_engine_row(
        found.as_deref(),
        &install,
    )]
}

/// Off Linux the capability row says the System voice cannot run yet; there
/// is no engine to report on.
#[cfg(not(target_os = "linux"))]
fn system_voice_rows() -> Vec<Dependency> {
    Vec::new()
}

/// Install reuses the audio crate's own idempotent `install()`: it ends
/// with exactly one device, whatever it started with.
#[cfg(target_os = "linux")]
fn install_virtual_microphone() -> Result<(), VoiceMeError> {
    voice_me_audio_linux::LinuxVirtualMicAdapter::new()
        .and_then(|adapter| adapter.install())
        .map(|_| ())
        .map_err(|error| {
            VoiceMeError::Other(format!("Could not install the Virtual Microphone: {error}"))
        })
}

#[cfg(not(target_os = "linux"))]
fn install_virtual_microphone() -> Result<(), VoiceMeError> {
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

    /// Decision 3: a GPU backend's runtime has no source until Story 3.8,
    /// so its row is manual and says so rather than offering an Install
    /// that would fetch the CPU runtime.
    #[test]
    fn a_gpu_backends_missing_runtime_is_manual() {
        let _env = no_configured_runtime();
        let dir = tempfile::tempdir().unwrap();
        let backend = SpeechBackend {
            target: SpeechExecutionTarget::WebGpu,
            device: None,
            weights: SpeechWeights::Fp16,
        };

        let rows = speech_engine_rows(dir.path(), backend, None, &Sources::pinned());
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

    /// Decision 1's other half: Windows gets no Virtual Microphone row at
    /// all until Story 2.8 gives the driver a control surface worth
    /// reporting on. Runs on the Windows CI job; on Linux the row above is
    /// what is exercised instead.
    #[test]
    #[cfg(not(target_os = "linux"))]
    fn no_virtual_microphone_row_is_reported_off_linux() {
        assert!(virtual_microphone_row().is_none());
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
                voice_me_tts_system_linux::find_program().is_none()
            );
        }
        #[cfg(not(target_os = "linux"))]
        assert_eq!(
            report.dependencies[0].kind,
            DependencyKind::BackendCapability
        );
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
}
