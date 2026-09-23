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
static COMMITTED_RUNTIME: Mutex<Option<PathBuf>> = Mutex::new(None);

/// The runtime library this process has committed, or `None` if no session
/// build has loaded one yet.
pub fn committed_runtime() -> Option<PathBuf> {
    COMMITTED_RUNTIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Load ONNX Runtime itself.
///
/// Under `webgpu-probe` there is no dylib to resolve — that build statically
/// links pyke's Dawn-bundling distribution — so `dylib` is ignored and the
/// environment is committed as-is.
#[cfg(not(feature = "dynamic-runtime"))]
pub fn init_runtime(_dylib: &Path) -> Result<(), VoiceMeError> {
    let _ = ort::init().with_name("voice-me").commit();
    Ok(())
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
#[cfg(feature = "dynamic-runtime")]
pub fn init_runtime(dylib: &Path) -> Result<(), VoiceMeError> {
    let mut committed = COMMITTED_RUNTIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(loaded) = committed.as_ref() {
        if loaded == dylib {
            return Ok(());
        }
        return Err(VoiceMeError::SpeechEngine(format!(
            "ONNX Runtime is already loaded from {} in this session; restart voice-me to use {}",
            loaded.display(),
            dylib.display()
        )));
    }

    if !dylib.exists() {
        return Err(VoiceMeError::MissingRuntimeAsset {
            path: dylib.to_path_buf(),
        });
    }

    // `commit()` returns `bool`, not `Result`: `false` means an environment
    // was already committed, which cannot happen here — this is the only
    // place that commits one, and it holds the lock.
    let _ = ort::init_from(dylib)
        .map_err(|error| {
            VoiceMeError::SpeechEngine(format!("could not load {}: {error}", dylib.display()))
        })?
        .with_name("voice-me")
        .commit();
    *committed = Some(dylib.to_path_buf());

    Ok(())
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

    #[cfg(feature = "dynamic-runtime")]
    #[test]
    fn a_missing_onnx_runtime_names_the_dylib_rather_than_panicking() {
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

    #[cfg(feature = "dynamic-runtime")]
    #[test]
    fn a_file_that_is_not_there_is_refused_by_the_probe_naming_it() {
        let error = probe_runtime(Path::new("/nonexistent/libonnxruntime.so")).unwrap_err();
        assert!(
            matches!(&error, VoiceMeError::MissingRuntimeAsset { .. }),
            "got {error:?}"
        );
    }
}
