//! Every row of Story 3.6's I/O matrix that lives below `TtsPort`, against
//! a loopback mock of DeepInfra and a real `FileSettingsStore` in temporary
//! directories. No real network.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use voice_me_core::{
    FileSettingsStore, RemoteProvider, RemoteSample, SAMPLE_RATE, SettingsStore, TtsPort,
    VoiceMeError,
};

use crate::mock::{MockServer, Recorded, Reply};
use crate::wav::wav_bytes;
use crate::{Deadlines, DeepInfra, RemoteTtsAdapter, delete_held_sample_with, sha256_hex};

const KEY: &str = "sk-di-very-secret";
const UPLOAD: &str = "POST /v1/voices/add";
const INFER: &str = "POST /v1/inference/ResembleAI/chatterbox-multilingual";
/// The check before every line that the held voice still exists.
const LOOKUP: &str = "GET /v1/voices/v-held";

struct Fixture {
    store: Arc<FileSettingsStore>,
    clip: PathBuf,
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
            .save_api_key(RemoteProvider::DeepInfra, Some(KEY))
            .unwrap();
        let clip = store
            .save_reference_voice_sample(&wav_bytes(24_000, 1, 4_800))
            .unwrap()
            .reference_voice_sample
            .unwrap();
        Self {
            store,
            clip,
            _config: config,
            _data: data,
        }
    }

    fn sample_hash(&self) -> String {
        sha256_hex(&std::fs::read(&self.clip).unwrap())
    }

    fn hold(&self, sha256: &str, voice_id: &str) {
        self.store
            .save_remote_sample(
                RemoteProvider::DeepInfra,
                Some(RemoteSample {
                    provider: RemoteProvider::DeepInfra,
                    sample_sha256: sha256.to_string(),
                    voice_id: voice_id.to_string(),
                }),
            )
            .unwrap();
    }

    fn held(&self) -> Option<RemoteSample> {
        self.store
            .load_remote_sample(RemoteProvider::DeepInfra)
            .unwrap()
    }

    fn adapter(&self, server: &MockServer) -> RemoteTtsAdapter<DeepInfra> {
        self.adapter_with(server, Deadlines::default())
    }

    fn adapter_with(
        &self,
        server: &MockServer,
        deadlines: Deadlines,
    ) -> RemoteTtsAdapter<DeepInfra> {
        RemoteTtsAdapter::new(
            DeepInfra::with_base_url(&server.base_url).with_deadlines(deadlines),
            self.store.clone(),
        )
    }

    fn speak(&self, adapter: &RemoteTtsAdapter<DeepInfra>) -> Result<Vec<f32>, VoiceMeError> {
        adapter
            .generate("Merhaba dünya", &self.clip, "tr")
            .map(|audio| audio.into_samples())
    }
}

fn audio_reply(rate: u32) -> Reply {
    let encoded =
        base64::engine::general_purpose::STANDARD.encode(wav_bytes(rate, 1, rate as usize / 2));
    Reply::json(
        200,
        format!(r#"{{"audio": "data:audio/wav;base64,{encoded}"}}"#),
    )
}

/// The happy provider: uploads return `v-new`, inference returns half a
/// second of 24 kHz audio, deletes succeed.
fn happy(request: &Recorded) -> Reply {
    match (request.method.as_str(), request.path.as_str()) {
        ("POST", "/v1/voices/add") => Reply::json(200, r#"{"voice_id": "v-new"}"#),
        ("POST", _) => audio_reply(SAMPLE_RATE),
        ("DELETE", _) => Reply::json(200, "{}"),
        ("GET", _) => Reply::json(200, r#"{"voice_id": "v-held"}"#),
        _ => Reply::json(404, "{}"),
    }
}

fn assert_key_absent(error: &VoiceMeError) {
    assert!(!error.to_string().contains(KEY), "{error}");
    assert!(!format!("{error:?}").contains(KEY), "{error:?}");
}

#[test]
fn the_first_line_uploads_stores_the_id_then_generates() {
    let fixture = Fixture::new();
    let server = MockServer::start(happy);

    let samples = fixture.speak(&fixture.adapter(&server)).unwrap();

    assert_eq!(samples.len(), 12_000, "24 kHz mono, as sent");
    assert_eq!(server.calls(), vec![UPLOAD, INFER]);
    assert_eq!(
        fixture.held(),
        Some(RemoteSample {
            provider: RemoteProvider::DeepInfra,
            sample_sha256: fixture.sample_hash(),
            voice_id: "v-new".to_string(),
        })
    );

    let requests = server.requests();
    for request in &requests {
        assert_eq!(
            request.header("authorization"),
            Some(format!("Bearer {KEY}").as_str()),
            "the key travels only as the auth header"
        );
        assert!(
            request.header("user-agent").is_none(),
            "nothing about the machine is sent"
        );
    }

    // The upload: the sample and the two fixed strings, nothing else.
    let upload = requests[0].body_text();
    assert!(upload.contains(r#"name="files""#), "{upload}");
    assert!(upload.contains("voice-me reference sample"));
    assert!(upload.contains("Uploaded by voice-me"));
    assert_eq!(upload.matches("Content-Disposition").count(), 3, "{upload}");
    assert!(!upload.contains(KEY));

    // The inference: text, voice id, language tag, format — nothing else.
    let body: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "text": "Merhaba dünya",
            "voice_id": "v-new",
            "language_id": "tr",
            "response_format": "wav",
        })
    );
}

#[test]
fn a_later_line_references_the_stored_voice_only() {
    let fixture = Fixture::new();
    fixture.hold(&fixture.sample_hash(), "v-held");
    let server = MockServer::start(happy);

    fixture.speak(&fixture.adapter(&server)).unwrap();

    assert_eq!(server.calls(), vec![LOOKUP, INFER]);
    let body: serde_json::Value = serde_json::from_slice(&server.requests()[1].body).unwrap();
    assert_eq!(body["voice_id"], "v-held");
}

#[test]
fn a_re_recorded_sample_deletes_the_old_voice_and_uploads_the_new_one() {
    let fixture = Fixture::new();
    fixture.hold("an-older-hash", "v-old");
    let server = MockServer::start(happy);

    fixture.speak(&fixture.adapter(&server)).unwrap();

    assert_eq!(
        server.calls(),
        vec!["DELETE /v1/voices/v-old", UPLOAD, INFER]
    );
    assert_eq!(fixture.held().unwrap().voice_id, "v-new");
    assert_eq!(fixture.held().unwrap().sample_sha256, fixture.sample_hash());
}

#[test]
fn a_failed_delete_of_the_old_voice_does_not_stop_the_upload() {
    let fixture = Fixture::new();
    fixture.hold("an-older-hash", "v-old");
    let server = MockServer::start(|request| match request.method.as_str() {
        "DELETE" => Reply::json(500, r#"{"detail": "try later"}"#),
        _ => happy(request),
    });

    fixture.speak(&fixture.adapter(&server)).unwrap();

    assert_eq!(
        server.calls(),
        vec!["DELETE /v1/voices/v-old", UPLOAD, INFER]
    );
    assert_eq!(fixture.held().unwrap().voice_id, "v-new");
}

#[test]
fn a_failed_upload_is_one_error_and_no_inference() {
    let fixture = Fixture::new();
    let server = MockServer::start(|request| match request.path.as_str() {
        "/v1/voices/add" => Reply::json(400, r#"{"detail": "Audio too short"}"#),
        _ => happy(request),
    });

    let error = fixture.speak(&fixture.adapter(&server)).unwrap_err();

    assert_eq!(error.to_string(), "DeepInfra: Audio too short");
    assert_eq!(server.calls(), vec![UPLOAD], "no inference, no re-send");
    assert_eq!(fixture.held(), None);
}

#[test]
fn a_rejected_key_is_named_and_never_re_sent() {
    let fixture = Fixture::new();
    fixture.hold(&fixture.sample_hash(), "v-held");
    let server = MockServer::start(|_| Reply::json(401, r#"{"detail": "Unauthorized"}"#));

    let error = fixture.speak(&fixture.adapter(&server)).unwrap_err();

    assert_eq!(error.to_string(), "DeepInfra rejected the API key.");
    assert_eq!(
        server.calls(),
        vec![LOOKUP],
        "stopped before any text is sent"
    );
    assert_key_absent(&error);
    assert_eq!(
        fixture.held().unwrap().voice_id,
        "v-held",
        "a key problem says nothing about the voice"
    );
}

#[test]
fn a_provider_that_does_not_answer_in_time_is_named_with_the_deadline() {
    let fixture = Fixture::new();
    fixture.hold(&fixture.sample_hash(), "v-held");
    let server = MockServer::start(|request| happy(request).after(Duration::from_secs(3)));
    let deadlines = Deadlines {
        inference: Duration::from_secs(1),
        ..Deadlines::default()
    };

    let error = fixture
        .speak(&fixture.adapter_with(&server, deadlines))
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "DeepInfra did not answer within 1 second."
    );
    assert_eq!(server.calls(), vec![LOOKUP, INFER], "no re-send");
}

#[test]
fn an_upload_that_does_not_answer_in_time_is_named_with_the_deadline() {
    let fixture = Fixture::new();
    let server = MockServer::start(|request| match request.path.as_str() {
        "/v1/voices/add" => happy(request).after(Duration::from_secs(3)),
        _ => happy(request),
    });
    let deadlines = Deadlines {
        upload: Duration::from_secs(1),
        ..Deadlines::default()
    };

    let error = fixture
        .speak(&fixture.adapter_with(&server, deadlines))
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "DeepInfra did not answer within 1 second."
    );
    assert_eq!(server.calls(), vec![UPLOAD], "no inference, no re-send");
    assert_eq!(fixture.held(), None);
}

#[test]
fn a_delete_that_does_not_answer_in_time_keeps_the_id() {
    let fixture = Fixture::new();
    fixture.hold(&fixture.sample_hash(), "v-held");
    let server = MockServer::start(|request| happy(request).after(Duration::from_secs(3)));
    let deadlines = Deadlines {
        delete: Duration::from_secs(1),
        ..Deadlines::default()
    };

    let error = delete_held_sample_with(
        &DeepInfra::with_base_url(&server.base_url).with_deadlines(deadlines),
        fixture.store.as_ref(),
    )
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "DeepInfra did not answer within 1 second."
    );
    assert_eq!(server.calls(), vec!["DELETE /v1/voices/v-held"]);
    assert_eq!(fixture.held().unwrap().voice_id, "v-held");
}

/// The production path: `generate` dispatched through the AD-5 bridge on
/// a multi-threaded runtime, so the adapter reuses that runtime's handle
/// rather than building its own.
#[test]
fn a_line_through_the_tokio_bridge_uploads_and_generates() {
    let fixture = Fixture::new();
    let server = MockServer::start(happy);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let adapter = fixture.adapter(&server);
    let clip = fixture.clip.clone();

    let audio = futures::executor::block_on(voice_me_core::tokio_bridge::spawn_blocking_on(
        runtime.handle(),
        move || adapter.generate("Merhaba", &clip, "tr"),
    ))
    .unwrap();

    assert_eq!(audio.len(), 12_000);
    assert_eq!(server.calls(), vec![UPLOAD, INFER]);
}

#[test]
fn an_inference_404_that_does_not_name_the_voice_keeps_the_id() {
    let fixture = Fixture::new();
    fixture.hold(&fixture.sample_hash(), "v-held");
    let server = MockServer::start(|request| match request.method.as_str() {
        "GET" => happy(request),
        _ => Reply::json(404, r#"{"detail": "Model not found"}"#),
    });

    let error = fixture.speak(&fixture.adapter(&server)).unwrap_err();

    assert_eq!(error.to_string(), "DeepInfra: Model not found");
    assert_eq!(server.calls(), vec![LOOKUP, INFER]);
    assert_eq!(
        fixture.held().unwrap().voice_id,
        "v-held",
        "a retired model path says nothing about the voice"
    );
}

#[test]
fn audio_that_is_not_a_wav_is_a_deepinfra_failure() {
    let fixture = Fixture::new();
    fixture.hold(&fixture.sample_hash(), "v-held");
    let server = MockServer::start(|_| {
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"definitely not a wav");
        Reply::json(200, format!(r#"{{"audio": "{encoded}"}}"#))
    });

    let error = fixture.speak(&fixture.adapter(&server)).unwrap_err();

    assert!(matches!(error, VoiceMeError::Provider { .. }), "{error:?}");
    assert!(
        error
            .to_string()
            .starts_with("DeepInfra returned audio voice-me could not read"),
        "{error}"
    );
}

#[test]
fn an_empty_wav_is_a_deepinfra_failure_not_silence() {
    let fixture = Fixture::new();
    fixture.hold(&fixture.sample_hash(), "v-held");
    let server = MockServer::start(|_| {
        let encoded = base64::engine::general_purpose::STANDARD.encode(wav_bytes(24_000, 1, 0));
        Reply::json(200, format!(r#"{{"audio": "{encoded}"}}"#))
    });

    let error = fixture.speak(&fixture.adapter(&server)).unwrap_err();

    assert!(
        error
            .to_string()
            .starts_with("DeepInfra returned audio voice-me could not read"),
        "{error}"
    );
}

#[test]
fn the_default_deadlines_are_the_specs() {
    let deadlines = Deadlines::default();
    assert_eq!(deadlines.inference, Duration::from_secs(120));
    assert_eq!(deadlines.upload, Duration::from_secs(60));
    assert_eq!(deadlines.delete, Duration::from_secs(30));
}

#[test]
fn a_provider_error_carries_its_shortened_detail() {
    let fixture = Fixture::new();
    fixture.hold(&fixture.sample_hash(), "v-held");
    let server = MockServer::start(|request| match request.method.as_str() {
        "GET" => happy(request),
        _ => Reply::json(
            503,
            r#"{"detail": "Model is\n   currently overloaded", "request_id": "abc"}"#,
        ),
    });

    let error = fixture.speak(&fixture.adapter(&server)).unwrap_err();

    assert_eq!(
        error.to_string(),
        "DeepInfra: Model is currently overloaded"
    );
    assert_eq!(server.calls(), vec![LOOKUP, INFER]);
}

#[test]
fn a_stored_voice_the_provider_lost_is_dropped_and_not_re_sent() {
    let fixture = Fixture::new();
    fixture.hold(&fixture.sample_hash(), "v-gone");
    let server = MockServer::start(|request| match request.method.as_str() {
        "POST" if request.path.starts_with("/v1/inference") => {
            Reply::json(404, r#"{"detail": "voice not found"}"#)
        }
        _ => happy(request),
    });

    let error = fixture.speak(&fixture.adapter(&server)).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("uploaded again with the next line"),
        "{error}"
    );
    assert_eq!(
        server.calls(),
        vec!["GET /v1/voices/v-gone", INFER],
        "no automatic re-send"
    );
    assert_eq!(fixture.held(), None, "the next line uploads afresh");
}

/// Against the live API an unknown `voice_id` does not fail inference — it
/// is spoken in a stock voice. The lookup before the line is what keeps a
/// voice deleted elsewhere from being silently replaced.
#[test]
fn a_held_voice_deleted_elsewhere_is_caught_before_any_text_is_sent() {
    let fixture = Fixture::new();
    fixture.hold(&fixture.sample_hash(), "v-held");
    let server = MockServer::start(|request| match request.method.as_str() {
        "GET" => Reply::json(404, r#"{"detail": {"error": "voice not found"}}"#),
        _ => happy(request),
    });

    let error = fixture.speak(&fixture.adapter(&server)).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("uploaded again with the next line"),
        "{error}"
    );
    assert_eq!(server.calls(), vec![LOOKUP], "no text, no stock voice");
    assert_eq!(fixture.held(), None, "the next line uploads afresh");
}

#[test]
fn no_key_means_no_request() {
    let fixture = Fixture::new();
    fixture
        .store
        .save_api_key(RemoteProvider::DeepInfra, None)
        .unwrap();
    let server = MockServer::start(happy);

    let error = fixture.speak(&fixture.adapter(&server)).unwrap_err();

    assert!(error.to_string().contains("no API key"), "{error}");
    assert!(server.calls().is_empty());
}

#[test]
fn a_provider_wav_at_another_rate_is_resampled_to_24k() {
    let fixture = Fixture::new();
    fixture.hold(&fixture.sample_hash(), "v-held");
    let server = MockServer::start(|_| audio_reply(48_000));

    let samples = fixture.speak(&fixture.adapter(&server)).unwrap();

    // Half a second at 48 kHz is half a second at 24 kHz.
    assert!(
        (11_000..=13_000).contains(&samples.len()),
        "{}",
        samples.len()
    );
}

#[test]
fn delete_removes_the_voice_and_forgets_the_id() {
    let fixture = Fixture::new();
    fixture.hold(&fixture.sample_hash(), "v-held");
    let server = MockServer::start(happy);

    delete_held_sample_with(
        &DeepInfra::with_base_url(&server.base_url),
        fixture.store.as_ref(),
    )
    .unwrap();

    assert_eq!(server.calls(), vec!["DELETE /v1/voices/v-held"]);
    assert_eq!(
        server.requests()[0].header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    assert_eq!(fixture.held(), None);
}

#[test]
fn a_failed_delete_keeps_the_id_and_says_why() {
    let fixture = Fixture::new();
    fixture.hold(&fixture.sample_hash(), "v-held");
    let server = MockServer::start(|_| Reply::json(500, r#"{"detail": "internal"}"#));

    let error = delete_held_sample_with(
        &DeepInfra::with_base_url(&server.base_url),
        fixture.store.as_ref(),
    )
    .unwrap_err();

    assert_eq!(error.to_string(), "DeepInfra: internal");
    assert_key_absent(&error);
    assert_eq!(fixture.held().unwrap().voice_id, "v-held");
}

#[test]
fn deleting_a_voice_the_provider_already_lost_counts_as_deleted() {
    let fixture = Fixture::new();
    fixture.hold(&fixture.sample_hash(), "v-held");
    let server = MockServer::start(|_| Reply::json(404, "{}"));

    delete_held_sample_with(
        &DeepInfra::with_base_url(&server.base_url),
        fixture.store.as_ref(),
    )
    .unwrap();

    assert_eq!(fixture.held(), None);
}

#[test]
fn nothing_held_is_nothing_to_delete() {
    let fixture = Fixture::new();
    let server = MockServer::start(happy);

    delete_held_sample_with(
        &DeepInfra::with_base_url(&server.base_url),
        fixture.store.as_ref(),
    )
    .unwrap();

    assert!(server.calls().is_empty());
}

#[test]
fn an_unreachable_provider_is_named_without_the_key() {
    let fixture = Fixture::new();
    fixture.hold(&fixture.sample_hash(), "v-held");
    // A port nothing listens on.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let adapter = RemoteTtsAdapter::new(DeepInfra::with_base_url(base_url), fixture.store.clone());

    let error = fixture.speak(&adapter).unwrap_err();

    assert!(
        error
            .to_string()
            .starts_with("DeepInfra could not be reached"),
        "{error}"
    );
    assert_key_absent(&error);
}

#[test]
fn the_remote_engine_is_always_ready_and_has_nothing_to_warm() {
    let fixture = Fixture::new();
    let server = MockServer::start(happy);
    let adapter = fixture.adapter(&server);

    adapter.warm_up().unwrap();
    assert!(adapter.is_ready());
    assert!(
        server.calls().is_empty(),
        "warm-up is not a reachability probe"
    );
}
