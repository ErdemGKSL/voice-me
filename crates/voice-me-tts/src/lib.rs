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

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use voice_me_core::{
    AppState, AudioBuffer, SpeechBackend, SpeechExecutionTarget, SpeechWeights, TtsPort,
    VoiceMeError,
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
    variant: LanguageModel,
    target: ExecutionTarget,
    sessions: SessionSlot<Sessions>,
}

impl TtsAdapter {
    /// Build the adapter for the backend `state` resolved to (AD-9).
    ///
    /// `AppState` is the *only* place this crate reads its execution target
    /// and weight variant from — it never calls `voice-me-deps`. No session
    /// is built here; that is [`TtsPort::warm_up`]'s job, so constructing
    /// the adapter stays free and a first-run user pays nothing.
    pub fn from_state(state: &AppState) -> Result<Self, VoiceMeError> {
        Ok(Self::new(ModelCache::from_env()?, state.speech_backend))
    }

    /// The same, against an explicit cache directory — used in tests, and by
    /// anything that does not want `VOICE_ME_MODEL_CACHE`'s answer.
    pub fn new(cache: ModelCache, backend: SpeechBackend) -> Self {
        Self {
            cache,
            variant: language_model_for(backend.weights),
            target: execution_target_for(backend),
            sessions: SessionSlot::new(),
        }
    }

    /// Which `language_model` this adapter loads.
    pub fn variant(&self) -> LanguageModel {
        self.variant
    }

    /// Where it places the graphs.
    pub fn target(&self) -> ExecutionTarget {
        self.target
    }

    /// The build step, as a closure the session slot can call under its lock.
    fn build_sessions(&self) -> impl FnOnce() -> Result<Sessions, VoiceMeError> + '_ {
        move || {
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
            sessions::init_runtime(None)?;
            Sessions::build(&self.cache, self.variant, self.target, false)
        }
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
        reference_clip: &Path,
        language: &str,
    ) -> Result<AudioBuffer, VoiceMeError> {
        // Both of these happen deliberately outside the lock. Rejecting
        // empty text before anything expensive is the I/O matrix's own rule,
        // and decoding the reference clip is per-utterance work with no
        // reason to hold a queued Speak Action back.
        if text.trim().is_empty() {
            return Err(VoiceMeError::EmptyText);
        }
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
fn execution_target_for(backend: SpeechBackend) -> ExecutionTarget {
    match backend.target {
        SpeechExecutionTarget::Cpu => ExecutionTarget::Cpu,
        SpeechExecutionTarget::WebGpu => ExecutionTarget::WebGpu {
            device_id: backend.device.unwrap_or(0) as i32,
        },
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

    #[test]
    fn empty_text_is_rejected_before_any_session_work() {
        let (_dir, adapter) = adapter_over_an_empty_cache();

        let error = adapter
            .generate("   ", Path::new("/data/reference.wav"), "tr")
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
}
