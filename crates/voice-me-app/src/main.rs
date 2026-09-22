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
use voice_me_core::{
    AppEvent, AppState, AudioBuffer, FileSettingsStore, HotkeyPort, NotificationPort,
    SettingsStore, SpeechBackend, TtsPort, tokio_bridge,
};
use voice_me_tts::TtsAdapter;
use voice_me_ui::{PromptOverlayView, SettingsView};

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

/// Set on a directory path, every generated utterance is also written there
/// as a wav.
///
/// The temporary seam Story 2.9 removes. `VirtualMicPort::play` is still
/// `todo!()`, so handing it the buffer would panic — this story stops at a
/// produced `AudioBuffer`, and this gate is what makes the end-to-end path
/// listenable in the meantime.
const DEBUG_WAV_DIR_VAR: &str = "VOICE_ME_DEBUG_WAV_DIR";

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

/// The persisted settings, plus this run's resolved backend.
///
/// Re-read per Speak Action rather than snapshotted at startup: a Reference
/// Voice Sample recorded in Settings since launch has to count, and so would
/// a speech language edited in `settings.toml`.
fn current_state(settings_store: &Arc<dyn SettingsStore>) -> AppState {
    let mut state = settings_store.load().unwrap_or_else(|error| {
        // Falling back to defaults keeps the Speak Action reaching a real
        // failure it can name ("no Reference Voice Sample") instead of the
        // app silently doing nothing.
        eprintln!("could not read settings: {error}");
        AppState::default()
    });
    state.speech_backend = resolved_speech_backend();
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

/// What this story does with a finished utterance, in place of playback.
fn report_generated(audio: &AudioBuffer) {
    println!(
        "generated {:.2} s of speech ({} samples @ {} Hz)",
        audio.duration().as_secs_f64(),
        audio.len(),
        audio.sample_rate()
    );

    let Some(dir) = std::env::var_os(DEBUG_WAV_DIR_VAR) else {
        return;
    };
    let path = std::path::PathBuf::from(dir).join(format!(
        "voice-me-{}.wav",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_millis())
            .unwrap_or_default()
    ));
    if let Err(error) = write_wav(&path, audio) {
        eprintln!("could not write the debug wav: {error}");
    } else {
        println!("wrote {}", path.display());
    }
}

/// 16-bit PCM, because that is what every audio player opens without
/// comment. The engine's own buffer stays f32 (AD-11) — this conversion
/// exists only so a human can listen to the result.
fn write_wav(path: &std::path::Path, audio: &AudioBuffer) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: audio.sample_rate(),
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    // The directory is whatever the user put in the environment variable,
    // so it routinely does not exist yet — including for the README's own
    // `/tmp/voice-me` example on a freshly booted machine. Creating it is
    // the difference between a listenable wav and a line in the log nobody
    // reads.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("{parent:?}: {error}"))?;
    }
    let mut writer =
        hound::WavWriter::create(path, spec).map_err(|error| format!("{path:?}: {error}"))?;
    for &sample in audio.samples() {
        writer
            .write_sample((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .map_err(|error| error.to_string())?;
    }
    writer.finalize().map_err(|error| error.to_string())
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
            current_state(&store).reference_voice_sample.is_none(),
            "nothing recorded yet at launch"
        );
        assert!(
            current_state(&store).reference_voice_sample.is_some(),
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
            current_state(&store).speech_backend,
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

    #[test]
    fn the_debug_wav_lands_in_a_directory_that_did_not_exist_yet() {
        let root = tempfile::tempdir().unwrap();
        // The README's own example points at a path that does not exist on a
        // freshly booted machine.
        let path = root.path().join("voice-me").join("utterance.wav");

        write_wav(&path, &AudioBuffer::new(vec![0.5; 240])).unwrap();

        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().sample_rate, voice_me_core::SAMPLE_RATE);
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.len(), 240);
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
            TtsAdapter::from_state(&current_state(&settings_store)),
        ) {
            (None, Ok(adapter)) => (Some(Arc::new(adapter)), None),
            (Some(error), _) => (None, Some(error)),
            (None, Err(error)) => {
                eprintln!("could not set up the speech engine: {error}");
                (None, Some(error.to_string()))
            }
        };

        #[cfg(target_os = "linux")]
        let notification_port: Arc<dyn NotificationPort> = Arc::new(LinuxNotificationAdapter);
        #[cfg(target_os = "windows")]
        let notification_port: Arc<dyn NotificationPort> = Arc::new(WindowsNotificationAdapter);

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

        // The same one-at-a-time guarantee for the Prompt Overlay: a press
        // while one is already open activates it instead of stacking a
        // second window on top.
        let overlay_slot: Rc<RefCell<Option<WindowHandle<Root>>>> = Rc::new(RefCell::new(None));

        let open_settings = {
            let settings_store = settings_store.clone();
            let hotkey_port = hotkey_port.clone();
            let hotkey_startup_error = hotkey_startup_error.clone();
            let window_slot = window_slot.clone();
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
                let handle = match cx.open_window(WindowOptions::default(), move |window, cx| {
                    let view = cx.new(|cx| {
                        SettingsView::new(
                            settings_store.clone(),
                            hotkey_port.clone(),
                            has_active_sample,
                            selected_mic_device,
                            saved_hotkey,
                            hotkey_startup_error,
                            window,
                            cx,
                        )
                    });
                    window.on_window_should_close(cx, move |_window, _cx| {
                        *window_slot_on_close.borrow_mut() = None;
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
        };

        let open_overlay = {
            let overlay_slot = overlay_slot.clone();
            let event_tx = event_tx.clone();
            move |cx: &mut App| {
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
                    let view = cx.new(|cx| PromptOverlayView::new(event_tx.clone(), window, cx));
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
            open_settings(cx);
        }
        #[cfg(target_os = "windows")]
        if let Err(error) = WindowsTrayAdapter.show(cx, event_tx.clone()) {
            eprintln!("failed to show tray: {error}");
            open_settings(cx);
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
            open_settings(cx);
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

        cx.spawn(async move |cx| {
            while let Some(event) = event_rx.next().await {
                match event {
                    AppEvent::SettingsRequested => {
                        cx.update(|cx| open_settings(cx));
                    }
                    AppEvent::HotkeyPressed => {
                        cx.update(|cx| open_overlay(cx));
                    }
                    AppEvent::SpeakRequested { text } => {
                        let settings_store = settings_store.clone();
                        let notifications = notification_port.clone();
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
                            let state = current_state(&settings_store);
                            let work = tokio_bridge::spawn_blocking(cx, move || {
                                let audio = voice_me_core::speak(
                                    &text,
                                    &state,
                                    tts.as_ref(),
                                    notifications.as_ref(),
                                )?;
                                report_generated(&audio);
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
