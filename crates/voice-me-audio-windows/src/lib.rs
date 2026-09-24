//! `voice-me-audio-windows` — the Windows `VirtualMicPort` adapter.
//!
//! The Virtual Microphone on Windows is VB-Audio's signed **VB-CABLE**
//! (donationware, www.vb-cable.com), not a driver of voice-me's own
//! (spec-2-8). Everything played to its "CABLE Input" playback device comes
//! out of its "CABLE Output" recording device, so voice-me plays each
//! utterance to CABLE Input and a voice-chat app selects CABLE Output as its
//! microphone.
//!
//! Three things live here, and nothing else in the workspace knows them:
//!
//! * **Which device.** The one output device whose name starts with
//!   [`CABLE_INPUT_PREFIX`] — exactly one. There is no fallback to the
//!   default output device: a line played to the speakers is the failure
//!   this product exists to prevent (spec-2-9), so a missing or duplicated
//!   cable is a `VirtualMicUnavailable`, never a guess.
//! * **Conversion.** The AD-11 buffer (24 kHz mono f32) crosses `play`
//!   unchanged; it is resampled to the device's rate, copied to every
//!   channel and converted to the device's sample format here, and only
//!   here.
//! * **The installer launch.** [`run_installer_elevated`] starts VB's own
//!   setup behind a Windows administrator prompt. It is never silent: the
//!   user sees and confirms VB's installer, which carries VB's licence.
//!
//! The pure helpers (name matching, channel fan-out, resampling, sample
//! conversion, PowerShell quoting) are ordinary functions so the Linux CI
//! job tests them; only the device and the process launch need Windows.

use std::path::Path;
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use voice_me_core::{AudioBuffer, SAMPLE_RATE, VirtualMicPort, VoiceMeError};

/// VB-CABLE's playback endpoint is named "CABLE Input (VB-Audio Virtual
/// Cable)". Only the prefix is matched: the parenthesised part is the
/// driver's and has changed between releases.
pub const CABLE_INPUT_PREFIX: &str = "CABLE Input";

/// What another application selects as its microphone.
pub const CABLE_OUTPUT_NAME: &str = "CABLE Output";

/// VB's 64-bit setup program inside the driver pack.
pub const SETUP_PROGRAM: &str = "VBCABLE_Setup_x64.exe";

/// What the row says when the administrator prompt was declined.
pub const INSTALLER_NOT_ALLOWED: &str = "Windows did not allow the installer to run.";

/// How long past the utterance's own length `play` waits for the stream to
/// drain before giving up on a device that stopped pulling audio.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// How long the stream is kept open after the callback first finds nothing
/// left to play, so the buffer holding the utterance's tail — which may
/// still be queued in the WASAPI endpoint — is played before the stream is
/// dropped.
const TAIL_GRACE: Duration = Duration::from_millis(200);

/// The resampler's fixed input chunk, in frames.
const RESAMPLE_CHUNK: usize = 1024;

/// Windows `VirtualMicPort` adapter: plays to VB-CABLE's "CABLE Input".
///
/// Holds nothing: each [`VirtualMicPort::play`] looks the device up, opens
/// a stream, plays and closes it — the cable may have been installed (or
/// removed) since the last Speak Action.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsVirtualMicAdapter;

impl WindowsVirtualMicAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl VirtualMicPort for WindowsVirtualMicAdapter {
    fn play(&self, audio: &AudioBuffer) -> Result<(), VoiceMeError> {
        // Nothing to play is not a failure, and no device is opened for it.
        if audio.is_empty() {
            return Ok(());
        }
        let samples = audio.samples().to_vec();
        let duration = audio.duration();
        on_audio_thread(move || play_samples(&samples, duration))?
    }
}

/// `play`'s work, on the audio thread.
fn play_samples(samples: &[f32], duration: Duration) -> Result<(), VoiceMeError> {
    {
        let device = cable_input_device()?;
        let supported = device.default_output_config().map_err(|error| {
            unavailable(format!(
                "{CABLE_INPUT_PREFIX} did not report an output format: {error}"
            ))
        })?;
        let channels = usize::from(supported.channels());
        let rate = supported.sample_rate();
        let format = supported.sample_format();
        if channels == 0 || rate == 0 {
            return Err(unavailable(format!(
                "{CABLE_INPUT_PREFIX} reports {channels} channels at {rate} Hz, which cannot be \
                 played to"
            )));
        }
        let config: cpal::StreamConfig = supported.into();
        let mono = resample(samples, SAMPLE_RATE, rate)?;
        let timeout = duration + DRAIN_GRACE;

        match format {
            SampleFormat::F32 => play_on::<f32>(&device, config, mono, channels, timeout),
            SampleFormat::F64 => play_on::<f64>(&device, config, mono, channels, timeout),
            SampleFormat::I8 => play_on::<i8>(&device, config, mono, channels, timeout),
            SampleFormat::I16 => play_on::<i16>(&device, config, mono, channels, timeout),
            SampleFormat::I32 => play_on::<i32>(&device, config, mono, channels, timeout),
            SampleFormat::U8 => play_on::<u8>(&device, config, mono, channels, timeout),
            SampleFormat::U16 => play_on::<u16>(&device, config, mono, channels, timeout),
            SampleFormat::U32 => play_on::<u32>(&device, config, mono, channels, timeout),
            other => Err(unavailable(format!(
                "{CABLE_INPUT_PREFIX} uses the {other} sample format, which voice-me cannot play \
                 to"
            ))),
        }
    }
}

/// Whether VB-CABLE's "CABLE Input" is present right now — the Dependency
/// Check's question. Duplicates count as present (the row is about whether
/// the driver is installed); `play` is what refuses them.
pub fn virtual_microphone_available() -> Result<bool, VoiceMeError> {
    on_audio_thread(|| Ok(!matching_indices(&output_device_names()?.1).is_empty()))?
}

/// Run `job` on voice-me's one audio thread, which never exits, and wait
/// for its result.
///
/// cpal's WASAPI host initialises COM per thread and uninitialises it when
/// the thread ends, while its device enumerator is one process-wide object
/// created in whichever thread asked first. Used from short-lived threads —
/// Tokio's blocking pool retires idle threads, and every test runs on its
/// own — the enumerator outlives the apartment it was made in, and the next
/// call through it is an access violation. Every cpal call in this crate
/// runs here instead, so that apartment lives as long as the process.
fn on_audio_thread<T: Send + 'static>(
    job: impl FnOnce() -> T + Send + 'static,
) -> Result<T, VoiceMeError> {
    type Job = Box<dyn FnOnce() + Send>;
    static AUDIO_THREAD: OnceLock<Mutex<mpsc::Sender<Job>>> = OnceLock::new();

    let sender = AUDIO_THREAD.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        let spawned = std::thread::Builder::new()
            .name("voice-me-audio".to_string())
            .spawn(move || {
                for job in rx {
                    // A job that panics must not take the thread (and the
                    // apartment) down with it; its caller sees the dropped
                    // reply instead.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                }
            });
        if let Err(error) = spawned {
            eprintln!("could not start the audio thread: {error}");
        }
        Mutex::new(tx)
    });

    let (reply_tx, reply_rx) = mpsc::channel();
    let job: Job = Box::new(move || {
        let _ = reply_tx.send(job());
    });
    sender
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .send(job)
        .map_err(|_| unavailable("voice-me's audio thread is not running".to_string()))?;
    reply_rx.recv().map_err(|_| {
        unavailable("the audio thread failed while talking to Windows audio".to_string())
    })
}

fn unavailable(reason: String) -> VoiceMeError {
    VoiceMeError::VirtualMicUnavailable(reason)
}

/// Whether `name` is a VB-CABLE playback endpoint.
pub fn is_cable_input(name: &str) -> bool {
    name.starts_with(CABLE_INPUT_PREFIX)
}

fn matching_indices(names: &[String]) -> Vec<usize> {
    names
        .iter()
        .enumerate()
        .filter(|(_, name)| is_cable_input(name))
        .map(|(index, _)| index)
        .collect()
}

/// The index of the one output device named like VB-CABLE's input, or why
/// there is not exactly one.
pub fn select_cable_input(names: &[String]) -> Result<usize, VoiceMeError> {
    match matching_indices(names).as_slice() {
        [one] => Ok(*one),
        [] => Err(unavailable(format!(
            "there is no \"{CABLE_INPUT_PREFIX}\" playback device — VB-CABLE is not installed \
             (Settings → Dependencies → Virtual Microphone installs it)"
        ))),
        several => {
            let named: Vec<&str> = several.iter().map(|index| names[*index].as_str()).collect();
            Err(unavailable(format!(
                "there are {} playback devices starting with \"{CABLE_INPUT_PREFIX}\" ({}); \
                 voice-me will not guess which one your voice chat listens to",
                several.len(),
                named.join(", ")
            )))
        }
    }
}

/// Every output device on the default host (WASAPI on Windows), with its
/// name. A device whose name cannot be read is left out rather than
/// matched: it cannot be told apart from the speakers.
fn output_device_names() -> Result<(Vec<cpal::Device>, Vec<String>), VoiceMeError> {
    let devices = cpal::default_host()
        .output_devices()
        .map_err(|error| unavailable(format!("could not list playback devices: {error}")))?;
    let mut kept = Vec::new();
    let mut names = Vec::new();
    for device in devices {
        // `description()` rather than `to_string()`: cpal's `Display`
        // fails when the name cannot be read, and `to_string` would panic.
        // cpal 0.18.2's WASAPI `description()` itself panics when the
        // device's property store cannot be opened; such a device is
        // skipped like one whose name cannot be read.
        let described =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| device.description()));
        if let Ok(Ok(description)) = described {
            names.push(description.name().to_string());
            kept.push(device);
        }
    }
    Ok((kept, names))
}

fn cable_input_device() -> Result<cpal::Device, VoiceMeError> {
    let (devices, names) = output_device_names()?;
    let index = select_cable_input(&names)?;
    Ok(devices
        .into_iter()
        .nth(index)
        .expect("names and devices are pushed together"))
}

/// Convert mono `samples` at `from` Hz to `to` Hz. Same rate: unchanged.
pub fn resample(samples: &[f32], from: u32, to: u32) -> Result<Vec<f32>, VoiceMeError> {
    if from == to || samples.is_empty() {
        return Ok(samples.to_vec());
    }
    if from == 0 || to == 0 {
        return Err(unavailable(format!(
            "{from} Hz cannot be converted to {to} Hz"
        )));
    }
    let frames = samples.len();
    let mut resampler = Fft::<f32>::new(
        from as usize,
        to as usize,
        RESAMPLE_CHUNK,
        1,
        FixedSync::Input,
    )
    .map_err(|error| unavailable(format!("{from} Hz cannot be converted to {to} Hz: {error}")))?;
    let input = InterleavedSlice::new(samples, 1, frames)
        .map_err(|error| VoiceMeError::Other(format!("could not wrap the audio: {error}")))?;
    let output = resampler
        .process_all(&input, frames, None)
        .map_err(|error| VoiceMeError::Other(format!("resampling failed: {error}")))?;
    Ok(output.take_data())
}

/// Fill one device buffer of interleaved `channels`-channel frames from
/// `source` starting at `position`: each mono sample is copied to every
/// channel and converted to `T`; once `source` runs out the rest is
/// silence. Advances `position` and returns how many source frames went in.
pub fn fill_frames<T>(out: &mut [T], source: &[f32], position: &mut usize, channels: usize) -> usize
where
    T: Sample + FromSample<f32>,
{
    let mut written = 0;
    for frame in out.chunks_mut(channels.max(1)) {
        let value = match source.get(*position) {
            Some(&sample) => {
                *position += 1;
                written += 1;
                T::from_sample(sample.clamp(-1.0, 1.0))
            }
            None => T::EQUILIBRIUM,
        };
        frame.fill(value);
    }
    written
}

/// What the stream's callbacks tell the waiting `play`.
enum Playback {
    Drained,
    Failed(String),
}

/// Open an output stream in `config` and feed it `mono` until it runs out.
/// Once a callback first finds nothing left to play, the stream is kept
/// open for a further [`TAIL_GRACE`] before it is dropped, so the buffer
/// holding the last samples can still leave the endpoint.
fn play_on<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mono: Vec<f32>,
    channels: usize,
    timeout: Duration,
) -> Result<(), VoiceMeError>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    let errors = tx.clone();
    let mut position = 0;
    let mut signalled = false;
    let stream = device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                let written = fill_frames(data, &mono, &mut position, channels);
                if written == 0 && !signalled {
                    signalled = true;
                    let _ = tx.send(Playback::Drained);
                }
            },
            move |error: cpal::Error| {
                let _ = errors.send(Playback::Failed(error.to_string()));
            },
            None,
        )
        .map_err(|error| {
            unavailable(format!(
                "could not open {CABLE_INPUT_PREFIX} for playback: {error}"
            ))
        })?;
    stream.play().map_err(|error| {
        unavailable(format!(
            "could not start playback to {CABLE_INPUT_PREFIX}: {error}"
        ))
    })?;

    let outcome = rx.recv_timeout(timeout);
    if matches!(outcome, Ok(Playback::Drained)) {
        std::thread::sleep(TAIL_GRACE);
    }
    drop(stream);
    match outcome {
        Ok(Playback::Drained) => Ok(()),
        Ok(Playback::Failed(reason)) => Err(unavailable(format!(
            "playback to {CABLE_INPUT_PREFIX} failed: {reason}"
        ))),
        Err(_) => Err(unavailable(format!(
            "{CABLE_INPUT_PREFIX} stopped taking audio; the utterance did not finish playing"
        ))),
    }
}

/// `path` as a PowerShell single-quoted string literal. PowerShell also
/// treats the typographic quotes ‘ ’ ‚ ‛ as single quotes, so each of them
/// is doubled as well as `'`.
pub fn powershell_quote(path: &Path) -> String {
    let mut quoted = String::from("'");
    for character in path.display().to_string().chars() {
        if matches!(
            character,
            '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}'
        ) {
            quoted.push(character);
        }
        quoted.push(character);
    }
    quoted.push('\'');
    quoted
}

/// The PowerShell command that runs `setup` behind the administrator
/// prompt, waits for it to finish and exits with the setup's own exit
/// code. A declined prompt fails `Start-Process` itself, which PowerShell
/// reports on stderr.
pub fn elevation_command(setup: &Path) -> String {
    format!(
        "$p = Start-Process -FilePath {} -Verb RunAs -Wait -PassThru -ErrorAction Stop; \
         exit $p.ExitCode",
        powershell_quote(setup)
    )
}

/// What PowerShell's exit tells about the setup. Success is exit 0. A
/// failure with something on stderr is PowerShell failing before the setup
/// started (the administrator prompt declined): [`INSTALLER_NOT_ALLOWED`]
/// and the first non-empty stderr line. A failure with a silent stderr is
/// the setup's own non-zero exit code (for example, cancelled inside VB's
/// installer).
pub fn installer_outcome(code: Option<i32>, stderr: &str) -> Result<(), VoiceMeError> {
    if code == Some(0) {
        return Ok(());
    }
    if let Some(line) = stderr.lines().map(str::trim).find(|line| !line.is_empty()) {
        return Err(VoiceMeError::Other(format!(
            "{INSTALLER_NOT_ALLOWED} {line}"
        )));
    }
    Err(VoiceMeError::Other(match code {
        Some(code) => format!("VB-CABLE's setup did not finish (exit code {code})."),
        None => "VB-CABLE's setup did not finish.".to_string(),
    }))
}

/// Run VB's own setup program elevated — Windows shows its administrator
/// prompt, then VB's installer UI — and wait for it to close.
///
/// A declined prompt makes `Start-Process` fail; that is reported as
/// [`INSTALLER_NOT_ALLOWED`], and a setup that exits non-zero by its exit
/// code (see [`installer_outcome`]). Nothing here installs silently: the user
/// confirms the prompt and VB's installer both.
#[cfg(target_os = "windows")]
pub fn run_installer_elevated(setup: &Path) -> Result<(), VoiceMeError> {
    use std::os::windows::process::CommandExt as _;

    /// No console window flashing up behind the prompt.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    if !setup.is_file() {
        return Err(VoiceMeError::Other(format!(
            "VB-CABLE's setup program is not at {}",
            setup.display()
        )));
    }
    // The system copy by full path, so a `powershell.exe` elsewhere on PATH
    // is never the one handed an elevation request.
    let powershell = std::env::var_os("SystemRoot")
        .map(|root| {
            Path::new(&root)
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe")
        })
        .filter(|path| path.is_file())
        .unwrap_or_else(|| "powershell.exe".into());
    let command = elevation_command(setup);
    let output = std::process::Command::new(powershell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            command.as_str(),
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| {
            VoiceMeError::Other(format!(
                "Could not start PowerShell to run VB-CABLE's setup: {error}"
            ))
        })?;
    installer_outcome(
        output.status.code(),
        &String::from_utf8_lossy(&output.stderr),
    )
}

/// Only Windows has VB-CABLE's setup to run.
#[cfg(not(target_os = "windows"))]
pub fn run_installer_elevated(_setup: &Path) -> Result<(), VoiceMeError> {
    Err(VoiceMeError::Other(
        "VB-CABLE's setup only runs on Windows.".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_string()).collect()
    }

    /// Every audio job runs on the same long-lived thread, whichever
    /// short-lived thread asked — and one that panics does not take it down.
    #[test]
    fn audio_jobs_from_short_lived_threads_share_one_audio_thread() {
        let names: Vec<String> = (0..4)
            .map(|_| {
                std::thread::spawn(|| {
                    on_audio_thread(|| std::thread::current().name().map(str::to_string))
                        .unwrap()
                        .unwrap()
                })
                .join()
                .unwrap()
            })
            .collect();
        assert!(
            names.iter().all(|name| name == "voice-me-audio"),
            "{names:?}"
        );

        assert!(on_audio_thread(|| panic!("a job that fails")).is_err());
        assert_eq!(on_audio_thread(|| 7).unwrap(), 7, "the thread survived");
    }

    #[test]
    fn playing_an_empty_buffer_does_nothing_and_succeeds() {
        assert!(
            WindowsVirtualMicAdapter
                .play(&AudioBuffer::new(Vec::new()))
                .is_ok()
        );
    }

    #[test]
    fn the_one_cable_input_is_selected_among_other_devices() {
        let devices = names(&[
            "Speakers (Realtek(R) Audio)",
            "CABLE Input (VB-Audio Virtual Cable)",
            "Headphones (USB Audio)",
        ]);
        assert_eq!(select_cable_input(&devices).unwrap(), 1);
    }

    /// Matrix row "Speak, cable missing": no fallback to the speakers.
    #[test]
    fn no_cable_input_is_a_virtual_mic_error_naming_vb_cable() {
        let error = select_cable_input(&names(&["Speakers (Realtek(R) Audio)", "CABLE Output"]))
            .unwrap_err();
        assert!(
            matches!(&error, VoiceMeError::VirtualMicUnavailable(reason)
                if reason.contains("VB-CABLE") && reason.contains(CABLE_INPUT_PREFIX)),
            "{error}"
        );
    }

    /// Matrix row "Two matching devices": refuse, naming both.
    #[test]
    fn two_cable_inputs_are_refused_naming_the_duplicates() {
        let error = select_cable_input(&names(&[
            "CABLE Input (VB-Audio Virtual Cable)",
            "Speakers",
            "CABLE Input 16ch (VB-Audio Virtual Cable)",
        ]))
        .unwrap_err();
        let VoiceMeError::VirtualMicUnavailable(reason) = &error else {
            panic!("{error}");
        };
        assert!(reason.contains("2 playback devices"), "{reason}");
        assert!(
            reason.contains("CABLE Input (VB-Audio Virtual Cable)")
                && reason.contains("CABLE Input 16ch"),
            "{reason}"
        );
    }

    #[test]
    fn only_the_prefix_matches() {
        assert!(is_cable_input("CABLE Input (VB-Audio Virtual Cable)"));
        assert!(!is_cable_input("Speakers (CABLE Input)"));
        assert!(!is_cable_input("cable input"));
        assert!(!is_cable_input("CABLE-A Input (VB-Audio Cable A)"));
    }

    /// Matrix row "Device rate/format": mono fans out to every channel.
    #[test]
    fn mono_is_copied_to_every_channel_then_silence_follows() {
        let source = [0.5_f32, -0.25];
        let mut position = 0;
        let mut out = [9.0_f32; 6];

        let written = fill_frames(&mut out, &source, &mut position, 2);

        assert_eq!(written, 2);
        assert_eq!(position, 2);
        assert_eq!(out, [0.5, 0.5, -0.25, -0.25, 0.0, 0.0]);

        let mut next = [9.0_f32; 4];
        assert_eq!(fill_frames(&mut next, &source, &mut position, 2), 0);
        assert_eq!(next, [0.0; 4]);
    }

    #[test]
    fn samples_are_converted_to_the_device_format() {
        let source = [1.0_f32, 0.0, -1.0, 2.0];
        let mut position = 0;
        let mut out = [0_i16; 4];
        fill_frames(&mut out, &source, &mut position, 1);
        assert_eq!(out[0], i16::MAX);
        assert_eq!(out[1], 0);
        assert!(out[2] <= -i16::MAX, "{}", out[2]);
        assert_eq!(out[3], i16::MAX, "out-of-range input is clamped");

        let mut position = 0;
        let mut unsigned = [0_u16; 2];
        fill_frames(&mut unsigned, &[0.0, 0.0], &mut position, 1);
        assert_eq!(unsigned, [u16::EQUILIBRIUM; 2]);
    }

    #[test]
    fn a_frame_boundary_mid_callback_resumes_where_it_left_off() {
        let source: Vec<f32> = (0..5).map(|n| n as f32 / 10.0).collect();
        let mut position = 0;
        let mut first = [0.0_f32; 3 * 2];
        let mut second = [0.0_f32; 3 * 2];
        fill_frames(&mut first, &source, &mut position, 3);
        fill_frames(&mut second, &source, &mut position, 3);
        assert_eq!(first, [0.0, 0.0, 0.0, 0.1, 0.1, 0.1]);
        assert_eq!(second, [0.2, 0.2, 0.2, 0.3, 0.3, 0.3]);
        assert_eq!(position, 4);
    }

    #[test]
    fn resampling_24k_to_48k_doubles_the_length() {
        let second: Vec<f32> = (0..24_000).map(|n| (n as f32 / 20.0).sin() * 0.5).collect();
        let out = resample(&second, 24_000, 48_000).unwrap();
        assert!(
            (47_000..=49_000).contains(&out.len()),
            "{} samples",
            out.len()
        );
        assert!(out.iter().any(|sample| sample.abs() > 0.1), "not silence");
    }

    #[test]
    fn the_same_rate_is_passed_through() {
        let samples = vec![0.1, 0.2, 0.3];
        assert_eq!(resample(&samples, 24_000, 24_000).unwrap(), samples);
    }

    #[test]
    fn a_zero_rate_is_a_virtual_mic_error() {
        assert!(matches!(
            resample(&[0.1], 24_000, 0),
            Err(VoiceMeError::VirtualMicUnavailable(_))
        ));
    }

    #[test]
    fn the_setup_path_is_quoted_for_powershell() {
        let path = Path::new(r"C:\Users\O'Brien\cache\vb-cable\VBCABLE_Setup_x64.exe");
        assert_eq!(
            powershell_quote(path),
            r"'C:\Users\O''Brien\cache\vb-cable\VBCABLE_Setup_x64.exe'"
        );
        let command = elevation_command(path);
        assert_eq!(
            command,
            r"$p = Start-Process -FilePath 'C:\Users\O''Brien\cache\vb-cable\VBCABLE_Setup_x64.exe' -Verb RunAs -Wait -PassThru -ErrorAction Stop; exit $p.ExitCode"
        );
    }

    /// PowerShell reads ‘ ’ ‚ ‛ as single quotes too: each is doubled.
    #[test]
    fn typographic_single_quotes_are_doubled() {
        let path = Path::new("C:\\a\u{2018}b\u{2019}c\u{201A}d\u{201B}e'f");
        assert_eq!(
            powershell_quote(path),
            "'C:\\a\u{2018}\u{2018}b\u{2019}\u{2019}c\u{201A}\u{201A}d\u{201B}\u{201B}e''f'"
        );
    }

    #[test]
    fn a_setup_that_exited_zero_is_success() {
        assert!(installer_outcome(Some(0), "").is_ok());
    }

    #[test]
    fn a_declined_prompt_names_powershells_reason() {
        let error = installer_outcome(
            Some(1),
            "\r\nStart-Process : This command cannot be run due to the error: The operation was \
             canceled by the user.\r\nAt line:1 char:6\r\n",
        )
        .unwrap_err()
        .to_string();
        assert_eq!(
            error,
            "Windows did not allow the installer to run. Start-Process : This command cannot be \
             run due to the error: The operation was canceled by the user."
        );
    }

    #[test]
    fn a_setup_that_exited_non_zero_reports_its_code() {
        let error = installer_outcome(Some(3), " \n").unwrap_err().to_string();
        assert_eq!(error, "VB-CABLE's setup did not finish (exit code 3).");
    }
}
