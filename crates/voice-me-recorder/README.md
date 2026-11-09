# voice-me-recorder

A second executable (`voice-me-recorder`) that records a training dataset for a Piper voice and exports it as a zip the Colab notebook [`notebooks/Piper_Train_Dizge.ipynb`](../../notebooks/Piper_Train_Dizge.ipynb) trains from.

```text
cargo run --release -p voice-me-recorder -- [--dir <folder>] [--sentences <id|text csv>] [--import <zip>]
```

- The prompts are **embedded in the binary**: 20 000 Turkish sentences (`assets/sentences-tr-20000.csv`) plus 248 short phrases, questions and exclamations (`assets/sentences-tr-short.csv`). `--sentences <csv>` replaces them.
- They are shown in **coverage order**, not list order: the sentence adding the most unseen words and letter pairs comes first, so the first 1 000–2 000 takes cover the most language (the 20 000 lines come from templates and share only ~540 distinct words). One short line follows every three long ones, so short utterances, questions and exclamations are in the first takes. The order is deterministic; recordings are keyed by sentence id, so it never loses work.
- Recordings are saved to `Documents/voice-me-recordings/wavs/<id>.wav` the moment a take stops, so closing the window loses nothing.
- **Eğitim zip'i oluştur** → `exports/voice-me-dataset-<speaker>-<n>.zip` (`metadata.csv`, `wavs/`, `manifest.json`), what the notebook reads.
- **Yedekle (devam için)** → `exports/voice-me-backup-<speaker>-<n>.zip`: the same files plus `resume.json`. On another PC, **İçe aktar** (or `--import`) merges it — recordings already on that PC are never overwritten — and the recorder continues from the sentence (by id) where the backup stopped. The notebook accepts either zip.

Keys: `Space` record / stop, `P` play, `←` `→` previous / next, `Enter` next sentence without a take, `Delete` remove the take.

## Audio quality

The training clips are 22 050 Hz mono 16-bit (Piper "medium"). To keep the dataset clean:

- the microphone is opened at the highest rate it offers up to 48 kHz (float preferred), and a warning appears if it only gives < 44.1 kHz (a Bluetooth headset in call mode);
- capture is DC-corrected, mixed to mono, resampled with a sinc (FFT) resampler, trimmed to 0.2 s of silence on each side, levelled to a −1 dBFS peak (gain capped at +12 dB) and given 8 ms edge fades, then quantised to 16-bit **once**, with rounding;
- a take is **rejected** if it is shorter than 0.8 s or its peak is below −26 dBFS; it is **kept but flagged** (and auto-advance stops) if the microphone clipped or the speech-to-room ratio is below 30 dB — record those again somewhere quieter.

Tips: wired or built-in microphone, 15–20 cm from the mouth, same distance and room for every session, no fan or music.
