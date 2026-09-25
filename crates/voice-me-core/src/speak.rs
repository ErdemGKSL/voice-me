//! The Speak Action, as a use-case function.
//!
//! The Prompt Overlay sends `AppEvent::SpeakRequested` and closes (AD-10);
//! everything after that is this function: generate the utterance, then play
//! it through the Virtual Microphone. It lives in the hexagon rather than in
//! `voice-me-app` for one reason: it is the place that decides *what the
//! user is told when the action goes wrong*, and that decision is domain
//! policy, not composition. Put in `main.rs` it would only be testable
//! through a running GPUI app; here it is a plain function over port trait
//! objects.
//!
//! It is written to be called from Tokio's blocking pool — it blocks for the
//! whole generation and for the whole of playback — never from GPUI's main
//! thread.

use std::sync::Mutex;

use crate::audio::AudioBuffer;
use crate::error::VoiceMeError;
use crate::ports::{NotificationPort, TtsPort, VirtualMicPort};
use crate::state::{
    AppState, BackendSelection, LanguageBackend, RemoteProvider, StockVoiceRefusal,
    resolve_stock_voice,
};

/// Held across [`VirtualMicPort::play`], so one utterance finishes draining
/// before the next one starts.
///
/// The TTS adapter's own session mutex does not cover this: it is released
/// the moment `generate` returns, which is *before* playback begins. Two
/// Speak Actions therefore overlap here whenever generation is faster than
/// the previous utterance takes to play — sub-realtime generation is the
/// normal case on the `cuda` build — and two streams into one device mix
/// rather than queue. The matrix promises "queued behind the first, both
/// heard in full", and this is what keeps that promise.
static PLAYBACK: Mutex<()> = Mutex::new(());

/// The title every failure notification carries.
///
/// One fixed sentence, with the specifics in the body: the user reads the
/// title first and always wants the same thing from it — "the line you typed
/// was not spoken" — while what went wrong differs every time.
pub const GENERATION_FAILED_SUMMARY: &str = "Couldn't generate speech.";

/// The title a *playback* failure carries instead.
///
/// The speech was generated: what is missing is its way out. Telling the
/// user "couldn't generate speech" there would send them after the model
/// cache when the fix is the Virtual Microphone device, which is exactly the
/// distinction [`VoiceMeError::VirtualMicUnavailable`] exists to keep.
pub const PLAYBACK_FAILED_SUMMARY: &str = "Couldn't reach the virtual microphone.";

/// The title of the one "this is taking a while" notification.
pub const STILL_WORKING_SUMMARY: &str = "voice-me is still getting ready.";

/// The body of that notification. Says both halves of what is happening:
/// the wait is the one-off engine start, and the line is not lost.
pub const STILL_WORKING_BODY: &str = "Loading the speech engine — this happens once and takes about a minute. \
     Your line will be spoken as soon as it's done.";

/// Run one Speak Action: generate `text` in the configured speech language,
/// in the voice of the active Reference Voice Sample, and play it through
/// the Virtual Microphone.
///
/// Notifies on failure — exactly once, whatever failed — and, when the
/// sessions are not built yet, notifies once up front that the wait is the
/// engine starting rather than the utterance being slow (spec-2-6 Decision
/// 4). A normal generation notifies nothing at all.
///
/// The generated audio is played through `virtual_mic` before this returns,
/// so a `Ok` means the line was actually spoken and drained by the audio
/// server — not merely generated. The buffer is returned as well, unchanged,
/// for callers that want to log or measure it.
pub fn speak(
    text: &str,
    state: &AppState,
    tts: &dyn TtsPort,
    virtual_mic: &dyn VirtualMicPort,
    notifications: &dyn NotificationPort,
) -> Result<AudioBuffer, VoiceMeError> {
    match speak_inner(text, state, tts, virtual_mic, notifications) {
        Ok(audio) => Ok(audio),
        Err(error) => {
            // A missing device and a failed generation have different fixes,
            // so they get different titles. Everything else — the body, the
            // exactly-once promise — is the same.
            let summary = match error {
                VoiceMeError::VirtualMicUnavailable(_) => PLAYBACK_FAILED_SUMMARY,
                _ => GENERATION_FAILED_SUMMARY,
            };
            // A notification that cannot be delivered has nowhere to be
            // reported to, so it is logged and the original failure — the
            // one the caller actually asked about — is returned intact.
            if let Err(delivery) = notifications.notify(summary, &error.to_string()) {
                eprintln!("could not show the failure notification: {delivery}");
            }
            Err(error)
        }
    }
}

fn speak_inner(
    text: &str,
    state: &AppState,
    tts: &dyn TtsPort,
    virtual_mic: &dyn VirtualMicPort,
    notifications: &dyn NotificationPort,
) -> Result<AudioBuffer, VoiceMeError> {
    // The overlay already drops a whitespace-only line, so this is a
    // backstop rather than the primary guard — but it is checked here, ahead
    // of everything expensive, so a second entry point can never reach the
    // engine with nothing to say.
    if text.trim().is_empty() {
        return Err(VoiceMeError::EmptyText);
    }

    // Story 3.11: the language is the *selected* backend's own, checked
    // against that backend's set (AD-12 for local Chatterbox, the provider's
    // model for a remote one). A value outside it — a hand-edited typo, or a
    // DeepInfra-only language saved for Local by hand — would otherwise reach
    // the model as a literal tag and come back as plausible audio in the
    // wrong language. It is refused by name, never replaced by a default.
    let backend = state.backend_selection.language_backend();
    let Some(stored) = state.speech_language() else {
        return Err(VoiceMeError::Other(format!(
            "{} has no speech language yet — choose another backend in Settings → Backend",
            backend.label()
        )));
    };

    // Story 3.12: a stock voice. The language and the stored voice are
    // checked against the voices the engine listed — refused by name,
    // never substituted — and there is no sample to check and nothing to
    // disclose: the engine runs on this machine. Story 3.15: Piper the
    // same way, against the voices installed in the cache.
    if matches!(
        backend,
        LanguageBackend::SystemVoice | LanguageBackend::Piper
    ) {
        let voice = resolve_stock_voice(
            backend,
            state.stock_voices(backend),
            stored,
            state.speech_voices.get(backend),
        )
        .map_err(|refusal| VoiceMeError::Other(refusal.to_string()))?;
        let audio = tts.generate(text, None, &voice.language, Some(&voice.id))?;
        return play(virtual_mic, audio);
    }

    // Story 3.17: Edge TTS speaks in a stock Microsoft voice through the
    // `edge-tts` program, with no key and no sample. The language and the
    // stored voice are checked against the voices the program listed —
    // refused by name, never substituted; an unset voice is the language's
    // first. Then the disclosure, before any process is spawned.
    if backend == LanguageBackend::Remote(RemoteProvider::EdgeTts) {
        let voice = resolve_stock_voice(
            backend,
            &state.edge_tts_voices,
            stored,
            state.speech_voices.get(backend),
        )
        .map_err(|refusal| VoiceMeError::Other(refusal.to_string()))?;
        if !state.disclosure_confirmed(RemoteProvider::EdgeTts) {
            return Err(VoiceMeError::DisclosureNotConfirmed(
                RemoteProvider::EdgeTts.label().to_string(),
            ));
        }
        let audio = tts.generate(text, None, &voice.language, Some(&voice.id))?;
        return play(virtual_mic, audio);
    }

    // Story 3.14: Azure speaks in a stock Microsoft voice. A voice has to
    // be chosen (D4). With the voice list fetched, the stored locale and
    // voice are checked against it — refused by name, never substituted;
    // without it, they are sent as stored and Azure's own answer is the
    // check. Then the disclosure, and never a sample.
    if backend == LanguageBackend::Remote(RemoteProvider::Azure) {
        let refuse = |refusal: StockVoiceRefusal| VoiceMeError::Other(refusal.to_string());
        let Some(voice) = state.speech_voices.get(backend) else {
            return Err(refuse(StockVoiceRefusal::NoVoice(backend)));
        };
        let (locale, voice) = if state.azure_voices.is_empty() {
            (stored.trim().to_string(), voice.to_string())
        } else {
            let listed = resolve_stock_voice(backend, &state.azure_voices, stored, Some(voice))
                .map_err(refuse)?;
            (listed.language.clone(), listed.id.clone())
        };
        if !state.disclosure_confirmed(RemoteProvider::Azure) {
            return Err(VoiceMeError::DisclosureNotConfirmed(
                RemoteProvider::Azure.label().to_string(),
            ));
        }
        let audio = tts.generate(text, None, &locale, Some(&voice))?;
        return play(virtual_mic, audio);
    }

    let Some(language) = backend.speech_language(stored) else {
        return Err(VoiceMeError::Other(format!(
            "{} can't speak the speech language {stored:?} — choose one in Settings → Backend",
            backend.label()
        )));
    };

    // Story 3.6: nothing leaves the machine for a provider whose disclosure
    // the user has not confirmed. Enforced here, in the use case, rather
    // than trusted to the adapter — and before the sample check, so not
    // even the "is there a voice to send" question is asked first.
    if let BackendSelection::Remote(provider) = &state.backend_selection
        && !state.disclosure_confirmed(*provider)
    {
        return Err(VoiceMeError::DisclosureNotConfirmed(
            provider.label().to_string(),
        ));
    }

    let Some(reference_clip) = state.reference_voice_sample.as_ref() else {
        // Deliberately before any session work: with no voice to clone
        // there is nothing a built engine could do, so a first-run user who
        // presses the hotkey never pays the 90-second build to be told that.
        return Err(VoiceMeError::NoReferenceVoiceSample);
    };

    if !tts.is_ready()
        && let Err(delivery) = notifications.notify(STILL_WORKING_SUMMARY, STILL_WORKING_BODY)
    {
        eprintln!("could not show the still-working notification: {delivery}");
    }

    let audio = tts.generate(text, Some(reference_clip), language.code, None)?;
    play(virtual_mic, audio)
}

fn play(virtual_mic: &dyn VirtualMicPort, audio: AudioBuffer) -> Result<AudioBuffer, VoiceMeError> {
    // The AD-11 buffer crosses straight from one port to the other,
    // unconverted: 24 kHz mono f32 is what the decoder emits and what the
    // adapter declares to the audio server. `play` blocks until the server
    // has drained it, which is why this whole function belongs on the AD-5
    // blocking pool — and why the wait for the lock is the queue the matrix
    // describes. A poisoned lock means a previous `play` panicked; the next
    // utterance is still better off spoken than refused.
    let _playing = PLAYBACK.lock().unwrap_or_else(|poison| poison.into_inner());
    virtual_mic.play(&audio)?;

    Ok(audio)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, Mutex};
    use std::time::Duration;

    use super::*;
    use crate::state::{RemoteProvider, SpeechLanguages};

    #[derive(Default)]
    struct FakeNotifier {
        shown: Mutex<Vec<(String, String)>>,
    }

    impl NotificationPort for FakeNotifier {
        fn notify(&self, summary: &str, body: &str) -> Result<(), VoiceMeError> {
            self.shown
                .lock()
                .unwrap()
                .push((summary.to_string(), body.to_string()));
            Ok(())
        }
    }

    impl FakeNotifier {
        fn summaries(&self) -> Vec<String> {
            self.shown
                .lock()
                .unwrap()
                .iter()
                .map(|(summary, _)| summary.clone())
                .collect()
        }

        fn bodies(&self) -> Vec<String> {
            self.shown
                .lock()
                .unwrap()
                .iter()
                .map(|(_, body)| body.clone())
                .collect()
        }
    }

    /// A `VirtualMicPort` that records the buffers it played and can be told
    /// to be unavailable. No audio server, no device.
    ///
    /// Like `ExclusiveTts`, it also notices overlap: a device that two
    /// utterances reach at once is the failure `PLAYBACK` exists to prevent,
    /// and counting calls cannot tell a queue from a collision.
    #[derive(Default)]
    struct FakeMic {
        played: Mutex<Vec<Vec<f32>>>,
        /// The `VirtualMicUnavailable` reason, if this device is missing. A
        /// `String` rather than a `VoiceMeError` because the failure is a
        /// standing condition — a missing device does not become present
        /// because it was asked twice — and `VoiceMeError` is not `Clone`.
        unavailable: Option<String>,
        in_flight: AtomicUsize,
        overlapped: AtomicBool,
    }

    impl FakeMic {
        fn unavailable(reason: &str) -> Self {
            Self {
                unavailable: Some(reason.to_string()),
                ..Self::default()
            }
        }

        fn played(&self) -> Vec<Vec<f32>> {
            self.played.lock().unwrap().clone()
        }
    }

    impl VirtualMicPort for FakeMic {
        fn play(&self, audio: &AudioBuffer) -> Result<(), VoiceMeError> {
            // Before anything is recorded: audio handed to a device that is
            // not there was never played.
            if let Some(reason) = self.unavailable.as_ref() {
                return Err(VoiceMeError::VirtualMicUnavailable(reason.clone()));
            }

            if self.in_flight.fetch_add(1, Ordering::SeqCst) != 0 {
                self.overlapped.store(true, Ordering::SeqCst);
            }
            // Deliberately longer than `ExclusiveTts`'s own 20 ms: the
            // second action cannot start playing until it has generated, so
            // a playback shorter than a generation would drain before the
            // next one arrives and the overlap this guards would never be
            // reachable — which is the real shape of the bug, an utterance
            // that takes longer to play than the next takes to generate.
            std::thread::sleep(Duration::from_millis(60));
            self.played.lock().unwrap().push(audio.samples().to_vec());
            self.in_flight.fetch_sub(1, Ordering::SeqCst);

            Ok(())
        }
    }

    /// A `TtsPort` that records its calls and can be told to fail. No model
    /// files, no ONNX Runtime, no network.
    struct FakeTts {
        ready: AtomicBool,
        calls: Mutex<Vec<(String, PathBuf, String)>>,
        /// The voice each call was given, in step with `calls`.
        voices: Mutex<Vec<(Option<PathBuf>, Option<String>)>>,
        fail_with: Mutex<Option<VoiceMeError>>,
    }

    impl Default for FakeTts {
        fn default() -> Self {
            Self {
                ready: AtomicBool::new(true),
                calls: Mutex::new(Vec::new()),
                voices: Mutex::new(Vec::new()),
                fail_with: Mutex::new(None),
            }
        }
    }

    impl FakeTts {
        fn cold() -> Self {
            let fake = Self::default();
            fake.ready.store(false, Ordering::SeqCst);
            fake
        }

        fn failing(error: VoiceMeError) -> Self {
            let fake = Self::default();
            *fake.fail_with.lock().unwrap() = Some(error);
            fake
        }
    }

    impl TtsPort for FakeTts {
        fn warm_up(&self) -> Result<(), VoiceMeError> {
            self.ready.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn is_ready(&self) -> bool {
            self.ready.load(Ordering::SeqCst)
        }

        fn generate(
            &self,
            text: &str,
            reference_clip: Option<&Path>,
            language: &str,
            voice: Option<&str>,
        ) -> Result<AudioBuffer, VoiceMeError> {
            self.calls.lock().unwrap().push((
                text.to_string(),
                reference_clip.map(Path::to_path_buf).unwrap_or_default(),
                language.to_string(),
            ));
            self.voices.lock().unwrap().push((
                reference_clip.map(Path::to_path_buf),
                voice.map(str::to_string),
            ));
            if let Some(error) = self.fail_with.lock().unwrap().take() {
                return Err(error);
            }
            Ok(AudioBuffer::new(vec![0.1; 24]))
        }
    }

    /// Local Chatterbox with a sample. Explicitly the bundled CPU: since
    /// Story 3.15 an unsaved selection is Piper on Linux.
    fn state_with_a_sample() -> AppState {
        AppState {
            reference_voice_sample: Some(PathBuf::from("/data/reference_voice_sample.wav")),
            backend_selection: BackendSelection::BUNDLED_CPU,
            ..AppState::default()
        }
    }

    #[test]
    fn a_normal_generation_notifies_nothing() {
        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();

        let audio = speak("Merhaba", &state_with_a_sample(), &tts, &mic, &notifier).unwrap();

        assert!(!audio.is_empty());
        assert!(
            notifier.summaries().is_empty(),
            "a ~20 s generation is normal, not an event worth interrupting the user for"
        );
        assert_eq!(
            tts.calls.lock().unwrap()[0],
            (
                "Merhaba".to_string(),
                PathBuf::from("/data/reference_voice_sample.wav"),
                "tr".to_string()
            ),
            "the selected speech language travels from AppState, not a constant"
        );
    }

    #[test]
    fn no_reference_voice_sample_notifies_and_never_touches_the_engine() {
        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();
        let state = AppState {
            backend_selection: BackendSelection::BUNDLED_CPU,
            ..AppState::default()
        };

        let error = speak("Merhaba", &state, &tts, &mic, &notifier).unwrap_err();

        assert!(matches!(error, VoiceMeError::NoReferenceVoiceSample));
        assert!(
            tts.calls.lock().unwrap().is_empty(),
            "no session work may happen when there is no voice to clone"
        );
        assert_eq!(notifier.summaries(), vec![GENERATION_FAILED_SUMMARY]);
        assert!(
            notifier.bodies()[0].contains("Reference Voice Sample"),
            "the notification has to name the missing sample: {:?}",
            notifier.bodies()
        );
    }

    #[test]
    fn a_missing_model_file_is_notified_with_the_exact_path() {
        let missing = PathBuf::from("/cache/voice-me/onnx/language_model_q4.onnx_data");
        let tts = FakeTts::failing(VoiceMeError::MissingRuntimeAsset {
            path: missing.clone(),
        });
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();

        let error = speak("Merhaba", &state_with_a_sample(), &tts, &mic, &notifier).unwrap_err();

        assert!(matches!(error, VoiceMeError::MissingRuntimeAsset { .. }));
        assert_eq!(notifier.summaries(), vec![GENERATION_FAILED_SUMMARY]);
        assert!(
            notifier.bodies()[0].contains(missing.to_str().unwrap()),
            "the whole point of MissingRuntimeAsset is that the path reaches the surface: {:?}",
            notifier.bodies()
        );
    }

    #[test]
    fn an_engine_failure_notifies_once_with_the_reason_and_yields_no_audio() {
        let tts = FakeTts::failing(VoiceMeError::SpeechEngine(
            "conditional_decoder: device lost".to_string(),
        ));
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();

        let result = speak("Merhaba", &state_with_a_sample(), &tts, &mic, &notifier);

        assert!(
            result.is_err(),
            "corrupted audio must never look like success"
        );
        assert_eq!(
            notifier.summaries(),
            vec![GENERATION_FAILED_SUMMARY],
            "exactly one notification per failed action"
        );
        assert!(notifier.bodies()[0].contains("device lost"));
    }

    #[test]
    fn a_cold_engine_notifies_once_and_still_produces_the_audio() {
        let tts = FakeTts::cold();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();

        let audio = speak("Merhaba", &state_with_a_sample(), &tts, &mic, &notifier).unwrap();

        assert!(!audio.is_empty(), "the audio still arrives afterwards");
        assert_eq!(
            notifier.summaries(),
            vec![STILL_WORKING_SUMMARY],
            "one notification, and no failure notification alongside it"
        );
    }

    fn with_languages(state: AppState, local: &str, deepinfra: &str) -> AppState {
        AppState {
            speech_languages: SpeechLanguages {
                local: local.to_string(),
                deepinfra: deepinfra.to_string(),
                ..SpeechLanguages::default()
            },
            ..state
        }
    }

    fn deepinfra_confirmed(state: AppState) -> AppState {
        AppState {
            backend_selection: BackendSelection::Remote(RemoteProvider::DeepInfra),
            confirmed_disclosures: vec![RemoteProvider::DeepInfra],
            ..state
        }
    }

    /// The I/O matrix's Out of set row: a value outside the *selected*
    /// backend's set is refused by name before the engine, naming the
    /// backend, the value and where to fix it.
    #[test]
    fn a_language_outside_the_selected_backends_set_is_refused_by_name() {
        for bad in ["es", "turkish"] {
            let tts = FakeTts::default();
            let notifier = FakeNotifier::default();
            let mic = FakeMic::default();
            let state = with_languages(state_with_a_sample(), bad, "tr");

            let error = speak("Merhaba", &state, &tts, &mic, &notifier).unwrap_err();

            assert!(
                tts.calls.lock().unwrap().is_empty(),
                "{bad} must not reach the model as a literal tag"
            );
            let message = error.to_string();
            assert!(
                message.contains(bad)
                    && message.contains("local Chatterbox")
                    && message.contains("Settings → Backend"),
                "the message names the value, the backend and the fix: {message}"
            );
            assert_eq!(notifier.summaries(), vec![GENERATION_FAILED_SUMMARY]);
        }
    }

    /// The same `es` that Local refuses is one DeepInfra speaks.
    #[test]
    fn a_language_is_checked_against_the_selected_backend_not_a_global_list() {
        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();
        let state = deepinfra_confirmed(with_languages(state_with_a_sample(), "en", "es"));

        speak("Hola", &state, &tts, &mic, &notifier).unwrap();

        assert_eq!(tts.calls.lock().unwrap()[0].2, "es");
        assert!(notifier.summaries().is_empty());
    }

    /// The I/O matrix's Switch backends row: each backend's own language
    /// reaches `generate`, and switching rewrites neither.
    #[test]
    fn switching_backends_switches_the_language_that_reaches_generate() {
        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();
        let languages = with_languages(state_with_a_sample(), "en", "es");

        speak(
            "One",
            &deepinfra_confirmed(languages.clone()),
            &tts,
            &mic,
            &notifier,
        )
        .unwrap();
        let cpu = AppState {
            backend_selection: BackendSelection::BUNDLED_CPU,
            ..deepinfra_confirmed(languages.clone())
        };
        speak("Two", &cpu, &tts, &mic, &notifier).unwrap();
        speak(
            "Three",
            &deepinfra_confirmed(languages),
            &tts,
            &mic,
            &notifier,
        )
        .unwrap();

        let tags: Vec<_> = tts
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| call.2.clone())
            .collect();
        assert_eq!(tags, vec!["es", "en", "es"]);
    }

    /// fal.ai has no speech language yet: refused by name, never given one.
    #[test]
    fn fal_ai_is_refused_rather_than_given_a_language() {
        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();
        let state = AppState {
            backend_selection: BackendSelection::Remote(RemoteProvider::FalAi),
            confirmed_disclosures: vec![RemoteProvider::FalAi],
            ..state_with_a_sample()
        };

        let error = speak("Merhaba", &state, &tts, &mic, &notifier).unwrap_err();

        assert!(tts.calls.lock().unwrap().is_empty());
        assert!(error.to_string().contains("fal.ai"), "{error}");
    }

    #[test]
    fn the_speech_language_is_normalised_before_it_becomes_a_tag() {
        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();
        let state = with_languages(state_with_a_sample(), "  EN \n", "tr");

        speak("Hello", &state, &tts, &mic, &notifier).unwrap();

        assert_eq!(
            tts.calls.lock().unwrap()[0].2,
            "en",
            "a hand-edited file with stray whitespace or capitals still works"
        );
    }

    #[test]
    fn empty_text_is_rejected_before_the_engine_and_before_the_sample_check() {
        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();

        let error = speak("   ", &AppState::default(), &tts, &mic, &notifier).unwrap_err();

        assert!(matches!(error, VoiceMeError::EmptyText));
        assert!(tts.calls.lock().unwrap().is_empty());
    }

    /// The I/O-matrix "second Speak mid-generation" row, at the `speak`
    /// level: two concurrent actions against a port that would notice
    /// overlap, proving `speak` itself adds no concurrency of its own. The
    /// serialization *guarantee* is the adapter's, and is tested in
    /// `voice-me-tts` against `SessionSlot` — the type that holds the
    /// sessions and the lock — rather than against a live `TtsAdapter`,
    /// which cannot be constructed without 1.56 GB of model files.
    #[test]
    fn two_concurrent_speak_actions_both_produce_audio() {
        /// Fails loudly if a second `generate` overlaps the first.
        struct ExclusiveTts {
            gate: Mutex<()>,
            in_flight: AtomicUsize,
            overlapped: AtomicBool,
        }

        impl TtsPort for ExclusiveTts {
            fn warm_up(&self) -> Result<(), VoiceMeError> {
                Ok(())
            }

            fn is_ready(&self) -> bool {
                true
            }

            fn generate(
                &self,
                _text: &str,
                _reference_clip: Option<&Path>,
                _language: &str,
                _voice: Option<&str>,
            ) -> Result<AudioBuffer, VoiceMeError> {
                let _held = self
                    .gate
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                if self.in_flight.fetch_add(1, Ordering::SeqCst) != 0 {
                    self.overlapped.store(true, Ordering::SeqCst);
                }
                std::thread::sleep(Duration::from_millis(20));
                self.in_flight.fetch_sub(1, Ordering::SeqCst);
                Ok(AudioBuffer::new(vec![0.1; 24]))
            }
        }

        let tts = Arc::new(ExclusiveTts {
            gate: Mutex::new(()),
            in_flight: AtomicUsize::new(0),
            overlapped: AtomicBool::new(false),
        });
        let notifier = Arc::new(FakeNotifier::default());
        let mic = Arc::new(FakeMic::default());
        let start = Arc::new(Barrier::new(2));
        let state = state_with_a_sample();

        let handles: Vec<_> = ["first", "second"]
            .into_iter()
            .map(|line| {
                let tts = tts.clone();
                let notifier = notifier.clone();
                let mic = mic.clone();
                let start = start.clone();
                let state = state.clone();
                std::thread::spawn(move || {
                    start.wait();
                    speak(line, &state, tts.as_ref(), mic.as_ref(), notifier.as_ref())
                })
            })
            .collect();

        for handle in handles {
            assert!(handle.join().unwrap().is_ok(), "both actions produce audio");
        }
        assert!(!tts.overlapped.load(Ordering::SeqCst));
        assert!(notifier.summaries().is_empty());
        assert_eq!(
            mic.played().len(),
            2,
            "both lines are heard in full, not just the first"
        );
        assert!(
            !mic.overlapped.load(Ordering::SeqCst),
            "two streams into one device mix rather than queue — the second \
             utterance has to wait for the first to drain"
        );
    }

    /// The matrix's happy path at this level: what the engine produced is
    /// what the Virtual Microphone is handed — no resampling, no trimming,
    /// no conversion in the hexagon (AD-11).
    #[test]
    fn the_generated_buffer_reaches_the_virtual_microphone_unchanged() {
        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();

        let audio = speak("Merhaba", &state_with_a_sample(), &tts, &mic, &notifier).unwrap();

        assert_eq!(
            mic.played(),
            vec![audio.samples().to_vec()],
            "the buffer crosses TtsPort → VirtualMicPort as-is, exactly once"
        );
    }

    /// Story 3.6: core, not the adapter, refuses an unconfirmed remote
    /// provider — before `generate` is ever reached.
    #[test]
    fn an_unconfirmed_remote_provider_is_refused_before_generate() {
        use crate::state::RemoteProvider;

        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();
        let state = AppState {
            backend_selection: BackendSelection::Remote(RemoteProvider::DeepInfra),
            ..state_with_a_sample()
        };

        let error = speak("Merhaba", &state, &tts, &mic, &notifier).unwrap_err();

        assert!(matches!(error, VoiceMeError::DisclosureNotConfirmed(_)));
        assert!(
            tts.calls.lock().unwrap().is_empty(),
            "no byte may reach the provider before the disclosure is confirmed"
        );
        assert_eq!(notifier.summaries(), vec![GENERATION_FAILED_SUMMARY]);
        assert!(notifier.bodies()[0].contains("DeepInfra"));

        // Confirming another provider does not count.
        let other = AppState {
            confirmed_disclosures: vec![RemoteProvider::FalAi],
            ..state.clone()
        };
        speak("Merhaba", &other, &tts, &mic, &notifier).unwrap_err();
        assert!(tts.calls.lock().unwrap().is_empty());

        let confirmed = AppState {
            confirmed_disclosures: vec![RemoteProvider::DeepInfra],
            ..state
        };
        speak("Merhaba", &confirmed, &tts, &mic, &notifier).unwrap();
        assert_eq!(tts.calls.lock().unwrap().len(), 1);
    }

    /// A provider failure is the one generation notification, naming the
    /// provider and the reason.
    #[test]
    fn a_provider_failure_is_one_notification_naming_the_provider() {
        let tts = FakeTts::failing(VoiceMeError::Provider {
            provider: "DeepInfra".to_string(),
            reason: "DeepInfra rejected the API key.".to_string(),
        });
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();

        speak("Merhaba", &state_with_a_sample(), &tts, &mic, &notifier).unwrap_err();

        assert_eq!(notifier.summaries(), vec![GENERATION_FAILED_SUMMARY]);
        assert_eq!(notifier.bodies(), vec!["DeepInfra rejected the API key."]);
        assert!(mic.played().is_empty());
    }

    /// A missing device and a failed generation have different fixes, so the
    /// user must not be sent after the wrong one.
    #[test]
    fn an_unavailable_virtual_microphone_is_notified_as_playback_not_generation() {
        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::unavailable("there is no `voice-me` device");

        let error = speak("Merhaba", &state_with_a_sample(), &tts, &mic, &notifier).unwrap_err();

        assert!(matches!(error, VoiceMeError::VirtualMicUnavailable(_)));
        assert_eq!(
            notifier.summaries(),
            vec![PLAYBACK_FAILED_SUMMARY],
            "exactly one notification, and not the generation one — the speech was fine"
        );
        assert!(
            notifier.bodies()[0].contains("voice-me"),
            "the reason from the adapter has to reach the user: {:?}",
            notifier.bodies()
        );
    }

    /// Nothing to play means nothing may be played: a failed generation must
    /// not reach the device at all, or the user would get two failures for
    /// one action.
    #[test]
    fn a_generation_failure_never_reaches_the_virtual_microphone() {
        let tts = FakeTts::failing(VoiceMeError::SpeechEngine("decoder exploded".to_string()));
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();

        speak("Merhaba", &state_with_a_sample(), &tts, &mic, &notifier).unwrap_err();

        assert!(
            mic.played().is_empty(),
            "play is never called without audio"
        );
        assert_eq!(notifier.summaries(), vec![GENERATION_FAILED_SUMMARY]);
    }

    fn system_voice(id: &str, language: &str, name: &str) -> crate::state::StockVoice {
        crate::state::StockVoice {
            id: id.to_string(),
            language: language.to_string(),
            language_label: name.to_string(),
            name: name.to_string(),
            priority: 5,
        }
    }

    /// The System voice selected, speaking `language` with `voice` stored,
    /// against a small eSpeak-shaped list — and no Reference Voice Sample.
    fn system_voice_state(language: &str, voice: Option<&str>) -> AppState {
        AppState {
            backend_selection: BackendSelection::SystemVoice,
            speech_languages: SpeechLanguages {
                system_voice: language.to_string(),
                ..SpeechLanguages::default()
            },
            speech_voices: crate::state::SpeechVoices {
                system_voice: voice.map(str::to_string),
                ..Default::default()
            },
            system_voices: vec![
                system_voice("gmw/en-US", "en-us", "English (America)"),
                system_voice("trk/tr", "tr", "Turkish"),
                system_voice("sit/yue", "yue", "Chinese (Cantonese)"),
                system_voice(
                    "sit/yue-Latn-jyutping",
                    "yue",
                    "Chinese (Cantonese, latin as Jyutping)",
                ),
            ],
            ..AppState::default()
        }
    }

    /// The matrix's Speak row: no sample, no disclosure, the voice id and
    /// no clip reach the engine, and the buffer is played.
    #[test]
    fn the_system_voice_speaks_with_no_sample_in_the_listed_voice() {
        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();

        speak(
            "Merhaba",
            &system_voice_state("tr", None),
            &tts,
            &mic,
            &notifier,
        )
        .unwrap();

        assert_eq!(tts.calls.lock().unwrap()[0].2, "tr");
        assert_eq!(
            tts.voices.lock().unwrap()[0],
            (None, Some("trk/tr".to_string()))
        );
        assert_eq!(mic.played().len(), 1);
        assert!(notifier.summaries().is_empty());
    }

    /// The Several voices and Pick a voice rows: unset is the top-priority
    /// voice; a stored one of the language's is used as stored.
    #[test]
    fn the_system_voice_uses_the_stored_voice_or_the_languages_top_one() {
        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();

        speak(
            "One",
            &system_voice_state("yue", None),
            &tts,
            &mic,
            &notifier,
        )
        .unwrap();
        speak(
            "Two",
            &system_voice_state("yue", Some("sit/yue-Latn-jyutping")),
            &tts,
            &mic,
            &notifier,
        )
        .unwrap();

        let voices: Vec<_> = tts
            .voices
            .lock()
            .unwrap()
            .iter()
            .map(|(_, voice)| voice.clone().unwrap())
            .collect();
        assert_eq!(voices, vec!["sit/yue", "sit/yue-Latn-jyutping"]);
    }

    /// The Change language row, at this level: the language saved with its
    /// voice cleared speaks in the new language's only voice.
    #[test]
    fn a_new_system_voice_language_with_no_voice_uses_its_own() {
        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();

        speak(
            "Bir",
            &system_voice_state("tr", None),
            &tts,
            &mic,
            &notifier,
        )
        .unwrap();

        assert_eq!(tts.voices.lock().unwrap()[0].1.as_deref(), Some("trk/tr"));
    }

    /// The Out of set row: an unlisted language, or a stored voice of
    /// another language, is refused by name before the engine, once.
    #[test]
    fn an_unlisted_system_voice_language_or_voice_is_refused_by_name() {
        for (state, named) in [
            (system_voice_state("xx", None), "\"xx\""),
            (system_voice_state("tr", Some("sit/yue")), "\"sit/yue\""),
            (
                AppState {
                    system_voices: Vec::new(),
                    ..system_voice_state("tr", None)
                },
                "Settings → Backend",
            ),
        ] {
            let tts = FakeTts::default();
            let notifier = FakeNotifier::default();
            let mic = FakeMic::default();

            let error = speak("Merhaba", &state, &tts, &mic, &notifier).unwrap_err();

            assert!(tts.calls.lock().unwrap().is_empty());
            let message = error.to_string();
            assert!(
                message.contains(named) && message.contains("System voice"),
                "{message}"
            );
            assert_eq!(notifier.summaries(), vec![GENERATION_FAILED_SUMMARY]);
        }
    }

    /// The cloning backends still require the sample.
    #[test]
    fn the_cloning_backends_still_require_the_sample() {
        for selection in [
            BackendSelection::BUNDLED_CPU,
            BackendSelection::Remote(RemoteProvider::DeepInfra),
        ] {
            let tts = FakeTts::default();
            let notifier = FakeNotifier::default();
            let mic = FakeMic::default();
            let state = AppState {
                backend_selection: selection,
                confirmed_disclosures: vec![RemoteProvider::DeepInfra],
                ..AppState::default()
            };

            let error = speak("Merhaba", &state, &tts, &mic, &notifier).unwrap_err();

            assert!(matches!(error, VoiceMeError::NoReferenceVoiceSample));
            assert!(tts.calls.lock().unwrap().is_empty());
        }

        // With one, the clip reaches the engine and no voice does.
        let tts = FakeTts::default();
        speak(
            "Merhaba",
            &state_with_a_sample(),
            &tts,
            &FakeMic::default(),
            &FakeNotifier::default(),
        )
        .unwrap();
        assert_eq!(
            tts.voices.lock().unwrap()[0],
            (
                Some(PathBuf::from("/data/reference_voice_sample.wav")),
                None
            )
        );
    }

    fn azure_voice(short_name: &str, locale: &str) -> crate::state::StockVoice {
        crate::state::StockVoice {
            id: short_name.to_string(),
            language: locale.to_string(),
            language_label: locale.to_string(),
            name: short_name.to_string(),
            priority: 0,
        }
    }

    /// Azure selected, key and region saved (the check guarantees those),
    /// speaking `locale` in `voice`, disclosure confirmed — and no sample.
    fn azure_state(locale: &str, voice: Option<&str>) -> AppState {
        AppState {
            backend_selection: BackendSelection::Remote(RemoteProvider::Azure),
            confirmed_disclosures: vec![RemoteProvider::Azure],
            azure_region: Some("westeurope".to_string()),
            speech_languages: SpeechLanguages {
                azure: locale.to_string(),
                ..SpeechLanguages::default()
            },
            speech_voices: crate::state::SpeechVoices {
                azure: voice.map(str::to_string),
                ..Default::default()
            },
            azure_voices: vec![
                azure_voice("tr-TR-EmelNeural", "tr-TR"),
                azure_voice("tr-TR-AhmetNeural", "tr-TR"),
                azure_voice("en-US-JennyNeural", "en-US"),
            ],
            ..AppState::default()
        }
    }

    /// The matrix's Speak row: the locale and voice reach `generate`, no
    /// clip does, and the buffer is played.
    #[test]
    fn azure_speaks_the_stored_voice_with_no_sample() {
        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();

        speak(
            "Merhaba",
            &azure_state("tr-TR", Some("tr-TR-EmelNeural")),
            &tts,
            &mic,
            &notifier,
        )
        .unwrap();

        assert_eq!(tts.calls.lock().unwrap()[0].2, "tr-TR");
        assert_eq!(
            tts.voices.lock().unwrap()[0],
            (None, Some("tr-TR-EmelNeural".to_string()))
        );
        assert_eq!(mic.played().len(), 1);
        assert!(notifier.summaries().is_empty());
    }

    /// The First line row: nothing is sent until the disclosure is
    /// confirmed.
    #[test]
    fn azure_is_refused_before_generate_until_its_disclosure_is_confirmed() {
        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();
        let state = AppState {
            confirmed_disclosures: vec![RemoteProvider::DeepInfra],
            ..azure_state("tr-TR", Some("tr-TR-EmelNeural"))
        };

        let error = speak("Merhaba", &state, &tts, &mic, &notifier).unwrap_err();

        assert!(matches!(error, VoiceMeError::DisclosureNotConfirmed(ref name) if name == "Azure"));
        assert!(tts.calls.lock().unwrap().is_empty());
        assert_eq!(notifier.summaries(), vec![GENERATION_FAILED_SUMMARY]);
    }

    /// The No voice row: refused by name, whatever the list says.
    #[test]
    fn azure_with_no_voice_is_refused_before_generate() {
        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();

        let error = speak(
            "Merhaba",
            &azure_state("tr-TR", None),
            &tts,
            &mic,
            &notifier,
        )
        .unwrap_err();

        assert!(tts.calls.lock().unwrap().is_empty());
        assert!(
            error.to_string().contains("Azure has no voice selected"),
            "{error}"
        );
    }

    /// The Stale voice row, and the Change locale row at this level: a
    /// stored voice that is not one of the locale's is refused by name,
    /// once, before any request.
    #[test]
    fn a_stale_azure_voice_is_refused_by_name_before_any_request() {
        for state in [
            azure_state("tr-TR", Some("tr-TR-GoneNeural")),
            azure_state("en-US", Some("tr-TR-EmelNeural")),
        ] {
            let tts = FakeTts::default();
            let notifier = FakeNotifier::default();
            let mic = FakeMic::default();

            let error = speak("Merhaba", &state, &tts, &mic, &notifier).unwrap_err();

            assert!(tts.calls.lock().unwrap().is_empty());
            let message = error.to_string();
            assert!(
                message.contains("Azure") && message.contains("Neural"),
                "{message}"
            );
            assert_eq!(notifier.summaries(), vec![GENERATION_FAILED_SUMMARY]);
        }
    }

    /// The List not fetched row: the stored locale and voice are spoken as
    /// stored; a listed one is sent in the list's own spelling.
    #[test]
    fn azure_with_no_list_speaks_the_stored_locale_and_voice() {
        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();
        let unlisted = AppState {
            azure_voices: Vec::new(),
            ..azure_state("tr-TR", Some("tr-TR-GoneNeural"))
        };

        speak("Bir", &unlisted, &tts, &mic, &notifier).unwrap();
        speak(
            "Iki",
            &azure_state(" tr-tr ", Some("tr-TR-AhmetNeural")),
            &tts,
            &mic,
            &notifier,
        )
        .unwrap();

        let calls: Vec<_> = tts
            .voices
            .lock()
            .unwrap()
            .iter()
            .map(|(_, voice)| voice.clone().unwrap())
            .collect();
        assert_eq!(calls, vec!["tr-TR-GoneNeural", "tr-TR-AhmetNeural"]);
        let locales: Vec<_> = tts
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| call.2.clone())
            .collect();
        assert_eq!(locales, vec!["tr-TR", "tr-TR"]);
    }

    fn edge_voice(id: &str, locale: &str, priority: u32) -> crate::state::StockVoice {
        crate::state::StockVoice {
            id: id.to_string(),
            language: locale.to_string(),
            language_label: locale.to_string(),
            name: id.to_string(),
            priority,
        }
    }

    /// Edge TTS selected, its voices listed, disclosure confirmed — no key
    /// and no sample.
    fn edge_state(locale: &str, voice: Option<&str>) -> AppState {
        AppState {
            backend_selection: BackendSelection::Remote(RemoteProvider::EdgeTts),
            confirmed_disclosures: vec![RemoteProvider::EdgeTts],
            speech_languages: SpeechLanguages {
                edge_tts: locale.to_string(),
                ..SpeechLanguages::default()
            },
            speech_voices: crate::state::SpeechVoices {
                edge_tts: voice.map(str::to_string),
                ..Default::default()
            },
            edge_tts_voices: vec![
                edge_voice("en-US-JennyNeural", "en-US", 0),
                edge_voice("tr-TR-AhmetNeural", "tr-TR", 1),
                edge_voice("tr-TR-EmelNeural", "tr-TR", 2),
            ],
            ..AppState::default()
        }
    }

    /// The matrix's Speak row: `tr-TR` with no voice set speaks the
    /// language's first listed voice, with no clip, and plays the buffer.
    #[test]
    fn edge_tts_speaks_the_languages_first_voice_with_no_sample_or_key() {
        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();

        speak("Merhaba", &edge_state("tr-TR", None), &tts, &mic, &notifier).unwrap();
        speak(
            "Merhaba",
            &edge_state("tr-tr", Some("tr-TR-EmelNeural")),
            &tts,
            &mic,
            &notifier,
        )
        .unwrap();

        assert_eq!(tts.calls.lock().unwrap()[0].2, "tr-TR");
        assert_eq!(
            *tts.voices.lock().unwrap(),
            vec![
                (None, Some("tr-TR-AhmetNeural".to_string())),
                (None, Some("tr-TR-EmelNeural".to_string())),
            ]
        );
        assert_eq!(mic.played().len(), 2);
        assert!(notifier.summaries().is_empty());
    }

    /// The No disclosure row: refused before anything is spawned, with one
    /// notification.
    #[test]
    fn edge_tts_is_refused_before_generate_until_its_disclosure_is_confirmed() {
        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();
        let state = AppState {
            confirmed_disclosures: vec![RemoteProvider::Azure],
            ..edge_state("tr-TR", None)
        };

        let error = speak("Merhaba", &state, &tts, &mic, &notifier).unwrap_err();

        assert!(
            matches!(error, VoiceMeError::DisclosureNotConfirmed(ref name) if name == "Edge TTS")
        );
        assert!(tts.calls.lock().unwrap().is_empty());
        assert_eq!(notifier.summaries(), vec![GENERATION_FAILED_SUMMARY]);
    }

    /// An unlisted language or voice is refused by name, never
    /// substituted; so is speaking before any voice was listed.
    #[test]
    fn an_unlisted_edge_tts_language_or_voice_is_refused_by_name() {
        for (state, needle) in [
            (edge_state("xx-XX", None), "\"xx-XX\""),
            (
                edge_state("tr-TR", Some("tr-TR-GoneNeural")),
                "\"tr-TR-GoneNeural\"",
            ),
            (
                edge_state("en-US", Some("tr-TR-EmelNeural")),
                "\"tr-TR-EmelNeural\"",
            ),
            (
                AppState {
                    edge_tts_voices: Vec::new(),
                    ..edge_state("tr-TR", None)
                },
                "no voices listed",
            ),
        ] {
            let tts = FakeTts::default();
            let notifier = FakeNotifier::default();
            let mic = FakeMic::default();

            let error = speak("Merhaba", &state, &tts, &mic, &notifier).unwrap_err();

            assert!(tts.calls.lock().unwrap().is_empty());
            let message = error.to_string();
            assert!(
                message.contains("Edge TTS") && message.contains(needle),
                "{message}"
            );
            assert_eq!(notifier.summaries(), vec![GENERATION_FAILED_SUMMARY]);
        }
    }

    fn piper_voice(id: &str, locale: &str) -> crate::state::StockVoice {
        crate::state::StockVoice {
            id: id.to_string(),
            language: locale.to_string(),
            language_label: "Turkish".to_string(),
            name: id.to_string(),
            priority: 0,
        }
    }

    /// Piper with the default language and voice, fahrettin (and dfki)
    /// installed — and no Reference Voice Sample.
    fn piper_state(voice: Option<&str>) -> AppState {
        AppState {
            backend_selection: BackendSelection::PIPER_CPU,
            speech_voices: crate::state::SpeechVoices {
                piper: voice.map(str::to_string),
                ..Default::default()
            },
            piper_voices: vec![
                piper_voice("tr_TR-dfki-medium", "tr_TR"),
                piper_voice("tr_TR-fahrettin-medium", "tr_TR"),
            ],
            ..AppState::default()
        }
    }

    /// The matrix's Speak row at core level: no sample, no disclosure, the
    /// installed voice and no clip reach the engine, and the buffer plays.
    #[test]
    fn piper_speaks_the_stored_voice_with_no_sample() {
        let tts = FakeTts::cold();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();

        speak(
            "Merhaba Erdem, bu yerel ve anında.",
            &piper_state(Some("tr_TR-fahrettin-medium")),
            &tts,
            &mic,
            &notifier,
        )
        .unwrap();

        assert_eq!(tts.calls.lock().unwrap()[0].2, "tr_TR");
        assert_eq!(
            tts.voices.lock().unwrap()[0],
            (None, Some("tr_TR-fahrettin-medium".to_string()))
        );
        assert_eq!(mic.played().len(), 1);
        assert!(
            notifier.summaries().is_empty(),
            "a Piper voice loads in about a second: no still-working notice"
        );
    }

    /// "Use" on another voice: the next line speaks in it. A voice that is
    /// not installed (deleted) is refused by name, before the engine.
    #[test]
    fn piper_uses_the_chosen_voice_and_refuses_one_not_installed() {
        let tts = FakeTts::default();
        speak(
            "Bir",
            &piper_state(Some("tr_TR-dfki-medium")),
            &tts,
            &FakeMic::default(),
            &FakeNotifier::default(),
        )
        .unwrap();
        assert_eq!(
            tts.voices.lock().unwrap()[0].1.as_deref(),
            Some("tr_TR-dfki-medium")
        );

        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let state = AppState {
            piper_voices: vec![piper_voice("tr_TR-dfki-medium", "tr_TR")],
            ..piper_state(Some("tr_TR-fahrettin-medium"))
        };
        let error = speak("Bir", &state, &tts, &FakeMic::default(), &notifier).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("Piper") && message.contains("tr_TR-fahrettin-medium"),
            "{message}"
        );
        assert!(tts.calls.lock().unwrap().is_empty());
        assert_eq!(notifier.summaries(), vec![GENERATION_FAILED_SUMMARY]);

        // Nothing installed at all.
        let error = speak(
            "Bir",
            &AppState {
                piper_voices: Vec::new(),
                ..piper_state(None)
            },
            &FakeTts::default(),
            &FakeMic::default(),
            &FakeNotifier::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("Piper"), "{error}");
    }

    /// A Piper failure is one notification naming Piper and the reason.
    #[test]
    fn a_piper_failure_is_one_notification_naming_piper() {
        let tts = FakeTts::failing(VoiceMeError::SpeechEngine(
            "Piper could not phonemize the text: espeak-ng could not be started".to_string(),
        ));
        let notifier = FakeNotifier::default();
        speak(
            "Merhaba",
            &piper_state(None),
            &tts,
            &FakeMic::default(),
            &notifier,
        )
        .unwrap_err();
        assert_eq!(notifier.summaries(), vec![GENERATION_FAILED_SUMMARY]);
        assert!(notifier.bodies()[0].contains("Piper"));
    }
}
