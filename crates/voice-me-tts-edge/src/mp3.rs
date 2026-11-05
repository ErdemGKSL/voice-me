//! `edge-tts`'s MP3, brought to AD-11's one format: 24 kHz mono f32.

use std::io::Cursor;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::TrackType;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;
use voice_me_core::{AudioBuffer, SAMPLE_RATE};

/// The resampler's fixed input chunk, in frames.
const RESAMPLE_CHUNK: usize = 1024;

/// Decode the MP3 `bytes` into the AD-11 buffer. `Err` is the reason, in
/// words, that no audio came out of them.
pub fn decode_mp3(bytes: &[u8]) -> Result<AudioBuffer, String> {
    if bytes.is_empty() {
        return Err("no bytes".to_string());
    }
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let stream = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
    let mut format = symphonia::default::get_probe()
        .probe(&hint, stream, Default::default(), Default::default())
        .map_err(|error| format!("not MP3: {error}"))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| "no audio track".to_string())?;
    let track_id = track.id;
    let Some(CodecParameters::Audio(params)) = track.codec_params.clone() else {
        return Err("an unrecognised codec".to_string());
    };
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &Default::default())
        .map_err(|error| format!("no decoder: {error}"))?;

    let mut interleaved: Vec<f32> = Vec::new();
    let mut scratch: Vec<f32> = Vec::new();
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
            Err(error) => return Err(format!("unreadable: {error}")),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = GenericAudioBufferRef::spec(&decoded);
                channels = spec.channels().count();
                rate = spec.rate();
                decoded.copy_to_vec_interleaved(&mut scratch);
                interleaved.extend_from_slice(&scratch);
            }
            // Recoverable per symphonia's contract: skip the packet.
            Err(SymphoniaError::DecodeError(_)) | Err(SymphoniaError::ResetRequired) => continue,
            Err(error) => return Err(format!("undecodable: {error}")),
        }
    }
    if interleaved.is_empty() {
        return Err("no audio in it".to_string());
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

/// Convert mono `samples` at `rate` to exactly [`SAMPLE_RATE`] — a no-op
/// for the 24 kHz `edge-tts` always writes.
fn resample(samples: Vec<f32>, rate: u32) -> Result<Vec<f32>, String> {
    if rate == SAMPLE_RATE || samples.is_empty() {
        return Ok(samples);
    }
    if rate == 0 {
        return Err("no sample rate".to_string());
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

    /// A 0.5 s tone, as `edge-tts` writes it: 24 kHz mono MP3.
    const TONE: &[u8] = include_bytes!("../tests/fixtures/tone.mp3");

    #[test]
    fn the_fixture_decodes_to_half_a_second_at_24_khz() {
        let audio = decode_mp3(TONE).unwrap();
        // MP3 frames pad the start and end a little.
        let frames = audio.len();
        assert!((11_000..=13_500).contains(&frames), "{frames} samples");
        assert!(audio.samples().iter().any(|sample| sample.abs() > 0.01));
    }

    #[test]
    fn empty_or_garbage_bytes_are_no_audio() {
        assert!(decode_mp3(&[]).is_err());
        assert!(decode_mp3(b"Traceback (most recent call last):\n").is_err());
    }

    #[test]
    fn a_rate_other_than_24_khz_is_resampled() {
        let samples = vec![0.25_f32; 22_050];
        let out = resample(samples, 22_050).unwrap();
        assert!((23_000..=25_000).contains(&out.len()), "{}", out.len());
        assert_eq!(resample(vec![0.5; 10], SAMPLE_RATE).unwrap(), vec![0.5; 10]);
    }
}
