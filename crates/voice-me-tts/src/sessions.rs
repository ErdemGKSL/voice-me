//! The four ONNX Runtime sessions, and where their files come from.
//!
//! AD-10's build-once/hold decision rests on how expensive construction is,
//! so construction is deliberately separated from generation: [`Sessions`]
//! is built once and then borrowed mutably per utterance.
//!
//! Every graph keeps its weights in an external `<name>.onnx_data` whose
//! recorded `location` is a **bare relative filename**. Two consequences,
//! both load-bearing: the `_data` file has to sit next to its `.onnx`, and
//! the session must be built from a *path* (`commit_from_file`) rather than
//! from bytes — there is no directory to resolve the sibling against
//! otherwise.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use ort::ep::ExecutionProviderDispatch;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use voice_me_core::{SpeechExecutionTarget, SpeechWeights, VoiceMeError, assets};

use crate::tokenizer::{TextTokenizer, tokenizer_path};

/// Which `language_model` variant to load.
///
/// The three differ only in weights: Q4's graph I/O is **identical** to
/// FP32's (only the weights are 4-bit), while FP16 is not a plain dtype
/// swap — explicit `Cast` nodes keep `inputs_embeds` and `logits` at f32 and
/// only the 60 KV tensors become f16, whose `GroupQueryAttention` kernel is
/// a GPU path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageModel {
    /// 4-bit weights, 354 MB. The CPU path.
    Q4,
    /// 1040 MB. GPU only in practice.
    Fp16,
    /// 2081 MB. The quality baseline.
    Fp32,
}

impl LanguageModel {
    /// The core-side name for this variant — the vocabulary
    /// `voice-me-deps` and the settings both speak (AD-9).
    pub fn weights(self) -> SpeechWeights {
        match self {
            LanguageModel::Q4 => SpeechWeights::Q4,
            LanguageModel::Fp16 => SpeechWeights::Fp16,
            LanguageModel::Fp32 => SpeechWeights::Fp32,
        }
    }

    /// The graph's filename inside `onnx/`.
    pub fn file_name(self) -> &'static str {
        assets::language_model_file_name(self.weights())
    }

    /// Whether the 60 KV tensors are f16 rather than f32. This is the one
    /// place the variant changes how the generation loop has to build its
    /// tensors, so it is named rather than inferred.
    pub fn kv_is_f16(self) -> bool {
        matches!(self, LanguageModel::Fp16)
    }

    /// Parse the spike's `--variant` argument.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "q4" => Some(LanguageModel::Q4),
            "fp16" => Some(LanguageModel::Fp16),
            "fp32" => Some(LanguageModel::Fp32),
            _ => None,
        }
    }
}

/// Which execution provider to place the graphs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionTarget {
    /// Every ONNX Runtime build has it.
    Cpu,
    /// CUDA, per device id (Story 3.3). Reachable only through a runtime
    /// library built with the CUDA provider, on a GPU at or above the
    /// compute-capability floor the Dependency Check enforces.
    Cuda { device_id: i32 },
    /// WebGPU (Dawn → Vulkan), per device id. Reachable only through a
    /// runtime library built with the WebGPU provider.
    WebGpu { device_id: i32 },
}

impl ExecutionTarget {
    /// The providers a session is built with.
    ///
    /// A GPU target is registered with `error_on_failure`: a provider that
    /// will not register is a hard error naming it, never ORT's default of
    /// quietly carrying on on CPU. That is the whole of the no-silent-
    /// fallback rule at this layer — and why "Active" can be believed.
    /// (Per-node fallback is invisible to it; spec-2-5 reads ORT's own
    /// placement logging for that.)
    pub fn providers(self) -> Result<Vec<ExecutionProviderDispatch>, VoiceMeError> {
        Ok(match self {
            ExecutionTarget::Cpu => vec![ort::ep::CPU::default().build()],
            ExecutionTarget::Cuda { device_id } => vec![
                ort::ep::CUDA::default()
                    .with_device_id(device_id)
                    .build()
                    .error_on_failure(),
            ],
            ExecutionTarget::WebGpu { device_id } => vec![
                ort::ep::WebGPU::default()
                    .with_device_id(device_id)
                    .build()
                    .error_on_failure(),
            ],
        })
    }

    /// A short label for timing output.
    pub fn label(self) -> String {
        match self {
            ExecutionTarget::Cpu => "cpu".to_string(),
            ExecutionTarget::Cuda { device_id } => format!("cuda:{device_id}"),
            ExecutionTarget::WebGpu { device_id } => format!("webgpu:{device_id}"),
        }
    }
}

/// The model cache directory: where the pinned graphs live, and the only
/// place this crate ever reads them from. It never downloads anything
/// (AD-8) — a missing file is reported by path so it can be fetched.
#[derive(Debug, Clone)]
pub struct ModelCache {
    root: PathBuf,
}

impl ModelCache {
    /// Point at an explicit directory.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// The conventional location, `$XDG_CACHE_HOME/voice-me` (overridable
    /// with `VOICE_ME_MODEL_CACHE`).
    ///
    /// Delegated to `voice-me-core`'s asset vocabulary since Story 3.1:
    /// `voice-me-deps` has to look in the same place this crate reads from,
    /// and it cannot depend on this crate to find out where that is.
    pub fn from_env() -> Result<Self, VoiceMeError> {
        Ok(Self::new(assets::model_cache_root()?))
    }

    /// The cache root itself.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn graph(&self, file_name: &str) -> PathBuf {
        assets::graph_file(&self.root, file_name)
    }

    /// Every file that must be present for `variant` to load, in the order
    /// the spike reports them. This *is* Story 3.2's provisioning list, and
    /// since Story 3.1 also Story 3.1's model-weights dependency row — one
    /// list, owned by `voice-me-core::assets` so the Dependency Check and
    /// the engine cannot disagree about it.
    pub fn required_files(&self, variant: LanguageModel) -> Vec<PathBuf> {
        assets::required_model_files(&self.root, variant.weights())
    }

    /// Report the first missing file, if any, before ONNX Runtime is asked
    /// to open anything. ORT's own message for an absent external-data file
    /// names the bare relative filename, not the path that was looked for,
    /// which is useless to whoever has to go fetch it.
    pub fn check(&self, variant: LanguageModel) -> Result<(), VoiceMeError> {
        for path in self.required_files(variant) {
            if !path.exists() {
                return Err(VoiceMeError::MissingRuntimeAsset { path });
            }
        }
        Ok(())
    }
}

/// Which runtime library this process committed, if any.
///
/// `ort` commits exactly one `libonnxruntime` per process; a second
/// `init_from` with a different path would silently keep using the first.
/// Recording it here is what lets [`init_runtime`] refuse that honestly,
/// and lets the composition root tell a switch that needs a restart from
/// one that does not (Decision 2).
static COMMITTED_RUNTIME: Mutex<Option<Committed>> = Mutex::new(None);

/// The committed library, and what its file looked like when it was
/// loaded (Story 3.8: Install can replace the bundled library in place).
struct Committed {
    path: PathBuf,
    identity: Option<FileIdentity>,
}

/// Enough of a file's metadata to tell that it was replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    len: u64,
    modified: Option<SystemTime>,
}

fn file_identity(path: &Path) -> Option<FileIdentity> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(FileIdentity {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn committed() -> std::sync::MutexGuard<'static, Option<Committed>> {
    COMMITTED_RUNTIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The runtime library this process has committed, or `None` if no session
/// build has loaded one yet.
pub fn committed_runtime() -> Option<PathBuf> {
    committed().as_ref().map(|committed| committed.path.clone())
}

/// Whether the committed library's file has been replaced on disk since it
/// was loaded (Story 3.8: Install put voice-me's all-provider build where
/// a CPU-only one was). The process keeps running the library it loaded,
/// so only a restart picks up the new one.
pub fn committed_runtime_replaced() -> bool {
    committed().as_ref().is_some_and(|committed| {
        file_identity(&committed.path).is_some_and(|now| Some(now) != committed.identity)
    })
}

/// Load ONNX Runtime itself, from `dylib` — the path the caller resolved
/// with `assets::resolve_runtime_dylib`, the same rule the Dependency Check
/// reports on.
///
/// The environment is immutable once committed, so the first successful
/// call wins for the process lifetime. A later call naming the same
/// library is a no-op; one naming a *different* library is refused with a
/// sentence saying a restart is needed, because `ort` would otherwise keep
/// running the first library while the caller believed it had switched.
pub fn init_runtime(dylib: &Path) -> Result<(), VoiceMeError> {
    let mut committed = committed();
    if let Some(loaded) = committed.as_ref() {
        if loaded.path == dylib {
            return Ok(());
        }
        return Err(VoiceMeError::SpeechEngine(format!(
            "ONNX Runtime is already loaded from {} in this session; restart voice-me to use {}",
            loaded.path.display(),
            dylib.display()
        )));
    }

    if !dylib.exists() {
        return Err(VoiceMeError::MissingRuntimeAsset {
            path: dylib.to_path_buf(),
        });
    }

    let identity = file_identity(dylib);
    // `commit()` returns `bool`, not `Result`: `false` means an environment
    // was already committed, which cannot happen here — this is the only
    // place that commits one, and it holds the lock.
    let _ = ort::init_from(dylib)
        .map_err(|error| {
            VoiceMeError::SpeechEngine(format!("could not load {}: {error}", dylib.display()))
        })?
        .with_name("voice-me")
        .commit();
    *committed = Some(Committed {
        path: dylib.to_path_buf(),
        identity,
    });

    Ok(())
}

/// NVIDIA libraries this process has loaded from the cache, held for its
/// lifetime: unloading one under a live CUDA context would crash.
static PRELOADED: Mutex<Vec<(PathBuf, libloading::Library)>> = Mutex::new(Vec::new());

/// Story 3.8: load the NVIDIA `libraries` named (the ones the installed
/// wheels recorded) from `dir` (`<cache>/runtime/cuda/`)
/// before the CUDA provider is registered, so that when ONNX Runtime loads
/// `onnxruntime_providers_cuda` its cudart, cuBLAS, cuFFT and cuDNN are
/// already in the process — the user's `PATH` and `LD_LIBRARY_PATH` are
/// never changed.
///
/// Linux: `dlopen` with `RTLD_GLOBAL`, so the provider's `DT_NEEDED`
/// entries resolve to them by soname. Windows: each DLL is loaded by full
/// path with its own folder searched for its dependencies; once loaded,
/// cuDNN's loads of its sub-libraries by name find them in the process.
/// A file not named — a library left by an older wheel, a `.part` — is
/// never loaded.
///
/// Libraries depend on each other (cuBLAS on cuBLASLt, cuDNN's engines on
/// its graph library), so loading repeats until a pass loads nothing more.
/// Returns one line per library that still would not load, with the
/// loader's reason — the caller adds them to the CUDA provider's own error
/// if it then fails. A directory that is not there loads nothing.
pub fn preload_cuda_libraries(dir: &Path, libraries: &[String]) -> Vec<String> {
    let mut pending: Vec<PathBuf> = libraries
        .iter()
        .map(|name| dir.join(name))
        .filter(|path| path.is_file() && is_shared_library(path))
        .collect();

    let mut loaded = PRELOADED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    pending.retain(|path| !loaded.iter().any(|(done, _)| done == path));
    if pending.is_empty() {
        return Vec::new();
    }
    let mut reasons: Vec<(PathBuf, String)> = Vec::new();
    loop {
        let before = pending.len();
        let mut still = Vec::new();
        for path in pending {
            match open_library(&path) {
                Ok(library) => loaded.push((path, library)),
                Err(error) => {
                    reasons.retain(|(seen, _)| seen != &path);
                    reasons.push((path.clone(), error.to_string()));
                    still.push(path);
                }
            }
        }
        pending = still;
        if pending.is_empty() || pending.len() == before {
            break;
        }
    }
    reasons
        .into_iter()
        .filter(|(path, _)| pending.contains(path))
        .map(|(path, reason)| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            format!("{name}: {reason}")
        })
        .collect()
}

/// A `.dll` on Windows; `lib*.so` or `lib*.so.<n>` elsewhere. A `.part`
/// left by an interrupted install is neither.
fn is_shared_library(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name.ends_with(".part") {
        return false;
    }
    if cfg!(target_os = "windows") {
        name.to_ascii_lowercase().ends_with(".dll")
    } else {
        name.ends_with(".so") || name.contains(".so.")
    }
}

#[cfg(unix)]
fn open_library(path: &Path) -> Result<libloading::Library, libloading::Error> {
    use libloading::os::unix::{Library, RTLD_GLOBAL, RTLD_NOW};
    // SAFETY: NVIDIA's own libraries, from the pinned wheels voice-me
    // installed; their initialisers only set up their own state.
    unsafe { Library::open(Some(path), RTLD_NOW | RTLD_GLOBAL) }.map(Into::into)
}

#[cfg(windows)]
fn open_library(path: &Path) -> Result<libloading::Library, libloading::Error> {
    use libloading::os::windows::{
        LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, Library,
    };
    // SAFETY: as on unix. By full path, with this DLL's own folder searched
    // for its dependencies — the process's DLL search path is not changed.
    unsafe {
        Library::load_with_flags(
            path,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
        )
    }
    .map(Into::into)
}

/// Load the ONNX Runtime library at `dylib` and report which of the CPU,
/// CUDA and WebGPU execution providers it offers (Story 3.3 Decision 1).
///
/// Only ever called in the `voice-me --probe-runtime` helper process: it
/// commits `dylib` as that process's runtime, which the app itself must
/// never do just to look at a file the user picked. A file that is not an
/// ONNX Runtime library fails here, with `ort`'s reason.
pub fn probe_runtime(dylib: &Path) -> Result<Vec<SpeechExecutionTarget>, VoiceMeError> {
    init_runtime(dylib)?;

    let available = |provider: &dyn ort::ep::ExecutionProvider| {
        provider.is_available().map_err(|error| {
            VoiceMeError::SpeechEngine(format!(
                "could not ask {} for its execution providers: {error}",
                dylib.display()
            ))
        })
    };

    let mut targets = Vec::new();
    if available(&ort::ep::CPU::default())? {
        targets.push(SpeechExecutionTarget::Cpu);
    }
    if available(&ort::ep::CUDA::default())? {
        targets.push(SpeechExecutionTarget::Cuda);
    }
    if available(&ort::ep::WebGPU::default())? {
        targets.push(SpeechExecutionTarget::WebGpu);
    }
    Ok(targets)
}

/// The four sessions plus the tokenizer, built once and held (AD-10).
pub struct Sessions {
    pub speech_encoder: Session,
    pub embed_tokens: Session,
    pub language_model: Session,
    pub conditional_decoder: Session,
    pub tokenizer: TextTokenizer,
    variant: LanguageModel,
    target: ExecutionTarget,
}

impl Sessions {
    /// Build all four from `cache`, placing them on `target`.
    ///
    /// `verbose_placement` turns on ORT's own per-node placement logging,
    /// which is the *only* way to tell a real GPU run from a graph that
    /// registered the provider and then fell back to CPU node by node.
    pub fn build(
        cache: &ModelCache,
        variant: LanguageModel,
        target: ExecutionTarget,
        verbose_placement: bool,
    ) -> Result<Self, VoiceMeError> {
        cache.check(variant)?;

        let tokenizer = TextTokenizer::load(&tokenizer_path(cache.root()))?;
        let providers = target.providers()?;

        let build = |file_name: &str| -> Result<Session, VoiceMeError> {
            let path = cache.graph(file_name);
            let mut builder = Session::builder()
                .map_err(|error| engine(file_name, error))?
                .with_execution_providers(&providers)
                .map_err(|error| engine(file_name, error))?
                .with_optimization_level(GraphOptimizationLevel::Level3)
                .map_err(|error| engine(file_name, error))?
                // Four physical cores; leaving this at ORT's default lets
                // it spawn one thread per logical core and thrash.
                .with_intra_threads(4)
                .map_err(|error| engine(file_name, error))?;

            if verbose_placement {
                builder = builder
                    .with_log_level(ort::logging::LogLevel::Verbose)
                    .map_err(|error| engine(file_name, error))?;
            }

            builder
                .commit_from_file(&path)
                .map_err(|error| engine(&path.display().to_string(), error))
        };

        Ok(Self {
            speech_encoder: build("speech_encoder.onnx")?,
            embed_tokens: build("embed_tokens.onnx")?,
            language_model: build(variant.file_name())?,
            conditional_decoder: build("conditional_decoder.onnx")?,
            tokenizer,
            variant,
            target,
        })
    }

    /// Which `language_model` these sessions hold.
    pub fn variant(&self) -> LanguageModel {
        self.variant
    }

    /// Where they were placed.
    pub fn target(&self) -> ExecutionTarget {
        self.target
    }
}

fn engine<R>(what: &str, error: ort::Error<R>) -> VoiceMeError {
    VoiceMeError::SpeechEngine(format!("{what}: {error}"))
}

/// Held by every test that commits, or depends on, this process's runtime
/// record, so none sees another's.
#[cfg(test)]
pub(crate) fn committed_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_is_reported_by_full_path_not_by_bare_name() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ModelCache::new(dir.path().to_path_buf());

        let error = cache.check(LanguageModel::Q4).unwrap_err();
        let VoiceMeError::MissingRuntimeAsset { path } = error else {
            panic!("expected a missing-asset error, got {error:?}");
        };
        assert!(
            path.is_absolute(),
            "{} must be fetchable as-is",
            path.display()
        );
        assert!(path.starts_with(dir.path()));
    }

    #[test]
    fn the_required_file_list_names_every_external_data_sibling() {
        let cache = ModelCache::new(PathBuf::from("/cache"));
        let files = cache.required_files(LanguageModel::Q4);

        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();

        // Nine files: the tokenizer plus four graphs, each with its weights.
        assert_eq!(files.len(), 9, "{names:?}");
        for graph in [
            "speech_encoder.onnx",
            "embed_tokens.onnx",
            "language_model_q4.onnx",
            "conditional_decoder.onnx",
        ] {
            assert!(names.contains(&graph.to_string()), "{graph} missing");
            assert!(
                names.contains(&format!("{graph}_data")),
                "{graph}_data missing — an `.onnx` without its weights loads to nothing"
            );
        }
        assert!(names.contains(&"tokenizer.json".to_string()));
    }

    #[test]
    fn each_variant_names_its_own_graph() {
        assert_eq!(LanguageModel::parse("q4"), Some(LanguageModel::Q4));
        assert_eq!(LanguageModel::parse("fp16"), Some(LanguageModel::Fp16));
        assert_eq!(LanguageModel::parse("fp32"), Some(LanguageModel::Fp32));
        assert_eq!(LanguageModel::parse("int8"), None);

        assert_eq!(LanguageModel::Fp32.file_name(), "language_model.onnx");
        assert!(LanguageModel::Fp16.kv_is_f16());
        assert!(
            !LanguageModel::Q4.kv_is_f16(),
            "q4's graph I/O is identical to fp32's — only the weights are 4-bit"
        );
    }

    #[test]
    fn a_missing_onnx_runtime_names_the_dylib_rather_than_panicking() {
        let _lock = committed_test_lock();
        let path = Path::new("/nonexistent/libonnxruntime.so");
        let error = init_runtime(path).unwrap_err();
        assert!(
            matches!(&error, VoiceMeError::MissingRuntimeAsset { path: p } if p == path),
            "got {error:?}"
        );
    }

    /// The no-silent-fallback rule at the provider layer: a GPU target is
    /// registered so that a failure is an error, never a quiet CPU run.
    #[test]
    fn gpu_targets_refuse_to_fall_back_to_cpu() {
        for target in [
            ExecutionTarget::Cuda { device_id: 0 },
            ExecutionTarget::WebGpu { device_id: 0 },
        ] {
            let providers = target.providers().unwrap();
            assert_eq!(providers.len(), 1, "no CPU provider listed after it");
            let described = format!("{:?}", providers[0]);
            assert!(
                described.contains("error_on_failure: true"),
                "{target:?}: {described}"
            );
        }
        let cuda = format!(
            "{:?}",
            ExecutionTarget::Cuda { device_id: 0 }.providers().unwrap()[0]
        );
        assert!(cuda.contains("CUDAExecutionProvider"), "{cuda}");
    }

    #[test]
    fn a_file_that_is_not_there_is_refused_by_the_probe_naming_it() {
        let _lock = committed_test_lock();
        let error = probe_runtime(Path::new("/nonexistent/libonnxruntime.so")).unwrap_err();
        assert!(
            matches!(&error, VoiceMeError::MissingRuntimeAsset { .. }),
            "got {error:?}"
        );
    }

    /// Story 3.8: a directory that is not there loads nothing and reports
    /// nothing — the CUDA provider's own error says what is missing.
    #[test]
    fn preloading_a_missing_nvidia_directory_loads_nothing() {
        assert!(
            preload_cuda_libraries(
                Path::new("/nonexistent/voice-me/runtime/cuda"),
                &["libcudnn.so.9".to_string()]
            )
            .is_empty()
        );
    }

    /// A named file that will not load is named with the loader's reason;
    /// files not named (one left by an older wheel) or not libraries (a
    /// `.part`, a wheel) are never tried.
    #[test]
    fn a_nvidia_library_that_will_not_load_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let bogus = if cfg!(target_os = "windows") {
            "cudnn64_9.dll"
        } else {
            "libcudnn.so.9"
        };
        std::fs::write(dir.path().join(bogus), b"not a library").unwrap();
        std::fs::write(dir.path().join(format!("{bogus}.part")), b"half").unwrap();
        std::fs::write(dir.path().join("nvidia-cudnn.whl"), b"zip").unwrap();
        let stale = if cfg!(target_os = "windows") {
            "cudnn_old64_9.dll"
        } else {
            "libcudnn_old.so.9"
        };
        std::fs::write(dir.path().join(stale), b"not a library either").unwrap();

        let failures = preload_cuda_libraries(
            dir.path(),
            &[
                bogus.to_string(),
                format!("{bogus}.part"),
                "nvidia-cudnn.whl".to_string(),
                "libnot-there.so.1".to_string(),
            ],
        );

        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(
            failures[0].starts_with(&format!("{bogus}: ")),
            "{failures:?}"
        );
    }

    #[test]
    fn only_shared_libraries_are_preloaded() {
        let unix = !cfg!(target_os = "windows");
        assert_eq!(is_shared_library(Path::new("/c/libcudart.so.12")), unix);
        assert_eq!(is_shared_library(Path::new("/c/libcufft.so")), unix);
        assert_eq!(is_shared_library(Path::new("/c/cudart64_12.dll")), !unix);
        assert!(!is_shared_library(Path::new("/c/libcudart.so.12.part")));
        assert!(!is_shared_library(Path::new("/c/nvidia-cudnn.whl")));
    }

    /// Nothing committed: nothing can have been replaced.
    #[test]
    fn a_runtime_never_loaded_is_never_replaced() {
        let _lock = committed_test_lock();
        let previous = committed().take();

        assert!(committed_runtime().is_none());
        assert!(!committed_runtime_replaced());

        *committed() = previous;
    }

    /// Story 3.8: a committed library whose file Install rewrote reads as
    /// replaced; until then it does not.
    #[test]
    fn a_rewritten_committed_runtime_reads_as_replaced() {
        let _lock = committed_test_lock();
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("libonnxruntime.so");
        std::fs::write(&library, b"the cpu build").unwrap();
        let previous = committed().replace(Committed {
            path: library.clone(),
            identity: file_identity(&library),
        });

        let before = committed_runtime_replaced();
        std::fs::write(&library, b"voice-me's all-provider build").unwrap();
        let after = committed_runtime_replaced();

        *committed() = previous;
        assert!(!before);
        assert!(after);
    }
}
