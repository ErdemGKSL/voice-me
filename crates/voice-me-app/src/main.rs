//! `voice-me-app` — composition root.
//!
//! Story 2.2's slice: the app is always tray-resident (`TrayPort::show` is
//! called unconditionally at startup) and the Settings window only opens
//! automatically on first run (no active Reference Voice Sample yet). Once
//! running, the tray's "Settings…" item (re)opens/activates that same
//! window on demand via the shared `AppEvent` channel (AD-3).
//!
//! Story 2.3 adds the `HotkeyPort` adapter alongside the tray: a hotkey
//! saved in a previous run is re-activated at startup, and each press
//! arrives here as `AppEvent::HotkeyPressed`.
//!
//! Story 2.4 makes that press do something: it summons the Prompt Overlay,
//! a borderless always-on-top window holding one `Input`. Confirming the
//! line comes back here as `AppEvent::SpeakRequested`.
//!
//! Story 2.6 makes that event produce audio. The TTS adapter holds its four
//! ONNX Runtime sessions for the process lifetime (AD-10), every generation
//! is dispatched onto Tokio's blocking pool through the AD-5 bridge so the
//! UI never freezes, and failures — plus the one genuinely unusual wait, a
//! Speak Action that lands while the sessions are still being built — reach
//! the user as OS-native notifications.
//!
//! Stories 3.1/3.4 add the Dependency Check: `voice-me-deps` is run once in
//! the background at startup and again on demand, its report arrives here
//! as `AppEvent::DependencyCheckCompleted`, and this file is the only place
//! that keeps it. Both readers — Settings → Dependencies and the hotkey
//! gate that decides whether the Prompt Overlay opens blocked — read that
//! one value.
//!
//! Story 2.9 closes the loop: the per-OS `VirtualMicPort` adapter is built
//! here and handed to `speak`, which plays the generated buffer through the
//! Virtual Microphone instead of dropping it. The device itself is ensured
//! once at startup, in the background, so a fresh machine needs no manual
//! setup step (spec-2-9 Decision 1).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::component::Root;
use gpui_kit::{
    App, AppContext as _, QuitMode, WindowBackgroundAppearance, WindowBounds, WindowDecorations,
    WindowHandle, WindowKind, WindowOptions, px, size,
};
#[cfg(target_os = "linux")]
use voice_me_core::VoiceMeError;
use voice_me_core::{
    AppEvent, AppState, DependencyOutcome, DependencyProvisioningPort, FileSettingsStore,
    HotkeyPort, NotificationPort, SettingsStore, SpeechBackend, TtsPort, VirtualMicPort,
    tokio_bridge,
};
use voice_me_deps::DepsAdapter;
use voice_me_tts::TtsAdapter;
use voice_me_ui::{DependenciesTab, PromptOverlayView, SettingsView, blocker_notice};

#[cfg(target_os = "linux")]
use voice_me_audio_linux::LinuxVirtualMicAdapter;
#[cfg(target_os = "windows")]
use voice_me_audio_windows::WindowsVirtualMicAdapter;
#[cfg(target_os = "linux")]
use voice_me_hotkey_linux::LinuxHotkeyAdapter;
#[cfg(target_os = "windows")]
use voice_me_hotkey_windows::WindowsHotkeyAdapter;
#[cfg(target_os = "linux")]
use voice_me_notify_linux::LinuxNotificationAdapter;
#[cfg(target_os = "windows")]
use voice_me_notify_windows::WindowsNotificationAdapter;
#[cfg(target_os = "linux")]
use voice_me_tray_linux::LinuxTrayAdapter;
#[cfg(target_os = "windows")]
use voice_me_tray_windows::WindowsTrayAdapter;

use voice_me_core::TrayPort as _;

/// The overlay's fixed comfortable width, and a height sized to one line of
/// input plus the surface's padding (spec-2-4 Design Notes).
const OVERLAY_WIDTH: f32 = 560.;
const OVERLAY_HEIGHT: f32 = 84.;

/// Always-on-top is a two-tier capability, mirroring Story 2.3's two hotkey
/// backends. `WindowKind::PopUp` is a real override-redirect, taskbar-less,
/// above-everything window under X11. Wayland has no equivalent in this GPUI
/// version — `PopUp` falls through to a plain xdg_toplevel and the
/// compositor places it itself — so a Wayland session gets an ordinary
/// focused window that may sit below a fullscreen game. `LayerShell` is
/// deliberately not used: GNOME/Mutter does not implement
/// `zwlr_layer_shell_v1`, so it would add a second unverifiable path for no
/// gain. The difference is documented in the README rather than worked
/// around.
#[cfg(target_os = "linux")]
fn overlay_window_kind() -> WindowKind {
    overlay_window_kind_for(voice_me_hotkey_linux::session_kind())
}

/// The decision itself, separated from reading the session the way
/// `voice_me_hotkey_linux::session_kind_for` is, so both arms can be
/// asserted without a window server.
#[cfg(target_os = "linux")]
fn overlay_window_kind_for(session: voice_me_hotkey_linux::SessionKind) -> WindowKind {
    match session {
        voice_me_hotkey_linux::SessionKind::X11 => WindowKind::PopUp,
        // Asking for `PopUp` here would not fail — it would simply behave
        // like `Normal`. Saying `Normal` keeps the code honest about what
        // this session actually gets.
        voice_me_hotkey_linux::SessionKind::Wayland => WindowKind::Normal,
    }
}

#[cfg(not(target_os = "linux"))]
fn overlay_window_kind() -> WindowKind {
    WindowKind::PopUp
}

/// The AD-9 resolved speech backend for this run.
///
/// Epic 3 owns detection: `voice-me-deps` emits nothing yet and
/// `DependencyProvisioningPort::check` has no `events` parameter, so there
/// is no detection result to read. Until Story 3.1 there is exactly one
/// honest input — which release variant this binary was compiled as — and
/// everything CI builds is the `cpu` variant. Written here, in the
/// composition root, and read by `voice-me-tts` off `AppState` (AD-9); the
/// TTS crate never calls `voice-me-deps`.
fn resolved_speech_backend() -> SpeechBackend {
    SpeechBackend::CPU
}

/// What a hotkey press should open, given what the last Dependency Check
/// said: `None` for the ordinary typeable overlay, or the notice a blocked
/// one leads with (Story 3.4).
///
/// Decision 3 lives here: only a missing *speech-engine* dependency blocks.
/// A missing Virtual Microphone is reported in Settings but still lets the
/// user type, and the failure surfaces at playback through the existing
/// `VirtualMicUnavailable` notification instead.
///
/// A check that has not completed yet does not block either. Blocking on
/// "we do not know" would turn a slow startup into a refused Speak Action,
/// and the engine's own failure path already names anything that is
/// actually wrong.
fn overlay_blocker(outcome: &DependencyOutcome) -> Option<String> {
    match outcome {
        DependencyOutcome::Pending => None,
        // The check could not run at all — an unresolvable cache root, say
        // — which is also every path the engine would have read. Saying so
        // up front beats a typed line that quietly fails afterwards.
        DependencyOutcome::Failed(reason) => {
            Some(format!("The dependency check could not run: {reason}"))
        }
        DependencyOutcome::Ready(report) => report.speech_engine_blocker().map(blocker_notice),
    }
}

/// Whether the Dependency Check that just landed should open Settings →
/// Dependencies by itself (Decision 2).
///
/// Only the *startup* check does, and only when it found something: later
/// checks never do, because by then either the window is already open or
/// the user asked for the check themselves. `already_auto_opened` is not
/// persisted — this must happen again on the next launch while the
/// dependency is still missing.
fn should_auto_open_dependencies(already_auto_opened: bool, anything_missing: bool) -> bool {
    !already_auto_opened && anything_missing
}

/// The persisted settings, plus this run's resolved backend and the latest
/// Dependency Check.
///
/// Re-read per Speak Action rather than snapshotted at startup: a Reference
/// Voice Sample recorded in Settings since launch has to count, and so would
/// a speech language edited in `settings.toml`.
fn current_state(
    settings_store: &Arc<dyn SettingsStore>,
    dependencies: &DependencyOutcome,
) -> AppState {
    let mut state = settings_store.load().unwrap_or_else(|error| {
        // Falling back to defaults keeps the Speak Action reaching a real
        // failure it can name ("no Reference Voice Sample") instead of the
        // app silently doing nothing.
        eprintln!("could not read settings: {error}");
        AppState::default()
    });
    state.speech_backend = resolved_speech_backend();
    // The report is held here, not in the settings file (it describes the
    // filesystem, not a preference), and merged into the state every other
    // reader already receives.
    state.dependencies = dependencies.clone();
    state
}

/// The body of the notification a Speak Action gets when there is no engine
/// to run it at all — a Tokio runtime that would not start, or a model cache
/// directory that could not be resolved.
///
/// The reason is carried through rather than flattened into a generic
/// sentence: every other failure in this story names the concrete thing that
/// is wrong (the missing path, the missing sample), and a startup failure is
/// no less actionable for having happened earlier.
fn engine_unavailable_body(reason: Option<&str>) -> String {
    match reason {
        Some(reason) => format!("The speech engine could not be set up: {reason}"),
        None => "The speech engine could not be set up on this machine.".to_string(),
    }
}

/// Tell the user the engine is unavailable. A notification that cannot be
/// delivered is logged, not escalated — there is nowhere else to report it.
fn notify_engine_unavailable(notifications: &dyn NotificationPort, reason: Option<&str>) {
    if let Err(delivery) = notifications.notify(
        voice_me_core::GENERATION_FAILED_SUMMARY,
        &engine_unavailable_body(reason),
    ) {
        eprintln!("could not show the failure notification: {delivery}");
    }
}

/// The `VirtualMicPort` used when the per-OS adapter could not even be
/// constructed — on Linux, a config directory that will not resolve.
///
/// A stand-in rather than an `Option<Arc<dyn VirtualMicPort>>` on purpose:
/// `speak` already owns the promise that a failed Speak Action tells the
/// user exactly once, in words, and a `None` here would mean re-implementing
/// that promise in the event loop for one rare cause. Instead the failure
/// becomes the same domain error every other virtual-mic failure is, carries
/// its original reason, and travels the one path that is already tested.
///
/// Linux-only because it is the only OS whose adapter has a fallible
/// constructor; `WindowsVirtualMicAdapter` says the same thing itself.
#[cfg(target_os = "linux")]
struct UnavailableVirtualMic(String);

#[cfg(target_os = "linux")]
impl VirtualMicPort for UnavailableVirtualMic {
    fn play(&self, _audio: &voice_me_core::AudioBuffer) -> Result<(), VoiceMeError> {
        Err(VoiceMeError::VirtualMicUnavailable(self.0.clone()))
    }
}

/// What a startup ensure did, so the caller can say so and a test can
/// assert it without a running audio server.
#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
enum EnsureOutcome {
    /// The machine was already set up: nothing loaded, nothing written.
    AlreadyPresent,
    /// The device was absent and is now installed; the drop-in is at this path.
    Installed(std::path::PathBuf),
    /// Nothing could be done, and this is why. Never fatal — the Speak
    /// Action's own notification is what reaches the user.
    Failed(String),
}

/// The two adapter calls the startup ensure makes, named so the sequence
/// between them is testable.
///
/// A private trait in the composition root rather than new public surface in
/// `voice-me-audio-linux`: the ordering decision — check first, install only
/// if absent, never propagate — belongs to this file, and it is the only
/// part of Decision 1 that a live-device test cannot already reach.
#[cfg(target_os = "linux")]
trait VirtualMicInstaller {
    fn is_present(&self) -> Result<bool, VoiceMeError>;
    fn install(&self) -> Result<std::path::PathBuf, VoiceMeError>;
}

#[cfg(target_os = "linux")]
impl VirtualMicInstaller for LinuxVirtualMicAdapter {
    fn is_present(&self) -> Result<bool, VoiceMeError> {
        LinuxVirtualMicAdapter::is_present(self)
    }

    fn install(&self) -> Result<std::path::PathBuf, VoiceMeError> {
        LinuxVirtualMicAdapter::install(self)
    }
}

/// Decision 1, as a decision: check, install only if absent, and turn every
/// failure into a description rather than a propagated error.
///
/// [`LinuxVirtualMicAdapter::install`] is idempotent and already refuses to
/// leave two devices behind, so "ensure" is the whole contract — the
/// presence check in front of it exists only so a machine that is already
/// set up loads no module and writes no file. A failed check is not followed
/// by an install attempt: no audio server to ask is also no audio server to
/// install into.
#[cfg(target_os = "linux")]
fn ensure_device(installer: &dyn VirtualMicInstaller) -> EnsureOutcome {
    match installer.is_present() {
        Ok(true) => return EnsureOutcome::AlreadyPresent,
        Ok(false) => {}
        Err(error) => {
            return EnsureOutcome::Failed(format!(
                "could not check for the virtual microphone: {error}"
            ));
        }
    }

    match installer.install() {
        Ok(conf) => EnsureOutcome::Installed(conf),
        Err(error) => {
            EnsureOutcome::Failed(format!("could not install the virtual microphone: {error}"))
        }
    }
}

/// Say what the startup ensure did, and nothing more.
///
/// Every failure is logged and swallowed: an app that will not launch
/// because an audio server is missing would be worse than one whose first
/// Speak Action says so in a notification.
#[cfg(target_os = "linux")]
fn report_ensure(outcome: EnsureOutcome) {
    match outcome {
        EnsureOutcome::AlreadyPresent => {}
        EnsureOutcome::Installed(conf) => println!(
            "installed the virtual microphone; drop-in at {}",
            conf.display()
        ),
        EnsureOutcome::Failed(reason) => eprintln!("{reason}"),
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use voice_me_core::{AppState, VoiceMeError};
    use voice_me_hotkey_linux::SessionKind;

    /// A store whose Reference Voice Sample appears only on the second
    /// `load` — i.e. recorded in Settings after launch.
    struct SampleAppearsLater {
        loads: AtomicUsize,
    }

    impl SettingsStore for SampleAppearsLater {
        fn load(&self) -> Result<AppState, VoiceMeError> {
            let nth = self.loads.fetch_add(1, Ordering::SeqCst);
            Ok(AppState {
                reference_voice_sample: (nth > 0).then(|| PathBuf::from("/data/sample.wav")),
                ..AppState::default()
            })
        }

        fn save_reference_voice_sample(&self, _wav: &[u8]) -> Result<AppState, VoiceMeError> {
            unimplemented!()
        }

        fn save_selected_mic_device(
            &self,
            _device: Option<&str>,
        ) -> Result<AppState, VoiceMeError> {
            unimplemented!()
        }

        fn save_hotkey(&self, _hotkey: Option<&str>) -> Result<AppState, VoiceMeError> {
            unimplemented!()
        }
    }

    #[derive(Default)]
    struct RecordingNotifier {
        shown: Mutex<Vec<(String, String)>>,
    }

    impl NotificationPort for RecordingNotifier {
        fn notify(&self, summary: &str, body: &str) -> Result<(), VoiceMeError> {
            self.shown
                .lock()
                .unwrap()
                .push((summary.to_string(), body.to_string()));
            Ok(())
        }
    }

    #[test]
    fn each_speak_action_reads_the_settings_again() {
        let store: Arc<dyn SettingsStore> = Arc::new(SampleAppearsLater {
            loads: AtomicUsize::new(0),
        });

        assert!(
            current_state(&store, &DependencyOutcome::Pending)
                .reference_voice_sample
                .is_none(),
            "nothing recorded yet at launch"
        );
        assert!(
            current_state(&store, &DependencyOutcome::Pending)
                .reference_voice_sample
                .is_some(),
            "a sample recorded in Settings since launch has to count — \
             snapshotting state at startup would fail every later Speak Action"
        );
    }

    #[test]
    fn the_resolved_backend_rides_along_with_every_read() {
        let store: Arc<dyn SettingsStore> = Arc::new(SampleAppearsLater {
            loads: AtomicUsize::new(0),
        });

        assert_eq!(
            current_state(&store, &DependencyOutcome::Pending).speech_backend,
            SpeechBackend::CPU,
            "AD-9: the engine reads its backend off AppState, so it has to be written there"
        );
    }

    #[test]
    fn an_unavailable_engine_is_reported_with_its_reason() {
        let notifier = RecordingNotifier::default();

        notify_engine_unavailable(&notifier, Some("could not resolve a cache directory"));

        let shown = notifier.shown.lock().unwrap();
        assert_eq!(shown.len(), 1, "the typed line is never dropped in silence");
        assert_eq!(shown[0].0, voice_me_core::GENERATION_FAILED_SUMMARY);
        assert!(
            shown[0].1.contains("could not resolve a cache directory"),
            "the startup reason has to survive to the notification: {}",
            shown[0].1
        );
    }

    #[test]
    fn an_unavailable_engine_with_no_recorded_reason_still_says_something() {
        let notifier = RecordingNotifier::default();

        notify_engine_unavailable(&notifier, None);

        assert_eq!(notifier.shown.lock().unwrap().len(), 1);
    }

    /// A stand-in for the two adapter calls the startup ensure makes, so
    /// Decision 1's sequencing is checked without an audio server.
    struct FakeInstaller {
        /// The presence answer, or the `VirtualMicUnavailable` reason this
        /// device is unreachable for. Reasons are held as `String`s rather
        /// than `VoiceMeError`s only because the latter is not `Clone`; each
        /// is returned verbatim, so the test sees the error shape the real
        /// adapter produces rather than one wrapped a second time.
        present: Result<bool, String>,
        install_fails_with: Option<String>,
        installs: Mutex<usize>,
    }

    impl FakeInstaller {
        fn present(present: bool) -> Self {
            Self {
                present: Ok(present),
                install_fails_with: None,
                installs: Mutex::new(0),
            }
        }

        fn unreachable() -> Self {
            Self {
                present: Err("no audio server to connect to".to_string()),
                install_fails_with: None,
                installs: Mutex::new(0),
            }
        }

        fn install_fails(reason: &str) -> Self {
            Self {
                install_fails_with: Some(reason.to_string()),
                ..Self::present(false)
            }
        }
    }

    impl VirtualMicInstaller for FakeInstaller {
        fn is_present(&self) -> Result<bool, VoiceMeError> {
            match &self.present {
                Ok(present) => Ok(*present),
                Err(reason) => Err(VoiceMeError::VirtualMicUnavailable(reason.clone())),
            }
        }

        fn install(&self) -> Result<std::path::PathBuf, VoiceMeError> {
            *self.installs.lock().unwrap() += 1;
            match self.install_fails_with.as_ref() {
                Some(reason) => Err(VoiceMeError::VirtualMicUnavailable(reason.clone())),
                None => Ok(std::path::PathBuf::from("/tmp/voice-me.conf")),
            }
        }
    }

    /// A machine that is already set up must load no module and write no
    /// file — that is the only reason the presence check exists.
    #[test]
    fn a_device_that_is_already_there_is_left_alone() {
        let installer = FakeInstaller::present(true);

        assert_eq!(ensure_device(&installer), EnsureOutcome::AlreadyPresent);
        assert_eq!(*installer.installs.lock().unwrap(), 0);
    }

    /// Decision 1's whole point: a fresh machine ends up with a device
    /// without the user running anything by hand.
    #[test]
    fn an_absent_device_is_installed_once() {
        let installer = FakeInstaller::present(false);

        assert_eq!(
            ensure_device(&installer),
            EnsureOutcome::Installed(std::path::PathBuf::from("/tmp/voice-me.conf"))
        );
        assert_eq!(*installer.installs.lock().unwrap(), 1);
    }

    /// No audio server to ask is also no audio server to install into, and
    /// neither is a reason to refuse to launch: the first Speak Action is
    /// what tells the user, through its own notification.
    #[test]
    fn an_unreachable_audio_server_is_described_and_never_installed_into() {
        let installer = FakeInstaller::unreachable();

        let outcome = ensure_device(&installer);

        assert!(
            matches!(&outcome, EnsureOutcome::Failed(reason) if reason.contains("audio server")),
            "the reason has to survive for the log: {outcome:?}"
        );
        assert_eq!(*installer.installs.lock().unwrap(), 0);
    }

    /// The other half of the non-fatal promise: the check succeeded and the
    /// install itself is what failed. The app still launches; the reason is
    /// carried out for the log rather than propagated.
    #[test]
    fn an_install_that_fails_is_described_and_never_propagated() {
        let installer = FakeInstaller::install_fails("the config directory is read-only");

        let outcome = ensure_device(&installer);

        assert!(
            matches!(&outcome, EnsureOutcome::Failed(reason) if reason.contains("read-only")),
            "the reason has to survive for the log: {outcome:?}"
        );
        assert_eq!(
            *installer.installs.lock().unwrap(),
            1,
            "the install was attempted — it is the failing step, not a skipped one"
        );
    }

    /// An adapter that could not be constructed still has to fail the way
    /// every other virtual-mic failure does, carrying its reason, so `speak`
    /// notifies once and names the right half of the app.
    #[test]
    fn a_virtual_microphone_that_could_not_be_built_fails_as_a_domain_error() {
        let mic = UnavailableVirtualMic("no config directory".to_string());

        let error = mic
            .play(&voice_me_core::AudioBuffer::new(vec![0.1; 24]))
            .unwrap_err();

        assert!(
            matches!(&error, VoiceMeError::VirtualMicUnavailable(reason)
                if reason.contains("no config directory")),
            "the startup reason has to survive to the notification: {error}"
        );
    }

    /// Story 3.4's gate, as a decision: which shape a hotkey press opens.
    mod overlay_gate {
        use voice_me_core::{Dependency, DependencyKind, DependencyReport, SpeechBackend};

        use super::*;

        fn report(rows: Vec<Dependency>) -> DependencyOutcome {
            DependencyOutcome::Ready(DependencyReport::new(SpeechBackend::CPU, rows))
        }

        fn ready_engine() -> Vec<Dependency> {
            vec![
                Dependency::ready(
                    DependencyKind::OnnxRuntime,
                    "ONNX Runtime",
                    "Loaded from /rt",
                ),
                Dependency::ready(
                    DependencyKind::ModelWeights,
                    "Speech model files (Q4)",
                    "All 9 files present in /cache.",
                ),
            ]
        }

        #[test]
        fn a_missing_speech_engine_dependency_blocks_and_is_named() {
            let mut rows = ready_engine();
            rows[1] = Dependency::missing(
                DependencyKind::ModelWeights,
                "Speech model files (Q4)",
                "Missing: /cache/onnx/language_model_q4.onnx",
            );

            let blocker = overlay_blocker(&report(rows)).expect("this press opens blocked");

            assert!(
                blocker.contains("language_model_q4.onnx"),
                "the overlay names the specific blocker, not `a dependency`: {blocker}"
            );
        }

        /// Decision 3, and the story's last acceptance criterion: the user
        /// can still type; the failure surfaces at playback instead.
        #[test]
        fn a_missing_virtual_microphone_does_not_block() {
            let mut rows = ready_engine();
            rows.push(Dependency::missing(
                DependencyKind::VirtualMicrophone,
                "Virtual Microphone",
                "The device is not loaded.",
            ));

            assert_eq!(overlay_blocker(&report(rows)), None);
        }

        #[test]
        fn an_all_ready_report_opens_the_ordinary_overlay() {
            assert_eq!(overlay_blocker(&report(ready_engine())), None);
        }

        /// Blocking on "we have not looked yet" would turn a slow startup
        /// into a refused Speak Action.
        #[test]
        fn a_check_that_has_not_completed_yet_does_not_block() {
            assert_eq!(overlay_blocker(&DependencyOutcome::Pending), None);
        }

        /// A check that could not run at all covers every path the engine
        /// would have read, so the press is blocked — with the reason.
        #[test]
        fn a_failed_check_blocks_with_its_reason() {
            let blocker = overlay_blocker(&DependencyOutcome::Failed(
                "could not resolve a cache directory".to_string(),
            ))
            .expect("nothing could be verified, so nothing can be generated");

            assert!(
                blocker.contains("could not resolve a cache directory"),
                "{blocker}"
            );
        }

        /// Decision 2, as a decision: the startup check opens Settings for
        /// a gap it found, and nothing else does.
        #[test]
        fn only_the_startup_check_opens_settings_and_only_for_a_gap() {
            assert!(
                should_auto_open_dependencies(false, true),
                "the startup check found something — the user has to be told"
            );
            assert!(
                !should_auto_open_dependencies(false, false),
                "a clean startup check must not pop a window unbidden"
            );
            assert!(
                !should_auto_open_dependencies(true, true),
                "a later `Check again` never re-opens the window it was pressed in"
            );
        }
    }

    #[test]
    fn an_x11_session_gets_a_true_always_on_top_overlay() {
        assert_eq!(
            overlay_window_kind_for(SessionKind::X11),
            WindowKind::PopUp,
            "X11 is the only session where the overlay can be drawn over a fullscreen window"
        );
    }

    #[test]
    fn a_wayland_session_gets_a_plain_window() {
        assert_eq!(
            overlay_window_kind_for(SessionKind::Wayland),
            WindowKind::Normal,
            "`PopUp` would silently behave like `Normal` here; say so outright"
        );
    }
}

fn main() {
    let settings_store: Arc<dyn SettingsStore> =
        Arc::new(FileSettingsStore::new().expect("failed to resolve settings/data directories"));

    // Story 1.5: determine whether an active Reference Voice Sample already
    // exists before deciding whether to auto-open the window at startup
    // (Story 2.2 — first run only; otherwise the app stays tray-only). A
    // load failure is treated the same as "no sample" — falling back to the
    // empty-state copy is safe, whereas silently treating an unreadable
    // store as "has a sample" would hide the first-run prompt from someone
    // who actually needs it.
    let has_active_sample = settings_store
        .load()
        .ok()
        .is_some_and(|state| state.reference_voice_sample.is_some());

    // Tray residency (Story 2.2) only holds under an explicit quit mode.
    // gpui's default quits the process the moment the last window closes on
    // non-macOS — which would make the Prompt Overlay's own dismissal
    // (Story 2.4) kill a tray-only session on the first `Enter` or
    // `Escape`. The tray's "Quit" item already calls `cx.quit()` itself.
    let app = gpui_kit::application()
        .with_quit_mode(QuitMode::Explicit)
        .with_assets(gpui_kit::assets::Assets);

    app.run(move |cx| {
        gpui_kit::init(cx);

        // AD-5: one Tokio runtime alongside GPUI, owned by the composition
        // root. Every blocking job in this process — generation above all —
        // goes through it, so GPUI's main-thread executor never blocks.
        // Without it `tokio_bridge::spawn_blocking` has no global to read.
        // The app still starts without it: the tray, Settings and the hotkey
        // are all unaffected, and the Speak Action is the only thing that
        // needs the runtime. But it cannot merely be logged and forgotten —
        // `tokio_bridge::spawn_blocking` resolves the runtime through
        // `cx.global`, which *panics* when the global was never set, so a
        // failed install has to disable the engine rather than leave a
        // landmine under the warm-up and every later Speak Action.
        let runtime_error = tokio_bridge::TokioRuntime::install(cx)
            .err()
            .map(|error| error.to_string());
        if let Some(error) = runtime_error.as_ref() {
            eprintln!("could not start the Tokio runtime: {error}");
        }

        let (event_tx, mut event_rx) = mpsc::unbounded::<AppEvent>();

        // The speech engine holds its four ONNX Runtime sessions for the
        // process lifetime (AD-10) and serializes generations itself, so it
        // is built once here and shared.
        //
        // A cache directory that cannot be resolved — or a Tokio runtime
        // that would not start — is not a reason to lose the tray, the
        // hotkey and Settings: every other adapter failure in this function
        // degrades that way, and these are no more fatal. The engine is
        // simply unavailable, and the Speak Action reports why, in words,
        // the moment it is pressed. `engine_error` carries that reason all
        // the way to the notification, because "could not be set up" with no
        // cause is exactly the unactionable message the rest of this story
        // works to avoid.
        let (tts_port, engine_error): (Option<Arc<dyn TtsPort>>, Option<String>) = match (
            runtime_error,
            TtsAdapter::from_state(&current_state(&settings_store, &DependencyOutcome::Pending)),
        ) {
            (None, Ok(adapter)) => (Some(Arc::new(adapter)), None),
            (Some(error), _) => (None, Some(error)),
            (None, Err(error)) => {
                eprintln!("could not set up the speech engine: {error}");
                (None, Some(error.to_string()))
            }
        };

        // Story 3.1: detection only — no network, nothing installed. The
        // concrete adapter is kept alongside the port because the startup
        // check runs on a background thread, and a `dyn` port would have to
        // promise `Send` to every implementor for that one call site.
        let deps_adapter = DepsAdapter::new();
        let deps_port: Arc<dyn DependencyProvisioningPort> = Arc::new(deps_adapter);

        #[cfg(target_os = "linux")]
        let notification_port: Arc<dyn NotificationPort> = Arc::new(LinuxNotificationAdapter);
        #[cfg(target_os = "windows")]
        let notification_port: Arc<dyn NotificationPort> = Arc::new(WindowsNotificationAdapter);

        // Story 2.9: where the generated line actually goes. Built beside
        // the notification port because `speak` needs both — the second is
        // how the user hears about the first not working.
        //
        // Built exactly once and shared with the startup ensure below: two
        // constructions of the same adapter would report one unresolvable
        // config directory twice, in two different sentences, and could in
        // principle disagree about it.
        #[cfg(target_os = "linux")]
        let (virtual_mic_port, mic_adapter): (
            Arc<dyn VirtualMicPort>,
            Option<Arc<LinuxVirtualMicAdapter>>,
        ) = match LinuxVirtualMicAdapter::new() {
            Ok(adapter) => {
                let adapter = Arc::new(adapter);
                (adapter.clone(), Some(adapter))
            }
            Err(error) => {
                eprintln!("could not set up the virtual microphone: {error}");
                (Arc::new(UnavailableVirtualMic(error.to_string())), None)
            }
        };
        // Story 2.8 implements this; until then `play` returns a domain
        // error naming itself rather than panicking on the first Speak
        // Action (spec-2-9 Decision 2).
        #[cfg(target_os = "windows")]
        let virtual_mic_port: Arc<dyn VirtualMicPort> = Arc::new(WindowsVirtualMicAdapter);

        // Decision 1: the device is ensured at launch, off the main thread —
        // `cx.background_spawn` rather than the Tokio bridge, because this
        // must happen even on a run where the runtime would not start, and
        // it is the only thing standing between a fresh machine and a Speak
        // Action nobody hears. An adapter that could not be built has
        // already said so; there is nothing to ensure with.
        #[cfg(target_os = "linux")]
        if let Some(adapter) = mic_adapter {
            cx.background_spawn(async move { report_ensure(ensure_device(adapter.as_ref())) })
                .detach();
        }

        // The hotkey adapter is built here, with the shared `AppEvent`
        // sender, but binds nothing until there is a combination to bind —
        // so an app that has never had a hotkey configured starts with no
        // hotkey active and no error (including on a Wayland session with
        // no `input`-group membership).
        #[cfg(target_os = "linux")]
        let hotkey_port: Arc<dyn HotkeyPort> = Arc::new(LinuxHotkeyAdapter::new(event_tx.clone()));
        #[cfg(target_os = "windows")]
        let hotkey_port: Arc<dyn HotkeyPort> = Arc::new(WindowsHotkeyAdapter);

        // A failed `start_listening` (most importantly: no read access to
        // `/dev/input` on Wayland) must not stop the app — it starts
        // normally and the Hotkey tab says what the problem is, in words,
        // whenever the window is opened.
        let hotkey_startup_error: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

        // Tracks the currently open Settings window (if any), so a second
        // "Settings…" click activates the existing window instead of
        // opening a duplicate.
        let window_slot: Rc<RefCell<Option<WindowHandle<Root>>>> = Rc::new(RefCell::new(None));

        // The view inside that window, kept beside the handle so a report
        // arriving while Settings is open can be pushed into the
        // Dependencies tab. A handle alone would only let us activate the
        // window, not talk to what is in it.
        let settings_view_slot: Rc<RefCell<Option<gpui_kit::Entity<SettingsView>>>> =
            Rc::new(RefCell::new(None));

        // The one held Dependency Check result (Story 3.1's Design Notes):
        // `voice-me-deps` holds nothing, this is the only copy, and both
        // readers — the Dependencies tab and the hotkey gate — read it.
        let dependency_outcome: Rc<RefCell<DependencyOutcome>> =
            Rc::new(RefCell::new(DependencyOutcome::Pending));

        // Decision 2: Settings opens itself at most once per launch, and
        // only for what the *startup* check found. Deliberately not
        // persisted — it must do so again on the next launch while the
        // dependency is still missing.
        let dependencies_auto_opened = Rc::new(RefCell::new(false));

        // The same one-at-a-time guarantee for the Prompt Overlay: a press
        // while one is already open activates it instead of stacking a
        // second window on top.
        let overlay_slot: Rc<RefCell<Option<WindowHandle<Root>>>> = Rc::new(RefCell::new(None));

        // An `Rc<dyn Fn>` rather than a plain closure because two callers
        // need it: the tray/first-run paths below, and the
        // "open Settings *on the Dependencies tab*" wrapper that Stories
        // 3.1 and 3.4 both go through.
        let open_settings: Rc<dyn Fn(&mut App)> = Rc::new({
            let settings_store = settings_store.clone();
            let hotkey_port = hotkey_port.clone();
            let hotkey_startup_error = hotkey_startup_error.clone();
            let window_slot = window_slot.clone();
            let settings_view_slot = settings_view_slot.clone();
            let deps_port = deps_port.clone();
            let event_tx = event_tx.clone();
            let dependency_outcome = dependency_outcome.clone();
            move |cx: &mut App| {
                if let Some(handle) = window_slot.borrow().as_ref() {
                    let _ = handle.update(cx, |_, window, _| window.activate_window());
                    return;
                }

                // Reload fresh each time a window opens (not just once at
                // binary startup) so re-opening after a sample was saved
                // shows the normal view, not the first-run empty state.
                let loaded_state = settings_store.load().ok();
                let has_active_sample = loaded_state
                    .as_ref()
                    .is_some_and(|state| state.reference_voice_sample.is_some());
                let saved_hotkey = loaded_state.as_ref().and_then(|state| state.hotkey.clone());
                let selected_mic_device = loaded_state.and_then(|state| state.selected_mic_device);
                let hotkey_startup_error = hotkey_startup_error.borrow().clone();

                let settings_store = settings_store.clone();
                let hotkey_port = hotkey_port.clone();
                let window_slot_on_close = window_slot.clone();
                let view_slot_on_close = settings_view_slot.clone();
                let dependencies = DependenciesTab {
                    deps_port: deps_port.clone(),
                    events: event_tx.clone(),
                    speech_backend: resolved_speech_backend(),
                    outcome: dependency_outcome.borrow().clone(),
                };
                let view_slot = settings_view_slot.clone();
                let handle = match cx.open_window(WindowOptions::default(), move |window, cx| {
                    let view = cx.new(|cx| {
                        SettingsView::new(
                            settings_store.clone(),
                            hotkey_port.clone(),
                            has_active_sample,
                            selected_mic_device,
                            saved_hotkey,
                            hotkey_startup_error,
                            dependencies,
                            window,
                            cx,
                        )
                    });
                    *view_slot.borrow_mut() = Some(view.clone());
                    window.on_window_should_close(cx, move |_window, _cx| {
                        *window_slot_on_close.borrow_mut() = None;
                        *view_slot_on_close.borrow_mut() = None;
                        true
                    });
                    cx.new(|cx| Root::new(view, window, cx))
                }) {
                    Ok(handle) => handle,
                    Err(error) => {
                        eprintln!("failed to open window: {error}");
                        return;
                    }
                };

                *window_slot.borrow_mut() = Some(handle);
            }
        });

        // Settings, focused on the Dependencies tab — what a blocked
        // overlay and the startup auto-open both need. Landing the user on
        // Voice would have told them nothing about what is missing.
        let open_dependencies: Rc<dyn Fn(&mut App)> = Rc::new({
            let open_settings = open_settings.clone();
            let settings_view_slot = settings_view_slot.clone();
            move |cx: &mut App| {
                (*open_settings)(cx);
                if let Some(view) = settings_view_slot.borrow().clone() {
                    view.update(cx, |view, cx| view.show_dependencies(cx));
                }
            }
        });

        let open_overlay = {
            let overlay_slot = overlay_slot.clone();
            let event_tx = event_tx.clone();
            let dependency_outcome = dependency_outcome.clone();
            move |cx: &mut App| {
                // Story 3.4: the same summon, a different shape. The gate
                // is read here rather than inside the view, so the overlay
                // never has to know `voice-me-deps` exists.
                let blocker = overlay_blocker(&dependency_outcome.borrow());
                // Re-summon while one is open: activate it, keeping whatever
                // is already typed. The view closes itself with
                // `remove_window`, which never runs `on_window_should_close`,
                // so a stale handle is normal — `update` failing is how we
                // learn the window is gone, and the slot is cleared so the
                // press still gets a fresh overlay.
                let existing = *overlay_slot.borrow();
                if let Some(handle) = existing {
                    if handle
                        .update(cx, |_, window, _| window.activate_window())
                        .is_ok()
                    {
                        return;
                    }
                    *overlay_slot.borrow_mut() = None;
                }

                let event_tx = event_tx.clone();
                let overlay_slot_on_close = overlay_slot.clone();
                let options = WindowOptions {
                    titlebar: None,
                    // Client-side decorations are what make a borderless
                    // window possible at all on Linux.
                    window_decorations: Some(WindowDecorations::Client),
                    window_background: WindowBackgroundAppearance::Transparent,
                    window_bounds: Some(WindowBounds::centered(
                        size(px(OVERLAY_WIDTH), px(OVERLAY_HEIGHT)),
                        cx,
                    )),
                    kind: overlay_window_kind(),
                    focus: true,
                    is_resizable: false,
                    is_movable: false,
                    is_minimizable: false,
                    ..Default::default()
                };

                let handle = match cx.open_window(options, move |window, cx| {
                    let view = cx.new(|cx| match blocker.clone() {
                        Some(blocker) => {
                            PromptOverlayView::blocked(event_tx.clone(), blocker, window, cx)
                        }
                        None => PromptOverlayView::new(event_tx.clone(), window, cx),
                    });
                    window.on_window_should_close(cx, move |_window, _cx| {
                        *overlay_slot_on_close.borrow_mut() = None;
                        true
                    });
                    cx.new(|cx| Root::new(view, window, cx))
                }) {
                    Ok(handle) => handle,
                    Err(error) => {
                        // The app stays tray-resident: a window that would
                        // not open is not a reason to lose the tray.
                        eprintln!("failed to open the prompt overlay: {error}");
                        return;
                    }
                };

                *overlay_slot.borrow_mut() = Some(handle);
            }
        };

        // Tray registration failure leaves the app with no way in unless we
        // fall back to opening the window — otherwise a headless process
        // with neither tray nor window would be silently unreachable.
        #[cfg(target_os = "linux")]
        if let Err(error) = LinuxTrayAdapter.show(cx, event_tx.clone()) {
            eprintln!("failed to show tray: {error}");
            (*open_settings)(cx);
        }
        #[cfg(target_os = "windows")]
        if let Err(error) = WindowsTrayAdapter.show(cx, event_tx.clone()) {
            eprintln!("failed to show tray: {error}");
            (*open_settings)(cx);
        }

        // Story 2.3: re-activate a hotkey saved in a previous run, after
        // the tray is up so a hotkey failure never costs the app its tray
        // presence. Nothing is bound when none was ever saved.
        if let Some(saved_hotkey) = settings_store.load().ok().and_then(|state| state.hotkey)
            && let Err(error) = hotkey_port.start_listening(&saved_hotkey, event_tx.clone())
        {
            eprintln!("failed to activate the hotkey {saved_hotkey}: {error}");
            *hotkey_startup_error.borrow_mut() = Some(error.to_string());
        }

        // First run only (Story 1.5 behavior, unchanged): auto-open the
        // window in addition to the tray being present. Otherwise the app
        // starts tray-only. Safe to call even if the tray-failure fallback
        // above already opened the window — `open_settings` activates the
        // existing window instead of duplicating it.
        if !has_active_sample {
            (*open_settings)(cx);
        }

        // Decision 3: warm-up is eager, but only for someone who already has
        // a Reference Voice Sample. Session construction measured 86–110 s
        // (spec-2-5), so paying it in the background at startup is what puts
        // it behind the user's first Speak Action rather than in front of
        // it — while a first-run user, who has no voice to clone yet, pays
        // nothing at all.
        if has_active_sample && let Some(tts) = tts_port.clone() {
            let warm_up = tokio_bridge::spawn_blocking(cx, move || tts.warm_up());
            cx.spawn(async move |_| {
                if let Err(error) = warm_up.await {
                    // Not notified: the user did not ask for this, and the
                    // same failure is reported properly, with the same
                    // message, the moment they actually press the hotkey.
                    eprintln!("speech engine warm-up failed: {error}");
                }
            })
            .detach();
        }

        // Story 3.1: the startup check, in the background. Nothing waits on
        // it — the tray, the hotkey and Settings are all up already — and
        // its result arrives on the same channel every on-demand check
        // reports on, so there is exactly one path into the held outcome.
        //
        // `cx.background_spawn` rather than the Tokio bridge, for the same
        // reason the virtual-microphone ensure uses it: this must happen
        // even on a run where the Tokio runtime would not start.
        {
            let backend = resolved_speech_backend();
            let events = event_tx.clone();
            let check = cx.background_spawn(async move { deps_adapter.check(backend, events) });
            let dependency_outcome = dependency_outcome.clone();
            let settings_view_slot = settings_view_slot.clone();
            let open_dependencies = open_dependencies.clone();
            let dependencies_auto_opened = dependencies_auto_opened.clone();
            cx.spawn(async move |cx| {
                // Only the failure needs handling here: a check that *ran*
                // reports itself by event, below.
                let Err(error) = check.await else { return };
                eprintln!("the dependency check could not run: {error}");
                let outcome = DependencyOutcome::Failed(error.to_string());
                *dependency_outcome.borrow_mut() = outcome.clone();
                cx.update(|cx| {
                    if let Some(view) = settings_view_slot.borrow().clone() {
                        view.update(cx, |view, cx| view.set_dependency_outcome(outcome, cx));
                    }
                    // A check that could not run is as missing as it gets:
                    // nothing could be verified at all.
                    let first_check = !*dependencies_auto_opened.borrow();
                    *dependencies_auto_opened.borrow_mut() = true;
                    if should_auto_open_dependencies(!first_check, true) {
                        (*open_dependencies)(cx);
                    }
                });
            })
            .detach();
        }

        cx.spawn(async move |cx| {
            while let Some(event) = event_rx.next().await {
                match event {
                    AppEvent::SettingsRequested => {
                        cx.update(|cx| (*open_settings)(cx));
                    }
                    AppEvent::HotkeyPressed => {
                        cx.update(|cx| {
                            // Story 3.4: the overlay opens either way. When
                            // a speech-engine dependency is missing it
                            // opens blocked *and* sends the user to the one
                            // place that explains it.
                            //
                            // Settings first, overlay second, and that
                            // order is load-bearing: the overlay dismisses
                            // itself when it loses window activation, so
                            // opening Settings on top of it would make the
                            // notice vanish the instant it appeared. This
                            // way the overlay ends up focused, over a
                            // Settings window already showing the tab that
                            // explains the blocker.
                            //
                            // Unless one is already up: a re-summon only
                            // activates the existing overlay, so opening
                            // Settings in front of it would take its focus
                            // and dismiss the very notice this press was
                            // meant to bring back. A no-op `update` is how
                            // we tell a live overlay from a stale handle
                            // without disturbing either.
                            let existing = *overlay_slot.borrow();
                            let already_open = existing
                                .is_some_and(|handle| handle.update(cx, |_, _, _| ()).is_ok());
                            if !already_open
                                && overlay_blocker(&dependency_outcome.borrow()).is_some()
                            {
                                (*open_dependencies)(cx);
                            }
                            open_overlay(cx);
                        });
                    }
                    AppEvent::DependencyCheckCompleted { report } => {
                        let anything_missing = report.has_missing();
                        let outcome = DependencyOutcome::Ready(report);
                        *dependency_outcome.borrow_mut() = outcome.clone();

                        let settings_view_slot = settings_view_slot.clone();
                        let open_dependencies = open_dependencies.clone();
                        let dependencies_auto_opened = dependencies_auto_opened.clone();
                        cx.update(|cx| {
                            // An open Settings window re-renders its rows
                            // from the new report, so "Check again" lands
                            // without the user leaving the window.
                            if let Some(view) = settings_view_slot.borrow().clone() {
                                view.update(cx, |view, cx| {
                                    view.set_dependency_outcome(outcome, cx)
                                });
                            }

                            // Decision 2: the *startup* check — the first
                            // one to complete this launch — opens Settings
                            // by itself when it finds a gap. Later checks
                            // never do; the window is already open by then.
                            let first_check = !*dependencies_auto_opened.borrow();
                            *dependencies_auto_opened.borrow_mut() = true;
                            if should_auto_open_dependencies(!first_check, anything_missing) {
                                (*open_dependencies)(cx);
                            }
                        });
                    }
                    AppEvent::SpeakRequested { text } => {
                        let settings_store = settings_store.clone();
                        let notifications = notification_port.clone();
                        let virtual_mic = virtual_mic_port.clone();
                        let Some(tts) = tts_port.clone() else {
                            // The engine never came up at startup. Say so
                            // rather than dropping the line silently — this
                            // is the same contract `speak` honours for every
                            // other failure (UX-DR15). Sent from a background
                            // task because a notification is a synchronous
                            // D-Bus round trip and this loop runs on GPUI's
                            // main thread; `cx.background_spawn` rather than
                            // the Tokio bridge, since a failed runtime is one
                            // of the reasons we are in this branch at all.
                            let reason = engine_error.clone();
                            cx.update(|cx| {
                                cx.background_spawn(async move {
                                    notify_engine_unavailable(
                                        notifications.as_ref(),
                                        reason.as_deref(),
                                    );
                                })
                                .detach();
                            });
                            continue;
                        };
                        cx.update(|cx| {
                            let state =
                                current_state(&settings_store, &dependency_outcome.borrow());
                            let work = tokio_bridge::spawn_blocking(cx, move || {
                                // Generation *and* playback, on the blocking
                                // pool: `play` blocks until the audio server
                                // has drained the buffer (AD-5).
                                let audio = voice_me_core::speak(
                                    &text,
                                    &state,
                                    tts.as_ref(),
                                    virtual_mic.as_ref(),
                                    notifications.as_ref(),
                                )?;
                                // A working run is otherwise indistinguishable
                                // from a broken one: nothing is written, and
                                // the audio went somewhere only another
                                // application can hear.
                                println!(
                                    "spoke {:.2} s of generated speech",
                                    audio.duration().as_secs_f64()
                                );
                                Ok(())
                            });

                            // Detached rather than awaited here: this loop is
                            // the only reader of the `AppEvent` channel, so
                            // awaiting a ~20 s generation inside it would
                            // leave the tray and the hotkey unresponsive for
                            // its whole duration. `speak` has already told
                            // the user about any failure; this only logs.
                            cx.spawn(async move |_| {
                                if let Err(error) = work.await {
                                    eprintln!("speak failed: {error}");
                                }
                            })
                            .detach();
                        });
                    }
                    // Not-yet-relevant variants (`#[non_exhaustive]` requires
                    // a wildcard arm).
                    _ => {}
                }
            }
        })
        .detach();
    });
}
