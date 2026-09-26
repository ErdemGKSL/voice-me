//! `voice-me-tts-remote` — `TtsPort` over a remote speech API, with the
//! user's own key (Story 3.6).
//!
//! The Speak Action path does not know this crate exists: the composition
//! root puts a [`RemoteTtsAdapter`] in the engine slot when a remote
//! provider is selected, and `speak` calls `generate` exactly as it calls
//! the ONNX engine. Everything vendor-specific sits behind
//! [`SpeechProvider`]; everything above `TtsPort` sees only
//! `RemoteProvider`, `VoiceMeError` and plain strings.
//!
//! The disclosure is *not* checked here. `voice-me-core`'s `speak` refuses
//! an unconfirmed provider before `generate` is reached, and this adapter is
//! deliberately not trusted to.
//!
//! The Reference Voice Sample is uploaded once per provider and then
//! referenced by id. The id is stored through `SettingsStore`, keyed by the
//! sample file's SHA-256, so a re-recorded sample is uploaded afresh (and
//! the stale voice deleted, best effort).
//!
//! Azure (Story 3.14) is a stock-voice provider and sits beside that
//! abstraction rather than behind it: [`AzureTtsAdapter`] never touches the
//! Reference Voice Sample, and [`list_azure_voices`] is the one call made
//! before the disclosure — the key only, no text.
//!
//! AD-8: this and `voice-me-deps` are the only crates that open sockets.

mod azure;
mod deepinfra;
mod http;
mod provider;
mod wav;

use std::path::Path;
use std::sync::{Arc, Mutex};

use sha2::{Digest as _, Sha256};
use voice_me_core::{
    AudioBuffer, RemoteProvider, RemoteSample, SettingsStore, StockVoice, TtsPort, VoiceMeError,
};

pub use azure::{
    AZURE_KEY_HEADER, AZURE_OUTPUT_FORMAT, AZURE_SPEECH_PATH, AZURE_VOICES_PATH, Azure,
    AzureDeadlines, ssml,
};
pub use deepinfra::{
    DEEPINFRA_BASE_URL, DEEPINFRA_MODEL, Deadlines, DeepInfra, UPLOAD_DESCRIPTION, UPLOAD_NAME,
};
pub use provider::{ProviderError, SpeechProvider};
pub use wav::decode_wav;

/// The settings handle the adapter keeps its voice-id cache in. `Send +
/// Sync` because generation runs on Tokio's blocking pool (AD-5).
pub type SharedSettingsStore = Arc<dyn SettingsStore + Send + Sync>;

/// Serializes every call that reads or changes a held sample — generation,
/// and Settings' delete — so an upload and a delete never cross, and one
/// line is generated at a time (AD-10).
static SAMPLE_LOCK: Mutex<()> = Mutex::new(());

fn lock_samples() -> std::sync::MutexGuard<'static, ()> {
    SAMPLE_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

/// `TtsPort` over one [`SpeechProvider`].
pub struct RemoteTtsAdapter<P: SpeechProvider> {
    provider: P,
    store: SharedSettingsStore,
}

impl<P: SpeechProvider> RemoteTtsAdapter<P> {
    pub fn new(provider: P, store: SharedSettingsStore) -> Self {
        Self { provider, store }
    }

    fn label(&self) -> &'static str {
        self.provider.label()
    }

    fn domain(&self, error: ProviderError) -> VoiceMeError {
        error.into_domain(self.provider.provider())
    }

    /// The held voice is gone: forget its id so the next line uploads
    /// again — never re-sent automatically.
    fn voice_gone(&self, provider: RemoteProvider) -> VoiceMeError {
        if let Err(error) = self.store.save_remote_sample(provider, None) {
            eprintln!("could not forget the missing voice id: {error}");
        }
        self.domain(ProviderError::VoiceGone)
    }

    fn generate_locked(
        &self,
        text: &str,
        reference_clip: &Path,
        language: &str,
    ) -> Result<AudioBuffer, VoiceMeError> {
        if text.trim().is_empty() {
            return Err(VoiceMeError::EmptyText);
        }
        let provider = self.provider.provider();
        let key = api_key(self.store.as_ref(), provider)?;
        let wav = std::fs::read(reference_clip)?;
        let sha256 = sha256_hex(&wav);

        block_on(async {
            let voice_id = match self.store.load_remote_sample(provider)? {
                Some(held) if held.sample_sha256 == sha256 => {
                    // An unknown voice id does not fail inference — the
                    // provider speaks in a stock voice instead — so the
                    // held voice is confirmed before every line.
                    match self.provider.confirm_sample(&key, &held.voice_id).await {
                        Ok(()) => held.voice_id,
                        Err(ProviderError::VoiceGone) => return Err(self.voice_gone(provider)),
                        Err(error) => return Err(self.domain(error)),
                    }
                }
                stale => {
                    // Re-recorded: the old voice is removed first, best
                    // effort. A failed delete is logged and forgotten — the
                    // stale id is dropped either way, and the new sample
                    // still goes up.
                    if let Some(stale) = stale {
                        if let Err(error) = self.provider.delete_sample(&key, &stale.voice_id).await
                        {
                            eprintln!(
                                "could not delete the previous voice sample from {}: {}",
                                self.label(),
                                self.domain(error)
                            );
                        }
                        self.store.save_remote_sample(provider, None)?;
                    }
                    let voice_id = self
                        .provider
                        .upload_sample(&key, wav)
                        .await
                        .map_err(|error| self.domain(error))?;
                    self.store.save_remote_sample(
                        provider,
                        Some(RemoteSample {
                            provider,
                            sample_sha256: sha256,
                            voice_id: voice_id.clone(),
                        }),
                    )?;
                    voice_id
                }
            };

            match self
                .provider
                .synthesize(&key, text, &voice_id, language)
                .await
            {
                // A reply that cannot be played is the provider's failure,
                // and the notification has to name it.
                Ok(bytes) => decode_wav(&bytes).map_err(|error| VoiceMeError::Provider {
                    provider: self.label().to_string(),
                    reason: format!(
                        "{} returned audio voice-me could not read: {error}",
                        self.label()
                    ),
                }),
                Err(ProviderError::VoiceGone) => Err(self.voice_gone(provider)),
                Err(error) => Err(self.domain(error)),
            }
        })
    }
}

impl<P: SpeechProvider> TtsPort for RemoteTtsAdapter<P> {
    /// Nothing to build: a remote provider has no sessions.
    fn warm_up(&self) -> Result<(), VoiceMeError> {
        Ok(())
    }

    /// Always ready — there is no engine start to warn about.
    fn is_ready(&self) -> bool {
        true
    }

    fn generate(
        &self,
        text: &str,
        reference_clip: Option<&Path>,
        language: &str,
        _voice: Option<&str>,
    ) -> Result<AudioBuffer, VoiceMeError> {
        // A cloning provider: with no sample there is nothing to upload.
        let reference_clip = reference_clip.ok_or(VoiceMeError::NoReferenceVoiceSample)?;
        let _held = lock_samples();
        self.generate_locked(text, reference_clip, language)
    }
}

/// Delete the Reference Voice Sample `provider` holds, and forget its id
/// (Settings' **Delete from DeepInfra**). Nothing held is nothing to do.
///
/// On failure the id is kept, so the sample is still shown as held and the
/// delete can be tried again. A voice the provider no longer has counts as
/// deleted.
pub fn delete_held_sample(
    provider: RemoteProvider,
    store: &dyn SettingsStore,
) -> Result<(), VoiceMeError> {
    match provider {
        RemoteProvider::DeepInfra => delete_held_sample_with(&DeepInfra::new(), store),
        RemoteProvider::FalAi => Err(VoiceMeError::Other(format!(
            "{} does not hold voice samples in this voice-me release.",
            provider.label()
        ))),
        RemoteProvider::Azure | RemoteProvider::EdgeTts => Err(VoiceMeError::Other(format!(
            "{} holds no voice samples — it speaks in a stock Microsoft voice.",
            provider.label()
        ))),
    }
}

/// `TtsPort` over Azure Neural TTS (Story 3.14): a stock Microsoft voice.
///
/// The key and region are read from the store at every `generate`, so a
/// change in Settings reaches the very next line. `reference_clip` is never
/// read — Azure never receives the Reference Voice Sample.
pub struct AzureTtsAdapter {
    azure: Azure,
    store: SharedSettingsStore,
}

impl AzureTtsAdapter {
    pub fn new(azure: Azure, store: SharedSettingsStore) -> Self {
        Self { azure, store }
    }
}

impl TtsPort for AzureTtsAdapter {
    /// Nothing to build: a remote provider has no sessions.
    fn warm_up(&self) -> Result<(), VoiceMeError> {
        Ok(())
    }

    /// Always ready — there is no engine start to warn about.
    fn is_ready(&self) -> bool {
        true
    }

    fn generate(
        &self,
        text: &str,
        _reference_clip: Option<&Path>,
        language: &str,
        voice: Option<&str>,
    ) -> Result<AudioBuffer, VoiceMeError> {
        let provider = RemoteProvider::Azure;
        let label = provider.label();
        if text.trim().is_empty() {
            return Err(VoiceMeError::EmptyText);
        }
        let refuse = |reason: &str| VoiceMeError::Provider {
            provider: label.to_string(),
            reason: reason.to_string(),
        };
        let Some(voice) = voice.filter(|voice| !voice.trim().is_empty()) else {
            return Err(refuse(
                "Azure has no voice selected — pick one in Settings → Speech.",
            ));
        };
        let key = api_key(self.store.as_ref(), provider)?;
        let Some(region) = self.store.load()?.azure_region else {
            return Err(refuse(
                "Azure has no region — add one in Settings → Speech.",
            ));
        };

        // One line at a time (AD-10), like every remote provider.
        let _held = lock_samples();
        let bytes = block_on(async {
            self.azure
                .synthesize(&key, &region, language, voice, text)
                .await
                .map_err(|error| error.into_domain(provider))
        })?;
        decode_wav(&bytes).map_err(|error| VoiceMeError::Provider {
            provider: label.to_string(),
            reason: format!("{label} returned audio voice-me could not read: {error}"),
        })
    }
}

/// Azure's voice list for `region`, with `key` (Story 3.14, D1): the only
/// Azure call made before the disclosure — the key, and no text. Blocking;
/// the caller runs it off the main thread. A failure names Azure.
pub fn list_azure_voices(key: &str, region: &str) -> Result<Vec<StockVoice>, VoiceMeError> {
    list_azure_voices_with(&Azure::new(), key, region)
}

/// [`list_azure_voices`] against a given [`Azure`] (a mock, in tests).
pub fn list_azure_voices_with(
    azure: &Azure,
    key: &str,
    region: &str,
) -> Result<Vec<StockVoice>, VoiceMeError> {
    block_on(async {
        azure
            .list_voices(key, region)
            .await
            .map_err(|error| error.into_domain(RemoteProvider::Azure))
    })
}

/// [`delete_held_sample`] against a given provider (a mock, in tests).
pub fn delete_held_sample_with<P: SpeechProvider>(
    speech: &P,
    store: &dyn SettingsStore,
) -> Result<(), VoiceMeError> {
    let _held = lock_samples();
    let provider = speech.provider();
    let Some(held) = store.load_remote_sample(provider)? else {
        return Ok(());
    };
    let key = api_key(store, provider)?;
    block_on(async {
        match speech.delete_sample(&key, &held.voice_id).await {
            Ok(()) | Err(ProviderError::VoiceGone) => Ok(()),
            Err(error) => Err(error.into_domain(provider)),
        }
    })?;
    store.save_remote_sample(provider, None)?;
    Ok(())
}

/// The saved key for `provider`, or a sentence saying there is none.
fn api_key(store: &dyn SettingsStore, provider: RemoteProvider) -> Result<String, VoiceMeError> {
    store
        .load()?
        .api_keys
        .get(provider)
        .map(str::to_string)
        .ok_or_else(|| VoiceMeError::Provider {
            provider: provider.label().to_string(),
            reason: format!(
                "{} has no API key — add one under Settings → Speech.",
                provider.label()
            ),
        })
}

/// Lowercase hex SHA-256 of `bytes`.
fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Drive `future` to completion from this blocking thread — the same
/// pattern as `voice-me-deps`.
///
/// In the app this runs on Tokio's blocking pool (the AD-5 bridge), so the
/// multi-threaded runtime it belongs to is reused. Anywhere else (tests) a
/// small runtime is built for the one call.
fn block_on<T>(future: impl Future<Output = Result<T, VoiceMeError>>) -> Result<T, VoiceMeError> {
    use tokio::runtime::{Builder, Handle, RuntimeFlavor};

    match Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == RuntimeFlavor::MultiThread => {
            handle.block_on(future)
        }
        _ => Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(future),
    }
}

#[cfg(test)]
mod azure_tests;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;
