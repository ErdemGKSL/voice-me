//! The KV-cached generation loop.
//!
//! One utterance is four graphs in sequence: `speech_encoder` once to turn
//! the Reference Voice Sample into conditioning, `embed_tokens` once to
//! embed the prompt, `language_model` once per speech token, and
//! `conditional_decoder` once to vocode. Everything expensive is in the
//! middle loop, which is why the 60 KV tensors are threaded through as
//! owned values rather than copied: `present.{L}.{kv}` becomes
//! `past_key_values.{L}.{kv}` by moving the handle, and the previous
//! generation's tensors drop at the same moment. transformers.js leaked a
//! full KV cache per call and reached ~24 GB; with 30 layers × 2 tensors
//! growing every step, that is not a theoretical concern.
//!
//! Every constant here comes from the model contract, not from a default in
//! some library: the sampler is pure greedy argmax because the ONNX export
//! has no temperature, top-p or cfg-weight input at all.

use std::borrow::Cow;
use std::time::{Duration, Instant};

use ort::session::SessionInputValue;
use ort::value::{DynValue, Tensor, TensorRef};
use voice_me_core::{AudioBuffer, VoiceMeError};

use crate::sessions::Sessions;

/// `SPEECH_VOCAB_SIZE`.
pub const SPEECH_VOCAB_SIZE: i64 = 6561;
/// The token that opens the speech stream — and the first entry the
/// repetition penalty applies to.
pub const START_SPEECH_TOKEN: i64 = 6561;
/// The token that ends it.
pub const STOP_SPEECH_TOKEN: i64 = 6562;
/// Prepended to the text by `tokenizer.json`'s template; takes position 0.
pub const EXAGGERATION_TOKEN: i64 = 6563;

/// 30 layers, each with a key and a value tensor.
const LAYERS: usize = 30;

/// Width of `language_model`'s `logits` output — the speech head's vocabulary.
///
/// The repetition penalty has to span this, not the speech-token range: the
/// head is wider than `STOP_SPEECH_TOKEN`, and a token the penalty cannot
/// reach is a token nothing discourages the model from repeating until
/// `max_new_tokens`.
const SPEECH_HEAD_VOCAB: usize = 8194;

/// The knobs the export actually exposes.
#[derive(Debug, Clone, Copy)]
pub struct GenerationSettings {
    /// How expressive the delivery is. The only continuous control the
    /// graph has.
    pub exaggeration: f32,
    /// Hard ceiling on the loop. At 25 tokens/s of speech, 256 tokens is
    /// about ten seconds.
    pub max_new_tokens: usize,
    /// Divides the logit of every already-generated token (multiplies it
    /// when negative). 1.0 disables it.
    pub repetition_penalty: f32,
}

impl Default for GenerationSettings {
    fn default() -> Self {
        Self {
            exaggeration: 0.5,
            max_new_tokens: 256,
            repetition_penalty: 1.2,
        }
    }
}

/// The audio, plus the timings the spike exists to collect.
pub struct GenerationOutcome {
    /// The generated speech, 24 kHz mono f32.
    pub audio: AudioBuffer,
    /// How many speech tokens the loop produced.
    pub tokens: usize,
    /// Whether the loop ended on `STOP_SPEECH_TOKEN` rather than running
    /// into `max_new_tokens`.
    pub stopped_on_token: bool,
    /// `speech_encoder`, once per utterance.
    pub encode: Duration,
    /// `embed_tokens`, once per utterance.
    pub embed: Duration,
    /// The whole `language_model` loop.
    pub decode_loop: Duration,
    /// `conditional_decoder`, once per utterance.
    pub vocode: Duration,
    /// Everything above, plus the bookkeeping between the stages.
    pub total: Duration,
}

impl GenerationOutcome {
    /// Milliseconds per generated token — the number that decides whether
    /// this is usable in a live voice chat.
    pub fn ms_per_token(&self) -> f64 {
        if self.tokens == 0 {
            return 0.0;
        }
        self.decode_loop.as_secs_f64() * 1000.0 / self.tokens as f64
    }

    /// Generated audio seconds per wall-clock second. Above 1.0 means
    /// generation outruns playback.
    pub fn realtime_factor(&self) -> f64 {
        if self.total.is_zero() {
            return 0.0;
        }
        self.audio.duration().as_secs_f64() / self.total.as_secs_f64()
    }
}

/// Generate one utterance.
///
/// `reference` must already be 24 kHz mono (see [`crate::reference`]) —
/// `speech_encoder` holds a fixed internal 24k→16k resample, so a clip at
/// any other rate conditions on speech playing at the wrong speed.
pub fn generate(
    sessions: &mut Sessions,
    text: &str,
    language: &str,
    reference: &AudioBuffer,
    settings: GenerationSettings,
) -> Result<GenerationOutcome, VoiceMeError> {
    if text.trim().is_empty() {
        return Err(VoiceMeError::EmptyText);
    }
    if reference.is_empty() {
        return Err(VoiceMeError::UnsupportedAudioInput {
            format: "an empty reference clip".to_string(),
        });
    }

    let started = Instant::now();
    let input_ids = sessions.tokenizer.encode(text, language)?;
    let prompt_len = input_ids.len();
    let position_ids = prefill_positions(&input_ids);

    // ---- speech_encoder: the reference clip → conditioning --------------
    let encode_started = Instant::now();
    let (cond_embeds, cond_len, audio_tokens, speaker_embeddings, speaker_features) = {
        let samples = reference.samples();
        let audio = TensorRef::from_array_view((vec![1_i64, samples.len() as i64], samples))
            .map_err(|error| engine("audio_values", error))?;
        let outputs = sessions
            .speech_encoder
            .run(ort::inputs!["audio_values" => audio])
            .map_err(|error| engine("speech_encoder", error))?;

        let (features_shape, features) = outputs["audio_features"]
            .try_extract_tensor::<f32>()
            .map_err(|error| engine("audio_features", error))?;
        let cond_len = features_shape[1] as usize;
        let cond_embeds = features.to_vec();

        let (_, tokens) = outputs["audio_tokens"]
            .try_extract_tensor::<i64>()
            .map_err(|error| engine("audio_tokens", error))?;
        let audio_tokens = tokens.to_vec();

        let (_, embeddings) = outputs["speaker_embeddings"]
            .try_extract_tensor::<f32>()
            .map_err(|error| engine("speaker_embeddings", error))?;
        let speaker_embeddings = embeddings.to_vec();

        let (features_shape, features) = outputs["speaker_features"]
            .try_extract_tensor::<f32>()
            .map_err(|error| engine("speaker_features", error))?;
        let speaker_features = (features_shape.to_vec(), features.to_vec());

        // `SessionOutputs` borrows the session mutably — it has to go
        // before anything else runs.
        drop(outputs);
        (
            cond_embeds,
            cond_len,
            audio_tokens,
            speaker_embeddings,
            speaker_features,
        )
    };
    let encode = encode_started.elapsed();

    // ---- embed_tokens: the prompt → embeddings --------------------------
    let embed_started = Instant::now();
    let mut embeds = {
        let shape = vec![1_i64, prompt_len as i64];
        let ids = TensorRef::from_array_view((shape.clone(), input_ids.as_slice()))
            .map_err(|error| engine("input_ids", error))?;
        let positions = TensorRef::from_array_view((shape, position_ids.as_slice()))
            .map_err(|error| engine("position_ids", error))?;
        let exaggeration = Tensor::from_array((vec![1_i64], vec![settings.exaggeration]))
            .map_err(|error| engine("exaggeration", error))?;

        let outputs = sessions
            .embed_tokens
            .run(ort::inputs![
                "input_ids" => ids,
                "position_ids" => positions,
                "exaggeration" => exaggeration,
            ])
            .map_err(|error| engine("embed_tokens", error))?;
        let (_, embeds) = outputs["inputs_embeds"]
            .try_extract_tensor::<f32>()
            .map_err(|error| engine("inputs_embeds", error))?;
        let embeds = embeds.to_vec();
        drop(outputs);
        embeds
    };
    let embed = embed_started.elapsed();

    // Step 0 only: the conditioning prefix goes in front of the prompt
    // embeddings, so the language model attends to the cloned voice before
    // it sees a word of text.
    let mut prefix = cond_embeds;
    prefix.append(&mut embeds);
    let mut step_embeds = prefix;
    let mut step_len = cond_len + prompt_len;
    let mut total_len = step_len;

    // ---- language_model: one run per speech token -----------------------
    let decode_started = Instant::now();
    let names = KvNames::new();
    let mut past = zero_kv_cache(sessions)?;
    // The opening START_SPEECH is part of the generated sequence for the
    // purposes of the repetition penalty, and is stripped again before
    // vocoding.
    let mut generated: Vec<i64> = vec![START_SPEECH_TOKEN];
    let mut penalised = vec![false; SPEECH_HEAD_VOCAB];
    penalised[START_SPEECH_TOKEN as usize] = true;
    let mut stopped_on_token = false;

    // `total_len` is not a loop counter that happens to track `step` — it is
    // the attention-mask length the model is given, which grows by exactly
    // one per generated token. Clippy's suggested `(step_len..).zip(..)`
    // rewrite hides that meaning behind an iterator adaptor.
    #[allow(
        clippy::explicit_counter_loop,
        reason = "total_len is the attention-mask length, not a counter"
    )]
    for step in 0..settings.max_new_tokens {
        let next = {
            let embeds_tensor = TensorRef::from_array_view((
                vec![1_i64, step_len as i64, 1024],
                step_embeds.as_slice(),
            ))
            .map_err(|error| engine("inputs_embeds", error))?;
            let mask = Tensor::from_array((vec![1_i64, total_len as i64], vec![1_i64; total_len]))
                .map_err(|error| engine("attention_mask", error))?;

            // The named arm of `inputs!` yields a plain `Vec`, which is what
            // makes threading 60 more entries onto it tractable.
            let mut inputs = ort::inputs![
                "inputs_embeds" => embeds_tensor,
                "attention_mask" => mask,
            ];
            for (name, value) in names.past.iter().zip(past.iter()) {
                inputs.push((Cow::from(name.as_str()), SessionInputValue::from(value)));
            }

            let mut outputs = sessions
                .language_model
                .run(inputs)
                .map_err(|error| engine("language_model", error))?;

            let next = {
                let (shape, logits) = outputs["logits"]
                    .try_extract_tensor::<f32>()
                    .map_err(|error| engine("logits", error))?;
                // `logits[:, -1, :]` — only the last position predicts the
                // next token; on the prefill run the rest is 6 MB of
                // positions nobody asked about.
                let vocab = shape[2] as usize;
                let last = &logits[logits.len() - vocab..];
                argmax_with_repetition_penalty(last, &penalised, settings.repetition_penalty)
            };

            // Move `present.{L}.{kv}` into `past_key_values.{L}.{kv}`. This
            // is a handle move, not a tensor copy, and dropping the old
            // `past` right after is what keeps the cache from accumulating.
            let mut next_past = Vec::with_capacity(names.present.len());
            for name in &names.present {
                let value = outputs.remove(name.as_str()).ok_or_else(|| {
                    VoiceMeError::SpeechEngine(format!("language_model produced no {name}"))
                })?;
                next_past.push(value);
            }
            drop(outputs);
            past = next_past;

            next
        };

        generated.push(next);
        if let Some(slot) = penalised.get_mut(next as usize) {
            *slot = true;
        }

        if next == STOP_SPEECH_TOKEN {
            stopped_on_token = true;
            break;
        }

        // Feed back only the new token. Speech positions start at 1: both
        // START_SPEECH tokens in the prompt sit at position 0.
        let outputs = {
            let ids = Tensor::from_array((vec![1_i64, 1], vec![next]))
                .map_err(|error| engine("input_ids", error))?;
            let positions = Tensor::from_array((vec![1_i64, 1], vec![step as i64 + 1]))
                .map_err(|error| engine("position_ids", error))?;
            let exaggeration = Tensor::from_array((vec![1_i64], vec![settings.exaggeration]))
                .map_err(|error| engine("exaggeration", error))?;
            sessions
                .embed_tokens
                .run(ort::inputs![
                    "input_ids" => ids,
                    "position_ids" => positions,
                    "exaggeration" => exaggeration,
                ])
                .map_err(|error| engine("embed_tokens", error))?
        };
        let (_, embeds) = outputs["inputs_embeds"]
            .try_extract_tensor::<f32>()
            .map_err(|error| engine("inputs_embeds", error))?;
        step_embeds = embeds.to_vec();
        drop(outputs);

        step_len = 1;
        total_len += 1;
    }
    let decode_loop = decode_started.elapsed();

    // Release the 60 KV tensors before the vocoder allocates.
    drop(past);

    // ---- conditional_decoder: speech tokens → waveform ------------------
    let mut speech_tokens = audio_tokens;
    speech_tokens.extend_from_slice(speech_body(&generated, stopped_on_token));

    let vocode_started = Instant::now();
    let audio = {
        let tokens = TensorRef::from_array_view((
            vec![1_i64, speech_tokens.len() as i64],
            speech_tokens.as_slice(),
        ))
        .map_err(|error| engine("speech_tokens", error))?;
        let embeddings = TensorRef::from_array_view((
            vec![1_i64, speaker_embeddings.len() as i64],
            speaker_embeddings.as_slice(),
        ))
        .map_err(|error| engine("speaker_embeddings", error))?;
        let (features_shape, features) = &speaker_features;
        let features = TensorRef::from_array_view((features_shape.clone(), features.as_slice()))
            .map_err(|error| engine("speaker_features", error))?;

        let outputs = sessions
            .conditional_decoder
            .run(ort::inputs![
                "speech_tokens" => tokens,
                "speaker_embeddings" => embeddings,
                "speaker_features" => features,
            ])
            .map_err(|error| engine("conditional_decoder", error))?;
        let (_, waveform) = outputs["waveform"]
            .try_extract_tensor::<f32>()
            .map_err(|error| engine("waveform", error))?;
        let audio = AudioBuffer::new(waveform.to_vec());
        drop(outputs);
        audio
    };
    let vocode = vocode_started.elapsed();

    Ok(GenerationOutcome {
        audio,
        tokens: generated.len() - 1,
        stopped_on_token,
        encode,
        embed,
        decode_loop,
        vocode,
        total: started.elapsed(),
    })
}

/// The 60 input and 60 output KV names, built once rather than formatted
/// 60 times per token.
struct KvNames {
    past: Vec<String>,
    present: Vec<String>,
}

impl KvNames {
    fn new() -> Self {
        let mut past = Vec::with_capacity(LAYERS * 2);
        let mut present = Vec::with_capacity(LAYERS * 2);
        for layer in 0..LAYERS {
            for kind in ["key", "value"] {
                past.push(format!("past_key_values.{layer}.{kind}"));
                present.push(format!("present.{layer}.{kind}"));
            }
        }
        Self { past, present }
    }
}

/// 60 empty `[1, 16, 0, 64]` tensors — the "nothing cached yet" state.
fn zero_kv_cache(sessions: &Sessions) -> Result<Vec<DynValue>, VoiceMeError> {
    let shape = vec![1_i64, 16, 0, 64];
    let mut cache = Vec::with_capacity(LAYERS * 2);
    for _ in 0..LAYERS * 2 {
        let value = if sessions.variant().kv_is_f16() {
            Tensor::<half::f16>::from_array((shape.clone(), Vec::new()))
                .map_err(|error| engine("past_key_values", error))?
                .into_dyn()
        } else {
            Tensor::<f32>::from_array((shape.clone(), Vec::new()))
                .map_err(|error| engine("past_key_values", error))?
                .into_dyn()
        };
        cache.push(value);
    }
    Ok(cache)
}

/// The speech tokens that actually get vocoded, out of everything the decode
/// loop produced.
pub fn speech_body(generated: &[i64], stopped_on_token: bool) -> &[i64] {
    // `generated[1..-1]`: the leading START_SPEECH is never spoken, and the
    // trailing STOP is only there to strip when the model actually emitted
    // one. Hitting `max_new_tokens` instead means the last token is real
    // speech, so trimming it unconditionally would silently clip every
    // capped utterance by one token.
    let body_end = if stopped_on_token {
        generated.len() - 1
    } else {
        generated.len()
    };
    &generated[1..body_end]
}

/// `where(input_ids >= 6561, 0, arange(seq) - 1)`.
///
/// The `-1` is not a typo and neither is the special case: BOS sits at
/// index 1 and must come out at position 0, while both 6563
/// (EXAGGERATION, index 0) and the two 6561s (START_SPEECH, at the end)
/// take position 0 as speech positions.
pub fn prefill_positions(input_ids: &[i64]) -> Vec<i64> {
    input_ids
        .iter()
        .enumerate()
        .map(|(index, &id)| {
            if id >= SPEECH_VOCAB_SIZE {
                0
            } else {
                index as i64 - 1
            }
        })
        .collect()
}

/// Repetition penalty, then greedy argmax.
///
/// The penalty divides positive logits and multiplies negative ones, so it
/// pushes an already-used token down regardless of sign — halving a
/// negative logit would *promote* it. It applies to every previously
/// generated token, the opening 6561 included.
pub fn argmax_with_repetition_penalty(logits: &[f32], penalised: &[bool], penalty: f32) -> i64 {
    let mut best_index = 0_usize;
    let mut best_score = f32::NEG_INFINITY;

    for (index, &score) in logits.iter().enumerate() {
        let score = if penalised.get(index).copied().unwrap_or(false) {
            if score < 0.0 {
                score * penalty
            } else {
                score / penalty
            }
        } else {
            score
        };
        if score > best_score {
            best_score = score;
            best_index = index;
        }
    }

    best_index as i64
}

fn engine<R>(what: &str, error: ort::Error<R>) -> VoiceMeError {
    VoiceMeError::SpeechEngine(format!("{what}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefill_positions_put_both_special_tokens_at_zero() {
        // EXAGGERATION, BOS, [tr], text, EOS, START_SPEECH, START_SPEECH
        let ids = vec![
            EXAGGERATION_TOKEN,
            255,
            712,
            300,
            0,
            START_SPEECH_TOKEN,
            START_SPEECH_TOKEN,
        ];
        let positions = prefill_positions(&ids);

        assert_eq!(positions[0], 0, "6563 is a speech token: position 0");
        assert_eq!(
            positions[1], 0,
            "BOS at index 1 is text position 0 — the -1"
        );
        assert_eq!(positions[2], 1);
        assert_eq!(positions[3], 2);
        assert_eq!(positions[4], 3);
        assert_eq!(&positions[5..], &[0, 0], "both 6561s take position 0");
    }

    #[test]
    fn the_penalty_pushes_a_used_token_down_whatever_its_sign() {
        let mut penalised = vec![false; 8];

        // Untouched, 1 wins outright.
        assert_eq!(
            argmax_with_repetition_penalty(&[1.0, 5.0, 4.0], &penalised, 1.2),
            1
        );

        // Once 1 has been generated, 5.0/1.2 = 4.17 still beats 4.0…
        penalised[1] = true;
        assert_eq!(
            argmax_with_repetition_penalty(&[1.0, 5.0, 4.0], &penalised, 1.2),
            1
        );
        // …but not 4.5.
        assert_eq!(
            argmax_with_repetition_penalty(&[1.0, 5.0, 4.5], &penalised, 1.2),
            2
        );
    }

    #[test]
    fn a_negative_logit_is_multiplied_not_divided() {
        let mut penalised = vec![false; 4];
        penalised[0] = true;

        // -1.0 penalised must become -1.2 (further from winning), so the
        // -1.1 candidate now wins. Dividing would give -0.83 and pick 0.
        assert_eq!(
            argmax_with_repetition_penalty(&[-1.0, -1.1], &penalised, 1.2),
            1
        );
    }

    #[test]
    fn a_penalty_of_one_changes_nothing() {
        let penalised = vec![true; 4];
        assert_eq!(
            argmax_with_repetition_penalty(&[1.0, 3.0, 2.0], &penalised, 1.0),
            1
        );
    }

    #[test]
    fn a_stop_token_is_stripped_before_vocoding() {
        // START_SPEECH, three real tokens, then the model's own STOP.
        let generated = vec![START_SPEECH_TOKEN, 11, 22, 33, STOP_SPEECH_TOKEN];
        assert_eq!(speech_body(&generated, true), &[11, 22, 33]);
    }

    #[test]
    fn hitting_the_token_cap_still_vocodes_every_token_generated() {
        // No STOP was ever emitted — the loop ran out at `max_new_tokens`,
        // so the final token is real speech. Trimming it the way the
        // stop-token path does would clip a token off every capped
        // utterance, and the audio would simply be a little short with
        // nothing to point at.
        let generated = vec![START_SPEECH_TOKEN, 11, 22, 33];
        assert_eq!(speech_body(&generated, false), &[11, 22, 33]);
    }

    #[test]
    fn a_run_that_produced_nothing_vocodes_nothing() {
        assert!(speech_body(&[START_SPEECH_TOKEN], false).is_empty());
        assert!(speech_body(&[START_SPEECH_TOKEN, STOP_SPEECH_TOKEN], true).is_empty());
    }

    #[test]
    fn the_kv_names_line_present_up_with_past_positionally() {
        let names = KvNames::new();
        assert_eq!(names.past.len(), 60);
        assert_eq!(names.present.len(), 60);
        assert_eq!(names.past[0], "past_key_values.0.key");
        assert_eq!(names.present[0], "present.0.key");
        assert_eq!(names.past[59], "past_key_values.29.value");
        assert_eq!(names.present[59], "present.29.value");
    }

    #[test]
    fn the_defaults_are_the_exports_own() {
        let settings = GenerationSettings::default();
        assert_eq!(settings.exaggeration, 0.5);
        assert_eq!(settings.max_new_tokens, 256);
        assert_eq!(settings.repetition_penalty, 1.2);
    }
}
