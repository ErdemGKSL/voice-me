//! The prompt list: one `id|text` line per sentence.

/// One sentence to read aloud.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sentence {
    /// The file stem its recording is saved under (`00042`).
    pub id: String,
    pub text: String,
}

/// `text` as it is shown, spoken and written to `metadata.csv`: a lowercased
/// capital `İ` leaves a combining dot after the `i` (`i̇yi`), which is
/// removed, and whitespace is collapsed. `|` would break `metadata.csv`'s
/// columns, so it becomes a comma.
pub fn clean_text(text: &str) -> String {
    let text = text.replace("i\u{307}", "i").replace("I\u{307}", "İ");
    let text = text.replace('|', ",");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse `id|text` lines. A byte-order mark, blank lines, lines without a
/// `|`, empty texts and repeated ids are skipped.
pub fn parse(csv: &str) -> Vec<Sentence> {
    let mut seen = std::collections::HashSet::new();
    csv.trim_start_matches('\u{feff}')
        .lines()
        .filter_map(|line| {
            let (id, text) = line.split_once('|')?;
            let id = id.trim();
            let text = clean_text(text);
            let safe_id =
                !id.is_empty() && id.chars().all(|ch| ch.is_alphanumeric() || ch == '_' || ch == '-');
            (safe_id && !text.is_empty() && seen.insert(id.to_string())).then(|| Sentence {
                id: id.to_string(),
                text,
            })
        })
        .collect()
}

/// The sentences in an order that teaches the most first: each pick is the
/// one adding the most not-yet-seen words (weight 3) and letter pairs inside
/// or at the edges of words (weight 1) *per character*, so the longest lines
/// do not lead just for having more to cover, and so a few hundred takes already cover
/// the language the 20 000 templated lines repeat. Ties keep list order, so
/// the result is the same on every PC (a resumed or imported session lines
/// up). Lazy greedy: coverage only shrinks a sentence's gain, so a stale
/// score is an upper bound.
pub fn order_for_coverage(sentences: Vec<Sentence>) -> Vec<Sentence> {
    use std::cmp::Reverse;
    use std::collections::{BinaryHeap, HashSet};
    use std::hash::{DefaultHasher, Hash, Hasher};

    let hash = |value: &str| {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    };
    // (feature, weight) for every distinct feature of every sentence.
    let features: Vec<Vec<(u64, u32)>> = sentences
        .iter()
        .map(|sentence| {
            let mut seen = HashSet::new();
            let mut out = Vec::new();
            for word in sentence
                .text
                .split(|ch: char| !ch.is_alphabetic())
                .filter(|word| !word.is_empty())
            {
                let word = word.replace('I', "ı").replace('İ', "i").to_lowercase();
                let padded: Vec<char> = std::iter::once('^')
                    .chain(word.chars())
                    .chain(std::iter::once('$'))
                    .collect();
                let mut add = |key: String, weight: u32| {
                    let id = hash(&key);
                    if seen.insert(id) {
                        out.push((id, weight));
                    }
                };
                add(format!("w:{word}"), 3);
                for pair in padded.windows(2) {
                    add(format!("b:{}{}", pair[0], pair[1]), 1);
                }
            }
            out
        })
        .collect();

    let gain = |index: usize, covered: &HashSet<u64>| -> u32 {
        features[index]
            .iter()
            .filter(|(id, _)| !covered.contains(id))
            .map(|(_, weight)| weight)
            .sum()
    };
    let lengths: Vec<u64> = sentences
        .iter()
        .map(|s| (s.text.chars().count() as u64).max(1))
        .collect();
    // Fixed-point gain per character, so the heap compares plain integers.
    let score = |index: usize, covered: &HashSet<u64>| -> u64 {
        u64::from(gain(index, covered)) * 1_000_000 / lengths[index]
    };
    let mut covered = HashSet::new();
    let mut heap: BinaryHeap<(u64, Reverse<usize>)> = (0..sentences.len())
        .map(|index| (score(index, &covered), Reverse(index)))
        .collect();
    let mut order = Vec::with_capacity(sentences.len());
    while let Some((stale, Reverse(index))) = heap.pop() {
        let fresh = score(index, &covered);
        if fresh < stale {
            heap.push((fresh, Reverse(index)));
            continue;
        }
        covered.extend(features[index].iter().map(|(id, _)| *id));
        order.push(index);
    }
    let mut slots: Vec<Option<Sentence>> = sentences.into_iter().map(Some).collect();
    order.into_iter().filter_map(|index| slots[index].take()).collect()
}

/// One short line is placed after every this many long ones.
const LONG_PER_SHORT: usize = 3;

/// The bundled prompts: the 20 000 long sentences and the short phrases,
/// questions and exclamations, each ordered for coverage and interleaved
/// (three long, one short) so short utterances appear from the first takes.
pub fn bundled() -> Vec<Sentence> {
    let long = order_for_coverage(parse(crate::BUNDLED_SENTENCES));
    let short = order_for_coverage(parse(crate::BUNDLED_SHORT));
    let mut long = long.into_iter();
    let mut short = short.into_iter();
    let mut out = Vec::new();
    loop {
        let before = out.len();
        out.extend(long.by_ref().take(LONG_PER_SHORT));
        out.extend(short.next());
        if out.len() == before {
            break;
        }
    }
    out.extend(short);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_become_sentences_and_junk_is_skipped() {
        let csv = "\u{feff}00001|Merhaba dünya.\n\nnobar\n00002|  \n00001|again\n../x|bad id\n00003|İyi  kalpli\n";
        let sentences = parse(csv);
        assert_eq!(
            sentences,
            vec![
                Sentence { id: "00001".into(), text: "Merhaba dünya.".into() },
                Sentence { id: "00003".into(), text: "İyi kalpli".into() },
            ]
        );
    }

    #[test]
    fn a_stray_combining_dot_is_removed() {
        assert_eq!(clean_text("o i\u{307}yi a|b"), "o iyi a,b");
    }

    #[test]
    fn the_bundled_sentences_parse_to_twenty_thousand() {
        let sentences = parse(crate::BUNDLED_SENTENCES);
        assert_eq!(sentences.len(), 20_000);
        assert!(sentences.iter().all(|s| !s.text.contains('\u{307}')));
        let short = parse(crate::BUNDLED_SHORT);
        assert_eq!(short.len(), 248);
        assert!(
            short.iter().all(|s| (12..=60).contains(&s.text.chars().count())),
            "short lines stay short"
        );
    }

    #[test]
    fn coverage_order_puts_new_words_first_and_is_stable() {
        let line = |id: &str, text: &str| Sentence { id: id.into(), text: text.into() };
        let list = vec![
            line("a", "bir bir bir"),
            line("b", "bir iki"),
            line("c", "üç dört beş altı"),
            line("d", "bir iki"),
        ];
        let ordered = order_for_coverage(list.clone());
        let ids: Vec<&str> = ordered.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["b", "c", "a", "d"], "most new per character first, repeats last, ties by list order");
        assert_eq!(ordered, order_for_coverage(list), "the order is deterministic");
    }

    /// Writes the bundled prompts, in the order the recorder shows them, to
    /// the file named by `VOICE_ME_DUMP` — what the phone app ships as
    /// `sentences.csv`, so both apps agree on the order:
    ///
    /// `VOICE_ME_DUMP=recorder-mobile/sentences.csv cargo test --release -p
    /// voice-me-recorder dump_bundled_order -- --ignored`
    #[test]
    #[ignore = "writes a file; run on purpose"]
    fn dump_bundled_order() {
        let path = std::env::var("VOICE_ME_DUMP").expect("set VOICE_ME_DUMP to the output path");
        let lines: String = bundled().iter().map(|s| format!("{}|{}\n", s.id, s.text)).collect();
        std::fs::write(path, lines).unwrap();
    }

    #[test]
    fn the_bundled_prompts_are_ordered_interleaved_and_complete() {
        let words = |sentences: &[Sentence]| -> std::collections::HashSet<String> {
            sentences
                .iter()
                .flat_map(|s| s.text.split_whitespace().map(str::to_lowercase))
                .collect()
        };
        let all = bundled();
        assert_eq!(all.len(), 20_000 + 248);
        let ids: std::collections::HashSet<&str> = all.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids.len(), all.len(), "ids are unique across both lists");
        let shorts = all[..8].iter().filter(|s| s.id.starts_with('k')).count();
        assert_eq!(shorts, 2, "one short line after every three long ones");
        let ordered = words(&all[..500]).len();
        let sequential = words(&parse(crate::BUNDLED_SENTENCES)[..500]).len();
        assert!(ordered > sequential, "{ordered} words in the first 500 vs {sequential} in list order");
    }
}
