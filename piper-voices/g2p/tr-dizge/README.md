# Turkish G2P model (DizgeBERT)

Turkish Piper voices read text through this model instead of eSpeak NG. It is
[`iatagun/dizge-g2p`](https://huggingface.co/iatagun/dizge-g2p) (MIT, revision
`2afa9b7941a6feac766e040107561fe6d27ac8f1`), a BERT token classifier that
labels each letter of a Turkish word with its IPA phoneme, exported to ONNX by
`export_onnx.py`:

- `model.onnx`: the graph. Its inputs are `input_ids` and `attention_mask`
  (`int64`, `[words, letters + 2]`) and its output is `logits`
  (`[words, letters + 2, 87]`). The word-embedding table is cut down to the
  letters it reads, and the MatMul weights are int8.
- `vocab.json`: each letter's token id, the special tokens, the longest input
  (64) and the 87 labels.

voice-me downloads both files from `main` through `raw.githubusercontent.com`
when the Dependency Check's **Turkish G2P model** row is installed. Their sizes
and SHA-256 values are pinned in `crates/voice-me-deps/src/sources.rs`
(`turkish_g2p_files`). A new export replaces the files at the same path, and
the same commit updates `SHA256SUMS` and those pins.

## How voice-me uses it

`crates/voice-me-tts-piper/src/turkish.rs` handles each clause in five steps:

1. Lowercase the text the Turkish way.
2. Spell out numbers.
3. Run the model over the clause's words.
4. Map the labels into the symbols Turkish Piper voices were trained on, which
   are eSpeak NG's Turkish inventory: `ɑ` → `a`, `ɨ` → `ɯ`, `ł` → `ɫ`, `I` →
   `ɪ`, aspiration dropped, and so on.
5. Mark stress on the last vowel of each word.

Sometimes the model's labels do not line up with a word's letters. It shifts
them on some `ğ` words, such as `olduğunu`, which happens to about 3.5 % of
frequent words. Such a word is spelled by rule instead.
