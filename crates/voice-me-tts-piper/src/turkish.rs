//! Turkish text → Piper phonemes through DizgeBERT (`iatagun/dizge-g2p`).
//!
//! Pure: the model's run is abstracted as a closure, so everything here is
//! tested with no model at all. The pipeline for one clause:
//!
//! 1. the clause is lowercased the Turkish way (`I` → `ı`, `İ` → `i`),
//!    numbers are spelled out in Turkish, and it is split into words of
//!    letters the model knows ([`words`]);
//! 2. the model labels each letter of each word with its phoneme (one label
//!    per letter, possibly empty-sounding like `ː` for `ğ`);
//! 3. a word whose labels do not line up with its letters — the model's
//!    known failure on some `ğ` words (`olduğunu`) — is spelled by rule
//!    instead ([`rule_phonemes`]);
//! 4. the labels are rewritten into the symbols Turkish Piper voices were
//!    trained on — eSpeak NG's Turkish inventory — and the word's stress
//!    mark is put before its last vowel ([`to_voice_symbols`]).
//!
//! The words are joined with a space, the way `espeak-ng --ipa` prints
//! them, so the rest of the Piper pipeline is unchanged.

use unicode_normalization::UnicodeNormalization as _;

/// The letters the model reads; anything else is dropped from a word.
pub const LETTERS: &str = "abcçdefgğhıijklmnoöpqrsştuüvwxyzâîû";

/// The Turkish vowels, as letters.
const VOWEL_LETTERS: &str = "aeıioöuüâîû";

/// The front vowels, as letters: they palatalize `g`, `k` and `l`.
const FRONT_VOWEL_LETTERS: &str = "eiöüî";

/// The symbols that are vowels in the model's labels.
const LABEL_VOWELS: &str = "IUYaeioøœuyɑɔɛɨ";

/// The symbols that are vowels in the voice's inventory: where a stress
/// mark may go.
const VOICE_VOWELS: &str = "aeɛæiɪoɔuʊyøœɯ";

/// The stress mark, as eSpeak NG prints it.
const STRESS: char = 'ˈ';

/// Words spoken without stress of their own: conjunctions and clitics.
const UNSTRESSED: [&str; 11] = [
    "ve", "de", "da", "ki", "mi", "mı", "mu", "mü", "ile", "bu", "şu",
];

/// Lowercase `text` the Turkish way: `I` is `ı` and `İ` is `i`.
pub fn lowercase(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            'I' => out.push('ı'),
            'İ' => out.push('i'),
            other => out.extend(other.to_lowercase()),
        }
    }
    // `İ` lowercased by `to_lowercase` elsewhere leaves a combining dot.
    out.replace("i\u{307}", "i")
}

/// A letter the model knows, for `ch`: itself, or its base letter when it
/// is an accented Latin letter the model does not know (`é` → `e`).
fn known_letter(ch: char) -> Option<char> {
    if LETTERS.contains(ch) {
        return Some(ch);
    }
    let base = ch.nfd().next()?;
    (base != ch && LETTERS.contains(base)).then_some(base)
}

/// The words of one clause, as the model reads them: lowercased, numbers
/// spelled out, apostrophes joined (`Erdem'in` → `erdemin`), and split at
/// everything that is not a letter. Words longer than `max_letters` are
/// cut into pieces of that length.
pub fn words(clause: &str, max_letters: usize) -> Vec<String> {
    let text = spell_numbers(&lowercase(clause));
    let mut words = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if let Some(letter) = known_letter(ch) {
            current.push(letter);
        } else if matches!(ch, '\'' | '’') {
            // A suffix after an apostrophe belongs to its word.
        } else if !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    let max_letters = max_letters.max(1);
    words
        .into_iter()
        .flat_map(|word| {
            let letters: Vec<char> = word.chars().collect();
            letters
                .chunks(max_letters)
                .map(|piece| piece.iter().collect::<String>())
                .collect::<Vec<_>>()
        })
        .collect()
}

// ---- numbers ------------------------------------------------------------

const ONES: [&str; 10] = [
    "", "bir", "iki", "üç", "dört", "beş", "altı", "yedi", "sekiz", "dokuz",
];
const TENS: [&str; 10] = [
    "", "on", "yirmi", "otuz", "kırk", "elli", "altmış", "yetmiş", "seksen", "doksan",
];
const SCALES: [&str; 7] = [
    "",
    "bin",
    "milyon",
    "milyar",
    "trilyon",
    "katrilyon",
    "kentilyon",
];

/// A number below 1000, in words, with no spaces between its parts the way
/// Turkish spells them apart: `yüz yirmi üç`.
fn below_thousand(n: u64, parts: &mut Vec<&'static str>) {
    let (hundreds, rest) = (n / 100, n % 100);
    if hundreds > 1 {
        parts.push(ONES[hundreds as usize]);
    }
    if hundreds > 0 {
        parts.push("yüz");
    }
    if rest / 10 > 0 {
        parts.push(TENS[(rest / 10) as usize]);
    }
    if rest % 10 > 0 {
        parts.push(ONES[(rest % 10) as usize]);
    }
}

/// `n` in Turkish words: `sıfır`, `bin iki yüz`, `iki milyon`.
pub fn number_words(n: u64) -> String {
    if n == 0 {
        return "sıfır".to_string();
    }
    let mut groups = Vec::new();
    let mut rest = n;
    while rest > 0 {
        groups.push(rest % 1000);
        rest /= 1000;
    }
    let mut parts = Vec::new();
    for (scale, &group) in groups.iter().enumerate().rev() {
        if group == 0 {
            continue;
        }
        // "bin", never "bir bin".
        if !(scale == 1 && group == 1) {
            below_thousand(group, &mut parts);
        }
        if scale > 0 {
            parts.push(SCALES[scale]);
        }
    }
    parts.join(" ")
}

/// Digits spelled out: a run of digits (with `.` between groups of three,
/// `1.500`) as one number, and a decimal comma between digits as
/// `virgül`. A run too long for a number, or with leading zeros, is read
/// digit by digit. A suffix after an apostrophe stays on the number
/// (`3'te` → `üçte`).
pub fn spell_numbers(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let digit_at = |index: usize| chars.get(index).is_some_and(char::is_ascii_digit);
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        if !digit_at(index) {
            out.push(chars[index]);
            index += 1;
            continue;
        }
        let mut digits = String::new();
        while digit_at(index) {
            digits.push(chars[index]);
            index += 1;
        }
        // Thousands groups: `.` and exactly three digits, after a first
        // group of at most three.
        if digits.len() <= 3 {
            while chars.get(index) == Some(&'.')
                && (1..=3).all(|offset| digit_at(index + offset))
                && !digit_at(index + 4)
            {
                digits.extend(&chars[index + 1..index + 4]);
                index += 4;
            }
        }
        out.push(' ');
        match digits.parse::<u64>() {
            Ok(n) if digits.len() <= 18 && (digits.len() == 1 || !digits.starts_with('0')) => {
                out.push_str(&number_words(n));
            }
            _ => {
                let spelled: Vec<String> = digits
                    .chars()
                    .map(|digit| number_words(u64::from(digit.to_digit(10).unwrap_or(0))))
                    .collect();
                out.push_str(&spelled.join(" "));
            }
        }
        if chars.get(index) == Some(&',') && digit_at(index + 1) {
            out.push_str(" virgül");
            index += 1;
        }
        if !matches!(chars.get(index), Some('\'' | '’')) {
            out.push(' ');
        }
    }
    out
}

// ---- labels -------------------------------------------------------------

fn is_label_vowel(symbol: char) -> bool {
    LABEL_VOWELS.contains(symbol)
}

/// Whether a label symbol is a consonant: not a vowel, and not one of the
/// marks that only modify the symbol before them.
fn is_label_consonant(symbol: char) -> bool {
    !is_label_vowel(symbol) && !matches!(symbol, 'ː' | 'ʰ' | '\u{325}')
}

/// Whether the model's `labels` line up with `word`'s letters: one label
/// per letter, a vowel's starting with a vowel, a consonant's holding a
/// consonant, `y` a glide or a length mark, `ğ` a length mark, a glide or
/// a vowel. A word that fails is one the model mislabelled — it shifts
/// its labels on some `ğ` words — and is spelled by rule instead.
pub fn labels_line_up(word: &str, labels: &[String]) -> bool {
    let letters: Vec<char> = word.chars().collect();
    if letters.len() != labels.len() {
        return false;
    }
    letters.iter().zip(labels).all(|(&letter, label)| {
        let Some(first) = label.chars().next() else {
            return false;
        };
        match letter {
            'ğ' => matches!(first, 'ː' | 'j' | 'ɣ') || is_label_vowel(first),
            'y' => matches!(first, 'j' | 'I' | 'ː'),
            vowel if VOWEL_LETTERS.contains(vowel) => is_label_vowel(first),
            _ => label.chars().any(is_label_consonant),
        }
    })
}

/// One label symbol in the voice's inventory: eSpeak NG's Turkish symbols,
/// which Turkish Piper voices were trained on. `None` drops it.
fn voice_symbol(symbol: char) -> Option<&'static str> {
    Some(match symbol {
        'I' => "ɪ",
        'U' => "ʊ",
        'Y' => "y",
        'ɑ' => "a",
        'ɨ' => "ɯ",
        'ł' => "ɫ",
        'ʋ' => "v",
        'x' | 'ç' => "h",
        // Dizge writes a word-final `r` as a fricative; eSpeak NG as `r`.
        'ɣ' => "r",
        'g' => "ɡ",
        'c' => "k",
        'ɱ' | 'ŋ' => return nasal(symbol),
        // Aspiration and devoicing are not in the voice's inventory.
        'ʰ' | '\u{325}' | '\u{327}' => return None,
        _ => return None,
    })
}

fn nasal(symbol: char) -> Option<&'static str> {
    Some(if symbol == 'ɱ' { "m" } else { "n" })
}

/// The symbols kept as they are: already in the voice's inventory.
const KEPT: &str = "aeiouyøœɔɛbdfjklmnprstvzʃʒɟɾː";

/// The model's labels for `word`, rewritten into the voice's symbols with
/// the word's stress mark.
pub fn to_voice_symbols(word: &str, labels: &[String]) -> String {
    let mut out = String::new();
    for (letter, label) in word.chars().zip(labels) {
        // `y` read as the vowel-like `I` is the glide `j`.
        if letter == 'y' && label == "I" {
            out.push('j');
            continue;
        }
        // `ç` (U+00E7) is one label symbol; decomposed it would be `c` + a
        // cedilla, which is a different sound.
        for symbol in label.nfc() {
            if KEPT.contains(symbol) {
                out.push(symbol);
            } else if let Some(mapped) = voice_symbol(symbol) {
                out.push_str(mapped);
            }
        }
    }
    stress(word, out)
}

/// `word` spelled by rule into the voice's symbols, with its stress mark:
/// what a word the model mislabels is read as.
pub fn rule_phonemes(word: &str) -> String {
    let letters: Vec<char> = word.chars().collect();
    // The vowel that colours a consonant: the next one, else the last one.
    let colouring_vowel = |index: usize| {
        letters[index + 1..]
            .iter()
            .chain(letters[..index].iter().rev())
            .copied()
            .find(|letter| VOWEL_LETTERS.contains(*letter))
    };
    let front =
        |index: usize| colouring_vowel(index).is_some_and(|v| FRONT_VOWEL_LETTERS.contains(v));
    let mut out = String::new();
    for (index, &letter) in letters.iter().enumerate() {
        let last = index + 1 == letters.len();
        let symbols: &str = match letter {
            'a' | 'â' => "a",
            'e' => "e",
            'ı' => "ɯ",
            'i' | 'î' => "i",
            'o' => "o",
            'ö' => "ø",
            'u' | 'û' => "u",
            'ü' => "y",
            'c' => "dʒ",
            'ç' => "tʃ",
            'j' => "ʒ",
            'ş' => "ʃ",
            'y' => "j",
            'r' if last => "r",
            'r' => "ɾ",
            'g' if front(index) => "ɟ",
            'g' => "ɡ",
            'l' if front(index) => "l",
            'l' => "ɫ",
            'ğ' => {
                let previous = letters[..index]
                    .iter()
                    .rev()
                    .find(|letter| VOWEL_LETTERS.contains(**letter));
                if previous.is_some_and(|v| FRONT_VOWEL_LETTERS.contains(*v)) {
                    "j"
                } else {
                    "ː"
                }
            }
            'q' => "k",
            'w' => "v",
            'x' => "ks",
            other => {
                out.push(other);
                continue;
            }
        };
        out.push_str(symbols);
    }
    stress(word, out)
}

/// `phonemes` with the stress mark before its last vowel, as eSpeak NG
/// marks Turkish's usual word-final stress — except on unstressed words
/// and words with no vowel.
fn stress(word: &str, phonemes: String) -> String {
    if UNSTRESSED.contains(&word) {
        return phonemes;
    }
    match phonemes
        .char_indices()
        .rfind(|(_, symbol)| VOICE_VOWELS.contains(*symbol))
    {
        Some((at, _)) => {
            let mut stressed = String::with_capacity(phonemes.len() + STRESS.len_utf8());
            stressed.push_str(&phonemes[..at]);
            stressed.push(STRESS);
            stressed.push_str(&phonemes[at..]);
            stressed
        }
        None => phonemes,
    }
}

/// One clause's phonemes, the way `espeak-ng --ipa` would print them: its
/// words' phonemes joined with a space. `label` runs the model over a
/// batch of words, answering each word's labels (one per letter).
pub fn phonemize_clause(
    clause: &str,
    max_letters: usize,
    label: impl FnOnce(&[String]) -> Result<Vec<Vec<String>>, String>,
) -> Result<String, String> {
    let words = words(clause, max_letters);
    if words.is_empty() {
        return Ok(String::new());
    }
    let labels = label(&words)?;
    if labels.len() != words.len() {
        return Err(format!(
            "the Turkish G2P model answered {} words for {}",
            labels.len(),
            words.len()
        ));
    }
    Ok(words
        .iter()
        .zip(&labels)
        .map(|(word, labels)| {
            if labels_line_up(word, labels) {
                to_voice_symbols(word, labels)
            } else {
                rule_phonemes(word)
            }
        })
        .collect::<Vec<_>>()
        .join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(symbols: &[&str]) -> Vec<String> {
        symbols.iter().map(|symbol| symbol.to_string()).collect()
    }

    #[test]
    fn turkish_lowercase_keeps_the_dotless_i_apart() {
        assert_eq!(lowercase("IRMAK İzmir Işık"), "ırmak izmir ışık");
        assert_eq!(lowercase("ÇĞÖŞÜ"), "çğöşü");
    }

    #[test]
    fn words_are_letters_split_at_everything_else() {
        assert_eq!(
            words("Merhaba Erdem'in, dünyası-güzel!", 62),
            vec!["merhaba", "erdemin", "dünyası", "güzel"]
        );
        assert_eq!(words("Café", 62), vec!["cafe"]);
        assert_eq!(words("abcdefg", 3), vec!["abc", "def", "g"]);
        assert!(words(" ... ", 62).is_empty());
    }

    #[test]
    fn numbers_are_spelled_in_turkish() {
        assert_eq!(number_words(0), "sıfır");
        assert_eq!(number_words(7), "yedi");
        assert_eq!(number_words(10), "on");
        assert_eq!(number_words(100), "yüz");
        assert_eq!(number_words(123), "yüz yirmi üç");
        assert_eq!(number_words(1000), "bin");
        assert_eq!(number_words(1500), "bin beş yüz");
        assert_eq!(number_words(2026), "iki bin yirmi altı");
        assert_eq!(number_words(1_000_000), "bir milyon");
        assert_eq!(number_words(3_001_010), "üç milyon bin on");
    }

    #[test]
    fn digits_in_text_become_words() {
        assert_eq!(
            words("Saat 3'te 1.500 kişi, pi 3,14", 62),
            vec![
                "saat", "üçte", "bin", "beş", "yüz", "kişi", "pi", "üç", "virgül", "on", "dört"
            ]
        );
        assert_eq!(words("007", 62), vec!["sıfır", "sıfır", "yedi"]);
    }

    #[test]
    fn labels_that_line_up_become_the_voices_symbols_with_stress() {
        // What the model answers for these words.
        assert_eq!(
            to_voice_symbols("güneş", &labels(&["ɟ", "Y", "n", "ɛ", "ʃ"])),
            "ɟynˈɛʃ"
        );
        assert_eq!(
            to_voice_symbols(
                "kahvaltı",
                &labels(&["kʰ", "ɑ", "x", "v", "ɑ", "ł", "t", "ɨ"])
            ),
            "kahvaɫtˈɯ"
        );
        assert_eq!(
            to_voice_symbols("hayır", &labels(&["x", "ɑː", "I", "ɨ", "ɣ"])),
            "haːjˈɯr"
        );
        assert_eq!(
            to_voice_symbols(
                "çikolata",
                &labels(&["tʃ", "I", "k", "ɔ", "ł", "ɑ", "t", "ɑ"])
            ),
            "tʃɪkɔɫatˈa"
        );
        // A conjunction keeps no stress of its own.
        assert_eq!(to_voice_symbols("ve", &labels(&["v", "ɛ"])), "vɛ");
    }

    #[test]
    fn every_label_maps_into_the_voices_inventory() {
        // The 87 labels of `iatagun/dizge-g2p`.
        let all = [
            "I", "Il", "Is", "It", "Iɾ", "Iʋ", "U", "Uː", "Y", "a", "aː", "b", "bI", "bɨ", "c",
            "cʰ", "d", "dI", "dɨ", "dʒ", "e", "eː", "f", "fI", "fɨ", "g", "gɨ", "i", "iː", "j",
            "k", "kʰ", "l", "m", "mI", "n", "o", "oː", "p", "pʰ", "r", "s", "sI", "sɨ", "t", "tʃ",
            "tʰ", "u", "uː", "v", "x", "y", "yː", "z", "zɨ", "z̥", "ç", "çI", "ø", "øː", "ł", "ŋ",
            "œ", "ɑ", "ɑː", "ɔ", "ɛ", "ɛː", "ɟ", "ɟI", "ɣ", "ɨ", "ɨł", "ɨɾ", "ɱ", "ɾ", "ʃ", "ʃI",
            "ʃɨ", "ʋ", "ʒ", "ʰ", "ː", "ːU", "ːo", "ːt", "̥",
        ];
        // eSpeak NG's Turkish symbols, and the stress mark.
        let inventory = "aeɛæiɪoɔuʊyøœɯbdfhjklɫmnprɾstvzʃʒɟɡkcː ˈ";
        for label in all {
            let symbols = to_voice_symbols("x", &labels(&[label]));
            assert!(
                symbols.chars().all(|symbol| inventory.contains(symbol)),
                "{label:?} became {symbols:?}"
            );
        }
    }

    #[test]
    fn a_shifted_labelling_is_caught_and_spelled_by_rule() {
        // What the model answers for "olduğunu": its labels are shifted.
        let shifted = labels(&["ʰ", "ł", "d", "u", "u", "ː", "n", "ː"]);
        assert!(!labels_line_up("olduğunu", &shifted));
        assert_eq!(rule_phonemes("olduğunu"), "oɫduːunˈu");
        // A length mark for `ğ`, a glide for `y`: both line up.
        assert!(labels_line_up(
            "kağıt",
            &labels(&["kʰ", "ɑ", "ː", "ɨ", "t"])
        ));
        assert!(labels_line_up("diye", &labels(&["d", "i", "ː", "ɛ"])));
        // An epenthetic vowel on a consonant lines up too (`kral`).
        assert!(labels_line_up("kral", &labels(&["kʰ", "ɨɾ", "ɑ", "ł"])));
        assert!(!labels_line_up("ab", &labels(&["a"])));
    }

    #[test]
    fn rules_palatalize_before_front_vowels() {
        assert_eq!(rule_phonemes("gel"), "ɟˈel");
        assert_eq!(rule_phonemes("gol"), "ɡˈoɫ");
        assert_eq!(rule_phonemes("değil"), "dejˈil");
        assert_eq!(rule_phonemes("dağ"), "dˈaː");
        assert_eq!(rule_phonemes("bir"), "bˈir");
        assert_eq!(rule_phonemes("çocuk"), "tʃodʒˈuk");
    }

    #[test]
    fn a_clause_is_its_words_joined_with_a_space() {
        let answered = phonemize_clause("Güneş ve olduğunu", 62, |words| {
            assert_eq!(words, ["güneş", "ve", "olduğunu"]);
            Ok(vec![
                labels(&["ɟ", "Y", "n", "ɛ", "ʃ"]),
                labels(&["v", "ɛ"]),
                labels(&["ʰ", "ł", "d", "u", "u", "ː", "n", "ː"]),
            ])
        })
        .unwrap();
        assert_eq!(answered, "ɟynˈɛʃ vɛ oɫduːunˈu");

        let nothing = phonemize_clause(" - ", 62, |_| panic!("no words, no run")).unwrap();
        assert_eq!(nothing, "");

        let error = phonemize_clause("bir", 62, |_| Ok(Vec::new())).unwrap_err();
        assert!(error.contains("answered 0 words for 1"), "{error}");
    }
}
