//! The real engine: `espeak-ng`, an ONNX Runtime library and a Piper voice.
//! Skips itself (passing) unless all three are there:
//!
//! * `espeak-ng` on PATH;
//! * `ORT_DYLIB_PATH` naming an ONNX Runtime shared library;
//! * `VOICE_ME_PIPER_TEST_VOICE` naming a voice directory holding
//!   `model.onnx` and `config.json` (fahrettin, say).

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use voice_me_core::{SAMPLE_RATE, TtsPort as _, VoiceMeError, assets};
use voice_me_tts_piper::{EspeakPhonemizer, PiperTts};

#[test]
fn a_turkish_line_is_spoken_at_24_khz_in_under_a_second() {
    let (Some(_), Some(dylib), Some(voice)) = (
        voice_me_espeak::find_program(),
        std::env::var_os("ORT_DYLIB_PATH").filter(|value| !value.is_empty()),
        std::env::var_os("VOICE_ME_PIPER_TEST_VOICE").filter(|value| !value.is_empty()),
    ) else {
        eprintln!("skipped: needs espeak-ng, ORT_DYLIB_PATH and VOICE_ME_PIPER_TEST_VOICE");
        return;
    };

    // The voice directory is linked into a cache root the way deps lays it
    // out: `<root>/piper/<key>/`.
    let voice = PathBuf::from(voice);
    let key = "test-voice";
    let root = tempfile::tempdir().unwrap();
    let files = assets::piper_voice_files(root.path(), key).unwrap();
    std::fs::create_dir_all(&files.dir).unwrap();
    std::os::unix::fs::symlink(voice.join("model.onnx"), &files.model).unwrap();
    std::os::unix::fs::symlink(voice.join("config.json"), &files.config).unwrap();

    let dylib = PathBuf::from(dylib);
    let tts = PiperTts::new(
        root.path().to_path_buf(),
        Some(key.to_string()),
        Arc::new(EspeakPhonemizer::new()),
        Arc::new(move || {
            ort::init_from(&dylib)
                .map_err(|error| VoiceMeError::SpeechEngine(error.to_string()))?
                .commit();
            Ok(())
        }),
    );

    // Warm-up builds the session; it needs no Reference Voice Sample.
    tts.warm_up().unwrap();
    assert!(tts.is_ready());

    let started = Instant::now();
    let audio = tts
        .generate(
            "Merhaba Erdem, bu yerel ve anında.",
            None,
            "tr_TR",
            Some(key),
        )
        .unwrap();
    let took = started.elapsed();
    let seconds = audio.len() as f64 / f64::from(SAMPLE_RATE);
    eprintln!("{seconds:.2} s of audio in {took:?}");
    assert!((1.5..4.5).contains(&seconds), "{seconds} s of audio");
    assert!(audio.samples().iter().any(|sample| sample.abs() > 0.5));
    assert!(audio.samples().iter().all(|sample| sample.abs() <= 1.0));
    assert!(took < Duration::from_secs(1), "took {took:?}");
}
