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
//! Virtual Microphone instead of dropping it. On Linux the device itself is
//! ensured once at startup, in the background, so a fresh machine needs no
//! manual setup step (spec-2-9 Decision 1). On Windows (Story 2.8) the
//! Virtual Microphone is VB-CABLE, which needs the user's administrator
//! consent to install, so it is installed only from its Dependencies row.
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
    App, AppContext as _, Bounds, Pixels, QuitMode, Size, WindowBackgroundAppearance, WindowBounds,
    WindowDecorations, WindowHandle, WindowKind, WindowOptions, point, px, size,
};
use voice_me_core::VoiceMeError;
use voice_me_core::{
    ActiveBackend, AppEvent, AppState, BackendSelection, CheckRequest, DependencyKind,
    DependencyOutcome, DependencyProvisioningPort, DependencyReport, FileSettingsStore, HotkeyPort,
    LanguageBackend, LocalRuntime, NotificationPort, PiperCatalogPort, RemoteProvider,
    SettingsStore, SpeechBackend, SpeechExecutionTarget, SpeechPhase, StockVoice, TrayVisualState,
    TtsPort, VirtualMicPort, assets, stock_voices_of, tokio_bridge,
};
use voice_me_deps::DepsAdapter;
use voice_me_tts::TtsAdapter;
use voice_me_tts_remote::{
    Azure, AzureTtsAdapter, DeepInfra, RemoteTtsAdapter, SharedSettingsStore,
};
use voice_me_ui::{
    BackendAction, BackendActions, BackendArea, BackendPanel, ConfirmDisclosure, DependenciesTab,
    DisclosureText, HotkeyDesktop, PROMPT_BAR_HEIGHT, PiperCatalogState, PiperVoicesAction,
    PiperVoicesActions, PiperVoicesPanel, PiperVoicesTab, PromptOverlayView, RowProvisioning,
    SettingsView, VoiceDownload, blocker_notice,
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

gpui_kit::assets::icon_assets!(PromptBarIcons, [AudioLines]);

/// gpui-kit's component icons plus the one icon voice-me adds (the prompt
/// bar's voice icon), rather than the whole Lucide catalog for one SVG.
struct AppAssets;

impl gpui_kit::AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        match PromptBarIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<gpui_kit::SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(PromptBarIcons.list(path)?);
        Ok(paths)
    }
}

/// The Settings window's initial size, centred; still resizable down to
/// `SettingsView::window_options`' minimum.
const SETTINGS_WIDTH: f32 = 900.;
const SETTINGS_HEIGHT: f32 = 640.;

/// The overlay's fixed comfortable width. Ready to type, the overlay is
/// the prompt bar and its window is the bar's own height
/// ([`PROMPT_BAR_HEIGHT`]); blocked, it is a card sized to its three lines
/// plus the card's padding (spec-2-4 Design Notes).
const OVERLAY_WIDTH: f32 = 560.;
const OVERLAY_BLOCKED_HEIGHT: f32 = 84.;

/// The confirm-first overlay's height (Story 3.6): the provider, the three
/// things sent, and the two buttons. It keeps this height after confirming.
const OVERLAY_DISCLOSURE_HEIGHT: f32 = 196.;

/// Where the overlay window sits in a display's visible area (taskbar and
/// panels excluded): horizontally centred, and `overlay_position` percent of
/// the way down the free vertical space (spec-overlay-vertical-position).
/// An area narrower (or shorter) than the window gets the window at its
/// left (or top) edge, never off-screen before it.
fn overlay_bounds_in(
    visible: Bounds<Pixels>,
    window_size: Size<Pixels>,
    overlay_position: u8,
) -> Bounds<Pixels> {
    let x = (visible.center().x - window_size.width / 2.).max(visible.origin.x);
    let y = voice_me_core::overlay_origin_y(
        f32::from(visible.origin.y),
        f32::from(visible.size.height),
        f32::from(window_size.height),
        overlay_position,
    );
    Bounds {
        origin: point(x, px(y)),
        size: window_size,
    }
}

/// The overlay's bounds on the primary display, or on the first display
/// GPUI lists when there is no primary one. With no display at all, GPUI's
/// `WindowBounds::centered` has nothing to centre on and puts the window at
/// the origin (0, 0).
fn overlay_window_bounds(
    window_size: Size<Pixels>,
    overlay_position: u8,
    cx: &App,
) -> WindowBounds {
    match cx
        .primary_display()
        .or_else(|| cx.displays().into_iter().next())
    {
        Some(display) => WindowBounds::Windowed(overlay_bounds_in(
            display.visible_bounds(),
            window_size,
            overlay_position,
        )),
        None => WindowBounds::centered(window_size, cx),
    }
}

/// Always-on-top is a two-tier capability, mirroring Story 2.3's two hotkey
/// backends. `WindowKind::PopUp` is a real override-redirect, taskbar-less,
/// above-everything window under X11. On Wayland the overlay first asks for
/// a `zwlr_layer_shell_v1` surface ([`overlay_layer_shell`]): it sits above
/// every window on the overlay layer, is never tiled (Hyprland, Sway and
/// KDE otherwise tile a plain toplevel), and honours the vertical position.
/// GNOME/Mutter has no layer shell; there the open fails with
/// `LayerShellNotSupportedError` and the overlay falls back to an ordinary
/// toplevel that the compositor places itself.
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

/// Set once the compositor has refused a layer-shell overlay (GNOME), so
/// later summons go straight to an ordinary window.
#[cfg(target_os = "linux")]
static LAYER_SHELL_REFUSED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// The overlay as a Wayland layer-shell surface: on the overlay layer,
/// anchored to the top edge only (so the compositor centres it
/// horizontally), `top_margin` below that edge, and holding the keyboard
/// while it is open so the prompt takes typing at once.
#[cfg(target_os = "linux")]
fn overlay_layer_shell(top_margin: f32) -> gpui_kit::layer_shell::LayerShellOptions {
    use gpui_kit::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
    LayerShellOptions {
        namespace: "voice-me-overlay".to_string(),
        layer: Layer::Overlay,
        anchor: Anchor::TOP,
        exclusive_zone: None,
        exclusive_edge: None,
        margin: Some((px(top_margin.max(0.)), px(0.), px(0.), px(0.))),
        keyboard_interactivity: KeyboardInteractivity::Exclusive,
    }
}

/// How far below the top of the display a layer-shell overlay sits: the
/// same percentage placement as [`overlay_bounds_in`], measured from the
/// display's own top edge (a layer surface's margin is relative to it).
fn overlay_top_margin(bounds: &WindowBounds, cx: &App) -> f32 {
    let WindowBounds::Windowed(bounds) = bounds else {
        return 0.;
    };
    let top = cx
        .primary_display()
        .or_else(|| cx.displays().into_iter().next())
        .map(|display| f32::from(display.bounds().origin.y))
        .unwrap_or(0.);
    f32::from(bounds.origin.y) - top
}

/// Whether this is a Wayland session, where the compositor places the
/// overlay itself and its position setting may do nothing.
fn is_wayland_session() -> bool {
    #[cfg(target_os = "linux")]
    {
        voice_me_hotkey_linux::session_kind() == voice_me_hotkey_linux::SessionKind::Wayland
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Switch off DWM's 1px border, drop shadow and corner rounding on the
/// overlay window. The overlay is a transparent, undecorated window whose
/// card draws its own edge, but Windows 11 still frames every top-level
/// window, and that frame showed as a dark rectangle around the card.
/// Failures are ignored: an older Windows without these attributes simply
/// keeps its frame.
#[cfg(target_os = "windows")]
fn remove_dwm_frame(window: &gpui_kit::Window) {
    use raw_window_handle::RawWindowHandle;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DWMNCRP_DISABLED, DWMWA_BORDER_COLOR, DWMWA_NCRENDERING_POLICY,
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DwmSetWindowAttribute,
    };
    // `DWMWA_COLOR_NONE`: no border at all.
    const COLOR_NONE: u32 = 0xFFFF_FFFE;

    let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(win32.hwnd.get() as *mut core::ffi::c_void);
    let corner = DWMWCP_DONOTROUND;
    let policy = DWMNCRP_DISABLED;
    // SAFETY: `hwnd` is this live window's handle, and each pointer is to a
    // local of exactly the size passed with it.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            (&COLOR_NONE as *const u32).cast::<core::ffi::c_void>(),
            size_of::<u32>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as *const core::ffi::c_void,
            size_of_val(&corner) as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_NCRENDERING_POLICY,
            &policy as *const _ as *const core::ffi::c_void,
            size_of_val(&policy) as u32,
        );
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

/// How often a running voice-me checks its custom Piper voices for updates.
const CUSTOM_VOICE_UPDATE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

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
/// the target implies (CPU → Q4, a GPU → FP16). Piper's target is its own
/// device (spec-backend-engine-and-device-selects); it has no weights to
/// pick, so only the target matters there.
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

/// The Piper voice `state` speaks in (Story 3.15): the saved one, or the
/// saved language's top installed voice — or, for the default language
/// with nothing installed, the default voice. `None` for any other
/// selection, or when no voice can be named.
/// Use on the Piper voices tab: the voice's language is saved first,
/// because saving a language clears the voice, then the voice itself.
fn use_piper_voice(
    store: &dyn SettingsStore,
    key: &str,
    locale: &str,
) -> Result<AppState, VoiceMeError> {
    store.save_speech_language(LanguageBackend::Piper, locale)?;
    store.save_speech_voice(LanguageBackend::Piper, Some(key))
}

fn piper_voice_for(state: &AppState) -> Option<String> {
    if !state.backend_selection.is_piper() {
        return None;
    }
    if let Some(voice) = state.speech_voices.get(LanguageBackend::Piper) {
        return Some(voice.to_string());
    }
    let language = state.speech_languages.piper.trim();
    stock_voices_of(&state.piper_voices, language)
        .first()
        .map(|voice| voice.id.clone())
        .or_else(|| {
            language
                .eq_ignore_ascii_case(assets::PIPER_DEFAULT_VOICE.locale)
                .then(|| assets::PIPER_DEFAULT_VOICE.key.to_string())
        })
}

/// The Piper voices installed in the cache, as stock voices. Read from disk
/// on every call: installing or deleting one changes it.
fn installed_piper_voices() -> Vec<StockVoice> {
    assets::model_cache_root()
        .map(|root| assets::installed_piper_stock_voices(&root))
        .unwrap_or_default()
}

/// What the Dependency Check is asked about for `state`.
fn check_request(state: &AppState) -> CheckRequest {
    CheckRequest {
        backend: resolve_backend(&state.backend_selection),
        selection: state.backend_selection.clone(),
        // Story 3.17: a keyless provider (Edge TTS) always "has" its key.
        has_api_key: match &state.backend_selection {
            BackendSelection::Remote(provider) => {
                !provider.needs_api_key() || state.api_keys.has(*provider)
            }
            BackendSelection::Local { .. }
            | BackendSelection::SystemVoice
            | BackendSelection::Piper { .. } => false,
        },
        // Story 3.14: only Azure has a region; only its voice is required.
        has_region: state.azure_region.is_some(),
        has_voice: state
            .speech_voices
            .get(state.backend_selection.language_backend())
            .is_some(),
        piper_voice: piper_voice_for(state),
    }
}

/// The runtime library a selection loads: the added one it names, or the
/// bundled one by the usual rule — which is also Piper's (Story 3.15: it
/// shares the bundled runtime, on any of its devices). `None` for a remote
/// selection and the System voice, or when there is no cache root to
/// resolve the bundled one against.
fn selection_library(selection: &BackendSelection) -> Option<PathBuf> {
    selection.local_target()?;
    let root = voice_me_core::assets::model_cache_root().ok()?;
    Some(voice_me_core::assets::resolve_runtime_dylib(&root, selection.added_runtime()).path)
}

/// Decision 2: a switch needs a restart exactly when this process already
/// committed a runtime library and the new selection needs a different
/// one. Nothing committed yet, the same library, or no library at all (a
/// remote selection) all apply without one.
///
/// Story 3.8: CPU, WebGPU and CUDA all live in the one bundled library, so
/// switching among them never needs a restart — with one exception,
/// `replaced_for_gpu`: Install put voice-me's all-provider build where the
/// CPU-only library this process loaded was, and the selection is a GPU
/// one, which only the new library can run.
fn needs_restart(committed: Option<&Path>, wanted: Option<&Path>, replaced_for_gpu: bool) -> bool {
    matches!(
        (committed, wanted),
        (Some(committed), Some(wanted)) if committed != wanted || replaced_for_gpu
    )
}

/// Story 3.8: whether a runtime install leaves a restart due for
/// `selection`. Either Install replaced the bundled library this process
/// loaded (voice-me-tts's one rule: bundled, GPU, replaced), or it staged
/// the new one for the next start because this process has the old one
/// loaded (Windows). Either way only a GPU selection needs the new library.
fn runtime_restart_due(
    selection: &BackendSelection,
    committed_is_bundled: bool,
    replaced: bool,
    staged: bool,
) -> bool {
    let gpu = is_gpu_selection(selection);
    voice_me_tts::replaced_runtime_needs_restart(committed_is_bundled, gpu, replaced)
        || (gpu && staged)
}

/// Whether `committed` is the bundled runtime in the cache.
fn committed_is_bundled(committed: Option<&Path>) -> bool {
    let Ok(root) = voice_me_core::assets::model_cache_root() else {
        return false;
    };
    committed.is_some_and(|committed| voice_me_core::assets::is_bundled_runtime(&root, committed))
}

/// Whether a runtime update waits for the next start (Story 3.8, Windows).
fn runtime_staged() -> bool {
    voice_me_core::assets::model_cache_root()
        .is_ok_and(|root| voice_me_deps::staged_runtime_pending(&root))
}

/// Story 3.8 (Windows): put a runtime update staged by the last run in
/// place, before anything resolves or loads the runtime.
fn apply_staged_runtime() {
    let Ok(root) = voice_me_core::assets::model_cache_root() else {
        return;
    };
    if let Err(error) = voice_me_deps::apply_staged_runtime(&root) {
        eprintln!("could not finish installing the staged ONNX Runtime: {error}");
    }
}

/// What "Use CPU backend" selects: the saved engine on CPU — Piper stays
/// Piper — or the bundled CPU runtime when the settings cannot be read.
fn use_cpu_selection(store: &dyn SettingsStore) -> BackendSelection {
    store
        .load()
        .map(|state| state.backend_selection.on_cpu())
        .unwrap_or(BackendSelection::BUNDLED_CPU)
}

/// Whether the bundled runtime voice-me installs has every execution
/// provider (Story 3.8), so the Backend tab lists its CUDA and WebGPU
/// devices.
fn bundled_all_providers() -> bool {
    voice_me_deps::sources::Sources::pinned().runtime_all_providers
}

/// Whether `selection` runs on a GPU execution provider — Chatterbox's or,
/// since spec-backend-engine-and-device-selects, Piper's.
fn is_gpu_selection(selection: &BackendSelection) -> bool {
    selection
        .local_target()
        .is_some_and(|target| target != SpeechExecutionTarget::Cpu)
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

/// The process's one Dependency Check and provisioning adapter.
///
/// Story 3.13: on Windows the System voice row asks Windows' speech engine
/// for its voices through the System voice's own crate — `voice-me-deps`
/// never calls a TTS adapter.
fn deps_adapter() -> DepsAdapter {
    // Story 3.8: whether the bundled runtime is loaded here, which Windows
    // cannot replace in place — asked of the speech engine, never by deps.
    let adapter = DepsAdapter::new().with_runtime_in_use(|| {
        committed_is_bundled(voice_me_tts::sessions::committed_runtime().as_deref())
    });
    #[cfg(target_os = "windows")]
    let adapter = adapter.with_system_voice_probe(voice_me_tts_system_windows::count_voices);
    adapter
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
    // Linux; Story 3.13: Windows' own speech engine through WinRT. It has
    // no engine elsewhere yet.
    if let BackendSelection::SystemVoice = &state.backend_selection {
        #[cfg(target_os = "linux")]
        return Engine {
            port: Some(Arc::new(voice_me_tts_system_linux::SystemVoiceLinux::new())),
            unavailable: None,
            generation,
        };
        #[cfg(target_os = "windows")]
        return Engine {
            port: Some(Arc::new(
                voice_me_tts_system_windows::SystemVoiceWindows::new(),
            )),
            unavailable: None,
            generation,
        };
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        return unavailable(
            "The System voice on this system arrives in a later voice-me release. Choose \
             another backend under Settings → Speech."
                .to_string(),
        );
    }
    // Story 3.17: Edge TTS is the `edge-tts` program as a child process on
    // Linux — no reqwest, no key — and has no engine elsewhere (E1).
    if let BackendSelection::Remote(RemoteProvider::EdgeTts) = &state.backend_selection {
        #[cfg(target_os = "linux")]
        return Engine {
            port: Some(Arc::new(voice_me_tts_edge::EdgeTts::new())),
            unavailable: None,
            generation,
        };
        #[cfg(not(target_os = "linux"))]
        return unavailable(
            "Edge TTS isn't available on this OS yet. Choose another backend under Settings → \
             Backend."
                .to_string(),
        );
    }
    // Story 3.15: Piper on the bundled runtime, committed through
    // `voice-me-tts`'s once-per-process guard, on its own device; phonemes
    // from `espeak-ng`.
    if state.backend_selection.is_piper() {
        return match build_piper(state) {
            Ok(port) => Engine {
                port: Some(port),
                unavailable: None,
                generation,
            },
            Err(reason) => unavailable(reason),
        };
    }
    if let BackendSelection::Remote(provider) = &state.backend_selection {
        return unavailable(format!(
            "{} is selected, and remote generation through it arrives in a later voice-me \
             release. Choose a local backend under Settings → Speech.",
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

/// Piper's engine (Story 3.15; Windows since Story 3.16), or why there is
/// none. The phonemizer finds `espeak-ng` on each call, so one installed
/// after the engine was built is used.
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn build_piper(state: &AppState) -> Result<Arc<dyn TtsPort>, String> {
    let root = assets::model_cache_root().map_err(|error| error.to_string())?;
    let runtime_root = root.clone();
    let providers_root = root.clone();
    // spec-backend-engine-and-device-selects: Piper's device, with the same
    // providers, restart rule and NVIDIA preload as Chatterbox's (AD-1:
    // injected, so `voice-me-tts-piper` never depends on `voice-me-tts`).
    let backend = resolve_backend(&state.backend_selection);
    Ok(Arc::new(
        voice_me_tts_piper::PiperTts::new(
            root,
            piper_voice_for(state),
            Arc::new(voice_me_tts_piper::EspeakPhonemizer::new()),
            Arc::new(move || {
                voice_me_tts::sessions::init_runtime(
                    &assets::resolve_runtime_dylib(&runtime_root, None).path,
                )
            }),
        )
        .with_providers(Arc::new(move || piper_providers(&providers_root, backend))),
    ))
}

/// The providers Piper's session is built with on `backend`'s target, on
/// the bundled runtime under `root`, and the NVIDIA libraries that did not
/// load as a note for a failed build.
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn piper_providers(
    root: &Path,
    backend: SpeechBackend,
) -> Result<voice_me_tts_piper::SessionProviders, voice_me_core::VoiceMeError> {
    let runtime = assets::resolve_runtime_dylib(root, None).path;
    let (providers, preload_failures) = voice_me_tts::session_providers(root, &runtime, backend)?;
    let failure_notes = if preload_failures.is_empty() {
        Vec::new()
    } else {
        vec![format!(
            "these NVIDIA libraries did not load: {}",
            preload_failures.join("; ")
        )]
    };
    Ok(voice_me_tts_piper::SessionProviders {
        providers,
        failure_notes,
    })
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn build_piper(_state: &AppState) -> Result<Arc<dyn TtsPort>, String> {
    Err(
        "Piper on this system arrives in a later voice-me release. Choose another backend under \
         Settings → Speech."
            .to_string(),
    )
}

/// The remote provider whose disclosure the hotkey press has to ask about
/// first (Story 3.6, Decision 1): DeepInfra, when it is selected, has a key
/// and has not been confirmed. With no key the capability row blocks
/// instead. Only DeepInfra, Azure (Story 3.14) and Edge TTS (Story 3.17)
/// can generate yet, so fal.ai is never asked about — Story 3.7 widens
/// this. Edge TTS has no key, so it has no key guard.
fn disclosure_needed(state: &AppState) -> Option<RemoteProvider> {
    match &state.backend_selection {
        BackendSelection::Remote(
            provider
            @ (RemoteProvider::DeepInfra | RemoteProvider::Azure | RemoteProvider::EdgeTts),
        ) if (!provider.needs_api_key() || state.api_keys.has(*provider))
            && !state.disclosure_confirmed(*provider) =>
        {
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

/// Whether to warm the engine up now: a Reference Voice Sample exists (or
/// the engine needs none — Piper, Story 3.15), this engine has not been
/// warmed yet, and the check that just landed was run for the *current*
/// selection and found it runnable — so a selection that cannot run here
/// never commits its runtime library just by trying, not even on the
/// strength of a late report about the previous selection.
fn should_warm_up(
    runnable: bool,
    for_current_selection: bool,
    has_active_sample: bool,
    needs_no_sample: bool,
    already_warmed: bool,
) -> bool {
    runnable && for_current_selection && (has_active_sample || needs_no_sample) && !already_warmed
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

fn tray_visual(
    outcome: &DependencyOutcome,
    work: &HashMap<u64, SpeechPhase>,
    speech_failure: Option<&str>,
) -> TrayVisualState {
    if work.values().any(|phase| *phase == SpeechPhase::Playing) {
        return TrayVisualState::Playing;
    }
    if !work.is_empty() {
        return TrayVisualState::Generating;
    }
    let blocker = overlay_blocker(outcome);
    if let Some(reason) = speech_failure.or(blocker.as_deref()) {
        return TrayVisualState::Attention(reason.to_string());
    }
    match outcome {
        DependencyOutcome::Pending => TrayVisualState::Starting,
        DependencyOutcome::Ready(_) => TrayVisualState::Ready,
        DependencyOutcome::Failed(reason) => TrayVisualState::Attention(reason.clone()),
    }
}

#[derive(Default)]
struct TrayActivity {
    work: HashMap<u64, SpeechPhase>,
    next_id: u64,
    failure: Option<String>,
    checking: bool,
}

impl TrayActivity {
    fn visual(&self, outcome: &DependencyOutcome) -> TrayVisualState {
        let visual = tray_visual(outcome, &self.work, self.failure.as_deref());
        if self.checking && visual == TrayVisualState::Ready {
            TrayVisualState::Starting
        } else {
            visual
        }
    }

    fn start_speech(&mut self) -> u64 {
        self.failure = None;
        self.next_id += 1;
        self.work.insert(self.next_id, SpeechPhase::Generating);
        self.next_id
    }

    fn phase(&mut self, id: u64, phase: SpeechPhase) {
        if let Some(active) = self.work.get_mut(&id) {
            *active = phase;
        }
    }

    fn finish(&mut self, id: u64, error: Option<String>) {
        self.work.remove(&id);
        if let Some(error) = error {
            self.failure = Some(error);
        }
    }

    fn readiness_report(
        &mut self,
        report: &DependencyReport,
        selected: &BackendSelection,
        engine_error: Option<String>,
    ) -> bool {
        let current = report.selection.as_ref().map_or_else(
            || report.backend == resolve_backend(selected),
            |selection| selection == selected,
        );
        if !current {
            return false;
        }
        self.checking = false;
        self.failure = engine_error;
        true
    }

    fn failed(&mut self, reason: String) {
        self.checking = false;
        self.failure = Some(reason);
    }
}

fn update_tray(cx: &mut App, state: &TrayVisualState) {
    #[cfg(target_os = "linux")]
    let result = LinuxTrayAdapter.set_visual(cx, state);
    #[cfg(target_os = "windows")]
    let result = WindowsTrayAdapter.set_visual(cx, state);
    if let Err(error) = result {
        eprintln!("could not update tray: {error}");
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
    edge_tts_voices: &[StockVoice],
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
    // Story 3.17: Edge TTS's list, as the program last listed it.
    state.edge_tts_voices = edge_tts_voices.to_vec();
    // Story 3.15: the installed Piper voices, read from the cache.
    state.piper_voices = installed_piper_voices();
    state
}

/// How a failed Azure voice-list fetch starts its message beside the
/// speech language (Story 3.14).
const AZURE_LIST_ERROR: &str = "Couldn't list Azure's voices: ";

/// Remove the speech-language error only if it is a failed Azure voice
/// listing, so a failed language save is never cleared by a fetch.
fn clear_azure_list_error(errors: &mut HashMap<BackendArea, String>) {
    clear_list_error(errors, AZURE_LIST_ERROR);
}

/// How a failed Edge TTS voice listing starts its message beside the
/// speech language (Story 3.17).
// Only the Linux listing (and the tests) use it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const EDGE_TTS_LIST_ERROR: &str = "Couldn't list Edge TTS voices: ";

/// Remove the speech-language error only if it is a failed Edge TTS voice
/// listing, so nothing else there is cleared by a listing.
// Only the Linux listing (and the tests) use it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn clear_edge_tts_list_error(errors: &mut HashMap<BackendArea, String>) {
    clear_list_error(errors, EDGE_TTS_LIST_ERROR);
}

fn clear_list_error(errors: &mut HashMap<BackendArea, String>, prefix: &str) {
    if errors
        .get(&BackendArea::SpeechLanguage)
        .is_some_and(|message| message.starts_with(prefix))
    {
        errors.remove(&BackendArea::SpeechLanguage);
    }
}

/// Apply one finished Edge TTS listing (Story 3.17): a list replaces the
/// held one and clears only the listing's own error; a failure keeps the
/// held list and says why beside the speech language — unless that slot
/// already holds another error (a failed language save), which it never
/// replaces.
// Only the Linux listing (and the tests) use it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn apply_edge_tts_listing(
    result: Result<Vec<StockVoice>, VoiceMeError>,
    voices: &mut Vec<StockVoice>,
    errors: &mut HashMap<BackendArea, String>,
) {
    match result {
        Ok(listed) => {
            clear_edge_tts_list_error(errors);
            *voices = listed;
        }
        Err(error) => {
            eprintln!("could not list Edge TTS voices: {error}");
            let unrelated = errors
                .get(&BackendArea::SpeechLanguage)
                .is_some_and(|message| !message.starts_with(EDGE_TTS_LIST_ERROR));
            if !unrelated {
                errors.insert(
                    BackendArea::SpeechLanguage,
                    format!("{EDGE_TTS_LIST_ERROR}{}", edge_tts_list_reason(&error)),
                );
            }
        }
    }
}

/// How decision 2's note starts beside the speech language (Story 3.13).
// Only the Windows listing (and the tests) use it.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const WINDOWS_VOICE_MISSING: &str = "No installed Windows voice speaks ";

/// How a failed listing of Windows' voices starts its message beside the
/// speech language (Story 3.13).
// Only the Windows listing (and the tests) use it.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const WINDOWS_VOICE_LIST_ERROR: &str = "Couldn't list the System voice's voices: ";

/// Story 3.13 (decision 2): what the Backend tab says beside the speech
/// language when no installed Windows voice speaks the saved `language`
/// (matched as Speak matches it, `tr` ↔ `tr-TR`) — with Windows' own steps
/// to add one. Speak stays blocked by the stock-voice resolution itself.
/// `None` when a voice speaks it, when the language is blank, or when
/// there is no list to judge by.
// Only the Windows listing (and the tests) use it.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn windows_voice_missing_note(language: &str, voices: &[StockVoice]) -> Option<String> {
    let language = language.trim();
    if language.is_empty()
        || voices.is_empty()
        || LanguageBackend::SystemVoice
            .resolve_language(language, voices)
            .is_some()
    {
        return None;
    }
    Some(format!(
        "{WINDOWS_VOICE_MISSING}{language:?}. To add one: {} Or choose another speech language.",
        voice_me_deps::capability::WINDOWS_ADD_VOICES_STEPS.join(" ")
    ))
}

/// Remove the speech-language message only if a listing of Windows' voices
/// set it (decision 2's note or a failed listing), so a failed language
/// save is never cleared by a listing.
// Only the Windows listing (and the tests) use it.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn clear_windows_listing_error(errors: &mut HashMap<BackendArea, String>) {
    clear_list_error(errors, WINDOWS_VOICE_MISSING);
    clear_list_error(errors, WINDOWS_VOICE_LIST_ERROR);
}

/// Story 3.13: apply one finished listing of Windows' voices. The list
/// replaces the held one (a failed listing is an empty list), and the slot
/// beside the speech language says that no installed voice speaks the saved
/// `language` (decision 2) or why the listing failed — clearing only what a
/// listing set itself, and never replacing another error there (a failed
/// language save). The Dependencies tab's System voice row covers a missing
/// engine (decision 3).
// Only the Windows listing (and the tests) use it.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn apply_windows_voice_listing(
    result: Result<Vec<StockVoice>, VoiceMeError>,
    language: &str,
    voices: &mut Vec<StockVoice>,
    errors: &mut HashMap<BackendArea, String>,
) {
    clear_windows_listing_error(errors);
    let message = match result {
        Ok(listed) => {
            let note = windows_voice_missing_note(language, &listed);
            *voices = listed;
            note
        }
        Err(error) => {
            eprintln!("could not list the System voice's voices: {error}");
            voices.clear();
            let reason = match error {
                VoiceMeError::SpeechEngine(reason) => reason,
                other => other.to_string(),
            };
            Some(format!("{WINDOWS_VOICE_LIST_ERROR}{reason}"))
        }
    };
    if let Some(message) = message
        && !errors.contains_key(&BackendArea::SpeechLanguage)
    {
        errors.insert(BackendArea::SpeechLanguage, message);
    }
}

/// The reason a failed Edge TTS listing gives, without the "speech engine
/// failure" frame: the adapter's message already names Edge TTS.
// Only the Linux listing (and the tests) use it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn edge_tts_list_reason(error: &VoiceMeError) -> String {
    match error {
        VoiceMeError::SpeechEngine(reason) => reason.clone(),
        other => other.to_string(),
    }
}

/// What the disclosure for `provider` lists (Story 3.14, D2): for Azure,
/// the saved language and the voice — by name, when the fetched list has
/// it — and for a cloning provider the fixed items.
fn disclosure_text(state: &AppState, provider: RemoteProvider) -> DisclosureText {
    let backend = state.backend_selection.language_backend();
    let language = state.speech_language().unwrap_or_default();
    // Story 3.17: with no voice stored, Edge TTS speaks the language's
    // first listed one, so that is the one named.
    let stored = state.speech_voices.get(backend).or_else(|| {
        (provider == RemoteProvider::EdgeTts)
            .then(|| {
                voice_me_core::resolve_stock_voice(
                    backend,
                    state.stock_voices(backend),
                    language,
                    None,
                )
                .ok()
                .map(|voice| voice.id.as_str())
            })
            .flatten()
    });
    let voice = stored.map(|id| {
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
/// constructor; `WindowsVirtualMicAdapter` reports a missing cable from
/// `play` itself.
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

    #[test]
    fn tray_tracks_overlapping_work_and_retains_attention() {
        let ready = DependencyOutcome::Ready(DependencyReport::new(SpeechBackend::CPU, vec![]));
        let mut work = HashMap::new();
        assert_eq!(
            tray_visual(&DependencyOutcome::Pending, &work, None),
            TrayVisualState::Starting
        );
        assert_eq!(tray_visual(&ready, &work, None), TrayVisualState::Ready);
        work.insert(1, SpeechPhase::Generating);
        work.insert(2, SpeechPhase::Generating);
        assert_eq!(
            tray_visual(&ready, &work, None),
            TrayVisualState::Generating
        );
        work.insert(1, SpeechPhase::Playing);
        assert_eq!(tray_visual(&ready, &work, None), TrayVisualState::Playing);
        work.remove(&1);
        assert_eq!(
            tray_visual(&ready, &work, None),
            TrayVisualState::Generating
        );
        work.remove(&2);
        assert!(matches!(
            tray_visual(&ready, &work, Some("playback failed")),
            TrayVisualState::Attention(_)
        ));
        assert_eq!(tray_visual(&ready, &work, None), TrayVisualState::Ready);
        assert!(matches!(
            tray_visual(
                &DependencyOutcome::Failed("check failed".into()),
                &work,
                None
            ),
            TrayVisualState::Attention(_)
        ));
    }

    #[test]
    fn tray_activity_preserves_work_across_checks_and_completions() {
        let report = DependencyReport::new(SpeechBackend::CPU, vec![])
            .with_selection(BackendSelection::BUNDLED_CPU);
        let ready = DependencyOutcome::Ready(report.clone());
        let pending = DependencyOutcome::Pending;
        let mut activity = TrayActivity::default();
        assert_eq!(activity.visual(&pending), TrayVisualState::Starting);
        activity.checking = true;
        assert_eq!(activity.visual(&ready), TrayVisualState::Starting);
        let blocked = DependencyOutcome::Failed("current blocker".into());
        assert!(matches!(
            activity.visual(&blocked),
            TrayVisualState::Attention(_)
        ));
        let first = activity.start_speech();
        let second = activity.start_speech();
        activity.phase(first, SpeechPhase::Playing);
        assert_eq!(activity.visual(&pending), TrayVisualState::Playing);
        assert!(!activity.readiness_report(&report, &BackendSelection::PIPER_CPU, None));
        activity.finish(first, None);
        assert_eq!(activity.visual(&pending), TrayVisualState::Generating);
        activity.failed("session failed".into());
        assert_eq!(activity.visual(&pending), TrayVisualState::Generating);
        activity.finish(second, None);
        assert!(matches!(
            activity.visual(&pending),
            TrayVisualState::Attention(_)
        ));
        assert!(activity.readiness_report(&report, &BackendSelection::BUNDLED_CPU, None));
        assert_eq!(activity.visual(&ready), TrayVisualState::Ready);
        let third = activity.start_speech();
        activity.finish(third, Some("playback failed".into()));
        assert!(matches!(
            activity.visual(&ready),
            TrayVisualState::Attention(_)
        ));
        activity.start_speech();
        assert_eq!(activity.visual(&ready), TrayVisualState::Generating);
    }

    /// The Hotkey tab's note follows the backend that actually bound
    /// (spec-native-gnome-kde-hotkey, review pass 1).
    #[test]
    fn the_hotkey_note_follows_the_bound_backend() {
        use voice_me_hotkey_linux::{ActiveBackend, Desktop};

        // GNOME/KDE with the native backend bound, or nothing bound yet.
        assert_eq!(
            hotkey_note(Desktop::Gnome, None),
            HotkeyNote::Native("GNOME")
        );
        assert_eq!(
            hotkey_note(Desktop::Gnome, Some(ActiveBackend::Gnome)),
            HotkeyNote::Native("GNOME")
        );
        assert_eq!(hotkey_note(Desktop::Kde, None), HotkeyNote::Native("KDE"));
        assert_eq!(
            hotkey_note(Desktop::Kde, Some(ActiveBackend::Kde)),
            HotkeyNote::Native("KDE")
        );
        // GNOME/KDE on a fallback: no note pointing at a missing entry.
        for fallback in [
            ActiveBackend::Portal,
            ActiveBackend::X11,
            ActiveBackend::Evdev,
        ] {
            assert_eq!(
                hotkey_note(Desktop::Gnome, Some(fallback)),
                HotkeyNote::Plain
            );
            assert_eq!(hotkey_note(Desktop::Kde, Some(fallback)), HotkeyNote::Plain);
        }
        // Other desktops: the compositor steps, whatever bound.
        assert_eq!(hotkey_note(Desktop::Other, None), HotkeyNote::Manual);
        assert_eq!(
            hotkey_note(Desktop::Other, Some(ActiveBackend::Evdev)),
            HotkeyNote::Manual
        );
    }

    /// The asset source serves the prompt bar's voice icon and still serves
    /// gpui-kit's own component icons: a missing SVG draws nothing, silently.
    #[test]
    fn app_assets_serve_the_voice_icon_and_the_component_icons() {
        use gpui_kit::AssetSource as _;
        for path in ["icons/audio-lines.svg", "icons/window-close.svg"] {
            let bytes = AppAssets.load(path).unwrap().expect(path);
            assert!(bytes.starts_with(b"<svg"), "{path} is an SVG");
        }
    }
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

        fn save_overlay_position(&self, _percent: u8) -> Result<AppState, VoiceMeError> {
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
            current_state(&store, &DependencyOutcome::Pending, &[], &[], &[])
                .reference_voice_sample
                .is_none(),
            "nothing recorded yet at launch"
        );
        assert!(
            current_state(&store, &DependencyOutcome::Pending, &[], &[], &[])
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
            current_state(&store, &DependencyOutcome::Pending, &voices, &[], &[]).system_voices,
            voices
        );
        // Story 3.14: Azure's cached list is merged the same way.
        let state = current_state(&store, &DependencyOutcome::Pending, &[], &voices, &[]);
        assert_eq!(state.azure_voices, voices);
        assert!(state.system_voices.is_empty());
        // Story 3.17: and Edge TTS's.
        let state = current_state(&store, &DependencyOutcome::Pending, &[], &[], &voices);
        assert_eq!(state.edge_tts_voices, voices);
        assert!(state.azure_voices.is_empty());
    }

    /// A fresh directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "voice-me-app-{name}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Held by every test that sets or relies on `CACHE_ROOT_ENV` staying
    /// put, so none of them sees the cache root change mid-test.
    static CACHE_ROOT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn cache_root_lock() -> std::sync::MutexGuard<'static, ()> {
        CACHE_ROOT_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Story 3.15: the installed Piper voices are read from the cache on
    /// every read.
    #[test]
    fn the_installed_piper_voices_are_merged_into_every_read() {
        let _lock = cache_root_lock();
        let cache = TempDir::new("piper-cache");
        let key = "tr_TR-dfki-medium";
        let files = assets::piper_voice_files(&cache.0, key).unwrap();
        std::fs::create_dir_all(&files.dir).unwrap();
        std::fs::write(&files.model, b"m").unwrap();
        std::fs::write(&files.config, b"c").unwrap();
        std::fs::write(
            &files.manifest,
            "name = \"dfki\"\nlocale = \"tr_TR\"\nlabel = \"Turkish (Turkey)\"\n\
             quality = \"medium\"\nsource = \"rhasspy/piper-voices\"\n",
        )
        .unwrap();
        let previous = std::env::var_os(assets::CACHE_ROOT_ENV);
        // SAFETY: only this test sets the cache root, under
        // `cache_root_lock`, which every test comparing cache-root reads
        // holds too.
        unsafe { std::env::set_var(assets::CACHE_ROOT_ENV, &cache.0) };
        let store: Arc<dyn SettingsStore> = Arc::new(SampleAppearsLater {
            loads: AtomicUsize::new(0),
        });

        let state = current_state(&store, &DependencyOutcome::Pending, &[], &[], &[]);

        // SAFETY: as above.
        unsafe {
            match previous {
                Some(value) => std::env::set_var(assets::CACHE_ROOT_ENV, value),
                None => std::env::remove_var(assets::CACHE_ROOT_ENV),
            }
        }
        let ids: Vec<_> = state
            .piper_voices
            .iter()
            .map(|voice| voice.id.as_str())
            .collect();
        assert_eq!(ids, vec![key]);
        assert_eq!(state.piper_voices[0].language, "tr_TR");
    }

    /// "Use CPU backend" keeps the saved engine: Piper on CUDA → Piper on
    /// CPU; Chatterbox on an added runtime's CUDA → the bundled CPU.
    #[test]
    fn use_cpu_keeps_the_saved_engine() {
        let dir = TempDir::new("use-cpu");
        let store = FileSettingsStore::with_dirs(dir.0.join("config"), dir.0.join("data"));

        store
            .save_backend_selection(&BackendSelection::Piper {
                target: SpeechExecutionTarget::Cuda,
            })
            .unwrap();
        assert_eq!(use_cpu_selection(&store), BackendSelection::PIPER_CPU);

        store
            .save_backend_selection(&BackendSelection::Local {
                runtime: Some(PathBuf::from("/opt/ort/libonnxruntime.so")),
                target: SpeechExecutionTarget::Cuda,
            })
            .unwrap();
        assert_eq!(use_cpu_selection(&store), BackendSelection::BUNDLED_CPU);
    }

    /// Use on the Piper voices tab saves the language before the voice,
    /// since saving a language clears the voice: a voice of another
    /// locale ends up saved.
    #[test]
    fn using_a_piper_voice_of_another_language_saves_that_voice() {
        let dir = TempDir::new("use-piper-voice");
        let store = FileSettingsStore::with_dirs(dir.0.join("config"), dir.0.join("data"));

        let state = use_piper_voice(&store, "en_US-lessac-medium", "en_US").unwrap();

        assert_eq!(state.speech_languages.piper, "en_US");
        assert_eq!(
            state.speech_voices.piper.as_deref(),
            Some("en_US-lessac-medium")
        );
        let reloaded = store.load().unwrap();
        assert_eq!(reloaded.speech_languages.piper, "en_US");
        assert_eq!(
            reloaded.speech_voices.piper.as_deref(),
            Some("en_US-lessac-medium")
        );
    }

    #[test]
    fn the_resolved_backend_rides_along_with_every_read() {
        let store: Arc<dyn SettingsStore> = Arc::new(SampleAppearsLater {
            loads: AtomicUsize::new(0),
        });

        assert_eq!(
            current_state(&store, &DependencyOutcome::Pending, &[], &[], &[]).speech_backend,
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

    /// spec-overlay-vertical-position: the overlay sits its percentage
    /// down the visible area's free space, horizontally centred.
    #[test]
    fn the_overlay_is_placed_its_percentage_down_the_visible_area() {
        // A 1920×1080 display with a 40 px top panel.
        let visible = Bounds {
            origin: point(px(0.), px(40.)),
            size: size(px(1920.), px(1040.)),
        };
        let window = size(px(OVERLAY_WIDTH), px(40.));

        let top = overlay_bounds_in(visible, window, 0);
        assert_eq!(top.origin, point(px(680.), px(40.)));
        assert_eq!(top.size, window, "the size is never changed");

        let bottom = overlay_bounds_in(visible, window, 100);
        assert_eq!(bottom.origin.y + bottom.size.height, px(1080.));

        let twenty = overlay_bounds_in(visible, window, 20);
        assert_eq!(twenty.origin, point(px(680.), px(240.)));

        let centred = overlay_bounds_in(visible, window, 50);
        assert_eq!(centred.origin, point(px(680.), px(540.)));
    }

    /// A visible area narrower than the overlay keeps the window's left
    /// edge on it rather than off-screen to the left.
    /// The Wayland overlay is a layer surface on the overlay layer,
    /// anchored to the top only (centred horizontally by the compositor),
    /// with the keyboard, and never a negative margin.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_wayland_overlay_is_a_top_anchored_layer_surface() {
        use gpui_kit::layer_shell::{Anchor, KeyboardInteractivity, Layer};
        let options = overlay_layer_shell(240.);
        assert_eq!(options.layer, Layer::Overlay);
        assert_eq!(options.anchor, Anchor::TOP);
        assert_eq!(
            options.keyboard_interactivity,
            KeyboardInteractivity::Exclusive
        );
        assert_eq!(options.margin, Some((px(240.), px(0.), px(0.), px(0.))));
        assert_eq!(overlay_layer_shell(-5.).margin.unwrap().0, px(0.));
    }

    #[test]
    fn the_overlay_stays_on_a_display_narrower_than_itself() {
        let visible = Bounds {
            origin: point(px(100.), px(0.)),
            size: size(px(400.), px(600.)),
        };
        let window = size(px(OVERLAY_WIDTH), px(40.));

        let bounds = overlay_bounds_in(visible, window, 0);
        assert_eq!(bounds.origin, point(px(100.), px(0.)));
        assert_eq!(bounds.size, window);
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
                !needs_restart(None, Some(cuda), false),
                "nothing committed yet: the switch applies to the next Speak Action"
            );
            assert!(
                !needs_restart(Some(bundled), Some(bundled), false),
                "same library: no restart"
            );
            assert!(needs_restart(Some(bundled), Some(cuda), false));
            assert!(
                !needs_restart(Some(bundled), None, false),
                "a remote selection loads no library"
            );
            assert!(
                !needs_restart(None, Some(bundled), true),
                "nothing loaded yet: the new library is the one that will be"
            );
        }

        /// Story 3.8, matrix row "Switch": CPU → CUDA → WebGPU → CPU on the
        /// bundled runtime is one library, so every switch is a session
        /// rebuild — while an added library keeps Decision 2's rule.
        #[test]
        fn switching_targets_on_the_bundled_runtime_never_needs_a_restart() {
            let _lock = super::cache_root_lock();
            let bundled_on = |target| BackendSelection::Local {
                runtime: None,
                target,
            };
            let switches = [
                SpeechExecutionTarget::Cpu,
                SpeechExecutionTarget::Cuda,
                SpeechExecutionTarget::WebGpu,
                SpeechExecutionTarget::Cpu,
            ];
            let committed = selection_library(&bundled_on(SpeechExecutionTarget::Cpu));
            assert!(committed.is_some());
            for target in switches {
                let selection = bundled_on(target);
                let wanted = selection_library(&selection);
                assert_eq!(wanted, committed, "{target:?}: one bundled library");
                assert!(
                    !needs_restart(committed.as_deref(), wanted.as_deref(), false),
                    "{target:?}"
                );
            }

            let added = BackendSelection::Local {
                runtime: Some(PathBuf::from("/opt/ort/libonnxruntime.so")),
                target: SpeechExecutionTarget::Cuda,
            };
            assert!(needs_restart(
                committed.as_deref(),
                selection_library(&added).as_deref(),
                false
            ));
        }

        /// The one exception: Install replaced the bundled library this
        /// process loaded, or staged the new one for the next start. A GPU
        /// selection needs the new one; CPU keeps running on the old one
        /// without a restart, and an added library is never replaced.
        #[test]
        fn a_replaced_or_staged_bundled_library_needs_a_restart_for_a_gpu_selection_only() {
            let bundled = Path::new("/cache/runtime/libonnxruntime.so");
            let gpu = BackendSelection::Local {
                runtime: None,
                target: SpeechExecutionTarget::WebGpu,
            };
            let cpu = BackendSelection::Local {
                runtime: None,
                target: SpeechExecutionTarget::Cpu,
            };

            assert!(is_gpu_selection(&gpu));
            assert!(!is_gpu_selection(&cpu));
            assert!(!is_gpu_selection(&BackendSelection::PIPER_CPU));
            // spec-backend-engine-and-device-selects: Piper on a GPU counts.
            let piper_gpu = BackendSelection::Piper {
                target: SpeechExecutionTarget::Cuda,
            };
            assert!(is_gpu_selection(&piper_gpu));
            assert!(is_gpu_selection(&BackendSelection::Piper {
                target: SpeechExecutionTarget::WebGpu,
            }));
            assert!(runtime_restart_due(&piper_gpu, true, true, false));
            assert!(runtime_restart_due(&piper_gpu, true, false, true));
            assert!(!runtime_restart_due(
                &BackendSelection::PIPER_CPU,
                true,
                true,
                true
            ));

            // bundled, replaced, staged
            assert!(runtime_restart_due(&gpu, true, true, false));
            assert!(
                !runtime_restart_due(&gpu, false, true, false),
                "only the bundled library is ever replaced"
            );
            assert!(runtime_restart_due(&gpu, true, false, true));
            assert!(!runtime_restart_due(&gpu, true, false, false));
            assert!(!runtime_restart_due(&cpu, true, true, true));

            assert!(needs_restart(
                Some(bundled),
                Some(bundled),
                runtime_restart_due(&gpu, true, true, false)
            ));
            assert!(!needs_restart(
                Some(bundled),
                Some(bundled),
                runtime_restart_due(&cpu, true, true, false)
            ));
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
            assert!(should_warm_up(true, true, true, false, false));
            assert!(
                !should_warm_up(false, true, true, false, false),
                "a check that found a blocker never triggers a warm-up"
            );
            assert!(
                !should_warm_up(true, false, true, false, false),
                "a late report about the previous selection never triggers one"
            );
            assert!(
                !should_warm_up(true, true, false, false, false),
                "no sample, no warm-up"
            );
            assert!(
                should_warm_up(true, true, false, true, false),
                "Story 3.15: Piper needs no sample to warm up"
            );
            assert!(
                !should_warm_up(true, true, true, false, true),
                "already warmed"
            );
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

        fn piper_voice(id: &str, locale: &str) -> StockVoice {
            StockVoice {
                id: id.to_string(),
                language: locale.to_string(),
                language_label: "Turkish".to_string(),
                name: id.to_string(),
                priority: 0,
            }
        }

        /// Story 3.15: the check asks about the voice Piper speaks in — the
        /// saved one, the language's top installed one, or on the default
        /// language the default voice — and never needs a key.
        #[test]
        fn the_check_request_names_pipers_voice() {
            let state = AppState {
                backend_selection: BackendSelection::PIPER_CPU,
                ..AppState::default()
            };
            let request = check_request(&state);
            assert_eq!(request.selection, BackendSelection::PIPER_CPU);
            assert_eq!(request.backend, SpeechBackend::CPU);
            // Piper's own device is what the check asks about.
            let webgpu = AppState {
                backend_selection: BackendSelection::Piper {
                    target: SpeechExecutionTarget::WebGpu,
                },
                ..state.clone()
            };
            assert_eq!(
                check_request(&webgpu).backend.target,
                SpeechExecutionTarget::WebGpu
            );
            assert!(!request.has_api_key);
            assert_eq!(
                request.piper_voice.as_deref(),
                Some("tr_TR-fahrettin-medium")
            );

            let mut english = state.clone();
            english.speech_voices.piper = None;
            english.speech_languages.piper = "en_US".to_string();
            assert_eq!(check_request(&english).piper_voice, None);
            english.piper_voices = vec![piper_voice("en_US-lessac-medium", "en_US")];
            assert_eq!(
                check_request(&english).piper_voice.as_deref(),
                Some("en_US-lessac-medium")
            );

            let cpu = AppState {
                backend_selection: BackendSelection::BUNDLED_CPU,
                ..state
            };
            assert_eq!(check_request(&cpu).piper_voice, None);
        }

        /// Story 3.15: Piper loads the bundled CPU runtime — the same
        /// library, so switching to or from the bundled CPU needs no restart.
        #[test]
        fn piper_loads_the_bundled_runtime() {
            let _lock = super::cache_root_lock();
            let _ = voice_me_core::assets::model_cache_root();
            assert_eq!(
                selection_library(&BackendSelection::PIPER_CPU),
                selection_library(&BackendSelection::BUNDLED_CPU)
            );
            if voice_me_core::assets::model_cache_root().is_ok() {
                assert!(selection_library(&BackendSelection::PIPER_CPU).is_some());
            }
            assert_eq!(selection_library(&BackendSelection::SystemVoice), None);
        }

        /// Story 3.15: Piper gets its own engine — nothing built until a
        /// warm-up or a line, and never the "later release" sentence.
        #[test]
        fn piper_gets_its_engine() {
            let (tx, _rx) = mpsc::unbounded();
            let state = AppState {
                backend_selection: BackendSelection::PIPER_CPU,
                ..AppState::default()
            };

            let engine = build_engine(&state, None, &tx, &unused_store());

            assert!(engine.unavailable.is_none(), "{:?}", engine.unavailable);
            let port = engine.port.expect("an engine in the slot");
            assert!(
                !port.is_ready(),
                "no session is built by building the engine"
            );
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

        /// Story 3.17: an Edge TTS selection — no key — with a listed
        /// voice pair.
        fn edge_tts_state() -> AppState {
            let voice = |id: &str, name: &str, priority| StockVoice {
                id: id.to_string(),
                language: "tr-TR".to_string(),
                language_label: "tr-TR".to_string(),
                name: name.to_string(),
                priority,
            };
            AppState {
                backend_selection: BackendSelection::Remote(RemoteProvider::EdgeTts),
                edge_tts_voices: vec![
                    voice("tr-TR-AhmetNeural", "Ahmet (Male)", 0),
                    voice("tr-TR-EmelNeural", "Emel (Female)", 1),
                ],
                ..AppState::default()
            }
        }

        /// Story 3.17: Edge TTS gets its own engine on Linux — built
        /// without looking for the program — and none elsewhere; it is
        /// never an ONNX target.
        #[test]
        fn edge_tts_gets_its_own_engine_and_no_onnx_runtime() {
            let state = edge_tts_state();
            assert_eq!(
                resolve_backend(&state.backend_selection),
                SpeechBackend::CPU
            );
            assert_eq!(selection_library(&state.backend_selection), None);

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
                        .contains("Edge TTS isn't available on this OS yet")
                );
            }
        }

        /// Story 3.17: with no key at all, the check is told the keyless
        /// provider has what it needs, and the disclosure is still asked
        /// for once.
        #[test]
        fn edge_tts_needs_no_key_for_the_check_or_the_disclosure() {
            let state = edge_tts_state();
            assert!(!state.api_keys.has(RemoteProvider::EdgeTts));
            assert!(check_request(&state).has_api_key);

            assert_eq!(disclosure_needed(&state), Some(RemoteProvider::EdgeTts));
            let confirmed = AppState {
                confirmed_disclosures: vec![RemoteProvider::EdgeTts],
                ..state
            };
            assert_eq!(disclosure_needed(&confirmed), None);
        }

        /// Story 3.17: the disclosure names the voice that will speak —
        /// the language's first when none is stored.
        #[test]
        fn edge_tts_disclosure_names_the_voice_in_effect() {
            let text = disclosure_text(&edge_tts_state(), RemoteProvider::EdgeTts);
            assert!(text.items.iter().any(|item| item.contains("tr-TR")));
            assert!(
                text.items
                    .iter()
                    .any(|item| item.contains("Ahmet (Male) (tr-TR-AhmetNeural)")),
                "{:?}",
                text.items
            );
            assert!(text.note.unwrap().contains("Edge Read Aloud"));
        }

        /// Story 3.17: an Edge TTS listing clears only its own error.
        #[test]
        fn the_edge_tts_list_clears_only_its_own_error() {
            let mut errors = HashMap::new();
            errors.insert(
                BackendArea::SpeechLanguage,
                format!("{AZURE_LIST_ERROR}Azure rejected the API key."),
            );
            clear_edge_tts_list_error(&mut errors);
            assert!(errors.contains_key(&BackendArea::SpeechLanguage));

            let reason = edge_tts_list_reason(&VoiceMeError::SpeechEngine(
                "Edge TTS listed no voices — update it: pipx upgrade edge-tts".to_string(),
            ));
            errors.insert(
                BackendArea::SpeechLanguage,
                format!("{EDGE_TTS_LIST_ERROR}{reason}"),
            );
            assert_eq!(
                errors[&BackendArea::SpeechLanguage],
                "Couldn't list Edge TTS voices: Edge TTS listed no voices — update it: pipx upgrade edge-tts"
            );
            clear_azure_list_error(&mut errors);
            assert!(errors.contains_key(&BackendArea::SpeechLanguage));
            clear_edge_tts_list_error(&mut errors);
            assert!(errors.is_empty());
        }

        fn listed(id: &str) -> StockVoice {
            StockVoice {
                id: id.to_string(),
                language: "tr-TR".to_string(),
                language_label: "tr-TR".to_string(),
                name: id.to_string(),
                priority: 0,
            }
        }

        /// Story 3.13 (decision 2): the saved `tr` is spoken by a listed
        /// `tr-TR` voice, so nothing is said; a saved `de` with no German
        /// voice names Windows' Add voices steps beside the speech
        /// language, and Speak stays refused by the stock-voice resolution.
        #[test]
        fn a_windows_language_with_no_installed_voice_names_windows_steps() {
            let listing = || Ok(vec![listed("tolga")]);
            let mut voices = Vec::new();
            let mut errors = HashMap::new();

            apply_windows_voice_listing(listing(), "tr", &mut voices, &mut errors);
            assert_eq!(voices, vec![listed("tolga")]);
            assert!(errors.is_empty(), "{errors:?}");

            apply_windows_voice_listing(listing(), "de", &mut voices, &mut errors);
            let note = &errors[&BackendArea::SpeechLanguage];
            assert!(
                note.contains("No installed Windows voice speaks \"de\""),
                "{note}"
            );
            assert!(
                note.contains("Settings → Time & language → Speech") && note.contains("Add voices"),
                "{note}"
            );
            assert!(
                voice_me_core::resolve_stock_voice(
                    LanguageBackend::SystemVoice,
                    &voices,
                    "de",
                    None
                )
                .is_err()
            );

            // Picking a spoken language clears the note on the next listing.
            apply_windows_voice_listing(listing(), "tr-TR", &mut voices, &mut errors);
            assert!(errors.is_empty(), "{errors:?}");
            assert_eq!(windows_voice_missing_note("de", &[]), None);
        }

        /// A blank or unset language gets no note naming `""`.
        #[test]
        fn a_blank_language_gets_no_windows_voice_note() {
            let voices = vec![listed("tolga")];
            assert_eq!(windows_voice_missing_note("", &voices), None);
            assert_eq!(windows_voice_missing_note("  \n", &voices), None);
            let mut errors = HashMap::new();
            let mut held = Vec::new();
            apply_windows_voice_listing(Ok(voices), "", &mut held, &mut errors);
            assert!(errors.is_empty(), "{errors:?}");
        }

        /// A failed language save beside the speech language survives
        /// every listing, whatever it finds; a listing clears only what a
        /// listing set.
        #[test]
        fn a_windows_listing_never_touches_a_failed_language_save() {
            let save_error = "Couldn't save the speech language: disk full".to_string();
            let failure = || {
                Err(VoiceMeError::SpeechEngine(
                    "Windows speech did not finish listing its voices within 15 seconds"
                        .to_string(),
                ))
            };
            let mut voices = Vec::new();
            let mut errors = HashMap::new();
            errors.insert(BackendArea::SpeechLanguage, save_error.clone());

            for language in ["tr", "de"] {
                apply_windows_voice_listing(
                    Ok(vec![listed("tolga")]),
                    language,
                    &mut voices,
                    &mut errors,
                );
                assert_eq!(errors[&BackendArea::SpeechLanguage], save_error);
            }
            apply_windows_voice_listing(failure(), "tr", &mut voices, &mut errors);
            assert_eq!(errors[&BackendArea::SpeechLanguage], save_error);
            assert!(voices.is_empty());

            // Its own failure is replaced by the next listing's note, then
            // cleared.
            errors.clear();
            apply_windows_voice_listing(failure(), "de", &mut voices, &mut errors);
            assert!(errors[&BackendArea::SpeechLanguage].starts_with(WINDOWS_VOICE_LIST_ERROR));
            apply_windows_voice_listing(Ok(vec![listed("tolga")]), "de", &mut voices, &mut errors);
            assert!(errors[&BackendArea::SpeechLanguage].starts_with(WINDOWS_VOICE_MISSING));
            apply_windows_voice_listing(Ok(vec![listed("tolga")]), "tr", &mut voices, &mut errors);
            assert!(errors.is_empty(), "{errors:?}");
        }

        /// Story 3.13: the adapter `main()` builds asks Windows speech
        /// itself for the System voice row, never the default probe.
        #[test]
        #[cfg(target_os = "windows")]
        fn the_windows_system_voice_row_asks_windows_speech() {
            let (tx, mut rx) = mpsc::unbounded();
            deps_adapter()
                .check(
                    CheckRequest {
                        backend: SpeechBackend::CPU,
                        selection: BackendSelection::SystemVoice,
                        has_api_key: false,
                        has_region: false,
                        has_voice: false,
                        piper_voice: None,
                    },
                    tx,
                )
                .unwrap();
            let AppEvent::DependencyCheckCompleted { report } = rx.try_recv().unwrap() else {
                panic!("the check sends exactly one kind of event");
            };
            let row = report
                .dependencies
                .iter()
                .find(|row| row.kind == DependencyKind::BackendCapability)
                .expect("the System voice row");
            assert!(
                !row.detail
                    .contains(voice_me_deps::capability::NO_SYSTEM_VOICE_PROBE),
                "{}",
                row.detail
            );
        }

        /// Story 3.13: a listing that fails is an empty list, and says why
        /// without the "speech engine failure" frame.
        #[test]
        fn a_failed_windows_listing_empties_the_list_and_says_why() {
            let mut voices = vec![listed("tolga")];
            let mut errors = HashMap::new();
            apply_windows_voice_listing(
                Err(VoiceMeError::SpeechEngine(
                    "Windows speech lists no installed voices".to_string(),
                )),
                "tr",
                &mut voices,
                &mut errors,
            );
            assert!(voices.is_empty());
            assert_eq!(
                errors[&BackendArea::SpeechLanguage],
                "Couldn't list the System voice's voices: Windows speech lists no installed voices"
            );
        }

        /// Story 3.17: a listing replaces the held list and clears only
        /// its own error.
        #[test]
        fn an_edge_tts_listing_replaces_the_list_and_clears_only_its_own_error() {
            let mut voices = vec![listed("tr-TR-OldNeural")];
            let mut errors = HashMap::new();
            errors.insert(
                BackendArea::SpeechLanguage,
                format!("{EDGE_TTS_LIST_ERROR}Edge TTS took longer than 15 s"),
            );
            apply_edge_tts_listing(
                Ok(vec![listed("tr-TR-AhmetNeural")]),
                &mut voices,
                &mut errors,
            );
            assert_eq!(voices, vec![listed("tr-TR-AhmetNeural")]);
            assert!(errors.is_empty());

            let save_error = "Couldn't save the speech language: disk full".to_string();
            errors.insert(BackendArea::SpeechLanguage, save_error.clone());
            apply_edge_tts_listing(Ok(Vec::new()), &mut voices, &mut errors);
            assert_eq!(errors[&BackendArea::SpeechLanguage], save_error);
        }

        /// Story 3.17: a failed listing keeps the held list and says why —
        /// but never over an unrelated speech-language error.
        #[test]
        fn a_failed_edge_tts_listing_keeps_the_list_and_never_hides_another_error() {
            let failure = || {
                Err(VoiceMeError::SpeechEngine(
                    "Edge TTS took longer than 15 s".to_string(),
                ))
            };
            let mut voices = vec![listed("tr-TR-AhmetNeural")];
            let mut errors = HashMap::new();
            apply_edge_tts_listing(failure(), &mut voices, &mut errors);
            assert_eq!(voices, vec![listed("tr-TR-AhmetNeural")]);
            assert_eq!(
                errors[&BackendArea::SpeechLanguage],
                "Couldn't list Edge TTS voices: Edge TTS took longer than 15 s"
            );

            let save_error = "Couldn't save the speech language: disk full".to_string();
            errors.insert(BackendArea::SpeechLanguage, save_error.clone());
            apply_edge_tts_listing(failure(), &mut voices, &mut errors);
            assert_eq!(errors[&BackendArea::SpeechLanguage], save_error);
        }

        /// Story 3.12: the System voice is never an ONNX target — it
        /// resolves to the CPU placeholder, needs no key, no library and no
        /// restart — and on Linux gets the eSpeak NG engine; on Windows
        /// (Story 3.13) Windows speech's, built without calling WinRT.
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
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            {
                assert!(engine.unavailable.is_none());
                assert!(engine.port.expect("an engine in the slot").is_ready());
            }
            #[cfg(not(any(target_os = "linux", target_os = "windows")))]
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

/// What the Hotkey tab shows for this desktop (spec-native-gnome-kde-hotkey).
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HotkeyNote {
    /// The GNOME/KDE note and settings button, naming the desktop.
    Native(&'static str),
    /// The `gdbus call` steps for a compositor binding.
    Manual,
    /// Nothing extra.
    Plain,
}

/// Pure choice of [`HotkeyNote`]: the GNOME/KDE note only when that
/// desktop's native backend is the one bound, or nothing has bound yet on
/// GNOME/KDE (a Save will try it first). A fallback bound on GNOME/KDE gets
/// no note — the desktop's settings hold no voice-me entry to point at.
/// Other desktops always get the compositor steps.
#[cfg(target_os = "linux")]
fn hotkey_note(
    desktop: voice_me_hotkey_linux::Desktop,
    active: Option<voice_me_hotkey_linux::ActiveBackend>,
) -> HotkeyNote {
    use voice_me_hotkey_linux::{ActiveBackend, Desktop};
    match (desktop, active) {
        (Desktop::Other, _) => HotkeyNote::Manual,
        (Desktop::Gnome, None | Some(ActiveBackend::Gnome)) => HotkeyNote::Native("GNOME"),
        (Desktop::Kde, None | Some(ActiveBackend::Kde)) => HotkeyNote::Native("KDE"),
        _ => HotkeyNote::Plain,
    }
}

/// The Hotkey tab's [`HotkeyDesktop`] for the adapter's current state.
#[cfg(target_os = "linux")]
fn hotkey_desktop(adapter: &LinuxHotkeyAdapter) -> HotkeyDesktop {
    match hotkey_note(voice_me_hotkey_linux::desktop(), adapter.active_backend()) {
        HotkeyNote::Native(name) => HotkeyDesktop::Native {
            name: name.into(),
            open_settings: Rc::new(|| {
                voice_me_hotkey_linux::open_shortcut_settings().map_err(|error| error.to_string())
            }),
        },
        HotkeyNote::Manual => HotkeyDesktop::Manual {
            summon_command: voice_me_hotkey_linux::SUMMON_COMMAND.into(),
        },
        HotkeyNote::Plain => HotkeyDesktop::Plain,
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
    // Story 3.8: once the old instance has let go of the runtime, a staged
    // update takes its place — before anything resolves or loads it.
    apply_staged_runtime();

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
        .with_assets(AppAssets);

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
            &current_state(&settings_store, &DependencyOutcome::Pending, &[], &[], &[]),
            runtime_error.as_deref(),
            &event_tx,
            &remote_store,
        )));

        // Stories 3.1/3.2: detection, and one-click provisioning. One
        // adapter for the whole process — it remembers which rows are
        // installing, so a second Install on the same row is refused
        // rather than racing the first on one `.part` file.
        let deps_adapter = Arc::new(deps_adapter());
        let deps_port: Arc<dyn DependencyProvisioningPort> = deps_adapter.clone();
        // Story 3.15: the same adapter serves the Piper voices tab, so a
        // voice its catalog listed is one the voice row can install too.
        let piper_catalog: Arc<dyn PiperCatalogPort> = deps_adapter;

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
        // Story 2.8: VB-CABLE. Each utterance plays to its "CABLE Input",
        // and voice chat selects "CABLE Output" as its microphone. No
        // startup ensure on Windows: installing the driver needs the user's
        // administrator consent, so it happens only from Install on the
        // Dependencies row; until then `play` reports the missing cable.
        #[cfg(target_os = "windows")]
        let virtual_mic_port: Arc<dyn VirtualMicPort> = Arc::new(WindowsVirtualMicAdapter::new());

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
        let hotkey_adapter = Arc::new(LinuxHotkeyAdapter::new(event_tx.clone()));
        #[cfg(target_os = "linux")]
        let hotkey_port: Arc<dyn HotkeyPort> = hotkey_adapter.clone();
        // What the Hotkey tab adds for this desktop, read from the adapter
        // each time Settings opens so it follows whichever backend bound.
        #[cfg(target_os = "linux")]
        let current_hotkey_desktop: Rc<dyn Fn() -> HotkeyDesktop> =
            Rc::new(move || hotkey_desktop(&hotkey_adapter));
        // Not `Send`: the Windows manager owns a window handle, and must
        // stay on GPUI's main thread, whose message loop delivers
        // `WM_HOTKEY`. `Arc<dyn HotkeyPort>` is never sent across threads.
        #[cfg(target_os = "windows")]
        #[allow(clippy::arc_with_non_send_sync)]
        let hotkey_port: Arc<dyn HotkeyPort> =
            Arc::new(WindowsHotkeyAdapter::new(event_tx.clone()));
        #[cfg(target_os = "windows")]
        let current_hotkey_desktop: Rc<dyn Fn() -> HotkeyDesktop> =
            Rc::new(|| HotkeyDesktop::Plain);

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
        let tray_activity = Rc::new(RefCell::new(TrayActivity::default()));

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
        // Story 3.13: bumped by every Windows listing, so one that finishes
        // late — overtaken by a newer one — is discarded.
        let system_voice_generation: Rc<Cell<u64>> = Rc::new(Cell::new(0));
        // Story 3.14 (D1): Azure's voice list, fetched at every check run
        // with Azure selected and a key and region saved. Cached for the
        // session, never persisted; saving a new key or region drops it.
        // The generation is bumped on every drop, so a fetch that was
        // already in flight for the old key or region is discarded.
        let azure_voices: Rc<RefCell<Vec<StockVoice>>> = Rc::new(RefCell::new(Vec::new()));
        let azure_generation: Rc<Cell<u64>> = Rc::new(Cell::new(0));
        // Story 3.17: the voices `edge-tts --list-voices` listed at the last
        // check run with Edge TTS selected and the program found. Held here,
        // never persisted, and merged into `AppState` and the panel.
        let edge_tts_voices: Rc<RefCell<Vec<StockVoice>>> = Rc::new(RefCell::new(Vec::new()));
        // Bumped by every new listing and every selection change, so a
        // listing that finishes late — overtaken, or for a backend no
        // longer selected — is discarded.
        let edge_tts_generation: Rc<Cell<u64>> = Rc::new(Cell::new(0));
        // Story 3.15: the Piper voices tab's state — the catalogs as last
        // fetched (only when the tab asked), each voice's download, and
        // the last failed delete or "Use". The installed list is read from
        // disk on every panel.
        let piper_catalog_state: Rc<RefCell<PiperCatalogState>> =
            Rc::new(RefCell::new(PiperCatalogState::NotLoaded));
        let piper_downloads: Rc<RefCell<HashMap<String, VoiceDownload>>> =
            Rc::new(RefCell::new(HashMap::new()));
        let piper_error: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

        let make_piper_panel: Rc<dyn Fn() -> PiperVoicesPanel> = Rc::new({
            let settings_store = settings_store.clone();
            let piper_catalog_state = piper_catalog_state.clone();
            let piper_downloads = piper_downloads.clone();
            let piper_error = piper_error.clone();
            move || {
                let installed = assets::model_cache_root()
                    .map(|root| assets::installed_piper_voices(&root))
                    .unwrap_or_default();
                // "In use" only while Piper really is the selection.
                let mut state = settings_store.load().unwrap_or_default();
                state.piper_voices = installed
                    .iter()
                    .map(|voice| voice.to_stock_voice())
                    .collect();
                PiperVoicesPanel {
                    in_use: piper_voice_for(&state),
                    installed,
                    catalog: piper_catalog_state.borrow().clone(),
                    downloads: piper_downloads.borrow().clone(),
                    error: piper_error.borrow().clone(),
                }
            }
        });

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
            let edge_tts_voices = edge_tts_voices.clone();
            move || {
                let mut state = settings_store.load().unwrap_or_default();
                state.piper_voices = installed_piper_voices();
                BackendPanel {
                    check_request: check_request(&state),
                    selection: state.backend_selection,
                    runtimes: state.local_runtimes,
                    // Story 3.8: the bundled CUDA and WebGPU devices are
                    // listed once voice-me's all-provider runtime is pinned.
                    bundled_all_providers: bundled_all_providers(),
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
                    edge_tts_voices: edge_tts_voices.borrow().clone(),
                    piper_voices: state.piper_voices,
                }
            }
        });

        // Push that into an open Settings window. Deferred, because the
        // action that caused it usually arrives from inside that very view.
        // Story 3.15: the Piper voices tab is pushed alongside, since a
        // voice installed, deleted or used changes both.
        let push_panel: Rc<dyn Fn(&mut App)> = Rc::new({
            let make_panel = make_panel.clone();
            let make_piper_panel = make_piper_panel.clone();
            let settings_view_slot = settings_view_slot.clone();
            move |cx: &mut App| {
                let make_panel = make_panel.clone();
                let make_piper_panel = make_piper_panel.clone();
                let settings_view_slot = settings_view_slot.clone();
                cx.defer(move |cx| {
                    if let Some(view) = settings_view_slot.borrow().clone() {
                        let panel = make_panel();
                        let piper = make_piper_panel();
                        view.update(cx, |view, cx| {
                            view.set_backend_panel(panel, cx);
                            view.set_piper_voices_panel(piper, cx);
                        });
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
            let system_voice_generation = system_voice_generation.clone();
            let backend_errors = backend_errors.clone();
            let push_panel = push_panel.clone();
            move |cx: &mut App| {
                let Ok(state) = settings_store.load() else {
                    return;
                };
                if !state.backend_selection.is_system_voice() {
                    return;
                }
                #[cfg(target_os = "linux")]
                {
                    // Only the Windows listing counts generations.
                    let _ = &system_voice_generation;
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
                // Story 3.13: Windows' installed voices, listed on the
                // System voice's own worker thread, off GPUI's. A saved
                // language no installed voice speaks is said beside the
                // speech language, with Windows' steps (decision 2).
                #[cfg(target_os = "windows")]
                {
                    let language = state.speech_language().unwrap_or_default().to_string();
                    let generation = system_voice_generation.get() + 1;
                    system_voice_generation.set(generation);
                    let list =
                        cx.background_spawn(
                            async move { voice_me_tts_system_windows::list_voices() },
                        );
                    let settings_store = settings_store.clone();
                    let system_voices = system_voices.clone();
                    let system_voice_generation = system_voice_generation.clone();
                    let backend_errors = backend_errors.clone();
                    let push_panel = push_panel.clone();
                    cx.spawn(async move |cx| {
                        let result = list.await;
                        // Overtaken by a newer listing, or the System voice
                        // or its language is no longer what is saved: not
                        // this list's to set.
                        let current = settings_store.load().is_ok_and(|state| {
                            state.backend_selection.is_system_voice()
                                && state.speech_language().unwrap_or_default() == language
                        });
                        if system_voice_generation.get() != generation || !current {
                            return;
                        }
                        apply_windows_voice_listing(
                            result,
                            &language,
                            &mut system_voices.borrow_mut(),
                            &mut backend_errors.borrow_mut(),
                        );
                        cx.update(|cx| (*push_panel)(cx));
                    })
                    .detach();
                }
                #[cfg(not(any(target_os = "linux", target_os = "windows")))]
                let _ = (
                    &state,
                    &system_voices,
                    &system_voice_generation,
                    &backend_errors,
                    &push_panel,
                    cx,
                );
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

        // Story 3.17: with Edge TTS selected and `edge-tts` found, every
        // Dependency Check also re-reads its voice list in the background
        // and pushes it into an open Backend tab. Selecting Edge TTS never
        // looks for the program; a missing one is the Dependencies row's to
        // say, not this. A failed listing keeps the last list and says why
        // beside the speech language; a listing only ever clears the error
        // it set itself.
        let refresh_edge_tts_voices: Rc<dyn Fn(&mut App)> = Rc::new({
            let settings_store = settings_store.clone();
            let edge_tts_voices = edge_tts_voices.clone();
            let edge_tts_generation = edge_tts_generation.clone();
            let backend_errors = backend_errors.clone();
            let push_panel = push_panel.clone();
            move |cx: &mut App| {
                let selected = settings_store.load().is_ok_and(|state| {
                    state.backend_selection == BackendSelection::Remote(RemoteProvider::EdgeTts)
                });
                if !selected {
                    return;
                }
                #[cfg(target_os = "linux")]
                {
                    if voice_me_tts_edge::find_program().is_none() {
                        clear_edge_tts_list_error(&mut backend_errors.borrow_mut());
                        return;
                    }
                    let generation = edge_tts_generation.get() + 1;
                    edge_tts_generation.set(generation);
                    let list = cx.background_spawn(async move { voice_me_tts_edge::list_voices() });
                    let settings_store = settings_store.clone();
                    let edge_tts_voices = edge_tts_voices.clone();
                    let edge_tts_generation = edge_tts_generation.clone();
                    let backend_errors = backend_errors.clone();
                    let push_panel = push_panel.clone();
                    cx.spawn(async move |cx| {
                        let result = list.await;
                        // Overtaken by a newer listing, or Edge TTS is no
                        // longer the saved selection: not this list's to set.
                        let still_selected = settings_store.load().is_ok_and(|state| {
                            state.backend_selection
                                == BackendSelection::Remote(RemoteProvider::EdgeTts)
                        });
                        if edge_tts_generation.get() != generation || !still_selected {
                            return;
                        }
                        apply_edge_tts_listing(
                            result,
                            &mut edge_tts_voices.borrow_mut(),
                            &mut backend_errors.borrow_mut(),
                        );
                        cx.update(|cx| (*push_panel)(cx));
                    })
                    .detach();
                }
                #[cfg(not(target_os = "linux"))]
                let _ = (
                    &edge_tts_voices,
                    &edge_tts_generation,
                    &backend_errors,
                    &push_panel,
                    cx,
                );
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
            let tray_activity = tray_activity.clone();
            let settings_view_slot = settings_view_slot.clone();
            let provisioning = provisioning.clone();
            let refresh_system_voices = refresh_system_voices.clone();
            let refresh_azure_voices = refresh_azure_voices.clone();
            let refresh_edge_tts_voices = refresh_edge_tts_voices.clone();
            move |cx: &mut App| {
                (*refresh_system_voices)(cx);
                (*refresh_azure_voices)(cx);
                (*refresh_edge_tts_voices)(cx);
                let request = check_request(&current_state(
                    &settings_store,
                    &dependency_outcome.borrow(),
                    &[],
                    &[],
                    &[],
                ));
                let checked_selection = request.selection.clone();
                tray_activity.borrow_mut().checking = true;
                update_tray(
                    cx,
                    &tray_activity.borrow().visual(&dependency_outcome.borrow()),
                );
                if let Some(view) = settings_view_slot.borrow().clone() {
                    view.update(cx, |view, cx| {
                        view.set_dependency_outcome(DependencyOutcome::Pending, cx)
                    });
                }
                let events = event_tx.clone();
                let deps_port = deps_port.clone();
                let check = cx.background_spawn(async move { deps_port.check(request, events) });
                let dependency_outcome = dependency_outcome.clone();
                let tray_activity = tray_activity.clone();
                let settings_store = settings_store.clone();
                let settings_view_slot = settings_view_slot.clone();
                let provisioning = provisioning.clone();
                cx.spawn(async move |cx| {
                    let Err(error) = check.await else { return };
                    if settings_store
                        .load()
                        .is_ok_and(|state| state.backend_selection != checked_selection)
                    {
                        return;
                    }
                    eprintln!("the dependency check could not run: {error}");
                    let outcome = DependencyOutcome::Failed(error.to_string());
                    tray_activity.borrow_mut().checking = false;
                    *dependency_outcome.borrow_mut() = outcome.clone();
                    cx.update(|cx| {
                        update_tray(cx, &tray_activity.borrow().visual(&outcome));
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
            let edge_tts_generation = edge_tts_generation.clone();
            move |selection: BackendSelection, area: BackendArea, cx: &mut App| {
                // Story 3.17: an Edge TTS listing in flight is for the old
                // selection.
                edge_tts_generation.set(edge_tts_generation.get() + 1);
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
                        let replaced_for_gpu = runtime_restart_due(
                            &selection,
                            committed_is_bundled(committed.as_deref()),
                            voice_me_tts::sessions::committed_runtime_replaced(),
                            runtime_staged(),
                        );
                        if needs_restart(committed.as_deref(), wanted.as_deref(), replaced_for_gpu)
                        {
                            restart_pending.set(true);
                        } else {
                            restart_pending.set(false);
                            let state = current_state(
                                &settings_store,
                                &dependency_outcome.borrow(),
                                &[],
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
                // The saved engine on CPU: Piper stays Piper.
                BackendAction::UseCpu => (*apply_selection)(
                    use_cpu_selection(settings_store.as_ref()),
                    BackendArea::Capability,
                    cx,
                ),
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

        // Custom voices — the ones voice-me's own catalog lists — follow
        // their catalog entry: a model replaced at the same URL is fetched
        // again, no Delete and Download needed. Checked at startup, every
        // few hours while voice-me runs, and after each tab refresh. Each
        // update reports like a download from the tab.
        let update_custom_voices: Rc<dyn Fn(&mut App)> = Rc::new({
            let piper_catalog = piper_catalog.clone();
            let event_tx = event_tx.clone();
            move |cx: &mut App| {
                let Some(handle) = cx
                    .try_global::<tokio_bridge::TokioRuntime>()
                    .map(|runtime| runtime.handle().clone())
                else {
                    return;
                };
                let piper_catalog = piper_catalog.clone();
                let events = event_tx.clone();
                let work = tokio_bridge::spawn_blocking_on(&handle, move || {
                    piper_catalog.update_custom_voices(events)
                });
                cx.spawn(async move |_| match work.await {
                    Ok(updated) if !updated.is_empty() => {
                        eprintln!("updated the Piper voices {}", updated.join(", "));
                    }
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!("checking the custom Piper voices for updates failed: {error}");
                    }
                })
                .detach();
            }
        });
        cx.spawn({
            let update_custom_voices = update_custom_voices.clone();
            async move |cx| {
                loop {
                    cx.update(|cx| (*update_custom_voices)(cx));
                    cx.background_executor()
                        .timer(CUSTOM_VOICE_UPDATE_INTERVAL)
                        .await;
                }
            }
        })
        .detach();

        // Story 3.15: everything the Piper voices tab asks for. Catalogs and
        // deletes run on GPUI's background executor; a download runs on the
        // Tokio blocking pool and reports by event.
        let piper_actions: PiperVoicesActions = Rc::new({
            let settings_store = settings_store.clone();
            let piper_catalog = piper_catalog.clone();
            let piper_catalog_state = piper_catalog_state.clone();
            let piper_downloads = piper_downloads.clone();
            let piper_error = piper_error.clone();
            let run_check = run_check.clone();
            let push_panel = push_panel.clone();
            let event_tx = event_tx.clone();
            let update_custom_voices = update_custom_voices.clone();
            move |action: PiperVoicesAction, cx: &mut App| match action {
                PiperVoicesAction::Refresh => {
                    if *piper_catalog_state.borrow() == PiperCatalogState::Loading {
                        return;
                    }
                    *piper_catalog_state.borrow_mut() = PiperCatalogState::Loading;
                    (*push_panel)(cx);
                    let fetch = {
                        let piper_catalog = piper_catalog.clone();
                        cx.background_spawn(async move { piper_catalog.fetch_catalog() })
                    };
                    let piper_catalog_state = piper_catalog_state.clone();
                    let push_panel = push_panel.clone();
                    let update_custom_voices = update_custom_voices.clone();
                    cx.spawn(async move |cx| {
                        let results = fetch.await;
                        *piper_catalog_state.borrow_mut() = PiperCatalogState::Loaded(results);
                        cx.update(|cx| {
                            (*push_panel)(cx);
                            (*update_custom_voices)(cx);
                        });
                    })
                    .detach();
                }
                PiperVoicesAction::Download(entry) => {
                    let key = entry.key.clone();
                    if matches!(
                        piper_downloads.borrow().get(&key),
                        Some(VoiceDownload::Downloading { .. })
                    ) {
                        return;
                    }
                    let Some(handle) = cx
                        .try_global::<tokio_bridge::TokioRuntime>()
                        .map(|runtime| runtime.handle().clone())
                    else {
                        piper_downloads.borrow_mut().insert(
                            key,
                            VoiceDownload::Failed(
                                "voice-me's background runtime did not start this session; \
                                 restart it to download voices."
                                    .to_string(),
                            ),
                        );
                        (*push_panel)(cx);
                        return;
                    };
                    piper_downloads.borrow_mut().insert(
                        key.clone(),
                        VoiceDownload::Downloading { done: 0, total: 0 },
                    );
                    (*push_panel)(cx);
                    let piper_catalog = piper_catalog.clone();
                    let events = event_tx.clone();
                    let work = tokio_bridge::spawn_blocking_on(&handle, move || {
                        piper_catalog.install(&entry, events)
                    });
                    let events = event_tx.clone();
                    cx.spawn(async move |_| {
                        // A job that panicked never reported; this does. An
                        // adapter that did report sends the same sentence
                        // again, which is handled idempotently.
                        if let Err(error) = work.await {
                            let _ = events.unbounded_send(AppEvent::PiperVoiceFinished {
                                key,
                                result: Err(error.to_string()),
                            });
                        }
                    })
                    .detach();
                }
                PiperVoicesAction::Delete(key) => {
                    let delete = {
                        let piper_catalog = piper_catalog.clone();
                        let key = key.clone();
                        cx.background_spawn(async move { piper_catalog.delete(&key) })
                    };
                    let piper_error = piper_error.clone();
                    let run_check = run_check.clone();
                    let push_panel = push_panel.clone();
                    cx.spawn(async move |cx| {
                        *piper_error.borrow_mut() = delete
                            .await
                            .err()
                            .map(|error| format!("Couldn't delete {key}: {error}"));
                        cx.update(|cx| {
                            (*run_check)(cx);
                            (*push_panel)(cx);
                        });
                    })
                    .detach();
                }
                PiperVoicesAction::Use { key, locale } => {
                    let result = use_piper_voice(settings_store.as_ref(), &key, &locale);
                    *piper_error.borrow_mut() = result
                        .err()
                        .map(|error| format!("Couldn't use {key}: {error}"));
                    (*run_check)(cx);
                    (*push_panel)(cx);
                }
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
            let current_hotkey_desktop = current_hotkey_desktop.clone();
            let settings_view_slot = settings_view_slot.clone();
            let deps_port = deps_port.clone();
            let event_tx = event_tx.clone();
            let dependency_outcome = dependency_outcome.clone();
            let provisioning = provisioning.clone();
            let make_panel = make_panel.clone();
            let backend_actions = backend_actions.clone();
            let make_piper_panel = make_piper_panel.clone();
            let piper_actions = piper_actions.clone();
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
                let overlay_position = loaded_state
                    .as_ref()
                    .map_or(voice_me_core::DEFAULT_OVERLAY_POSITION, |state| {
                        state.overlay_position
                    });
                let selected_mic_device = loaded_state.and_then(|state| state.selected_mic_device);
                let hotkey_startup_error = hotkey_startup_error.borrow().clone();

                let settings_store = settings_store.clone();
                let hotkey_port = hotkey_port.clone();
                let current_hotkey_desktop = current_hotkey_desktop.clone();
                let window_slot_on_close = window_slot.clone();
                let view_slot_on_close = settings_view_slot.clone();
                let dependencies = DependenciesTab {
                    deps_port: deps_port.clone(),
                    events: event_tx.clone(),
                    backend: make_panel(),
                    actions: backend_actions.clone(),
                    outcome: dependency_outcome.borrow().clone(),
                    provisioning: provisioning.borrow().clone(),
                    piper: PiperVoicesTab {
                        panel: make_piper_panel(),
                        actions: piper_actions.clone(),
                    },
                };
                let view_slot = settings_view_slot.clone();
                // A modest centred window: with no bounds of its own it
                // opened at the platform default, which filled the screen.
                let options = WindowOptions {
                    app_id: Some("voice-me".into()),
                    window_bounds: Some(WindowBounds::centered(
                        size(px(SETTINGS_WIDTH), px(SETTINGS_HEIGHT)),
                        cx,
                    )),
                    ..SettingsView::window_options()
                };
                let handle = match cx.open_window(options, move |window, cx| {
                    let view = cx.new(|cx| {
                        SettingsView::new(
                            settings_store.clone(),
                            hotkey_port.clone(),
                            has_active_sample,
                            selected_mic_device,
                            saved_hotkey,
                            hotkey_startup_error,
                            overlay_position,
                            is_wayland_session(),
                            dependencies,
                            window,
                            cx,
                        )
                    });
                    *view_slot.borrow_mut() = Some(view.clone());
                    // The GNOME/KDE settings button, or the compositor
                    // steps (spec-native-gnome-kde-hotkey).
                    let hotkey_desktop = current_hotkey_desktop();
                    view.update(cx, |view, cx| view.set_hotkey_desktop(hotkey_desktop, cx));
                    // However the window closes, the slots are emptied, or
                    // the next "Open Settings" would activate a window that
                    // is gone and open nothing.
                    let forget_window: Rc<dyn Fn()> = Rc::new(move || {
                        *window_slot_on_close.borrow_mut() = None;
                        *view_slot_on_close.borrow_mut() = None;
                    });
                    view.update(cx, |view, _| {
                        let forget_window = forget_window.clone();
                        view.set_on_close(move |window, _| {
                            forget_window();
                            window.remove_window();
                        });
                    });
                    window.on_window_should_close(cx, move |_window, _cx| {
                        forget_window();
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
            let edge_tts_voices = edge_tts_voices.clone();
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
                    &edge_tts_voices.borrow(),
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
                let window_bounds = overlay_window_bounds(
                    size(
                        px(OVERLAY_WIDTH),
                        px(if disclosure.is_some() {
                            OVERLAY_DISCLOSURE_HEIGHT
                        } else if blocker.is_some() {
                            OVERLAY_BLOCKED_HEIGHT
                        } else {
                            PROMPT_BAR_HEIGHT
                        }),
                    ),
                    state.overlay_position,
                    cx,
                );
                #[cfg(target_os = "linux")]
                let top_margin = overlay_top_margin(&window_bounds, cx);
                let options_with = |kind: WindowKind| WindowOptions {
                    app_id: Some("voice-me".into()),
                    titlebar: None,
                    // Client-side decorations are what make a borderless
                    // window possible at all on Linux.
                    window_decorations: Some(WindowDecorations::Client),
                    window_background: WindowBackgroundAppearance::Transparent,
                    // Read from settings at every open, so a slider moved
                    // in Settings applies to the next summon.
                    window_bounds: Some(window_bounds),
                    kind,
                    focus: true,
                    is_resizable: false,
                    is_movable: false,
                    is_minimizable: false,
                    ..Default::default()
                };

                // A fresh build closure per attempt: the layer-shell open may
                // be refused and retried as an ordinary window.
                let make_build = {
                    let event_tx = event_tx.clone();
                    let overlay_slot_on_close = overlay_slot_on_close.clone();
                    move || {
                        let event_tx = event_tx.clone();
                        let overlay_slot_on_close = overlay_slot_on_close.clone();
                        let blocker = blocker.clone();
                        let disclosure = disclosure.clone();
                        let disclosure_items = disclosure_items.clone();
                        let on_confirm = on_confirm.clone();
                        move |window: &mut gpui_kit::Window, cx: &mut App| {
                            // Windows draws its own frame around every top-level
                            // window, which showed as a dark rectangle around the
                            // overlay's rounded card.
                            #[cfg(target_os = "windows")]
                            remove_dwm_frame(window);
                            let view = cx.new(|cx| {
                                match (blocker.clone(), disclosure.clone(), on_confirm.clone()) {
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
                                }
                            });
                            window.on_window_should_close(cx, move |_window, _cx| {
                                *overlay_slot_on_close.borrow_mut() = None;
                                true
                            });
                            cx.new(|cx| PromptOverlayView::root(view, window, cx))
                        }
                    }
                };

                #[cfg(target_os = "linux")]
                let first_kind = if is_wayland_session()
                    && !LAYER_SHELL_REFUSED.load(std::sync::atomic::Ordering::Relaxed)
                {
                    WindowKind::LayerShell(overlay_layer_shell(top_margin))
                } else {
                    overlay_window_kind()
                };
                #[cfg(not(target_os = "linux"))]
                let first_kind = overlay_window_kind();
                #[cfg(target_os = "linux")]
                let tried_layer_shell = matches!(first_kind, WindowKind::LayerShell(_));

                let opened = cx.open_window(options_with(first_kind), make_build());
                #[cfg(target_os = "linux")]
                let opened = match opened {
                    Err(error) if tried_layer_shell => {
                        // GNOME has no layer shell: an ordinary toplevel,
                        // placed by the compositor, from now on.
                        LAYER_SHELL_REFUSED.store(true, std::sync::atomic::Ordering::Relaxed);
                        eprintln!("overlay: no layer shell ({error}); using a window");
                        cx.open_window(options_with(WindowKind::Normal), make_build())
                    }
                    other => other,
                };
                let handle = match opened {
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
            (*refresh_edge_tts_voices)(cx);
            let request = check_request(&current_state(
                &settings_store,
                &DependencyOutcome::Pending,
                &[],
                &[],
                &[],
            ));
            let checked_selection = request.selection.clone();
            let events = event_tx.clone();
            let deps_port = deps_port.clone();
            let check = cx.background_spawn(async move { deps_port.check(request, events) });
            let dependency_outcome = dependency_outcome.clone();
            let tray_activity = tray_activity.clone();
            let settings_store = settings_store.clone();
            let settings_view_slot = settings_view_slot.clone();
            let open_dependencies = open_dependencies.clone();
            let dependencies_auto_opened = dependencies_auto_opened.clone();
            cx.spawn(async move |cx| {
                // Only the failure needs handling here: a check that *ran*
                // reports itself by event, below.
                let Err(error) = check.await else { return };
                if settings_store
                    .load()
                    .is_ok_and(|state| state.backend_selection != checked_selection)
                {
                    return;
                }
                eprintln!("the dependency check could not run: {error}");
                let outcome = DependencyOutcome::Failed(error.to_string());
                tray_activity.borrow_mut().checking = false;
                *dependency_outcome.borrow_mut() = outcome.clone();
                cx.update(|cx| {
                    update_tray(cx, &tray_activity.borrow().visual(&outcome));
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
                    AppEvent::SpeechPhaseChanged { id, phase } => {
                        tray_activity.borrow_mut().phase(id, phase);
                        let visual = tray_activity.borrow().visual(&dependency_outcome.borrow());
                        cx.update(|cx| update_tray(cx, &visual));
                    }
                    AppEvent::SpeechFinished { id, error } => {
                        tray_activity.borrow_mut().finish(id, error);
                        let visual = tray_activity.borrow().visual(&dependency_outcome.borrow());
                        cx.update(|cx| update_tray(cx, &visual));
                    }
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
                        let selected = current_state(
                            &settings_store,
                            &DependencyOutcome::Pending,
                            &[],
                            &[],
                            &[],
                        )
                        .backend_selection;
                        if !tray_activity.borrow_mut().readiness_report(
                            &report,
                            &selected,
                            engine.borrow().unavailable.clone(),
                        ) {
                            continue;
                        }
                        let anything_missing = report.has_missing();
                        let runnable = report.speech_engine_blocker().is_none();
                        // A row the check now calls ready has nothing left
                        // to install; whatever was held against it goes.
                        retain_missing_rows(&mut provisioning.borrow_mut(), &report);
                        let rows = provisioning.borrow().clone();
                        let outcome = DependencyOutcome::Ready(report);
                        *dependency_outcome.borrow_mut() = outcome.clone();
                        let visual = tray_activity.borrow().visual(&outcome);

                        let settings_view_slot = settings_view_slot.clone();
                        let open_dependencies = open_dependencies.clone();
                        let dependencies_auto_opened = dependencies_auto_opened.clone();
                        cx.update(|cx| {
                            update_tray(cx, &visual);
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
                            // Story 3.15: Piper needs no sample; its
                            // session builds in about a second.
                            let needs_no_sample = settings_store
                                .load()
                                .is_ok_and(|state| state.backend_selection.is_piper());
                            if should_warm_up(
                                runnable,
                                true,
                                has_active_sample,
                                needs_no_sample,
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

                        // Story 3.8: a runtime install for a GPU selection
                        // that only a restart can load says so at once.
                        if finished_ok
                            && matches!(
                                kind,
                                DependencyKind::OnnxRuntime
                                    | DependencyKind::CudaProvider
                                    | DependencyKind::NvidiaLibraries
                            )
                        {
                            let selection = settings_store
                                .load()
                                .map(|settings| settings.backend_selection)
                                .unwrap_or_default();
                            if runtime_restart_due(
                                &selection,
                                committed_is_bundled(
                                    voice_me_tts::sessions::committed_runtime().as_deref(),
                                ),
                                voice_me_tts::sessions::committed_runtime_replaced(),
                                runtime_staged(),
                            ) {
                                restart_pending.set(true);
                            }
                        }

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
                    // Story 3.15: a Piper voice downloading from its tab.
                    AppEvent::PiperVoiceProgress {
                        key,
                        done_bytes,
                        total_bytes,
                    } => {
                        piper_downloads.borrow_mut().insert(
                            key,
                            VoiceDownload::Downloading {
                                done: done_bytes,
                                total: total_bytes,
                            },
                        );
                        cx.update(|cx| (*push_panel)(cx));
                    }
                    AppEvent::PiperVoiceFinished { key, result } => {
                        match result {
                            Ok(()) => {
                                piper_downloads.borrow_mut().remove(&key);
                            }
                            Err(reason) => {
                                eprintln!("downloading the Piper voice {key} failed: {reason}");
                                piper_downloads
                                    .borrow_mut()
                                    .insert(key, VoiceDownload::Failed(reason));
                            }
                        }
                        // The voice is in the pickers now, and may be the
                        // one a blocking row was waiting for.
                        cx.update(|cx| {
                            (*run_check)(cx);
                            (*push_panel)(cx);
                        });
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
                            Err(reason) => {
                                tray_activity.borrow_mut().failed(reason.clone());
                                ActiveBackend::Failed(reason)
                            }
                        };
                        let visual = tray_activity.borrow().visual(&dependency_outcome.borrow());
                        cx.update(|cx| {
                            update_tray(cx, &visual);
                            (*push_panel)(cx);
                        });
                    }
                    AppEvent::SpeakRequested { text } => {
                        let speech_id = tray_activity.borrow_mut().start_speech();
                        let visual = tray_activity.borrow().visual(&dependency_outcome.borrow());
                        cx.update(|cx| update_tray(cx, &visual));
                        let settings_store = settings_store.clone();
                        let notifications = notification_port.clone();
                        let virtual_mic = virtual_mic_port.clone();
                        let current_engine = engine.borrow().port.clone();
                        let Some(tts) = current_engine else {
                            tray_activity.borrow_mut().finish(
                                speech_id,
                                Some(
                                    engine
                                        .borrow()
                                        .unavailable
                                        .clone()
                                        .unwrap_or_else(|| "Speech engine unavailable".into()),
                                ),
                            );
                            let visual =
                                tray_activity.borrow().visual(&dependency_outcome.borrow());
                            cx.update(|cx| update_tray(cx, &visual));
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
                        let phase_events = event_tx.clone();
                        let finished_events = event_tx.clone();
                        cx.update(|cx| {
                            let state = current_state(
                                &settings_store,
                                &dependency_outcome.borrow(),
                                &system_voices.borrow(),
                                &azure_voices.borrow(),
                                &edge_tts_voices.borrow(),
                            );
                            let work = tokio_bridge::spawn_blocking(cx, move || {
                                // Generation *and* playback, on the blocking
                                // pool: `play` blocks until the audio server
                                // has drained the buffer (AD-5).
                                let audio = voice_me_core::speak_with_phase(
                                    &text,
                                    &state,
                                    tts.as_ref(),
                                    virtual_mic.as_ref(),
                                    notifications.as_ref(),
                                    |phase| {
                                        let _ = phase_events.unbounded_send(
                                            AppEvent::SpeechPhaseChanged {
                                                id: speech_id,
                                                phase,
                                            },
                                        );
                                    },
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
                                let result = work.await;
                                let error = result.as_ref().err().map(ToString::to_string);
                                let _ = finished_events.unbounded_send(AppEvent::SpeechFinished {
                                    id: speech_id,
                                    error,
                                });
                                if let Err(error) = result {
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
