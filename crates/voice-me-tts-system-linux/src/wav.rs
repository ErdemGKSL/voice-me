//! eSpeak NG's WAV, brought to AD-11's one format: 24 kHz mono f32.
//!
//! `espeak-ng --stdout` writes 16-bit PCM at 22 050 Hz with a *streaming*
//! header: it cannot seek back on a pipe, so the RIFF and `data` sizes are
//! placeholders (`0x7ffff…`). The header is therefore parsed by hand and
//! the `data` chunk is read to the end of the bytes, whatever it declares.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use voice_me_core::{AudioBuffer, SAMPLE_RATE};

/// The resampler's fixed input chunk, in frames.
const RESAMPLE_CHUNK: usize = 1024;

/// Decode `bytes` into the AD-11 buffer. `Err` is the reason, in words,
/// that the bytes are not a WAV this can read.
pub fn decode_wav(bytes: &[u8]) -> Result<AudioBuffer, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("its output is not a WAV file".to_string());
    }

    let mut format: Option<(u16, u16, u32, u16)> = None;
    let mut offset = 12;
    let data = loop {
        let Some(header) = bytes.get(offset..offset + 8) else {
            return Err("its WAV has no audio data".to_string());
        };
        let id = &header[0..4];
        let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let body = offset + 8;
        if id == b"data" {
            // The streaming header's size is a placeholder: read to the end.
            break &bytes[body..];
        }
        if id == b"fmt " {
            let Some(fmt) = bytes.get(body..body + 16) else {
                return Err("its WAV format header is cut short".to_string());
            };
            let u16_at = |at: usize| u16::from_le_bytes([fmt[at], fmt[at + 1]]);
            let rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
            format = Some((u16_at(0), u16_at(2), rate, u16_at(14)));
        }
        // Chunks are padded to an even length.
        offset = body.saturating_add(size).saturating_add(size & 1);
    };

    let Some((tag, channels, rate, bits)) = format else {
        return Err("its WAV has no format header".to_string());
    };
    if tag != 1 || bits != 16 {
        return Err(format!(
            "its WAV is not 16-bit PCM (format {tag}, {bits} bits)"
        ));
    }
    if channels == 0 || rate == 0 {
        return Err("its WAV declares no channels or no sample rate".to_string());
    }

    let interleaved: Vec<f32> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|sample| f32::from(i16::from_le_bytes(*sample)) / 32_768.0)
        .collect();
    if interleaved.is_empty() {
        return Err("its WAV holds no audio".to_string());
    }
    let mono = downmix(&interleaved, usize::from(channels));
    Ok(AudioBuffer::new(resample(mono, rate)?))
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

/// Convert mono `samples` at `rate` to exactly [`SAMPLE_RATE`]. Copied from
/// `voice-me-tts-remote`'s `wav.rs`, which this crate must not depend on
/// (it is the network-capable one).
fn resample(samples: Vec<f32>, rate: u32) -> Result<Vec<f32>, String> {
    if rate == SAMPLE_RATE || samples.is_empty() {
        return Ok(samples);
    }
    let frames = samples.len();
    let mut resampler = Fft::<f32>::new(
        rate as usize,
        SAMPLE_RATE as usize,
        RESAMPLE_CHUNK,
        1,
        FixedSync::Input,
    )
    .map_err(|error| format!("{rate} Hz cannot be converted to {SAMPLE_RATE} Hz: {error}"))?;
    let input = InterleavedSlice::new(&samples, 1, frames)
        .map_err(|error| format!("its audio could not be wrapped: {error}"))?;
    let output = resampler
        .process_all(&input, frames, None)
        .map_err(|error| format!("resampling failed: {error}"))?;
    Ok(output.take_data())
}

/// A WAV the way `espeak-ng --stdout` writes one: 16-bit mono at `rate`,
/// with the streaming header's placeholder sizes.
#[cfg(test)]
pub(crate) fn streaming_wav(rate: u32, frames: usize) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&0x7fff_ffff_u32.to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16_u32.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2_u16.to_le_bytes());
    out.extend_from_slice(&16_u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&0x7fff_ffdb_u32.to_le_bytes());
    for frame in 0..frames {
        let value = ((frame as f32 / 20.0).sin() * 16_000.0) as i16;
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_streaming_header_is_read_to_the_end_and_resampled_to_24_khz() {
        let frames = 22_050;
        let audio = decode_wav(&streaming_wav(22_050, frames)).unwrap();

        let expected = frames * 24_000 / 22_050;
        let got = audio.len();
        assert!(
            got.abs_diff(expected) <= expected / 100,
            "{got} samples, expected about {expected}"
        );
        assert!(audio.samples().iter().any(|sample| sample.abs() > 0.1));
    }

    #[test]
    fn a_24_khz_wav_passes_through_unresampled() {
        let audio = decode_wav(&streaming_wav(24_000, 480)).unwrap();
        assert_eq!(audio.len(), 480);
    }

    #[test]
    fn garbage_is_refused_with_a_reason() {
        assert!(decode_wav(b"").is_err());
        assert!(decode_wav(b"this is not audio at all").is_err());
        // A header with no samples after it.
        let empty = streaming_wav(22_050, 0);
        assert!(decode_wav(&empty).unwrap_err().contains("no audio"));
        // Not 16-bit PCM.
        let mut float = streaming_wav(22_050, 10);
        float[20] = 3;
        assert!(decode_wav(&float).unwrap_err().contains("16-bit"));
    }
}
