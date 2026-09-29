//! The real Turkish G2P model: an ONNX Runtime library and DizgeBERT's
//! files. Skips itself (passing) unless both are there:
//!
//! * `ORT_DYLIB_PATH` naming an ONNX Runtime shared library;
//! * `VOICE_ME_TR_G2P_DIR` naming a directory holding `model.onnx` and
//!   `vocab.json` (`piper-voices/g2p/tr-dizge` in this repository).
//!
//! With `VOICE_ME_PIPER_TEST_VOICE` also set (a Turkish voice directory),
//! a line is spoken through it too.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use voice_me_core::{SAMPLE_RATE, TtsPort as _, VoiceMeError, assets};
use voice_me_tts_piper::{DizgeG2p, Phonemizer, PiperTts, RuntimeInit, TurkishG2pPhonemizer};

/// Never asked: every line here is Turkish.
struct NoOther;

impl Phonemizer for NoOther {
    fn phonemize(&self, voice: &str, _: &str) -> Result<String, String> {
        Err(format!("{voice} is not Turkish"))
    }
}

#[test]
fn turkish_text_is_phonemized_by_the_model() {
    let (Some(dylib), Some(model_dir)) = (
        std::env::var_os("ORT_DYLIB_PATH").filter(|value| !value.is_empty()),
        std::env::var_os("VOICE_ME_TR_G2P_DIR").filter(|value| !value.is_empty()),
    ) else {
        eprintln!("skipped: needs ORT_DYLIB_PATH and VOICE_ME_TR_G2P_DIR");
        return;
    };

    // The model is linked into a cache root the way deps lays it out.
    let model_dir = PathBuf::from(model_dir);
    let root = tempfile::tempdir().unwrap();
    let files = assets::turkish_g2p_files(root.path());
    std::fs::create_dir_all(&files.dir).unwrap();
    for file in [
        assets::TURKISH_G2P_MODEL_FILE,
        assets::TURKISH_G2P_VOCAB_FILE,
    ] {
        std::fs::copy(model_dir.join(file), files.dir.join(file)).unwrap();
    }

    let dylib = PathBuf::from(dylib);
    let runtime_init: RuntimeInit = Arc::new(move || {
        ort::init_from(&dylib)
            .map_err(|error| VoiceMeError::SpeechEngine(error.to_string()))?
            .commit();
        Ok(())
    });
    let phonemizer = Arc::new(TurkishG2pPhonemizer::new(
        DizgeG2p::new(root.path(), runtime_init.clone()),
        Arc::new(NoOther),
    ));

    let started = Instant::now();
    let ipa = phonemizer
        .phonemize(
            "tr",
            "Merhaba Erdem, güneş ve çikolata 12 kahvaltı olduğunu",
        )
        .unwrap();
    eprintln!("{ipa} ({:?})", started.elapsed());
    let words: Vec<&str> = ipa.split(' ').collect();
    assert_eq!(words.len(), 9, "{ipa}");
    assert_eq!(words[2], "ɟynˈɛʃ");
    assert_eq!(words[3], "vɛ");
    assert_eq!(words[5], "ˈɔn", "12 is spelled out");
    // The model mislabels "olduğunu"; it is spelled by rule.
    assert_eq!(words[8], "oɫduːunˈu");
    assert!(phonemizer.phonemize("en-us", "hello").is_err());

    let started = Instant::now();
    phonemizer
        .phonemize("tr", "Bu yerel ve anında çalışan bir ses.")
        .unwrap();
    assert!(
        started.elapsed().as_millis() < 500,
        "a loaded model phonemizes a clause quickly: {:?}",
        started.elapsed()
    );

    let Some(voice) = std::env::var_os("VOICE_ME_PIPER_TEST_VOICE").filter(|v| !v.is_empty())
    else {
        return;
    };
    let voice = PathBuf::from(voice);
    let key = "test-voice";
    let voice_files = assets::piper_voice_files(root.path(), key).unwrap();
    std::fs::create_dir_all(&voice_files.dir).unwrap();
    std::fs::copy(voice.join("model.onnx"), &voice_files.model).unwrap();
    std::fs::copy(voice.join("config.json"), &voice_files.config).unwrap();
    let tts = PiperTts::new(
        root.path().to_path_buf(),
        Some(key.to_string()),
        phonemizer,
        runtime_init,
    );
    let audio = tts
        .generate(
            "Merhaba Erdem, bu yerel ve anında.",
            None,
            "tr_TR",
            Some(key),
        )
        .unwrap();
    let seconds = audio.len() as f32 / SAMPLE_RATE as f32;
    assert!((1.0..5.0).contains(&seconds), "{seconds} s");
    if let Some(out) = std::env::var_os("VOICE_ME_TR_G2P_WAV").filter(|v| !v.is_empty()) {
        write_wav(&PathBuf::from(out), audio.samples());
    }
}

/// A 16-bit mono WAV at voice-me's rate, to listen to.
fn write_wav(path: &std::path::Path, samples: &[f32]) {
    let data: Vec<u8> = samples
        .iter()
        .flat_map(|sample| ((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
        .collect();
    let mut wav = Vec::with_capacity(44 + data.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    std::fs::write(path, wav).unwrap();
}
