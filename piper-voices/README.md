# voice-me Piper voice catalog

`catalog.json` is voice-me's own list of Piper voices. The app fetches it
live from this repository's `main` branch when Settings → Piper voices is
opened or refreshed, so adding a voice is an edit of this file — no release
is needed. It is the first of three catalogs: a voice listed here wins over
the same key in `rhasspy/piper-voices` or the speaches-ai repositories.

## Format

```json
{
  "version": 1,
  "voices": [
    {
      "key": "tr_TR-fahrettin-medium",
      "name": "fahrettin",
      "locale": "tr_TR",
      "language": "Turkish (Turkey)",
      "quality": "medium",
      "licence": "CC0-1.0",
      "model":  { "url": "https://…/model.onnx",  "size": 63201294, "sha256": "…" },
      "config": { "url": "https://…/config.json", "size": 5022,     "sha256": "…" }
    }
  ]
}
```

- `key` names the voice and its cache directory: letters, digits, `_`, `-`
  and `.` only (`<locale>-<name>-<quality>` by convention).
- `locale` is Piper's speech language; `language` is how it is shown.
- `licence` is optional; without one the tab says "see model card".
- `model` is the Piper VITS graph and `config` its `config.json` (the file
  Piper calls `<voice>.onnx.json`). Each needs an `https://` URL that never
  changes (pin a revision, e.g. a Hugging Face `resolve/<commit>/` URL), its
  exact size in bytes, and its lowercase hex SHA-256. An entry with a
  non-HTTPS URL or a missing SHA-256 is ignored.

## Adding your own voice

1. Train or export a Piper voice (`model.onnx` + `config.json`) and host both
   files at a permanent HTTPS URL.
2. Compute `sha256sum` and the byte size of each file.
3. Add an entry above and commit it to `main`. Users see it the next time they
   open or refresh Settings → Piper voices.

## Erdem voice in this repository

`custom/tr_TR-erdem-high/` contains the Turkish Erdem model exported from the
`epoch=824-step=6600.ckpt` checkpoint. The 114 MB ONNX graph is stored as
three xz chunks so each Git object stays below GitHub's file-size limit.
`config.json` and `SHA256SUMS` are kept alongside the chunks.

On a push to `main` that changes these files, `piper-erdem-voice.yml`
reassembles the graph, verifies both files, and creates the immutable
`voice-tr_TR-erdem-high-v1` release. The catalog entry above downloads its
`model.onnx` and `config.json` assets. For a changed model, use a new versioned
tag and update the catalog URLs and checksums; do not replace v1 assets.

`custom/tr_TR-erdem-medium/` contains the Erdem medium model, fine-tuned from
`tr_TR-fahrettin-medium` (currently exported from `epoch=3809-step=30480.ckpt`). Its 63 MB graph fits in Git without splitting.
`piper-erdem-medium.yml` publishes it to the replaceable
`voice-tr_TR-erdem-medium-dev` release. Future checkpoints can replace the
model and config at the same paths: update `SHA256SUMS` and the catalog's
byte sizes and SHA-256 values in the same commit. The catalog URL stays the
same. Users who installed an older copy must delete it in voice-me and
install it again to receive the replacement.
