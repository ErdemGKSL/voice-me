//! What every remote provider's HTTP calls share (Stories 3.6, 3.14): one
//! client per call, one send that maps every failure, and the reduction of
//! an error body to a short `detail` before it goes anywhere.

use std::time::Duration;

use reqwest::StatusCode;

use crate::provider::ProviderError;

/// The longest `detail` a notification carries.
pub(crate) const MAX_DETAIL_CHARS: usize = 200;

/// Which call an error response answered — it decides what a 404 means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Call {
    Upload,
    /// Names a stored voice; only a detail that says the voice is unknown
    /// means it is gone — a bare 404 may be a retired model path.
    Inference,
    /// Names a stored voice by path, so a 404 means it is gone.
    Delete,
    /// Looks a stored voice up by path; a 404 means it is gone.
    Lookup,
    /// A stock-voice provider's call (Azure, Story 3.14): it names no
    /// uploaded voice, so no answer ever means one is gone.
    Stock,
}

/// A client per call: the call may run on a short-lived runtime, and a
/// pooled connection must not outlive the runtime that drives it.
pub(crate) fn client() -> Result<reqwest::Client, ProviderError> {
    reqwest::Client::builder()
        .build()
        .map_err(|error| ProviderError::Transport(error.without_url().to_string()))
}

/// Send `request` and read the whole body, mapping every failure.
pub(crate) async fn send(
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

/// A transport failure in words, without the URL's query or any body.
pub(crate) fn describe(error: reqwest::Error) -> String {
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
pub(crate) fn classify(status: StatusCode, body: &[u8], call: Call) -> ProviderError {
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
        Call::Delete | Call::Lookup if status == StatusCode::NOT_FOUND || names_missing_voice => {
            return ProviderError::VoiceGone;
        }
        Call::Inference if names_missing_voice => return ProviderError::VoiceGone,
        Call::Upload | Call::Inference | Call::Delete | Call::Lookup | Call::Stock => {}
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
/// validation errors as the first one's `msg` with the field it names
/// ("Field required (files)").
pub(crate) fn detail(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    // Azure (Story 3.14) answers `{"error": {"code": …, "message": …}}`.
    if let Some(message) = value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(serde_json::Value::as_str)
    {
        let text = shorten(message);
        return (!text.is_empty()).then_some(text);
    }
    let detail = value.get("detail")?;
    let text = match detail {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(items) => {
            let first = items.first()?;
            let msg = first.get("msg")?.as_str()?;
            match first
                .get("loc")
                .and_then(serde_json::Value::as_array)
                .and_then(|loc| loc.last())
                .and_then(serde_json::Value::as_str)
                .filter(|field| *field != "body")
            {
                Some(field) => format!("{msg} ({field})"),
                None => msg.to_string(),
            }
        }
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
pub(crate) fn shorten(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= MAX_DETAIL_CHARS {
        return line;
    }
    let cut: String = line.chars().take(MAX_DETAIL_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
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
        let named = br#"{"detail": [{"loc": ["body", "files"], "msg": "Field required"}]}"#;
        assert_eq!(
            classify(StatusCode::UNPROCESSABLE_ENTITY, named, Call::Upload),
            ProviderError::Failed("Field required (files)".to_string())
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
    fn a_stock_voice_call_never_reads_as_a_gone_voice() {
        assert_eq!(
            classify(StatusCode::UNAUTHORIZED, b"", Call::Stock),
            ProviderError::RejectedKey
        );
        assert_eq!(
            classify(
                StatusCode::NOT_FOUND,
                br#"{"detail": "Voice not found"}"#,
                Call::Stock
            ),
            ProviderError::Failed("Voice not found".to_string())
        );
        assert_eq!(
            classify(StatusCode::BAD_REQUEST, b"", Call::Stock),
            ProviderError::Failed("HTTP 400 Bad Request".to_string())
        );
    }
}
