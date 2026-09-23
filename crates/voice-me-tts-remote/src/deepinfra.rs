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
use reqwest::StatusCode;
use voice_me_core::RemoteProvider;

use crate::provider::{ProviderError, SpeechProvider};

/// DeepInfra's API root.
pub const DEEPINFRA_BASE_URL: &str = "https://api.deepinfra.com";

/// The model every line is generated with.
pub const DEEPINFRA_MODEL: &str = "ResembleAI/chatterbox-multilingual";

/// The fixed strings the upload carries — nothing about the machine or the
/// user.
pub const UPLOAD_NAME: &str = "voice-me reference sample";
pub const UPLOAD_DESCRIPTION: &str = "Uploaded by voice-me";

/// The longest `detail` a notification carries.
const MAX_DETAIL_CHARS: usize = 200;

/// Which call an error response answered — it decides what a 404 means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    Upload,
    /// Names a stored voice; only a detail that says the voice is unknown
    /// means it is gone — a bare 404 may be a retired model path.
    Inference,
    /// Names a stored voice by path, so a 404 means it is gone.
    Delete,
}

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

    /// A client per call: the call may run on a short-lived runtime, and a
    /// pooled connection must not outlive the runtime that drives it.
    fn client() -> Result<reqwest::Client, ProviderError> {
        reqwest::Client::builder()
            .build()
            .map_err(|error| ProviderError::Transport(error.without_url().to_string()))
    }

    /// Send `request` and read the whole body, mapping every failure.
    async fn send(
        request: reqwest::RequestBuilder,
        deadline: Duration,
        call: Call,
    ) -> Result<Vec<u8>, ProviderError> {
        let transport = |error: reqwest::Error| {
            if error.is_timeout() {
                ProviderError::Timeout(deadline)
            } else {
                ProviderError::Transport(describe(error))
            }
        };
        let response = request.timeout(deadline).send().await.map_err(transport)?;
        let status = response.status();
        let body = response.bytes().await.map_err(transport)?;
        if status.is_success() {
            return Ok(body.to_vec());
        }
        Err(classify(status, &body, call))
    }
}

/// A transport failure in words, without the URL's query or any body.
fn describe(error: reqwest::Error) -> String {
    if error.is_connect() {
        "no connection could be made.".to_string()
    } else {
        let error = error.without_url();
        let mut text = error.to_string();
        let mut source = std::error::Error::source(&error);
        while let Some(inner) = source {
            text = format!("{text}: {inner}");
            source = inner.source();
        }
        shorten(&text)
    }
}

/// Map an error response for `call`. On a delete a 404 means the voice is
/// gone; on inference only a detail that says so does.
fn classify(status: StatusCode, body: &[u8], call: Call) -> ProviderError {
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return ProviderError::RejectedKey;
    }
    let detail = detail(body);
    let names_missing_voice = detail.as_deref().is_some_and(|detail| {
        let lower = detail.to_lowercase();
        lower.contains("voice")
            && ["not found", "unknown", "does not exist"]
                .iter()
                .any(|phrase| lower.contains(phrase))
    });
    match call {
        Call::Delete if status == StatusCode::NOT_FOUND || names_missing_voice => {
            return ProviderError::VoiceGone;
        }
        Call::Inference if names_missing_voice => return ProviderError::VoiceGone,
        _ => {}
    }
    ProviderError::Failed(detail.unwrap_or_else(|| {
        format!(
            "HTTP {} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("error")
        )
    }))
}

/// The `detail` of an error body, shortened: a string as it is, a list of
/// validation errors as the first one's `msg`.
fn detail(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let detail = value.get("detail")?;
    let text = match detail {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(items) => items.first()?.get("msg")?.as_str()?.to_string(),
        serde_json::Value::Object(object) => object
            .get("error")
            .or_else(|| object.get("message"))?
            .as_str()?
            .to_string(),
        _ => return None,
    };
    let text = shorten(&text);
    (!text.is_empty()).then_some(text)
}

/// One line, at most [`MAX_DETAIL_CHARS`] characters.
fn shorten(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= MAX_DETAIL_CHARS {
        return line;
    }
    let cut: String = line.chars().take(MAX_DETAIL_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
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
            .part("audio", part)
            .text("name", UPLOAD_NAME)
            .text("description", UPLOAD_DESCRIPTION);
        let request = Self::client()?
            .post(self.url("/v1/voices/add"))
            .bearer_auth(key)
            .multipart(form);
        let body = Self::send(request, self.deadlines.upload, Call::Upload).await?;
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

    async fn delete_sample(&self, key: &str, voice_id: &str) -> Result<(), ProviderError> {
        let request = Self::client()?
            .delete(self.url(&format!("/v1/voices/{voice_id}")))
            .bearer_auth(key);
        Self::send(request, self.deadlines.delete, Call::Delete)
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
        let request = Self::client()?
            .post(self.url(&format!("/v1/inference/{DEEPINFRA_MODEL}")))
            .bearer_auth(key)
            .json(&serde_json::json!({
                "text": text,
                "voice_id": voice_id,
                "language_id": language,
                "response_format": "wav",
            }));
        let body = Self::send(request, self.deadlines.inference, Call::Inference).await?;
        audio_from_response(&body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rejected_key_is_named_whatever_the_body() {
        assert_eq!(
            classify(StatusCode::UNAUTHORIZED, b"{}", Call::Upload),
            ProviderError::RejectedKey
        );
        assert_eq!(
            classify(StatusCode::FORBIDDEN, b"<html>", Call::Inference),
            ProviderError::RejectedKey
        );
    }

    #[test]
    fn the_detail_is_all_that_is_kept_of_an_error_body() {
        let body = br#"{"detail": "Model is\n overloaded", "trace": "internal"}"#;
        assert_eq!(
            classify(StatusCode::SERVICE_UNAVAILABLE, body, Call::Inference),
            ProviderError::Failed("Model is overloaded".to_string())
        );
        let long = format!(r#"{{"detail": "{}"}}"#, "x".repeat(500));
        let ProviderError::Failed(short) =
            classify(StatusCode::BAD_REQUEST, long.as_bytes(), Call::Upload)
        else {
            panic!()
        };
        assert_eq!(short.chars().count(), MAX_DETAIL_CHARS);
    }

    #[test]
    fn a_validation_list_keeps_its_first_message() {
        let body = br#"{"detail": [{"loc": ["body"], "msg": "field required"}]}"#;
        assert_eq!(
            classify(StatusCode::UNPROCESSABLE_ENTITY, body, Call::Upload),
            ProviderError::Failed("field required".to_string())
        );
    }

    #[test]
    fn a_body_with_no_detail_falls_back_to_the_status() {
        assert_eq!(
            classify(StatusCode::INTERNAL_SERVER_ERROR, b"oops", Call::Inference),
            ProviderError::Failed("HTTP 500 Internal Server Error".to_string())
        );
    }

    #[test]
    fn a_missing_voice_is_recognised_per_call() {
        // A delete names the voice in its path: a 404 means it is gone.
        assert_eq!(
            classify(StatusCode::NOT_FOUND, b"{}", Call::Delete),
            ProviderError::VoiceGone
        );
        // Inference: only a detail that says the voice is unknown.
        assert_eq!(
            classify(
                StatusCode::BAD_REQUEST,
                br#"{"detail": "Voice not found"}"#,
                Call::Inference
            ),
            ProviderError::VoiceGone
        );
        assert_eq!(
            classify(StatusCode::NOT_FOUND, b"{}", Call::Inference),
            ProviderError::Failed("HTTP 404 Not Found".to_string()),
            "a bare 404 may be a retired model path"
        );
        assert!(matches!(
            classify(
                StatusCode::BAD_REQUEST,
                br#"{"detail": "invalid voice settings"}"#,
                Call::Inference
            ),
            ProviderError::Failed(_)
        ));
        assert!(matches!(
            classify(StatusCode::NOT_FOUND, b"{}", Call::Upload),
            ProviderError::Failed(_)
        ));
    }

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
