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

use serde::{Deserialize, Serialize};

use crate::error::VoiceMeError;
use crate::state::{SpeechWeights, StockVoice};

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

/// Where Piper voices live inside the cache root (Story 3.15), one
/// directory per voice key.
pub const PIPER_DIR: &str = "piper";

/// A Piper voice's graph, inside its directory.
pub const PIPER_MODEL_FILE: &str = "model.onnx";

/// A Piper voice's `config.json`, inside its directory.
pub const PIPER_CONFIG_FILE: &str = "config.json";

/// What `voice-me-deps` records about a Piper voice it installed, inside its
/// directory. Written last, so a voice with one is a complete install.
pub const PIPER_MANIFEST_FILE: &str = "voice.toml";

/// The Piper voice a Linux or Windows profile starts with (Stories 3.15,
/// 3.16): its key and its locale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PiperDefaultVoice {
    pub key: &'static str,
    pub locale: &'static str,
}

/// `tr_TR-fahrettin-medium`: CC0, 63 MB, Turkish.
pub const PIPER_DEFAULT_VOICE: PiperDefaultVoice = PiperDefaultVoice {
    key: "tr_TR-fahrettin-medium",
    locale: "tr_TR",
};

/// `<root>/piper`.
pub fn piper_dir(root: &Path) -> PathBuf {
    root.join(PIPER_DIR)
}

/// Where an eSpeak NG unpacked by `voice-me-deps` lives inside the cache
/// root on Windows (Story 3.16): the MSI's administrative image, so the
/// program sits at `<root>/espeak-ng/eSpeak NG/espeak-ng.exe`.
pub const ESPEAK_DIR: &str = "espeak-ng";

/// `<root>/espeak-ng`.
pub fn espeak_dir(root: &Path) -> PathBuf {
    root.join(ESPEAK_DIR)
}

/// The directory eSpeak NG's MSI installs into, under Program Files — and,
/// in an administrative image, under its target directory.
pub const ESPEAK_WINDOWS_INSTALL_DIR: &str = "eSpeak NG";

/// The program's file name on Windows.
pub const ESPEAK_WINDOWS_PROGRAM: &str = "espeak-ng.exe";

/// Where the program sits in an eSpeak NG unpacked into `espeak_dir` (the
/// MSI's administrative image): `<espeak_dir>/eSpeak NG/espeak-ng.exe`.
pub fn espeak_program(espeak_dir: &Path) -> PathBuf {
    espeak_dir
        .join(ESPEAK_WINDOWS_INSTALL_DIR)
        .join(ESPEAK_WINDOWS_PROGRAM)
}

/// Whether `key` can name a voice directory: letters, digits, `_`, `-` and
/// `.`, not starting with a dot. Anything else — a `/`, a `..` — could
/// reach outside `<root>/piper`, so it is never a voice.
pub fn is_valid_piper_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && !key.starts_with('.')
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// The files of one Piper voice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiperVoiceFiles {
    /// `<root>/piper/<key>`.
    pub dir: PathBuf,
    pub model: PathBuf,
    pub config: PathBuf,
    pub manifest: PathBuf,
}

/// Where voice `key`'s files live under `root`; `None` for a key that is
/// not a voice name ([`is_valid_piper_key`]).
pub fn piper_voice_files(root: &Path, key: &str) -> Option<PiperVoiceFiles> {
    if !is_valid_piper_key(key) {
        return None;
    }
    let dir = piper_dir(root).join(key);
    Some(PiperVoiceFiles {
        model: dir.join(PIPER_MODEL_FILE),
        config: dir.join(PIPER_CONFIG_FILE),
        manifest: dir.join(PIPER_MANIFEST_FILE),
        dir,
    })
}

/// What `voice.toml` records about an installed voice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiperVoiceManifest {
    /// The voice's own name (`fahrettin`).
    pub name: String,
    /// Its locale (`tr_TR`) — Piper's speech language.
    pub locale: String,
    /// How its language is named to the user (`Turkish (Turkey)`).
    pub label: String,
    /// `x_low`, `low`, `medium`, `high`.
    pub quality: String,
    /// Which catalog it came from, as the user reads it.
    pub source: String,
    /// Its licence, when the source declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence: Option<String>,
}

impl PiperVoiceManifest {
    /// The TOML `voice.toml` holds.
    pub fn to_toml(&self) -> Result<String, VoiceMeError> {
        toml::to_string_pretty(self)
            .map_err(|error| VoiceMeError::Other(format!("could not write voice.toml: {error}")))
    }

    /// Read `voice.toml`'s text; `None` when it is not one.
    pub fn from_toml(text: &str) -> Option<Self> {
        toml::from_str(text).ok()
    }
}

/// One Piper voice installed under the cache root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPiperVoice {
    pub key: String,
    pub manifest: PiperVoiceManifest,
    /// The graph and config together, in bytes.
    pub size_bytes: u64,
}

impl InstalledPiperVoice {
    /// The voice as the stock-voice vocabulary names it: its key is the id,
    /// its locale the language, labelled with the language's name.
    pub fn to_stock_voice(&self) -> StockVoice {
        StockVoice {
            id: self.key.clone(),
            language: self.manifest.locale.clone(),
            language_label: self.manifest.label.clone(),
            name: format!("{} ({})", self.manifest.name, self.manifest.quality),
            priority: 0,
        }
    }
}

/// Every complete Piper voice under `root`, sorted by key: a directory with
/// a valid key holding the graph, the config and a readable `voice.toml`.
/// Read from disk every time — the installed list is a fact about the
/// cache, never a setting.
pub fn installed_piper_voices(root: &Path) -> Vec<InstalledPiperVoice> {
    let Ok(entries) = std::fs::read_dir(piper_dir(root)) else {
        return Vec::new();
    };
    let mut voices: Vec<InstalledPiperVoice> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let key = entry.file_name().to_str()?.to_string();
            let files = piper_voice_files(root, &key)?;
            let size = |path: &Path| std::fs::metadata(path).ok().map(|meta| meta.len());
            let size_bytes = size(&files.model)? + size(&files.config)?;
            let manifest =
                PiperVoiceManifest::from_toml(&std::fs::read_to_string(&files.manifest).ok()?)?;
            Some(InstalledPiperVoice {
                key,
                manifest,
                size_bytes,
            })
        })
        .collect();
    voices.sort_by(|a, b| a.key.cmp(&b.key));
    voices
}

/// The installed voices as stock voices: Piper's languages and voices.
pub fn installed_piper_stock_voices(root: &Path) -> Vec<StockVoice> {
    installed_piper_voices(root)
        .iter()
        .map(InstalledPiperVoice::to_stock_voice)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn espeak_ng_is_unpacked_next_to_the_piper_voices() {
        assert_eq!(
            espeak_dir(Path::new("/cache")),
            PathBuf::from("/cache/espeak-ng")
        );
        assert_eq!(
            espeak_program(Path::new("/cache/espeak-ng")),
            PathBuf::from("/cache/espeak-ng/eSpeak NG/espeak-ng.exe")
        );
    }

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

    fn install_fake_voice(root: &Path, key: &str, manifest: Option<&str>) {
        let files = piper_voice_files(root, key).unwrap();
        std::fs::create_dir_all(&files.dir).unwrap();
        std::fs::write(&files.model, b"graph").unwrap();
        std::fs::write(&files.config, b"{}").unwrap();
        if let Some(manifest) = manifest {
            std::fs::write(&files.manifest, manifest).unwrap();
        }
    }

    const MANIFEST: &str = "name = \"fahrettin\"\nlocale = \"tr_TR\"\nlabel = \"Turkish\"\n\
                            quality = \"medium\"\nsource = \"voice-me\"\nlicence = \"CC0-1.0\"\n";

    #[test]
    fn a_piper_voice_lives_in_its_own_directory_under_piper() {
        let files = piper_voice_files(Path::new("/cache"), "tr_TR-fahrettin-medium").unwrap();
        assert_eq!(
            files.model,
            PathBuf::from("/cache/piper/tr_TR-fahrettin-medium/model.onnx")
        );
        assert_eq!(
            files.manifest,
            PathBuf::from("/cache/piper/tr_TR-fahrettin-medium/voice.toml")
        );
        for bad in ["", "..", "../etc", "a/b", ".hidden", "a b"] {
            assert!(
                piper_voice_files(Path::new("/cache"), bad).is_none(),
                "{bad}"
            );
        }
    }

    #[test]
    fn only_complete_voices_are_installed_and_they_become_stock_voices() {
        let root = tempfile::tempdir().unwrap();
        install_fake_voice(root.path(), "tr_TR-fahrettin-medium", Some(MANIFEST));
        // No voice.toml yet: an install still in progress.
        install_fake_voice(root.path(), "tr_TR-dfki-medium", None);
        // A voice.toml that is not one.
        install_fake_voice(root.path(), "en_US-lessac-medium", Some("not = [toml"));

        let installed = installed_piper_voices(root.path());

        assert_eq!(installed.len(), 1, "{installed:?}");
        assert_eq!(installed[0].key, "tr_TR-fahrettin-medium");
        assert_eq!(installed[0].size_bytes, 7);
        assert_eq!(installed[0].manifest.licence.as_deref(), Some("CC0-1.0"));
        let stock = installed[0].to_stock_voice();
        assert_eq!(stock.id, "tr_TR-fahrettin-medium");
        assert_eq!(stock.language, "tr_TR");
        assert_eq!(stock.language_label, "Turkish");
        assert_eq!(stock.name, "fahrettin (medium)");
        assert!(installed_piper_voices(Path::new("/nonexistent-voice-me")).is_empty());
    }

    #[test]
    fn a_manifest_round_trips_through_toml() {
        let manifest = PiperVoiceManifest::from_toml(MANIFEST).unwrap();
        assert_eq!(
            PiperVoiceManifest::from_toml(&manifest.to_toml().unwrap()),
            Some(manifest)
        );
    }
}
