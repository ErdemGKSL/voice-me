//! Against the real Windows speech engine, when this machine has a voice.
//! Skips itself (passing) when it has none, so it needs no `#[ignore]`.

// The WinRT module is Windows-only, so on any other target this compiles
// to nothing.
#![cfg(target_os = "windows")]

use voice_me_core::{SAMPLE_RATE, TtsPort as _};
use voice_me_tts_system_windows::{SystemVoiceWindows, count_voices, list_voices};

#[test]
fn hello_is_spoken_at_24_khz_in_the_first_installed_voice() {
    match count_voices() {
        Err(reason) => {
            eprintln!("skipped: Windows speech could not be asked for its voices: {reason}");
            return;
        }
        Ok(0) => {
            eprintln!("skipped: Windows speech lists no installed voices");
            return;
        }
        Ok(_) => {}
    }

    let voices = list_voices().unwrap();
    let voice = &voices[0];

    let audio = SystemVoiceWindows::new()
        .generate("hello", None, &voice.language, Some(&voice.id))
        .unwrap();
    assert_eq!(audio.sample_rate(), SAMPLE_RATE);
    assert_eq!(SAMPLE_RATE, 24_000);
    let seconds = audio.len() as f64 / f64::from(SAMPLE_RATE);
    assert!(
        (0.2..5.0).contains(&seconds),
        "{seconds} s of audio for one word in {} ({})",
        voice.name,
        voice.language
    );
    assert!(
        audio.samples().iter().any(|sample| sample.abs() > 0.01),
        "silent audio from {}",
        voice.name
    );
}
