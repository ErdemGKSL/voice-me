//! `voice-me-recorder` — records a Piper training dataset sentence by
//! sentence and exports it as a zip the Colab notebook trains from.
//!
//! The window lives in `main.rs`; everything that can be tested without a
//! microphone or a screen lives here:
//!
//! * [`sentences`] — the prompt list (`id|text` lines);
//! * [`audio`] — captured samples → mono, 22.05 kHz, trimmed, 16-bit WAV;
//! * [`session`] — the folder recordings are kept in, so a run can be
//!   stopped and resumed;
//! * [`export`] — the zip: `metadata.csv`, `wavs/`, `manifest.json`.

pub mod audio;
pub mod export;
pub mod sentences;
pub mod session;

/// The sentences bundled into the binary: 20 000 Turkish prompts for a
/// DizgeBERT-phonemized Piper voice.
pub const BUNDLED_SENTENCES: &str = include_str!("../assets/sentences-tr-20000.csv");

/// Short phrases, questions and exclamations (12-50 characters) the long,
/// templated list lacks.
pub const BUNDLED_SHORT: &str = include_str!("../assets/sentences-tr-short.csv");
