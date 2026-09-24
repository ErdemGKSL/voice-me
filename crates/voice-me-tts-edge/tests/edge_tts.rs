//! Against the real `edge-tts`, when this machine has it. Skips itself
//! (passing) when it does not, so it needs no `#[ignore]`. Synthesis needs
//! Microsoft's unofficial service, so it runs only with
//! `VOICE_ME_EDGE_TTS_ONLINE=1`: CI never depends on it.

// The library is Linux-only, so on any other target this compiles to
// nothing.
#![cfg(target_os = "linux")]

use voice_me_core::{SAMPLE_RATE, TtsPort as _};
use voice_me_tts_edge::{EdgeTts, find_program, list_voices};

#[test]
fn the_real_program_lists_the_turkish_voices() {
    if find_program().is_none() {
        eprintln!("skipped: edge-tts is not on PATH or in ~/.local/bin");
        return;
    }

    let voices = list_voices().unwrap();
    let emel = voices
        .iter()
        .find(|voice| voice.id == "tr-TR-EmelNeural")
        .expect("edge-tts lists tr-TR-EmelNeural");
    assert_eq!(emel.language, "tr-TR");
    assert_eq!(emel.name, "Emel (Female)");
}

#[test]
fn merhaba_is_spoken_at_24_khz_when_online() {
    if find_program().is_none() || std::env::var("VOICE_ME_EDGE_TTS_ONLINE").as_deref() != Ok("1") {
        eprintln!("skipped: needs edge-tts and VOICE_ME_EDGE_TTS_ONLINE=1");
        return;
    }

    let audio = EdgeTts::new()
        .generate("merhaba", None, "tr-TR", Some("tr-TR-EmelNeural"))
        .unwrap();
    let seconds = audio.len() as f64 / f64::from(SAMPLE_RATE);
    assert!(
        (0.2..5.0).contains(&seconds),
        "{seconds} s of audio for one word"
    );
    assert!(audio.samples().iter().any(|sample| sample.abs() > 0.01));
}
