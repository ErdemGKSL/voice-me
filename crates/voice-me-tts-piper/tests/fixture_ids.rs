//! The reference phoneme ids, captured from Piper 1.8.0 with the raw
//! per-clause `espeak-ng --ipa` output beside them: every case must give
//! exactly those ids, sentence by sentence. No program and no model needed.

use serde_json::Value;
use voice_me_tts_piper::PiperConfig;
use voice_me_tts_piper::phonemes::{sentence_phonemes, split_clauses, to_ids};

const CASES: &str = include_str!("fixtures/piper_ids.json");
/// Every Piper espeak voice shares this map; fahrettin's is used.
const CONFIG: &str = include_str!("fixtures/tr_TR-fahrettin-medium.config.json");

#[test]
fn every_captured_case_gives_the_reference_ids() {
    let config = PiperConfig::from_json(CONFIG).unwrap();
    let cases: Vec<Value> = serde_json::from_str(CASES).unwrap();
    assert_eq!(cases.len(), 3);

    for case in &cases {
        let text = case["text"].as_str().unwrap();
        let by_clause = case["espeak_ipa_by_clause"].as_object().unwrap();
        let clauses = split_clauses(text);
        let ipa: Vec<String> = clauses
            .iter()
            .map(|clause| {
                by_clause
                    .get(&clause.text)
                    .unwrap_or_else(|| panic!("{text:?}: no capture for clause {:?}", clause.text))
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(clauses.len(), by_clause.len(), "{text:?}: {clauses:?}");

        let got: Vec<Vec<i64>> = sentence_phonemes(&clauses, &ipa)
            .iter()
            .map(|sentence| to_ids(sentence, &config.phoneme_id_map))
            .collect();
        let expected: Vec<Vec<i64>> = serde_json::from_value(case["sentence_ids"].clone()).unwrap();
        assert_eq!(got, expected, "{text:?}");
    }
}
