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

use ort::ep::ExecutionProviderDispatch;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use voice_me_core::{SpeechWeights, VoiceMeError, assets};

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
    /// The only target the default build can reach: Microsoft's linux-x64
    /// tarball ships the core library and no provider libraries at all.
    Cpu,
    /// WebGPU (Dawn → Vulkan), per device id. Available only under the
    /// `webgpu-probe` feature; see this crate's `Cargo.toml` for why that is
    /// not the default. CUDA is deliberately absent — see spec-2-5's
    /// Decision 1: this machine's GM107 is sm_50 and ONNX Runtime's prebuilt
    /// CUDA floor is sm_60 with no PTX target to JIT from.
    WebGpu { device_id: i32 },
}

impl ExecutionTarget {
    fn providers(self) -> Result<Vec<ExecutionProviderDispatch>, VoiceMeError> {
        match self {
            ExecutionTarget::Cpu => Ok(vec![ort::ep::CPU::default().build()]),
            #[cfg(feature = "webgpu-probe")]
            ExecutionTarget::WebGpu { device_id } => Ok(vec![
                // `error_on_failure` turns a silent CPU fallback into a hard
                // error — but only for EP *registration*. Per-node fallback
                // is invisible to it, which is why the spike also reads
                // ORT's own node-placement logging before calling any
                // measurement a GPU measurement.
                ort::ep::WebGPU::default()
                    .with_device_id(device_id)
                    .build()
                    .error_on_failure(),
            ]),
            #[cfg(not(feature = "webgpu-probe"))]
            ExecutionTarget::WebGpu { .. } => Err(VoiceMeError::SpeechEngine(
                "this build has no WebGPU support — rebuild with `--no-default-features \
                 --features webgpu-probe`"
                    .to_string(),
            )),
        }
    }

    /// A short label for timing output.
    pub fn label(self) -> String {
        match self {
            ExecutionTarget::Cpu => "cpu".to_string(),
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

/// Load ONNX Runtime itself.
///
/// `load-dynamic` means the library is resolved at run time, from
/// `ORT_DYLIB_PATH` or from `dylib` here. The environment is immutable once
/// committed, so this is called once per process, before any session.
///
/// Under `webgpu-probe` there is no dylib to resolve — that build statically
/// links pyke's Dawn-bundling distribution — so `dylib` is ignored and the
/// environment is committed as-is.
#[cfg(not(feature = "dynamic-runtime"))]
pub fn init_runtime(_dylib: Option<&Path>) -> Result<(), VoiceMeError> {
    let _ = ort::init().with_name("voice-me").commit();
    Ok(())
}

/// Load ONNX Runtime itself. See the `webgpu-probe` sibling above.
#[cfg(feature = "dynamic-runtime")]
pub fn init_runtime(dylib: Option<&Path>) -> Result<(), VoiceMeError> {
    let path = match dylib {
        Some(path) => path.to_path_buf(),
        // The same resolution rule the Dependency Check reports on
        // (Story 3.1): `ORT_DYLIB_PATH` wins, otherwise the cache root's
        // own copy. Reading it from one place is what keeps the runtime row
        // from claiming "ready" about a library the engine would then fail
        // to find.
        None => assets::resolve_runtime_dylib(&assets::model_cache_root()?).path,
    };

    if !path.exists() {
        return Err(VoiceMeError::MissingRuntimeAsset { path });
    }

    // `commit()` returns `bool`, not `Result`: `false` means an environment
    // was already committed, which is fine — this is idempotent by design.
    let _ = ort::init_from(&path)
        .map_err(|error| {
            VoiceMeError::SpeechEngine(format!("could not load {}: {error}", path.display()))
        })?
        .with_name("voice-me")
        .commit();

    Ok(())
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
        let error = init_runtime(Some(path)).unwrap_err();
        assert!(
            matches!(&error, VoiceMeError::MissingRuntimeAsset { path: p } if p == path),
            "got {error:?}"
        );
    }

    /// The widened resolution rule, which is the whole reason the
    /// Dependency Check's "ready" runtime row can be believed: with
    /// `ORT_DYLIB_PATH` unset the engine must look in exactly the place
    /// `voice-me-deps` reports on — `<cache root>/runtime/<dylib>` — and
    /// nowhere else. If these two drift apart the tab says "ready" and
    /// generation then fails naming a path the user was never shown.
    ///
    /// Both variables are process-global, so this restores whatever it
    /// found; `ORT_DYLIB_PATH` is also read by the sibling test above.
    #[cfg(feature = "dynamic-runtime")]
    #[test]
    fn an_unset_dylib_path_resolves_to_the_cache_root_the_check_reports_on() {
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
        let _guard = EnvGuard(vec![
            ("ORT_DYLIB_PATH", std::env::var_os("ORT_DYLIB_PATH")),
            (
                voice_me_core::assets::CACHE_ROOT_ENV,
                std::env::var_os(voice_me_core::assets::CACHE_ROOT_ENV),
            ),
        ]);
        unsafe {
            std::env::remove_var("ORT_DYLIB_PATH");
            std::env::set_var(voice_me_core::assets::CACHE_ROOT_ENV, cache.path());
        }

        let expected = assets::bundled_runtime_dylib(cache.path());
        let error = init_runtime(None).unwrap_err();

        assert!(
            matches!(&error, VoiceMeError::MissingRuntimeAsset { path } if *path == expected),
            "the engine must look where the Dependency Check says it looks \
             ({}), got {error:?}",
            expected.display()
        );
    }

    #[cfg(not(feature = "webgpu-probe"))]
    #[test]
    fn the_default_build_says_outright_that_it_cannot_reach_webgpu() {
        let error = ExecutionTarget::WebGpu { device_id: 0 }
            .providers()
            .unwrap_err();
        assert!(matches!(error, VoiceMeError::SpeechEngine(_)));
    }
}
