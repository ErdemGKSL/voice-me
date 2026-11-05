//! The Windows speech engine's WAV, brought to AD-11's one format: 24 kHz
//! mono f32.
//!
//! Copied from `voice-me-tts-system-linux`'s `wav.rs` (AD-2 keeps adapters
//! apart, and no shared crate exists) and extended: besides 16-bit PCM it
//! reads 24- and 32-bit PCM and 32-bit float, plain or as
//! `WAVE_FORMAT_EXTENSIBLE`. A `data` size that runs past the end of the
//! bytes (a streaming header) is read to the end instead.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use voice_me_core::{AudioBuffer, SAMPLE_RATE};

/// The resampler's fixed input chunk, in frames.
const RESAMPLE_CHUNK: usize = 1024;

/// `WAVE_FORMAT_PCM`.
const FORMAT_PCM: u16 = 1;
/// `WAVE_FORMAT_IEEE_FLOAT`.
const FORMAT_FLOAT: u16 = 3;
/// `WAVE_FORMAT_EXTENSIBLE`: the real tag is the first two bytes of the
/// sub-format GUID, 24 bytes into the `fmt ` chunk.
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// How the samples are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Pcm16,
    Pcm24,
    Pcm32,
    Float32,
}

impl Encoding {
    fn bytes(self) -> usize {
        match self {
            Encoding::Pcm16 => 2,
            Encoding::Pcm24 => 3,
            Encoding::Pcm32 | Encoding::Float32 => 4,
        }
    }

    fn sample(self, bytes: &[u8]) -> f32 {
        match self {
            Encoding::Pcm16 => f32::from(i16::from_le_bytes([bytes[0], bytes[1]])) / 32_768.0,
            Encoding::Pcm24 => {
                // Sign-extend by placing the 24 bits in the top of an i32.
                let value = i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8;
                value as f32 / 8_388_608.0
            }
            Encoding::Pcm32 => {
                i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32
                    / 2_147_483_648.0
            }
            Encoding::Float32 => f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        }
    }
}

/// Decode `bytes` into the AD-11 buffer. `Err` is the reason, in words,
/// that the bytes are not a WAV this can read.
pub fn decode_wav(bytes: &[u8]) -> Result<AudioBuffer, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("its output is not a WAV file".to_string());
    }

    let mut format: Option<(u16, u16, u32, u16)> = None;
    let mut offset: usize = 12;
    let data = loop {
        let Some(header) = bytes.get(offset..offset.saturating_add(8)) else {
            return Err("its WAV has no audio data".to_string());
        };
        let id = &header[0..4];
        let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let body = offset + 8;
        if id == b"data" {
            // A declared size that fits is honoured (a chunk may follow);
            // one past the end is a streaming placeholder: read to the end.
            let end = body.saturating_add(size).min(bytes.len());
            break &bytes[body..end];
        }
        if id == b"fmt " {
            // A `fmt ` chunk shorter than the 16 bytes every format has
            // is refused, never read past its end.
            let fmt = (size >= 16)
                .then(|| bytes.get(body..body.saturating_add(size)))
                .flatten();
            let Some(fmt) = fmt else {
                return Err("its WAV format header is cut short".to_string());
            };
            let u16_at = |at: usize| u16::from_le_bytes([fmt[at], fmt[at + 1]]);
            let rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
            let mut tag = u16_at(0);
            if tag == FORMAT_EXTENSIBLE {
                if fmt.len() < 26 {
                    return Err("its WAV format header is cut short".to_string());
                }
                tag = u16_at(24);
            }
            format = Some((tag, u16_at(2), rate, u16_at(14)));
        }
        // Chunks are padded to an even length.
        offset = body.saturating_add(size).saturating_add(size & 1);
    };

    let Some((tag, channels, rate, bits)) = format else {
        return Err("its WAV has no format header".to_string());
    };
    let encoding = match (tag, bits) {
        (FORMAT_PCM, 16) => Encoding::Pcm16,
        (FORMAT_PCM, 24) => Encoding::Pcm24,
        (FORMAT_PCM, 32) => Encoding::Pcm32,
        (FORMAT_FLOAT, 32) => Encoding::Float32,
        _ => {
            return Err(format!(
                "its WAV is not 16/24/32-bit PCM or 32-bit float (format {tag}, {bits} bits)"
            ));
        }
    };
    if channels == 0 || rate == 0 {
        return Err("its WAV declares no channels or no sample rate".to_string());
    }

    let interleaved: Vec<f32> = data
        .chunks_exact(encoding.bytes())
        .map(|sample| encoding.sample(sample))
        .collect();
    let channels = usize::from(channels);
    if interleaved.len() < channels {
        return Err("its WAV holds no audio".to_string());
    }
    let mono = downmix(&interleaved, channels);
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

/// Convert mono `samples` at `rate` to exactly [`SAMPLE_RATE`].
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A WAV with a correct header: `tag`, `channels`, `rate`, `bits`, and
    /// `samples` already encoded.
    fn wav(tag: u16, channels: u16, rate: u32, bits: u16, samples: &[u8]) -> Vec<u8> {
        let block = channels * (bits / 8);
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16_u32.to_le_bytes());
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * u32::from(block)).to_le_bytes());
        out.extend_from_slice(&block.to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        out.extend_from_slice(samples);
        out
    }

    fn sine(frames: usize) -> impl Iterator<Item = f32> {
        (0..frames).map(|frame| (frame as f32 / 20.0).sin() * 0.5)
    }

    fn pcm16(values: impl Iterator<Item = f32>) -> Vec<u8> {
        values
            .flat_map(|value| ((value * 32_767.0) as i16).to_le_bytes())
            .collect()
    }

    /// The Windows engine's usual output: 16-bit PCM, 22 050 Hz, mono.
    #[test]
    fn a_16_bit_22_khz_wav_is_resampled_to_24_khz() {
        let frames = 22_050;
        let audio = decode_wav(&wav(1, 1, 22_050, 16, &pcm16(sine(frames)))).unwrap();

        let expected = frames * 24_000 / 22_050;
        let got = audio.len();
        assert!(
            got.abs_diff(expected) <= expected / 100,
            "{got} samples, expected about {expected}"
        );
        assert!(audio.samples().iter().any(|sample| sample.abs() > 0.1));
    }

    #[test]
    fn a_32_bit_float_wav_is_read() {
        let samples: Vec<u8> = [0.5_f32, -0.25, 1.0]
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let audio = decode_wav(&wav(3, 1, 24_000, 32, &samples)).unwrap();
        assert_eq!(audio.samples(), &[0.5, -0.25, 1.0]);
    }

    #[test]
    fn a_wave_format_extensible_float_wav_is_read() {
        let samples: Vec<u8> = [0.5_f32, -0.5]
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&0_u32.to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&40_u32.to_le_bytes());
        out.extend_from_slice(&FORMAT_EXTENSIBLE.to_le_bytes());
        out.extend_from_slice(&1_u16.to_le_bytes());
        out.extend_from_slice(&24_000_u32.to_le_bytes());
        out.extend_from_slice(&96_000_u32.to_le_bytes());
        out.extend_from_slice(&4_u16.to_le_bytes());
        out.extend_from_slice(&32_u16.to_le_bytes());
        out.extend_from_slice(&22_u16.to_le_bytes()); // cbSize
        out.extend_from_slice(&32_u16.to_le_bytes()); // valid bits
        out.extend_from_slice(&4_u32.to_le_bytes()); // channel mask
        out.extend_from_slice(&FORMAT_FLOAT.to_le_bytes()); // sub-format GUID…
        out.extend_from_slice(&[0; 14]);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        out.extend_from_slice(&samples);

        let audio = decode_wav(&out).unwrap();
        assert_eq!(audio.samples(), &[0.5, -0.5]);
    }

    #[test]
    fn a_24_bit_and_a_32_bit_pcm_wav_are_read() {
        // -0.5 and 0.5 full scale.
        let pcm24: Vec<u8> = [-4_194_304_i32, 4_194_304]
            .iter()
            .flat_map(|value| value.to_le_bytes()[0..3].to_vec())
            .collect();
        let audio = decode_wav(&wav(1, 1, 24_000, 24, &pcm24)).unwrap();
        assert_eq!(audio.samples(), &[-0.5, 0.5]);

        let pcm32: Vec<u8> = [-1_073_741_824_i32, 1_073_741_824]
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let audio = decode_wav(&wav(1, 1, 24_000, 32, &pcm32)).unwrap();
        assert_eq!(audio.samples(), &[-0.5, 0.5]);
    }

    #[test]
    fn stereo_is_downmixed_to_mono() {
        // Frames (0.5, -0.5) and (0.5, 0.5) at 24 kHz: no resampling.
        let samples = pcm16([0.5, -0.5, 0.5, 0.5].into_iter());
        let audio = decode_wav(&wav(1, 2, 24_000, 16, &samples)).unwrap();
        assert_eq!(audio.len(), 2);
        assert!(audio.samples()[0].abs() < 1e-3, "{:?}", audio.samples());
        assert!(
            (audio.samples()[1] - 0.5).abs() < 1e-3,
            "{:?}",
            audio.samples()
        );
    }

    #[test]
    fn a_chunk_after_the_data_is_not_read_as_audio() {
        let mut bytes = wav(1, 1, 24_000, 16, &pcm16([0.5, 0.5].into_iter()));
        bytes.extend_from_slice(b"LIST");
        bytes.extend_from_slice(&4_u32.to_le_bytes());
        bytes.extend_from_slice(b"INFO");
        assert_eq!(decode_wav(&bytes).unwrap().len(), 2);
    }

    #[test]
    fn a_fmt_chunk_declared_shorter_than_16_bytes_is_cut_short() {
        let mut bytes = wav(1, 1, 24_000, 16, &pcm16([0.5, 0.5].into_iter()));
        // The `fmt ` size field sits at 16..20; the chunk body is intact.
        bytes[16..20].copy_from_slice(&14_u32.to_le_bytes());
        assert!(
            decode_wav(&bytes).unwrap_err().contains("cut short"),
            "{:?}",
            decode_wav(&bytes)
        );
    }

    #[test]
    fn garbage_and_unexpected_formats_are_refused_with_a_reason() {
        assert!(decode_wav(b"").is_err());
        assert!(decode_wav(b"this is not audio at all").is_err());
        // A header with no samples after it.
        assert!(
            decode_wav(&wav(1, 1, 22_050, 16, &[]))
                .unwrap_err()
                .contains("no audio")
        );
        // 8-bit PCM and A-law are not formats this reads.
        assert!(
            decode_wav(&wav(1, 1, 22_050, 8, &[0, 1]))
                .unwrap_err()
                .contains("format 1, 8 bits")
        );
        assert!(decode_wav(&wav(6, 1, 8_000, 8, &[0, 1])).is_err());
        // No format header at all.
        let mut no_fmt = b"RIFF\0\0\0\0WAVEdata".to_vec();
        no_fmt.extend_from_slice(&2_u32.to_le_bytes());
        no_fmt.extend_from_slice(&[0, 0]);
        assert!(decode_wav(&no_fmt).unwrap_err().contains("no format"));
    }
}
