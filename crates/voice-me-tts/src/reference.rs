//! Decoding the Reference Voice Sample into what `speech_encoder` demands.
//!
//! The encoder's `audio_values` input must *literally* be 24 kHz mono f32:
//! the exported graph holds a fixed internal 24k→16k resample, so handing it
//! 48 kHz audio does not produce slightly-off conditioning, it produces
//! conditioning derived from a clip playing at half speed. The user's clip,
//! meanwhile, is whatever they recorded or picked — this module is the
//! bridge, and it is the only place in the crate that knows about containers
//! and sample rates.

use std::fs::File;
use std::path::Path;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::TrackType;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;
use voice_me_core::{AudioBuffer, SAMPLE_RATE, VoiceMeError};

/// FFT chunk size for the offline conversion. Large enough that the
/// per-chunk overhead is irrelevant for a one-shot 30 s clip, small enough
/// that `process_all` still trims a short startup delay.
const RESAMPLE_CHUNK: usize = 4096;

/// Decode `path` and return it as 24 kHz mono f32, whatever it started as.
pub fn load_reference_clip(path: &Path) -> Result<AudioBuffer, VoiceMeError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(VoiceMeError::MissingRuntimeAsset {
                path: path.to_path_buf(),
            });
        }
        Err(error) => return Err(error.into()),
    };

    // The extension is a *hint* only — symphonia probes the bytes, so a wav
    // named `.mp3` still decodes. It just shortens the search.
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(extension);
    }

    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut format = symphonia::default::get_probe()
        .probe(&hint, stream, Default::default(), Default::default())
        .map_err(|error| unsupported_audio(path, error))?;

    let track = format.default_track(TrackType::Audio).ok_or_else(|| {
        VoiceMeError::UnsupportedAudioInput {
            format: format!("{} — the file holds no audio track", describe(path)),
        }
    })?;
    let track_id = track.id;
    let params = match track.codec_params.as_ref() {
        Some(CodecParameters::Audio(params)) => params.clone(),
        _ => {
            return Err(VoiceMeError::UnsupportedAudioInput {
                format: format!("{} — unrecognised audio codec", describe(path)),
            });
        }
    };

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &Default::default())
        .map_err(|error| unsupported_audio(path, error))?;

    let mut interleaved: Vec<f32> = Vec::new();
    let mut scratch: Vec<f32> = Vec::new();
    // Both are only known for certain once a packet has actually decoded —
    // the container's declared values can be absent.
    let mut channels = params.channels.as_ref().map_or(1, |c| c.count());
    let mut rate = params.sample_rate.unwrap_or(SAMPLE_RATE);

    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(SymphoniaError::IoError(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(error) => return Err(unsupported_audio(path, error)),
        };
        if packet.track_id != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = GenericAudioBufferRef::spec(&decoded);
                channels = spec.channels().count();
                rate = spec.rate();
                // `copy_to_vec_interleaved` *resizes* its destination, so it
                // gets a scratch buffer and the result is appended.
                decoded.copy_to_vec_interleaved(&mut scratch);
                interleaved.extend_from_slice(&scratch);
            }
            // Both are recoverable per symphonia's own contract: skip the
            // packet and keep going rather than losing the whole clip.
            Err(SymphoniaError::DecodeError(_)) | Err(SymphoniaError::ResetRequired) => continue,
            Err(error) => return Err(unsupported_audio(path, error)),
        }
    }

    if interleaved.is_empty() {
        return Err(VoiceMeError::UnsupportedAudioInput {
            format: format!("{} — decoded to no audio at all", describe(path)),
        });
    }

    let mono = downmix_to_mono(&interleaved, channels);
    let resampled = resample_to_model_rate(mono, rate)?;
    Ok(AudioBuffer::new(resampled))
}

/// Average the channels together. Averaging rather than taking the left
/// channel: a clip recorded on one side of a stereo pair would otherwise
/// come through at half amplitude or silent.
pub fn downmix_to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    let scale = 1.0 / channels as f32;
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() * scale)
        .collect()
}

/// Convert mono `samples` from `rate` to exactly [`SAMPLE_RATE`].
pub fn resample_to_model_rate(samples: Vec<f32>, rate: u32) -> Result<Vec<f32>, VoiceMeError> {
    if rate == SAMPLE_RATE {
        return Ok(samples);
    }
    if rate == 0 {
        return Err(VoiceMeError::UnsupportedAudioInput {
            format: "a clip that declares no sample rate".to_string(),
        });
    }

    let frames = samples.len();
    let mut resampler = Fft::<f32>::new(
        rate as usize,
        SAMPLE_RATE as usize,
        RESAMPLE_CHUNK,
        1,
        FixedSync::Input,
    )
    .map_err(|error| VoiceMeError::UnsupportedAudioInput {
        format: format!("{rate} Hz cannot be converted to {SAMPLE_RATE} Hz: {error}"),
    })?;

    let input = InterleavedSlice::new(&samples, 1, frames).map_err(|error| {
        VoiceMeError::Other(format!("could not wrap the reference clip: {error}"))
    })?;

    let output = resampler
        .process_all(&input, frames, None)
        .map_err(|error| VoiceMeError::Other(format!("resampling failed: {error}")))?;

    Ok(output.take_data())
}

fn describe(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_else(|| "unknown container".to_string())
}

fn unsupported_audio(path: &Path, error: SymphoniaError) -> VoiceMeError {
    VoiceMeError::UnsupportedAudioInput {
        format: format!("{} — {error}", describe(path)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_is_downmixed_by_averaging_both_channels() {
        // Left-only content: taking channel 0 would be right by accident,
        // so the second frame puts the content on the right instead.
        let interleaved = vec![1.0, 0.0, 0.0, 1.0, 0.5, 0.5];
        assert_eq!(downmix_to_mono(&interleaved, 2), vec![0.5, 0.5, 0.5]);
    }

    #[test]
    fn mono_passes_through_untouched() {
        let mono = vec![0.1, -0.2, 0.3];
        assert_eq!(downmix_to_mono(&mono, 1), mono);
    }

    #[test]
    fn the_models_own_rate_is_not_resampled_at_all() {
        let samples = vec![0.25; 1000];
        let out = resample_to_model_rate(samples.clone(), SAMPLE_RATE).unwrap();
        assert_eq!(out, samples, "a 24 kHz clip must not be touched");
    }

    #[test]
    fn forty_eight_kilohertz_halves_the_sample_count() {
        // Exactly one second in, so the ratio is directly checkable.
        let one_second = vec![0.0_f32; 48_000];
        let out = resample_to_model_rate(one_second, 48_000).unwrap();

        let expected = SAMPLE_RATE as i64;
        let error = (out.len() as i64 - expected).abs();
        assert!(
            error < 128,
            "48 kHz → 24 kHz should yield ~{expected} samples, got {}",
            out.len()
        );
    }

    #[test]
    fn upsampling_from_sixteen_kilohertz_lengthens_by_the_same_ratio() {
        let one_second = vec![0.0_f32; 16_000];
        let out = resample_to_model_rate(one_second, 16_000).unwrap();

        let expected = SAMPLE_RATE as i64;
        let error = (out.len() as i64 - expected).abs();
        assert!(
            error < 128,
            "16 kHz → 24 kHz should yield ~{expected} samples, got {}",
            out.len()
        );
    }

    #[test]
    fn a_resampled_sine_keeps_its_shape() {
        // A ratio test alone would pass on silence; this checks the content
        // survives, which is what conditioning quality actually depends on.
        let input: Vec<f32> = (0..48_000)
            .map(|n| (n as f32 * std::f32::consts::TAU * 220.0 / 48_000.0).sin())
            .collect();
        let out = resample_to_model_rate(input, 48_000).unwrap();

        let peak = out.iter().fold(0.0_f32, |acc, s| acc.max(s.abs()));
        assert!(peak > 0.9, "a 220 Hz sine should survive; peak was {peak}");
    }

    /// A minimal 16-bit PCM RIFF file, written by hand so the end-to-end
    /// decode path has a fixture that costs nothing to ship and needs no
    /// encoder: probe → decode → downmix → resample, all of it.
    fn stereo_wav_bytes(rate: u32, frames: usize) -> Vec<u8> {
        let data_len = (frames * 2 * 2) as u32;
        let mut bytes = Vec::with_capacity(44 + data_len as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&2_u16.to_le_bytes()); // stereo
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 4).to_le_bytes()); // byte rate
        bytes.extend_from_slice(&4_u16.to_le_bytes()); // block align
        bytes.extend_from_slice(&16_u16.to_le_bytes()); // bits
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for n in 0..frames {
            let value =
                ((n as f32 * std::f32::consts::TAU * 220.0 / rate as f32).sin() * 20_000.0) as i16;
            // Left carries the tone, right is silent: averaging must halve
            // it rather than dropping or doubling it.
            bytes.extend_from_slice(&value.to_le_bytes());
            bytes.extend_from_slice(&0_i16.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn a_forty_eight_kilohertz_stereo_file_arrives_as_twenty_four_kilohertz_mono() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reference.wav");
        std::fs::write(&path, stereo_wav_bytes(48_000, 48_000)).unwrap();

        let buffer = load_reference_clip(&path).unwrap();

        assert_eq!(buffer.sample_rate(), SAMPLE_RATE);
        let error = (buffer.len() as i64 - i64::from(SAMPLE_RATE)).abs();
        assert!(
            error < 512,
            "one second in must be one second out; got {} samples",
            buffer.len()
        );

        let peak = buffer
            .samples()
            .iter()
            .fold(0.0_f32, |acc, s| acc.max(s.abs()));
        assert!(
            (0.25..0.35).contains(&peak),
            "a 0.61 peak on the left channel alone must average to ~0.305; got {peak}"
        );
    }

    #[test]
    fn a_missing_clip_names_the_path_it_looked_for() {
        let path = Path::new("/nonexistent/voice-me/reference.wav");
        let error = load_reference_clip(path).unwrap_err();
        assert!(
            matches!(&error, VoiceMeError::MissingRuntimeAsset { path: p } if p == path),
            "got {error:?}"
        );
    }

    #[test]
    fn a_file_that_is_not_audio_reports_the_format_rather_than_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-audio.wav");
        std::fs::write(&path, b"this is not a RIFF file").unwrap();

        let error = load_reference_clip(&path).unwrap_err();
        assert!(
            matches!(&error, VoiceMeError::UnsupportedAudioInput { format } if format.contains("wav")),
            "got {error:?}"
        );
    }
}
