use std::path::PathBuf;

/// The single, canonical application state, owned by `voice-me-core`.
///
/// Mutation only happens through `voice-me-core` use-case functions (AD-3) —
/// adapters never write to this struct directly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppState {
    pub hotkey: Option<String>,
    pub reference_voice_sample: Option<PathBuf>,
    pub ui_language: String,
    pub selected_mic_device: Option<String>,
}
