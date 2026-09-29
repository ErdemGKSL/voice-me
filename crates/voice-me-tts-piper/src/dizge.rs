//! DizgeBERT (`iatagun/dizge-g2p`) run in-process on ONNX Runtime: the
//! Turkish grapheme-to-phoneme model Turkish Piper voices are phonemized
//! through instead of eSpeak NG.
//!
//! The model is a BERT token classifier over single letters: each word is
//! `[CLS]`, one token per letter, `[SEP]`, and each letter's token is
//! labelled with its phoneme. The graph voice-me downloads is the original
//! weights exported to ONNX with the word-embedding table cut down to the
//! letters it ever reads and its matrices quantized to int8;
//! `vocab.json` holds those letters' token ids and the 87 labels.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::TensorRef;
use serde::Deserialize;
use voice_me_core::assets::{self, TurkishG2pFiles};

use crate::turkish;
use crate::{Phonemizer, RuntimeInit};

/// The model's `vocab.json`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DizgeVocab {
    pub pad: i64,
    pub unk: i64,
    pub cls: i64,
    pub sep: i64,
    /// The longest input, `[CLS]` and `[SEP]` included.
    pub max_length: usize,
    /// Each letter's token id.
    pub chars: HashMap<char, i64>,
    /// Each class's label, by class index.
    pub labels: Vec<String>,
}

impl DizgeVocab {
    /// Parse `vocab.json`; `Err` is the reason, in words.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let vocab: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if vocab.labels.is_empty() {
            return Err("it has no labels".to_string());
        }
        if vocab.max_length < 3 {
            return Err(format!("its longest input is {}", vocab.max_length));
        }
        Ok(vocab)
    }

    /// The most letters one word can have: the longest input less `[CLS]`
    /// and `[SEP]`.
    pub fn max_letters(&self) -> usize {
        self.max_length - 2
    }

    /// A batch of words as the graph's `input_ids` and `attention_mask`,
    /// both `[words, longest + 2]`, row-major, padded with `[PAD]`.
    pub fn encode(&self, words: &[String]) -> (Vec<i64>, Vec<i64>, usize) {
        let width = words
            .iter()
            .map(|word| word.chars().count().min(self.max_letters()))
            .max()
            .unwrap_or(0)
            + 2;
        let mut ids = vec![self.pad; words.len() * width];
        let mut mask = vec![0_i64; words.len() * width];
        for (row, word) in words.iter().enumerate() {
            let tokens = std::iter::once(self.cls)
                .chain(
                    word.chars()
                        .take(self.max_letters())
                        .map(|letter| self.chars.get(&letter).copied().unwrap_or(self.unk)),
                )
                .chain(std::iter::once(self.sep));
            for (column, token) in tokens.enumerate() {
                ids[row * width + column] = token;
                mask[row * width + column] = 1;
            }
        }
        (ids, mask, width)
    }

    /// Each word's labels, one per letter, from the graph's logits
    /// (`[words, width, classes]`, row-major): the best class of each
    /// letter's token, skipping `[CLS]`.
    pub fn decode(
        &self,
        words: &[String],
        logits: &[f32],
        width: usize,
    ) -> Result<Vec<Vec<String>>, String> {
        let classes = self.labels.len();
        if logits.len() != words.len() * width * classes {
            return Err(format!(
                "its output has {} values, not {} words × {width} tokens × {classes} labels",
                logits.len(),
                words.len()
            ));
        }
        Ok(words
            .iter()
            .enumerate()
            .map(|(row, word)| {
                (0..word.chars().count().min(self.max_letters()))
                    .map(|letter| {
                        let at = (row * width + letter + 1) * classes;
                        let scores = &logits[at..at + classes];
                        let best = scores
                            .iter()
                            .enumerate()
                            .fold((0, f32::NEG_INFINITY), |best, (class, &score)| {
                                if score > best.1 { (class, score) } else { best }
                            })
                            .0;
                        self.labels[best].clone()
                    })
                    .collect()
            })
            .collect())
    }
}

/// The loaded model.
struct Loaded {
    session: Session,
    vocab: DizgeVocab,
    /// What its files were when loaded: an update that replaced them makes
    /// the session stale.
    stamp: Option<[(u64, std::time::SystemTime); 2]>,
}

/// The Turkish G2P model under the cache root, loaded on first use and
/// kept. CPU only: it is small, and runs once per clause.
pub struct DizgeG2p {
    files: TurkishG2pFiles,
    runtime_init: RuntimeInit,
    loaded: Mutex<Option<Loaded>>,
}

impl DizgeG2p {
    /// The model under the cache root `root`; `runtime_init` commits ONNX
    /// Runtime (the same closure Piper's own session is built after).
    pub fn new(root: &Path, runtime_init: RuntimeInit) -> Self {
        Self {
            files: assets::turkish_g2p_files(root),
            runtime_init,
            loaded: Mutex::new(None),
        }
    }

    fn stamp(&self) -> Option<[(u64, std::time::SystemTime); 2]> {
        let stamp = |path: &Path| {
            let meta = std::fs::metadata(path).ok()?;
            Some((meta.len(), meta.modified().ok()?))
        };
        Some([stamp(&self.files.model)?, stamp(&self.files.vocab)?])
    }

    fn load(&self) -> Result<Loaded, String> {
        for path in [&self.files.model, &self.files.vocab] {
            if !path.is_file() {
                return Err(format!(
                    "the Turkish G2P model is not installed ({} is missing); install it from \
                     the Dependency Check",
                    path.display()
                ));
            }
        }
        (self.runtime_init)().map_err(|error| error.to_string())?;
        let stamp = self.stamp();
        let vocab = std::fs::read_to_string(&self.files.vocab)
            .map_err(|error| error.to_string())
            .and_then(|json| DizgeVocab::from_json(&json))
            .map_err(|reason| format!("could not read {}: {reason}", self.files.vocab.display()))?;
        let engine =
            |error: ort::Error| format!("could not load {}: {error}", self.files.model.display());
        let session = Session::builder()
            .map_err(engine)?
            .with_execution_providers([ort::ep::CPU::default().build()])
            .map_err(|error| engine(error.into()))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|error| engine(error.into()))?
            .with_intra_threads(4)
            .map_err(|error| engine(error.into()))?
            .commit_from_file(&self.files.model)
            .map_err(engine)?;
        Ok(Loaded {
            session,
            vocab,
            stamp,
        })
    }

    /// One clause's phonemes, the way `espeak-ng --ipa` would print them.
    pub fn phonemize(&self, clause: &str) -> Result<String, String> {
        let mut held = self
            .loaded
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if held
            .as_ref()
            .is_none_or(|loaded| loaded.stamp != self.stamp())
        {
            *held = None;
            *held = Some(self.load()?);
        }
        let Loaded { session, vocab, .. } = held.as_mut().expect("just loaded");
        turkish::phonemize_clause(clause, vocab.max_letters(), |words| {
            run(session, vocab, words)
        })
    }
}

/// One run of the graph over a batch of words.
fn run(
    session: &mut Session,
    vocab: &DizgeVocab,
    words: &[String],
) -> Result<Vec<Vec<String>>, String> {
    let failed =
        |what: &str, error: ort::Error| format!("the Turkish G2P model failed at {what}: {error}");
    let (ids, mask, width) = vocab.encode(words);
    let shape = vec![words.len() as i64, width as i64];
    let input_ids = TensorRef::from_array_view((shape.clone(), ids.as_slice()))
        .map_err(|error| failed("input_ids", error))?;
    let attention_mask = TensorRef::from_array_view((shape, mask.as_slice()))
        .map_err(|error| failed("attention_mask", error))?;
    let outputs = session
        .run(ort::inputs![
            "input_ids" => input_ids,
            "attention_mask" => attention_mask,
        ])
        .map_err(|error| failed("its graph", error))?;
    let (_, logits) = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|error| failed("its output", error))?;
    vocab.decode(words, logits, width)
}

/// The phonemizer Piper voices speak through: DizgeBERT for a Turkish
/// voice (`espeak.voice` of `tr`), and `other` — `espeak-ng` — for every
/// other language.
pub struct TurkishG2pPhonemizer {
    dizge: DizgeG2p,
    other: Arc<dyn Phonemizer>,
}

impl TurkishG2pPhonemizer {
    pub fn new(dizge: DizgeG2p, other: Arc<dyn Phonemizer>) -> Self {
        Self { dizge, other }
    }
}

/// Whether `espeak_voice` (a voice's `espeak.voice`) is Turkish.
pub fn is_turkish(espeak_voice: &str) -> bool {
    let language = espeak_voice.split(['-', '_']).next().unwrap_or_default();
    language.eq_ignore_ascii_case("tr")
}

impl Phonemizer for TurkishG2pPhonemizer {
    fn phonemize(&self, espeak_voice: &str, clause: &str) -> Result<String, String> {
        if is_turkish(espeak_voice) {
            self.dizge.phonemize(clause)
        } else {
            self.other.phonemize(espeak_voice, clause)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab() -> DizgeVocab {
        DizgeVocab::from_json(
            r#"{"pad":0,"unk":1,"cls":2,"sep":3,"max_length":5,
                "chars":{"a":4,"b":5},"labels":["a","b","ɑ"]}"#,
        )
        .unwrap()
    }

    #[test]
    fn words_are_encoded_between_cls_and_sep_and_padded() {
        let (ids, mask, width) = vocab().encode(&["ab".to_string(), "x".to_string()]);
        assert_eq!(width, 4);
        assert_eq!(ids, vec![2, 4, 5, 3, 2, 1, 3, 0]);
        assert_eq!(mask, vec![1, 1, 1, 1, 1, 1, 1, 0]);
    }

    #[test]
    fn a_word_longer_than_the_input_is_cut() {
        let (ids, _, width) = vocab().encode(&["abab".to_string()]);
        assert_eq!(width, 5);
        assert_eq!(ids, vec![2, 4, 5, 4, 3]);
    }

    #[test]
    fn each_letter_takes_its_best_label() {
        let vocab = vocab();
        let words = ["ab".to_string()];
        #[rustfmt::skip]
        let logits = [
            9.0, 0.0, 0.0, // [CLS]
            0.0, 0.1, 0.9, // a → ɑ
            0.0, 0.8, 0.1, // b → b
            9.0, 0.0, 0.0, // [SEP]
        ];
        assert_eq!(
            vocab.decode(&words, &logits, 4).unwrap(),
            vec![vec!["ɑ".to_string(), "b".to_string()]]
        );
        assert!(vocab.decode(&words, &logits[..9], 4).is_err());
    }

    #[test]
    fn a_vocab_with_no_labels_is_refused() {
        assert!(
            DizgeVocab::from_json(
                r#"{"pad":0,"unk":1,"cls":2,"sep":3,"max_length":64,"chars":{},"labels":[]}"#
            )
            .is_err()
        );
        assert!(DizgeVocab::from_json("not json").is_err());
    }

    #[test]
    fn only_turkish_voices_are_phonemized_by_the_model() {
        assert!(is_turkish("tr"));
        assert!(is_turkish("TR-tr"));
        assert!(!is_turkish("en-us"));
        assert!(!is_turkish("trk"));

        struct Echo;
        impl Phonemizer for Echo {
            fn phonemize(&self, voice: &str, clause: &str) -> Result<String, String> {
                Ok(format!("{voice}:{clause}"))
            }
        }
        let root = tempfile::tempdir().unwrap();
        let phonemizer = TurkishG2pPhonemizer::new(
            DizgeG2p::new(
                root.path(),
                Arc::new(|| panic!("no runtime is loaded for a model that is not there")),
            ),
            Arc::new(Echo),
        );
        assert_eq!(
            phonemizer.phonemize("en-us", "hello").unwrap(),
            "en-us:hello"
        );
        let error = phonemizer.phonemize("tr", "merhaba").unwrap_err();
        assert!(
            error.contains("Turkish G2P model is not installed"),
            "{error}"
        );
        assert!(
            error.contains("vocab.json") || error.contains("model.onnx"),
            "{error}"
        );
    }
}
