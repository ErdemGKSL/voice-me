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

/// One file taken out of an archive: the entry's path inside it, and the
/// file name it takes in the directory it is extracted into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryEntry {
    pub entry: String,
    pub file_name: String,
}

impl LibraryEntry {
    pub fn new(entry: impl Into<String>, file_name: impl Into<String>) -> Self {
        Self {
            entry: entry.into(),
            file_name: file_name.into(),
        }
    }
}

/// An archive and the shared libraries taken out of it — nothing else in
/// it (headers, import libraries, licences, symlinks) is ever extracted.
///
/// The ONNX Runtime archives (Microsoft's CPU one, voice-me's own core and
/// CUDA provider archives, Story 3.8) and NVIDIA's wheels, which are zip
/// files, all have this shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeArchive {
    pub archive: Asset,
    /// Every library to extract, in order.
    pub library_entries: Vec<LibraryEntry>,
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
    /// The core runtime, extracted into `<cache>/runtime/`. `None` where
    /// no runtime can be installed automatically — every target but Linux
    /// x64 and Windows x64 (Decision 1, extended by spec 3-2 for Windows).
    pub runtime: Option<RuntimeArchive>,
    /// Story 3.8: whether [`Self::runtime`] is voice-me's own build, with
    /// the WebGPU and CUDA execution providers in it, rather than
    /// Microsoft's CPU-only one. Until the release is pinned it is the
    /// latter, and a GPU backend's runtime row stays manual (decision 7).
    pub runtime_all_providers: bool,
    /// Story 3.8: the CUDA execution provider that goes with the core
    /// runtime, extracted beside it. Fetched only for a CUDA backend.
    /// `None` until the release is pinned.
    pub cuda_runtime: Option<RuntimeArchive>,
    /// Story 3.8: NVIDIA's own wheels (cudart, cuBLAS, cuFFT, cuDNN) from
    /// PyPI, whose shared libraries are extracted into
    /// `<cache>/runtime/cuda/`. Fetched only for a CUDA backend. Empty
    /// until every one of them is pinned.
    pub nvidia_wheels: Vec<RuntimeArchive>,
    /// Story 3.16: eSpeak NG's official `espeak-ng.msi`, which Install on
    /// the eSpeak NG row unpacks into the cache. `None` where eSpeak NG is
    /// not installed automatically — every target but Windows x64.
    pub espeak: Option<Asset>,
    /// Story 2.8: VB-Audio's VB-CABLE driver pack, whose setup Install on
    /// the Virtual Microphone row runs. `Some` on Windows only; elsewhere
    /// the row installs without downloading anything.
    pub virtual_mic: Option<Asset>,
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
        let (runtime, runtime_all_providers) = match voiceme_runtime() {
            Some(core) => (Some(core), true),
            None => (microsoft_runtime(), false),
        };
        Self {
            model_files,
            runtime,
            runtime_all_providers,
            cuda_runtime: voiceme_cuda_runtime(),
            nvidia_wheels: pinned_nvidia_wheels(),
            espeak: pinned_espeak(),
            virtual_mic: pinned_virtual_mic(),
        }
    }

    /// Whether a CUDA backend's runtime pieces — the CUDA provider and the
    /// NVIDIA libraries — can be installed with one click.
    pub fn cuda_installable(&self) -> bool {
        self.runtime_all_providers && self.cuda_runtime.is_some() && !self.nvidia_wheels.is_empty()
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

/// Microsoft's CPU-only archive: the runtime until voice-me's own build is
/// pinned (decision 7).
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn microsoft_runtime() -> Option<RuntimeArchive> {
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
        library_entries: vec![LibraryEntry::new(
            format!(
                "onnxruntime-linux-x64-{RUNTIME_VERSION}/lib/libonnxruntime.so.{RUNTIME_VERSION}"
            ),
            assets::runtime_dylib_file_name(),
        )],
    })
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn microsoft_runtime() -> Option<RuntimeArchive> {
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
        library_entries: vec![LibraryEntry::new(
            format!("onnxruntime-win-x64-{RUNTIME_VERSION}/lib/onnxruntime.dll"),
            assets::runtime_dylib_file_name(),
        )],
    })
}

#[cfg(not(any(
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "windows", target_arch = "x86_64")
)))]
fn microsoft_runtime() -> Option<RuntimeArchive> {
    None
}

// ---------------------------------------------------------------------------
// Story 3.8: voice-me's own ONNX Runtime, and NVIDIA's libraries for CUDA.
//
// Everything below is data. Pinning a published release is filling in the
// `pin` fields (from the release's `SHA256SUMS.txt` and `SIZES.txt`) and,
// if the workflow's `*-contents.txt` lists more shared libraries in an
// archive than the table below does (Dawn, `dxil.dll`, `dxcompiler.dll`),
// adding them to its `libraries`. Until a pin is there, `Sources::pinned`
// keeps Microsoft's CPU archive and the GPU rows stay manual (decision 7).
// ---------------------------------------------------------------------------

/// The GitHub Release voice-me's ONNX Runtime build is mirrored on
/// (decision 6): built by `.github/workflows/onnxruntime.yml`, never
/// replaced under the same tag.
pub const VOICEME_RUNTIME_TAG: &str = "onnxruntime-1.28.2-voiceme.1";

/// This repository's release downloads.
const VOICEME_RELEASES_URL: &str = "https://github.com/ErdemGKSL/voice-me/releases/download";

/// A published file's exact size and SHA-256 (lowercase hex).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pin {
    pub size: u64,
    pub sha256: &'static str,
}

/// One archive of the voice-me release, as the workflow's Package step
/// names it: `<dir>.tgz` on Linux, `<dir>.zip` on Windows, holding
/// `<dir>/lib/<library>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReleaseArchive {
    /// The archive's top-level directory, which is also its file name
    /// without the extension.
    pub dir: &'static str,
    /// `tgz` or `zip`.
    pub extension: &'static str,
    /// `None` until the release is published and pinned.
    pub pin: Option<Pin>,
    /// The shared libraries under `<dir>/lib/`, extracted under the same
    /// name — except the core library on Linux, whose versioned file
    /// (`libonnxruntime.so.<version>`) becomes `libonnxruntime.so`.
    pub libraries: &'static [(&'static str, &'static str)],
}

impl ReleaseArchive {
    pub fn file_name(&self) -> String {
        format!("{}.{}", self.dir, self.extension)
    }

    /// The archive as a source, when it is pinned: its release URL, and
    /// where it is downloaded to (`<cache>/runtime/<file name>`).
    pub fn source(&self) -> Option<RuntimeArchive> {
        let pin = self.pin?;
        let file_name = self.file_name();
        Some(RuntimeArchive {
            archive: Asset {
                relative_path: format!("{}/{file_name}", assets::RUNTIME_DIR),
                url: format!("{VOICEME_RELEASES_URL}/{VOICEME_RUNTIME_TAG}/{file_name}"),
                size: pin.size,
                digest: Digest::Sha256(pin.sha256.to_string()),
            },
            library_entries: self
                .libraries
                .iter()
                .map(|(entry, file_name)| {
                    LibraryEntry::new(format!("{}/lib/{entry}", self.dir), *file_name)
                })
                .collect(),
        })
    }
}

/// Linux x64 core: `onnxruntime` (CPU and WebGPU) and the provider bridge.
pub const VOICEME_LINUX_CORE: ReleaseArchive = ReleaseArchive {
    dir: "onnxruntime-voiceme-linux-x64-1.28.2",
    extension: "tgz",
    pin: None,
    libraries: &[
        ("libonnxruntime.so.1.28.2", "libonnxruntime.so"),
        (
            "libonnxruntime_providers_shared.so",
            "libonnxruntime_providers_shared.so",
        ),
    ],
};

/// Linux x64 CUDA provider.
pub const VOICEME_LINUX_CUDA: ReleaseArchive = ReleaseArchive {
    dir: "onnxruntime-voiceme-linux-x64-cuda12-1.28.2",
    extension: "tgz",
    pin: None,
    libraries: &[(
        "libonnxruntime_providers_cuda.so",
        "libonnxruntime_providers_cuda.so",
    )],
};

/// Windows x64 core.
pub const VOICEME_WINDOWS_CORE: ReleaseArchive = ReleaseArchive {
    dir: "onnxruntime-voiceme-win-x64-1.28.2",
    extension: "zip",
    pin: None,
    libraries: &[
        ("onnxruntime.dll", "onnxruntime.dll"),
        (
            "onnxruntime_providers_shared.dll",
            "onnxruntime_providers_shared.dll",
        ),
    ],
};

/// Windows x64 CUDA provider.
pub const VOICEME_WINDOWS_CUDA: ReleaseArchive = ReleaseArchive {
    dir: "onnxruntime-voiceme-win-x64-cuda12-1.28.2",
    extension: "zip",
    pin: None,
    libraries: &[(
        "onnxruntime_providers_cuda.dll",
        "onnxruntime_providers_cuda.dll",
    )],
};

/// This target's `(core, CUDA provider)` archives, if it has any.
pub fn voiceme_release_archives() -> Option<(ReleaseArchive, ReleaseArchive)> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some((VOICEME_LINUX_CORE, VOICEME_LINUX_CUDA))
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some((VOICEME_WINDOWS_CORE, VOICEME_WINDOWS_CUDA))
    } else {
        None
    }
}

/// The core runtime from voice-me's release, once pinned.
fn voiceme_runtime() -> Option<RuntimeArchive> {
    voiceme_release_archives()?.0.source()
}

/// The CUDA provider from voice-me's release, once pinned — and only with
/// the core it was built with.
fn voiceme_cuda_runtime() -> Option<RuntimeArchive> {
    let (core, cuda) = voiceme_release_archives()?;
    core.source()?;
    cuda.source()
}

/// Where a wheel is published on PyPI: its `files.pythonhosted.org` URL,
/// size and SHA-256 (PyPI's own digest).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WheelPin {
    pub url: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// One NVIDIA wheel (decision 3). Only the shared libraries in `members`
/// are extracted, each into `<cache>/runtime/cuda/` under its own file
/// name. The member paths are the wheels' real layout (read from the
/// published wheels of the versions below).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NvidiaWheel {
    /// The PyPI project.
    pub package: &'static str,
    pub version: &'static str,
    /// `None` until pinned, together with the voice-me release.
    pub pin: Option<WheelPin>,
    pub members: &'static [&'static str],
}

impl NvidiaWheel {
    /// The wheel as a source, when it is pinned. It is downloaded to
    /// `<cache>/runtime/cuda/<package>-<version>.whl`.
    pub fn source(&self) -> Option<RuntimeArchive> {
        let pin = self.pin?;
        Some(RuntimeArchive {
            archive: Asset {
                relative_path: format!(
                    "{}/{}/{}-{}.whl",
                    assets::RUNTIME_DIR,
                    assets::CUDA_LIBRARIES_DIR,
                    self.package,
                    self.version
                ),
                url: pin.url.to_string(),
                size: pin.size,
                digest: Digest::Sha256(pin.sha256.to_string()),
            },
            library_entries: self
                .members
                .iter()
                .map(|member| {
                    let file_name = member.rsplit('/').next().unwrap_or(member);
                    LibraryEntry::new(*member, file_name)
                })
                .collect(),
        })
    }
}

/// CUDA 12.8 (decision 5) and cuDNN 9.8, the versions the workflow builds
/// against. Linux x64 (`manylinux` wheels).
pub const NVIDIA_WHEELS_LINUX: [NvidiaWheel; 4] = [
    NvidiaWheel {
        package: "nvidia-cuda-runtime-cu12",
        version: "12.8.90",
        pin: None,
        members: &["nvidia/cuda_runtime/lib/libcudart.so.12"],
    },
    NvidiaWheel {
        package: "nvidia-cublas-cu12",
        version: "12.8.4.1",
        pin: None,
        members: &[
            "nvidia/cublas/lib/libcublasLt.so.12",
            "nvidia/cublas/lib/libcublas.so.12",
        ],
    },
    NvidiaWheel {
        package: "nvidia-cufft-cu12",
        version: "11.3.3.83",
        pin: None,
        members: &["nvidia/cufft/lib/libcufft.so.11"],
    },
    NvidiaWheel {
        package: "nvidia-cudnn-cu12",
        version: "9.8.0.87",
        pin: None,
        members: &[
            "nvidia/cudnn/lib/libcudnn_graph.so.9",
            "nvidia/cudnn/lib/libcudnn_engines_precompiled.so.9",
            "nvidia/cudnn/lib/libcudnn_engines_runtime_compiled.so.9",
            "nvidia/cudnn/lib/libcudnn_heuristic.so.9",
            "nvidia/cudnn/lib/libcudnn_ops.so.9",
            "nvidia/cudnn/lib/libcudnn_cnn.so.9",
            "nvidia/cudnn/lib/libcudnn_adv.so.9",
            "nvidia/cudnn/lib/libcudnn.so.9",
        ],
    },
];

/// The same wheels for Windows x64 (`win_amd64`).
pub const NVIDIA_WHEELS_WINDOWS: [NvidiaWheel; 4] = [
    NvidiaWheel {
        package: "nvidia-cuda-runtime-cu12",
        version: "12.8.90",
        pin: None,
        members: &["nvidia/cuda_runtime/bin/cudart64_12.dll"],
    },
    NvidiaWheel {
        package: "nvidia-cublas-cu12",
        version: "12.8.4.1",
        pin: None,
        members: &[
            "nvidia/cublas/bin/cublasLt64_12.dll",
            "nvidia/cublas/bin/cublas64_12.dll",
        ],
    },
    NvidiaWheel {
        package: "nvidia-cufft-cu12",
        version: "11.3.3.83",
        pin: None,
        members: &["nvidia/cufft/bin/cufft64_11.dll"],
    },
    NvidiaWheel {
        package: "nvidia-cudnn-cu12",
        version: "9.8.0.87",
        pin: None,
        members: &[
            "nvidia/cudnn/bin/cudnn_graph64_9.dll",
            "nvidia/cudnn/bin/cudnn_engines_precompiled64_9.dll",
            "nvidia/cudnn/bin/cudnn_engines_runtime_compiled64_9.dll",
            "nvidia/cudnn/bin/cudnn_heuristic64_9.dll",
            "nvidia/cudnn/bin/cudnn_ops64_9.dll",
            "nvidia/cudnn/bin/cudnn_cnn64_9.dll",
            "nvidia/cudnn/bin/cudnn_adv64_9.dll",
            "nvidia/cudnn/bin/cudnn64_9.dll",
        ],
    },
];

/// This target's NVIDIA wheels, if it has any.
pub fn nvidia_wheels() -> Option<&'static [NvidiaWheel]> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(&NVIDIA_WHEELS_LINUX)
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some(&NVIDIA_WHEELS_WINDOWS)
    } else {
        None
    }
}

/// Every wheel as a source — or none at all while any one is unpinned: a
/// CUDA install that fetched three of four would still not run.
fn pinned_nvidia_wheels() -> Vec<RuntimeArchive> {
    nvidia_wheels()
        .and_then(|wheels| wheels.iter().map(NvidiaWheel::source).collect())
        .unwrap_or_default()
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

/// Where the VB-CABLE pack lands under the cache root, and the directory
/// it is unpacked into.
pub const VB_CABLE_DIR: &str = "vb-cable";

/// VB-Audio's official VB-CABLE driver pack (donationware,
/// www.vb-cable.com). Downloaded from VB-Audio itself and never bundled in
/// voice-me's own package or release assets: VB's licence forbids
/// embedding it in another installer, so voice-me only fetches it and runs
/// VB's own setup, which the user confirms.
pub fn vb_cable_pack() -> Asset {
    Asset {
        relative_path: format!("{VB_CABLE_DIR}/VBCABLE_Driver_Pack45.zip"),
        url: "https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack45.zip".to_string(),
        size: 1_318_877,
        digest: Digest::Sha256(
            "b950e39f01af1d04ea623c8f6d8eb9b6ea5c477c637295fabf20631c85116bfb".to_string(),
        ),
    }
}

#[cfg(target_os = "windows")]
fn pinned_virtual_mic() -> Option<Asset> {
    Some(vb_cable_pack())
}

#[cfg(not(target_os = "windows"))]
fn pinned_virtual_mic() -> Option<Asset> {
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
        if !Sources::pinned().runtime_all_providers {
            assert_eq!(runtime.library_entries.len(), 1);
        }
        assert!(
            runtime.library_entries[0]
                .entry
                .ends_with("/lib/onnxruntime.dll"),
            "{:?}",
            runtime.library_entries
        );
        assert_eq!(runtime.library_entries[0].file_name, "onnxruntime.dll");
    }

    /// Story 2.8: the VB-CABLE pack is pinned like everything else — an
    /// HTTPS URL on VB-Audio's own server, an exact size and a full
    /// SHA-256 — and only Windows downloads it.
    #[test]
    fn the_vb_cable_pack_is_pinned_and_windows_only() {
        let pack = vb_cable_pack();
        assert!(
            pack.url
                .starts_with("https://download.vb-audio.com/Download_CABLE/"),
            "{}",
            pack.url
        );
        assert_eq!(pack.file_name(), "VBCABLE_Driver_Pack45.zip");
        assert_eq!(pack.size, 1_318_877);
        assert!(matches!(&pack.digest, Digest::Sha256(hex) if hex.len() == 64));
        assert_eq!(
            Sources::pinned().virtual_mic.is_some(),
            cfg!(target_os = "windows")
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

    /// Story 3.8: the release tables name exactly what the workflow's
    /// Package steps produce, for the runtime version this crate pins.
    #[test]
    fn the_voiceme_release_tables_match_the_workflows_archives() {
        assert!(
            VOICEME_RUNTIME_TAG.starts_with(&format!("onnxruntime-{RUNTIME_VERSION}-voiceme."))
        );
        let archives = [
            (
                VOICEME_LINUX_CORE,
                "onnxruntime-voiceme-linux-x64-1.28.2.tgz",
            ),
            (
                VOICEME_LINUX_CUDA,
                "onnxruntime-voiceme-linux-x64-cuda12-1.28.2.tgz",
            ),
            (
                VOICEME_WINDOWS_CORE,
                "onnxruntime-voiceme-win-x64-1.28.2.zip",
            ),
            (
                VOICEME_WINDOWS_CUDA,
                "onnxruntime-voiceme-win-x64-cuda12-1.28.2.zip",
            ),
        ];
        for (archive, file_name) in archives {
            assert_eq!(archive.file_name(), file_name);
            assert!(archive.dir.ends_with(RUNTIME_VERSION), "{}", archive.dir);
            if let Some(pin) = archive.pin {
                assert!(pin.sha256.len() == 64 && pin.size > 0, "{file_name}");
            }
        }
        // The core carries the runtime and the provider bridge; the CUDA
        // archive only its provider.
        assert_eq!(
            VOICEME_LINUX_CORE.libraries[0],
            ("libonnxruntime.so.1.28.2", "libonnxruntime.so")
        );
        assert!(VOICEME_WINDOWS_CORE.libraries.contains(&(
            "onnxruntime_providers_shared.dll",
            "onnxruntime_providers_shared.dll"
        )));
        assert_eq!(VOICEME_LINUX_CUDA.libraries.len(), 1);
        assert_eq!(VOICEME_WINDOWS_CUDA.libraries.len(), 1);
    }

    /// Pinning is data: a pin turns an archive into a release source at the
    /// tag's URL, with every library taken from `<dir>/lib/`.
    #[test]
    fn a_pinned_release_archive_is_a_source_at_the_release_tag() {
        assert_eq!(
            VOICEME_LINUX_CORE.pin.map(|_| ()),
            VOICEME_LINUX_CORE.source().map(|_| ())
        );
        let pinned = ReleaseArchive {
            pin: Some(Pin {
                size: 42,
                sha256: "ab",
            }),
            ..VOICEME_LINUX_CORE
        };

        let source = pinned.source().unwrap();

        assert_eq!(
            source.archive.url,
            "https://github.com/ErdemGKSL/voice-me/releases/download/\
             onnxruntime-1.28.2-voiceme.1/onnxruntime-voiceme-linux-x64-1.28.2.tgz"
        );
        assert_eq!(
            source.archive.relative_path,
            "runtime/onnxruntime-voiceme-linux-x64-1.28.2.tgz"
        );
        assert_eq!(source.archive.size, 42);
        assert_eq!(
            source.library_entries,
            vec![
                LibraryEntry::new(
                    "onnxruntime-voiceme-linux-x64-1.28.2/lib/libonnxruntime.so.1.28.2",
                    "libonnxruntime.so"
                ),
                LibraryEntry::new(
                    "onnxruntime-voiceme-linux-x64-1.28.2/lib/libonnxruntime_providers_shared.so",
                    "libonnxruntime_providers_shared.so"
                ),
            ]
        );
    }

    /// A pinned wheel lands in `runtime/cuda/`, and only its shared
    /// libraries come out, each under its own file name.
    #[test]
    fn a_pinned_wheel_extracts_its_libraries_into_runtime_cuda() {
        let wheel = NvidiaWheel {
            pin: Some(WheelPin {
                url: "https://files.pythonhosted.org/x.whl",
                size: 7,
                sha256: "cd",
            }),
            ..NVIDIA_WHEELS_LINUX[3]
        };

        let source = wheel.source().unwrap();

        assert_eq!(
            source.archive.relative_path,
            "runtime/cuda/nvidia-cudnn-cu12-9.8.0.87.whl"
        );
        assert!(source.library_entries.contains(&LibraryEntry::new(
            "nvidia/cudnn/lib/libcudnn.so.9",
            "libcudnn.so.9"
        )));
        for wheels in [&NVIDIA_WHEELS_LINUX, &NVIDIA_WHEELS_WINDOWS] {
            let packages: Vec<_> = wheels.iter().map(|wheel| wheel.package).collect();
            assert_eq!(
                packages,
                [
                    "nvidia-cuda-runtime-cu12",
                    "nvidia-cublas-cu12",
                    "nvidia-cufft-cu12",
                    "nvidia-cudnn-cu12"
                ]
            );
            for wheel in wheels {
                assert!(
                    wheel
                        .members
                        .iter()
                        .all(|member| member.starts_with("nvidia/")
                            && (member.ends_with(".dll") || member.contains(".so."))),
                    "{wheel:?}"
                );
            }
        }
    }

    /// Decision 7: until voice-me's release is pinned, the runtime is
    /// Microsoft's CPU archive and nothing CUDA can be installed. Once
    /// pinned, the core replaces it for everyone.
    #[test]
    fn the_default_runtime_follows_whether_the_release_is_pinned() {
        let sources = Sources::pinned();
        let Some((core, cuda)) = voiceme_release_archives() else {
            assert!(!sources.runtime_all_providers);
            assert!(!sources.cuda_installable());
            return;
        };
        assert_eq!(sources.runtime_all_providers, core.pin.is_some());
        let runtime = sources
            .runtime
            .clone()
            .expect("this target installs a runtime");
        if core.pin.is_some() {
            assert_eq!(runtime.archive.url, core.source().unwrap().archive.url);
        } else {
            assert!(
                runtime
                    .archive
                    .url
                    .starts_with("https://github.com/microsoft/onnxruntime/"),
                "{}",
                runtime.archive.url
            );
            assert!(sources.cuda_runtime.is_none());
        }
        let wheels_pinned = nvidia_wheels()
            .unwrap()
            .iter()
            .all(|wheel| wheel.pin.is_some());
        assert_eq!(
            sources.cuda_installable(),
            core.pin.is_some() && cuda.pin.is_some() && wheels_pinned
        );
    }
}
