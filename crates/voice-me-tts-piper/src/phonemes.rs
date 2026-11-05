//! Text → clauses → phonemes → ids, the way Piper 1.8.0 does it.
//!
//! Pure: `espeak-ng` is asked for each clause's IPA by the caller, and what
//! it printed comes in here as a string. That is what lets the reference
//! ids in `tests/fixtures/piper_ids.json` be checked with no program at all.
//!
//! The `espeak-ng` program drops clause punctuation, which the voices were
//! trained with, so the text is split at `, : ; . ! ?` first, each clause is
//! phonemized alone, and its mark is put back after it.

use std::collections::HashMap;

use unicode_normalization::UnicodeNormalization as _;

/// The marks a clause ends at, when whitespace or the end of the text
/// follows them.
const CLAUSE_MARKS: [char; 6] = [',', ':', ';', '.', '!', '?'];

/// The marks that also end a sentence. The end of the text does too.
const SENTENCE_MARKS: [char; 3] = ['.', '!', '?'];

/// The marks followed by a space when put back.
const SPACED_MARKS: [char; 3] = [',', ':', ';'];

/// Beginning of sentence, end of sentence, and the pad between ids.
const BOS: &str = "^";
const EOS: &str = "$";
const PAD: &str = "_";

/// One clause of the text: what is phonemized, and the mark that ended it
/// (`None` for a trailing clause with no mark).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clause {
    pub text: String,
    pub terminator: Option<char>,
}

impl Clause {
    /// Whether this clause closes its sentence: a sentence mark, or none at
    /// all (the end of the text).
    pub fn ends_sentence(&self) -> bool {
        self.terminator
            .is_none_or(|mark| SENTENCE_MARKS.contains(&mark))
    }
}

/// Split `text` into clauses at `, : ; . ! ?` followed by whitespace or the
/// end of the text. A mark inside a word or number (`1,500`, `e.g.x`) is
/// part of the clause. Clauses with nothing to say are dropped, except that
/// a lone mark still ends the clause before it.
pub fn split_clauses(text: &str) -> Vec<Clause> {
    let chars: Vec<char> = text.chars().collect();
    let mut clauses = Vec::new();
    let mut current = String::new();
    for (index, &ch) in chars.iter().enumerate() {
        let boundary = CLAUSE_MARKS.contains(&ch)
            && chars.get(index + 1).is_none_or(|next| next.is_whitespace());
        if boundary {
            let clause = current.trim().to_string();
            current.clear();
            if !clause.is_empty() {
                clauses.push(Clause {
                    text: clause,
                    terminator: Some(ch),
                });
            }
        } else {
            current.push(ch);
        }
    }
    let rest = current.trim();
    if !rest.is_empty() {
        clauses.push(Clause {
            text: rest.to_string(),
            terminator: None,
        });
    }
    clauses
}

/// What `espeak-ng --ipa` printed for one clause, cleaned the way Piper
/// cleans it: every non-empty line trimmed and joined with a space, then
/// every `(…)` language-switch marker removed.
pub fn clean_ipa(raw: &str) -> String {
    let joined = raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    strip_parenthesised(&joined)
}

/// Remove every `\([^)]+\)`: an opening parenthesis, at least one
/// character that is not a closing one, and the closing one. An unmatched
/// or empty pair is left as it is.
fn strip_parenthesised(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('(') {
        let after = &rest[open + 1..];
        match after.find(')') {
            Some(close) if close > 0 => {
                out.push_str(&rest[..open]);
                rest = &after[close + 1..];
            }
            _ => {
                out.push_str(&rest[..=open]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The sentences `clauses` make, each as its phoneme codepoints (NFD).
/// `ipa` is what `espeak-ng` printed for each clause, in step with
/// `clauses`.
pub fn sentence_phonemes(clauses: &[Clause], ipa: &[String]) -> Vec<Vec<char>> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    for (clause, raw) in clauses.iter().zip(ipa) {
        current.push_str(&clean_ipa(raw));
        if let Some(mark) = clause.terminator {
            current.push(mark);
            if SPACED_MARKS.contains(&mark) {
                current.push(' ');
            }
        }
        if clause.ends_sentence() {
            sentences.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        sentences.push(current);
    }
    sentences
        .into_iter()
        .map(|sentence| sentence.nfd().collect::<Vec<char>>())
        .filter(|phonemes| !phonemes.is_empty())
        .collect()
}

/// A voice's phoneme → ids map, as its `config.json` gives it.
pub type PhonemeIdMap = HashMap<String, Vec<i64>>;

/// One sentence's ids: BOS, a pad, then each phoneme's ids followed by a
/// pad, then EOS. A phoneme the map lacks is skipped.
pub fn to_ids(phonemes: &[char], map: &PhonemeIdMap) -> Vec<i64> {
    let ids_of = |symbol: &str| map.get(symbol).map(Vec::as_slice).unwrap_or(&[]);
    let pad = ids_of(PAD);
    let mut ids = Vec::with_capacity(phonemes.len() * 2 + 3);
    ids.extend_from_slice(ids_of(BOS));
    ids.extend_from_slice(pad);
    let mut buffer = [0_u8; 4];
    for phoneme in phonemes {
        let Some(phoneme_ids) = map.get(&*phoneme.encode_utf8(&mut buffer)) else {
            continue;
        };
        ids.extend_from_slice(phoneme_ids);
        ids.extend_from_slice(pad);
    }
    ids.extend_from_slice(ids_of(EOS));
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clause(text: &str, terminator: Option<char>) -> Clause {
        Clause {
            text: text.to_string(),
            terminator,
        }
    }

    #[test]
    fn a_number_with_a_comma_stays_one_clause() {
        assert_eq!(
            split_clauses("I have 1,500 coins; don't spend them"),
            vec![
                clause("I have 1,500 coins", Some(';')),
                clause("don't spend them", None),
            ]
        );
    }

    #[test]
    fn trailing_text_without_a_mark_is_its_own_clause() {
        assert_eq!(
            split_clauses("Merhaba Erdem, bu yerel"),
            vec![clause("Merhaba Erdem", Some(',')), clause("bu yerel", None),]
        );
        assert_eq!(
            split_clauses("  Bitti.  "),
            vec![clause("Bitti", Some('.'))]
        );
    }

    #[test]
    fn every_mark_splits_before_whitespace_or_the_end() {
        let clauses = split_clauses("a, b: c; d. e! f? g");
        let marks: Vec<_> = clauses.iter().map(|c| c.terminator).collect();
        assert_eq!(
            marks,
            vec![
                Some(','),
                Some(':'),
                Some(';'),
                Some('.'),
                Some('!'),
                Some('?'),
                None
            ]
        );
        assert!(split_clauses("  ").is_empty());
        assert!(split_clauses(" . ").is_empty());
    }

    #[test]
    fn language_switch_markers_and_line_breaks_are_cleaned() {
        assert_eq!(clean_ipa(" həlˈoʊ\n(en)wˈɜːld(tr) \n\n"), "həlˈoʊ wˈɜːld");
        assert_eq!(clean_ipa("a () b ("), "a () b (");
    }

    #[test]
    fn marks_are_put_back_and_sentences_close_at_their_marks() {
        let clauses = vec![
            clause("a", Some(',')),
            clause("b", Some('!')),
            clause("c", None),
        ];
        let ipa = vec!["a\n".to_string(), "b\n".to_string(), "c\n".to_string()];
        assert_eq!(
            sentence_phonemes(&clauses, &ipa),
            vec![vec!['a', ',', ' ', 'b', '!'], vec!['c']]
        );
    }

    #[test]
    fn phonemes_are_decomposed_to_nfd() {
        let clauses = vec![clause("x", None)];
        // U+00E7 (ç) decomposes to c + U+0327.
        let phonemes = sentence_phonemes(&clauses, &["\u{e7}".to_string()]);
        assert_eq!(phonemes, vec![vec!['c', '\u{327}']]);
    }

    fn tiny_map() -> PhonemeIdMap {
        [("_", 0), ("^", 1), ("$", 2), (" ", 3), ("a", 14), ("b", 15)]
            .into_iter()
            .map(|(symbol, id)| (symbol.to_string(), vec![id]))
            .collect()
    }

    #[test]
    fn ids_run_bos_pad_each_id_and_a_pad_then_eos() {
        assert_eq!(
            to_ids(&['a', ' ', 'b'], &tiny_map()),
            vec![1, 0, 14, 0, 3, 0, 15, 0, 2]
        );
    }

    /// The Missing phoneme row: a symbol the map lacks is skipped, and the
    /// rest still speaks.
    #[test]
    fn a_symbol_missing_from_the_map_is_skipped() {
        assert_eq!(
            to_ids(&['a', '\u{200d}', 'b'], &tiny_map()),
            vec![1, 0, 14, 0, 15, 0, 2]
        );
    }
}
