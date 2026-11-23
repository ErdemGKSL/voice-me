# Corpus rules (read fully before writing)

Goal: natural Turkish sentences for a text-to-speech training corpus.
Write exactly 250 lines to your file: one sentence (or a short 2-sentence passage) per line, UTF-8, no numbering, no blank lines. Write in chunks of ~60 lines with `cat >> FILE <<'EOF' ... EOF`.

Mix across your 250 lines (besides your theme's dominant pattern):
- at least 35 lines ending with "!"
- at least 35 questions ending with "?"
- at least 12 lines with a natural colon (e.g. "Annem şunu söyledi: ...", "Alınacaklar: un, şeker ve yumurta.")
- at least 15 lines with numbers written as DIGITS, ONLY in these forms: plain integers (7, 250, 1999), thousands with a dot and three digits (1.500, 12.750), decimal comma (2,5), apostrophe suffix (3'te, 10'a, 1453'te, 2'nci). Units, percent, money and times as words: "yüzde 20", "45 lira", "2,5 kilo", "saat 8'de". NEVER: %, ₺, $, €, times with colons (14:30), slashes, ranges (3-5), leading zeros, phone numbers.
- lengths: about 60 very short (1–4 words), 120 medium (5–14 words), 70 long (15–35 words with commas, semicolons, subordinate clauses).

Language rules:
- Everyday, natural Turkish with correct spelling and all Turkish letters (ç ğ ı İ ö ş ü; â where standard: hâlâ, kâğıt, rüzgâr), proper capitalization, apostrophes for suffixes on proper nouns.
- NO templates: never reuse a sentence frame with swapped words. Vary vocabulary as widely as possible. Avoid over-using "bir".
- No emoji, no English, no parentheses, no quotation marks, no abbreviations, no chat spelling, no offensive content, no duplicates.

Finish by checking: `wc -l FILE` (= 250), `sort FILE | uniq -d | wc -l` (= 0), `grep -c '!$' FILE`, `grep -c '?$' FILE`, `grep -c ':' FILE`, `grep -c '[0-9]' FILE`, `grep -c '[%₺$€/]' FILE` (= 0). Fix and recheck if needed. Report only these numbers.
