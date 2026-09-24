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
//! Story 3.2 lets that tab fix what it found. Install on a row runs
//! `DependencyProvisioningPort::provision` on Tokio's blocking pool; its
//! progress and its end arrive here as `ProvisioningProgress` and
//! `ProvisioningFinished`, this file keeps the one copy of each row's
//! install state, and a finished install re-runs the check — so a row turns
//! "ready" and the overlay unblocks without a restart.
//!
//! Story 2.9 closes the loop: the per-OS `VirtualMicPort` adapter is built
//! here and handed to `speak`, which plays the generated buffer through the
//! Virtual Microphone instead of dropping it. The device itself is ensured
//! once at startup, in the background, so a fresh machine needs no manual
//! setup step (spec-2-9 Decision 1).
//!
//! Story 3.3 makes the backend a choice. The selection is persisted through
//! `SettingsStore` and resolved here into the AD-9 backend on every read;
//! the Dependency Check says whether it can run on this machine; the TTS
//! adapter reports what it actually built (`SpeechSessionBuilt`), which is
//! the only source of "Active". A switch that needs another runtime library
//! than the one this process committed is saved and waits for a restart;
//! any other switch rebuilds the adapter for the next Speak Action. Added
//! runtime libraries are probed in a helper process (`--probe-runtime`),
//! never loaded here.
//!
//! Story 3.6 puts a remote engine in the same slot: selecting DeepInfra
//! builds `voice-me-tts-remote`'s adapter, and the Speak Action path does
//! not change. The first hotkey press after that opens the overlay in its
//! confirm-first shape, whose answer is recorded here; Settings' **Delete
//! from DeepInfra** is carried out here too.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

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
    ActiveBackend, AppEvent, AppState, BackendSelection, CheckRequest, DependencyKind,
    DependencyOutcome, DependencyProvisioningPort, DependencyReport, FileSettingsStore, HotkeyPort,
    LocalRuntime, NotificationPort, RemoteProvider, SettingsStore, SpeechBackend,
    SpeechExecutionTarget, StockVoice, TtsPort, VirtualMicPort, tokio_bridge,
};
use voice_me_deps::DepsAdapter;
use voice_me_tts::TtsAdapter;
use voice_me_tts_remote::{
    Azure, AzureTtsAdapter, DeepInfra, RemoteTtsAdapter, SharedSettingsStore,
};
use voice_me_ui::{
    BackendAction, BackendActions, BackendArea, BackendPanel, ConfirmDisclosure, DependenciesTab,
    DisclosureText, PromptOverlayView, RowProvisioning, SettingsView, blocker_notice,
};

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

/// The confirm-first overlay's height (Story 3.6): the provider, the three
/// things sent, and the two buttons. It keeps this height after confirming.
const OVERLAY_DISCLOSURE_HEIGHT: f32 = 196.;

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

/// The helper-process mode (Story 3.3 Decision 1): `voice-me
/// --probe-runtime <path>` loads the library at `path` through `ort`,
/// prints which execution providers it offers, and exits. The app runs
/// itself this way so an added library is never loaded into the main
/// process just to look at it — `ort` commits one library per process.
const PROBE_RUNTIME_FLAG: &str = "--probe-runtime";

/// The one line the helper prints on success, followed by the targets.
const PROBE_OUTPUT_PREFIX: &str = "voice-me-probe-targets:";

/// How long the helper may take before it is killed.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Set on a relaunched process: the pid of the instance that relaunched
/// it, which has to be gone before this one grabs the tray and hotkey.
const RESTART_WAIT_ENV: &str = "VOICE_ME_RESTART_WAIT_PID";

/// Run the helper mode, returning the process exit code.
fn run_runtime_probe(path: Option<std::ffi::OsString>) -> i32 {
    let Some(path) = path else {
        eprintln!("usage: voice-me {PROBE_RUNTIME_FLAG} <path to an ONNX Runtime library>");
        return 2;
    };
    match voice_me_tts::sessions::probe_runtime(Path::new(&path)) {
        Ok(targets) => {
            println!("{}", probe_output_line(&targets));
            0
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

/// `voice-me-probe-targets:cpu,cuda`.
fn probe_output_line(targets: &[SpeechExecutionTarget]) -> String {
    let names = targets
        .iter()
        .map(|target| match target {
            SpeechExecutionTarget::Cpu => "cpu",
            SpeechExecutionTarget::Cuda => "cuda",
            SpeechExecutionTarget::WebGpu => "webgpu",
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{PROBE_OUTPUT_PREFIX}{names}")
}

/// Read what the helper said about `file`. Anything but a clean exit with
/// the one expected line is a refusal carrying the helper's own reason.
fn parse_probe_output(
    succeeded: bool,
    stdout: &str,
    stderr: &str,
    file: &Path,
) -> Result<Vec<SpeechExecutionTarget>, String> {
    let reason = || {
        stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .map(|line| line.trim().to_string())
            .unwrap_or_else(|| "the probe exited without saying why".to_string())
    };
    if !succeeded {
        return Err(format!(
            "{} is not a usable ONNX Runtime library: {}",
            file.display(),
            reason()
        ));
    }
    let Some(line) = stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix(PROBE_OUTPUT_PREFIX))
    else {
        return Err(format!(
            "Probing {} gave no answer: {}",
            file.display(),
            reason()
        ));
    };
    let targets: Vec<_> = line
        .split(',')
        .filter_map(|name| match name.trim() {
            "cpu" => Some(SpeechExecutionTarget::Cpu),
            "cuda" => Some(SpeechExecutionTarget::Cuda),
            "webgpu" => Some(SpeechExecutionTarget::WebGpu),
            _ => None,
        })
        .collect();
    if targets.is_empty() {
        return Err(format!(
            "{} offers none of the CPU, CUDA or WebGPU execution providers.",
            file.display()
        ));
    }
    Ok(targets)
}

/// Probe `file` in a helper process, killing it after [`PROBE_TIMEOUT`].
///
/// Blocking: run it off the main thread. Both pipes are drained on their
/// own threads so a chatty library cannot fill one and wedge the helper.
fn probe_runtime_in_helper(file: &Path) -> Result<Vec<SpeechExecutionTarget>, String> {
    let exe = std::env::current_exe()
        .map_err(|error| format!("Could not find voice-me's own executable: {error}"))?;
    let mut helper = std::process::Command::new(exe);
    helper.arg(PROBE_RUNTIME_FLAG).arg(file);
    run_probe(helper, file, PROBE_TIMEOUT)
}

/// Run the probe `helper` about `file`, killing it after `timeout` — split
/// from [`probe_runtime_in_helper`] so the "Probe hangs" row can be driven
/// with any stand-in command and a short timeout.
fn run_probe(
    mut helper: std::process::Command,
    file: &Path,
    timeout: Duration,
) -> Result<Vec<SpeechExecutionTarget>, String> {
    use std::io::Read as _;
    use std::process::Stdio;

    let mut child = helper
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not start the runtime probe: {error}"))?;

    let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_string(&mut text);
            }
            text
        })
    };
    let stdout = drain(child.stdout.take().map(|pipe| Box::new(pipe) as _));
    let stderr = drain(child.stderr.take().map(|pipe| Box::new(pipe) as _));

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Probing {} timed out", file.display()));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Could not wait for the runtime probe: {error}"));
            }
        }
    };

    parse_probe_output(
        status.success(),
        &stdout.join().unwrap_or_default(),
        &stderr.join().unwrap_or_default(),
        file,
    )
}

/// The AD-9 resolved backend for `selection`: the target, and the weights
/// the target implies (CPU → Q4, a GPU → FP16).
///
/// A remote selection — and the System voice (Story 3.12) — never reaches
/// the ONNX engine; no ONNX adapter is built for either, so they resolve to
/// the CPU backend only so that a report has a backend to carry; their
/// capability or engine row is what the user sees.
fn resolve_backend(selection: &BackendSelection) -> SpeechBackend {
    match selection.local_target() {
        Some(target) => SpeechBackend::for_target(target),
        None => SpeechBackend::CPU,
    }
}

/// What the Dependency Check is asked about for `state`.
fn check_request(state: &AppState) -> CheckRequest {
    CheckRequest {
        backend: resolve_backend(&state.backend_selection),
        selection: state.backend_selection.clone(),
        has_api_key: match &state.backend_selection {
            BackendSelection::Remote(provider) => state.api_keys.has(*provider),
            BackendSelection::Local { .. } | BackendSelection::SystemVoice => false,
        },
        // Story 3.14: only Azure has a region; only its voice is required.
        has_region: state.azure_region.is_some(),
        has_voice: state
            .speech_voices
            .get(state.backend_selection.language_backend())
            .is_some(),
    }
}

/// The runtime library a selection loads: the added one it names, or the
/// bundled one by the usual rule. `None` for a remote selection, or when
/// there is no cache root to resolve the bundled one against.
fn selection_library(selection: &BackendSelection) -> Option<PathBuf> {
    selection.local_target()?;
    let root = voice_me_core::assets::model_cache_root().ok()?;
    Some(voice_me_core::assets::resolve_runtime_dylib(&root, selection.added_runtime()).path)
}

/// Decision 2: a switch needs a restart exactly when this process already
/// committed a runtime library and the new selection needs a different
/// one. Nothing committed yet, the same library, or no library at all (a
/// remote selection) all apply without one.
fn needs_restart(committed: Option<&Path>, wanted: Option<&Path>) -> bool {
    matches!((committed, wanted), (Some(committed), Some(wanted)) if committed != wanted)
}

/// Wait for the instance that relaunched this one to exit, so the tray and
/// the hotkey grab are free before this one claims them.
fn wait_for_previous_instance() {
    let Some(pid) = std::env::var_os(RESTART_WAIT_ENV) else {
        return;
    };
    // SAFETY: called first thing in `main`, before any thread exists.
    unsafe { std::env::remove_var(RESTART_WAIT_ENV) };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    #[cfg(target_os = "linux")]
    {
        let proc_entry = Path::new("/proc").join(&pid);
        while proc_entry.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (pid, deadline);
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Relaunch voice-me with the same arguments (the "Restart now" button).
/// The caller quits once this succeeds.
fn relaunch() -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|error| format!("Could not find voice-me's own executable: {error}"))?;
    std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env(RESTART_WAIT_ENV, std::process::id().to_string())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not restart voice-me: {error}"))
}

/// The speech engine for the current selection, or why there is none.
struct Engine {
    port: Option<Arc<dyn TtsPort>>,
    unavailable: Option<String>,
    /// Which build of the engine this is. Every rebuild gets a new one, and
    /// the adapter tags its `SpeechSessionBuilt` reports with it.
    generation: u64,
}

/// The source of engine generations: each `build_engine` call takes the
/// next one.
static NEXT_ENGINE_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Build the engine `state` selects. Never falls back: a selection that
/// cannot generate gets no engine and a sentence saying why, rather than a
/// CPU one.
///
/// DeepInfra gets the remote adapter (Story 3.6), which keeps its voice-id
/// cache through `store` and reads the key per request, so a key changed
/// in Settings needs no rebuild. fal.ai arrives with Story 3.7.
fn build_engine(
    state: &AppState,
    runtime_error: Option<&str>,
    events: &mpsc::UnboundedSender<AppEvent>,
    store: &SharedSettingsStore,
) -> Engine {
    let generation = NEXT_ENGINE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let unavailable = |reason: String| Engine {
        port: None,
        unavailable: Some(reason),
        generation,
    };
    if let Some(error) = runtime_error {
        return unavailable(error.to_string());
    }
    if let BackendSelection::Remote(RemoteProvider::DeepInfra) = &state.backend_selection {
        return Engine {
            port: Some(Arc::new(RemoteTtsAdapter::new(
                DeepInfra::new(),
                store.clone(),
            ))),
            unavailable: None,
            generation,
        };
    }
    // Story 3.14: Azure reads its key and region from the store per line,
    // so neither needs a rebuild.
    if let BackendSelection::Remote(RemoteProvider::Azure) = &state.backend_selection {
        return Engine {
            port: Some(Arc::new(AzureTtsAdapter::new(Azure::new(), store.clone()))),
            unavailable: None,
            generation,
        };
    }
    // Story 3.12: the System voice is eSpeak NG as a child process on
    // Linux, and has no engine elsewhere yet.
    if let BackendSelection::SystemVoice = &state.backend_selection {
        #[cfg(target_os = "linux")]
        return Engine {
            port: Some(Arc::new(voice_me_tts_system_linux::SystemVoiceLinux::new())),
            unavailable: None,
            generation,
        };
        #[cfg(not(target_os = "linux"))]
        return unavailable(
            "The System voice on this system arrives in a later voice-me release. Choose \
             another backend under Settings → Backend."
                .to_string(),
        );
    }
    if let BackendSelection::Remote(provider) = &state.backend_selection {
        return unavailable(format!(
            "{} is selected, and remote generation through it arrives in a later voice-me \
             release. Choose a local backend under Settings → Backend.",
            provider.label()
        ));
    }
    match TtsAdapter::from_state(state) {
        Ok(adapter) => Engine {
            port: Some(Arc::new(adapter.with_events(events.clone(), generation))),
            unavailable: None,
            generation,
        },
        Err(error) => {
            eprintln!("could not set up the speech engine: {error}");
            unavailable(error.to_string())
        }
    }
}

/// The remote provider whose disclosure the hotkey press has to ask about
/// first (Story 3.6, Decision 1): DeepInfra, when it is selected, has a key
/// and has not been confirmed. With no key the capability row blocks
/// instead. Only DeepInfra and Azure (Story 3.14) can generate yet, so
/// fal.ai is never asked about — Story 3.7 widens this.
fn disclosure_needed(state: &AppState) -> Option<RemoteProvider> {
    match &state.backend_selection {
        BackendSelection::Remote(
            provider @ (RemoteProvider::DeepInfra | RemoteProvider::Azure),
        ) if state.api_keys.has(*provider) && !state.disclosure_confirmed(*provider) => {
            Some(*provider)
        }
        _ => None,
    }
}

/// Whether a `SpeechSessionBuilt` report may set "Active": only when it
/// came from the engine in the slot now. A replaced engine's late report —
/// an in-flight warm-up finishing after a switch — describes sessions the
/// current engine never built.
fn is_current_engine_report(current_generation: u64, reported_generation: u64) -> bool {
    current_generation == reported_generation
}

/// Whether to warm the engine up now: a Reference Voice Sample exists, this
/// engine has not been warmed yet, and the check that just landed was run
/// for the *current* selection and found it runnable — so a selection that
/// cannot run here never commits its runtime library just by trying, not
/// even on the strength of a late report about the previous selection.
fn should_warm_up(
    runnable: bool,
    for_current_selection: bool,
    has_active_sample: bool,
    already_warmed: bool,
) -> bool {
    runnable && for_current_selection && has_active_sample && !already_warmed
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

/// Fold one provisioning event into the held per-row install state
/// (Story 3.2). Returns whether the Dependency Check should run again —
/// true for every `ProvisioningFinished`, success or failure, since either
/// can have changed the files on disk.
///
/// Progress marks the row installing with its figure; a successful finish
/// drops the row (the re-run check is what says "ready"); a failed one
/// holds the sentence saying why. Any other event is ignored.
fn apply_provisioning_event(
    rows: &mut HashMap<DependencyKind, RowProvisioning>,
    event: &AppEvent,
) -> bool {
    match event {
        AppEvent::ProvisioningProgress {
            kind,
            done_bytes,
            total_bytes,
        } => {
            rows.insert(
                *kind,
                RowProvisioning::Installing {
                    done: *done_bytes,
                    total: *total_bytes,
                },
            );
            false
        }
        AppEvent::ProvisioningFinished { kind, result } => {
            match result {
                Ok(()) => rows.remove(kind),
                Err(reason) => rows.insert(*kind, RowProvisioning::Failed(reason.clone())),
            };
            true
        }
        _ => false,
    }
}

/// Drop the install state of every row `report` no longer calls missing:
/// a ready row has nothing left to install, so nothing held against it —
/// progress or failure — should outlive the report that saw it ready.
fn retain_missing_rows(
    rows: &mut HashMap<DependencyKind, RowProvisioning>,
    report: &DependencyReport,
) {
    rows.retain(|kind, _| {
        report
            .dependencies
            .iter()
            .any(|row| row.kind == *kind && row.status.is_missing())
    });
}

/// The persisted settings, plus this run's resolved backend, the latest
/// Dependency Check and the System voice's latest voice list.
///
/// Re-read per Speak Action rather than snapshotted at startup: a Reference
/// Voice Sample recorded in Settings since launch has to count, and so would
/// a speech language edited in `settings.toml`.
fn current_state(
    settings_store: &Arc<dyn SettingsStore>,
    dependencies: &DependencyOutcome,
    system_voices: &[StockVoice],
    azure_voices: &[StockVoice],
) -> AppState {
    let mut state = settings_store.load().unwrap_or_else(|error| {
        // Falling back to defaults keeps the Speak Action reaching a real
        // failure it can name ("no Reference Voice Sample") instead of the
        // app silently doing nothing.
        eprintln!("could not read settings: {error}");
        AppState::default()
    });
    // AD-9: resolved from the persisted selection on every read, so a
    // switch in Settings reaches the next Speak Action.
    state.speech_backend = resolve_backend(&state.backend_selection);
    // The report is held here, not in the settings file (it describes the
    // filesystem, not a preference), and merged into the state every other
    // reader already receives.
    state.dependencies = dependencies.clone();
    // Story 3.12: held beside the report for the same reason — it is what
    // the engine listed on this machine a moment ago, not a preference.
    state.system_voices = system_voices.to_vec();
    // Story 3.14: Azure's voice list, cached for the session (D1).
    state.azure_voices = azure_voices.to_vec();
    state
}

/// How a failed Azure voice-list fetch starts its message beside the
/// speech language (Story 3.14).
const AZURE_LIST_ERROR: &str = "Couldn't list Azure's voices: ";

/// Remove the speech-language error only if it is a failed Azure voice
/// listing, so a failed language save is never cleared by a fetch.
fn clear_azure_list_error(errors: &mut HashMap<BackendArea, String>) {
    if errors
        .get(&BackendArea::SpeechLanguage)
        .is_some_and(|message| message.starts_with(AZURE_LIST_ERROR))
    {
        errors.remove(&BackendArea::SpeechLanguage);
    }
}

/// What the disclosure for `provider` lists (Story 3.14, D2): for Azure,
/// the saved language and the voice — by name, when the fetched list has
/// it — and for a cloning provider the fixed items.
fn disclosure_text(state: &AppState, provider: RemoteProvider) -> DisclosureText {
    let backend = state.backend_selection.language_backend();
    let language = state.speech_language().unwrap_or_default();
    let voice = state.speech_voices.get(backend).map(|id| {
        match state
            .stock_voices(backend)
            .iter()
            .find(|voice| voice.id == id)
        {
            Some(listed) => format!("{} ({id})", listed.name),
            None => id.to_string(),
        }
    });
    DisclosureText::for_provider(provider, language, voice.as_deref())
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

    fn progress_event(kind: DependencyKind, done: u64, total: u64) -> AppEvent {
        AppEvent::ProvisioningProgress {
            kind,
            done_bytes: done,
            total_bytes: total,
        }
    }

    #[test]
    fn progress_marks_the_row_installing_without_a_recheck() {
        let mut rows = HashMap::new();

        let recheck = apply_provisioning_event(
            &mut rows,
            &progress_event(DependencyKind::ModelWeights, 412, 1_560),
        );

        assert!(!recheck);
        assert_eq!(
            rows.get(&DependencyKind::ModelWeights),
            Some(&RowProvisioning::Installing {
                done: 412,
                total: 1_560
            })
        );
    }

    #[test]
    fn a_successful_finish_drops_the_row_and_asks_for_a_recheck() {
        let mut rows = HashMap::new();
        apply_provisioning_event(
            &mut rows,
            &progress_event(DependencyKind::ModelWeights, 1, 1),
        );

        let recheck = apply_provisioning_event(
            &mut rows,
            &AppEvent::ProvisioningFinished {
                kind: DependencyKind::ModelWeights,
                result: Ok(()),
            },
        );

        assert!(recheck);
        assert!(rows.is_empty());
    }

    #[test]
    fn a_failed_finish_holds_the_reason_and_asks_for_a_recheck() {
        let mut rows = HashMap::new();

        let recheck = apply_provisioning_event(
            &mut rows,
            &AppEvent::ProvisioningFinished {
                kind: DependencyKind::OnnxRuntime,
                result: Err("Download of x failed: reset".to_string()),
            },
        );

        assert!(recheck);
        assert_eq!(
            rows.get(&DependencyKind::OnnxRuntime),
            Some(&RowProvisioning::Failed(
                "Download of x failed: reset".to_string()
            ))
        );
        assert!(!apply_provisioning_event(
            &mut rows,
            &AppEvent::HotkeyPressed
        ));
    }

    #[test]
    fn a_report_drops_ready_rows_and_keeps_failures_on_missing_ones() {
        let mut rows = HashMap::from([
            (
                DependencyKind::ModelWeights,
                RowProvisioning::Installing { done: 5, total: 5 },
            ),
            (
                DependencyKind::OnnxRuntime,
                RowProvisioning::Failed("Extraction failed".to_string()),
            ),
        ]);
        let report = DependencyReport::new(
            SpeechBackend::CPU,
            vec![
                voice_me_core::Dependency::ready(DependencyKind::ModelWeights, "Model", "ok"),
                voice_me_core::Dependency::missing(DependencyKind::OnnxRuntime, "Runtime", "gone"),
            ],
        );

        retain_missing_rows(&mut rows, &report);

        assert_eq!(
            rows,
            HashMap::from([(
                DependencyKind::OnnxRuntime,
                RowProvisioning::Failed("Extraction failed".to_string()),
            )])
        );
    }

    /// A store whose Reference Voice Sample appears only on the second
    /// `load` — i.e. recorded in Settings after launch.
    struct SampleAppearsLater {
        loads: AtomicUsize,
    }

    impl SettingsStore for SampleAppearsLater {
        fn save_backend_selection(
            &self,
            _selection: &voice_me_core::BackendSelection,
        ) -> Result<AppState, VoiceMeError> {
            unimplemented!("not exercised by these tests")
        }

        fn save_local_runtimes(
            &self,
            _runtimes: &[voice_me_core::LocalRuntime],
        ) -> Result<AppState, VoiceMeError> {
            unimplemented!("not exercised by these tests")
        }

        fn save_speech_language(
            &self,
            _backend: voice_me_core::LanguageBackend,
            _code: &str,
        ) -> Result<AppState, VoiceMeError> {
            unimplemented!("not exercised by these tests")
        }

        fn save_speech_voice(
            &self,
            _backend: voice_me_core::LanguageBackend,
            _voice: Option<&str>,
        ) -> Result<AppState, VoiceMeError> {
            unimplemented!("not exercised by these tests")
        }

        fn save_api_key(
            &self,
            _provider: voice_me_core::RemoteProvider,
            _key: Option<&str>,
        ) -> Result<AppState, VoiceMeError> {
            unimplemented!("not exercised by these tests")
        }

        fn save_azure_region(&self, _region: Option<&str>) -> Result<AppState, VoiceMeError> {
            unimplemented!("not exercised by these tests")
        }

        fn save_disclosure_confirmed(
            &self,
            _provider: voice_me_core::RemoteProvider,
        ) -> Result<AppState, VoiceMeError> {
            unimplemented!("not exercised by these tests")
        }

        fn save_remote_sample(
            &self,
            _provider: voice_me_core::RemoteProvider,
            _sample: Option<voice_me_core::RemoteSample>,
        ) -> Result<AppState, VoiceMeError> {
            unimplemented!("not exercised by these tests")
        }

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
            current_state(&store, &DependencyOutcome::Pending, &[], &[])
                .reference_voice_sample
                .is_none(),
            "nothing recorded yet at launch"
        );
        assert!(
            current_state(&store, &DependencyOutcome::Pending, &[], &[])
                .reference_voice_sample
                .is_some(),
            "a sample recorded in Settings since launch has to count — \
             snapshotting state at startup would fail every later Speak Action"
        );
    }

    /// Story 3.12: the held voice list rides along with every read.
    #[test]
    fn the_system_voice_list_is_merged_into_every_read() {
        let store: Arc<dyn SettingsStore> = Arc::new(SampleAppearsLater {
            loads: AtomicUsize::new(0),
        });
        let voices = vec![StockVoice {
            id: "trk/tr".to_string(),
            language: "tr".to_string(),
            language_label: "Turkish".to_string(),
            name: "Turkish".to_string(),
            priority: 5,
        }];

        assert_eq!(
            current_state(&store, &DependencyOutcome::Pending, &voices, &[]).system_voices,
            voices
        );
        // Story 3.14: Azure's cached list is merged the same way.
        let state = current_state(&store, &DependencyOutcome::Pending, &[], &voices);
        assert_eq!(state.azure_voices, voices);
        assert!(state.system_voices.is_empty());
    }

    #[test]
    fn the_resolved_backend_rides_along_with_every_read() {
        let store: Arc<dyn SettingsStore> = Arc::new(SampleAppearsLater {
            loads: AtomicUsize::new(0),
        });

        assert_eq!(
            current_state(&store, &DependencyOutcome::Pending, &[], &[]).speech_backend,
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

    /// Story 3.3's pure decisions: resolution, the check request, restart
    /// detection, and reading the probe helper's answer.
    mod backend_choice {
        use std::path::{Path, PathBuf};

        use voice_me_core::{
            ApiKeys, AppState, BackendSelection, RemoteProvider, SpeechBackend,
            SpeechExecutionTarget, SpeechWeights,
        };

        use super::*;

        fn cuda_on(path: &str) -> BackendSelection {
            BackendSelection::Local {
                runtime: Some(PathBuf::from(path)),
                target: SpeechExecutionTarget::Cuda,
            }
        }

        #[test]
        fn the_selection_resolves_to_its_target_and_the_weights_it_implies() {
            assert_eq!(
                resolve_backend(&BackendSelection::BUNDLED_CPU),
                SpeechBackend::CPU
            );
            let cuda = resolve_backend(&cuda_on("/opt/ort/libonnxruntime.so"));
            assert_eq!(cuda.target, SpeechExecutionTarget::Cuda);
            assert_eq!(cuda.weights, SpeechWeights::Fp16);
        }

        #[test]
        fn the_check_request_carries_the_selection_and_only_whether_a_key_exists() {
            let mut keys = ApiKeys::default();
            keys.set(RemoteProvider::DeepInfra, Some("secret".to_string()));
            let state = AppState {
                backend_selection: BackendSelection::Remote(RemoteProvider::DeepInfra),
                api_keys: keys,
                ..AppState::default()
            };

            let request = check_request(&state);

            assert!(request.has_api_key);
            assert_eq!(
                request.selection,
                BackendSelection::Remote(RemoteProvider::DeepInfra)
            );
            assert!(!format!("{request:?}").contains("secret"));

            let fal = AppState {
                backend_selection: BackendSelection::Remote(RemoteProvider::FalAi),
                ..state
            };
            assert!(
                !check_request(&fal).has_api_key,
                "each provider's key is its own"
            );
        }

        /// Decision 2, as a decision.
        #[test]
        fn only_a_different_library_than_the_committed_one_needs_a_restart() {
            let bundled = Path::new("/cache/runtime/libonnxruntime.so");
            let cuda = Path::new("/opt/ort/libonnxruntime.so");

            assert!(
                !needs_restart(None, Some(cuda)),
                "nothing committed yet: the switch applies to the next Speak Action"
            );
            assert!(
                !needs_restart(Some(bundled), Some(bundled)),
                "same library: no restart"
            );
            assert!(needs_restart(Some(bundled), Some(cuda)));
            assert!(
                !needs_restart(Some(bundled), None),
                "a remote selection loads no library"
            );
        }

        #[test]
        fn a_replaced_engines_late_report_is_ignored() {
            assert!(is_current_engine_report(3, 3));
            assert!(
                !is_current_engine_report(4, 3),
                "a warm-up of the engine before the switch must not set Active"
            );
        }

        #[test]
        fn warm_up_waits_for_a_runnable_check_and_happens_once_per_engine() {
            assert!(should_warm_up(true, true, true, false));
            assert!(
                !should_warm_up(false, true, true, false),
                "a check that found a blocker never triggers a warm-up"
            );
            assert!(
                !should_warm_up(true, false, true, false),
                "a late report about the previous selection never triggers one"
            );
            assert!(
                !should_warm_up(true, true, false, false),
                "no sample, no warm-up"
            );
            assert!(!should_warm_up(true, true, true, true), "already warmed");
        }

        /// A store `build_engine` can hold; it is never touched by
        /// building, so the directories need not exist.
        fn unused_store() -> voice_me_tts_remote::SharedSettingsStore {
            let root = std::env::temp_dir().join("voice-me-build-engine-test-unused");
            Arc::new(FileSettingsStore::with_dirs(
                root.join("config"),
                root.join("data"),
            ))
        }

        /// Story 3.6: DeepInfra gets the remote engine — ready at once,
        /// nothing to warm — rather than the "later release" sentence.
        #[test]
        fn deepinfra_gets_the_remote_engine() {
            let (tx, _rx) = mpsc::unbounded();
            let state = AppState {
                backend_selection: BackendSelection::Remote(RemoteProvider::DeepInfra),
                ..AppState::default()
            };

            let engine = build_engine(&state, None, &tx, &unused_store());

            assert!(engine.unavailable.is_none());
            let port = engine.port.expect("an engine in the slot");
            assert!(port.is_ready(), "a remote engine has no sessions to build");
        }

        /// Story 3.14: Azure gets its own remote engine, not the "later
        /// release" sentence; its key and region are read per line.
        #[test]
        fn azure_gets_the_remote_engine() {
            let (tx, _rx) = mpsc::unbounded();
            let state = AppState {
                backend_selection: BackendSelection::Remote(RemoteProvider::Azure),
                ..AppState::default()
            };

            let engine = build_engine(&state, None, &tx, &unused_store());

            assert!(engine.unavailable.is_none());
            let port = engine.port.expect("an engine in the slot");
            assert!(port.is_ready(), "a remote engine has no sessions to build");
        }

        /// Story 3.14: an Azure selection, keyed and with a voice list.
        fn azure_state() -> AppState {
            let mut keys = ApiKeys::default();
            keys.set(RemoteProvider::Azure, Some("az-secret".to_string()));
            AppState {
                backend_selection: BackendSelection::Remote(RemoteProvider::Azure),
                api_keys: keys,
                azure_region: Some("westeurope".to_string()),
                speech_languages: voice_me_core::SpeechLanguages {
                    azure: "tr-TR".to_string(),
                    ..Default::default()
                },
                speech_voices: voice_me_core::SpeechVoices {
                    azure: Some("tr-TR-EmelNeural".to_string()),
                    ..Default::default()
                },
                azure_voices: vec![StockVoice {
                    id: "tr-TR-EmelNeural".to_string(),
                    language: "tr-TR".to_string(),
                    language_label: "Turkish (Türkiye)".to_string(),
                    name: "Emel (Female)".to_string(),
                    priority: 0,
                }],
                ..AppState::default()
            }
        }

        /// Story 3.14: the check learns whether Azure has a region and a
        /// voice of its own — another backend's voice does not count.
        #[test]
        fn the_check_request_carries_azures_region_and_voice() {
            let request = check_request(&azure_state());
            assert!(request.has_api_key && request.has_region && request.has_voice);
            assert!(!format!("{request:?}").contains("az-secret"));

            let bare = AppState {
                azure_region: None,
                speech_voices: voice_me_core::SpeechVoices {
                    system_voice: Some("tr".to_string()),
                    ..Default::default()
                },
                ..azure_state()
            };
            let request = check_request(&bare);
            assert!(!request.has_region);
            assert!(!request.has_voice, "the System voice's voice is its own");
        }

        /// Story 3.14: Azure's disclosure is asked for until confirmed.
        #[test]
        fn azures_disclosure_is_asked_once() {
            let state = azure_state();
            assert_eq!(disclosure_needed(&state), Some(RemoteProvider::Azure));
            let confirmed = AppState {
                confirmed_disclosures: vec![RemoteProvider::Azure],
                ..state
            };
            assert_eq!(disclosure_needed(&confirmed), None);
        }

        /// Story 3.14 (D2): the disclosure names the saved locale and the
        /// voice — by name when the list has it, else by its id.
        #[test]
        fn azures_disclosure_names_the_locale_and_voice() {
            let text = disclosure_text(&azure_state(), RemoteProvider::Azure);
            assert!(text.items.iter().any(|item| item.contains("tr-TR")));
            assert!(
                text.items
                    .iter()
                    .any(|item| item.contains("Emel (Female) (tr-TR-EmelNeural)")),
                "{:?}",
                text.items
            );

            let unlisted = AppState {
                azure_voices: Vec::new(),
                ..azure_state()
            };
            let text = disclosure_text(&unlisted, RemoteProvider::Azure);
            assert!(
                text.items
                    .iter()
                    .any(|item| item.ends_with(": tr-TR-EmelNeural")),
                "{:?}",
                text.items
            );
        }

        /// Story 3.14: a voice-list fetch clears only its own error, never
        /// a failed language save.
        #[test]
        fn the_azure_list_clears_only_its_own_error() {
            let mut errors = HashMap::new();
            errors.insert(
                BackendArea::SpeechLanguage,
                "Couldn't save the speech language: disk full".to_string(),
            );
            clear_azure_list_error(&mut errors);
            assert!(errors.contains_key(&BackendArea::SpeechLanguage));

            errors.insert(
                BackendArea::SpeechLanguage,
                format!("{AZURE_LIST_ERROR}Azure rejected the API key."),
            );
            clear_azure_list_error(&mut errors);
            assert!(errors.is_empty());
        }

        /// Story 3.12: the System voice is never an ONNX target — it
        /// resolves to the CPU placeholder, needs no key, no library and no
        /// restart — and on Linux gets the eSpeak NG engine.
        #[test]
        fn the_system_voice_gets_its_own_engine_and_no_onnx_runtime() {
            let selection = BackendSelection::SystemVoice;
            assert_eq!(resolve_backend(&selection), SpeechBackend::CPU);
            assert_eq!(selection_library(&selection), None);
            let state = AppState {
                backend_selection: selection.clone(),
                ..AppState::default()
            };
            let request = check_request(&state);
            assert_eq!(request.selection, selection);
            assert!(!request.has_api_key);
            assert_eq!(disclosure_needed(&state), None);

            let (tx, _rx) = mpsc::unbounded();
            let engine = build_engine(&state, None, &tx, &unused_store());
            #[cfg(target_os = "linux")]
            {
                assert!(engine.unavailable.is_none());
                assert!(engine.port.expect("an engine in the slot").is_ready());
            }
            #[cfg(not(target_os = "linux"))]
            {
                assert!(engine.port.is_none());
                assert!(
                    engine
                        .unavailable
                        .unwrap()
                        .contains("later voice-me release")
                );
            }
        }

        /// No silent fallback: fal.ai (Story 3.7) gets no engine at all —
        /// never a CPU one — and a sentence naming the provider.
        #[test]
        fn fal_ai_gets_no_engine_yet() {
            let (tx, _rx) = mpsc::unbounded();
            let state = AppState {
                backend_selection: BackendSelection::Remote(RemoteProvider::FalAi),
                ..AppState::default()
            };

            let engine = build_engine(&state, None, &tx, &unused_store());

            assert!(engine.port.is_none());
            let reason = engine.unavailable.expect("the user is told why");
            assert!(reason.contains("fal.ai"), "{reason}");
            assert!(reason.contains("later voice-me release"), "{reason}");
        }

        #[test]
        fn each_engine_build_gets_a_new_generation() {
            let (tx, _rx) = mpsc::unbounded();
            let state = AppState {
                backend_selection: BackendSelection::Remote(RemoteProvider::FalAi),
                ..AppState::default()
            };

            let first = build_engine(&state, None, &tx, &unused_store()).generation;
            let second = build_engine(&state, None, &tx, &unused_store()).generation;

            assert_ne!(first, second);
        }

        /// Decision 1: the confirm-first overlay is asked for exactly when
        /// a remote provider with a key has not been confirmed.
        #[test]
        fn the_disclosure_is_asked_once_per_provider_and_only_with_a_key() {
            let mut keys = ApiKeys::default();
            keys.set(RemoteProvider::DeepInfra, Some("secret".to_string()));
            let state = AppState {
                backend_selection: BackendSelection::Remote(RemoteProvider::DeepInfra),
                api_keys: keys,
                ..AppState::default()
            };
            assert_eq!(disclosure_needed(&state), Some(RemoteProvider::DeepInfra));

            let confirmed = AppState {
                confirmed_disclosures: vec![RemoteProvider::DeepInfra],
                ..state.clone()
            };
            assert_eq!(disclosure_needed(&confirmed), None);

            let no_key = AppState {
                api_keys: ApiKeys::default(),
                ..state.clone()
            };
            assert_eq!(
                disclosure_needed(&no_key),
                None,
                "the capability row blocks instead"
            );

            assert_eq!(
                disclosure_needed(&AppState::default()),
                None,
                "local: nothing to ask"
            );

            let mut fal_keys = ApiKeys::default();
            fal_keys.set(RemoteProvider::FalAi, Some("secret".to_string()));
            let fal_ai = AppState {
                backend_selection: BackendSelection::Remote(RemoteProvider::FalAi),
                api_keys: fal_keys,
                ..AppState::default()
            };
            assert_eq!(
                disclosure_needed(&fal_ai),
                None,
                "fal.ai cannot generate yet, so nothing is confirmed for it"
            );
        }

        #[test]
        fn the_probe_answer_round_trips() {
            let targets = vec![SpeechExecutionTarget::Cpu, SpeechExecutionTarget::Cuda];
            let file = Path::new("/opt/ort/libonnxruntime.so");

            let parsed = parse_probe_output(
                true,
                &format!("some ort chatter\n{}\n", probe_output_line(&targets)),
                "",
                file,
            );

            assert_eq!(parsed, Ok(targets));
        }

        /// "Add runtime: bad file": nothing is added, and the helper's own
        /// reason is what the user reads.
        #[test]
        fn a_file_the_helper_refused_is_reported_with_its_reason() {
            let error = parse_probe_output(
                false,
                "",
                "could not load /tmp/notes.txt: invalid ELF header\n",
                Path::new("/tmp/notes.txt"),
            )
            .unwrap_err();

            assert!(error.contains("/tmp/notes.txt"), "{error}");
            assert!(error.contains("invalid ELF header"), "{error}");
        }

        #[test]
        fn a_helper_that_printed_nothing_useful_is_refused() {
            assert!(parse_probe_output(true, "", "", Path::new("/x.so")).is_err());
            assert!(
                parse_probe_output(true, "voice-me-probe-targets:", "", Path::new("/x.so"))
                    .is_err()
            );
        }

        /// "Probe hangs": a helper that never answers is killed at the
        /// deadline, nothing is added, and the error says so.
        #[cfg(unix)]
        #[test]
        fn a_helper_that_hangs_is_killed_and_reported_as_timed_out() {
            let mut hang = std::process::Command::new("sleep");
            hang.arg("30");
            let started = std::time::Instant::now();

            let error = run_probe(
                hang,
                Path::new("/opt/ort/libonnxruntime.so"),
                Duration::from_millis(200),
            )
            .unwrap_err();

            assert_eq!(error, "Probing /opt/ort/libonnxruntime.so timed out");
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the helper was killed rather than waited out"
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
    // Decision 1's helper mode: probe one runtime library and exit, before
    // anything else in this process exists.
    let mut args = std::env::args_os().skip(1);
    if args.next().is_some_and(|arg| arg == PROBE_RUNTIME_FLAG) {
        std::process::exit(run_runtime_probe(args.next()));
    }
    // "Restart now": let the previous instance release the tray and the
    // hotkey before this one claims them.
    wait_for_previous_instance();

    let file_store =
        Arc::new(FileSettingsStore::new().expect("failed to resolve settings/data directories"));
    // The same store, twice typed: every main-thread reader takes the plain
    // port; the remote engine and the sample delete run on other threads
    // and need it `Send + Sync` (Story 3.6).
    let settings_store: Arc<dyn SettingsStore> = file_store.clone();
    let remote_store: SharedSettingsStore = file_store;

    // Story 1.5: determine whether an active Reference Voice Sample already
    // exists before deciding whether to auto-open the window at startup
    // (Story 2.2 — first run only; otherwise the app stays tray-only). A
    // load failure is treated the same as "no sample" — falling back to the
    // empty-state copy is safe, whereas silently treating an unreadable
    // store as "has a sample" would hide the first-run prompt from someone
    // who actually needs it. Story 3.12: the System voice speaks without a
    // sample, so a user who selected it is not a first run either.
    let startup_state = settings_store.load().ok();
    let has_active_sample = startup_state
        .as_ref()
        .is_some_and(|state| state.reference_voice_sample.is_some());
    let skip_first_run_open = has_active_sample
        || startup_state.is_some_and(|state| state.backend_selection.is_stock_voice());

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
        // is built once here and shared — until the user switches backend
        // (Story 3.3), when the slot is rebuilt for the next Speak Action.
        //
        // A cache directory that cannot be resolved — or a Tokio runtime
        // that would not start — is not a reason to lose the tray, the
        // hotkey and Settings: every other adapter failure in this function
        // degrades that way, and these are no more fatal. The engine is
        // simply unavailable, and the Speak Action reports why, in words,
        // the moment it is pressed. `unavailable` carries that reason all
        // the way to the notification, because "could not be set up" with no
        // cause is exactly the unactionable message the rest of this story
        // works to avoid.
        let engine: Rc<RefCell<Engine>> = Rc::new(RefCell::new(build_engine(
            &current_state(&settings_store, &DependencyOutcome::Pending, &[], &[]),
            runtime_error.as_deref(),
            &event_tx,
            &remote_store,
        )));

        // Stories 3.1/3.2: detection, and one-click provisioning. One
        // adapter for the whole process — it remembers which rows are
        // installing, so a second Install on the same row is refused
        // rather than racing the first on one `.part` file.
        let deps_port: Arc<dyn DependencyProvisioningPort> = Arc::new(DepsAdapter::new());

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
        // Not `Send`: the Windows manager owns a window handle, and must
        // stay on GPUI's main thread, whose message loop delivers
        // `WM_HOTKEY`. `Arc<dyn HotkeyPort>` is never sent across threads.
        #[cfg(target_os = "windows")]
        #[allow(clippy::arc_with_non_send_sync)]
        let hotkey_port: Arc<dyn HotkeyPort> =
            Arc::new(WindowsHotkeyAdapter::new(event_tx.clone()));

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

        // Story 3.2: each row's install state — running with a byte figure,
        // or failed with the sentence saying why. Kept here, beside the
        // report rather than inside it, because the report must go on
        // saying "missing" (and the overlay gate on blocking) until the
        // re-run check has actually seen the files. A Settings window
        // opened mid-download reads it, so it never offers a second
        // Install on a row that is still downloading.
        let provisioning: Rc<RefCell<HashMap<DependencyKind, RowProvisioning>>> =
            Rc::new(RefCell::new(HashMap::new()));

        // Story 3.3: the backend section's own state. "Active" comes only
        // from the adapter's `SpeechSessionBuilt`; the rest is what the
        // last backend action left behind.
        let active_backend: Rc<RefCell<ActiveBackend>> =
            Rc::new(RefCell::new(ActiveBackend::NotStarted));
        let restart_pending = Rc::new(Cell::new(false));
        let probing: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
        let backend_errors: Rc<RefCell<HashMap<BackendArea, String>>> =
            Rc::new(RefCell::new(HashMap::new()));
        // Warm-up waits for the first check that finds the selected backend
        // runnable, so a selection that cannot run here never commits its
        // runtime library by trying.
        let warmed_up = Rc::new(Cell::new(false));
        // Story 3.6: providers whose held sample is being deleted now, so a
        // second click does not send a second DELETE.
        let deleting_samples: Rc<RefCell<Vec<RemoteProvider>>> = Rc::new(RefCell::new(Vec::new()));
        // Story 3.12: the voices eSpeak NG listed at the last check run with
        // the System voice selected. Held here, never persisted, and merged
        // into `AppState` and the panel like the dependency outcome.
        let system_voices: Rc<RefCell<Vec<StockVoice>>> = Rc::new(RefCell::new(Vec::new()));
        // Story 3.14 (D1): Azure's voice list, fetched at every check run
        // with Azure selected and a key and region saved. Cached for the
        // session, never persisted; saving a new key or region drops it.
        // The generation is bumped on every drop, so a fetch that was
        // already in flight for the old key or region is discarded.
        let azure_voices: Rc<RefCell<Vec<StockVoice>>> = Rc::new(RefCell::new(Vec::new()));
        let azure_generation: Rc<Cell<u64>> = Rc::new(Cell::new(0));

        // What the backend section shows, from the settings file and the
        // state above.
        let make_panel: Rc<dyn Fn() -> BackendPanel> = Rc::new({
            let settings_store = settings_store.clone();
            let active_backend = active_backend.clone();
            let restart_pending = restart_pending.clone();
            let probing = probing.clone();
            let backend_errors = backend_errors.clone();
            let deleting_samples = deleting_samples.clone();
            let system_voices = system_voices.clone();
            let azure_voices = azure_voices.clone();
            move || {
                let state = settings_store.load().unwrap_or_default();
                BackendPanel {
                    check_request: check_request(&state),
                    selection: state.backend_selection,
                    runtimes: state.local_runtimes,
                    api_keys: state.api_keys,
                    active: active_backend.borrow().clone(),
                    restart_pending: restart_pending.get(),
                    probing: probing.borrow().clone(),
                    errors: backend_errors.borrow().clone(),
                    remote_samples: state.remote_samples,
                    deleting_samples: deleting_samples.borrow().clone(),
                    speech_languages: state.speech_languages,
                    speech_voices: state.speech_voices,
                    system_voices: system_voices.borrow().clone(),
                    azure_region: state.azure_region,
                    azure_voices: azure_voices.borrow().clone(),
                }
            }
        });

        // Push that into an open Settings window. Deferred, because the
        // action that caused it usually arrives from inside that very view.
        let push_panel: Rc<dyn Fn(&mut App)> = Rc::new({
            let make_panel = make_panel.clone();
            let settings_view_slot = settings_view_slot.clone();
            move |cx: &mut App| {
                let make_panel = make_panel.clone();
                let settings_view_slot = settings_view_slot.clone();
                cx.defer(move |cx| {
                    if let Some(view) = settings_view_slot.borrow().clone() {
                        let panel = make_panel();
                        view.update(cx, |view, cx| view.set_backend_panel(panel, cx));
                    }
                });
            }
        });

        // Story 3.12: with the System voice selected, every Dependency Check
        // also re-reads eSpeak NG's voice list, in the background, and
        // pushes it into an open Backend tab. A list that cannot be read is
        // an empty one, and the tab says why beside the speech language:
        // the engine row can be Ready while the listing itself failed.
        let refresh_system_voices: Rc<dyn Fn(&mut App)> = Rc::new({
            let settings_store = settings_store.clone();
            let system_voices = system_voices.clone();
            let backend_errors = backend_errors.clone();
            let push_panel = push_panel.clone();
            move |cx: &mut App| {
                let selected = settings_store
                    .load()
                    .is_ok_and(|state| state.backend_selection.is_system_voice());
                if !selected {
                    return;
                }
                #[cfg(target_os = "linux")]
                {
                    let list = cx
                        .background_spawn(async move { voice_me_tts_system_linux::list_voices() });
                    let system_voices = system_voices.clone();
                    let backend_errors = backend_errors.clone();
                    let push_panel = push_panel.clone();
                    cx.spawn(async move |cx| {
                        let voices = match list.await {
                            Ok(voices) => {
                                backend_errors
                                    .borrow_mut()
                                    .remove(&BackendArea::SpeechLanguage);
                                voices
                            }
                            Err(error) => {
                                eprintln!("could not list the System voice's voices: {error}");
                                backend_errors.borrow_mut().insert(
                                    BackendArea::SpeechLanguage,
                                    format!("Couldn't list the System voice's voices: {error}"),
                                );
                                Vec::new()
                            }
                        };
                        *system_voices.borrow_mut() = voices;
                        cx.update(|cx| (*push_panel)(cx));
                    })
                    .detach();
                }
                #[cfg(not(target_os = "linux"))]
                let _ = (&system_voices, &backend_errors, &push_panel, cx);
            }
        });

        // Story 3.14 (D1): with Azure selected and a key and region saved,
        // every Dependency Check also fetches Azure's voice list in the
        // background — the key only, no text, the one Azure call allowed
        // before the disclosure — and pushes it into an open Backend tab. A
        // failed fetch leaves the list empty and says why beside the speech
        // language. A list already held is for the saved key and region
        // (saving either drops it), so it is kept and not fetched again.
        // The fetch only ever clears the error it set itself.
        let refresh_azure_voices: Rc<dyn Fn(&mut App)> = Rc::new({
            let settings_store = settings_store.clone();
            let azure_voices = azure_voices.clone();
            let azure_generation = azure_generation.clone();
            let backend_errors = backend_errors.clone();
            let push_panel = push_panel.clone();
            move |cx: &mut App| {
                let Ok(state) = settings_store.load() else {
                    return;
                };
                if state.backend_selection != BackendSelection::Remote(RemoteProvider::Azure) {
                    return;
                }
                let credentials = (
                    state
                        .api_keys
                        .get(RemoteProvider::Azure)
                        .map(str::to_string),
                    state.azure_region.clone(),
                );
                let (Some(key), Some(region)) = credentials else {
                    clear_azure_list_error(&mut backend_errors.borrow_mut());
                    return;
                };
                if !azure_voices.borrow().is_empty() {
                    return;
                }
                let generation = azure_generation.get();
                let list = cx.background_spawn(async move {
                    voice_me_tts_remote::list_azure_voices(&key, &region)
                });
                let azure_voices = azure_voices.clone();
                let azure_generation = azure_generation.clone();
                let backend_errors = backend_errors.clone();
                let push_panel = push_panel.clone();
                cx.spawn(async move |cx| {
                    let result = list.await;
                    // A key or region saved since: this list is not for it.
                    if azure_generation.get() != generation {
                        return;
                    }
                    let voices = match result {
                        Ok(voices) => {
                            clear_azure_list_error(&mut backend_errors.borrow_mut());
                            voices
                        }
                        Err(error) => {
                            eprintln!("could not list Azure's voices: {error}");
                            backend_errors.borrow_mut().insert(
                                BackendArea::SpeechLanguage,
                                format!("{AZURE_LIST_ERROR}{error}"),
                            );
                            Vec::new()
                        }
                    };
                    *azure_voices.borrow_mut() = voices;
                    cx.update(|cx| (*push_panel)(cx));
                })
                .detach();
            }
        });

        // Saving a new Azure key or region drops the cached list, and any
        // fetch still in flight for the old one.
        let drop_azure_voices: Rc<dyn Fn()> = Rc::new({
            let azure_voices = azure_voices.clone();
            let azure_generation = azure_generation.clone();
            move || {
                azure_voices.borrow_mut().clear();
                azure_generation.set(azure_generation.get() + 1);
            }
        });

        // Re-run the Dependency Check for whatever is selected now, in the
        // background. A check that ran reports by event; only one that
        // could not run lands here.
        let run_check: Rc<dyn Fn(&mut App)> = Rc::new({
            let settings_store = settings_store.clone();
            let deps_port = deps_port.clone();
            let event_tx = event_tx.clone();
            let dependency_outcome = dependency_outcome.clone();
            let settings_view_slot = settings_view_slot.clone();
            let provisioning = provisioning.clone();
            let refresh_system_voices = refresh_system_voices.clone();
            let refresh_azure_voices = refresh_azure_voices.clone();
            move |cx: &mut App| {
                (*refresh_system_voices)(cx);
                (*refresh_azure_voices)(cx);
                let request = check_request(&current_state(
                    &settings_store,
                    &dependency_outcome.borrow(),
                    &[],
                    &[],
                ));
                let events = event_tx.clone();
                let deps_port = deps_port.clone();
                let check = cx.background_spawn(async move { deps_port.check(request, events) });
                let dependency_outcome = dependency_outcome.clone();
                let settings_view_slot = settings_view_slot.clone();
                let provisioning = provisioning.clone();
                cx.spawn(async move |cx| {
                    let Err(error) = check.await else { return };
                    eprintln!("the dependency check could not run: {error}");
                    let outcome = DependencyOutcome::Failed(error.to_string());
                    *dependency_outcome.borrow_mut() = outcome.clone();
                    cx.update(|cx| {
                        if let Some(view) = settings_view_slot.borrow().clone() {
                            // No report is coming to replace the view's
                            // map, so a row left "installing" after an `Ok`
                            // finish is resynced from the held one.
                            let rows = provisioning.borrow().clone();
                            view.update(cx, |view, cx| {
                                view.set_dependency_outcome(outcome, cx);
                                view.replace_provisioning(rows, cx);
                            });
                        }
                    });
                })
                .detach();
            }
        });

        // Make `selection` the selected backend (Decisions 2 and 3): saved
        // at once; the engine is rebuilt for the next Speak Action unless
        // the selection needs another runtime library than the one this
        // process committed, in which case the tab says a restart is due.
        let apply_selection: Rc<dyn Fn(BackendSelection, BackendArea, &mut App)> = Rc::new({
            let settings_store = settings_store.clone();
            let engine = engine.clone();
            let event_tx = event_tx.clone();
            let dependency_outcome = dependency_outcome.clone();
            let active_backend = active_backend.clone();
            let restart_pending = restart_pending.clone();
            let backend_errors = backend_errors.clone();
            let run_check = run_check.clone();
            let push_panel = push_panel.clone();
            let runtime_error = runtime_error.clone();
            let warmed_up = warmed_up.clone();
            let remote_store = remote_store.clone();
            move |selection: BackendSelection, area: BackendArea, cx: &mut App| {
                match settings_store.save_backend_selection(&selection) {
                    Err(error) => {
                        backend_errors
                            .borrow_mut()
                            .insert(area, format!("Couldn't save the backend choice: {error}"));
                    }
                    Ok(_) => {
                        let mut errors = backend_errors.borrow_mut();
                        errors.remove(&BackendArea::Selection);
                        errors.remove(&BackendArea::Capability);
                        errors.remove(&BackendArea::SpeechLanguage);
                        errors.remove(&BackendArea::SpeechVoice);
                        drop(errors);
                        let committed = voice_me_tts::sessions::committed_runtime();
                        let wanted = selection_library(&selection);
                        if needs_restart(committed.as_deref(), wanted.as_deref()) {
                            restart_pending.set(true);
                        } else {
                            restart_pending.set(false);
                            let state = current_state(
                                &settings_store,
                                &dependency_outcome.borrow(),
                                &[],
                                &[],
                            );
                            *engine.borrow_mut() = build_engine(
                                &state,
                                runtime_error.as_deref(),
                                &event_tx,
                                &remote_store,
                            );
                            // A fresh adapter has built nothing yet, and
                            // is warmed up by the next runnable check.
                            *active_backend.borrow_mut() = ActiveBackend::NotStarted;
                            warmed_up.set(false);
                        }
                    }
                }
                (*run_check)(cx);
                (*push_panel)(cx);
            }
        });

        // Everything the backend section asks for arrives here; the view
        // itself never touches `SettingsStore`.
        let backend_actions: BackendActions = Rc::new({
            let settings_store = settings_store.clone();
            let probing = probing.clone();
            let backend_errors = backend_errors.clone();
            let run_check = run_check.clone();
            let push_panel = push_panel.clone();
            let apply_selection = apply_selection.clone();
            let remote_store = remote_store.clone();
            let deleting_samples = deleting_samples.clone();
            let drop_azure_voices = drop_azure_voices.clone();
            move |action: BackendAction, cx: &mut App| match action {
                BackendAction::Select(selection) => {
                    (*apply_selection)(selection, BackendArea::Selection, cx)
                }
                BackendAction::UseCpu => {
                    (*apply_selection)(BackendSelection::BUNDLED_CPU, BackendArea::Capability, cx)
                }
                BackendAction::AddRuntime(path) => {
                    if probing.borrow().is_some() {
                        return;
                    }
                    *probing.borrow_mut() = Some(path.clone());
                    backend_errors.borrow_mut().remove(&BackendArea::Runtimes);
                    (*push_panel)(cx);

                    let probe = {
                        let path = path.clone();
                        cx.background_spawn(async move { probe_runtime_in_helper(&path) })
                    };
                    let settings_store = settings_store.clone();
                    let probing = probing.clone();
                    let backend_errors = backend_errors.clone();
                    let push_panel = push_panel.clone();
                    cx.spawn(async move |cx| {
                        let result = probe.await.and_then(|targets| {
                            let mut runtimes = settings_store
                                .load()
                                .map_err(|error| error.to_string())?
                                .local_runtimes;
                            runtimes.retain(|runtime| runtime.path != path);
                            runtimes.push(LocalRuntime {
                                path: path.clone(),
                                targets,
                            });
                            settings_store
                                .save_local_runtimes(&runtimes)
                                .map(|_| ())
                                .map_err(|error| format!("Couldn't save the runtime: {error}"))
                        });
                        *probing.borrow_mut() = None;
                        if let Err(reason) = result {
                            eprintln!("adding a runtime failed: {reason}");
                            backend_errors
                                .borrow_mut()
                                .insert(BackendArea::Runtimes, reason);
                        }
                        cx.update(|cx| (*push_panel)(cx));
                    })
                    .detach();
                }
                BackendAction::RemoveRuntime(path) => {
                    let result = settings_store.load().and_then(|state| {
                        let runtimes: Vec<_> = state
                            .local_runtimes
                            .into_iter()
                            .filter(|runtime| runtime.path != path)
                            .collect();
                        settings_store.save_local_runtimes(&runtimes)
                    });
                    match result {
                        Ok(state) => {
                            backend_errors.borrow_mut().remove(&BackendArea::Runtimes);
                            // A selection backed by the removed library has
                            // nothing left to load; fall back to the
                            // default, visibly.
                            if state.backend_selection.added_runtime() == Some(path.as_path()) {
                                (*apply_selection)(
                                    BackendSelection::BUNDLED_CPU,
                                    BackendArea::Runtimes,
                                    cx,
                                );
                                return;
                            }
                        }
                        Err(error) => {
                            backend_errors.borrow_mut().insert(
                                BackendArea::Runtimes,
                                format!("Couldn't remove the runtime: {error}"),
                            );
                        }
                    }
                    (*push_panel)(cx);
                }
                BackendAction::SaveApiKey(provider, key) => {
                    match settings_store.save_api_key(provider, key.as_deref()) {
                        Ok(_) => {
                            backend_errors
                                .borrow_mut()
                                .remove(&BackendArea::ApiKey(provider));
                            if provider == RemoteProvider::Azure {
                                (*drop_azure_voices)();
                            }
                        }
                        Err(error) => {
                            backend_errors.borrow_mut().insert(
                                BackendArea::ApiKey(provider),
                                format!("Couldn't save the key: {error}"),
                            );
                        }
                    }
                    (*run_check)(cx);
                    (*push_panel)(cx);
                }
                // Story 3.14: the same shape for Azure's region, which also
                // drops the cached voice list; the check re-fetches it.
                BackendAction::SaveAzureRegion(region) => {
                    match settings_store.save_azure_region(region.as_deref()) {
                        Ok(_) => {
                            backend_errors
                                .borrow_mut()
                                .remove(&BackendArea::AzureRegion);
                            (*drop_azure_voices)();
                        }
                        Err(error) => {
                            backend_errors.borrow_mut().insert(
                                BackendArea::AzureRegion,
                                format!("Couldn't save the region: {error}"),
                            );
                        }
                    }
                    (*run_check)(cx);
                    (*push_panel)(cx);
                }
                BackendAction::DeleteRemoteSample(provider) => {
                    if deleting_samples.borrow().contains(&provider) {
                        return;
                    }
                    deleting_samples.borrow_mut().push(provider);
                    backend_errors
                        .borrow_mut()
                        .remove(&BackendArea::RemoteSample(provider));
                    (*push_panel)(cx);

                    // Off the main thread — a DELETE may take up to its 30 s
                    // deadline — and on GPUI's background executor rather
                    // than the Tokio bridge, so it works even on a run whose
                    // runtime would not start.
                    let delete = {
                        let store = remote_store.clone();
                        cx.background_spawn(async move {
                            voice_me_tts_remote::delete_held_sample(provider, store.as_ref())
                        })
                    };
                    let deleting_samples = deleting_samples.clone();
                    let backend_errors = backend_errors.clone();
                    let push_panel = push_panel.clone();
                    cx.spawn(async move |cx| {
                        let result = delete.await;
                        deleting_samples
                            .borrow_mut()
                            .retain(|deleting| *deleting != provider);
                        if let Err(error) = result {
                            eprintln!("deleting the voice sample failed: {error}");
                            backend_errors.borrow_mut().insert(
                                BackendArea::RemoteSample(provider),
                                format!("Couldn't delete the sample: {error}"),
                            );
                        }
                        cx.update(|cx| (*push_panel)(cx));
                    })
                    .detach();
                }
                // Story 3.11: saved and pushed back. The language is read
                // from settings on every Speak Action, so there is no engine
                // to rebuild. Story 3.14: the check is re-run, because a new
                // Azure locale clears its voice, which the check requires.
                BackendAction::SetSpeechLanguage(backend, code) => {
                    match settings_store.save_speech_language(backend, &code) {
                        Ok(_) => {
                            let mut errors = backend_errors.borrow_mut();
                            errors.remove(&BackendArea::SpeechLanguage);
                            // A new System voice language clears the voice,
                            // and with it any failed save of one.
                            errors.remove(&BackendArea::SpeechVoice);
                        }
                        Err(error) => {
                            backend_errors.borrow_mut().insert(
                                BackendArea::SpeechLanguage,
                                format!("Couldn't save the speech language: {error}"),
                            );
                        }
                    }
                    (*run_check)(cx);
                    (*push_panel)(cx);
                }
                // Story 3.12: the same, for the System voice's voice.
                BackendAction::SetSpeechVoice(backend, voice) => {
                    match settings_store.save_speech_voice(backend, voice.as_deref()) {
                        Ok(_) => {
                            backend_errors
                                .borrow_mut()
                                .remove(&BackendArea::SpeechVoice);
                        }
                        Err(error) => {
                            backend_errors.borrow_mut().insert(
                                BackendArea::SpeechVoice,
                                format!("Couldn't save the voice: {error}"),
                            );
                        }
                    }
                    (*run_check)(cx);
                    (*push_panel)(cx);
                }
                BackendAction::Restart => match relaunch() {
                    Ok(()) => cx.quit(),
                    Err(reason) => {
                        eprintln!("{reason}");
                        backend_errors
                            .borrow_mut()
                            .insert(BackendArea::Restart, reason);
                        (*push_panel)(cx);
                    }
                },
            }
        });

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
            let provisioning = provisioning.clone();
            let make_panel = make_panel.clone();
            let backend_actions = backend_actions.clone();
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
                    backend: make_panel(),
                    actions: backend_actions.clone(),
                    outcome: dependency_outcome.borrow().clone(),
                    provisioning: provisioning.borrow().clone(),
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
            let azure_voices = azure_voices.clone();
            let event_tx = event_tx.clone();
            let dependency_outcome = dependency_outcome.clone();
            let settings_store = settings_store.clone();
            move |cx: &mut App| {
                // Story 3.4: the same summon, a different shape. The gate
                // is read here rather than inside the view, so the overlay
                // never has to know `voice-me-deps` exists.
                let blocker = overlay_blocker(&dependency_outcome.borrow());
                // Story 3.6: an unconfirmed remote provider opens the
                // confirm-first shape. A blocker wins — there is nothing to
                // confirm for a selection that cannot run.
                let state = current_state(
                    &settings_store,
                    &dependency_outcome.borrow(),
                    &[],
                    &azure_voices.borrow(),
                );
                let disclosure = if blocker.is_none() {
                    disclosure_needed(&state)
                } else {
                    None
                };
                // Story 3.14: what the disclosure lists is the provider's own.
                let disclosure_items = disclosure.map(|provider| disclosure_text(&state, provider));
                let on_confirm: Option<ConfirmDisclosure> = disclosure.map(|provider| {
                    let settings_store = settings_store.clone();
                    Rc::new(move |_cx: &mut App| {
                        // Core refuses the line anyway if this did not
                        // land, and says so in its one notification.
                        if let Err(error) = settings_store.save_disclosure_confirmed(provider) {
                            eprintln!(
                                "could not record the {} confirmation: {error}",
                                provider.label()
                            );
                        }
                    }) as ConfirmDisclosure
                });
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
                        size(
                            px(OVERLAY_WIDTH),
                            px(if disclosure.is_some() {
                                OVERLAY_DISCLOSURE_HEIGHT
                            } else {
                                OVERLAY_HEIGHT
                            }),
                        ),
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
                    let view =
                        cx.new(
                            |cx| match (blocker.clone(), disclosure, on_confirm.clone()) {
                                (Some(blocker), _, _) => PromptOverlayView::blocked(
                                    event_tx.clone(),
                                    blocker,
                                    window,
                                    cx,
                                ),
                                (None, Some(provider), Some(on_confirm)) => {
                                    PromptOverlayView::confirm_disclosure(
                                        event_tx.clone(),
                                        provider.label(),
                                        disclosure_items.clone().unwrap_or_else(|| {
                                            DisclosureText::for_provider(provider, "", None)
                                        }),
                                        on_confirm,
                                        window,
                                        cx,
                                    )
                                }
                                _ => PromptOverlayView::new(event_tx.clone(), window, cx),
                            },
                        );
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
        if !skip_first_run_open {
            (*open_settings)(cx);
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
            (*refresh_system_voices)(cx);
            (*refresh_azure_voices)(cx);
            let request = check_request(&current_state(
                &settings_store,
                &DependencyOutcome::Pending,
                &[],
                &[],
            ));
            let events = event_tx.clone();
            let deps_port = deps_port.clone();
            let check = cx.background_spawn(async move { deps_port.check(request, events) });
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
                        let runnable = report.speech_engine_blocker().is_none();
                        // A late report about the previous selection must
                        // not start a warm-up for the current one.
                        let for_current_selection = report.backend
                            == resolve_backend(
                                &current_state(
                                    &settings_store,
                                    &DependencyOutcome::Pending,
                                    &[],
                                    &[],
                                )
                                .backend_selection,
                            );
                        // A row the check now calls ready has nothing left
                        // to install; whatever was held against it goes.
                        retain_missing_rows(&mut provisioning.borrow_mut(), &report);
                        let rows = provisioning.borrow().clone();
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
                                    view.set_dependency_outcome(outcome, cx);
                                    view.replace_provisioning(rows, cx);
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

                            // Decision 3 (spec-2-6): warm-up is eager, but
                            // only for someone who already has a Reference
                            // Voice Sample. Session construction measured
                            // 86–110 s (spec-2-5), so paying it in the
                            // background is what puts it behind the first
                            // Speak Action rather than in front of it. Story
                            // 3.3 adds the other condition: only once a
                            // check finds the selected backend runnable, so
                            // a selection that cannot run here never commits
                            // its runtime library just by trying — which
                            // would turn "Use CPU backend" into a restart.
                            if should_warm_up(
                                runnable,
                                for_current_selection,
                                has_active_sample,
                                warmed_up.get(),
                            ) {
                                let current_engine = engine.borrow().port.clone();
                                if let Some(tts) = current_engine {
                                    warmed_up.set(true);
                                    let warm_up =
                                        tokio_bridge::spawn_blocking(cx, move || tts.warm_up());
                                    cx.spawn(async move |_| {
                                        if let Err(error) = warm_up.await {
                                            // Not notified: the user did not
                                            // ask for this, and the same
                                            // failure is reported properly
                                            // the moment they press the
                                            // hotkey — and "Active" says it.
                                            eprintln!("speech engine warm-up failed: {error}");
                                        }
                                    })
                                    .detach();
                                }
                            }
                        });
                    }
                    event @ (AppEvent::ProvisioningProgress { .. }
                    | AppEvent::ProvisioningFinished { .. }) => {
                        let (kind, finished_ok) = match &event {
                            AppEvent::ProvisioningProgress { kind, .. } => (*kind, false),
                            AppEvent::ProvisioningFinished { kind, result } => {
                                if let Err(reason) = result {
                                    eprintln!("installing {kind:?} failed: {reason}");
                                }
                                (*kind, result.is_ok())
                            }
                            _ => unreachable!("matched above"),
                        };
                        let recheck =
                            apply_provisioning_event(&mut provisioning.borrow_mut(), &event);

                        // A successful finish is dropped from the held map
                        // but deliberately not pushed: the open view keeps
                        // showing "installing" until the re-run check
                        // lands and replaces its map, so the row never
                        // flashes back to "missing" with Install enabled
                        // in between.
                        if !finished_ok {
                            let state = provisioning.borrow().get(&kind).cloned();
                            cx.update(|cx| {
                                if let Some(view) = settings_view_slot.borrow().clone() {
                                    view.update(cx, |view, cx| {
                                        view.set_provisioning(kind, state, cx)
                                    });
                                }
                            });
                        }
                        if !recheck {
                            continue;
                        }

                        // Either way the files on disk changed — a failed
                        // run can still have completed some of them — so
                        // the check runs again, on the same background
                        // path as the startup one. It reports by event;
                        // only a check that could not run lands here.
                        cx.update(|cx| (*run_check)(cx));
                    }
                    AppEvent::SpeechSessionBuilt { generation, result } => {
                        // A replaced engine's late report is not about the
                        // engine in the slot now.
                        if !is_current_engine_report(engine.borrow().generation, generation) {
                            continue;
                        }
                        // The only writer of "Active" (AD-9): what the
                        // engine actually built, or its own error.
                        *active_backend.borrow_mut() = match result {
                            Ok(backend) => ActiveBackend::Acquired(backend),
                            Err(reason) => ActiveBackend::Failed(reason),
                        };
                        cx.update(|cx| (*push_panel)(cx));
                    }
                    AppEvent::SpeakRequested { text } => {
                        let settings_store = settings_store.clone();
                        let notifications = notification_port.clone();
                        let virtual_mic = virtual_mic_port.clone();
                        let current_engine = engine.borrow().port.clone();
                        let Some(tts) = current_engine else {
                            // The engine never came up at startup. Say so
                            // rather than dropping the line silently — this
                            // is the same contract `speak` honours for every
                            // other failure (UX-DR15). Sent from a background
                            // task because a notification is a synchronous
                            // D-Bus round trip and this loop runs on GPUI's
                            // main thread; `cx.background_spawn` rather than
                            // the Tokio bridge, since a failed runtime is one
                            // of the reasons we are in this branch at all.
                            let reason = engine.borrow().unavailable.clone();
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
                            let state = current_state(
                                &settings_store,
                                &dependency_outcome.borrow(),
                                &system_voices.borrow(),
                                &azure_voices.borrow(),
                            );
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
                            let push_panel = push_panel.clone();
                            cx.spawn(async move |cx| {
                                if let Err(error) = work.await {
                                    eprintln!("speak failed: {error}");
                                }
                                // A remote line can have uploaded (or
                                // dropped) the held sample; an open
                                // Settings window shows it (Story 3.6).
                                cx.update(|cx| (*push_panel)(cx));
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
