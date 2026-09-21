//! The AD-11 audio buffer: the one in-memory representation generated
//! speech takes between `TtsPort::generate` and `VirtualMicPort::play`.
//!
//! It is defined here, in the hexagon, rather than in either adapter,
//! because it is exactly the guess AD-11 exists to prevent: the TTS side
//! would otherwise hand over "some bytes" and the audio side would have to
//! assume a container, a sample format, a channel count and a rate. The
//! format is not negotiable and not carried per-buffer — Chatterbox's
//! `conditional_decoder` emits 24 kHz mono f32 and nothing else, so the rate
//! is a constant and mono is a property of the type.

use std::time::Duration;

/// The one sample rate in this application, in Hz.
///
/// `conditional_decoder`'s `waveform` output is 24 kHz (`S3GEN_SR`), and
/// `speech_encoder`'s `audio_values` input must *literally* be 24 kHz
/// because the graph holds a fixed internal 24k→16k resample. Both sides of
/// the pipeline therefore agree on this number; it is a constant rather
/// than a field so no buffer can claim a rate the models cannot produce.
pub const SAMPLE_RATE: u32 = 24_000;

/// A buffer of generated (or reference) speech: 24 kHz, mono, `f32` samples
/// nominally in `[-1.0, 1.0]`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioBuffer {
    samples: Vec<f32>,
}

impl AudioBuffer {
    /// Wrap already-24 kHz mono samples.
    pub fn new(samples: Vec<f32>) -> Self {
        Self { samples }
    }

    /// The samples, in presentation order.
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    /// Take ownership of the samples without copying them.
    pub fn into_samples(self) -> Vec<f32> {
        self.samples
    }

    /// Number of samples — which, being mono, is also the number of frames.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether the buffer holds no audio at all.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The sample rate of every `AudioBuffer`, for callers that need it as a
    /// value (a WAV header, a device configuration) rather than a constant.
    pub fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }

    /// How long this buffer plays for.
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.samples.len() as f64 / f64::from(SAMPLE_RATE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_follows_the_fixed_sample_rate() {
        let one_second = AudioBuffer::new(vec![0.0; SAMPLE_RATE as usize]);
        assert_eq!(one_second.duration(), Duration::from_secs(1));

        let half = AudioBuffer::new(vec![0.0; SAMPLE_RATE as usize / 2]);
        assert_eq!(half.duration(), Duration::from_millis(500));
    }

    #[test]
    fn an_empty_buffer_has_zero_duration() {
        let empty = AudioBuffer::default();
        assert!(empty.is_empty());
        assert_eq!(empty.duration(), Duration::ZERO);
    }
}
