"""Training-side port of voice-me's Turkish DizgeBERT phonemizer.

A line-for-line Python port of `crates/voice-me-tts-piper/src/{turkish,dizge,
phonemes}.rs`, so a Piper voice trained on this module's phonemes hears the
same symbols voice-me feeds it at inference (`"phoneme_type": "dizge"`).

    p = DizgePhonemizer("model.onnx", "vocab.json")
    p.phonemize("tr", "Merhaba, dünya!")   # -> [["m", ...], ...] per sentence

`phonemize(voice, text)` has the signature of piper's `EspeakPhonemizer`, so it
drops into piper's training dataset unchanged. It returns one list of
single-codepoint NFD symbols per sentence, with clause marks put back.
"""

import json
import unicodedata

LETTERS = "abcçdefgğhıijklmnoöpqrsştuüvwxyzâîû"
VOWEL_LETTERS = "aeıioöuüâîû"
FRONT_VOWEL_LETTERS = "eiöüî"
LABEL_VOWELS = "IUYaeioøœuyɑɔɛɨ"
VOICE_VOWELS = "aeɛæiɪoɔuʊyøœɯ"
STRESS = "ˈ"
UNSTRESSED = {"ve", "de", "da", "ki", "mi", "mı", "mu", "mü", "ile", "bu", "şu"}
KEPT = "aeiouyøœɔɛbdfjklmnprstvzʃʒɟɾː"

CLAUSE_MARKS = ",:;.!?"
SENTENCE_MARKS = ".!?"
SPACED_MARKS = ",:;"

VOICE_SYMBOL = {
    "I": "ɪ",
    "U": "ʊ",
    "Y": "y",
    "ɑ": "a",
    "ɨ": "ɯ",
    "ł": "ɫ",
    "ʋ": "v",
    "x": "h",
    "ç": "h",
    "ɣ": "r",  # dizge writes a word-final r as a fricative; eSpeak NG as r
    "g": "ɡ",  # LATIN SMALL LETTER SCRIPT G
    "c": "k",
    "ɱ": "m",
    "ŋ": "n",
}
DROPPED = {"ʰ", "̥", "̧"}


# ---- text ---------------------------------------------------------------


def lowercase(text):
    out = []
    for ch in text:
        if ch == "I":
            out.append("ı")
        elif ch == "İ":
            out.append("i")
        else:
            out.append(ch.lower())
    return "".join(out).replace("i̇", "i")


def _known_letter(ch):
    if ch in LETTERS:
        return ch
    base = unicodedata.normalize("NFD", ch)[:1]
    if base and base != ch and base in LETTERS:
        return base
    return None


def words(clause, max_letters):
    text = spell_numbers(lowercase(clause))
    out, current = [], []
    for ch in text:
        letter = _known_letter(ch)
        if letter is not None:
            current.append(letter)
        elif ch in ("'", "’"):
            pass  # a suffix after an apostrophe belongs to its word
        elif current:
            out.append("".join(current))
            current = []
    if current:
        out.append("".join(current))
    max_letters = max(max_letters, 1)
    pieces = []
    for word in out:
        for i in range(0, len(word), max_letters):
            pieces.append(word[i : i + max_letters])
    return pieces


# ---- numbers ------------------------------------------------------------

ONES = ["", "bir", "iki", "üç", "dört", "beş", "altı", "yedi", "sekiz", "dokuz"]
TENS = ["", "on", "yirmi", "otuz", "kırk", "elli", "altmış", "yetmiş", "seksen", "doksan"]
SCALES = ["", "bin", "milyon", "milyar", "trilyon", "katrilyon", "kentilyon"]


def _below_thousand(n, parts):
    hundreds, rest = divmod(n, 100)
    if hundreds > 1:
        parts.append(ONES[hundreds])
    if hundreds > 0:
        parts.append("yüz")
    if rest // 10 > 0:
        parts.append(TENS[rest // 10])
    if rest % 10 > 0:
        parts.append(ONES[rest % 10])


def number_words(n):
    if n == 0:
        return "sıfır"
    groups = []
    rest = n
    while rest > 0:
        groups.append(rest % 1000)
        rest //= 1000
    parts = []
    for scale in range(len(groups) - 1, -1, -1):
        group = groups[scale]
        if group == 0:
            continue
        if not (scale == 1 and group == 1):  # "bin", never "bir bin"
            _below_thousand(group, parts)
        if scale > 0:
            parts.append(SCALES[scale])
    return " ".join(parts)


def spell_numbers(text):
    chars = list(text)

    def digit_at(i):
        return i < len(chars) and chars[i] in "0123456789"

    out = []
    index = 0
    while index < len(chars):
        if not digit_at(index):
            out.append(chars[index])
            index += 1
            continue
        digits = ""
        while digit_at(index):
            digits += chars[index]
            index += 1
        if len(digits) <= 3:
            while (
                index < len(chars)
                and chars[index] == "."
                and all(digit_at(index + o) for o in (1, 2, 3))
                and not digit_at(index + 4)
            ):
                digits += "".join(chars[index + 1 : index + 4])
                index += 4
        out.append(" ")
        if len(digits) <= 18 and (len(digits) == 1 or not digits.startswith("0")):
            out.append(number_words(int(digits)))
        else:
            out.append(" ".join(number_words(int(d)) for d in digits))
        if index < len(chars) and chars[index] == "," and digit_at(index + 1):
            out.append(" virgül")
            index += 1
        if not (index < len(chars) and chars[index] in ("'", "’")):
            out.append(" ")
    return "".join(out)


# ---- labels -------------------------------------------------------------


def _is_label_vowel(symbol):
    return symbol in LABEL_VOWELS


def _is_label_consonant(symbol):
    return not _is_label_vowel(symbol) and symbol not in ("ː", "ʰ", "̥")


def labels_line_up(word, labels):
    if len(word) != len(labels):
        return False
    for letter, label in zip(word, labels):
        if not label:
            return False
        first = label[0]
        if letter == "ğ":
            ok = first in ("ː", "j", "ɣ") or _is_label_vowel(first)
        elif letter == "y":
            ok = first in ("j", "I", "ː")
        elif letter in VOWEL_LETTERS:
            ok = _is_label_vowel(first)
        else:
            ok = any(_is_label_consonant(s) for s in label)
        if not ok:
            return False
    return True


def _stress(word, phonemes):
    if word in UNSTRESSED:
        return phonemes
    for at in range(len(phonemes) - 1, -1, -1):
        if phonemes[at] in VOICE_VOWELS:
            return phonemes[:at] + STRESS + phonemes[at:]
    return phonemes


def to_voice_symbols(word, labels):
    out = []
    for letter, label in zip(word, labels):
        if letter == "y" and label == "I":
            out.append("j")
            continue
        for symbol in unicodedata.normalize("NFC", label):
            if symbol in KEPT:
                out.append(symbol)
            elif symbol in DROPPED:
                continue
            elif symbol in VOICE_SYMBOL:
                out.append(VOICE_SYMBOL[symbol])
    return _stress(word, "".join(out))


def rule_phonemes(word):
    letters = list(word)

    def colouring_vowel(index):
        for letter in letters[index + 1 :] + letters[:index][::-1]:
            if letter in VOWEL_LETTERS:
                return letter
        return None

    def front(index):
        v = colouring_vowel(index)
        return v is not None and v in FRONT_VOWEL_LETTERS

    plain = {
        "a": "a", "â": "a", "e": "e", "ı": "ɯ", "i": "i", "î": "i", "o": "o",
        "ö": "ø", "u": "u", "û": "u", "ü": "y", "c": "dʒ", "ç": "tʃ",
        "j": "ʒ", "ş": "ʃ", "y": "j", "q": "k", "w": "v", "x": "ks",
    }  # fmt: skip
    out = []
    for index, letter in enumerate(letters):
        last = index + 1 == len(letters)
        if letter in plain:
            out.append(plain[letter])
        elif letter == "r":
            out.append("r" if last else "ɾ")
        elif letter == "g":
            out.append("ɟ" if front(index) else "ɡ")
        elif letter == "l":
            out.append("l" if front(index) else "ɫ")
        elif letter == "ğ":
            previous = next((l for l in reversed(letters[:index]) if l in VOWEL_LETTERS), None)
            out.append("j" if previous is not None and previous in FRONT_VOWEL_LETTERS else "ː")
        else:
            out.append(letter)
    return _stress(word, "".join(out))


def phonemize_clause(clause, max_letters, label):
    ws = words(clause, max_letters)
    if not ws:
        return ""
    labels = label(ws)
    if len(labels) != len(ws):
        raise RuntimeError(f"the model answered {len(labels)} words for {len(ws)}")
    return " ".join(
        to_voice_symbols(w, l) if labels_line_up(w, l) else rule_phonemes(w)
        for w, l in zip(ws, labels)
    )


# ---- clauses and sentences (phonemes.rs) --------------------------------


def split_clauses(text):
    """[(clause text, terminator or None)], split at `, : ; . ! ?` followed by
    whitespace or the end of the text."""
    clauses, current = [], []
    for index, ch in enumerate(text):
        boundary = ch in CLAUSE_MARKS and (
            index + 1 == len(text) or text[index + 1].isspace()
        )
        if boundary:
            clause = "".join(current).strip()
            current = []
            if clause:
                clauses.append((clause, ch))
        else:
            current.append(ch)
    rest = "".join(current).strip()
    if rest:
        clauses.append((rest, None))
    return clauses


def sentence_phonemes(clauses, ipa):
    sentences, current = [], ""
    for (_, mark), raw in zip(clauses, ipa):
        current += raw
        if mark is not None:
            current += mark
            if mark in SPACED_MARKS:
                current += " "
        if mark is None or mark in SENTENCE_MARKS:
            sentences.append(current)
            current = ""
    if current:
        sentences.append(current)
    result = [list(unicodedata.normalize("NFD", s)) for s in sentences]
    return [s for s in result if s]


# ---- the model ----------------------------------------------------------


class DizgePhonemizer:
    def __init__(self, model_path, vocab_path, intra_threads=4):
        import onnxruntime as ort

        with open(vocab_path, encoding="utf-8") as f:
            vocab = json.load(f)
        self.pad, self.unk = vocab["pad"], vocab["unk"]
        self.cls, self.sep = vocab["cls"], vocab["sep"]
        self.max_letters = vocab["max_length"] - 2
        self.chars = vocab["chars"]
        self.labels = vocab["labels"]
        options = ort.SessionOptions()
        options.intra_op_num_threads = intra_threads
        self.session = ort.InferenceSession(
            model_path, options, providers=["CPUExecutionProvider"]
        )

    def _label(self, ws):
        import numpy as np

        width = max(min(len(w), self.max_letters) for w in ws) + 2
        ids = np.full((len(ws), width), self.pad, dtype=np.int64)
        mask = np.zeros((len(ws), width), dtype=np.int64)
        for row, w in enumerate(ws):
            tokens = (
                [self.cls]
                + [self.chars.get(ch, self.unk) for ch in w[: self.max_letters]]
                + [self.sep]
            )
            ids[row, : len(tokens)] = tokens
            mask[row, : len(tokens)] = 1
        logits = self.session.run(None, {"input_ids": ids, "attention_mask": mask})[0]
        best = logits.argmax(axis=-1)
        return [
            [self.labels[best[row, letter + 1]] for letter in range(min(len(w), self.max_letters))]
            for row, w in enumerate(ws)
        ]

    def phonemize_clause(self, clause):
        return phonemize_clause(clause, self.max_letters, self._label)

    def phonemize(self, voice, text):  # `voice` is ignored; same signature as eSpeak's
        clauses = split_clauses(text)
        ipa = [self.phonemize_clause(c) for c, _ in clauses]
        return sentence_phonemes(clauses, ipa)


# ---- self-test (expected values from turkish.rs's tests) ----------------

if __name__ == "__main__":
    L = lambda *s: list(s)
    assert lowercase("IRMAK İzmir Işık") == "ırmak izmir ışık"
    assert words("Merhaba Erdem'in, dünyası-güzel!", 62) == ["merhaba", "erdemin", "dünyası", "güzel"]
    assert words("Café", 62) == ["cafe"]
    assert words("abcdefg", 3) == ["abc", "def", "g"]
    assert number_words(2026) == "iki bin yirmi altı"
    assert number_words(3_001_010) == "üç milyon bin on"
    assert words("Saat 3'te 1.500 kişi, pi 3,14", 62) == [
        "saat", "üçte", "bin", "beş", "yüz", "kişi", "pi", "üç", "virgül", "on", "dört"
    ]
    assert words("007", 62) == ["sıfır", "sıfır", "yedi"]
    assert to_voice_symbols("güneş", L("ɟ", "Y", "n", "ɛ", "ʃ")) == "ɟynˈɛʃ"
    assert to_voice_symbols("kahvaltı", L("kʰ", "ɑ", "x", "v", "ɑ", "ł", "t", "ɨ")) == "kahvaɫtˈɯ"
    assert to_voice_symbols("hayır", L("x", "ɑː", "I", "ɨ", "ɣ")) == "haːjˈɯr"
    assert to_voice_symbols("çikolata", L("tʃ", "I", "k", "ɔ", "ł", "ɑ", "t", "ɑ")) == "tʃɪkɔɫatˈa"
    assert to_voice_symbols("ve", L("v", "ɛ")) == "vɛ"
    shifted = L("ʰ", "ł", "d", "u", "u", "ː", "n", "ː")
    assert not labels_line_up("olduğunu", shifted)
    assert rule_phonemes("olduğunu") == "oɫduːunˈu"
    assert labels_line_up("kağıt", L("kʰ", "ɑ", "ː", "ɨ", "t"))
    assert labels_line_up("kral", L("kʰ", "ɨɾ", "ɑ", "ł"))
    assert rule_phonemes("gel") == "ɟˈel"
    assert rule_phonemes("gol") == "ɡˈoɫ"
    assert rule_phonemes("değil") == "dejˈil"
    assert rule_phonemes("dağ") == "dˈaː"
    assert rule_phonemes("çocuk") == "tʃodʒˈuk"
    assert split_clauses("Merhaba, dünya! Nasılsın") == [
        ("Merhaba", ","), ("dünya", "!"), ("Nasılsın", None)
    ]
    assert "".join(sentence_phonemes([("a", ","), ("b", ".")], ["x", "y"])[0]) == "x, y."
    print("dizge_phonemizer self-test passed")
