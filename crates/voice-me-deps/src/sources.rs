//! Where every provisioned file comes from, pinned (Story 3.2).
//!
//! One table, holding a URL, a size and a SHA-256 per asset, so that the
//! day this repo mirrors the assets on its own GitHub Releases (Story 3.8)
//! only the URLs change. Every URL is versioned and immutable: the weights
//! are addressed by Hugging Face commit, the runtime by release tag. A URL
//! that could move under us would make the pinned checksum a guess.
//!
//! *What* to fetch is never decided here. The plan walks
//! `voice-me-core::assets::required_model_files` — the same list the
//! Dependency Check and the engine read — and only looks each file up in
//! this table. A file that list names and this table lacks is an error,
//! not a silent skip.

use std::path::{Path, PathBuf};

use voice_me_core::{SpeechWeights, VoiceMeError, assets};

/// The `onnx-community/chatterbox-multilingual-ONNX` commit every weight
/// file is pinned to. The tokenizer and the graphs have drifted against
/// each other before, so the revision is part of the source, not a detail.
pub const MODEL_REVISION: &str = "452d3f434aa592098f1eedac9099f33642ab2da5";

/// The ONNX Runtime release the Linux x64 and Windows x64 runtimes come
/// from.
pub const RUNTIME_VERSION: &str = "1.28.2";

/// The repository's resolve root; the revision is appended from
/// [`MODEL_REVISION`] so it is written down exactly once.
const MODEL_REPO_URL: &str =
    "https://huggingface.co/onnx-community/chatterbox-multilingual-ONNX/resolve";

/// `(path relative to the cache root, size in bytes, SHA-256)`.
///
/// The SHA-256s are the Git LFS object ids Hugging Face reports for the
/// pinned revision's `onnx/` tree; `tokenizer.json` is not in LFS, so its
/// hash was computed from the file at that revision (whose Git blob id
/// matched the tree's). Every weight variant [`SpeechWeights`] can name is
/// here — Q4, FP16, FP32 — and no other: q4f16 is not a variant this app
/// selects.
const MODEL_FILES: [(&str, u64, &str); 13] = [
    (
        "tokenizer.json",
        71_798,
        "29d48c4a178f6af3ad5130097c34744639e9294847b38a7b912c8c68027cb819",
    ),
    (
        "onnx/speech_encoder.onnx",
        1_184_608,
        "8f1c8a0f89b77bf9cd5dd8f2e034eb2c79dc00fe70d41196b28c257643b00ccb",
    ),
    (
        "onnx/speech_encoder.onnx_data",
        591_274_880,
        "92f8f290fc9720e169bc2412c507209e20b03f6564bc3243739e25c56f7dfb8f",
    ),
    (
        "onnx/embed_tokens.onnx",
        13_286,
        "f785819ca4f6271262d5bb8971d62796c3a909e3b031982c113dbe83a4c3b854",
    ),
    (
        "onnx/embed_tokens.onnx_data",
        68_390_912,
        "2a15f7dd73b2ee47f6edf87740324011594b5a528ed6471ae55e327ed6cad68c",
    ),
    (
        "onnx/language_model_q4.onnx",
        227_911,
        "7f8cdca83b2493536cbf3acf421199808a3d68736f55f4eabd20ef8a99da4313",
    ),
    (
        "onnx/language_model_q4.onnx_data",
        353_621_248,
        "e79ab8784122a501718868b9631ff46e151c552d9b24e50f25d721f375e3526c",
    ),
    (
        "onnx/language_model_fp16.onnx",
        172_657,
        "0c36a5bbbc2a4ed8c345033896612cd320fd0971a0f5e6447ab4cdd2d7f22e36",
    ),
    (
        "onnx/language_model_fp16.onnx_data",
        1_040_316_416,
        "16dca11ae994e78427fa3090cc6faf347a15988ca40809c1bd9f2721f3b759a0",
    ),
    (
        "onnx/language_model.onnx",
        171_387,
        "861a34585605e8ad671051788afc495dcbeaee833a41523a1b33aded9c3babc7",
    ),
    (
        "onnx/language_model.onnx_data",
        2_080_632_832,
        "b3556d41085196c122b7197e4d44ec4475b6d7cfe0971a70faa95caa38ad787a",
    ),
    (
        "onnx/conditional_decoder.onnx",
        6_350_448,
        "1656d0d31332bae1854839959a3139300ebb67c178651dfa3f8c5fbfa5351351",
    ),
    (
        "onnx/conditional_decoder.onnx_data",
        533_970_816,
        "51d58345a272747665ec9d5bb61e01835258a940e321a288582ac4c18cf01b5a",
    ),
];

/// What a downloaded file is checked against before it takes its final
/// name: lowercase hex, of the kind its source publishes (Story 3.15).
///
/// Everything pinned in code is SHA-256. A user-chosen Piper voice is
/// checked with whatever its catalog gives — MD5 for `rhasspy/piper-voices`,
/// the Git blob SHA-1 Hugging Face reports for a small non-LFS file — since
/// that is the only integrity data there is for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Digest {
    Sha256(String),
    Md5(String),
    /// SHA-1 of `blob <len>\0` followed by the bytes: Git's object id.
    GitBlobSha1(String),
}

impl Digest {
    /// The expected value, lowercase hex.
    pub fn expected(&self) -> &str {
        match self {
            Digest::Sha256(hex) | Digest::Md5(hex) | Digest::GitBlobSha1(hex) => hex,
        }
    }

    /// The checksum's name, for messages.
    pub fn label(&self) -> &'static str {
        match self {
            Digest::Sha256(_) => "SHA-256",
            Digest::Md5(_) => "MD5",
            Digest::GitBlobSha1(_) => "Git blob SHA-1",
        }
    }
}

/// One file this crate knows how to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    /// Where it lands, relative to the cache root, `/`-separated.
    pub relative_path: String,
    /// A versioned, immutable HTTPS URL.
    pub url: String,
    /// Its exact size — the progress total, and how an early end of the
    /// body is told apart from a finished one.
    pub size: u64,
    /// Nothing takes its final name without matching this.
    pub digest: Digest,
}

impl Asset {
    /// The bare filename, which is what every message names.
    pub fn file_name(&self) -> &str {
        self.relative_path
            .rsplit('/')
            .next()
            .unwrap_or(&self.relative_path)
    }

    /// Where it lands under `root`.
    pub fn destination(&self, root: &Path) -> PathBuf {
        self.relative_path
            .split('/')
            .fold(root.to_path_buf(), |path, segment| path.join(segment))
    }
}

/// The runtime ships inside an archive; this names the one entry in it
/// that is the real shared library (the others are symlinks to it,
/// headers, and a provider bridge the CPU path never loads).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeArchive {
    pub archive: Asset,
    /// The entry's path inside the archive.
    pub library_entry: String,
}

/// One file still to fetch, and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedDownload {
    pub asset: Asset,
    pub destination: PathBuf,
}

/// The whole source table. Injectable, so the tests can point every URL at
/// an in-process server and pin the hashes of small fake files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sources {
    pub model_files: Vec<Asset>,
    /// `None` where no runtime can be installed automatically — every
    /// target but Linux x64 and Windows x64 (Decision 1, extended by
    /// spec 3-2 for Windows).
    pub runtime: Option<RuntimeArchive>,
    /// Story 3.16: eSpeak NG's official `espeak-ng.msi`, which Install on
    /// the eSpeak NG row unpacks into the cache. `None` where eSpeak NG is
    /// not installed automatically — every target but Windows x64.
    pub espeak: Option<Asset>,
}

impl Default for Sources {
    fn default() -> Self {
        Self::pinned()
    }
}

impl Sources {
    /// The real, pinned sources.
    pub fn pinned() -> Self {
        let model_files = MODEL_FILES
            .iter()
            .map(|(relative_path, size, sha256)| Asset {
                relative_path: (*relative_path).to_string(),
                url: format!("{MODEL_REPO_URL}/{MODEL_REVISION}/{relative_path}"),
                size: *size,
                digest: Digest::Sha256((*sha256).to_string()),
            })
            .collect();
        Self {
            model_files,
            runtime: pinned_runtime(),
            espeak: pinned_espeak(),
        }
    }

    /// The model files `weights` needs under `root` that are not there
    /// yet, in the order `assets::required_model_files` lists them.
    ///
    /// "There" means present under its final name. Only a verified file
    /// ever gets that name (it arrives by atomic rename from `.part`), so
    /// presence is the same test the Dependency Check applies, and the two
    /// cannot disagree about what is left.
    pub fn model_plan(
        &self,
        root: &Path,
        weights: SpeechWeights,
    ) -> Result<Vec<PlannedDownload>, VoiceMeError> {
        let mut plan = Vec::new();
        for path in assets::required_model_files(root, weights) {
            if path.exists() {
                continue;
            }
            let asset = self
                .model_files
                .iter()
                .find(|asset| asset.destination(root) == path)
                .ok_or_else(|| {
                    VoiceMeError::Other(format!(
                        "voice-me has no pinned download for {}",
                        path.display()
                    ))
                })?;
            plan.push(PlannedDownload {
                asset: asset.clone(),
                destination: path,
            });
        }
        Ok(plan)
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn pinned_runtime() -> Option<RuntimeArchive> {
    Some(RuntimeArchive {
        archive: Asset {
            relative_path: format!(
                "{}/onnxruntime-linux-x64-{RUNTIME_VERSION}.tgz",
                assets::RUNTIME_DIR
            ),
            url: format!(
                "https://github.com/microsoft/onnxruntime/releases/download/v{RUNTIME_VERSION}/onnxruntime-linux-x64-{RUNTIME_VERSION}.tgz"
            ),
            size: 9_128_991,
            // GitHub's own digest for the release asset, and the hash of
            // the copy this spec was written against.
            digest: Digest::Sha256(
                "d7209b8751b27b862b0c76332c2e20e203396edb5dab700ecf4bb485cf147415".to_string(),
            ),
        },
        library_entry: format!(
            "onnxruntime-linux-x64-{RUNTIME_VERSION}/lib/libonnxruntime.so.{RUNTIME_VERSION}"
        ),
    })
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn pinned_runtime() -> Option<RuntimeArchive> {
    Some(RuntimeArchive {
        archive: Asset {
            relative_path: format!(
                "{}/onnxruntime-win-x64-{RUNTIME_VERSION}.zip",
                assets::RUNTIME_DIR
            ),
            url: format!(
                "https://github.com/microsoft/onnxruntime/releases/download/v{RUNTIME_VERSION}/onnxruntime-win-x64-{RUNTIME_VERSION}.zip"
            ),
            size: 78_620_837,
            // GitHub's own digest for the release asset (the same API
            // returns the Linux pin above).
            digest: Digest::Sha256(
                "c4eedd29489d5feca21866d054638416f3655bf6b18851b3b6b85c8313e95c35".to_string(),
            ),
        },
        library_entry: format!("onnxruntime-win-x64-{RUNTIME_VERSION}/lib/onnxruntime.dll"),
    })
}

#[cfg(not(any(
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "windows", target_arch = "x86_64")
)))]
fn pinned_runtime() -> Option<RuntimeArchive> {
    None
}

/// The eSpeak NG release Windows' Install fetches (Story 3.16).
pub const ESPEAK_VERSION: &str = "1.52.0";

/// eSpeak NG's own release asset, pinned by URL, size and SHA-256.
/// Whatever the target, so the pin itself is testable anywhere.
pub fn espeak_msi() -> Asset {
    Asset {
        relative_path: "espeak-ng.msi".to_string(),
        url: format!(
            "https://github.com/espeak-ng/espeak-ng/releases/download/{ESPEAK_VERSION}/espeak-ng.msi"
        ),
        size: 12_765_862,
        digest: Digest::Sha256(
            "7f673c709ea5dd579d3b5ebb98688cc575328a6ab7438d2bc405b88cedaeafb9".to_string(),
        ),
    }
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn pinned_espeak() -> Option<Asset> {
    Some(espeak_msi())
}

#[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
fn pinned_espeak() -> Option<Asset> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Story 3.16: the one eSpeak NG download is the official 1.52.0 MSI,
    /// fully pinned, and only Windows x64 installs it.
    #[test]
    fn espeak_ng_is_pinned_to_its_official_msi_on_windows_x64_only() {
        let msi = espeak_msi();
        assert_eq!(
            msi.url,
            "https://github.com/espeak-ng/espeak-ng/releases/download/1.52.0/espeak-ng.msi"
        );
        assert_eq!(msi.size, 12_765_862);
        assert_eq!(
            msi.digest,
            Digest::Sha256(
                "7f673c709ea5dd579d3b5ebb98688cc575328a6ab7438d2bc405b88cedaeafb9".to_string()
            )
        );
        assert_eq!(
            Sources::pinned().espeak.is_some(),
            cfg!(all(target_os = "windows", target_arch = "x86_64"))
        );
    }

    fn total(plan: &[PlannedDownload]) -> u64 {
        plan.iter().map(|planned| planned.asset.size).sum()
    }

    /// Every file the engine can ask for has a pin, for every variant —
    /// otherwise Install would fail on a file the check just reported.
    #[test]
    fn every_required_file_of_every_variant_has_a_pinned_source() {
        let root = Path::new("/nonexistent-voice-me-cache");
        for weights in [SpeechWeights::Q4, SpeechWeights::Fp16, SpeechWeights::Fp32] {
            let plan = Sources::pinned().model_plan(root, weights).unwrap();
            assert_eq!(plan.len(), 9, "{weights:?}: {plan:?}");
        }
    }

    #[test]
    fn every_pin_is_an_immutable_revision_with_a_full_sha256() {
        for asset in Sources::pinned().model_files {
            assert!(asset.url.contains(MODEL_REVISION), "{}", asset.url);
            assert!(asset.url.starts_with("https://"), "{}", asset.url);
            assert!(
                matches!(&asset.digest, Digest::Sha256(hex) if hex.len() == 64),
                "{}",
                asset.relative_path
            );
        }
    }

    /// The epic's own figure for the CPU set: roughly 1.56 GB.
    #[test]
    fn the_q4_set_is_the_one_and_a_half_gigabytes_the_epic_quotes() {
        let plan = Sources::pinned()
            .model_plan(Path::new("/nonexistent-voice-me-cache"), SpeechWeights::Q4)
            .unwrap();

        assert_eq!(voice_me_core::format_bytes(total(&plan)), "1.56 GB");
    }

    /// Backend relativity at the plan: a Q4 plan never names an FP16 (or
    /// FP32) file, even on an empty cache.
    #[test]
    fn a_q4_plan_never_contains_another_variants_files() {
        let plan = Sources::pinned()
            .model_plan(Path::new("/nonexistent-voice-me-cache"), SpeechWeights::Q4)
            .unwrap();

        for planned in &plan {
            let name = planned.asset.file_name();
            assert!(!name.contains("fp16"), "{name}");
            assert!(
                !name.starts_with("language_model.onnx"),
                "the FP32 graph is not part of the Q4 set: {name}"
            );
        }
    }

    /// Windows x64 installs its runtime from Microsoft's `.zip`, and takes
    /// only `onnxruntime.dll` out of it.
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    #[test]
    fn windows_x64_pins_the_zip_runtime_and_its_dll() {
        let runtime = Sources::pinned()
            .runtime
            .expect("Windows x64 has an automatic runtime install");

        assert!(
            runtime.archive.relative_path.ends_with(".zip"),
            "{}",
            runtime.archive.relative_path
        );
        assert!(runtime.archive.url.starts_with("https://"));
        assert!(runtime.archive.url.contains(RUNTIME_VERSION));
        assert!(matches!(&runtime.archive.digest, Digest::Sha256(hex) if hex.len() == 64));
        assert!(
            runtime.library_entry.ends_with("/lib/onnxruntime.dll"),
            "{}",
            runtime.library_entry
        );
    }

    #[test]
    fn a_partial_set_plans_only_what_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let required = assets::required_model_files(dir.path(), SpeechWeights::Q4);
        for path in &required[..7] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        }

        let plan = Sources::pinned()
            .model_plan(dir.path(), SpeechWeights::Q4)
            .unwrap();

        assert_eq!(plan.len(), 2);
        assert_eq!(
            plan.iter()
                .map(|planned| planned.destination.clone())
                .collect::<Vec<_>>(),
            required[7..].to_vec()
        );
    }
}
