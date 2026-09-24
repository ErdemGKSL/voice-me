//! A Piper voice's `config.json`: only what synthesis reads.

use serde::Deserialize;

use crate::phonemes::PhonemeIdMap;

/// The parts of a voice's `config.json` the engine uses. Everything else
/// in the file (dataset, language names, …) is ignored.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PiperConfig {
    pub audio: AudioConfig,
    pub espeak: EspeakConfig,
    #[serde(default)]
    pub inference: InferenceConfig,
    /// More than one means the graph takes a `sid` input. voice-me always
    /// speaks as speaker 0 (no speaker picker).
    #[serde(default = "one")]
    pub num_speakers: u32,
    pub phoneme_id_map: PhonemeIdMap,
    /// Where the phonemes come from. voice-me only phonemizes through
    /// eSpeak NG, so anything but absent or `"espeak"` is refused.
    #[serde(default)]
    pub phoneme_type: Option<String>,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AudioConfig {
    /// What the graph's output is sampled at (22 050 Hz for most voices).
    pub sample_rate: u32,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EspeakConfig {
    /// The `espeak-ng -v` voice the phonemes come from (`tr`, `en-us`).
    pub voice: String,
}

/// The three synthesis scales, in the order the graph's `scales` input
/// takes them.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct InferenceConfig {
    #[serde(default = "default_noise_scale")]
    pub noise_scale: f32,
    #[serde(default = "default_length_scale")]
    pub length_scale: f32,
    #[serde(default = "default_noise_w")]
    pub noise_w: f32,
}

/// Piper's own defaults, for a config that leaves one out.
fn default_noise_scale() -> f32 {
    0.667
}
fn default_length_scale() -> f32 {
    1.0
}
fn default_noise_w() -> f32 {
    0.8
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            noise_scale: default_noise_scale(),
            length_scale: default_length_scale(),
            noise_w: default_noise_w(),
        }
    }
}

impl PiperConfig {
    /// Parse a voice's `config.json`; `Err` is the reason, in words.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let config: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if config.audio.sample_rate == 0 {
            return Err("its sample rate is 0".to_string());
        }
        if config.phoneme_id_map.is_empty() {
            return Err("its phoneme map is empty".to_string());
        }
        if let Some(kind) = config.phoneme_type.as_deref()
            && kind != "espeak"
        {
            return Err(format!(
                "its phoneme type is \"{kind}\"; voice-me only reads voices phonemized by \
                 eSpeak NG"
            ));
        }
        Ok(config)
    }

    /// `[noise_scale, length_scale, noise_w]`.
    pub fn scales(&self) -> [f32; 3] {
        [
            self.inference.noise_scale,
            self.inference.length_scale,
            self.inference.noise_w,
        ]
    }

    /// The `sid` input, only for a multi-speaker voice: speaker 0.
    pub fn speaker_id(&self) -> Option<i64> {
        (self.num_speakers > 1).then_some(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAHRETTIN: &str = include_str!("../tests/fixtures/tr_TR-fahrettin-medium.config.json");

    #[test]
    fn the_fahrettin_config_parses() {
        let config = PiperConfig::from_json(FAHRETTIN).unwrap();
        assert_eq!(config.audio.sample_rate, 22_050);
        assert_eq!(config.espeak.voice, "tr");
        assert_eq!(config.scales(), [0.667, 1.0, 0.8]);
        assert_eq!(config.num_speakers, 1);
        assert_eq!(config.phoneme_id_map.get("ˈ"), Some(&vec![120]));
    }

    /// `sid` only when the voice has more than one speaker.
    #[test]
    fn only_a_multi_speaker_voice_takes_a_speaker_id() {
        let mut config = PiperConfig::from_json(FAHRETTIN).unwrap();
        assert_eq!(config.speaker_id(), None);
        config.num_speakers = 4;
        assert_eq!(config.speaker_id(), Some(0));
    }

    /// Only eSpeak-phonemized voices are accepted; `text` or `pinyin`
    /// voices would speak garbage.
    #[test]
    fn a_voice_not_phonemized_by_espeak_is_refused() {
        let json = |kind: &str| {
            format!(
                r#"{{"audio":{{"sample_rate":22050}},"espeak":{{"voice":"zh"}},
                    "phoneme_id_map":{{"_":[0]}}{kind}}}"#
            )
        };
        assert!(PiperConfig::from_json(&json("")).is_ok());
        assert!(PiperConfig::from_json(&json(r#","phoneme_type":"espeak""#)).is_ok());
        for kind in ["text", "pinyin"] {
            let error =
                PiperConfig::from_json(&json(&format!(r#","phoneme_type":"{kind}""#))).unwrap_err();
            assert!(error.contains(kind), "{error}");
            assert!(error.contains("eSpeak NG"), "{error}");
        }
    }

    #[test]
    fn missing_scales_take_pipers_defaults_and_garbage_is_refused() {
        let config = PiperConfig::from_json(
            r#"{"audio":{"sample_rate":16000},"espeak":{"voice":"en-us"},
                "phoneme_id_map":{"_":[0]}}"#,
        )
        .unwrap();
        assert_eq!(config.scales(), [0.667, 1.0, 0.8]);
        assert!(PiperConfig::from_json("not json").is_err());
        assert!(
            PiperConfig::from_json(
                r#"{"audio":{"sample_rate":16000},"espeak":{"voice":"x"},"phoneme_id_map":{}}"#
            )
            .is_err()
        );
    }
}
