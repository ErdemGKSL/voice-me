//! The Reference Voice Sample is whatever the user recorded or picked, so
//! the decoder has to cover more than wav. The wav path is covered by a
//! hand-written fixture in `reference.rs`'s own tests; mp3, flac and ogg
//! need a real encoder to produce, which is not something a test run should
//! depend on.
//!
//! So this test is opt-in: point `VOICE_ME_TEST_CLIPS` at a directory
//! holding `ref.mp3`, `ref.flac` and `ref.ogg` — all three re-encodes of one
//! source clip — and it asserts they all arrive as the same 24 kHz mono
//! audio. With the variable unset it does nothing, which is what keeps
//! `cargo test --workspace` green with no fixtures and no network (AD-8).
//!
//! ```bash
//! src=~/.local/share/voice-me/reference_voice_sample.wav
//! for ext in mp3 flac ogg; do ffmpeg -i "$src" /tmp/clips/ref.$ext; done
//! VOICE_ME_TEST_CLIPS=/tmp/clips cargo test -p voice-me-tts --test reference_containers
//! ```

use std::path::PathBuf;

use voice_me_core::SAMPLE_RATE;
use voice_me_tts::reference::load_reference_clip;

#[test]
fn every_supported_container_decodes_to_the_same_twenty_four_kilohertz_mono_audio() {
    let Some(dir) = std::env::var_os("VOICE_ME_TEST_CLIPS").map(PathBuf::from) else {
        eprintln!("VOICE_ME_TEST_CLIPS is unset — see this file's header");
        return;
    };

    let mut durations = Vec::new();
    for extension in ["mp3", "flac", "ogg"] {
        let path = dir.join(format!("ref.{extension}"));
        let buffer = load_reference_clip(&path)
            .unwrap_or_else(|error| panic!("{} did not decode: {error}", path.display()));

        assert_eq!(buffer.sample_rate(), SAMPLE_RATE);
        assert!(!buffer.is_empty(), "{extension} decoded to nothing");
        durations.push((extension, buffer.duration().as_secs_f64()));
    }

    // Lossy re-encodes pad and trim differently, so they are not
    // sample-identical — but a decoder that mishandled the channel count or
    // the rate would be out by a factor, not by a fraction of a second.
    let (_, first) = durations[0];
    for (extension, duration) in &durations {
        assert!(
            (duration - first).abs() < 0.5,
            "{extension} decoded to {duration:.2} s against {first:.2} s for \
             {}, which is a rate or channel-count error rather than an \
             encoder boundary",
            durations[0].0
        );
    }
}
