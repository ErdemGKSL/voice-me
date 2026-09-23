//! Where the speech engine's files live, as shared vocabulary.
//!
//! The model-file list used to exist only inside `voice-me-tts`'s
//! `ModelCache`. Story 3.1 needs the same list in `voice-me-deps`, and
//! `deps → tts` is the wrong direction (AD-9 keeps the two apart), so the
//! path and filename knowledge moves here — the same reason
//! [`crate::SpeechExecutionTarget`] lives in core rather than in either
//! side.
//!
//! Core gains nothing by knowing this beyond strings: no `ort`, no
//! filesystem policy, no network. It owns *where to look*; the two callers
//! own what to do about what they find.

use std::path::{Path, PathBuf};

use crate::error::VoiceMeError;
use crate::state::SpeechWeights;

/// Overrides the cache root entirely, for a machine that keeps the engine's
/// assets somewhere else (and for every test that must not touch the user's
/// real cache).
pub const CACHE_ROOT_ENV: &str = "VOICE_ME_MODEL_CACHE";

/// The directory under the OS cache directory.
pub const CACHE_DIR_NAME: &str = "voice-me";

/// ONNX Runtime's own environment variable, read by `ort`'s `load-dynamic`
/// path. Named here because the Dependency Check has to report on exactly
/// the rule the engine will later apply.
pub const RUNTIME_DYLIB_ENV: &str = "ORT_DYLIB_PATH";

/// Where the graphs sit inside the cache root.
pub const GRAPH_DIR: &str = "onnx";

/// Where a provisioned ONNX Runtime sits inside the cache root.
pub const RUNTIME_DIR: &str = "runtime";

/// The tokenizer's filename, at the cache root rather than under
/// [`GRAPH_DIR`].
pub const TOKENIZER_FILE: &str = "tokenizer.json";

/// The graphs every weight variant needs, besides the language model itself.
const SHARED_GRAPHS: [&str; 2] = ["speech_encoder.onnx", "embed_tokens.onnx"];

/// The graph loaded after the language model.
const DECODER_GRAPH: &str = "conditional_decoder.onnx";

/// The conventional cache root: `$XDG_CACHE_HOME/voice-me`, overridable
/// with [`CACHE_ROOT_ENV`].
///
/// `Err` when there is no cache directory to resolve at all — no `HOME`, no
/// `XDG_CACHE_HOME`, no override. That is a real answer, not a panic: the
/// Dependency Check reports "the check itself could not run, and this is
/// why" rather than inventing a path nobody asked for.
pub fn model_cache_root() -> Result<PathBuf, VoiceMeError> {
    // An empty override is unset, not a relative root at the process's
    // working directory — symmetric with `resolve_runtime_dylib` below, and
    // the difference between a report full of absolute paths and one full
    // of `onnx/...` fragments nobody can act on.
    if let Some(explicit) = std::env::var_os(CACHE_ROOT_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(explicit));
    }
    let cache = os_cache_dir()
        .ok_or_else(|| VoiceMeError::Other("could not resolve a cache directory".to_string()))?;
    Ok(cache.join(CACHE_DIR_NAME))
}

/// The OS cache directory, hand-rolled on unix rather than taken from
/// `directories` so that `XDG_CACHE_HOME` alone — with no `HOME` — still
/// resolves. Moved verbatim from `voice-me-tts` so the two sides cannot
/// drift.
fn os_cache_dir() -> Option<PathBuf> {
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
    }
    #[cfg(target_os = "windows")]
    {
        directories::BaseDirs::new().map(|dirs| dirs.cache_dir().to_path_buf())
    }
}

/// Where `tokenizer.json` lives inside `root`: at the root, not under
/// `onnx/`.
pub fn tokenizer_file(root: &Path) -> PathBuf {
    root.join(TOKENIZER_FILE)
}

/// Where one graph lives inside `root`.
pub fn graph_file(root: &Path, file_name: &str) -> PathBuf {
    root.join(GRAPH_DIR).join(file_name)
}

/// The `language_model` graph filename `weights` implies.
pub fn language_model_file_name(weights: SpeechWeights) -> &'static str {
    match weights {
        SpeechWeights::Q4 => "language_model_q4.onnx",
        SpeechWeights::Fp16 => "language_model_fp16.onnx",
        SpeechWeights::Fp32 => "language_model.onnx",
    }
}

/// Every file that must be present under `root` for `weights` to load.
///
/// This *is* Story 3.2's provisioning list, and Story 3.1's dependency row:
/// one list, read by the engine before it opens anything and by the
/// Dependency Check before the user has typed anything.
///
/// Each graph keeps its weights in a sibling `<name>.onnx_data` whose
/// recorded location is a bare relative filename, so the `_data` file is as
/// required as the graph itself and is listed first — the order the engine
/// reports them in.
pub fn required_model_files(root: &Path, weights: SpeechWeights) -> Vec<PathBuf> {
    let mut files = vec![tokenizer_file(root)];
    for graph in SHARED_GRAPHS
        .iter()
        .copied()
        .chain([language_model_file_name(weights), DECODER_GRAPH])
    {
        let path = graph_file(root, graph);
        files.push(path.with_extension("onnx_data"));
        files.push(path);
    }
    files
}

/// The platform's ONNX Runtime shared-library filename.
pub fn runtime_dylib_file_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "onnxruntime.dll"
    }
    #[cfg(target_os = "macos")]
    {
        "libonnxruntime.dylib"
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        "libonnxruntime.so"
    }
}

/// Where a provisioned ONNX Runtime is expected inside `root`, when
/// [`RUNTIME_DYLIB_ENV`] says nothing.
pub fn bundled_runtime_dylib(root: &Path) -> PathBuf {
    root.join(RUNTIME_DIR).join(runtime_dylib_file_name())
}

/// How the ONNX Runtime library was located — which is half of what the
/// Dependency Check has to report, since "not at the path you configured"
/// and "not where we would have put it" are different problems with
/// different fixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDylib {
    /// The absolute path the engine will try to load.
    pub path: PathBuf,
    /// Whether [`RUNTIME_DYLIB_ENV`] is what named it.
    pub configured: bool,
    /// Whether it is a library the user added through "Add runtime…" and
    /// selected (Story 3.3).
    pub added: bool,
}

/// Apply the runtime-resolution rule: a selected added library wins
/// (Story 3.3 Decision 1), then an explicit [`RUNTIME_DYLIB_ENV`], then the
/// cache root's own copy.
///
/// Reads no filesystem — resolution and existence are separate questions,
/// and the caller needs both answers separately to say anything useful.
pub fn resolve_runtime_dylib(root: &Path, added: Option<&Path>) -> RuntimeDylib {
    if let Some(added) = added {
        return RuntimeDylib {
            path: added.to_path_buf(),
            configured: false,
            added: true,
        };
    }
    match std::env::var_os(RUNTIME_DYLIB_ENV).filter(|value| !value.is_empty()) {
        Some(configured) => RuntimeDylib {
            path: PathBuf::from(configured),
            configured: true,
            added: false,
        },
        None => RuntimeDylib {
            path: bundled_runtime_dylib(root),
            configured: false,
            added: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tokenizer_sits_at_the_cache_root_not_under_onnx() {
        assert_eq!(
            tokenizer_file(Path::new("/cache")),
            PathBuf::from("/cache/tokenizer.json")
        );
    }

    #[test]
    fn every_graph_is_listed_with_its_external_data_sibling() {
        let files = required_model_files(Path::new("/cache"), SpeechWeights::Q4);

        assert_eq!(
            files.len(),
            9,
            "the tokenizer plus four graphs, each with its `_data`: {files:?}"
        );
        assert!(files.contains(&PathBuf::from("/cache/onnx/language_model_q4.onnx")));
        assert!(files.contains(&PathBuf::from("/cache/onnx/language_model_q4.onnx_data")));
    }

    /// The whole point of deriving the list from the selected backend: a Q4
    /// selection must never be satisfied — or even described — by FP16
    /// files.
    #[test]
    fn a_weight_variant_never_lists_another_variants_graph() {
        let q4 = required_model_files(Path::new("/cache"), SpeechWeights::Q4);

        assert!(
            !q4.iter()
                .any(|path| path.to_string_lossy().contains("fp16")),
            "{q4:?}"
        );
    }

    /// An empty `VOICE_ME_MODEL_CACHE` would otherwise resolve to `""` and
    /// make every path in every reported row relative to wherever the
    /// process happened to be started.
    #[test]
    fn an_empty_cache_override_is_treated_as_unset() {
        let previous = std::env::var_os(CACHE_ROOT_ENV);
        // The whole crate's tests run in one process; this is the only one
        // that touches this variable, and it puts it back.
        unsafe { std::env::set_var(CACHE_ROOT_ENV, "") };

        let resolved = model_cache_root();

        match previous {
            Some(value) => unsafe { std::env::set_var(CACHE_ROOT_ENV, value) },
            None => unsafe { std::env::remove_var(CACHE_ROOT_ENV) },
        }

        if let Ok(root) = resolved {
            assert!(
                root.is_absolute(),
                "an empty override must fall through to the OS cache directory: {}",
                root.display()
            );
        }
    }

    #[test]
    fn the_cache_root_is_where_the_runtime_is_looked_for_without_the_env_var() {
        assert_eq!(
            bundled_runtime_dylib(Path::new("/cache")),
            PathBuf::from("/cache/runtime").join(runtime_dylib_file_name())
        );
    }

    #[test]
    fn a_selected_added_library_wins_over_everything_else() {
        let resolved = resolve_runtime_dylib(
            Path::new("/cache"),
            Some(Path::new("/opt/ort/libonnxruntime.so")),
        );

        assert_eq!(resolved.path, PathBuf::from("/opt/ort/libonnxruntime.so"));
        assert!(resolved.added);
        assert!(!resolved.configured);
    }
}
