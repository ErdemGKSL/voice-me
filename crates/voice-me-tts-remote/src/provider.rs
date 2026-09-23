//! The one abstraction every remote vendor sits behind (Story 3.6).
//!
//! A [`SpeechProvider`] owns its endpoints, its auth, its request and
//! response shapes and its error mapping. Nothing it defines crosses
//! `TtsPort`: the adapter turns a [`ProviderError`] into
//! `VoiceMeError::Provider` before anything above it sees it.

use std::time::Duration;

use voice_me_core::{RemoteProvider, VoiceMeError};

/// How one remote call went wrong, before it is put into words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// HTTP 401/403: the key was refused.
    RejectedKey,
    /// No answer before the call's deadline.
    Timeout(Duration),
    /// The provider no longer knows the uploaded voice (a 404, or an
    /// "unknown voice" error on inference).
    VoiceGone,
    /// The provider answered with an error; this is its shortened `detail`.
    Failed(String),
    /// The request never got an answer at all: no connection, a broken
    /// response. Already free of the key and of any body.
    Transport(String),
}

impl ProviderError {
    /// The domain error, naming `provider`.
    pub fn into_domain(self, provider: RemoteProvider) -> VoiceMeError {
        let label = provider.label();
        let reason = match self {
            ProviderError::RejectedKey => format!("{label} rejected the API key."),
            ProviderError::Timeout(deadline) => {
                format!("{label} did not answer within {}.", seconds(deadline))
            }
            ProviderError::VoiceGone => format!(
                "{label} no longer has the Reference Voice Sample. It will be uploaded again \
                 with the next line."
            ),
            ProviderError::Failed(detail) => detail,
            ProviderError::Transport(reason) => format!("{label} could not be reached: {reason}"),
        };
        VoiceMeError::Provider {
            provider: label.to_string(),
            reason,
        }
    }
}

/// "120 seconds", "1 second".
fn seconds(duration: Duration) -> String {
    match duration.as_secs() {
        1 => "1 second".to_string(),
        secs => format!("{secs} seconds"),
    }
}

/// One remote vendor. The key is handed in per call rather than held, so a
/// key changed in Settings reaches the very next request.
///
/// Only three things ever leave the machine through it: the typed text, the
/// language tag, and the Reference Voice Sample.
#[allow(async_fn_in_trait)] // Used generically, never as `dyn`.
pub trait SpeechProvider: Send + Sync {
    /// Which provider this is.
    fn provider(&self) -> RemoteProvider;

    /// How the provider is named to the user.
    fn label(&self) -> &'static str {
        self.provider().label()
    }

    /// Upload the Reference Voice Sample (WAV bytes) once; returns the
    /// provider's id for it.
    async fn upload_sample(&self, key: &str, wav: Vec<u8>) -> Result<String, ProviderError>;

    /// Delete a previously uploaded voice. A voice the provider no longer
    /// has is [`ProviderError::VoiceGone`].
    async fn delete_sample(&self, key: &str, voice_id: &str) -> Result<(), ProviderError>;

    /// Generate `text` in `language` in the voice `voice_id`; returns WAV
    /// bytes.
    async fn synthesize(
        &self,
        key: &str,
        text: &str,
        voice_id: &str,
        language: &str,
    ) -> Result<Vec<u8>, ProviderError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_failure_reads_the_way_the_matrix_writes_it() {
        let words = |error: ProviderError| error.into_domain(RemoteProvider::DeepInfra).to_string();

        assert_eq!(
            words(ProviderError::RejectedKey),
            "DeepInfra rejected the API key."
        );
        assert_eq!(
            words(ProviderError::Timeout(Duration::from_secs(120))),
            "DeepInfra did not answer within 120 seconds."
        );
        assert_eq!(
            words(ProviderError::Failed("Model is overloaded".to_string())),
            "DeepInfra: Model is overloaded"
        );
        assert!(words(ProviderError::VoiceGone).contains("uploaded again"));
    }
}
