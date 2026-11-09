//! Captured samples → the clip that is saved: mono, 22.05 kHz, silence
//! trimmed, 16-bit WAV.

use std::io::Cursor;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};

/// The rate the dataset is trained at (Piper "medium").
pub const TARGET_RATE: u32 = 22_050;

const RESAMPLE_CHUNK: usize = 1024;

/// Silence kept before and after the speech.
const KEEP_SECONDS: f32 = 0.20;

/// Samples quieter than this are silence when trimming.
const SILENCE_LEVEL: f32 = 0.010;

/// Shorter than this is a mis-click, not a sentence.
pub const MIN_SECONDS: f32 = 0.8;

/// A peak this high has clipped.
const CLIP_PEAK: f32 = 0.985;

/// A peak this low is too quiet to train on.
const QUIET_PEAK: f32 = 0.05;

/// Takes are scaled so their peak lands here (about -1 dBFS): one level
/// across the whole dataset, no clipping, and the gain is applied in float
/// before the single 16-bit quantisation.
const TARGET_PEAK: f32 = 0.89;

/// The most a quiet take is lifted, so a distant microphone's noise is not
/// lifted with it.
const MAX_GAIN: f32 = 4.0;

/// Below this speech-to-noise ratio the room is too noisy for training.
pub const MIN_SNR_DB: f32 = 30.0;

/// Edge fades, so trimming never leaves a click.
const FADE_SECONDS: f32 = 0.008;

/// What is wrong with a take, if anything.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quality {
    pub seconds: f32,
    pub peak: f32,
    /// Loud speech against the room, in dB; `None` when it cannot be told.
    pub snr_db: Option<f32>,
}

impl Quality {
    pub fn too_short(&self) -> bool {
        self.seconds < MIN_SECONDS
    }
    pub fn clipped(&self) -> bool {
        self.peak >= CLIP_PEAK
    }
    pub fn too_quiet(&self) -> bool {
        self.peak < QUIET_PEAK
    }
    pub fn noisy(&self) -> bool {
        self.snr_db.is_some_and(|snr| snr < MIN_SNR_DB)
    }
}

/// Average the interleaved `channels` into one.
pub fn to_mono(samples: &[f32], channels: u16) -> Vec<f32> {
    let channels = usize::from(channels.max(1));
    if channels == 1 {
        return samples.to_vec();
    }
    samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Convert mono `samples` at `rate` to [`TARGET_RATE`].
pub fn resample(samples: Vec<f32>, rate: u32) -> Result<Vec<f32>, String> {
    if rate == TARGET_RATE || samples.is_empty() {
        return Ok(samples);
    }
    if rate == 0 {
        return Err("the microphone reports no sample rate".to_string());
    }
    let frames = samples.len();
    let mut resampler = Fft::<f32>::new(
        rate as usize,
        TARGET_RATE as usize,
        RESAMPLE_CHUNK,
        1,
        FixedSync::Input,
    )
    .map_err(|error| format!("{rate} Hz cannot be converted to {TARGET_RATE} Hz: {error}"))?;
    let input = InterleavedSlice::new(&samples, 1, frames)
        .map_err(|error| format!("its audio could not be wrapped: {error}"))?;
    let output = resampler
        .process_all(&input, frames, None)
        .map_err(|error| format!("resampling failed: {error}"))?;
    Ok(output.take_data())
}

/// Cut leading and trailing silence, keeping [`KEEP_SECONDS`] of it.
pub fn trim_silence(samples: &[f32], rate: u32) -> &[f32] {
    let loud = |sample: &f32| sample.abs() >= SILENCE_LEVEL;
    let Some(first) = samples.iter().position(loud) else {
        return &samples[..0];
    };
    let last = samples.iter().rposition(loud).unwrap_or(first);
    let keep = (KEEP_SECONDS * rate as f32) as usize;
    &samples[first.saturating_sub(keep)..(last + 1 + keep).min(samples.len())]
}

pub fn quality(samples: &[f32], rate: u32) -> Quality {
    Quality {
        seconds: samples.len() as f32 / rate.max(1) as f32,
        peak: peak(samples),
        snr_db: snr_db(samples, rate),
    }
}

pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()))
}

/// Speech level over room level: the RMS of the loudest tenth of 20 ms
/// frames against the quietest tenth. `None` for a clip too short to have
/// both.
pub fn snr_db(samples: &[f32], rate: u32) -> Option<f32> {
    let frame = (rate as usize / 50).max(1);
    let mut levels: Vec<f32> = samples
        .chunks_exact(frame)
        .map(|f| (f.iter().map(|s| s * s).sum::<f32>() / frame as f32).sqrt())
        .collect();
    if levels.len() < 20 {
        return None;
    }
    levels.sort_by(f32::total_cmp);
    let tenth = levels.len() / 10;
    let rms = |part: &[f32]| (part.iter().map(|l| l * l).sum::<f32>() / part.len() as f32).sqrt();
    let noise = rms(&levels[..tenth]).max(1e-6);
    let speech = rms(&levels[levels.len() - tenth..]);
    Some(20.0 * (speech / noise).log10())
}

/// Remove the microphone's constant offset, which would otherwise ride
/// under the voice and clip the fades.
pub fn remove_dc(samples: &mut [f32]) {
    if samples.is_empty() {
        return;
    }
    let mean = samples.iter().sum::<f32>() / samples.len() as f32;
    samples.iter_mut().for_each(|s| *s -= mean);
}

/// Scale so the peak lands on [`TARGET_PEAK`] (never lifting by more than
/// [`MAX_GAIN`]); a clip that already peaks higher is brought down.
pub fn normalize(samples: &mut [f32]) {
    let current = peak(samples);
    if current < 1e-6 {
        return;
    }
    let gain = (TARGET_PEAK / current).min(MAX_GAIN);
    samples.iter_mut().for_each(|s| *s *= gain);
}

/// Short raised-cosine fades on both ends.
pub fn fade_edges(samples: &mut [f32], rate: u32) {
    let length = ((FADE_SECONDS * rate as f32) as usize).min(samples.len() / 2);
    for n in 0..length {
        let gain = 0.5 - 0.5 * (std::f32::consts::PI * n as f32 / length as f32).cos();
        samples[n] *= gain;
        let last = samples.len() - 1 - n;
        samples[last] *= gain;
    }
}

/// The clip to save, from a raw capture: DC removed, mono, [`TARGET_RATE`],
/// trimmed, levelled and faded, with the take's [`Quality`] measured before
/// levelling (so "too quiet" and "clipped" describe the microphone, not the
/// gain).
pub fn prepare(raw: &[f32], rate: u32, channels: u16) -> Result<(Vec<f32>, Quality), String> {
    let mut mono = to_mono(raw, channels);
    remove_dc(&mut mono);
    let raw_peak = peak(&mono);
    let clip = resample(mono, rate)?;
    let mut clip = trim_silence(&clip, TARGET_RATE).to_vec();
    let quality = Quality { peak: raw_peak, ..quality(&clip, TARGET_RATE) };
    normalize(&mut clip);
    fade_edges(&mut clip, TARGET_RATE);
    Ok((clip, quality))
}

/// `samples` as a 16-bit mono WAV at [`TARGET_RATE`].
pub fn wav_bytes(samples: &[f32]) -> Result<Vec<u8>, String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec).map_err(|e| e.to_string())?;
        for &sample in samples {
            let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
            writer.write_sample(value).map_err(|e| e.to_string())?;
        }
        writer.finalize().map_err(|e| e.to_string())?;
    }
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(seconds: f32, rate: u32, amplitude: f32) -> Vec<f32> {
        (0..(seconds * rate as f32) as usize)
            .map(|n| amplitude * (n as f32 * 0.05).sin())
            .collect()
    }

    #[test]
    fn stereo_is_averaged_to_mono() {
        assert_eq!(to_mono(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        assert_eq!(to_mono(&[0.25, 0.75], 1), vec![0.25, 0.75]);
    }

    #[test]
    fn resampling_lands_on_the_training_rate() {
        let out = resample(tone(1.0, 48_000, 0.5), 48_000).unwrap();
        let expected = TARGET_RATE as f32;
        assert!((out.len() as f32 - expected).abs() < 0.03 * expected, "{}", out.len());
        assert_eq!(resample(vec![0.1; 10], TARGET_RATE).unwrap().len(), 10);
        assert!(resample(vec![0.1; 10], 0).is_err());
    }

    #[test]
    fn silence_is_trimmed_but_a_margin_stays() {
        let rate = TARGET_RATE;
        let mut clip = vec![0.0; rate as usize];
        clip.extend(tone(1.0, rate, 0.5));
        clip.extend(vec![0.0; rate as usize]);
        let trimmed = trim_silence(&clip, rate);
        let expected = (1.0 + 2.0 * KEEP_SECONDS) * rate as f32;
        assert!((trimmed.len() as f32 - expected).abs() < 0.05 * expected, "{}", trimmed.len());
        assert!(trim_silence(&vec![0.0; 100], rate).is_empty());
    }

    #[test]
    fn a_take_is_judged_by_length_and_level() {
        let good = quality(&tone(2.0, TARGET_RATE, 0.5), TARGET_RATE);
        assert!(!good.too_short() && !good.clipped() && !good.too_quiet());
        assert!(quality(&tone(0.2, TARGET_RATE, 0.5), TARGET_RATE).too_short());
        assert!(quality(&tone(2.0, TARGET_RATE, 1.0), TARGET_RATE).clipped());
        assert!(quality(&tone(2.0, TARGET_RATE, 0.01), TARGET_RATE).too_quiet());
    }

    #[test]
    fn a_take_is_levelled_without_changing_its_shape() {
        let mut clip = tone(1.0, TARGET_RATE, 0.3);
        normalize(&mut clip);
        assert!((peak(&clip) - TARGET_PEAK).abs() < 1e-3);
        let mut hot = tone(1.0, TARGET_RATE, 1.0);
        normalize(&mut hot);
        assert!((peak(&hot) - TARGET_PEAK).abs() < 1e-3);
        let mut faint = tone(1.0, TARGET_RATE, 0.1);
        normalize(&mut faint);
        assert!((peak(&faint) - 0.4).abs() < 1e-3, "lift is capped at 12 dB");
    }

    #[test]
    fn dc_offset_is_removed_and_edges_fade() {
        let mut clip: Vec<f32> = tone(0.5, TARGET_RATE, 0.4).iter().map(|s| s + 0.2).collect();
        remove_dc(&mut clip);
        assert!((clip.iter().sum::<f32>() / clip.len() as f32).abs() < 1e-4);
        let mut flat = vec![0.5; 2000];
        fade_edges(&mut flat, TARGET_RATE);
        assert!(flat[0].abs() < 1e-6 && flat[1999].abs() < 1e-6 && (flat[1000] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_quiet_room_has_a_high_snr_and_a_noisy_one_a_low_one() {
        let speech = tone(0.5, TARGET_RATE, 0.5);
        let build = |floor: f32| {
            let mut clip = vec![floor; TARGET_RATE as usize / 2];
            clip.extend(&speech);
            clip.extend(vec![floor; TARGET_RATE as usize / 2]);
            clip
        };
        let quiet = quality(&build(0.001), TARGET_RATE);
        let noisy = quality(&build(0.05), TARGET_RATE);
        assert!(!quiet.noisy(), "{:?}", quiet.snr_db);
        assert!(noisy.noisy(), "{:?}", noisy.snr_db);
        assert_eq!(snr_db(&[0.1; 10], TARGET_RATE), None);
    }

    #[test]
    fn a_wav_is_16_bit_mono_at_the_training_rate() {
        let bytes = wav_bytes(&tone(0.5, TARGET_RATE, 0.5)).unwrap();
        let reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
        let spec = reader.spec();
        assert_eq!((spec.channels, spec.sample_rate, spec.bits_per_sample), (1, TARGET_RATE, 16));
    }
}
