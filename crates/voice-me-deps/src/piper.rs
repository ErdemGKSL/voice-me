//! Piper voices (Story 3.15): the three catalogs, the built-in default
//! voice, the installer and the voice row.
//!
//! Three catalogs are merged, and the first source wins a duplicate key:
//!
//! 1. **voice-me's own** `piper-voices/catalog.json`, fetched live from the
//!    repository: each file with its URL, size and SHA-256. Adding a voice
//!    means editing that file; no release is needed.
//! 2. **`rhasspy/piper-voices`**' `voices.json` at a pinned revision. Its
//!    files are checked by the size and MD5 it gives.
//! 3. **speaches-ai** on Hugging Face: the list of `piper-*` repositories.
//!    Each holds `model.onnx` and `config.json`; at download the repository
//!    is asked (`?blobs=true`) for its revision, the LFS SHA-256 of
//!    `model.onnx` and the Git blob SHA-1 of `config.json`, and both files
//!    are fetched at that revision.
//!
//! The parsers are pure and fixture-tested. The three catalogs are fetched
//! only when the Piper voices tab is opened or refreshed, and the default
//! voice is pinned in code, so a first run needs no catalog. voice-me's own
//! catalog alone is also read at startup and every few hours to update the
//! custom voices it lists ([`outdated_custom_voices`]): their files can be
//! replaced at the same URL, and an installed copy follows.
//!
//! A voice is installed like every other asset: each file to `.part`,
//! verified, moved into place by rename; `voice.toml` last, so a voice with
//! one is complete.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::Deserialize;
use voice_me_core::{
    AppEventSender, CatalogResult, Dependency, DependencyKind, PiperCatalogEntry, PiperSource,
    VoiceMeError,
    assets::{self, PIPER_DEFAULT_VOICE, PiperVoiceManifest},
    format_bytes,
};

use crate::provision::ProgressTarget;
use crate::sources::{Asset, Digest, PlannedDownload};

/// voice-me's own catalog, fetched live.
pub const VOICE_ME_CATALOG_URL: &str =
    "https://raw.githubusercontent.com/ErdemGKSL/voice-me/main/piper-voices/catalog.json";

/// The `rhasspy/piper-voices` revision `voices.json` and its files are read
/// at.
pub const OFFICIAL_REVISION: &str = "c10ece1aade47bb51c153c893d14e5bf8e5b7117";

/// The speaches-ai repositories' list.
pub const SPEACHES_LIST_URL: &str =
    "https://huggingface.co/api/models?author=speaches-ai&search=piper-";

/// The largest catalog read; `voices.json` is about 250 KB.
const CATALOG_LIMIT: usize = 8 << 20;

/// The Piper voice row's name.
pub const PIPER_VOICE_LABEL: &str = "Piper voice";

/// Where the three catalogs, and the files they name, are fetched from.
/// Injectable, so the tests point everything at an in-process server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiperSources {
    pub voice_me_catalog: String,
    pub official_voices: String,
    /// The root an official file's path is appended to.
    pub official_files: String,
    pub speaches_list: String,
    /// `…/api/models`: a repository's `?blobs=true` is asked here.
    pub hf_api_models: String,
    /// `https://huggingface.co`: a repository's files are resolved here.
    pub hf_base: String,
}

impl Default for PiperSources {
    fn default() -> Self {
        Self::pinned()
    }
}

impl PiperSources {
    /// The real sources.
    pub fn pinned() -> Self {
        Self {
            voice_me_catalog: VOICE_ME_CATALOG_URL.to_string(),
            official_voices: format!(
                "https://huggingface.co/rhasspy/piper-voices/resolve/{OFFICIAL_REVISION}/voices.json"
            ),
            official_files: format!(
                "https://huggingface.co/rhasspy/piper-voices/resolve/{OFFICIAL_REVISION}"
            ),
            speaches_list: SPEACHES_LIST_URL.to_string(),
            hf_api_models: "https://huggingface.co/api/models".to_string(),
            hf_base: "https://huggingface.co".to_string(),
        }
    }
}

/// One file of a voice, where to fetch it and how to check it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    pub url: String,
    pub size: u64,
    pub digest: Digest,
}

/// How a voice's two files are found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceFiles {
    /// Named in the catalog itself.
    Direct {
        model: RemoteFile,
        config: RemoteFile,
    },
    /// A speaches-ai repository, resolved at download.
    Speaches { repo: String },
}

/// One voice a catalog offers: what the tab shows, and how to fetch it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogVoice {
    pub entry: PiperCatalogEntry,
    pub files: VoiceFiles,
}

/// The built-in default voice: `tr_TR-fahrettin-medium` from
/// `speaches-ai/piper-tr_TR-fahrettin-medium` at a pinned revision, both
/// files with a SHA-256 pinned here. CC0 (the NabuCasa dataset).
pub fn default_voice() -> CatalogVoice {
    const REVISION: &str = "aab8f92429ede58091e17de506484a2c84384792";
    let file = |name: &str, size: u64, sha256: &str| RemoteFile {
        url: format!(
            "https://huggingface.co/speaches-ai/piper-tr_TR-fahrettin-medium/resolve/{REVISION}/{name}"
        ),
        size,
        digest: Digest::Sha256(sha256.to_string()),
    };
    CatalogVoice {
        entry: PiperCatalogEntry {
            key: PIPER_DEFAULT_VOICE.key.to_string(),
            name: "fahrettin".to_string(),
            locale: PIPER_DEFAULT_VOICE.locale.to_string(),
            language_label: "Turkish (Turkey)".to_string(),
            quality: "medium".to_string(),
            licence: Some("CC0-1.0".to_string()),
            source: PiperSource::VoiceMe,
            size_bytes: Some(63_201_294 + 5_022),
        },
        files: VoiceFiles::Direct {
            model: file(
                "model.onnx",
                63_201_294,
                "39081c47270180e8a0dfac69b07bf329fb6d039fcc1279dbe26c2daf2848b190",
            ),
            config: file(
                "config.json",
                5_022,
                "93741b234acc5f123430a0e343f619e01cfda6b1dc0970ee80bce7da22b1a09b",
            ),
        },
    }
}

// ---- voice-me's catalog -------------------------------------------------

#[derive(Deserialize)]
struct VoiceMeCatalog {
    voices: Vec<VoiceMeVoice>,
}

#[derive(Deserialize)]
struct VoiceMeVoice {
    key: String,
    name: String,
    locale: String,
    language: String,
    quality: String,
    #[serde(default)]
    licence: Option<String>,
    model: VoiceMeFile,
    config: VoiceMeFile,
}

#[derive(Deserialize)]
struct VoiceMeFile {
    url: String,
    size: u64,
    sha256: String,
}

/// voice-me's `catalog.json`. A voice with an unusable key, a URL that is
/// not HTTPS or a digest that is not a SHA-256 is left out; a file that is
/// not a catalog at all is an `Err`.
pub fn parse_voice_me_catalog(json: &str) -> Result<Vec<CatalogVoice>, String> {
    let catalog: VoiceMeCatalog =
        serde_json::from_str(json).map_err(|error| format!("not a voice catalog: {error}"))?;
    let file = |file: VoiceMeFile| {
        (file.url.starts_with("https://") && is_hex(&file.sha256, 64)).then(|| RemoteFile {
            url: file.url,
            size: file.size,
            digest: Digest::Sha256(file.sha256.to_ascii_lowercase()),
        })
    };
    Ok(catalog
        .voices
        .into_iter()
        .filter(|voice| assets::is_valid_piper_key(&voice.key))
        .filter_map(|voice| {
            let size = voice.model.size + voice.config.size;
            let model = file(voice.model)?;
            let config = file(voice.config)?;
            Some(CatalogVoice {
                entry: PiperCatalogEntry {
                    key: voice.key,
                    name: voice.name,
                    locale: voice.locale,
                    language_label: voice.language,
                    quality: voice.quality,
                    licence: voice.licence.filter(|licence| !licence.trim().is_empty()),
                    source: PiperSource::VoiceMe,
                    size_bytes: Some(size),
                },
                files: VoiceFiles::Direct { model, config },
            })
        })
        .collect())
}

// ---- rhasspy/piper-voices -----------------------------------------------

#[derive(Deserialize)]
struct OfficialVoice {
    key: String,
    name: String,
    language: OfficialLanguage,
    quality: String,
    files: BTreeMap<String, OfficialFile>,
}

#[derive(Deserialize)]
struct OfficialLanguage {
    code: String,
    name_english: String,
    #[serde(default)]
    country_english: Option<String>,
}

#[derive(Deserialize)]
struct OfficialFile {
    size_bytes: u64,
    md5_digest: String,
}

/// `rhasspy/piper-voices`' `voices.json`; each file's URL is `files_base`
/// followed by its path. It declares no licence (the model card does).
pub fn parse_official_catalog(json: &str, files_base: &str) -> Result<Vec<CatalogVoice>, String> {
    let voices: BTreeMap<String, OfficialVoice> =
        serde_json::from_str(json).map_err(|error| format!("not a voices.json: {error}"))?;
    let base = files_base.trim_end_matches('/');
    Ok(voices
        .into_values()
        .filter(|voice| assets::is_valid_piper_key(&voice.key))
        .filter_map(|voice| {
            let find = |suffix: &str| {
                voice
                    .files
                    .iter()
                    .find(|(path, _)| path.ends_with(suffix))
                    .filter(|(_, file)| is_hex(&file.md5_digest, 32))
                    .map(|(path, file)| RemoteFile {
                        url: format!("{base}/{path}"),
                        size: file.size_bytes,
                        digest: Digest::Md5(file.md5_digest.to_ascii_lowercase()),
                    })
            };
            let model = find(".onnx")?;
            let config = find(".onnx.json")?;
            let language = &voice.language;
            let language_label = match &language.country_english {
                Some(country) => format!("{} ({country})", language.name_english),
                None => language.name_english.clone(),
            };
            Some(CatalogVoice {
                entry: PiperCatalogEntry {
                    key: voice.key,
                    name: voice.name,
                    locale: language.code.clone(),
                    language_label,
                    quality: voice.quality,
                    licence: None,
                    source: PiperSource::Official,
                    size_bytes: Some(model.size + config.size),
                },
                files: VoiceFiles::Direct { model, config },
            })
        })
        .collect())
}

// ---- speaches-ai --------------------------------------------------------

#[derive(Deserialize)]
struct HfModel {
    id: String,
}

/// The repository prefix every speaches-ai Piper voice has.
const SPEACHES_PREFIX: &str = "speaches-ai/piper-";

/// The speaches-ai repository list: every `speaches-ai/piper-<locale>-<name>-<quality>`.
/// Sizes and digests come at download, from the repository itself.
pub fn parse_speaches_list(json: &str) -> Result<Vec<CatalogVoice>, String> {
    let models: Vec<HfModel> =
        serde_json::from_str(json).map_err(|error| format!("not a model list: {error}"))?;
    Ok(models
        .into_iter()
        .filter_map(|model| {
            let key = model.id.strip_prefix(SPEACHES_PREFIX)?.to_string();
            let (locale, rest) = key.split_once('-')?;
            let (name, quality) = rest.rsplit_once('-')?;
            if !assets::is_valid_piper_key(&key)
                || locale.is_empty()
                || name.is_empty()
                || !matches!(quality, "x_low" | "low" | "medium" | "high")
            {
                return None;
            }
            Some(CatalogVoice {
                entry: PiperCatalogEntry {
                    name: name.to_string(),
                    locale: locale.to_string(),
                    language_label: locale_label(locale),
                    quality: quality.to_string(),
                    licence: None,
                    source: PiperSource::Speaches,
                    size_bytes: None,
                    key: key.clone(),
                },
                files: VoiceFiles::Speaches { repo: model.id },
            })
        })
        .collect())
}

#[derive(Deserialize)]
struct HfBlobs {
    sha: String,
    siblings: Vec<HfSibling>,
}

#[derive(Deserialize)]
struct HfSibling {
    rfilename: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default, rename = "blobId")]
    blob_id: Option<String>,
    #[serde(default)]
    lfs: Option<HfLfs>,
}

#[derive(Deserialize)]
struct HfLfs {
    sha256: String,
    size: u64,
}

/// A speaches-ai repository's `?blobs=true` answer: the two files at its
/// revision — `model.onnx` by its LFS SHA-256, `config.json` by its size
/// and Git blob SHA-1.
pub fn parse_speaches_blobs(
    json: &str,
    hf_base: &str,
    repo: &str,
) -> Result<(RemoteFile, RemoteFile), String> {
    let blobs: HfBlobs = serde_json::from_str(json)
        .map_err(|error| format!("{repo} did not describe its files: {error}"))?;
    if !is_hex(&blobs.sha, 40) {
        return Err(format!("{repo} gave no usable revision"));
    }
    let url = |name: &str| {
        format!(
            "{}/{repo}/resolve/{}/{name}",
            hf_base.trim_end_matches('/'),
            blobs.sha
        )
    };
    let sibling = |name: &str| {
        blobs
            .siblings
            .iter()
            .find(|sibling| sibling.rfilename == name)
            .ok_or_else(|| format!("{repo} has no {name}"))
    };
    let model = sibling("model.onnx")?;
    let lfs = model
        .lfs
        .as_ref()
        .filter(|lfs| is_hex(&lfs.sha256, 64))
        .ok_or_else(|| format!("{repo} gives no SHA-256 for model.onnx"))?;
    let config = sibling("config.json")?;
    let (Some(size), Some(blob)) = (config.size, config.blob_id.as_ref()) else {
        return Err(format!("{repo} gives no size or blob id for config.json"));
    };
    if !is_hex(blob, 40) {
        return Err(format!("{repo} gives no usable blob id for config.json"));
    }
    Ok((
        RemoteFile {
            url: url("model.onnx"),
            size: lfs.size,
            digest: Digest::Sha256(lfs.sha256.to_ascii_lowercase()),
        },
        RemoteFile {
            url: url("config.json"),
            size,
            digest: Digest::GitBlobSha1(blob.to_ascii_lowercase()),
        },
    ))
}

/// `tr_TR` → "Turkish (TR)": the language named in English where it is one
/// of Piper's, the code as it is otherwise.
fn locale_label(locale: &str) -> String {
    let (language, region) = locale.split_once('_').unwrap_or((locale, ""));
    let name = match language {
        "ar" => "Arabic",
        "ca" => "Catalan",
        "cs" => "Czech",
        "cy" => "Welsh",
        "da" => "Danish",
        "de" => "German",
        "el" => "Greek",
        "en" => "English",
        "es" => "Spanish",
        "fa" => "Persian",
        "fi" => "Finnish",
        "fr" => "French",
        "hi" => "Hindi",
        "hu" => "Hungarian",
        "id" => "Indonesian",
        "is" => "Icelandic",
        "it" => "Italian",
        "ka" => "Georgian",
        "kk" => "Kazakh",
        "lb" => "Luxembourgish",
        "lv" => "Latvian",
        "ml" => "Malayalam",
        "ne" => "Nepali",
        "nl" => "Dutch",
        "no" => "Norwegian",
        "pl" => "Polish",
        "pt" => "Portuguese",
        "ro" => "Romanian",
        "ru" => "Russian",
        "sk" => "Slovak",
        "sl" => "Slovenian",
        "sr" => "Serbian",
        "sv" => "Swedish",
        "sw" => "Swahili",
        "te" => "Telugu",
        "tr" => "Turkish",
        "uk" => "Ukrainian",
        "vi" => "Vietnamese",
        "zh" => "Chinese",
        _ => return locale.to_string(),
    };
    if region.is_empty() {
        name.to_string()
    } else {
        format!("{name} ({region})")
    }
}

fn is_hex(text: &str, len: usize) -> bool {
    text.len() == len && text.chars().all(|c| c.is_ascii_hexdigit())
}

// ---- merging ------------------------------------------------------------

/// Deduplicate `results`, which are in precedence order: a key an earlier
/// source (or earlier in the same source) listed is dropped. A failed
/// source stays a failure.
pub fn dedupe(
    results: Vec<(PiperSource, Result<Vec<CatalogVoice>, String>)>,
) -> Vec<(PiperSource, Result<Vec<CatalogVoice>, String>)> {
    let mut seen = HashSet::new();
    results
        .into_iter()
        .map(|(source, result)| {
            let result = result.map(|voices| {
                voices
                    .into_iter()
                    .filter(|voice| seen.insert(voice.entry.key.clone()))
                    .collect()
            });
            (source, result)
        })
        .collect()
}

/// What the port hands the tab: the entries, per source.
pub fn catalog_results(
    results: &[(PiperSource, Result<Vec<CatalogVoice>, String>)],
) -> Vec<CatalogResult> {
    results
        .iter()
        .map(|(source, result)| CatalogResult {
            source: *source,
            result: result
                .as_ref()
                .map(|voices| voices.iter().map(|voice| voice.entry.clone()).collect())
                .map_err(Clone::clone),
        })
        .collect()
}

// ---- network ------------------------------------------------------------

/// GET `url` as text, bounded, over the one client shape this crate uses.
async fn get_text(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let mut response = client.get(url).send().await.map_err(|error| {
        format!(
            "could not reach {url}: {}",
            crate::provision::reason(&error)
        )
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("{url} answered {status}"));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("reading {url} failed: {}", crate::provision::reason(&error)))?
    {
        body.extend_from_slice(&chunk);
        if body.len() > CATALOG_LIMIT {
            return Err(format!("{url} is larger than a catalog can be"));
        }
    }
    String::from_utf8(body).map_err(|_| format!("{url} is not text"))
}

/// Fetch the three catalogs, in precedence order, deduplicated.
pub fn fetch_catalogs(
    sources: &PiperSources,
) -> Vec<(PiperSource, Result<Vec<CatalogVoice>, String>)> {
    let fetched = crate::block_on(async {
        let client = crate::provision::client()?;
        let voice_me = get_text(&client, &sources.voice_me_catalog)
            .await
            .and_then(|json| parse_voice_me_catalog(&json));
        let official = get_text(&client, &sources.official_voices)
            .await
            .and_then(|json| parse_official_catalog(&json, &sources.official_files));
        let speaches = get_text(&client, &sources.speaches_list)
            .await
            .and_then(|json| parse_speaches_list(&json));
        Ok(vec![
            (PiperSource::VoiceMe, voice_me),
            (PiperSource::Official, official),
            (PiperSource::Speaches, speaches),
        ])
    });
    match fetched {
        Ok(results) => dedupe(results),
        Err(error) => PiperSource::ALL
            .into_iter()
            .map(|source| (source, Err(error.to_string())))
            .collect(),
    }
}

/// The two files of `voice`, resolving a speaches-ai repository first.
fn resolve_files(
    voice: &CatalogVoice,
    sources: &PiperSources,
) -> Result<(RemoteFile, RemoteFile), VoiceMeError> {
    match &voice.files {
        VoiceFiles::Direct { model, config } => Ok((model.clone(), config.clone())),
        VoiceFiles::Speaches { repo } => {
            let url = format!(
                "{}/{repo}?blobs=true",
                sources.hf_api_models.trim_end_matches('/')
            );
            crate::block_on(async {
                let client = crate::provision::client()?;
                let json = get_text(&client, &url).await.map_err(VoiceMeError::Other)?;
                parse_speaches_blobs(&json, &sources.hf_base, repo).map_err(VoiceMeError::Other)
            })
        }
    }
}

/// Download `voice` into `<root>/piper/<key>/`, reporting on `target`:
/// each file not already exactly the one named (its size and digest)
/// through `.part`, verified, moved into place — so installing a voice
/// whose catalog entry changed replaces only what changed; then
/// `voice.toml`, which is what makes it installed.
pub fn install_voice(
    root: &Path,
    voice: &CatalogVoice,
    sources: &PiperSources,
    target: ProgressTarget,
    events: &AppEventSender,
) -> Result<(), VoiceMeError> {
    let key = &voice.entry.key;
    let files = assets::piper_voice_files(root, key)
        .ok_or_else(|| VoiceMeError::Other(format!("{key:?} is not a voice name")))?;
    let (model, config) = resolve_files(voice, sources)?;
    let model_sha256 = sha256_of(&model.digest);
    let config_sha256 = sha256_of(&config.digest);

    let mut plan = Vec::new();
    for (remote, destination, file_name) in [
        (model, &files.model, assets::PIPER_MODEL_FILE),
        (config, &files.config, assets::PIPER_CONFIG_FILE),
    ] {
        let asset = Asset {
            relative_path: format!("{}/{key}/{file_name}", assets::PIPER_DIR),
            url: remote.url,
            size: remote.size,
            digest: remote.digest,
        };
        if crate::provision::is_verified(&asset, destination) {
            continue;
        }
        plan.push(PlannedDownload {
            asset,
            destination: destination.clone(),
        });
    }
    crate::fetch_to(target, &plan, events)?;

    let manifest = PiperVoiceManifest {
        name: voice.entry.name.clone(),
        locale: voice.entry.locale.clone(),
        label: voice.entry.language_label.clone(),
        quality: voice.entry.quality.clone(),
        source: voice.entry.source.label().to_string(),
        licence: voice.entry.licence.clone(),
        model_sha256,
        config_sha256,
    };
    write_manifest(&files, key, &manifest)
}

/// The SHA-256 `digest` names, if it is one.
fn sha256_of(digest: &Digest) -> Option<String> {
    match digest {
        Digest::Sha256(hex) => Some(hex.to_ascii_lowercase()),
        _ => None,
    }
}

/// Write `voice.toml` through `.part` and a rename.
fn write_manifest(
    files: &assets::PiperVoiceFiles,
    key: &str,
    manifest: &PiperVoiceManifest,
) -> Result<(), VoiceMeError> {
    let part = crate::provision::part_path(&files.manifest);
    let written = manifest
        .to_toml()
        .and_then(|text| std::fs::write(&part, text).map_err(VoiceMeError::from))
        .and_then(|()| std::fs::rename(&part, &files.manifest).map_err(VoiceMeError::from));
    written.map_err(|error| {
        let _ = std::fs::remove_file(&part);
        VoiceMeError::Other(format!(
            "Could not record {key} in {}: {error}",
            files.dir.display()
        ))
    })
}

/// The voice the tab's `entry` downloads: the one the last catalog fetch
/// listed under that key *and* source, or — for the default voice's key —
/// the built-in pinned one, whichever source the tab listed it under (it
/// shows under speaches-ai while voice-me's own catalog cannot be read).
pub fn voice_for_entry(listed: &[CatalogVoice], entry: &PiperCatalogEntry) -> Option<CatalogVoice> {
    listed
        .iter()
        .find(|voice| voice.entry.key == entry.key && voice.entry.source == entry.source)
        .cloned()
        .or_else(|| (entry.key == PIPER_DEFAULT_VOICE.key).then(default_voice))
}

// ---- updates ------------------------------------------------------------

/// Fetch voice-me's own catalog alone: what an update checks against.
pub fn fetch_voice_me_catalog(sources: &PiperSources) -> Result<Vec<CatalogVoice>, String> {
    crate::block_on(async {
        let client = crate::provision::client()?;
        Ok(get_text(&client, &sources.voice_me_catalog)
            .await
            .and_then(|json| parse_voice_me_catalog(&json)))
    })
    .unwrap_or_else(|error| Err(error.to_string()))
}

/// The installed voices from voice-me's own catalog whose files are no
/// longer the ones `listed` names: a custom voice whose model was replaced
/// at the same URL. Each comes back as the catalog voice to install again.
///
/// A `voice.toml` records the SHA-256 of what was installed. One written
/// before it did is checked by hashing the files once; when they still
/// match, the digests are recorded so the next check reads them instead.
pub fn outdated_custom_voices(root: &Path, listed: &[CatalogVoice]) -> Vec<CatalogVoice> {
    assets::installed_piper_voices(root)
        .into_iter()
        .filter(|installed| installed.manifest.source == PiperSource::VoiceMe.label())
        .filter_map(|installed| {
            let voice = listed.iter().find(|voice| {
                voice.entry.key == installed.key && voice.entry.source == PiperSource::VoiceMe
            })?;
            let VoiceFiles::Direct { model, config } = &voice.files else {
                return None;
            };
            let (want_model, want_config) = (sha256_of(&model.digest)?, sha256_of(&config.digest)?);
            let files = assets::piper_voice_files(root, &installed.key)?;
            let mut manifest = installed.manifest;
            if manifest.model_sha256.is_none() || manifest.config_sha256.is_none() {
                let hash = |path: &Path| crate::provision::sha256_file(path).ok();
                manifest.model_sha256 = hash(&files.model);
                manifest.config_sha256 = hash(&files.config);
                let current = manifest.model_sha256.as_deref() == Some(want_model.as_str())
                    && manifest.config_sha256.as_deref() == Some(want_config.as_str());
                if current {
                    // Best effort: failing to record only means hashing again.
                    let _ = write_manifest(&files, &installed.key, &manifest);
                }
            }
            let current = manifest.model_sha256.as_deref() == Some(want_model.as_str())
                && manifest.config_sha256.as_deref() == Some(want_config.as_str());
            (!current).then(|| voice.clone())
        })
        .collect()
}

/// Delete voice `key` from `<root>/piper/`.
pub fn delete_voice(root: &Path, key: &str) -> Result<(), VoiceMeError> {
    let files = assets::piper_voice_files(root, key)
        .ok_or_else(|| VoiceMeError::Other(format!("{key:?} is not a voice name")))?;
    match std::fs::remove_dir_all(&files.dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(VoiceMeError::Other(format!(
            "Could not delete {key} from {}: {error}",
            files.dir.display()
        ))),
    }
}

// ---- the row ------------------------------------------------------------

/// The Piper voice row: is the voice the selection speaks in installed?
///
/// * nothing installed at all: missing, and Install downloads the default
///   voice;
/// * `voice` named but not installed (deleted, say): missing, and Install
///   downloads it when voice-me knows where from (`known`), otherwise
///   manual steps;
/// * no voice can be named for the saved language: missing, manual.
pub fn piper_voice_row(
    root: &Path,
    voice: Option<&str>,
    known: &dyn Fn(&str) -> bool,
) -> Dependency {
    let installed = assets::installed_piper_voices(root);
    let default_key = assets::PIPER_DEFAULT_VOICE.key;
    if installed.is_empty() && voice.is_none_or(|key| key == default_key) {
        let default = default_voice();
        return Dependency::missing(
            DependencyKind::PiperVoice,
            PIPER_VOICE_LABEL,
            format!(
                "No Piper voice installed. Install downloads {} ({}, {}, {}).",
                default.entry.name,
                default.entry.language_label,
                format_bytes(default.entry.size_bytes.unwrap_or_default()),
                default.entry.licence.unwrap_or_default()
            ),
        );
    }
    match voice {
        Some(key) => match installed.iter().find(|voice| voice.key == key) {
            Some(voice) => Dependency::ready(
                DependencyKind::PiperVoice,
                PIPER_VOICE_LABEL,
                format!(
                    "{} ({}) is installed in {}.",
                    voice.manifest.name,
                    voice.manifest.quality,
                    assets::piper_dir(root).join(&voice.key).display()
                ),
            ),
            None => {
                let row = Dependency::missing(
                    DependencyKind::PiperVoice,
                    PIPER_VOICE_LABEL,
                    format!("The selected voice {key} is not installed."),
                );
                if known(key) {
                    row
                } else {
                    row.manual([
                        "Download it again in Settings → Piper voices, or pick an installed \
                         voice in Settings → Backend.",
                        "Press Check again.",
                    ])
                }
            }
        },
        None => Dependency::missing(
            DependencyKind::PiperVoice,
            PIPER_VOICE_LABEL,
            "No installed Piper voice speaks the selected language.",
        )
        .manual([
            "Choose another language or voice in Settings → Backend, or download one in \
             Settings → Piper voices.",
            "Press Check again.",
        ]),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use voice_me_core::{AppEvent, DependencyStatus};

    use super::*;
    use crate::provision_tests::TestServer;
    use crate::test_support::EnvGuard;

    const OFFICIAL: &str = include_str!("../fixtures/piper/voices.json");
    const SPEACHES_LIST: &str = include_str!("../fixtures/piper/speaches-list.json");
    const SPEACHES_BLOBS: &str = include_str!("../fixtures/piper/speaches-dfki-blobs.json");
    const VOICE_ME: &str = include_str!("../../../piper-voices/catalog.json");

    #[test]
    fn voice_mes_own_catalog_lists_the_default_voice_as_pinned_in_code() {
        let voices = parse_voice_me_catalog(VOICE_ME).unwrap();
        let fahrettin = voices
            .iter()
            .find(|voice| voice.entry.key == PIPER_DEFAULT_VOICE.key)
            .expect("the catalog lists fahrettin");
        assert_eq!(*fahrettin, default_voice());
    }

    #[test]
    fn a_voice_me_entry_that_is_not_https_or_sha256_is_left_out() {
        let json = r#"{"voices":[
            {"key":"a_A-x-low","name":"x","locale":"a_A","language":"A","quality":"low",
             "model":{"url":"http://x/model.onnx","size":1,"sha256":"00"},
             "config":{"url":"https://x/config.json","size":1,"sha256":"00"}},
            {"key":"../evil","name":"x","locale":"a_A","language":"A","quality":"low",
             "model":{"url":"https://x/m","size":1,"sha256":"0000000000000000000000000000000000000000000000000000000000000000"},
             "config":{"url":"https://x/c","size":1,"sha256":"0000000000000000000000000000000000000000000000000000000000000000"}}
        ]}"#;
        assert!(parse_voice_me_catalog(json).unwrap().is_empty());
        assert!(parse_voice_me_catalog("[]").is_err());
    }

    #[test]
    fn the_official_catalog_parses_with_md5_digests_and_no_licence() {
        let voices = parse_official_catalog(OFFICIAL, "https://hf.example/resolve/rev").unwrap();
        assert_eq!(voices.len(), 3);
        let dfki = voices
            .iter()
            .find(|voice| voice.entry.key == "tr_TR-dfki-medium")
            .unwrap();
        assert_eq!(dfki.entry.locale, "tr_TR");
        assert_eq!(dfki.entry.language_label, "Turkish (Turkey)");
        assert_eq!(dfki.entry.licence, None);
        assert_eq!(dfki.entry.source, PiperSource::Official);
        assert_eq!(dfki.entry.size_bytes, Some(63_201_294 + 4_960));
        let VoiceFiles::Direct { model, config } = &dfki.files else {
            panic!("official files are named directly");
        };
        assert_eq!(
            model.url,
            "https://hf.example/resolve/rev/tr/tr_TR/dfki/medium/tr_TR-dfki-medium.onnx"
        );
        assert_eq!(
            model.digest,
            Digest::Md5("f51287b350a042dd8d67b2b215145e5a".to_string())
        );
        assert!(config.url.ends_with("tr_TR-dfki-medium.onnx.json"));
    }

    #[test]
    fn the_speaches_list_parses_its_repository_names() {
        let voices = parse_speaches_list(SPEACHES_LIST).unwrap();
        let keys: Vec<_> = voices
            .iter()
            .map(|voice| voice.entry.key.as_str())
            .collect();
        assert_eq!(
            keys,
            vec![
                "en_US-lessac-medium",
                "tr_TR-dfki-medium",
                "tr_TR-fahrettin-medium",
                "tr_TR-fettah-medium"
            ]
        );
        let fettah = &voices[3];
        assert_eq!(fettah.entry.name, "fettah");
        assert_eq!(fettah.entry.quality, "medium");
        assert_eq!(fettah.entry.language_label, "Turkish (TR)");
        assert_eq!(fettah.entry.size_bytes, None);
        assert_eq!(
            fettah.files,
            VoiceFiles::Speaches {
                repo: "speaches-ai/piper-tr_TR-fettah-medium".to_string()
            }
        );
    }

    #[test]
    fn a_speaches_repository_resolves_to_its_revision_and_digests() {
        let (model, config) = parse_speaches_blobs(
            SPEACHES_BLOBS,
            "https://huggingface.co",
            "speaches-ai/piper-tr_TR-dfki-medium",
        )
        .unwrap();
        assert_eq!(
            model.url,
            "https://huggingface.co/speaches-ai/piper-tr_TR-dfki-medium/resolve/\
             f7b897c16fc09f627b14b40b492a5d70a1dc455a/model.onnx"
        );
        assert_eq!(model.size, 63_201_294);
        assert_eq!(
            model.digest,
            Digest::Sha256(
                "2844717f524ab965d3fe86e60562cbb601d3e456836efcc2196cc3a14112a8fb".to_string()
            )
        );
        assert_eq!(config.size, 4_960);
        assert_eq!(
            config.digest,
            Digest::GitBlobSha1("ea4036df5c9e4e07b8c2f42acce4d77d0fb6afdc".to_string())
        );
        assert!(parse_speaches_blobs("{}", "https://huggingface.co", "r").is_err());
    }

    /// The first source wins a duplicate; a failed source stays one line.
    #[test]
    fn duplicates_go_to_the_first_source_in_order() {
        let voice_me = parse_voice_me_catalog(VOICE_ME).unwrap();
        let official = parse_official_catalog(OFFICIAL, "https://x").unwrap();
        let speaches = parse_speaches_list(SPEACHES_LIST).unwrap();
        let merged = dedupe(vec![
            (PiperSource::VoiceMe, Ok(voice_me)),
            (PiperSource::Official, Ok(official)),
            (PiperSource::Speaches, Ok(speaches)),
        ]);
        let keys = |index: usize| -> Vec<String> {
            merged[index]
                .1
                .as_ref()
                .unwrap()
                .iter()
                .map(|voice| voice.entry.key.clone())
                .collect()
        };
        assert!(keys(0).contains(&"tr_TR-fahrettin-medium".to_string()));
        assert!(keys(1).contains(&"tr_TR-dfki-medium".to_string()));
        assert_eq!(keys(2), vec!["tr_TR-fettah-medium".to_string()]);

        let with_failure = dedupe(vec![
            (PiperSource::VoiceMe, Err("offline".to_string())),
            (
                PiperSource::Speaches,
                Ok(parse_speaches_list(SPEACHES_LIST).unwrap()),
            ),
        ]);
        assert_eq!(with_failure[0].1, Err("offline".to_string()));
        assert_eq!(with_failure[1].1.as_ref().unwrap().len(), 4);
        let results = catalog_results(&with_failure);
        assert_eq!(results[0].result, Err("offline".to_string()));
        assert_eq!(results[1].result.as_ref().unwrap().len(), 4);
    }

    /// A tab Download finds its voice by key and source; the default voice
    /// is found even when only speaches-ai listed it (voice-me's own
    /// catalog unreachable), and a voice gone from the catalog is not.
    #[test]
    fn a_download_finds_its_voice_by_key_and_source_and_the_default_always() {
        let listed = dedupe(vec![
            (PiperSource::VoiceMe, Err("offline".to_string())),
            (
                PiperSource::Official,
                parse_official_catalog(OFFICIAL, "https://x"),
            ),
            (PiperSource::Speaches, parse_speaches_list(SPEACHES_LIST)),
        ])
        .into_iter()
        .filter_map(|(_, result)| result.ok())
        .flatten()
        .collect::<Vec<_>>();
        let entry_of = |key: &str| {
            listed
                .iter()
                .find(|voice| voice.entry.key == key)
                .unwrap()
                .entry
                .clone()
        };

        let fahrettin = entry_of(PIPER_DEFAULT_VOICE.key);
        assert_eq!(fahrettin.source, PiperSource::Speaches);
        assert!(voice_for_entry(&listed, &fahrettin).is_some());
        assert!(voice_for_entry(&[], &fahrettin).is_some(), "built in");

        let dfki = entry_of("tr_TR-dfki-medium");
        assert_eq!(
            voice_for_entry(&listed, &dfki).unwrap().entry.source,
            PiperSource::Official
        );
        assert!(voice_for_entry(&[], &dfki).is_none());
        let mut moved = dfki.clone();
        moved.source = PiperSource::Speaches;
        assert!(voice_for_entry(&listed, &moved).is_none());
    }

    fn manifest(key: &str) -> String {
        format!(
            "name = \"{key}\"\nlocale = \"tr_TR\"\nlabel = \"Turkish\"\nquality = \"medium\"\n\
             source = \"voice-me\"\n"
        )
    }

    fn install_fake(root: &Path, key: &str) {
        let files = assets::piper_voice_files(root, key).unwrap();
        std::fs::create_dir_all(&files.dir).unwrap();
        std::fs::write(&files.model, b"m").unwrap();
        std::fs::write(&files.config, b"c").unwrap();
        std::fs::write(&files.manifest, manifest(key)).unwrap();
    }

    #[test]
    fn the_voice_row_says_none_installed_selected_missing_or_ready() {
        let root = tempfile::tempdir().unwrap();
        let known = |key: &str| key == PIPER_DEFAULT_VOICE.key;

        let none = piper_voice_row(root.path(), Some(PIPER_DEFAULT_VOICE.key), &known);
        assert_eq!(none.status, DependencyStatus::Missing);
        assert!(none.automatable, "Install downloads fahrettin");
        assert!(
            none.detail.contains("No Piper voice installed"),
            "{}",
            none.detail
        );
        assert!(none.detail.contains("fahrettin"), "{}", none.detail);
        assert!(none.kind.blocks_speech());

        install_fake(root.path(), "tr_TR-dfki-medium");
        let missing = piper_voice_row(root.path(), Some(PIPER_DEFAULT_VOICE.key), &known);
        assert_eq!(missing.status, DependencyStatus::Missing);
        assert!(missing.automatable, "a known voice installs with one click");
        assert!(missing.detail.contains(PIPER_DEFAULT_VOICE.key));

        let unknown = piper_voice_row(root.path(), Some("xx_XX-gone-low"), &known);
        assert!(!unknown.automatable);
        assert!(!unknown.manual_steps.is_empty());

        let ready = piper_voice_row(root.path(), Some("tr_TR-dfki-medium"), &known);
        assert_eq!(ready.status, DependencyStatus::Ready);

        let no_voice = piper_voice_row(root.path(), None, &known);
        assert_eq!(no_voice.status, DependencyStatus::Missing);
        assert!(!no_voice.automatable);
    }

    #[test]
    fn an_empty_cache_with_a_named_voice_asks_for_that_voice_not_the_default() {
        let root = tempfile::tempdir().unwrap();
        let known = |key: &str| key == "tr_TR-dfki-medium" || key == PIPER_DEFAULT_VOICE.key;

        let row = piper_voice_row(root.path(), Some("tr_TR-dfki-medium"), &known);
        assert_eq!(row.status, DependencyStatus::Missing);
        assert!(row.automatable, "a known voice installs with one click");
        assert!(row.detail.contains("tr_TR-dfki-medium"), "{}", row.detail);
        assert!(
            !row.detail.contains("No Piper voice installed"),
            "{}",
            row.detail
        );
        assert!(!row.detail.contains("fahrettin"), "{}", row.detail);
    }

    fn voice_served_by(
        server: &TestServer,
        key: &str,
        model: &[u8],
        config: &[u8],
    ) -> CatalogVoice {
        use sha2::Digest as _;
        let sha = |bytes: &[u8]| -> String {
            sha2::Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        };
        CatalogVoice {
            entry: PiperCatalogEntry {
                key: key.to_string(),
                name: "dfki".to_string(),
                locale: "tr_TR".to_string(),
                language_label: "Turkish (Turkey)".to_string(),
                quality: "medium".to_string(),
                licence: None,
                source: PiperSource::Official,
                size_bytes: Some((model.len() + config.len()) as u64),
            },
            files: VoiceFiles::Direct {
                model: RemoteFile {
                    url: format!("{}/model.onnx", server.base),
                    size: model.len() as u64,
                    digest: Digest::Sha256(sha(model)),
                },
                config: RemoteFile {
                    url: format!("{}/config.json", server.base),
                    size: config.len() as u64,
                    digest: Digest::Md5(
                        md5::Md5::digest(config)
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect(),
                    ),
                },
            },
        }
    }

    fn no_proxy() -> EnvGuard {
        EnvGuard::new()
            .unset("http_proxy")
            .unset("HTTP_PROXY")
            .unset("all_proxy")
            .unset("ALL_PROXY")
    }

    /// The Download row: progress, then verified, moved into place and
    /// recorded — and it is installed.
    #[test]
    fn a_download_reports_progress_verifies_and_records_the_voice() {
        let _env = no_proxy();
        let model = vec![7_u8; 5_000];
        let config = b"{\"audio\":{}}".to_vec();
        let server = TestServer::start(HashMap::from([
            ("model.onnx".to_string(), model.clone()),
            ("config.json".to_string(), config.clone()),
        ]));
        let voice = voice_served_by(&server, "tr_TR-dfki-medium", &model, &config);
        let root = tempfile::tempdir().unwrap();
        let (tx, mut rx) = futures::channel::mpsc::unbounded();

        install_voice(
            root.path(),
            &voice,
            &PiperSources::pinned(),
            ProgressTarget::PiperVoice("tr_TR-dfki-medium".to_string()),
            &tx,
        )
        .unwrap();

        let mut progress = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let AppEvent::PiperVoiceProgress {
                key,
                done_bytes,
                total_bytes,
            } = event
            {
                assert_eq!(key, "tr_TR-dfki-medium");
                progress.push((done_bytes, total_bytes));
            }
        }
        let total = (model.len() + config.len()) as u64;
        assert_eq!(progress.last(), Some(&(total, total)), "{progress:?}");
        let installed = assets::installed_piper_voices(root.path());
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].manifest.source, "rhasspy/piper-voices");
        assert_eq!(installed[0].manifest.licence, None);

        delete_voice(root.path(), "tr_TR-dfki-medium").unwrap();
        assert!(assets::installed_piper_voices(root.path()).is_empty());
        assert!(delete_voice(root.path(), "../..").is_err());
    }

    /// The Download row's bad hash: the `.part` is removed, the row gets
    /// the error, and nothing is installed.
    #[test]
    fn a_bad_hash_removes_the_part_and_installs_nothing() {
        let _env = no_proxy();
        let model = vec![7_u8; 5_000];
        let config = b"{}".to_vec();
        let server = TestServer::start(HashMap::from([
            ("model.onnx".to_string(), model.clone()),
            ("config.json".to_string(), config.clone()),
        ]));
        let mut voice = voice_served_by(&server, "tr_TR-dfki-medium", &model, &config);
        if let VoiceFiles::Direct { model, .. } = &mut voice.files {
            model.digest = Digest::Md5("0".repeat(32));
        }
        let root = tempfile::tempdir().unwrap();
        let (tx, _rx) = futures::channel::mpsc::unbounded();

        let error = install_voice(
            root.path(),
            &voice,
            &PiperSources::pinned(),
            ProgressTarget::PiperVoice("tr_TR-dfki-medium".to_string()),
            &tx,
        )
        .unwrap_err()
        .to_string();

        assert!(
            error.contains("model.onnx") && error.contains("checksum"),
            "{error}"
        );
        let files = assets::piper_voice_files(root.path(), "tr_TR-dfki-medium").unwrap();
        assert!(!crate::provision::part_path(&files.model).exists());
        assert!(!files.model.exists());
        assert!(assets::installed_piper_voices(root.path()).is_empty());
    }

    /// A speaches-ai voice is resolved through its repository first, then
    /// fetched at that revision, `config.json` checked by its Git blob id.
    #[test]
    fn a_speaches_voice_is_resolved_then_fetched_at_its_revision() {
        use sha1::Digest as _;
        let _env = no_proxy();
        let model = vec![3_u8; 4_000];
        let config = b"{\"espeak\":{\"voice\":\"tr\"}}".to_vec();
        let model_sha: String = {
            use sha2::Digest as _;
            sha2::Sha256::digest(&model)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        };
        let mut hasher = sha1::Sha1::new();
        hasher.update(format!("blob {}\0", config.len()).as_bytes());
        hasher.update(&config);
        let blob: String = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let revision = "a".repeat(40);
        let blobs = format!(
            r#"{{"sha":"{revision}","siblings":[
                {{"rfilename":"config.json","blobId":"{blob}","size":{}}},
                {{"rfilename":"model.onnx","blobId":"x","size":{},
                  "lfs":{{"sha256":"{model_sha}","size":{}}}}}]}}"#,
            config.len(),
            model.len(),
            model.len()
        );
        let repo = "speaches-ai/piper-tr_TR-fettah-medium";
        let server = TestServer::start(HashMap::from([
            (format!("api/models/{repo}?blobs=true"), blobs.into_bytes()),
            (
                format!("{repo}/resolve/{revision}/model.onnx"),
                model.clone(),
            ),
            (
                format!("{repo}/resolve/{revision}/config.json"),
                config.clone(),
            ),
        ]));
        let sources = PiperSources {
            hf_api_models: format!("{}/api/models", server.base),
            hf_base: server.base.clone(),
            ..PiperSources::pinned()
        };
        let voice = parse_speaches_list(SPEACHES_LIST)
            .unwrap()
            .into_iter()
            .find(|voice| voice.entry.key == "tr_TR-fettah-medium")
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        let (tx, _rx) = futures::channel::mpsc::unbounded();

        install_voice(
            root.path(),
            &voice,
            &sources,
            ProgressTarget::PiperVoice(voice.entry.key.clone()),
            &tx,
        )
        .unwrap();

        let installed = assets::installed_piper_voices(root.path());
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].manifest.source, "speaches-ai");
        assert_eq!(installed[0].size_bytes, (model.len() + config.len()) as u64);
    }

    /// The three catalogs fetched from a server: one that fails is its own
    /// line; the others are still listed.
    #[test]
    fn a_failed_catalog_is_one_error_and_the_others_still_list() {
        let _env = no_proxy();
        let server = TestServer::start(HashMap::from([
            ("voices.json".to_string(), OFFICIAL.as_bytes().to_vec()),
            ("list".to_string(), SPEACHES_LIST.as_bytes().to_vec()),
        ]));
        let sources = PiperSources {
            voice_me_catalog: format!("{}/missing-catalog.json", server.base),
            official_voices: format!("{}/voices.json", server.base),
            official_files: format!("{}/files", server.base),
            speaches_list: format!("{}/list", server.base),
            ..PiperSources::pinned()
        };

        let results = fetch_catalogs(&sources);

        assert_eq!(results.len(), 3);
        assert!(
            results[0].1.as_ref().unwrap_err().contains("404"),
            "{:?}",
            results[0].1
        );
        assert_eq!(results[1].1.as_ref().unwrap().len(), 3);
        // dfki and lessac are the official catalog's; fahrettin and fettah
        // are left to speaches-ai.
        let speaches: Vec<_> = results[2]
            .1
            .as_ref()
            .unwrap()
            .iter()
            .map(|voice| voice.entry.key.clone())
            .collect();
        assert_eq!(
            speaches,
            vec!["tr_TR-fahrettin-medium", "tr_TR-fettah-medium"]
        );
    }

    /// A custom voice as voice-me's own catalog lists it: both files by
    /// SHA-256, served by `server`.
    fn custom_voice_served_by(
        server: &TestServer,
        key: &str,
        model: &[u8],
        config: &[u8],
    ) -> CatalogVoice {
        use sha2::Digest as _;
        let sha = |bytes: &[u8]| -> String {
            sha2::Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        };
        let mut voice = voice_served_by(server, key, model, config);
        voice.entry.source = PiperSource::VoiceMe;
        if let VoiceFiles::Direct { config: file, .. } = &mut voice.files {
            file.digest = Digest::Sha256(sha(config));
        }
        voice
    }

    /// A custom model replaced at the same URL: the installed copy is
    /// outdated, installing again replaces only the changed file and
    /// records its digest, and then it is current.
    #[test]
    fn a_custom_voice_whose_model_changed_is_updated_in_place() {
        let _env = no_proxy();
        let key = "tr_TR-erdem-medium";
        let config = b"{\"audio\":{}}".to_vec();
        let (old_model, new_model) = (vec![1_u8; 4_000], vec![2_u8; 4_100]);
        let old_server = TestServer::start(HashMap::from([
            ("model.onnx".to_string(), old_model.clone()),
            ("config.json".to_string(), config.clone()),
        ]));
        let root = tempfile::tempdir().unwrap();
        let (tx, _rx) = futures::channel::mpsc::unbounded();
        let sources = PiperSources::pinned();
        let old = custom_voice_served_by(&old_server, key, &old_model, &config);
        install_voice(
            root.path(),
            &old,
            &sources,
            ProgressTarget::PiperVoice(key.into()),
            &tx,
        )
        .unwrap();
        assert!(outdated_custom_voices(root.path(), std::slice::from_ref(&old)).is_empty());

        // The same URLs now serve the new model; the config is unchanged.
        let new_server = TestServer::start(HashMap::from([(
            "model.onnx".to_string(),
            new_model.clone(),
        )]));
        let mut new = custom_voice_served_by(&new_server, key, &new_model, &config);
        if let (VoiceFiles::Direct { config: new, .. }, VoiceFiles::Direct { config: old, .. }) =
            (&mut new.files, &old.files)
        {
            new.url = old.url.clone();
        }
        let outdated = outdated_custom_voices(root.path(), std::slice::from_ref(&new));
        assert_eq!(outdated, vec![new.clone()]);

        drop(old_server);
        install_voice(
            root.path(),
            &new,
            &sources,
            ProgressTarget::PiperVoice(key.into()),
            &tx,
        )
        .unwrap();
        let files = assets::piper_voice_files(root.path(), key).unwrap();
        assert_eq!(std::fs::read(&files.model).unwrap(), new_model);
        assert!(outdated_custom_voices(root.path(), std::slice::from_ref(&new)).is_empty());
        let installed = assets::installed_piper_voices(root.path());
        assert_eq!(
            installed[0].manifest.model_sha256.as_deref(),
            sha256_of(match &new.files {
                VoiceFiles::Direct { model, .. } => &model.digest,
                VoiceFiles::Speaches { .. } => unreachable!(),
            })
            .as_deref()
        );
    }

    /// A `voice.toml` from before digests were recorded: a copy that still
    /// matches is current and gets its digests recorded; a voice from
    /// another catalog is never updated here.
    #[test]
    fn an_older_manifest_is_checked_by_hashing_once_and_other_sources_are_left_alone() {
        let key = "tr_TR-erdem-medium";
        let (model, config) = (vec![3_u8; 1_000], b"{}".to_vec());
        let server = TestServer::start(HashMap::new());
        let voice = custom_voice_served_by(&server, key, &model, &config);
        let root = tempfile::tempdir().unwrap();
        let files = assets::piper_voice_files(root.path(), key).unwrap();
        std::fs::create_dir_all(&files.dir).unwrap();
        std::fs::write(&files.model, &model).unwrap();
        std::fs::write(&files.config, &config).unwrap();
        std::fs::write(
            &files.manifest,
            "name = \"erdem\"\nlocale = \"tr_TR\"\nlabel = \"Turkish (Turkey)\"\n\
             quality = \"medium\"\nsource = \"voice-me\"\n",
        )
        .unwrap();

        assert!(outdated_custom_voices(root.path(), std::slice::from_ref(&voice)).is_empty());
        let manifest = &assets::installed_piper_voices(root.path())[0].manifest;
        assert!(manifest.model_sha256.is_some() && manifest.config_sha256.is_some());

        std::fs::write(&files.model, [9_u8; 1_000]).unwrap();
        std::fs::write(
            &files.manifest,
            "name = \"erdem\"\nlocale = \"tr_TR\"\nlabel = \"Turkish (Turkey)\"\n\
             quality = \"medium\"\nsource = \"voice-me\"\n",
        )
        .unwrap();
        assert_eq!(
            outdated_custom_voices(root.path(), std::slice::from_ref(&voice)).len(),
            1
        );

        let mut elsewhere = voice.clone();
        elsewhere.entry.source = PiperSource::Official;
        assert!(outdated_custom_voices(root.path(), &[elsewhere]).is_empty());
    }
}
