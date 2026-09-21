//! Text → token ids, exactly as the exported model expects them.
//!
//! Two things are easy to get wrong here and both are silent failures — the
//! model happily generates *something* either way:
//!
//! 1. The language is not a parameter. It is a literal bracketed tag glued
//!    to the front of the string (`"[tr]Merhaba"`), and the tags are real
//!    single tokens in the vocabulary (`[en]` = 708, `[tr]` = 712), not
//!    three characters that fall apart into `[`, `tr`, `]`.
//! 2. `tokenizer.json`'s `TemplateProcessing` wraps every sequence as
//!    `[6563, 255, …text…, 0, 6561, 6561]` — EXAGGERATION, BOS, the text,
//!    EOS, then **two** START_SPEECH. `encode(s, true)` reproduces that for
//!    free; hand-rolling it means reimplementing the template exactly.

use std::path::{Path, PathBuf};

use tokenizers::Tokenizer;
use voice_me_core::VoiceMeError;

use crate::generate::{SPEECH_VOCAB_SIZE, START_SPEECH_TOKEN};

/// Whether an encoded prompt still has the shape the decode loop assumes:
/// a special token first (so `prefill_positions` never emits a negative
/// position) and two `START_SPEECH` tokens last (so the speech stream opens
/// where the loop expects it to).
fn template_is_intact(ids: &[i64]) -> bool {
    let head_is_special = ids.first().is_some_and(|&id| id >= SPEECH_VOCAB_SIZE);
    let tail_is_start_speech = ids.len() >= 2
        && ids[ids.len() - 2..]
            .iter()
            .all(|&id| id == START_SPEECH_TOKEN);
    head_is_special && tail_is_start_speech
}

/// The wrapped BPE tokenizer from the pinned model revision.
pub struct TextTokenizer {
    inner: Tokenizer,
}

impl TextTokenizer {
    /// Load `tokenizer.json` from an explicit path.
    pub fn load(path: &Path) -> Result<Self, VoiceMeError> {
        if !path.exists() {
            return Err(VoiceMeError::MissingRuntimeAsset {
                path: path.to_path_buf(),
            });
        }
        // `tokenizers`' error is a `Box<dyn Error + Send + Sync>`, which is
        // not a domain error and must not escape the crate.
        let inner = Tokenizer::from_file(path).map_err(|error| {
            VoiceMeError::SpeechEngine(format!("could not load {}: {error}", path.display()))
        })?;
        Ok(Self { inner })
    }

    /// Encode `text` for `language`, template and all.
    ///
    /// `language` is lowercased before it becomes a tag, so `"TR"` and
    /// `"tr"` are the same request rather than one of them silently
    /// tokenizing as unknown characters.
    pub fn encode(&self, text: &str, language: &str) -> Result<Vec<i64>, VoiceMeError> {
        if text.trim().is_empty() {
            return Err(VoiceMeError::EmptyText);
        }

        let tagged = format!("[{}]{}", language.to_lowercase(), text);
        let encoding = self.inner.encode(tagged, true).map_err(|error| {
            VoiceMeError::SpeechEngine(format!("could not tokenize the prompt: {error}"))
        })?;

        let ids: Vec<i64> = encoding.get_ids().iter().map(|&id| i64::from(id)).collect();

        // The export's `TemplateProcessing` wraps every sequence as
        // `[6563, 255, …text…, 0, 6561, 6561]`, and the decode loop depends
        // on that shape: `prefill_positions` gives index 0 position `-1`
        // unless it holds a token >= 6561, and a negative position is an
        // out-of-range gather into `text_pos_emb`. The tokenizer and the
        // graphs in this repo have drifted apart before (HF discussions #4,
        // #10), and the file loaded here is whatever was provisioned into
        // the cache — not the fixture the tests pin. So check it, rather
        // than discovering the drift as a crash or as quietly wrong audio.
        if !template_is_intact(&ids) {
            return Err(VoiceMeError::SpeechEngine(format!(
                "tokenizer.json does not produce the template this model export expects \
                 (got {:?}… …{:?}, expected to start with a token >= {SPEECH_VOCAB_SIZE} and \
                 end with two {START_SPEECH_TOKEN}s) — the provisioned tokenizer and the \
                 ONNX graphs are out of step",
                &ids[..ids.len().min(2)],
                &ids[ids.len().saturating_sub(2)..],
            )));
        }

        Ok(ids)
    }
}

/// Where `tokenizer.json` lives inside the model cache directory: at the
/// repo root, not under `onnx/`.
pub fn tokenizer_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("tokenizer.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exports_own_template_passes_validation() {
        // EXAGGERATION, BOS, one text token, EOS, two START_SPEECH.
        assert!(template_is_intact(&[6563, 255, 42, 0, 6561, 6561]));
    }

    #[test]
    fn a_prompt_not_starting_with_a_special_token_is_rejected() {
        // This is the shape that would hand `prefill_positions` an index-0
        // token below 6561 and make it emit position -1 — an out-of-range
        // gather into `text_pos_emb` rather than an obvious failure.
        assert!(!template_is_intact(&[255, 42, 0, 6561, 6561]));
    }

    #[test]
    fn a_prompt_not_ending_in_two_start_speech_tokens_is_rejected() {
        assert!(!template_is_intact(&[6563, 255, 42, 0, 6561]));
        assert!(!template_is_intact(&[6563, 255, 42, 0]));
    }

    #[test]
    fn a_degenerate_encoding_is_rejected_rather_than_indexing_out_of_bounds() {
        assert!(!template_is_intact(&[]));
        assert!(!template_is_intact(&[6561]));
    }

    /// The tokenizer from the pinned revision
    /// `452d3f434aa592098f1eedac9099f33642ab2da5`, vendored (72 KB, MIT) so
    /// this test runs with no model cache and no network — and so a future
    /// bump of the pin has to come past these assertions. The tokenizer and
    /// the graphs have drifted against each other before.
    fn fixture() -> TextTokenizer {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tokenizer.json");
        TextTokenizer::load(&path).unwrap()
    }

    const EXAGGERATION: i64 = 6563;
    const BOS: i64 = 255;
    const EOS: i64 = 0;
    const START_SPEECH: i64 = 6561;

    #[test]
    fn the_template_wraps_the_text_exactly_as_the_export_expects() {
        let ids = fixture().encode("Merhaba, bugün nasılsın?", "tr").unwrap();

        assert_eq!(ids[0], EXAGGERATION, "[6563, …]");
        assert_eq!(ids[1], BOS, "[…, 255, …]");
        assert_eq!(
            &ids[ids.len() - 3..],
            &[EOS, START_SPEECH, START_SPEECH],
            "…, 0, 6561, 6561] — two START_SPEECH, not one"
        );
        assert!(
            ids.len() > 5,
            "the text itself has to survive between the wrappers"
        );
    }

    #[test]
    fn the_language_tag_is_a_single_token() {
        let tokenizer = fixture();

        // Position 2, right after EXAGGERATION and BOS: the tag must occupy
        // exactly one slot, or the model is being told to say "[tr]" out
        // loud instead of being told which language this is.
        let turkish = tokenizer.encode("a", "tr").unwrap();
        assert_eq!(turkish[2], 712, "[tr] = 712");

        let english = tokenizer.encode("a", "en").unwrap();
        assert_eq!(english[2], 708, "[en] = 708");
    }

    #[test]
    fn the_language_code_is_case_insensitive() {
        let tokenizer = fixture();
        assert_eq!(
            tokenizer.encode("a", "TR").unwrap(),
            tokenizer.encode("a", "tr").unwrap()
        );
    }

    #[test]
    fn empty_text_is_rejected_before_anything_runs() {
        let tokenizer = fixture();
        assert!(matches!(
            tokenizer.encode("", "tr"),
            Err(VoiceMeError::EmptyText)
        ));
        assert!(matches!(
            tokenizer.encode("   \n ", "tr"),
            Err(VoiceMeError::EmptyText)
        ));
    }

    #[test]
    fn a_missing_tokenizer_names_the_path() {
        let path = Path::new("/nonexistent/voice-me/tokenizer.json");
        assert!(
            matches!(TextTokenizer::load(path), Err(VoiceMeError::MissingRuntimeAsset { path: p }) if p == path)
        );
    }

    #[test]
    fn the_tokenizer_path_sits_at_the_cache_root_not_under_onnx() {
        let path = tokenizer_path(Path::new("/cache"));
        assert_eq!(path, Path::new("/cache/tokenizer.json"));
    }
}
