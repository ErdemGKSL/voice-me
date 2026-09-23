//! Against the real `espeak-ng`, when this machine has it. Skips itself
//! (passing) when it does not, so it needs no `#[ignore]`.

use voice_me_core::{SAMPLE_RATE, TtsPort as _};
use voice_me_tts_system_linux::{SystemVoiceLinux, find_program, list_voices};

#[test]
fn merhaba_is_spoken_at_24_khz_in_the_turkish_voice() {
    if find_program().is_none() {
        eprintln!("skipped: espeak-ng is not on PATH");
        return;
    }

    let voices = list_voices().unwrap();
    let turkish = voices
        .iter()
        .find(|voice| voice.language == "tr")
        .expect("eSpeak NG lists a Turkish voice");
    assert_eq!(turkish.id, "trk/tr");

    let started = std::time::Instant::now();
    let audio = SystemVoiceLinux::new()
        .generate("merhaba", None, "tr", Some(&turkish.id))
        .unwrap();
    let seconds = audio.len() as f64 / f64::from(SAMPLE_RATE);
    assert!(
        (0.2..5.0).contains(&seconds),
        "{seconds} s of audio for one word"
    );
    assert!(audio.samples().iter().any(|sample| sample.abs() > 0.01));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "took {:?}",
        started.elapsed()
    );
}
