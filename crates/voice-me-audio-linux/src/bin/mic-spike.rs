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
//!   `cargo run -p voice-me-audio-linux --bin mic-spike -- --install-only`
//!   `cargo run -p voice-me-audio-linux --bin mic-spike -- --play-only`
//!   `cargo run -p voice-me-audio-linux --bin mic-spike -- --speak`
//!   `cargo run -p voice-me-audio-linux --bin mic-spike -- --uninstall`
//!
//! `--speak` runs the whole Speak Action (spec-2-9) rather than calling
//! `play` directly: `voice_me_core::speak` over a stub `TtsPort` that hands
//! back an utterance-shaped buffer instead of loading 1.56 GB of ONNX. It is
//! what proves the *path* — generation port to virtual-mic port to a capture
//! client — where the tone only ever proved the device.
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
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
use voice_me_audio_linux::{DEVICE_NAME, LinuxVirtualMicAdapter};
#[cfg(target_os = "linux")]
use voice_me_core::{
    AppState, AudioBuffer, NotificationPort, SAMPLE_RATE, TtsPort, VirtualMicPort, VoiceMeError,
};

/// Long enough to start a capture client by hand after seeing the device.
#[cfg(target_os = "linux")]
const TONE_SECONDS: u32 = 3;
#[cfg(target_os = "linux")]
const TONE_HZ: f32 = 440.0;

#[cfg(target_os = "linux")]
fn main() {
    let argument = std::env::args().nth(1);
    match argument.as_deref() {
        None
        | Some("--install-only")
        | Some("--play-only")
        | Some("--speak")
        | Some("--uninstall") => {}
        Some(other) => {
            // A mistyped `--uninstall` that silently installed and played a
            // tone instead would be a confusing way to find out.
            eprintln!(
                "unknown argument `{other}`; expected --install-only, --play-only, --speak or --uninstall"
            );
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

    // Everything but `--play-only` installs first. The playing process has
    // to be the one that loaded the module: a device installed by a
    // *different* process is one this one cannot address by name — playing
    // at it lands on the default sink instead, which is the same
    // unexplained asymmetry that made `mic-spike` a bin rather than an
    // example. `--install-only` therefore only stands the device up so a
    // capture client has something to attach to; the playing child then
    // reinstalls, and pipewire-pulse moves the recorder to the new node.
    if argument.as_deref() != Some("--play-only") {
        match adapter.install() {
            Ok(conf) => println!("installed; drop-in at {}", conf.display()),
            Err(error) => {
                eprintln!("install failed: {error}");
                std::process::exit(1);
            }
        }
    }

    if argument.as_deref() == Some("--install-only") {
        // The device is up; a capture client can attach now and the playing
        // child follows. Playing a tone here would only be heard by nobody.
        return;
    }

    match adapter.is_present() {
        Ok(true) => println!("`{DEVICE_NAME}` is present as a capture source."),
        Ok(false) => eprintln!("warning: `{DEVICE_NAME}` is not listed as a source"),
        Err(error) => eprintln!("warning: could not check for the device: {error}"),
    }

    if argument.as_deref() == Some("--speak") {
        println!("running the whole Speak Action into `{DEVICE_NAME}` — start your capture now…");
        let state = AppState {
            // The stub never opens it; `speak` only checks that one is set.
            reference_voice_sample: Some(PathBuf::from("/nonexistent/reference.wav")),
            speech_language: "tr".to_string(),
            ..AppState::default()
        };
        match voice_me_core::speak("Merhaba", &state, &StubTts, &adapter, &StderrNotifier) {
            Ok(audio) => println!(
                "done; {:.2} s spoken and drained by the audio server.",
                audio.duration().as_secs_f32()
            ),
            Err(error) => {
                eprintln!("the Speak Action failed: {error}");
                std::process::exit(1);
            }
        }
        return;
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

/// A `TtsPort` that produces an utterance-shaped buffer without a model.
///
/// The point of `--speak` is the *wiring* — that `speak` hands what
/// `generate` returned straight to `VirtualMicPort::play` and that it arrives
/// at a capture client. Running the real engine here would add 1.56 GB of
/// model files and ~90 s of session build to a check about routing, and
/// would make the test unrunnable on any machine without a provisioned cache.
#[cfg(target_os = "linux")]
struct StubTts;

#[cfg(target_os = "linux")]
impl TtsPort for StubTts {
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
        Ok(utterance_shaped())
    }
}

/// `speak` notifies on failure; here that is a line on stderr, so a failing
/// run says why in the child's output rather than through D-Bus.
#[cfg(target_os = "linux")]
struct StderrNotifier;

#[cfg(target_os = "linux")]
impl NotificationPort for StderrNotifier {
    fn notify(&self, summary: &str, body: &str) -> Result<(), VoiceMeError> {
        eprintln!("[notification] {summary} — {body}");
        Ok(())
    }
}

/// Shaped like a generated utterance rather than like a test signal: a few
/// harmonics under an attack/decay envelope, with the near-silent head and
/// tail every generation has. A pure tone at full amplitude is the one
/// signal that would survive any amount of mishandling on the way out.
#[cfg(target_os = "linux")]
fn utterance_shaped() -> AudioBuffer {
    let total = (SAMPLE_RATE * TONE_SECONDS) as usize;
    AudioBuffer::new(
        (0..total)
            .map(|i| {
                let t = i as f32 / SAMPLE_RATE as f32;
                let progress = i as f32 / total as f32;
                // Fade in over the first 10%, out over the last 20%.
                let envelope = (progress / 0.1).min(1.0) * ((1.0 - progress) / 0.2).min(1.0);
                let f0 = 120.0 + 8.0 * (TAU * 4.0 * t).sin();
                let voiced = (TAU * f0 * t).sin()
                    + 0.5 * (TAU * 2.0 * f0 * t).sin()
                    + 0.25 * (TAU * 3.0 * f0 * t).sin();
                0.3 * envelope * voiced
            })
            .collect(),
    )
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
