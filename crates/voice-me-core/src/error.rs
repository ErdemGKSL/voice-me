/// The one domain error type (AD-4). Adapters convert their own failures
/// into this enum at the port boundary; no adapter-specific error type
/// crosses a crate boundary.
#[derive(Debug, thiserror::Error)]
pub enum VoiceMeError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}
