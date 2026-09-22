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

    /// A file the speech engine needs is not on disk. Its own variant
    /// because it is the *only* failure a user can actually fix, and the
    /// fix is mechanical: fetch that exact path. voice-me never downloads
    /// model or runtime files itself (AD-8/AD-12), so the path has to
    /// travel all the way to the surface intact rather than being folded
    /// into a generic "failed to start" message.
    #[error("missing runtime asset: {path}")]
    MissingRuntimeAsset {
        /// The absolute path that was looked for and not found.
        path: std::path::PathBuf,
    },

    /// ONNX Runtime itself failed — building a session, or running one.
    /// Separate from [`VoiceMeError::MissingRuntimeAsset`] because nothing
    /// the user does to their filesystem fixes it: it means the graph, the
    /// execution provider, or the inputs are wrong, which is a bug report,
    /// not a provisioning step.
    #[error("speech engine failure: {0}")]
    SpeechEngine(String),

    /// The Reference Voice Sample is in a container or codec this build
    /// cannot decode. Its own variant so the message can name the format
    /// the user handed over — "unsupported audio" without the format is
    /// unactionable when the fix is a one-line `ffmpeg` conversion.
    #[error("unsupported audio input: {format}")]
    UnsupportedAudioInput {
        /// The container/codec as far as it could be identified.
        format: String,
    },

    /// Generation was asked for with nothing to say. Rejected as its own
    /// variant rather than returning an empty buffer, because every caller
    /// downstream (playback, notifications) would then have to decide what
    /// an empty buffer means.
    #[error("nothing to speak: the text is empty")]
    EmptyText,

    /// There is no active Reference Voice Sample, so there is no voice to
    /// clone. Its own variant rather than a `MissingRuntimeAsset` because
    /// the fix is not "fetch this file" — nothing can be downloaded to
    /// satisfy it. The user has to record or import a clip in Settings →
    /// Voice, and the message has to say that rather than naming a path
    /// they were never supposed to create by hand.
    #[error("no Reference Voice Sample yet — record or import one in Settings → Voice")]
    NoReferenceVoiceSample,

    /// The Virtual Microphone could not be created, addressed, or written
    /// to. Its own variant rather than [`VoiceMeError::Other`] because it
    /// is the one failure whose fix is a *device* problem, not a voice-me
    /// problem: no audio server running, the device removed from under us,
    /// or a config the audio server never picked up. Callers that already
    /// have generated audio in hand need to tell those apart from "speech
    /// generation failed", since the speech is fine and only its way out is
    /// missing.
    #[error("virtual microphone unavailable: {0}")]
    VirtualMicUnavailable(String),

    #[error("{0}")]
    Other(String),
}
