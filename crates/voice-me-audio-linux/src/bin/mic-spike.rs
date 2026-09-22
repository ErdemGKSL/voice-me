//! Manual verification for spec-2-7: installs the Virtual Microphone, plays
//! a buffer into it, and holds it there long enough to capture from.
//!
//! A binary rather than an example so the crate's own end-to-end test can
//! drive it through `CARGO_BIN_EXE_mic-spike` — and because a playback
//! stream from the cargo *test* binary is routed to the default sink rather
//! than to the device, for reasons spec-2-7 could not pin down. The same
//! code from an ordinary binary is routed correctly, every time.
//!
//! Run with:
//!   `cargo run -p voice-me-audio-linux --bin mic-spike`               (a 440 Hz tone)
//!   `cargo run -p voice-me-audio-linux --bin mic-spike -- --play-only`
//!   `cargo run -p voice-me-audio-linux --bin mic-spike -- --uninstall`
//!
//! While it runs, another application selecting "voice-me" as its
//! microphone receives the audio. To prove it without one:
//!
//!   pactl list short sources | grep voice-me
//!   pw-record --target=<id> /tmp/cap.wav   # then check the peak is not 0

// The library body is `#![cfg(target_os = "linux")]`, so on any other
// target it compiles to an empty crate and these imports would not resolve
// — and `cargo build --workspace` on the Windows CI runner builds this bin.

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("the Virtual Microphone spike is Linux-only");
}

#[cfg(target_os = "linux")]
use std::f32::consts::TAU;

#[cfg(target_os = "linux")]
use voice_me_audio_linux::{DEVICE_NAME, LinuxVirtualMicAdapter};
#[cfg(target_os = "linux")]
use voice_me_core::{AudioBuffer, SAMPLE_RATE, VirtualMicPort};

/// Long enough to start a capture client by hand after seeing the device.
#[cfg(target_os = "linux")]
const TONE_SECONDS: u32 = 3;
#[cfg(target_os = "linux")]
const TONE_HZ: f32 = 440.0;

#[cfg(target_os = "linux")]
fn main() {
    let argument = std::env::args().nth(1);
    match argument.as_deref() {
        None | Some("--play-only") | Some("--uninstall") => {}
        Some(other) => {
            // A mistyped `--uninstall` that silently installed and played a
            // tone instead would be a confusing way to find out.
            eprintln!("unknown argument `{other}`; expected --play-only or --uninstall");
            std::process::exit(2);
        }
    }

    let adapter = match LinuxVirtualMicAdapter::new() {
        Ok(adapter) => adapter,
        Err(error) => {
            eprintln!("could not resolve the config directory: {error}");
            std::process::exit(1);
        }
    };

    if argument.as_deref() == Some("--uninstall") {
        match adapter.uninstall() {
            Ok(()) => println!("Virtual Microphone removed."),
            Err(error) => {
                eprintln!("uninstall failed: {error}");
                std::process::exit(1);
            }
        }
        return;
    }

    // `--play-only` exists for the crate's own end-to-end test: installing
    // there would both write the developer's real config and tear down the
    // device the test's capture client is already attached to.
    if argument.as_deref() != Some("--play-only") {
        match adapter.install() {
            Ok(conf) => println!("installed; drop-in at {}", conf.display()),
            Err(error) => {
                eprintln!("install failed: {error}");
                std::process::exit(1);
            }
        }
    }

    match adapter.is_present() {
        Ok(true) => println!("`{DEVICE_NAME}` is present as a capture source."),
        Ok(false) => eprintln!("warning: `{DEVICE_NAME}` is not listed as a source"),
        Err(error) => eprintln!("warning: could not check for the device: {error}"),
    }

    let audio = tone();
    println!(
        "playing {:.2} s into `{DEVICE_NAME}` — start your capture now…",
        audio.duration().as_secs_f32()
    );

    match adapter.play(&audio) {
        Ok(()) => println!("done; the buffer was drained by the audio server."),
        Err(error) => {
            eprintln!("playback failed: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(target_os = "linux")]
fn tone() -> AudioBuffer {
    let total = (SAMPLE_RATE * TONE_SECONDS) as usize;
    AudioBuffer::new(
        (0..total)
            .map(|i| 0.5 * (TAU * TONE_HZ * i as f32 / SAMPLE_RATE as f32).sin())
            .collect(),
    )
}
