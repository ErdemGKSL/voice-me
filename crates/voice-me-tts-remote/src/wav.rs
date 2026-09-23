//! The provider's WAV, brought to AD-11's one format: 24 kHz mono f32.

use std::io::Cursor;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use voice_me_core::{AudioBuffer, SAMPLE_RATE, VoiceMeError};

/// The resampler's fixed input chunk, in frames.
const RESAMPLE_CHUNK: usize = 1024;

/// Decode `bytes` (any PCM or float WAV, any channel count, any rate) into
/// the AD-11 buffer: channels averaged to mono, resampled to 24 kHz when
/// the rate differs.
pub fn decode_wav(bytes: &[u8]) -> Result<AudioBuffer, VoiceMeError> {
    let unsupported = |why: String| VoiceMeError::UnsupportedAudioInput {
        format: format!("the provider's WAV — {why}"),
    };
    let reader =
        hound::WavReader::new(Cursor::new(bytes)).map_err(|e| unsupported(e.to_string()))?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels.max(1));

    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(|e| unsupported(e.to_string()))?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1_i64 << (spec.bits_per_sample.clamp(1, 32) - 1)) as f32;
            reader
                .into_samples::<i32>()
                .map(|sample| sample.map(|value| value as f32 * scale))
                .collect::<Result<_, _>>()
                .map_err(|e| unsupported(e.to_string()))?
        }
    };

    if interleaved.is_empty() {
        return Err(unsupported("it holds no audio".to_string()));
    }
    let mono = downmix(&interleaved, channels);
    Ok(AudioBuffer::new(resample(mono, spec.sample_rate)?))
}

/// Average each frame's channels.
fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    let scale = 1.0 / channels as f32;
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() * scale)
        .collect()
}

/// Convert mono `samples` at `rate` to exactly [`SAMPLE_RATE`].
fn resample(samples: Vec<f32>, rate: u32) -> Result<Vec<f32>, VoiceMeError> {
    if rate == SAMPLE_RATE || samples.is_empty() {
        return Ok(samples);
    }
    if rate == 0 {
        return Err(VoiceMeError::UnsupportedAudioInput {
            format: "the provider's WAV declares no sample rate".to_string(),
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
        VoiceMeError::Other(format!("could not wrap the provider's audio: {error}"))
    })?;
    let output = resampler
        .process_all(&input, frames, None)
        .map_err(|error| VoiceMeError::Other(format!("resampling failed: {error}")))?;
    Ok(output.take_data())
}

#[cfg(test)]
pub(crate) fn wav_bytes(rate: u32, channels: u16, frames: usize) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut out = Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut out, spec).unwrap();
        for frame in 0..frames {
            let value = ((frame as f32 / 20.0).sin() * 16_000.0) as i16;
            for _ in 0..channels {
                writer.write_sample(value).unwrap();
            }
        }
        writer.finalize().unwrap();
    }
    out.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_24k_mono_wav_passes_through_at_its_length() {
        let audio = decode_wav(&wav_bytes(24_000, 1, 2_400)).unwrap();
        assert_eq!(audio.len(), 2_400);
        assert!(audio.samples().iter().all(|s| (-1.0..=1.0).contains(s)));
        assert!(audio.samples().iter().any(|s| s.abs() > 0.1), "not silence");
    }

    #[test]
    fn a_48k_stereo_wav_becomes_24k_mono() {
        let audio = decode_wav(&wav_bytes(48_000, 2, 48_000)).unwrap();
        // One second of audio stays one second, give or take the
        // resampler's edge.
        assert!(
            (23_000..=25_000).contains(&audio.len()),
            "{} samples",
            audio.len()
        );
    }

    #[test]
    fn a_wav_with_no_frames_is_refused() {
        let error = decode_wav(&wav_bytes(24_000, 1, 0)).unwrap_err();
        assert!(error.to_string().contains("no audio"), "{error}");
    }

    #[test]
    fn bytes_that_are_not_a_wav_are_refused_in_words() {
        let error = decode_wav(b"not a wav").unwrap_err();
        assert!(error.to_string().contains("provider's WAV"), "{error}");
    }
}
