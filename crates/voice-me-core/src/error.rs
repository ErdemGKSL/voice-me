/// The one domain error type (AD-4). Adapters convert their own failures
/// into this enum at the port boundary; no adapter-specific error type
/// crosses a crate boundary.
#[derive(Debug, thiserror::Error)]
pub enum VoiceMeError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The requested hotkey combination is already registered by something
    /// else on this system. Only a backend that takes a real exclusive grab
    /// (X11's `XGrabKey`) can detect this; no OS API reports *which* process
    /// holds the combination, so this variant deliberately carries no
    /// application name (spec-2-3 decision 2).
    #[error("this combination is already in use")]
    HotkeyAlreadyInUse,

    /// Reading the raw input devices under `/dev/input` was denied. The
    /// Wayland hotkey backend needs `input`-group membership; surfaced as
    /// its own variant so the UI can say so in words rather than showing a
    /// generic failure.
    #[error(
        "no read access to /dev/input/event* — add your user to the `input` group and log back in"
    )]
    InputDevicePermissionDenied,

    #[error("{0}")]
    Other(String),
}
