//! Azure Neural TTS over REST (Story 3.14): a stock Microsoft voice, with
//! the user's own key and region.
//!
//! Two calls, each with its own deadline and neither ever retried:
//!
//! - `GET /cognitiveservices/voices/list` → the voice list, fetched in the
//!   background by the Dependency Check (D1). It carries the key and
//!   nothing else, and is the only Azure call made before the disclosure;
//! - `POST /cognitiveservices/v1` with an SSML body → a
//!   `riff-24khz-16bit-mono-pcm` WAV, decoded as it is (AD-11).
//!
//! The root is `https://{region}.tts.speech.microsoft.com`. The region is
//! checked to be `[a-z0-9]+` before it is put into the host, so it can
//! never name another one. The key travels only as
//! `Ocp-Apim-Subscription-Key`. Azure never receives the Reference Voice
//! Sample.

use std::time::Duration;

use serde::Deserialize;
use voice_me_core::{RemoteProvider, StockVoice, parse_azure_region};

use crate::http::{Call, client, send};
use crate::provider::ProviderError;

/// The header the key travels in.
pub const AZURE_KEY_HEADER: &str = "Ocp-Apim-Subscription-Key";

/// The audio Azure is asked for: AD-11's own format, so nothing is
/// resampled.
pub const AZURE_OUTPUT_FORMAT: &str = "riff-24khz-16bit-mono-pcm";

/// The voice-list path.
pub const AZURE_VOICES_PATH: &str = "/cognitiveservices/voices/list";

/// The speech path.
pub const AZURE_SPEECH_PATH: &str = "/cognitiveservices/v1";

/// How long each Azure call may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AzureDeadlines {
    pub speech: Duration,
    pub voices: Duration,
}

impl Default for AzureDeadlines {
    fn default() -> Self {
        Self {
            speech: Duration::from_secs(30),
            voices: Duration::from_secs(15),
        }
    }
}

/// Azure's REST speech API.
#[derive(Debug, Clone, Default)]
pub struct Azure {
    /// A fixed root instead of the region's own — a loopback mock in tests.
    base_url: Option<String>,
    deadlines: AzureDeadlines,
}

impl Azure {
    /// Against the real API, at the region each call names.
    pub fn new() -> Self {
        Self::default()
    }

    /// Against a fixed root whatever the region — a loopback mock in tests.
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            base_url: Some(base_url.into().trim_end_matches('/').to_string()),
            deadlines: AzureDeadlines::default(),
        }
    }

    /// Replace the deadlines (tests shorten them).
    pub fn with_deadlines(mut self, deadlines: AzureDeadlines) -> Self {
        self.deadlines = deadlines;
        self
    }

    /// The full URL of `path` in `region`. A region that is not already
    /// `[a-z0-9]+` is refused here too, before any host is built from it.
    fn url(&self, region: &str, path: &str) -> Result<String, ProviderError> {
        let checked = parse_azure_region(region)
            .ok()
            .flatten()
            .filter(|checked| checked == region)
            .ok_or_else(|| {
                ProviderError::Failed(format!(
                    "the region {region:?} is not an Azure region — letters and digits only."
                ))
            })?;
        Ok(match &self.base_url {
            Some(root) => format!("{root}{path}"),
            None => format!("https://{checked}.tts.speech.microsoft.com{path}"),
        })
    }

    /// Every voice Azure offers in `region`, as stock voices: the id is the
    /// `ShortName`, the language the `Locale` (labelled with its
    /// `LocaleName`), the name `"{LocalName} ({Gender})"`.
    pub async fn list_voices(
        &self,
        key: &str,
        region: &str,
    ) -> Result<Vec<StockVoice>, ProviderError> {
        let request = client()?
            .get(self.url(region, AZURE_VOICES_PATH)?)
            .header(AZURE_KEY_HEADER, key);
        let body = send(request, self.deadlines.voices, Call::Stock).await?;
        parse_voice_list(&body)
    }

    /// Speak `text` in `voice` (a `ShortName`) and `locale`; returns the
    /// WAV bytes Azure answered with.
    pub async fn synthesize(
        &self,
        key: &str,
        region: &str,
        locale: &str,
        voice: &str,
        text: &str,
    ) -> Result<Vec<u8>, ProviderError> {
        let request = client()?
            .post(self.url(region, AZURE_SPEECH_PATH)?)
            .header(AZURE_KEY_HEADER, key)
            .header(reqwest::header::CONTENT_TYPE, "application/ssml+xml")
            .header("X-Microsoft-OutputFormat", AZURE_OUTPUT_FORMAT)
            .header(reqwest::header::USER_AGENT, "voice-me")
            .body(ssml(locale, voice, text));
        send(request, self.deadlines.speech, Call::Stock).await
    }

    /// Which provider this is.
    pub fn provider(&self) -> RemoteProvider {
        RemoteProvider::Azure
    }
}

/// The SSML one line is sent as. Every value is XML-escaped, so typed text
/// such as `</voice><voice name='x'>` or `&` is spoken literally and can
/// never change the voice.
pub fn ssml(locale: &str, voice: &str, text: &str) -> String {
    format!(
        "<speak version='1.0' xml:lang='{}'><voice name='{}'>{}</voice></speak>",
        xml_escape(locale),
        xml_escape(voice),
        xml_escape(text)
    )
}

/// `text` with the five XML specials escaped.
fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// One entry of Azure's voice list; only the fields used here.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct VoiceEntry {
    short_name: Option<String>,
    locale: Option<String>,
    locale_name: Option<String>,
    local_name: Option<String>,
    display_name: Option<String>,
    gender: Option<String>,
}

/// Azure's voice list as stock voices. An entry with no `ShortName` or
/// `Locale` cannot be spoken with, and drops out.
fn parse_voice_list(body: &[u8]) -> Result<Vec<StockVoice>, ProviderError> {
    let entries: Vec<serde_json::Value> = serde_json::from_slice(body)
        .map_err(|_| ProviderError::Failed("the voice list was not a JSON list.".to_string()))?;
    Ok(entries
        .into_iter()
        .filter_map(|value| serde_json::from_value::<VoiceEntry>(value).ok())
        .filter_map(|entry| {
            let id = entry.short_name.filter(|id| !id.trim().is_empty())?;
            let locale = entry.locale.filter(|locale| !locale.trim().is_empty())?;
            let local = entry
                .local_name
                .or(entry.display_name)
                .unwrap_or_else(|| id.clone());
            let name = match entry.gender.filter(|gender| !gender.is_empty()) {
                Some(gender) => format!("{local} ({gender})"),
                None => local,
            };
            Some(StockVoice {
                language_label: entry.locale_name.unwrap_or_else(|| locale.clone()),
                id,
                language: locale,
                name,
                priority: 0,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ssml_is_exactly_the_specified_shape() {
        assert_eq!(
            ssml("tr-TR", "tr-TR-EmelNeural", "Merhaba dünya"),
            "<speak version='1.0' xml:lang='tr-TR'><voice name='tr-TR-EmelNeural'>Merhaba \
             dünya</voice></speak>"
        );
    }

    #[test]
    fn hostile_text_is_escaped_and_spoken_literally() {
        let body = ssml(
            "tr-TR",
            "tr-TR-EmelNeural",
            "</voice><voice name='x'> & <b>\"",
        );
        assert_eq!(
            body,
            "<speak version='1.0' xml:lang='tr-TR'><voice name='tr-TR-EmelNeural'>&lt;/voice&gt;\
             &lt;voice name=&apos;x&apos;&gt; &amp; &lt;b&gt;&quot;</voice></speak>"
        );
        assert_eq!(body.matches("<voice").count(), 1);
    }

    #[test]
    fn the_voice_list_parses_into_stock_voices() {
        let body = r#"[
            {"Name": "Microsoft Server Speech Text to Speech Voice (tr-TR, EmelNeural)",
             "DisplayName": "Emel", "LocalName": "Emel", "ShortName": "tr-TR-EmelNeural",
             "Gender": "Female", "Locale": "tr-TR", "LocaleName": "Turkish (Türkiye)",
             "SampleRateHertz": "24000", "VoiceType": "Neural", "Status": "GA"},
            {"DisplayName": "Jenny", "LocalName": "Jenny", "ShortName": "en-US-JennyNeural",
             "Gender": "Female", "Locale": "en-US", "LocaleName": "English (United States)"},
            {"DisplayName": "Broken", "Locale": "en-US"},
            42
        ]"#;

        let voices = parse_voice_list(body.as_bytes()).unwrap();

        assert_eq!(voices.len(), 2);
        assert_eq!(
            voices[0],
            StockVoice {
                id: "tr-TR-EmelNeural".to_string(),
                language: "tr-TR".to_string(),
                language_label: "Turkish (Türkiye)".to_string(),
                name: "Emel (Female)".to_string(),
                priority: 0,
            }
        );
        assert!(parse_voice_list(b"{}").is_err());
    }

    #[test]
    fn a_region_that_could_change_the_host_is_refused_before_any_url() {
        let azure = Azure::new();
        assert_eq!(
            azure.url("westeurope", AZURE_SPEECH_PATH).unwrap(),
            "https://westeurope.tts.speech.microsoft.com/cognitiveservices/v1"
        );
        for bad in ["evil.com/x", "west europe", "WestEurope", ""] {
            assert!(azure.url(bad, AZURE_SPEECH_PATH).is_err(), "{bad}");
        }
    }
}
