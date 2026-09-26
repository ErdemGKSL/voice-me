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

`custom/tr_TR-erdem/` contains the Erdem voice (medium quality), fine-tuned
from `tr_TR-fahrettin-medium` (currently exported from `last.ckpt`, epoch
10000, step 80000). Its 63 MB graph fits in Git without splitting, so the
catalog serves both files straight from `main` through
`raw.githubusercontent.com`; no release is involved. A new checkpoint replaces
`model.onnx` (and `config.json` if it changed) at the same path: update
`SHA256SUMS` and the catalog's byte sizes and SHA-256 values in the same
commit.

## Updates

An installed voice from this catalog follows its entry. voice-me reads this
catalog at startup, every six hours while it runs, and whenever Settings →
Piper voices is refreshed; a voice whose `model` or `config` SHA-256 no longer
matches the installed copy is downloaded again (only the files that changed)
and used from the next line spoken. No Delete and Download is needed. While
GitHub's cache still serves the previous file, the check fails its checksum and
keeps the old copy; the next check picks the new one up.
