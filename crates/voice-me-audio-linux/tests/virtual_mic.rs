//! End-to-end coverage of the Virtual Microphone against a real audio
//! server — the matrix rows that cannot be checked without one.
//!
//! All `#[ignore]`d: CI has no audio server. Run them on a desktop with
//!
//! `cargo test -p voice-me-audio-linux --test virtual_mic -- --ignored --test-threads=1`
//!
//! Single-threaded because they all install and remove the one device that
//! carries this name.

#![cfg(target_os = "linux")]

use voice_me_audio_linux::{DEVICE_NAME, LinuxVirtualMicAdapter};
use voice_me_core::{AudioBuffer, SAMPLE_RATE, VirtualMicPort, VoiceMeError};

fn tone(seconds: u32) -> AudioBuffer {
    let total = (SAMPLE_RATE * seconds) as usize;
    AudioBuffer::new(
        (0..total)
            .map(|i| 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / SAMPLE_RATE as f32).sin())
            .collect(),
    )
}

/// Install against a throwaway config directory, so no test writes to the
/// developer's own PipeWire config file.
///
/// The *device* cannot be isolated that way — there is one audio server and
/// one name — so these tests do take down and rebuild a `voice-me` device
/// the developer may have installed. Running `mic-spike` afterwards puts it
/// back.
fn fixture() -> (tempfile::TempDir, LinuxVirtualMicAdapter) {
    let temp = tempfile::tempdir().expect("temp dir");
    let adapter = LinuxVirtualMicAdapter::with_config_dir(temp.path().to_path_buf());
    adapter.install().expect("install");
    (temp, adapter)
}

/// Run `mic-spike` with `args` while an independent `parecord` client is
/// attached to the Virtual Microphone, and return the child's result
/// alongside what that client heard.
///
/// Nothing is asserted here: the caller has a device to uninstall first, and
/// a `mic-spike` that failed — precisely the run being debugged — must not
/// leave a null-sink module loaded on the developer's machine because a
/// helper panicked on the way out.
///
/// Both halves are separate processes on purpose — `parecord` captures
/// exactly as Discord would, and the playing side is the `mic-spike` binary
/// rather than this test, because a playback stream opened from the cargo
/// test binary is routed to the default sink despite carrying the right
/// `target.object` (see `src/bin/mic-spike.rs`).
fn capture_while_mic_spike_runs(
    temp: &tempfile::TempDir,
    args: &[&str],
) -> (std::process::Output, Vec<u8>) {
    let capture = temp.path().join("capture.raw");

    // The device has to exist *before* `parecord` is told to attach to it:
    // a recorder pointed at a name that is not there yet captures nothing
    // and reports no error, which reads exactly like a routing failure —
    // and with each of these tests uninstalling on its way out, the second
    // one to run would always find nothing there. The playing child below
    // still installs for itself, because only the process that loaded the
    // module can address it by name; pipewire-pulse moves the attached
    // recorder onto the replacement node.
    let installed = std::process::Command::new(env!("CARGO_BIN_EXE_mic-spike"))
        .arg("--install-only")
        .env("XDG_CONFIG_HOME", temp.path())
        .output()
        .expect("run mic-spike --install-only");
    assert!(
        installed.status.success(),
        "mic-spike --install-only failed: {installed:?}"
    );

    let mut recorder = std::process::Command::new("parecord")
        .args([
            "--device",
            DEVICE_NAME,
            "--rate=24000",
            "--channels=1",
            "--format=float32le",
            "--raw",
        ])
        .stdout(std::fs::File::create(&capture).expect("capture file"))
        .spawn()
        .expect("parecord — install pulseaudio-utils to run this test");
    // Let the recording client attach before the audio starts; one that
    // joins late hears only the tail.
    std::thread::sleep(std::time::Duration::from_millis(700));

    // `XDG_CONFIG_HOME` points the drop-in at the temp directory, so the
    // developer's real config is never touched by either child.
    let played = std::process::Command::new(env!("CARGO_BIN_EXE_mic-spike"))
        .args(args)
        .env("XDG_CONFIG_HOME", temp.path())
        .output();

    recorder.kill().expect("stop parecord");
    recorder.wait().expect("reap parecord");

    (
        played.expect("run mic-spike"),
        std::fs::read(&capture).expect("capture file"),
    )
}

/// The loudest sample in a raw `float32le` capture.
///
/// Asserts first that the capture holds at least a second of audio: a
/// `parecord` that never attached yields an empty file whose peak folds to
/// `0.0`, indistinguishable from a device that received silence. Failing on
/// the length here is what keeps "the recorder was broken" from being
/// reported as "the audio was routed to the speakers" and sending the reader
/// hunting in the wrong place.
fn peak_of(captured: &[u8]) -> f32 {
    assert!(
        captured.len() > SAMPLE_RATE as usize,
        "parecord captured {} bytes, far less than a second — it did not \
         attach to `{DEVICE_NAME}`, so this says nothing about routing",
        captured.len()
    );

    let (samples, _) = captured.as_chunks::<{ size_of::<f32>() }>();
    samples
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk).abs())
        .fold(0.0f32, f32::max)
}

/// The acceptance criterion itself: another application selecting
/// the Virtual Microphone receives what voice-me played.
#[test]
#[ignore = "needs a running audio server and `parecord`"]
fn audio_played_into_the_device_reaches_a_capture_client() {
    let (temp, adapter) = fixture();

    let (played, captured) = capture_while_mic_spike_runs(&temp, &[]);

    adapter.uninstall().expect("uninstall");
    assert!(played.status.success(), "mic-spike failed: {played:?}");

    let peak = peak_of(&captured);
    assert!(
        peak > 0.05,
        "the capture client heard {peak}, i.e. silence — the audio was routed \
             somewhere other than the Virtual Microphone"
    );
}

/// Story 2.9's own row: not `play` called directly, but the whole Speak
/// Action — `voice_me_core::speak` handing what the `TtsPort` returned
/// straight to `VirtualMicPort::play` — landing on a capture client.
///
/// The four rows above prove the *device*; this one proves the *path*, which
/// is the thing this story added and the thing a future refactor of `speak`
/// could silently break. The child's `TtsPort` is a stub returning an
/// utterance-shaped buffer: the real engine would add 1.56 GB of model files
/// to a check about routing.
#[test]
#[ignore = "needs a running audio server and `parecord`"]
fn a_speak_action_reaches_a_capture_client() {
    let (temp, adapter) = fixture();

    let (played, captured) = capture_while_mic_spike_runs(&temp, &["--speak"]);

    adapter.uninstall().expect("uninstall");
    assert!(played.status.success(), "mic-spike failed: {played:?}");

    let peak = peak_of(&captured);
    assert!(
        peak > 0.05,
        "the capture client heard {peak}, i.e. silence — the generated line \
         never reached the Virtual Microphone"
    );
}

#[test]
#[ignore = "needs a running audio server"]
fn installing_twice_leaves_exactly_one_device() {
    let (_temp, adapter) = fixture();

    adapter.install().expect("second install");
    let count = adapter.device_count().expect("count");

    adapter.uninstall().expect("uninstall");
    assert_eq!(count, 1, "install must never duplicate the device");
}

#[test]
#[ignore = "needs a running audio server"]
fn uninstall_removes_the_device_and_is_quiet_when_there_is_nothing_to_remove() {
    let (_temp, adapter) = fixture();

    adapter.uninstall().expect("uninstall");
    assert!(!adapter.is_present().expect("presence"));

    adapter
        .uninstall()
        .expect("uninstall with nothing installed");
}

/// Playing to a device that was never installed must say so rather
/// than land on the default sink and come out of the speakers.
#[test]
#[ignore = "needs a running audio server"]
fn playing_without_installing_is_a_domain_error() {
    let temp = tempfile::tempdir().expect("temp dir");
    let adapter = LinuxVirtualMicAdapter::with_config_dir(temp.path().to_path_buf());
    adapter.uninstall().expect("start from nothing installed");

    let result = adapter.play(&tone(1));

    assert!(
        matches!(result, Err(VoiceMeError::VirtualMicUnavailable(_))),
        "a missing device must be reported, not silently ignored"
    );
}
