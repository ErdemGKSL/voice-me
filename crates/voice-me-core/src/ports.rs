use std::path::Path;

use crate::AppEventSender;
use crate::audio::AudioBuffer;
use crate::error::VoiceMeError;
use crate::state::AppState;

/// Driving adapter port: captures the configured global hotkey per OS.
pub trait HotkeyPort {
    /// Start listening for `hotkey`, signaling each press through `events`.
    ///
    /// `events` is the sender half of the shared `AppEvent` channel (AD-3),
    /// threaded in the same way as `TrayPort::show`: the adapter signals
    /// presses only by sending `AppEvent::HotkeyPressed`, never by depending
    /// on `voice-me-ui` or touching windows itself.
    ///
    /// Called once at startup by the composition root, and only when a
    /// hotkey was actually saved in a previous run — an app that has never
    /// had one configured must start with no hotkey active and no error.
    fn start_listening(&self, hotkey: &str, events: AppEventSender) -> Result<(), VoiceMeError>;

    /// Make `hotkey` the live combination, replacing whichever one is
    /// currently bound.
    ///
    /// This is the "confirm it works before persisting it" step the Settings
    /// UI calls on Save: on a backend that can detect a conflict it returns
    /// [`VoiceMeError::HotkeyAlreadyInUse`] *without* disturbing the
    /// previously bound combination, so the caller can surface the conflict
    /// and leave both the old binding and `settings.toml` untouched. Any
    /// other `Err` leaves the old binding in place too.
    fn rebind(&self, hotkey: &str) -> Result<(), VoiceMeError>;
}

/// Driven adapter port: plays generated audio through the OS virtual microphone.
pub trait VirtualMicPort {
    /// Play a buffer of generated speech through the virtual microphone device.
    ///
    /// Takes the core-owned [`AudioBuffer`] (AD-11), not bytes: the format
    /// is fixed at 24 kHz mono f32 by what Chatterbox's decoder emits, so
    /// there is nothing for this side to detect, negotiate, or guess wrong.
    fn play(&self, audio: &AudioBuffer) -> Result<(), VoiceMeError>;
}

/// Driven adapter port: manages the OS system tray presence.
pub trait TrayPort {
    /// Show the application's tray icon and menu.
    ///
    /// Takes a `gpui_kit::App` context because tray-backed implementations
    /// (e.g. `gpui-tray`, see spec-2-1) build their native tray item and
    /// dispatch menu-item clicks as ordinary GPUI actions through `cx` —
    /// there is no way to stand up or drive a tray without it. `voice-me-core`
    /// depends on `gpui-kit` only for this context type, never on the tray
    /// crate itself, which stays confined to the `voice-me-tray-*` adapters.
    ///
    /// `events` is the sender half of the shared `AppEvent` channel (AD-3):
    /// adapters use it to signal actions like "Settings…" clicked back to
    /// the composition root, without depending on `voice-me-ui` or calling
    /// window APIs themselves.
    fn show(&self, cx: &mut gpui_kit::App, events: AppEventSender) -> Result<(), VoiceMeError>;
}

/// Driven adapter port: generates speech with Chatterbox-Multilingual,
/// in-process, on ONNX Runtime (AD-12 — there is no sidecar process and no
/// Python anywhere in the pipeline; spec-2-5 verified this).
///
/// `Send + Sync` because every call is driven through
/// [`crate::tokio_bridge::spawn_blocking`] (AD-5): the adapter is shared
/// across GPUI's main thread and Tokio's blocking pool, so the port itself
/// has to say so rather than each composition root discovering it.
pub trait TtsPort: Send + Sync {
    /// Build and hold whatever generation needs, ahead of the first Speak
    /// Action.
    ///
    /// Session construction measured 86–110 s in spec-2-5, against a ~20 s
    /// utterance — far too much to hide inside the first call and call it a
    /// lazy cost (AD-10). The composition root calls this once at startup,
    /// on the blocking pool, and only when there is an active Reference
    /// Voice Sample: a first-run user who has not set one up pays nothing.
    ///
    /// Idempotent, and safe to race with [`TtsPort::generate`] — whichever
    /// arrives second waits for the first to finish rather than building a
    /// second set of sessions.
    fn warm_up(&self) -> Result<(), VoiceMeError>;

    /// Whether [`TtsPort::generate`] can start generating immediately, i.e.
    /// whether warm-up has already completed.
    ///
    /// This is what distinguishes "this will take the usual ~20 s" from
    /// "this will take a minute and a half first", which is the only wait
    /// unusual enough to be worth a notification (spec-2-6 Decision 4). It
    /// must never block — in particular it must not wait on whatever lock
    /// serializes generation, or a perfectly normal in-flight utterance
    /// would report "not ready" and notify for nothing.
    fn is_ready(&self) -> bool;

    /// Generate speech for `text` in `language`, cloning the voice in the
    /// Reference Voice Sample at `reference_clip`.
    ///
    /// `language` is a lowercase ISO code (`"tr"`, `"en"`) — the model
    /// takes it as a literal bracketed tag prepended to the text, so it is
    /// part of the prompt rather than a separate knob. `reference_clip` is
    /// a path, not decoded audio: whichever container the user's clip is in
    /// has to be decoded and resampled to exactly 24 kHz before the speech
    /// encoder sees it, and that belongs to the adapter that knows the
    /// model's requirements.
    ///
    /// This call blocks for as long as generation takes (seconds), so it is
    /// always driven through [`crate::tokio_bridge::spawn_blocking`] (AD-5)
    /// and never on GPUI's main thread.
    ///
    /// Exactly one generation runs at a time (AD-10). A second call arriving
    /// while one is in flight waits for it and then runs on the same
    /// sessions; it never builds a second set and never runs concurrently.
    fn generate(
        &self,
        text: &str,
        reference_clip: &Path,
        language: &str,
    ) -> Result<AudioBuffer, VoiceMeError>;
}

/// Driven adapter port: delivers an OS-native desktop notification.
///
/// The same driven shape as [`TrayPort`] — one trait in the hexagon, one
/// per-OS adapter crate behind it — but with no `gpui_kit::App` context:
/// every notification this application sends originates from a background
/// job that has already left the main thread (a failed generation, a slow
/// session build), and requiring a context would mean hopping back just to
/// say something went wrong.
///
/// It is the *only* surface the Speak Action has for failure or unusual
/// slowness in this story (UX-DR14/15). The Prompt Overlay is fire-and-forget
/// — it is already closed by the time generation starts — and its inline
/// notice belongs to Story 3.4.
///
/// `Send + Sync` for the same reason [`TtsPort`] is: the adapter is shared
/// with Tokio's blocking pool.
pub trait NotificationPort: Send + Sync {
    /// Show one notification with `summary` as its title and `body` beneath.
    ///
    /// Returning `Err` means the notification could not be *delivered* —
    /// there is no notification daemon, or the bus call failed. Callers
    /// treat that as unreportable rather than as a second failure to report:
    /// notifying about a failed notification has nowhere to go.
    fn notify(&self, summary: &str, body: &str) -> Result<(), VoiceMeError>;
}

/// Driven adapter port: detects and provisions runtime dependencies.
pub trait DependencyProvisioningPort {
    /// Run the Dependency Check, reporting results via `AppEvent`.
    fn check(&self) -> Result<(), VoiceMeError>;
}

/// Port for reading/writing persisted settings (implemented inside
/// `voice-me-core` itself, per AD-6 — not a separate adapter crate).
pub trait SettingsStore {
    /// Load settings into an `AppState`.
    fn load(&self) -> Result<AppState, VoiceMeError>;

    /// Persist `wav_bytes` as the active Reference Voice Sample (AD-6):
    /// writes the audio file to the OS data directory and updates the
    /// settings, replacing whichever clip was previously active. Returns the
    /// resulting `AppState`. Adapters (e.g. `voice-me-ui`'s recorder) call
    /// this instead of writing to the data directory themselves.
    fn save_reference_voice_sample(&self, wav_bytes: &[u8]) -> Result<AppState, VoiceMeError>;

    /// Persist the selected input (microphone) device name, or `None` to
    /// clear the selection back to the OS default. Returns the resulting
    /// `AppState`.
    fn save_selected_mic_device(&self, device: Option<&str>) -> Result<AppState, VoiceMeError>;

    /// Persist the configured global hotkey, or `None` to clear it. The
    /// string is stored in `global-hotkey`'s accelerator syntax (e.g.
    /// `Ctrl+Alt+KeyV`) so it round-trips through `HotKey::from_str` with no
    /// bespoke parser; the display form shown on a chip is derived for
    /// rendering only and never persisted. Returns the resulting `AppState`.
    fn save_hotkey(&self, hotkey: Option<&str>) -> Result<AppState, VoiceMeError>;
}
