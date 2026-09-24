//! Story 3.14's rows that live below `TtsPort`, against a loopback mock of
//! Azure and a real `FileSettingsStore` in temporary directories. No real
//! network.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use voice_me_core::{FileSettingsStore, RemoteProvider, SettingsStore, TtsPort, VoiceMeError};

use crate::mock::{MockServer, Recorded, Reply};
use crate::wav::wav_bytes;
use crate::{
    AZURE_OUTPUT_FORMAT, Azure, AzureDeadlines, AzureTtsAdapter, decode_wav, list_azure_voices_with,
};

const KEY: &str = "az-very-secret-key";
const SPEAK: &str = "POST /cognitiveservices/v1";
const LIST: &str = "GET /cognitiveservices/voices/list";

struct Fixture {
    store: Arc<FileSettingsStore>,
    _config: tempfile::TempDir,
    _data: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let config = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = Arc::new(FileSettingsStore::with_dirs(
            config.path().to_path_buf(),
            data.path().to_path_buf(),
        ));
        store
            .save_api_key(RemoteProvider::Azure, Some(KEY))
            .unwrap();
        store.save_azure_region(Some("westeurope")).unwrap();
        Self {
            store,
            _config: config,
            _data: data,
        }
    }

    fn adapter(&self, server: &MockServer) -> AzureTtsAdapter {
        self.adapter_with(server, AzureDeadlines::default())
    }

    fn adapter_with(&self, server: &MockServer, deadlines: AzureDeadlines) -> AzureTtsAdapter {
        AzureTtsAdapter::new(
            Azure::with_base_url(&server.base_url).with_deadlines(deadlines),
            self.store.clone(),
        )
    }
}

/// Half a second of real 24 kHz mono 16-bit RIFF.
fn riff() -> Vec<u8> {
    wav_bytes(24_000, 1, 12_000)
}

fn happy(request: &Recorded) -> Reply {
    match (request.method.as_str(), request.path.as_str()) {
        ("POST", "/cognitiveservices/v1") => Reply::bytes(200, "audio/x-wav", riff()),
        ("GET", "/cognitiveservices/voices/list") => Reply::json(
            200,
            r#"[{"ShortName": "tr-TR-EmelNeural", "LocalName": "Emel", "Gender": "Female",
                 "Locale": "tr-TR", "LocaleName": "Turkish (Türkiye)"},
                {"ShortName": "en-US-JennyNeural", "LocalName": "Jenny", "Gender": "Female",
                 "Locale": "en-US", "LocaleName": "English (United States)"}]"#,
        ),
        _ => Reply::json(404, "{}"),
    }
}

/// A clip path that does not exist: reading it would fail the call.
fn unreadable_clip() -> PathBuf {
    PathBuf::from("/nonexistent/voice-me/reference_voice_sample.wav")
}

/// The matrix's Speak row: one POST, the exact headers and SSML, the 24 kHz
/// reply decoded unchanged, and the sample never read.
#[test]
fn a_line_is_one_post_with_the_exact_headers_and_ssml() {
    let fixture = Fixture::new();
    let server = MockServer::start(happy);

    let audio = fixture
        .adapter(&server)
        .generate(
            "Merhaba dünya",
            Some(&unreadable_clip()),
            "tr-TR",
            Some("tr-TR-EmelNeural"),
        )
        .unwrap();

    assert_eq!(server.calls(), vec![SPEAK]);
    let request = &server.requests()[0];
    assert_eq!(request.header("Ocp-Apim-Subscription-Key"), Some(KEY));
    assert_eq!(request.header("Content-Type"), Some("application/ssml+xml"));
    assert_eq!(
        request.header("X-Microsoft-OutputFormat"),
        Some(AZURE_OUTPUT_FORMAT)
    );
    assert_eq!(AZURE_OUTPUT_FORMAT, "riff-24khz-16bit-mono-pcm");
    assert_eq!(
        request.body_text(),
        "<speak version='1.0' xml:lang='tr-TR'><voice name='tr-TR-EmelNeural'>Merhaba \
         dünya</voice></speak>"
    );
    // AD-11: 24 kHz mono in, the same samples out — nothing resampled.
    assert_eq!(audio, decode_wav(&riff()).unwrap());
    assert_eq!(audio.len(), 12_000);
}

/// The Hostile text row: escaped, one voice element, spoken literally.
#[test]
fn hostile_text_is_escaped_in_the_request() {
    let fixture = Fixture::new();
    let server = MockServer::start(happy);

    fixture
        .adapter(&server)
        .generate(
            "</voice><voice name='x'> & more",
            None,
            "tr-TR",
            Some("tr-TR-EmelNeural"),
        )
        .unwrap();

    let body = server.requests()[0].body_text();
    assert_eq!(body.matches("<voice").count(), 1, "{body}");
    assert!(
        body.contains("&lt;/voice&gt;&lt;voice name=&apos;x&apos;&gt; &amp; more"),
        "{body}"
    );
}

/// The Rejected key row, on speech: one error naming Azure, not re-sent,
/// and the key nowhere in it.
#[test]
fn a_rejected_key_on_speech_names_azure_and_is_not_resent() {
    let fixture = Fixture::new();
    let server = MockServer::start(|_| Reply::json(401, ""));

    let error = fixture
        .adapter(&server)
        .generate("Merhaba", None, "tr-TR", Some("tr-TR-EmelNeural"))
        .unwrap_err();

    assert_eq!(error.to_string(), "Azure rejected the API key.");
    assert!(!format!("{error:?}").contains(KEY));
    assert_eq!(server.calls(), vec![SPEAK], "never re-sent");
}

/// The Rejected key row, on the voice list.
#[test]
fn a_rejected_key_on_the_voice_list_names_azure() {
    let server = MockServer::start(|_| Reply::json(403, r#"{"error": "denied"}"#));

    let error = list_azure_voices_with(&Azure::with_base_url(&server.base_url), KEY, "westeurope")
        .unwrap_err();

    assert_eq!(error.to_string(), "Azure rejected the API key.");
    assert_eq!(server.calls(), vec![LIST]);
}

#[test]
fn the_voice_list_is_fetched_with_the_key_only() {
    let server = MockServer::start(happy);

    let voices =
        list_azure_voices_with(&Azure::with_base_url(&server.base_url), KEY, "westeurope").unwrap();

    assert_eq!(voices.len(), 2);
    assert_eq!(voices[0].id, "tr-TR-EmelNeural");
    assert_eq!(voices[0].name, "Emel (Female)");
    assert_eq!(voices[0].language_label, "Turkish (Türkiye)");
    let request = &server.requests()[0];
    assert_eq!(request.header("Ocp-Apim-Subscription-Key"), Some(KEY));
    assert!(
        request.body.is_empty(),
        "no text leaves before the disclosure"
    );
}

/// A timeout is one error naming Azure and the deadline, not re-sent.
#[test]
fn a_timeout_names_azure_and_the_deadline() {
    let fixture = Fixture::new();
    let server = MockServer::start(|_| {
        Reply::bytes(200, "audio/x-wav", riff()).after(Duration::from_secs(3))
    });
    let deadlines = AzureDeadlines {
        speech: Duration::from_secs(1),
        voices: Duration::from_secs(1),
    };

    let error = fixture
        .adapter_with(&server, deadlines)
        .generate("Merhaba", None, "tr-TR", Some("tr-TR-EmelNeural"))
        .unwrap_err();

    assert_eq!(error.to_string(), "Azure did not answer within 1 second.");
    assert_eq!(server.calls(), vec![SPEAK]);
}

/// A provider error is its shortened detail, or the status.
#[test]
fn a_provider_error_names_azure_and_the_reason() {
    let fixture = Fixture::new();
    let server = MockServer::start(|_| Reply::json(400, ""));

    let error = fixture
        .adapter(&server)
        .generate("Merhaba", None, "tr-TR", Some("tr-TR-GoneNeural"))
        .unwrap_err();

    assert_eq!(error.to_string(), "Azure: HTTP 400 Bad Request");

    // Azure's own JSON error names the reason.
    let server = MockServer::start(|_| {
        Reply::json(
            400,
            r#"{"error":{"code":"InvalidRequest","message":"Unsupported voice."}}"#,
        )
    });
    let error = fixture
        .adapter(&server)
        .generate("Merhaba", None, "tr-TR", Some("tr-TR-GoneNeural"))
        .unwrap_err();
    assert!(error.to_string().contains("Unsupported voice."), "{error}");
}

/// No voice, no region: refused before any request.
#[test]
fn no_voice_or_no_region_is_refused_before_any_request() {
    let fixture = Fixture::new();
    let server = MockServer::start(happy);

    let error = fixture
        .adapter(&server)
        .generate("Merhaba", None, "tr-TR", None)
        .unwrap_err();
    assert!(error.to_string().contains("no voice selected"), "{error}");

    fixture.store.save_azure_region(None).unwrap();
    let error = fixture
        .adapter(&server)
        .generate("Merhaba", None, "tr-TR", Some("tr-TR-EmelNeural"))
        .unwrap_err();
    assert!(error.to_string().contains("Azure has no region"), "{error}");
    assert!(server.calls().is_empty());
}

/// The sample is never read, even when one exists and is handed in.
#[test]
fn an_existing_sample_is_never_sent() {
    let fixture = Fixture::new();
    let clip = fixture
        .store
        .save_reference_voice_sample(&wav_bytes(24_000, 1, 4_800))
        .unwrap()
        .reference_voice_sample
        .unwrap();
    let server = MockServer::start(happy);

    fixture
        .adapter(&server)
        .generate(
            "Merhaba",
            Some(Path::new(&clip)),
            "tr-TR",
            Some("tr-TR-EmelNeural"),
        )
        .unwrap();

    let body = &server.requests()[0].body;
    assert!(!body.windows(4).any(|window| window == b"RIFF"));
    assert!(matches!(
        crate::delete_held_sample(RemoteProvider::Azure, fixture.store.as_ref()),
        Err(VoiceMeError::Other(message)) if message.contains("holds no voice samples")
    ));
}
