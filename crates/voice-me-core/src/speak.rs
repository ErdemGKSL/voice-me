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
use crate::state::AppState;

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

/// The speech languages v1 generates in (AD-12): the ones that need no
/// Python-only text normalization. Chinese, Japanese, Hebrew and Korean are
/// out until that normalization has a Rust path.
const SUPPORTED_SPEECH_LANGUAGES: [&str; 2] = ["tr", "en"];

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

    // Decision 2 makes hand-editing `settings.toml` the only way to set the
    // speech language until Epic 4 builds the selector, so a typo is the
    // expected failure mode rather than a remote one. Unchecked, it reaches
    // the model as a literal `[turkish]` tag and comes back as plausible
    // audio in the wrong language — a failure nobody can diagnose from the
    // result. Checked here, it is a sentence naming the value and the two
    // languages v1 supports (AD-12).
    let language = state.speech_language.trim().to_lowercase();
    if !SUPPORTED_SPEECH_LANGUAGES.contains(&language.as_str()) {
        return Err(VoiceMeError::Other(format!(
            "unsupported speech language {:?} in settings.toml — voice-me speaks {}",
            state.speech_language,
            SUPPORTED_SPEECH_LANGUAGES.join(" and ")
        )));
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

    let audio = tts.generate(text, reference_clip, &language)?;

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
        fail_with: Mutex<Option<VoiceMeError>>,
    }

    impl Default for FakeTts {
        fn default() -> Self {
            Self {
                ready: AtomicBool::new(true),
                calls: Mutex::new(Vec::new()),
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
            reference_clip: &Path,
            language: &str,
        ) -> Result<AudioBuffer, VoiceMeError> {
            self.calls.lock().unwrap().push((
                text.to_string(),
                reference_clip.to_path_buf(),
                language.to_string(),
            ));
            if let Some(error) = self.fail_with.lock().unwrap().take() {
                return Err(error);
            }
            Ok(AudioBuffer::new(vec![0.1; 24]))
        }
    }

    fn state_with_a_sample() -> AppState {
        AppState {
            reference_voice_sample: Some(PathBuf::from("/data/reference_voice_sample.wav")),
            speech_language: "tr".to_string(),
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
            speech_language: "tr".to_string(),
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

    #[test]
    fn an_unsupported_speech_language_is_refused_by_name_before_the_engine() {
        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();
        let state = AppState {
            speech_language: "turkish".to_string(),
            ..state_with_a_sample()
        };

        let error = speak("Merhaba", &state, &tts, &mic, &notifier).unwrap_err();

        assert!(
            tts.calls.lock().unwrap().is_empty(),
            "a typo must not reach the model as a literal [turkish] tag"
        );
        let message = error.to_string();
        assert!(
            message.contains("turkish") && message.contains("tr") && message.contains("en"),
            "the message has to name the bad value and the supported ones: {message}"
        );
        assert_eq!(notifier.summaries(), vec![GENERATION_FAILED_SUMMARY]);
    }

    #[test]
    fn the_speech_language_is_normalised_before_it_becomes_a_tag() {
        let tts = FakeTts::default();
        let notifier = FakeNotifier::default();
        let mic = FakeMic::default();
        let state = AppState {
            speech_language: "  EN \n".to_string(),
            ..state_with_a_sample()
        };

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
                _reference_clip: &Path,
                _language: &str,
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
}
