//! `voice-me-tts` — the `TtsPort` adapter: Chatterbox-Multilingual V3
//! running **in-process** on ONNX Runtime (AD-12).
//!
//! There is no sidecar process and no Python anywhere in this pipeline;
//! spec-2-5 exists to prove that, and these modules are the proof. The crate
//! reads its model files from a cache directory and never downloads them
//! (AD-8) — a file that is not there is reported by path, which is exactly
//! the list Story 3.2's provisioning work has to satisfy.
//!
//! Story 2.6 adds the lifecycle spec-2-5 deliberately left open: the four
//! sessions are built once and held for the process lifetime (AD-10), and
//! generations are serialized so exactly one ever runs at a time.

pub mod generate;
pub mod reference;
pub mod sessions;
pub mod tokenizer;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use voice_me_core::{
    AppEvent, AppEventSender, AppState, AudioBuffer, SpeechBackend, SpeechExecutionTarget,
    SpeechWeights, TtsPort, VoiceMeError, assets,
};

pub use generate::{GenerationOutcome, GenerationSettings, generate};
pub use sessions::{ExecutionTarget, LanguageModel, ModelCache, Sessions};

/// AD-10's whole contract in one place: something expensive is built at most
/// once, held for the process lifetime, and used by one caller at a time.
///
/// **The mutex is the queue.** `generate::generate` takes `&mut Sessions`, so
/// exclusive use is forced anyway; holding the lock across the entire
/// generation turns that into "one at a time, the second one waits" with no
/// channel, no worker thread and no ordering machinery to get wrong.
///
/// Generic over what it holds purely so this behaviour is testable without
/// 1.56 GB of model files — `Sessions` cannot be constructed in a unit test,
/// and the part worth testing was never ONNX-specific.
struct SessionSlot<T> {
    held: Mutex<Option<T>>,
    /// Mirrors "the slot is populated", readable without touching the mutex.
    ///
    /// Separate from the slot on purpose: the lock is held for the entire
    /// duration of a generation, so answering `is_ready` by trying to lock
    /// would report "not ready" throughout a perfectly ordinary utterance
    /// and fire the still-loading notification for nothing.
    ready: AtomicBool,
}

impl<T> SessionSlot<T> {
    fn new() -> Self {
        Self {
            held: Mutex::new(None),
            ready: AtomicBool::new(false),
        }
    }

    /// Whether the held value already exists. Never blocks.
    fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    /// Take the lock, recovering from poisoning.
    ///
    /// A panic inside one generation would otherwise poison the slot and
    /// make every later Speak Action fail forever — a far worse outcome than
    /// reusing sessions a panicking run touched. `Sessions` holds ORT
    /// handles and a tokenizer, none of which a partial run leaves in a
    /// state the next run can observe.
    fn lock(&self) -> MutexGuard<'_, Option<T>> {
        self.held
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Run `use_it` against the held value, building it with `build` first
    /// if this is the first call to get here.
    ///
    /// A `build` that fails leaves the slot empty, so the next caller tries
    /// again: provisioning the missing file is exactly the fix for the most
    /// likely failure, and a remembered error would survive the fix.
    fn with<R>(
        &self,
        build: impl FnOnce() -> Result<T, VoiceMeError>,
        use_it: impl FnOnce(&mut T) -> Result<R, VoiceMeError>,
    ) -> Result<R, VoiceMeError> {
        let mut held = self.lock();
        if held.is_none() {
            held.replace(build()?);
            self.ready.store(true, Ordering::SeqCst);
        }
        use_it(held.as_mut().expect("just built"))
    }

    /// Build the held value if it is not there yet, and do nothing else.
    fn ensure(&self, build: impl FnOnce() -> Result<T, VoiceMeError>) -> Result<(), VoiceMeError> {
        self.with(build, |_| Ok(()))
    }
}

/// `TtsPort` adapter over the in-process engine.
///
/// Holds the four ONNX Runtime sessions for the process lifetime, because
/// building them costs 86–110 s against a ~20 s utterance — spec-2-5's
/// measurements are what AD-10's build-once/hold rule rests on.
pub struct TtsAdapter {
    cache: ModelCache,
    /// The ONNX Runtime library this adapter loads, resolved once at
    /// construction by the same rule the Dependency Check reports on.
    runtime: PathBuf,
    backend: SpeechBackend,
    variant: LanguageModel,
    target: ExecutionTarget,
    sessions: SessionSlot<Sessions>,
    /// Where each session build is reported (Story 3.3): the only source
    /// of "Active" on the backend line.
    events: Option<AppEventSender>,
    /// The generation the composition root gave this adapter, carried on
    /// every report so a replaced adapter's late report can be told apart.
    generation: u64,
}

impl TtsAdapter {
    /// Build the adapter for the backend `state` resolved to (AD-9), loading
    /// the runtime library its selection names.
    ///
    /// `AppState` is the *only* place this crate reads its execution target,
    /// weight variant and runtime library from — it never calls
    /// `voice-me-deps`. No session is built here; that is
    /// [`TtsPort::warm_up`]'s job, so constructing the adapter stays free
    /// and a first-run user pays nothing.
    pub fn from_state(state: &AppState) -> Result<Self, VoiceMeError> {
        let cache = ModelCache::from_env()?;
        let runtime =
            assets::resolve_runtime_dylib(cache.root(), state.backend_selection.added_runtime())
                .path;
        Ok(Self::new(cache, state.speech_backend).with_runtime(runtime))
    }

    /// The same, against an explicit cache directory — used in tests, and by
    /// anything that does not want `VOICE_ME_MODEL_CACHE`'s answer. Loads
    /// the cache root's own runtime copy unless [`Self::with_runtime`] says
    /// otherwise.
    pub fn new(cache: ModelCache, backend: SpeechBackend) -> Self {
        Self {
            runtime: assets::bundled_runtime_dylib(cache.root()),
            cache,
            backend,
            variant: language_model_for(backend.weights),
            target: execution_target_for(backend),
            sessions: SessionSlot::new(),
            events: None,
            generation: 0,
        }
    }

    /// Load the runtime library at `runtime` instead.
    pub fn with_runtime(mut self, runtime: PathBuf) -> Self {
        self.runtime = runtime;
        self
    }

    /// Report every session build on `events` as
    /// [`AppEvent::SpeechSessionBuilt`], tagged with `generation`.
    pub fn with_events(mut self, events: AppEventSender, generation: u64) -> Self {
        self.events = Some(events);
        self.generation = generation;
        self
    }

    /// Which `language_model` this adapter loads.
    pub fn variant(&self) -> LanguageModel {
        self.variant
    }

    /// Where it places the graphs.
    pub fn target(&self) -> ExecutionTarget {
        self.target
    }

    /// Which runtime library it loads.
    pub fn runtime(&self) -> &Path {
        &self.runtime
    }

    /// The build step, as a closure the session slot can call under its lock.
    ///
    /// Every attempt is reported, success and failure alike. A success names
    /// `self.backend` because nothing else can have been built: the GPU
    /// providers are registered with `error_on_failure`, so a target that
    /// cannot be reached fails the build rather than landing on CPU.
    fn build_sessions(&self) -> impl FnOnce() -> Result<Sessions, VoiceMeError> + '_ {
        move || {
            let built = self.build_sessions_unreported();
            if let Some(events) = self.events.as_ref() {
                let _ = events.unbounded_send(AppEvent::SpeechSessionBuilt {
                    generation: self.generation,
                    result: built
                        .as_ref()
                        .map(|_| self.backend)
                        .map_err(ToString::to_string),
                });
            }
            built
        }
    }

    fn build_sessions_unreported(&self) -> Result<Sessions, VoiceMeError> {
        // Model files before the runtime library, the same order the
        // spike example reports them in: the nine-file model list is the
        // larger and likelier-incomplete half of provisioning, and its
        // error names an exact absolute path, which is the single most
        // useful thing to put in front of someone.
        self.cache.check(self.variant)?;
        // Committing the ONNX Runtime environment is idempotent and
        // cheap. It happens here rather than at startup so a build with
        // nothing provisioned yet fails at the moment something is
        // actually asked of the engine, naming the dylib, instead of at
        // launch.
        sessions::init_runtime(&self.runtime)?;

        let preload_failures = prepare_target(self.cache.root(), &self.runtime, self.target)?;
        Sessions::build(&self.cache, self.variant, self.target, false)
            .map_err(|error| with_preload_failures(error, &preload_failures))
    }
}

/// The execution providers another engine's session is built with — Piper's
/// (spec-backend-engine-and-device-selects) — on `backend`'s target, on the
/// runtime at `runtime` under the cache `root`, which the caller has
/// already committed with [`sessions::init_runtime`]. The same providers,
/// restart rule and NVIDIA preload Chatterbox's sessions get. Returns the
/// providers and, for CUDA, the NVIDIA libraries that did not load, which
/// the caller adds to a failed build's error.
pub fn session_providers(
    root: &Path,
    runtime: &Path,
    backend: SpeechBackend,
) -> Result<(Vec<ort::ep::ExecutionProviderDispatch>, Vec<String>), VoiceMeError> {
    let target = execution_target_for(backend);
    let preload_failures = prepare_target(root, runtime, target)?;
    Ok((target.providers()?, preload_failures))
}

/// What a session build on `target` needs first, on the committed runtime
/// at `runtime` under the cache `root`. Story 3.8: on the bundled runtime
/// the GPU providers come from voice-me's own build, and one that Install
/// put in place after this process loaded the old library only runs after
/// a restart. CUDA's NVIDIA libraries are then loaded from the cache before
/// the provider is registered; a library the user added or configured finds
/// its own, exactly as before. Returns the NVIDIA libraries that did not
/// load.
fn prepare_target(
    root: &Path,
    runtime: &Path,
    target: ExecutionTarget,
) -> Result<Vec<String>, VoiceMeError> {
    let bundled = assets::is_bundled_runtime(root, runtime);
    let gpu = target != ExecutionTarget::Cpu;
    if replaced_runtime_needs_restart(bundled, gpu, sessions::committed_runtime_replaced()) {
        return Err(VoiceMeError::SpeechEngine(format!(
            "ONNX Runtime at {} was replaced since voice-me loaded it; restart voice-me to use \
             the new one",
            runtime.display()
        )));
    }
    Ok(match preload_dir(root, runtime, target) {
        Some(dir) => {
            sessions::preload_cuda_libraries(&dir, &assets::installed_cuda_libraries(root))
        }
        None => Vec::new(),
    })
}

/// Story 3.8: the one restart rule for a runtime Install replaced on disk
/// after this process loaded it. Only the bundled runtime is replaced, and
/// only a GPU target needs the new one — CPU runs on the old library.
pub fn replaced_runtime_needs_restart(bundled: bool, gpu: bool, replaced: bool) -> bool {
    bundled && gpu && replaced
}

/// Where the NVIDIA libraries are loaded from before a session is built:
/// `<root>/runtime/cuda/` for CUDA on the bundled runtime, nowhere
/// otherwise.
fn preload_dir(root: &Path, runtime: &Path, target: ExecutionTarget) -> Option<PathBuf> {
    (assets::is_bundled_runtime(root, runtime) && matches!(target, ExecutionTarget::Cuda { .. }))
        .then(|| assets::cuda_libraries_dir(root))
}

/// The engine's error, with the NVIDIA libraries that did not load named
/// after it — usually the reason the CUDA provider failed.
fn with_preload_failures(error: VoiceMeError, failures: &[String]) -> VoiceMeError {
    match error {
        VoiceMeError::SpeechEngine(reason) if !failures.is_empty() => {
            VoiceMeError::SpeechEngine(format!(
                "{reason} (these NVIDIA libraries did not load: {})",
                failures.join("; ")
            ))
        }
        error => error,
    }
}

impl TtsPort for TtsAdapter {
    fn warm_up(&self) -> Result<(), VoiceMeError> {
        self.sessions.ensure(self.build_sessions())
    }

    fn is_ready(&self) -> bool {
        self.sessions.is_ready()
    }

    fn generate(
        &self,
        text: &str,
        reference_clip: Option<&Path>,
        language: &str,
        _voice: Option<&str>,
    ) -> Result<AudioBuffer, VoiceMeError> {
        // Both of these happen deliberately outside the lock. Rejecting
        // empty text before anything expensive is the I/O matrix's own rule,
        // and decoding the reference clip is per-utterance work with no
        // reason to hold a queued Speak Action back.
        if text.trim().is_empty() {
            return Err(VoiceMeError::EmptyText);
        }
        // A cloning engine: with no sample there is no voice to clone.
        let reference_clip = reference_clip.ok_or(VoiceMeError::NoReferenceVoiceSample)?;
        let reference = reference::load_reference_clip(reference_clip)?;

        self.sessions
            .with(self.build_sessions(), |sessions| {
                generate::generate(
                    sessions,
                    text,
                    language,
                    &reference,
                    GenerationSettings::default(),
                )
            })
            .map(|outcome| outcome.audio)
    }
}

/// `AppState`'s weight variant → this crate's private vocabulary.
fn language_model_for(weights: SpeechWeights) -> LanguageModel {
    match weights {
        SpeechWeights::Q4 => LanguageModel::Q4,
        SpeechWeights::Fp16 => LanguageModel::Fp16,
        SpeechWeights::Fp32 => LanguageModel::Fp32,
    }
}

/// `AppState`'s backend → this crate's private vocabulary.
///
/// A device id only means something to a multi-device provider; ONNX
/// Runtime's CPU provider has no such concept, so a device recorded
/// alongside `Cpu` is ignored rather than refused — it is stale state from a
/// previous detection, not a user error.
///
/// Every target maps to itself and nothing else: there is no fallback here,
/// so a GPU selection that cannot run fails as that GPU selection.
fn execution_target_for(backend: SpeechBackend) -> ExecutionTarget {
    let device_id = backend.device.unwrap_or(0) as i32;
    match backend.target {
        SpeechExecutionTarget::Cpu => ExecutionTarget::Cpu,
        SpeechExecutionTarget::Cuda => ExecutionTarget::Cuda { device_id },
        SpeechExecutionTarget::WebGpu => ExecutionTarget::WebGpu { device_id },
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use super::*;

    // ---- the session slot: AD-10's build-once/hold/serialize contract ----
    //
    // Exercised against a stand-in type rather than `Sessions`, so the whole
    // contract is covered with no model files, no ONNX Runtime library and
    // no network.

    #[test]
    fn the_held_value_is_built_once_and_reused() {
        let slot: SessionSlot<u32> = SessionSlot::new();
        let builds = AtomicUsize::new(0);
        let build = || {
            builds.fetch_add(1, Ordering::SeqCst);
            Ok(7_u32)
        };

        assert!(!slot.is_ready(), "nothing is built until something asks");

        assert_eq!(slot.with(build, |held| Ok(*held)).unwrap(), 7);
        assert!(slot.is_ready());
        assert_eq!(slot.with(build, |held| Ok(*held)).unwrap(), 7);

        assert_eq!(
            builds.load(Ordering::SeqCst),
            1,
            "a second Speak Action runs on the same sessions — never a second build"
        );
    }

    #[test]
    fn a_warm_up_before_the_first_use_means_the_use_builds_nothing() {
        let slot: SessionSlot<u32> = SessionSlot::new();
        let builds = AtomicUsize::new(0);
        let build = || {
            builds.fetch_add(1, Ordering::SeqCst);
            Ok(7_u32)
        };

        slot.ensure(build).unwrap();
        assert!(slot.is_ready());
        slot.with(build, |_| Ok(())).unwrap();

        assert_eq!(builds.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failed_build_is_retried_rather_than_remembered() {
        let slot: SessionSlot<u32> = SessionSlot::new();
        let attempts = AtomicUsize::new(0);
        let build = || {
            // Fail once, then succeed — provisioning the missing file is
            // precisely the fix, so a cached error would survive the fix.
            if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(VoiceMeError::MissingRuntimeAsset {
                    path: PathBuf::from("/cache/onnx/language_model_q4.onnx_data"),
                })
            } else {
                Ok(7_u32)
            }
        };

        assert!(slot.with(build, |_| Ok(())).is_err());
        assert!(
            !slot.is_ready(),
            "a failed build must not leave the slot claiming it is ready"
        );
        assert_eq!(slot.with(build, |held| Ok(*held)).unwrap(), 7);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    /// The I/O matrix's "second Speak mid-generation" row: one generation is
    /// in flight, a second arrives, and the two must never overlap.
    #[test]
    fn a_second_use_waits_for_the_first_and_then_runs() {
        let slot: Arc<SessionSlot<u32>> = Arc::new(SessionSlot::new());
        let in_flight = Arc::new(AtomicUsize::new(0));
        let overlapped = Arc::new(AtomicBool::new(false));
        let start = Arc::new(std::sync::Barrier::new(2));

        let handles: Vec<_> = (0..2)
            .map(|_| {
                let slot = slot.clone();
                let in_flight = in_flight.clone();
                let overlapped = overlapped.clone();
                let start = start.clone();
                std::thread::spawn(move || {
                    start.wait();
                    slot.with(
                        || Ok(7_u32),
                        |held| {
                            if in_flight.fetch_add(1, Ordering::SeqCst) != 0 {
                                overlapped.store(true, Ordering::SeqCst);
                            }
                            std::thread::sleep(Duration::from_millis(30));
                            in_flight.fetch_sub(1, Ordering::SeqCst);
                            Ok(*held)
                        },
                    )
                })
            })
            .collect();

        for handle in handles {
            assert_eq!(
                handle.join().unwrap().unwrap(),
                7,
                "both Speak Actions produce audio — the second is queued, not dropped"
            );
        }
        assert!(
            !overlapped.load(Ordering::SeqCst),
            "the sessions mutex is AD-10's queue: two generations must never overlap"
        );
    }

    /// The race `TtsPort::warm_up`'s own documentation promises is safe:
    /// warm-up and a Speak Action arriving together must not produce two
    /// sets of sessions.
    #[test]
    fn a_warm_up_racing_a_generation_still_builds_only_one_set() {
        let slot: Arc<SessionSlot<u32>> = Arc::new(SessionSlot::new());
        let builds = Arc::new(AtomicUsize::new(0));
        let start = Arc::new(std::sync::Barrier::new(2));

        let build = {
            let builds = builds.clone();
            move || {
                builds.fetch_add(1, Ordering::SeqCst);
                // Wide enough that a second builder would overlap it rather
                // than tidily following it.
                std::thread::sleep(Duration::from_millis(30));
                Ok(7_u32)
            }
        };

        let warming = {
            let slot = slot.clone();
            let build = build.clone();
            let start = start.clone();
            std::thread::spawn(move || {
                start.wait();
                slot.ensure(build)
            })
        };
        let generating = {
            let slot = slot.clone();
            let start = start.clone();
            std::thread::spawn(move || {
                start.wait();
                slot.with(build, |held| Ok(*held))
            })
        };

        warming.join().unwrap().unwrap();
        assert_eq!(generating.join().unwrap().unwrap(), 7);
        assert_eq!(
            builds.load(Ordering::SeqCst),
            1,
            "whichever arrived second had to wait, not build a second set"
        );
    }

    #[test]
    fn a_panicking_use_does_not_wedge_the_slot_for_every_later_action() {
        let slot: Arc<SessionSlot<u32>> = Arc::new(SessionSlot::new());

        let poisoner = {
            let slot = slot.clone();
            std::thread::spawn(move || slot.with(|| Ok(7_u32), |_| -> Result<(), _> { panic!() }))
        };
        assert!(poisoner.join().is_err());

        assert_eq!(
            slot.with(|| Ok(1_u32), |held| Ok(*held)).unwrap(),
            7,
            "the held sessions are still usable — poisoning must not be permanent"
        );
    }

    // ---- the adapter itself ---------------------------------------------

    fn adapter_over_an_empty_cache() -> (tempfile::TempDir, TtsAdapter) {
        let dir = tempfile::tempdir().unwrap();
        let adapter = TtsAdapter::new(
            ModelCache::new(dir.path().to_path_buf()),
            SpeechBackend::CPU,
        );
        (dir, adapter)
    }

    #[test]
    fn a_fresh_adapter_has_built_nothing() {
        let (_dir, adapter) = adapter_over_an_empty_cache();

        assert!(
            !adapter.is_ready(),
            "construction must stay free — the 90 s build belongs to warm_up"
        );
    }

    #[test]
    fn the_backend_in_app_state_is_what_gets_loaded() {
        let cache = ModelCache::new(PathBuf::from("/cache"));

        let cpu = TtsAdapter::new(cache.clone(), SpeechBackend::CPU);
        assert_eq!(cpu.variant(), LanguageModel::Q4);
        assert_eq!(cpu.target(), ExecutionTarget::Cpu);

        let gpu = TtsAdapter::new(
            cache,
            SpeechBackend {
                target: SpeechExecutionTarget::WebGpu,
                device: Some(1),
                weights: SpeechWeights::Fp16,
            },
        );
        assert_eq!(gpu.variant(), LanguageModel::Fp16);
        assert_eq!(gpu.target(), ExecutionTarget::WebGpu { device_id: 1 });
    }

    #[test]
    fn the_shipped_cpu_default_is_q4() {
        let adapter = TtsAdapter::new(
            ModelCache::new(PathBuf::from("/cache")),
            AppState::default().speech_backend,
        );

        assert_eq!(
            adapter.variant(),
            LanguageModel::Q4,
            "spec-2-6 Decision 1: 354 MB and ~18 % faster, against FP32's 2.08 GB"
        );
    }

    /// Story 3.12: the port's sample became optional; this cloning engine
    /// still needs one, and says so before any session work.
    #[test]
    fn no_reference_clip_is_refused_before_any_session_work() {
        let (_dir, adapter) = adapter_over_an_empty_cache();

        let error = adapter.generate("Merhaba", None, "tr", None).unwrap_err();

        assert!(matches!(error, VoiceMeError::NoReferenceVoiceSample));
        assert!(!adapter.is_ready());
    }

    #[test]
    fn empty_text_is_rejected_before_any_session_work() {
        let (_dir, adapter) = adapter_over_an_empty_cache();

        let error = adapter
            .generate("   ", Some(Path::new("/data/reference.wav")), "tr", None)
            .unwrap_err();

        assert!(matches!(error, VoiceMeError::EmptyText));
        assert!(
            !adapter.is_ready(),
            "an empty line must not trigger the session build"
        );
    }

    #[test]
    fn a_missing_model_file_is_reported_by_absolute_path_not_a_panic() {
        let (dir, adapter) = adapter_over_an_empty_cache();

        let error = adapter.warm_up().unwrap_err();

        let VoiceMeError::MissingRuntimeAsset { path } = error else {
            panic!("expected MissingRuntimeAsset, got {error:?}");
        };
        assert!(
            path.starts_with(dir.path()),
            "the path the user has to go fetch must survive to the surface: {}",
            path.display()
        );
        assert!(!adapter.is_ready());
    }

    // ---- Story 3.3: honest reporting of what was built -------------------

    #[test]
    fn every_target_maps_to_itself_and_never_to_cpu() {
        let cache = ModelCache::new(PathBuf::from("/cache"));
        let cuda = TtsAdapter::new(
            cache,
            SpeechBackend::for_target(SpeechExecutionTarget::Cuda),
        );

        assert_eq!(cuda.target(), ExecutionTarget::Cuda { device_id: 0 });
        assert_eq!(cuda.variant(), LanguageModel::Fp16);
    }

    /// A CUDA selection whose runtime cannot be loaded reports `Failed`
    /// with the engine's reason — and never an acquired CPU session.
    #[test]
    fn an_unreachable_target_reports_failed_and_never_cpu() {
        let _lock = sessions::committed_test_lock();
        let dir = tempfile::tempdir().unwrap();
        for path in assets::required_model_files(dir.path(), SpeechWeights::Fp16) {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"stand-in").unwrap();
        }
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let adapter = TtsAdapter::new(
            ModelCache::new(dir.path().to_path_buf()),
            SpeechBackend::for_target(SpeechExecutionTarget::Cuda),
        )
        .with_runtime(dir.path().join("missing").join("libonnxruntime.so"))
        .with_events(tx, 7);

        assert!(adapter.warm_up().is_err());

        let Ok(AppEvent::SpeechSessionBuilt { generation, result }) = rx.try_recv() else {
            panic!("every build attempt is reported");
        };
        assert_eq!(generation, 7, "the report names the engine that sent it");
        let reason = result.expect_err("nothing was built, so nothing is active");
        assert!(reason.contains("libonnxruntime.so"), "{reason}");
        assert!(!adapter.is_ready());
    }

    /// A missing model file is a failed build too, and says so.
    #[test]
    fn a_build_that_fails_on_a_missing_file_is_reported_as_failed() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let adapter = TtsAdapter::new(
            ModelCache::new(dir.path().to_path_buf()),
            SpeechBackend::CPU,
        )
        .with_events(tx, 0);

        let _ = adapter.warm_up();

        assert!(matches!(
            rx.try_recv(),
            Ok(AppEvent::SpeechSessionBuilt { result: Err(_), .. })
        ));
    }

    /// With `ORT_DYLIB_PATH` unset and nothing added, the engine loads
    /// exactly the library the Dependency Check reports on —
    /// `<cache root>/runtime/<dylib>` — and a selected added library wins.
    #[test]
    fn the_adapter_loads_the_library_the_check_reports_on() {
        let cache = ModelCache::new(PathBuf::from("/cache"));
        let adapter = TtsAdapter::new(cache.clone(), SpeechBackend::CPU);
        assert_eq!(
            adapter.runtime(),
            assets::bundled_runtime_dylib(Path::new("/cache"))
        );

        let added = PathBuf::from("/opt/ort/libonnxruntime.so");
        let adapter = TtsAdapter::new(cache, SpeechBackend::CPU).with_runtime(added.clone());
        assert_eq!(adapter.runtime(), added);
    }

    /// `from_state` loads exactly the library the Dependency Check reports
    /// on: the selected added library when there is one, otherwise the usual
    /// rule (`ORT_DYLIB_PATH`, then the cache copy).
    ///
    /// Both variables are process-global; this is the only test in the crate
    /// that touches them, and it puts back what it found.
    #[test]
    fn from_state_loads_the_library_the_check_reports_on() {
        use std::ffi::OsString;

        struct EnvGuard(Vec<(&'static str, Option<OsString>)>);

        impl Drop for EnvGuard {
            fn drop(&mut self) {
                for (key, value) in self.0.drain(..) {
                    match value {
                        Some(value) => unsafe { std::env::set_var(key, value) },
                        None => unsafe { std::env::remove_var(key) },
                    }
                }
            }
        }

        let cache = tempfile::tempdir().unwrap();
        let configured = cache.path().join("configured").join("libonnxruntime.so");
        let _guard = EnvGuard(vec![
            (
                assets::RUNTIME_DYLIB_ENV,
                std::env::var_os(assets::RUNTIME_DYLIB_ENV),
            ),
            (
                assets::CACHE_ROOT_ENV,
                std::env::var_os(assets::CACHE_ROOT_ENV),
            ),
        ]);
        unsafe {
            std::env::set_var(assets::RUNTIME_DYLIB_ENV, &configured);
            std::env::set_var(assets::CACHE_ROOT_ENV, cache.path());
        }

        let added = PathBuf::from("/opt/ort/libonnxruntime.so");
        let with_added = AppState {
            backend_selection: voice_me_core::BackendSelection::Local {
                runtime: Some(added.clone()),
                target: SpeechExecutionTarget::Cuda,
            },
            speech_backend: SpeechBackend::for_target(SpeechExecutionTarget::Cuda),
            ..AppState::default()
        };
        assert_eq!(
            TtsAdapter::from_state(&with_added).unwrap().runtime(),
            added,
            "a selected added library wins over ORT_DYLIB_PATH"
        );

        let bundled = TtsAdapter::from_state(&AppState::default()).unwrap();
        assert_eq!(
            bundled.runtime(),
            assets::resolve_runtime_dylib(cache.path(), None).path
        );
        assert_eq!(bundled.runtime(), configured);
    }

    /// Story 3.8: NVIDIA's libraries are preloaded for CUDA on the bundled
    /// runtime only.
    #[test]
    fn only_cuda_on_the_bundled_runtime_preloads_nvidia_libraries() {
        let root = Path::new("/cache");
        let bundled = assets::bundled_runtime_dylib(root);
        let added = Path::new("/opt/ort/libonnxruntime.so");
        let cuda = ExecutionTarget::Cuda { device_id: 0 };

        assert_eq!(
            preload_dir(root, &bundled, cuda),
            Some(assets::cuda_libraries_dir(root))
        );
        assert_eq!(preload_dir(root, added, cuda), None);
        assert_eq!(preload_dir(root, &bundled, ExecutionTarget::Cpu), None);
        assert_eq!(
            preload_dir(root, &bundled, ExecutionTarget::WebGpu { device_id: 0 }),
            None
        );
    }

    /// spec-backend-engine-and-device-selects: Piper's session gets the
    /// provider its target names, and nothing is preloaded off CUDA.
    #[test]
    fn session_providers_follow_the_backend_target() {
        let _lock = sessions::committed_test_lock();
        let root = tempfile::tempdir().unwrap();
        let bundled = assets::bundled_runtime_dylib(root.path());
        for target in [
            SpeechExecutionTarget::Cpu,
            SpeechExecutionTarget::WebGpu,
            SpeechExecutionTarget::Cuda,
        ] {
            let (providers, failures) =
                session_providers(root.path(), &bundled, SpeechBackend::for_target(target))
                    .unwrap();
            assert_eq!(providers.len(), 1, "{target:?}");
            let provider = &providers[0];
            let matches = match target {
                SpeechExecutionTarget::Cpu => provider.downcast_ref::<ort::ep::CPU>().is_some(),
                SpeechExecutionTarget::Cuda => provider.downcast_ref::<ort::ep::CUDA>().is_some(),
                SpeechExecutionTarget::WebGpu => {
                    provider.downcast_ref::<ort::ep::WebGPU>().is_some()
                }
            };
            assert!(matches, "{target:?}: {provider:?}");
            // No NVIDIA record in an empty cache: nothing to preload.
            assert!(failures.is_empty(), "{target:?}: {failures:?}");
        }
    }

    #[test]
    fn nvidia_libraries_that_did_not_load_are_named_in_the_engine_error() {
        let failures = vec![
            "libcudnn.so.9: bad".to_string(),
            "libcufft.so.11: gone".to_string(),
        ];

        let merged = with_preload_failures(
            VoiceMeError::SpeechEngine("CUDA provider failed".to_string()),
            &failures,
        );

        let VoiceMeError::SpeechEngine(reason) = merged else {
            panic!("still an engine error: {merged:?}");
        };
        assert_eq!(
            reason,
            "CUDA provider failed (these NVIDIA libraries did not load: libcudnn.so.9: bad; \
             libcufft.so.11: gone)"
        );
        let untouched = with_preload_failures(VoiceMeError::SpeechEngine("fine".to_string()), &[]);
        assert!(matches!(&untouched, VoiceMeError::SpeechEngine(reason) if reason == "fine"));
        let missing = with_preload_failures(
            VoiceMeError::MissingRuntimeAsset {
                path: PathBuf::from("/x"),
            },
            &failures,
        );
        assert!(matches!(missing, VoiceMeError::MissingRuntimeAsset { .. }));
    }

    /// One rule: a replaced runtime needs a restart only when it is the
    /// bundled one and the target is a GPU.
    #[test]
    fn a_replaced_runtime_needs_a_restart_only_for_a_bundled_gpu_session() {
        assert!(replaced_runtime_needs_restart(true, true, true));
        assert!(!replaced_runtime_needs_restart(false, true, true));
        assert!(!replaced_runtime_needs_restart(true, false, true));
        assert!(!replaced_runtime_needs_restart(true, true, false));
    }
}
