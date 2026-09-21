//! `voice-me-tts` — the `TtsPort` adapter: Chatterbox-Multilingual V3
//! running **in-process** on ONNX Runtime (AD-12).
//!
//! There is no sidecar process and no Python anywhere in this pipeline;
//! spec-2-5 exists to prove that, and these modules are the proof. The crate
//! reads its model files from a cache directory and never downloads them
//! (AD-8) — a file that is not there is reported by path, which is exactly
//! the list Story 3.2's provisioning work has to satisfy.

pub mod generate;
pub mod reference;
pub mod sessions;
pub mod tokenizer;

use std::path::Path;

use voice_me_core::{AudioBuffer, TtsPort, VoiceMeError};

pub use generate::{GenerationOutcome, GenerationSettings, generate};
pub use sessions::{ExecutionTarget, ModelCache, Sessions};

/// `TtsPort` adapter over the in-process engine.
pub struct TtsAdapter;

impl TtsPort for TtsAdapter {
    /// Still `todo!()` on purpose: spec-2-5 proves the engine runs, Story
    /// 2.6 owns *when* it runs — session lifecycle (built once and held, per
    /// AD-10), queueing, and the Speak Action wiring are all its decisions,
    /// and guessing at them here would mean 2.6 rewriting them. Everything
    /// the port needs already exists in [`sessions`] and [`generate`].
    fn generate(
        &self,
        _text: &str,
        _reference_clip: &Path,
        _language: &str,
    ) -> Result<AudioBuffer, VoiceMeError> {
        todo!("Story 2.6 owns session lifecycle and sequencing")
    }
}
