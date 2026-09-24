//! DeepInfra's `ResembleAI/chatterbox-multilingual` (Story 3.6).
//!
//! Three calls, each with its own deadline and none ever retried:
//!
//! - `POST /v1/voices/add` (multipart `audio`, `name`, `description`) →
//!   `{voice_id}` — the one-time sample upload;
//! - `POST /v1/inference/ResembleAI/chatterbox-multilingual` with
//!   `{text, voice_id, language_id, response_format: "wav"}` → `{audio}`,
//!   a base64 WAV (a `data:` URL, or bare base64);
//! - `DELETE /v1/voices/{id}`.
//!
//! The key travels only as `Authorization: Bearer <key>`. Error bodies are
//! reduced to their `detail` message before they go anywhere.

use std::time::Duration;

use base64::Engine as _;
use voice_me_core::RemoteProvider;

use crate::http::{Call, client, send};
use crate::provider::{ProviderError, SpeechProvider};

/// DeepInfra's API root.
pub const DEEPINFRA_BASE_URL: &str = "https://api.deepinfra.com";

/// The model every line is generated with.
pub const DEEPINFRA_MODEL: &str = "ResembleAI/chatterbox-multilingual";

/// The fixed strings the upload carries — nothing about the machine or the
/// user.
pub const UPLOAD_NAME: &str = "voice-me reference sample";
pub const UPLOAD_DESCRIPTION: &str = "Uploaded by voice-me";

/// How long each call may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadlines {
    pub inference: Duration,
    pub upload: Duration,
    pub delete: Duration,
}

impl Default for Deadlines {
    fn default() -> Self {
        Self {
            inference: Duration::from_secs(120),
            upload: Duration::from_secs(60),
            delete: Duration::from_secs(30),
        }
    }
}

/// DeepInfra as a [`SpeechProvider`].
#[derive(Debug, Clone)]
pub struct DeepInfra {
    base_url: String,
    deadlines: Deadlines,
}

impl Default for DeepInfra {
    fn default() -> Self {
        Self::new()
    }
}

impl DeepInfra {
    /// Against the real API.
    pub fn new() -> Self {
        Self::with_base_url(DEEPINFRA_BASE_URL)
    }

    /// Against another root — a loopback mock in tests.
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            deadlines: Deadlines::default(),
        }
    }

    /// Replace the deadlines (tests shorten them).
    pub fn with_deadlines(mut self, deadlines: Deadlines) -> Self {
        self.deadlines = deadlines;
        self
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }
}

/// The WAV inside an inference response: `{audio: "data:audio/wav;base64,…"}`
/// or bare base64.
fn audio_from_response(body: &[u8]) -> Result<Vec<u8>, ProviderError> {
    let value: serde_json::Value = serde_json::from_slice(body)
        .map_err(|_| ProviderError::Failed("the response was not JSON.".to_string()))?;
    let audio = value
        .get("audio")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ProviderError::Failed("the response carried no audio.".to_string()))?;
    let encoded = match audio.split_once(";base64,") {
        Some((prefix, data)) if prefix.starts_with("data:") => data,
        _ => audio,
    };
    base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|_| ProviderError::Failed("the audio in the response was not base64.".to_string()))
}

impl SpeechProvider for DeepInfra {
    fn provider(&self) -> RemoteProvider {
        RemoteProvider::DeepInfra
    }

    async fn upload_sample(&self, key: &str, wav: Vec<u8>) -> Result<String, ProviderError> {
        let part = reqwest::multipart::Part::bytes(wav)
            .file_name("reference_voice_sample.wav")
            .mime_str("audio/wav")
            .map_err(|error| ProviderError::Transport(error.without_url().to_string()))?;
        let form = reqwest::multipart::Form::new()
            // `files`, as DeepInfra's ElevenLabs-compatible clone endpoint
            // names it; its curl sample says `audio`, which the live API
            // rejects with "Field required".
            .part("files", part)
            .text("name", UPLOAD_NAME)
            .text("description", UPLOAD_DESCRIPTION);
        let request = client()?
            .post(self.url("/v1/voices/add"))
            .bearer_auth(key)
            .multipart(form);
        let body = send(request, self.deadlines.upload, Call::Upload).await?;
        let value: serde_json::Value = serde_json::from_slice(&body)
            .map_err(|_| ProviderError::Failed("the upload response was not JSON.".to_string()))?;
        value
            .get("voice_id")
            .or_else(|| value.get("id"))
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                ProviderError::Failed("the upload response carried no voice id.".to_string())
            })
    }

    /// `GET /v1/voices/{id}`, under the delete deadline. Needed because
    /// inference with an unknown `voice_id` does not fail: DeepInfra
    /// answers in a stock voice (seen against the live API, 2026-09-23).
    async fn confirm_sample(&self, key: &str, voice_id: &str) -> Result<(), ProviderError> {
        let request = client()?
            .get(self.url(&format!("/v1/voices/{voice_id}")))
            .bearer_auth(key);
        send(request, self.deadlines.delete, Call::Lookup)
            .await
            .map(|_| ())
    }

    async fn delete_sample(&self, key: &str, voice_id: &str) -> Result<(), ProviderError> {
        let request = client()?
            .delete(self.url(&format!("/v1/voices/{voice_id}")))
            .bearer_auth(key);
        send(request, self.deadlines.delete, Call::Delete)
            .await
            .map(|_| ())
    }

    async fn synthesize(
        &self,
        key: &str,
        text: &str,
        voice_id: &str,
        language: &str,
    ) -> Result<Vec<u8>, ProviderError> {
        let request = client()?
            .post(self.url(&format!("/v1/inference/{DEEPINFRA_MODEL}")))
            .bearer_auth(key)
            .json(&serde_json::json!({
                "text": text,
                "voice_id": voice_id,
                "language_id": language,
                "response_format": "wav",
            }));
        let body = send(request, self.deadlines.inference, Call::Inference).await?;
        audio_from_response(&body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_is_read_from_a_data_url_or_bare_base64() {
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"RIFF");
        let data_url = format!(r#"{{"audio": "data:audio/wav;base64,{encoded}"}}"#);
        assert_eq!(audio_from_response(data_url.as_bytes()).unwrap(), b"RIFF");
        let bare = format!(r#"{{"audio": "{encoded}"}}"#);
        assert_eq!(audio_from_response(bare.as_bytes()).unwrap(), b"RIFF");
        assert!(audio_from_response(br#"{"audio": null}"#).is_err());
    }
}
