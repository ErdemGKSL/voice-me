//! Voice Setup screen (Story 1.2): Record/Stop/Accept controls with a live
//! Recording indicator and inline playback, backed by `cpal` capture,
//! `hound` WAV encoding, and `rodio` playback. On Accept, the encoded clip is
//! handed to `voice-me-core`'s `SettingsStore` to persist — this view never
//! touches the data/config directory itself (AD-6).

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IndexPath,
    alert::Alert,
    button::{Button, ButtonVariants as _},
    h_flex,
    select::{Select, SelectEvent, SelectState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    PathPromptOptions, Render, SharedString, Styled as _, Task, TestSupportExt as _, Window, div,
    px,
};
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Source as _};
use voice_me_core::{SettingsStore, VoiceMeError};

/// Chatterbox-Multilingual V3's own usage examples use ~10s reference
/// clips; community guidance is 10-30s of clean audio. Enforced here as a
/// hard minimum/maximum per the story's Boundaries & Constraints.
pub const MIN_RECORDING_SECS: f32 = 5.0;
pub const MAX_RECORDING_SECS: f32 = 60.0;

const TOO_SHORT_MESSAGE: &str = "Recording too short — speak for at least 5 seconds";
const MIC_UNAVAILABLE_MESSAGE: &str = "Couldn't access your microphone.";
const PLAYBACK_FAILED_MESSAGE: &str = "Couldn't play back the recording.";
const UNSUPPORTED_FORMAT_MESSAGE: &str = "Unsupported file format.";
const IMPORT_TOO_SHORT_MESSAGE: &str = "Imported clip too short — must be at least 5 seconds";
const IMPORT_TOO_LONG_MESSAGE: &str = "Imported clip too long — must be at most 60 seconds";
const FILE_UNREADABLE_MESSAGE: &str = "Couldn't read the selected file.";
const IMPORT_ENCODE_FAILED_MESSAGE: &str = "Couldn't process the imported file.";
const PICKER_FAILED_MESSAGE: &str = "Couldn't open the file picker.";

/// Whether a just-stopped recording of `duration_secs` may be accepted.
/// Pure function so the 5s rule can be unit-tested without any audio I/O.
pub fn recording_meets_minimum_duration(duration_secs: f32) -> bool {
    duration_secs >= MIN_RECORDING_SECS
}

/// Whether a recording that has been running for `elapsed_secs` should be
/// auto-stopped. Pure function so the 60s rule can be unit-tested without
/// any audio I/O or waiting out a real 60-second timer.
pub fn should_auto_stop(elapsed_secs: f32) -> bool {
    elapsed_secs >= MAX_RECORDING_SECS
}

/// Where `VoiceSetupView` gets microphone capture from. Abstracted behind a
/// trait — rather than calling `cpal` directly — so tests can exercise the
/// Record/Stop/Accept state machine deterministically, without a real audio
/// device (or a real wall-clock 60s wait) in the loop.
pub trait CaptureSource: Send + Sync {
    /// `device` names the input device to capture from (Story 1.5); `None`
    /// means the OS default, matching this method's original behavior.
    fn start(&self, device: Option<&str>) -> Result<Box<dyn CaptureHandle>, VoiceMeError>;
}

/// Where `VoiceSetupView` enumerates available input (microphone) devices
/// from (Story 1.5). Abstracted behind a trait for the same reason as
/// `CaptureSource` — tests get a fixed device list without a real `cpal`
/// host in the loop.
pub trait InputDeviceSource: Send + Sync {
    /// List available input device names. Empty on enumeration failure —
    /// never surfaced as an error; the caller falls back to the OS default
    /// device and hides the device selector.
    fn list_devices(&self) -> Vec<String>;
}

/// Production `InputDeviceSource`: every input device `cpal` can enumerate
/// on the OS default host.
pub struct CpalInputDeviceSource;

impl InputDeviceSource for CpalInputDeviceSource {
    fn list_devices(&self) -> Vec<String> {
        cpal::default_host()
            .input_devices()
            .map(|devices| devices.map(|device| device.to_string()).collect())
            .unwrap_or_default()
    }
}

/// A single in-progress recording started by a `CaptureSource`.
pub trait CaptureHandle: Send {
    fn elapsed_secs(&self) -> f32;
    /// Stop capture and return the collected samples, sample rate and
    /// channel count.
    fn stop(self: Box<Self>) -> (Vec<f32>, u32, u16);
}

/// Production `CaptureSource`: the OS default input device via `cpal`.
pub struct CpalCaptureSource;

/// Live `cpal` microphone capture in progress: owns the input stream and the
/// in-memory sample buffer it feeds.
struct ActiveCapture {
    // Held only to keep the input stream alive for the recording's
    // duration; dropping it stops capture (see `CaptureHandle::stop`).
    #[allow(dead_code)]
    stream: cpal::Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    sample_rate: u32,
    channels: u16,
    started_at: Instant,
}

impl CaptureHandle for ActiveCapture {
    fn elapsed_secs(&self) -> f32 {
        self.started_at.elapsed().as_secs_f32()
    }

    fn stop(self: Box<Self>) -> (Vec<f32>, u32, u16) {
        let samples = self
            .samples
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        (samples, self.sample_rate, self.channels)
        // `self.stream` drops here, stopping the underlying cpal capture.
    }
}

impl CaptureSource for CpalCaptureSource {
    fn start(&self, device: Option<&str>) -> Result<Box<dyn CaptureHandle>, VoiceMeError> {
        start_capture(device).map(|capture| Box::new(capture) as Box<dyn CaptureHandle>)
    }
}

/// Where `VoiceSetupView` gets a user-picked file path from. Abstracted
/// behind a trait — rather than calling `cx.prompt_for_paths` directly — so
/// tests can drive the import flow deterministically, without a real native
/// file dialog in the loop. Mirrors `CaptureSource`'s test-seam pattern.
pub trait ImportSource: Send + Sync {
    fn pick_file(
        &self,
        cx: &mut Context<VoiceSetupView>,
    ) -> Task<Result<Option<PathBuf>, VoiceMeError>>;
}

/// Production `ImportSource`: GPUI's own native file-picker API.
pub struct GpuiImportSource;

impl ImportSource for GpuiImportSource {
    fn pick_file(
        &self,
        cx: &mut Context<VoiceSetupView>,
    ) -> Task<Result<Option<PathBuf>, VoiceMeError>> {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        cx.foreground_executor().spawn(async move {
            match receiver.await {
                Ok(Ok(Some(mut paths))) => Ok(paths.pop()),
                Ok(Ok(None)) => Ok(None),
                Ok(Err(err)) => Err(VoiceMeError::Other(format!(
                    "file picker prompt failed: {err}"
                ))),
                Err(err) => Err(VoiceMeError::Other(format!(
                    "file picker prompt channel dropped: {err}"
                ))),
            }
        })
    }
}

/// A stopped recording long enough to accept, encoded and ready for playback.
struct PendingClip {
    wav_bytes: Vec<u8>,
}

/// `InputDeviceSource` with no devices — used as the default for
/// constructors that don't take one explicitly, so the device selector
/// stays hidden (identical to this view's behavior before Story 1.5's
/// device-selection addition) unless a caller opts in via
/// [`VoiceSetupView::new_with_all_sources`].
struct NoInputDevices;

impl InputDeviceSource for NoInputDevices {
    fn list_devices(&self) -> Vec<String> {
        Vec::new()
    }
}

/// The Voice Setup view.
pub struct VoiceSetupView {
    settings_store: Arc<dyn SettingsStore>,
    capture_source: Arc<dyn CaptureSource>,
    import_source: Arc<dyn ImportSource>,
    capture: Option<Box<dyn CaptureHandle>>,
    is_importing: bool,
    pending_clip: Option<PendingClip>,
    error_message: Option<SharedString>,
    just_saved: bool,
    /// Whether an active Reference Voice Sample exists (Story 1.5): seeded
    /// at construction from the caller's `AppState.reference_voice_sample`
    /// check (the sole "has active sample" signal — no separate persisted
    /// first-run flag), then flipped to `true` by a successful `accept()`
    /// so the empty-state copy this drives never reappears for the rest of
    /// the run once a sample has been accepted, even across a subsequent
    /// re-record/re-import.
    has_active_sample: bool,
    /// The input-device picker (Story 1.5): `None` when no input devices are
    /// enumerable at construction, in which case Record always uses the OS
    /// default (identical to this view's behavior before this story). A
    /// proper `Select` component — chosen over a hand-rolled control per
    /// FR9 — owns the enumerated device list and the current selection;
    /// this view only reacts to its `SelectEvent::Confirm` to persist the
    /// pick via `SettingsStore`.
    mic_device_select: Option<Entity<SelectState<Vec<String>>>>,
    _duration_watch: Option<Task<()>>,
    _playback: Option<MixerDeviceSink>,
}

impl VoiceSetupView {
    /// Constructs the view. `has_active_sample` reflects whether an active
    /// Reference Voice Sample already existed at launch (Story 1.5) —
    /// callers (`voice-me-app`'s composition root) determine this via
    /// `settings_store.load()` up front and pass the result in, since the
    /// view itself is the sole thing deciding which copy to show from it.
    /// `selected_mic_device` is that same load's `AppState.selected_mic_device`.
    pub fn new(
        settings_store: Arc<dyn SettingsStore>,
        has_active_sample: bool,
        selected_mic_device: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_all_sources(
            settings_store,
            Arc::new(CpalCaptureSource),
            Arc::new(GpuiImportSource),
            Arc::new(CpalInputDeviceSource),
            has_active_sample,
            selected_mic_device,
            window,
            cx,
        )
    }

    /// Construct with an explicit `CaptureSource` — used in tests to drive
    /// the Record/Stop/Accept state machine without a real audio device. No
    /// input devices are enumerable (the selector stays hidden); tests that
    /// need one use [`Self::new_with_all_sources`].
    pub fn new_with_capture_source(
        settings_store: Arc<dyn SettingsStore>,
        capture_source: Arc<dyn CaptureSource>,
        has_active_sample: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_all_sources(
            settings_store,
            capture_source,
            Arc::new(GpuiImportSource),
            Arc::new(NoInputDevices),
            has_active_sample,
            None,
            window,
            cx,
        )
    }

    /// Construct with explicit `CaptureSource` and `ImportSource` — used in
    /// tests to drive both the Record/Stop/Accept and Import state machines
    /// deterministically, without a real audio device or native file dialog.
    /// No input devices are enumerable (the selector stays hidden); tests
    /// that need one use [`Self::new_with_all_sources`].
    pub fn new_with_capture_and_import_sources(
        settings_store: Arc<dyn SettingsStore>,
        capture_source: Arc<dyn CaptureSource>,
        import_source: Arc<dyn ImportSource>,
        has_active_sample: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_all_sources(
            settings_store,
            capture_source,
            import_source,
            Arc::new(NoInputDevices),
            has_active_sample,
            None,
            window,
            cx,
        )
    }

    /// Construct with every source explicit, including `InputDeviceSource`
    /// and the previously-selected device (Story 1.5) — used by tests that
    /// exercise device enumeration/selection.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_all_sources(
        settings_store: Arc<dyn SettingsStore>,
        capture_source: Arc<dyn CaptureSource>,
        import_source: Arc<dyn ImportSource>,
        input_device_source: Arc<dyn InputDeviceSource>,
        has_active_sample: bool,
        selected_mic_device: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let available_devices = input_device_source.list_devices();
        let initial_index = selected_mic_device
            .as_ref()
            .and_then(|name| available_devices.iter().position(|device| device == name))
            .map(IndexPath::new);
        let mic_device_select = if available_devices.is_empty() {
            None
        } else {
            let select_state =
                cx.new(|cx| SelectState::new(available_devices, initial_index, window, cx));
            cx.subscribe(&select_state, |this, _state, event, cx| {
                let SelectEvent::Confirm(value) = event;
                match this
                    .settings_store
                    .save_selected_mic_device(value.as_deref())
                {
                    Ok(_) => this.error_message = None,
                    Err(err) => {
                        this.error_message =
                            Some(format!("Couldn't save microphone selection: {err}").into());
                    }
                }
                cx.notify();
            })
            .detach();
            Some(select_state)
        };

        Self {
            settings_store,
            capture_source,
            import_source,
            capture: None,
            is_importing: false,
            pending_clip: None,
            error_message: None,
            just_saved: false,
            has_active_sample,
            mic_device_select,
            _duration_watch: None,
            _playback: None,
        }
    }

    fn start_recording(&mut self, cx: &mut Context<Self>) {
        self.error_message = None;
        self.pending_clip = None;
        self.just_saved = false;
        self._playback = None;

        let selected_device = self
            .mic_device_select
            .as_ref()
            .and_then(|select_state| select_state.read(cx).selected_value().cloned());
        match self.capture_source.start(selected_device.as_deref()) {
            Ok(capture) => {
                self.capture = Some(capture);
                self._duration_watch = Some(cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(100))
                            .await;
                        let hit_max = this
                            .update(cx, |view, cx| {
                                let hit_max = view
                                    .capture
                                    .as_ref()
                                    .map(|capture| should_auto_stop(capture.elapsed_secs()))
                                    .unwrap_or(true);
                                if hit_max {
                                    view.stop_recording(cx);
                                }
                                cx.notify();
                                hit_max
                            })
                            .unwrap_or(true);
                        if hit_max {
                            break;
                        }
                    }
                }));
            }
            Err(_) => {
                self.error_message = Some(MIC_UNAVAILABLE_MESSAGE.into());
            }
        }
        cx.notify();
    }

    fn stop_recording(&mut self, cx: &mut Context<Self>) {
        let Some(capture) = self.capture.take() else {
            return;
        };
        self._duration_watch = None;
        let duration_secs = capture.elapsed_secs();
        let (samples, sample_rate, channels) = capture.stop();

        if !recording_meets_minimum_duration(duration_secs) {
            self.error_message = Some(TOO_SHORT_MESSAGE.into());
            self.pending_clip = None;
            cx.notify();
            return;
        }

        // Wall-clock duration alone doesn't prove any audio was actually
        // captured (e.g. the mic goes silent/disconnects right after
        // start): reject an empty buffer rather than encode/accept it.
        if samples.is_empty() {
            self.error_message = Some(TOO_SHORT_MESSAGE.into());
            self.pending_clip = None;
            cx.notify();
            return;
        }

        match encode_wav(&samples, sample_rate, channels) {
            Ok(wav_bytes) => {
                self.pending_clip = Some(PendingClip { wav_bytes });
                self.error_message = None;
            }
            Err(_) => {
                self.error_message = Some("Couldn't process the recording.".into());
                self.pending_clip = None;
            }
        }
        cx.notify();
    }

    fn start_import(&mut self, cx: &mut Context<Self>) {
        self.is_importing = true;
        cx.notify();

        let pick = self.import_source.pick_file(cx);
        cx.spawn(async move |this, cx| {
            let result = pick.await;
            this.update(cx, |view, cx| {
                view.handle_imported_path(result, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Clears the pending clip, the just-saved banner, and any in-progress
    /// playback together. Used by every `handle_imported_path` branch that
    /// invalidates whatever pending state existed before it ran.
    fn reset_pending_state(&mut self) {
        self.pending_clip = None;
        self.just_saved = false;
        self._playback = None;
    }

    /// Reads, decodes and validates a picked file, converging on the same
    /// `PendingClip` representation `stop_recording` produces. `Ok(None)` is
    /// a true cancel (the file dialog was dismissed without a pick) — in
    /// that case all existing view state (any prior pending clip, error, or
    /// success banner) is left untouched, so a cancel is a true no-op. `Err`
    /// is a genuine file-picker failure and is surfaced as an error message.
    fn handle_imported_path(
        &mut self,
        path: Result<Option<PathBuf>, VoiceMeError>,
        cx: &mut Context<Self>,
    ) {
        self.is_importing = false;

        let path = match path {
            Ok(Some(path)) => path,
            Ok(None) => {
                cx.notify();
                return;
            }
            Err(_) => {
                self.error_message = Some(PICKER_FAILED_MESSAGE.into());
                cx.notify();
                return;
            }
        };

        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => {
                self.reset_pending_state();
                self.error_message = Some(FILE_UNREADABLE_MESSAGE.into());
                cx.notify();
                return;
            }
        };

        let (samples, sample_rate, channels) = match decode_audio_file(&bytes) {
            Ok(decoded) => decoded,
            Err(_) => {
                self.reset_pending_state();
                self.error_message = Some(UNSUPPORTED_FORMAT_MESSAGE.into());
                cx.notify();
                return;
            }
        };

        let duration_secs = if sample_rate == 0 || channels == 0 {
            0.0
        } else {
            samples.len() as f32 / (sample_rate as f32 * channels as f32)
        };

        self.reset_pending_state();

        if !recording_meets_minimum_duration(duration_secs) {
            self.error_message = Some(IMPORT_TOO_SHORT_MESSAGE.into());
            cx.notify();
            return;
        }

        if duration_secs > MAX_RECORDING_SECS {
            self.error_message = Some(IMPORT_TOO_LONG_MESSAGE.into());
            cx.notify();
            return;
        }

        match encode_wav(&samples, sample_rate, channels) {
            Ok(wav_bytes) => {
                self.pending_clip = Some(PendingClip { wav_bytes });
                self.error_message = None;
            }
            Err(_) => {
                self.error_message = Some(IMPORT_ENCODE_FAILED_MESSAGE.into());
                self.pending_clip = None;
            }
        }
        cx.notify();
    }

    fn play_pending_clip(&mut self, cx: &mut Context<Self>) {
        let Some(clip) = self.pending_clip.as_ref() else {
            return;
        };
        match play_wav_bytes(&clip.wav_bytes) {
            Ok(sink) => {
                self._playback = Some(sink);
                self.error_message = None;
            }
            Err(_) => {
                self.error_message = Some(PLAYBACK_FAILED_MESSAGE.into());
            }
        }
        cx.notify();
    }

    fn accept(&mut self, cx: &mut Context<Self>) {
        let Some(clip) = self.pending_clip.take() else {
            return;
        };
        match self
            .settings_store
            .save_reference_voice_sample(&clip.wav_bytes)
        {
            Ok(_state) => {
                self.just_saved = true;
                // An active sample now exists (Story 1.5): flip this
                // permanently for the rest of the run so the first-run
                // empty-state copy never comes back, even if the user goes
                // on to re-record/re-import again this same run — it's
                // still an accurate reflection of `SettingsStore` state,
                // just kept in memory rather than reloaded on every render.
                self.has_active_sample = true;
                self.error_message = None;
                self._playback = None;
            }
            Err(err) => {
                self.error_message = Some(format!("Couldn't save your recording: {err}").into());
                // Keep the clip available so the user can retry Accept.
                self.pending_clip = Some(clip);
            }
        }
        cx.notify();
    }
}

impl Render for VoiceSetupView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_recording = self.capture.is_some();
        let is_importing = self.is_importing;
        let has_pending_clip = self.pending_clip.is_some();
        // First-run empty state (Story 1.5): shown only while no active
        // Reference Voice Sample exists. `has_active_sample` starts from
        // the caller's `AppState` check and flips permanently to `true` on
        // a successful `accept()`, so this branch is never taken again once
        // a sample exists — for the rest of this run or any later one.
        let is_first_run = !self.has_active_sample;

        v_flex()
            .size_full()
            .p_6()
            .gap_4()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(div().text_lg().child(if is_first_run {
                "Record your voice to get started"
            } else {
                "Voice Setup"
            }))
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(if is_first_run {
                        "Record a short clip of your voice (5-60 seconds) to use as your Reference Voice Sample — you'll need one before you can use voice-me."
                    } else {
                        "Record a short clip of your voice (5-60 seconds) to use as your Reference Voice Sample."
                    }),
            )
            // Test-only marker (zero visual footprint) so UI tests can assert
            // the empty-state copy renders — as element presence/absence,
            // per this file's established pattern — without needing to read
            // rendered text back out of a `div`.
            .when(is_first_run, |el| {
                el.child(div().id("voice-setup-first-run-empty-state").test_support())
            })
            .when(is_recording, |el| {
                el.child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(div().size(px(8.)).rounded_full().bg(cx.theme().primary))
                        .child("Recording…"),
                )
            })
            .when_some(self.error_message.clone(), |el, message| {
                el.child(Alert::error("voice-setup-error", message))
            })
            .when(self.just_saved, |el| {
                el.child(Alert::success(
                    "voice-setup-saved",
                    "Saved as your Reference Voice Sample.",
                ))
            })
            .when(has_pending_clip, |el| {
                el.child(
                    Button::new("voice-setup-play")
                        .label("Play")
                        .disabled(is_importing)
                        .on_click(cx.listener(|this, _, _, cx| this.play_pending_clip(cx))),
                )
            })
            .when_some(self.mic_device_select.clone(), |el, select_state| {
                el.child(
                    h_flex().items_center().gap_2().child("Microphone:").child(
                        Select::new(&select_state)
                            .placeholder("System default")
                            .disabled(is_recording || is_importing),
                    ),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("voice-setup-record")
                            .primary()
                            .label("Record")
                            .disabled(is_recording || is_importing)
                            .on_click(cx.listener(|this, _, _, cx| this.start_recording(cx))),
                    )
                    .child(
                        Button::new("voice-setup-stop")
                            .label("Stop")
                            .disabled(!is_recording)
                            .on_click(cx.listener(|this, _, _, cx| this.stop_recording(cx))),
                    )
                    .child(
                        Button::new("voice-setup-import")
                            .label("Import")
                            .disabled(is_recording || is_importing)
                            .on_click(cx.listener(|this, _, _, cx| this.start_import(cx))),
                    )
                    .child(
                        Button::new("voice-setup-accept")
                            .primary()
                            .label("Accept")
                            .disabled(!has_pending_clip || is_importing)
                            .on_click(cx.listener(|this, _, _, cx| this.accept(cx))),
                    ),
            )
    }
}

/// Start capturing from `device` (matched by name against `cpal`'s
/// enumerated input devices), or the OS default input device when `device`
/// is `None` or no longer present (e.g. unplugged since it was selected).
/// Any failure (no device, unsupported config, stream start failure) is
/// mapped to the single "microphone unavailable" case at this `voice-me-ui`
/// boundary — never surfaced from `voice-me-core`.
fn start_capture(device: Option<&str>) -> Result<ActiveCapture, VoiceMeError> {
    let host = cpal::default_host();
    let named_device = device.and_then(|name| {
        host.input_devices()
            .ok()?
            .find(|candidate| candidate.to_string() == name)
    });
    let device = named_device
        .or_else(|| host.default_input_device())
        .ok_or_else(|| VoiceMeError::Other("no default input device".to_string()))?;
    let supported_config = device
        .default_input_config()
        .map_err(|err| VoiceMeError::Other(format!("no default input config: {err}")))?;
    let sample_format = supported_config.sample_format();
    let sample_rate = supported_config.sample_rate();
    let channels = supported_config.channels();
    let stream_config: cpal::StreamConfig = supported_config.into();

    let samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let err_fn = |_err: cpal::Error| {};

    let stream = {
        let samples = samples.clone();
        match sample_format {
            cpal::SampleFormat::F32 => device.build_input_stream(
                stream_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut buf) = samples.lock() {
                        buf.extend_from_slice(data);
                    }
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                stream_config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut buf) = samples.lock() {
                        buf.extend(data.iter().map(|s| *s as f32 / i16::MAX as f32));
                    }
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                stream_config,
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut buf) = samples.lock() {
                        buf.extend(data.iter().map(|s| {
                            (*s as f32 - u16::MAX as f32 / 2.0) / (u16::MAX as f32 / 2.0)
                        }));
                    }
                },
                err_fn,
                None,
            ),
            other => {
                return Err(VoiceMeError::Other(format!(
                    "unsupported input sample format: {other:?}"
                )));
            }
        }
        .map_err(|err| VoiceMeError::Other(format!("failed to build input stream: {err}")))?
    };

    stream
        .play()
        .map_err(|err| VoiceMeError::Other(format!("failed to start input stream: {err}")))?;

    Ok(ActiveCapture {
        stream,
        samples,
        sample_rate,
        channels,
        started_at: Instant::now(),
    })
}

/// Encode captured samples to WAV bytes in memory (no temp file).
fn encode_wav(samples: &[f32], sample_rate: u32, channels: u16) -> Result<Vec<u8>, VoiceMeError> {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec)
            .map_err(|err| VoiceMeError::Other(format!("failed to encode wav: {err}")))?;
        for &sample in samples {
            let clamped = sample.clamp(-1.0, 1.0);
            let value = (clamped * i16::MAX as f32) as i16;
            writer
                .write_sample(value)
                .map_err(|err| VoiceMeError::Other(format!("failed to encode wav: {err}")))?;
        }
        writer
            .finalize()
            .map_err(|err| VoiceMeError::Other(format!("failed to encode wav: {err}")))?;
    }
    Ok(cursor.into_inner())
}

/// Decode an in-memory audio file (WAV or MP3 — the two formats
/// `rodio`'s `wav`/`mp3` decode features support) to raw samples, sample
/// rate and channel count. Mirrors `play_wav_bytes`'s `Decoder` usage.
/// Any failure (wrong format, corrupt data, or a format outside these two)
/// is reported as a single "unsupported format" case at this `voice-me-ui`
/// boundary — callers map it to the user-facing message.
fn decode_audio_file(bytes: &[u8]) -> Result<(Vec<f32>, u32, u16), VoiceMeError> {
    let decoder = Decoder::new(Cursor::new(bytes.to_vec()))
        .map_err(|err| VoiceMeError::Other(format!("failed to decode audio file: {err}")))?;
    let channels = decoder.channels().get();
    let sample_rate = decoder.sample_rate().get();
    let samples: Vec<f32> = decoder.collect();
    Ok((samples, sample_rate, channels))
}

/// Play WAV bytes through the OS default output device, in memory.
fn play_wav_bytes(wav_bytes: &[u8]) -> Result<MixerDeviceSink, VoiceMeError> {
    let sink = DeviceSinkBuilder::open_default_sink()
        .map_err(|err| VoiceMeError::Other(format!("failed to open audio output: {err}")))?;
    let decoder = Decoder::new(Cursor::new(wav_bytes.to_vec()))
        .map_err(|err| VoiceMeError::Other(format!("failed to decode wav: {err}")))?;
    sink.mixer().add(decoder);
    Ok(sink)
}

#[cfg(test)]
mod tests {
    //! UI-level coverage for the story's I/O & Edge-Case Matrix rows that
    //! the pure `recording_meets_minimum_duration`/`should_auto_stop`
    //! functions don't reach on their own: the actual Record/Stop/Accept
    //! state machine, driven through real button clicks against a fake
    //! `CaptureSource` and a fake `SettingsStore` — no real audio device or
    //! wall-clock wait involved. Inline playback itself (which needs a real
    //! output device) stays a manual check, per the spec's Verification
    //! section.

    use std::sync::{Arc, Mutex};

    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{TestAppContext, component::Root, px, size};
    use voice_me_core::{AppState, SettingsStore, VoiceMeError};

    use super::*;

    #[derive(Clone)]
    struct FakeCaptureConfig {
        elapsed_secs: f32,
        samples: Vec<f32>,
        sample_rate: u32,
        channels: u16,
    }

    enum FakeOutcome {
        Fail,
        Succeed(FakeCaptureConfig),
    }

    struct FakeCaptureSource {
        outcome: FakeOutcome,
        calls: Mutex<u32>,
        /// The `device` argument passed to the most recent `start()` call —
        /// lets tests assert Record used the currently selected device
        /// (Story 1.5).
        last_requested_device: Mutex<Option<String>>,
    }

    impl FakeCaptureSource {
        fn new(outcome: FakeOutcome) -> Self {
            Self {
                outcome,
                calls: Mutex::new(0),
                last_requested_device: Mutex::new(None),
            }
        }
    }

    impl CaptureSource for FakeCaptureSource {
        fn start(&self, device: Option<&str>) -> Result<Box<dyn CaptureHandle>, VoiceMeError> {
            *self.calls.lock().unwrap() += 1;
            *self.last_requested_device.lock().unwrap() = device.map(str::to_string);
            match &self.outcome {
                FakeOutcome::Fail => Err(VoiceMeError::Other("no fake microphone".to_string())),
                FakeOutcome::Succeed(config) => {
                    Ok(Box::new(FakeCaptureHandle(config.clone())) as Box<dyn CaptureHandle>)
                }
            }
        }
    }

    struct FakeCaptureHandle(FakeCaptureConfig);

    impl CaptureHandle for FakeCaptureHandle {
        fn elapsed_secs(&self) -> f32 {
            self.0.elapsed_secs
        }

        fn stop(self: Box<Self>) -> (Vec<f32>, u32, u16) {
            (self.0.samples, self.0.sample_rate, self.0.channels)
        }
    }

    fn valid_clip_config(elapsed_secs: f32) -> FakeCaptureConfig {
        FakeCaptureConfig {
            elapsed_secs,
            samples: vec![0.1_f32; 1_000],
            sample_rate: 8_000,
            channels: 1,
        }
    }

    #[derive(Default)]
    struct FakeSettingsStore {
        saved_clips: Mutex<Vec<Vec<u8>>>,
        /// Number of upcoming `save_reference_voice_sample` calls that
        /// should fail before calls start succeeding again. Lets a test
        /// exercise `accept()`'s `Err` branch (which restores the pending
        /// clip for a retry) without touching any real filesystem.
        remaining_failures: Mutex<u32>,
        /// Every device name (or `None`, for "system default") passed to
        /// `save_selected_mic_device`, in call order — lets tests assert
        /// which selection(s) were persisted (Story 1.5).
        saved_mic_devices: Mutex<Vec<Option<String>>>,
        /// When `true`, every `save_selected_mic_device` call fails without
        /// recording anything into `saved_mic_devices` — lets a test
        /// exercise `cycle_device`'s `Err` branch (which must roll back
        /// `selected_device` rather than leave it diverged from disk).
        mic_device_save_fails: Mutex<bool>,
    }

    impl FakeSettingsStore {
        fn failing(times: u32) -> Self {
            Self {
                saved_clips: Mutex::new(Vec::new()),
                remaining_failures: Mutex::new(times),
                saved_mic_devices: Mutex::new(Vec::new()),
                mic_device_save_fails: Mutex::new(false),
            }
        }

        fn failing_mic_device_save() -> Self {
            Self {
                mic_device_save_fails: Mutex::new(true),
                ..Self::default()
            }
        }
    }

    impl SettingsStore for FakeSettingsStore {
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
            Ok(AppState::default())
        }

        fn save_reference_voice_sample(&self, wav_bytes: &[u8]) -> Result<AppState, VoiceMeError> {
            let mut remaining = self.remaining_failures.lock().unwrap();
            if *remaining > 0 {
                *remaining -= 1;
                return Err(VoiceMeError::Other("fake save failure".to_string()));
            }
            self.saved_clips.lock().unwrap().push(wav_bytes.to_vec());
            Ok(AppState::default())
        }

        fn save_hotkey(&self, _hotkey: Option<&str>) -> Result<AppState, VoiceMeError> {
            Ok(AppState::default())
        }

        fn save_selected_mic_device(&self, device: Option<&str>) -> Result<AppState, VoiceMeError> {
            if *self.mic_device_save_fails.lock().unwrap() {
                return Err(VoiceMeError::Other(
                    "fake mic device save failure".to_string(),
                ));
            }
            self.saved_mic_devices
                .lock()
                .unwrap()
                .push(device.map(str::to_string));
            Ok(AppState::default())
        }
    }

    /// Fake `InputDeviceSource`: hands back a fixed device-name list, no
    /// real `cpal` host in the loop.
    struct FakeInputDeviceSource {
        devices: Vec<String>,
    }

    impl InputDeviceSource for FakeInputDeviceSource {
        fn list_devices(&self) -> Vec<String> {
            self.devices.clone()
        }
    }

    /// Fake `ImportSource`: hands back a fixed path (or `None` for a
    /// cancelled dialog) without ever opening a real native file picker.
    struct FakeImportSource {
        result: Result<Option<PathBuf>, ()>,
    }

    impl FakeImportSource {
        fn new(path: Option<PathBuf>) -> Self {
            Self { result: Ok(path) }
        }

        /// Simulates a genuine file-picker failure (as opposed to a user
        /// cancel, which is `Ok(None)`).
        fn failing() -> Self {
            Self { result: Err(()) }
        }
    }

    impl ImportSource for FakeImportSource {
        fn pick_file(
            &self,
            _cx: &mut Context<VoiceSetupView>,
        ) -> Task<Result<Option<PathBuf>, VoiceMeError>> {
            Task::ready(match &self.result {
                Ok(path) => Ok(path.clone()),
                Err(()) => Err(VoiceMeError::Other("fake picker failure".to_string())),
            })
        }
    }

    /// An `ImportSource` that never resolves within the test — used to keep
    /// `is_importing` true for the duration of a test, simulating the window
    /// between an Import click and the picker resolving.
    struct PendingImportSource;

    impl ImportSource for PendingImportSource {
        fn pick_file(
            &self,
            cx: &mut Context<VoiceSetupView>,
        ) -> Task<Result<Option<PathBuf>, VoiceMeError>> {
            cx.foreground_executor()
                .spawn(async { std::future::pending().await })
        }
    }

    /// Encodes `duration_secs` of silence as WAV bytes, reusing the same
    /// `encode_wav` the Record/Stop path produces its `PendingClip` from.
    fn wav_bytes_for_duration(duration_secs: f32) -> Vec<u8> {
        let sample_rate: u32 = 8_000;
        let channels: u16 = 1;
        let sample_count = (duration_secs * sample_rate as f32) as usize;
        let samples = vec![0.1_f32; sample_count];
        encode_wav(&samples, sample_rate, channels).unwrap()
    }

    /// Minimal MSB-first bit writer, used only to hand-assemble a
    /// syntactically valid (spec-legal) MPEG-1 Layer III frame for test
    /// fixtures — no encoder crate needed for silence.
    struct BitWriter {
        bytes: Vec<u8>,
        cur: u8,
        nbits: u8,
    }

    impl BitWriter {
        fn new() -> Self {
            Self {
                bytes: Vec::new(),
                cur: 0,
                nbits: 0,
            }
        }

        fn write_bits(&mut self, value: u32, width: u8) {
            for i in (0..width).rev() {
                let bit = ((value >> i) & 1) as u8;
                self.cur = (self.cur << 1) | bit;
                self.nbits += 1;
                if self.nbits == 8 {
                    self.bytes.push(self.cur);
                    self.cur = 0;
                    self.nbits = 0;
                }
            }
        }

        fn finish(mut self) -> Vec<u8> {
            if self.nbits > 0 {
                self.cur <<= 8 - self.nbits;
                self.bytes.push(self.cur);
            }
            self.bytes
        }
    }

    /// One frame of digital silence: MPEG-1 Layer III, 32kbps, 44.1kHz,
    /// mono, no CRC. With `main_data_begin`/all side-info fields zeroed,
    /// each granule's `part2_3_length` is 0 — no scalefactor/Huffman bits
    /// are read for it, which decodes as silence — so frames need no shared
    /// bit-reservoir state and can just repeat identically. Frame size is
    /// `floor(144 * 32000 / 44100) = 104` bytes: a 4-byte header, 17
    /// zeroed side-info bytes (mono side info is 136 bits), and 83 zeroed
    /// ancillary filler bytes.
    fn silent_mp3_frame() -> Vec<u8> {
        let mut writer = BitWriter::new();
        writer.write_bits(0x7FF, 11); // frame sync
        writer.write_bits(0b11, 2); // MPEG version 1
        writer.write_bits(0b01, 2); // Layer III
        writer.write_bits(1, 1); // protection bit: 1 = no CRC
        writer.write_bits(0b0001, 4); // bitrate index: 32kbps
        writer.write_bits(0b00, 2); // sampling rate index: 44100Hz
        writer.write_bits(0, 1); // padding bit
        writer.write_bits(0, 1); // private bit
        writer.write_bits(0b11, 2); // channel mode: mono
        writer.write_bits(0b00, 2); // mode extension
        writer.write_bits(0, 1); // copyright
        writer.write_bits(0, 1); // original
        writer.write_bits(0b00, 2); // emphasis
        let mut frame = writer.finish();
        debug_assert_eq!(frame.len(), 4, "MP3 header must be exactly 4 bytes");
        frame.resize(104, 0); // zeroed side info + ancillary filler
        frame
    }

    /// Encodes at least `duration_secs` of silence as MP3 bytes by
    /// repeating [`silent_mp3_frame`] enough times (each frame carries
    /// 1152 samples at 44.1kHz).
    fn mp3_bytes_for_duration(duration_secs: f32) -> Vec<u8> {
        const SAMPLE_RATE: f32 = 44_100.0;
        const SAMPLES_PER_FRAME: f32 = 1152.0;
        let frame_count = ((duration_secs * SAMPLE_RATE) / SAMPLES_PER_FRAME)
            .ceil()
            .max(1.0) as usize;
        silent_mp3_frame().repeat(frame_count)
    }

    /// Writes `bytes` to a new temp file with the given extension. Returns
    /// the `NamedTempFile` guard alongside its path — the caller must keep
    /// the guard alive for as long as the path needs to remain readable
    /// (it deletes the file on drop), rather than persisting and leaking it.
    fn write_temp_file(bytes: &[u8], suffix: &str) -> (tempfile::NamedTempFile, PathBuf) {
        use std::io::Write as _;
        let mut file = tempfile::Builder::new()
            .suffix(suffix)
            .tempfile()
            .expect("failed to create temp file");
        file.write_all(bytes).expect("failed to write temp file");
        let path = file.path().to_path_buf();
        (file, path)
    }

    // Assertions below stick to native `Button` state (`disabled()`) and
    // presence/absence of elements that are only ever mounted via `.when(...)`
    // (so absence is a real fact, not a missing test-registration), plus the
    // fake collaborators' call counts. `Alert` (used for the error/success
    // banners) doesn't register itself for `find`/`try_find` — it would need
    // an explicit `.test_support()` wrapper, which requires gpui-kit's
    // `test-support` feature to be compiled into non-test builds too — so
    // banner text isn't asserted on directly here.

    #[gpui_kit::test]
    fn too_short_recording_is_rejected_and_blocks_accept(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Succeed(
            valid_clip_config(2.0), // below MIN_RECORDING_SECS
        )));
        let settings_store = Arc::new(FakeSettingsStore::default());
        let settings_store_dyn: Arc<dyn SettingsStore> = settings_store.clone();
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_capture_source(
                    settings_store_dyn.clone(),
                    capture_source.clone(),
                    true,
                    window,
                    cx,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("voice-setup-record", cx);
            window.click("voice-setup-stop", cx);
            window.render_frame(cx);

            assert!(
                window.try_find("voice-setup-play").is_none(),
                "no pending clip to play when the recording was rejected"
            );
            // Accept is disabled by `has_pending_clip`, and GPUI's real click
            // dispatch refuses disabled controls — clicking it must not reach
            // the store.
            window.click("voice-setup-accept", cx);
        })
        .unwrap();

        assert!(
            settings_store.saved_clips.lock().unwrap().is_empty(),
            "a too-short recording must not be acceptable, even if Accept is clicked"
        );
    }

    #[gpui_kit::test]
    fn microphone_unavailable_leaves_record_usable_for_a_retry(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Fail));
        let settings_store: Arc<dyn SettingsStore> = Arc::new(FakeSettingsStore::default());
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_capture_source(
                    settings_store.clone(),
                    capture_source.clone(),
                    true,
                    window,
                    cx,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("voice-setup-record", cx);
            window.render_frame(cx);
            // Capture never started, so Stop/Accept never got anything to
            // act on and stay effectively inert.
            assert!(window.try_find("voice-setup-play").is_none());

            // Retrying is still possible — Record wasn't left stuck disabled
            // by the failed attempt; a second click is dispatched and handled
            // the same way (proven by the call count below), not swallowed.
            window.click("voice-setup-record", cx);
        })
        .unwrap();

        assert_eq!(
            *capture_source.calls.lock().unwrap(),
            2,
            "Record must accept a retry after a failed capture attempt"
        );
    }

    #[gpui_kit::test]
    fn accepting_a_valid_recording_saves_it_and_disables_accept_again(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Succeed(
            valid_clip_config(12.0),
        )));
        let settings_store = Arc::new(FakeSettingsStore::default());
        let settings_store_dyn: Arc<dyn SettingsStore> = settings_store.clone();
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_capture_source(
                    settings_store_dyn.clone(),
                    capture_source.clone(),
                    true,
                    window,
                    cx,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("voice-setup-record", cx);
            window.click("voice-setup-stop", cx);
            window.render_frame(cx);

            assert!(
                window.try_find("voice-setup-play").is_some(),
                "a valid recording must offer inline playback before Accept"
            );

            window.click("voice-setup-accept", cx);
            window.render_frame(cx);

            assert!(
                window.try_find("voice-setup-play").is_none(),
                "the pending clip is consumed once Accept has processed it"
            );
            // Accept is disabled again (`has_pending_clip` is now false) — a
            // second click must not reach the store a second time.
            window.click("voice-setup-accept", cx);
        })
        .unwrap();

        assert_eq!(
            settings_store.saved_clips.lock().unwrap().len(),
            1,
            "Accept has nothing left to accept once the clip is saved"
        );
    }

    #[gpui_kit::test]
    fn re_recording_after_accept_replaces_the_previously_saved_clip(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Succeed(
            valid_clip_config(12.0),
        )));
        let settings_store = Arc::new(FakeSettingsStore::default());
        let settings_store_dyn: Arc<dyn SettingsStore> = settings_store.clone();
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_capture_source(
                    settings_store_dyn.clone(),
                    capture_source.clone(),
                    true,
                    window,
                    cx,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("voice-setup-record", cx);
            window.click("voice-setup-stop", cx);
            window.click("voice-setup-accept", cx);

            // Recording again (re-record flow) must be possible right after
            // an Accept, and Accept must become available once more —
            // the prior clip is only replaced once this *new* Accept fires.
            window.click("voice-setup-record", cx);
            window.click("voice-setup-stop", cx);
            window.render_frame(cx);
            assert_eq!(window.find("voice-setup-accept").disabled(), None);
            window.click("voice-setup-accept", cx);
        })
        .unwrap();

        assert_eq!(
            settings_store.saved_clips.lock().unwrap().len(),
            2,
            "both the original and the re-recorded clip must have been accepted"
        );
    }

    #[gpui_kit::test]
    fn re_importing_after_accept_replaces_the_previously_saved_clip(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_wav_guard, wav_path) = write_temp_file(&wav_bytes_for_duration(12.0), ".wav");
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Succeed(
            valid_clip_config(12.0),
        )));
        let import_source = Arc::new(FakeImportSource::new(Some(wav_path)));
        let settings_store = Arc::new(FakeSettingsStore::default());
        let settings_store_dyn: Arc<dyn SettingsStore> = settings_store.clone();
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_capture_and_import_sources(
                    settings_store_dyn.clone(),
                    capture_source.clone(),
                    import_source.clone(),
                    true,
                    window,
                    cx,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("voice-setup-record", cx);
            window.click("voice-setup-stop", cx);
            window.click("voice-setup-accept", cx);
            window.click("voice-setup-import", cx);
        })
        .unwrap();

        cx.run_until_parked();

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);

            // Re-importing (as opposed to re-recording) after an Accept must
            // also be possible, and Accept must become available once more —
            // the prior (recorded) clip is only replaced once this *new*
            // (imported) Accept fires.
            assert_eq!(window.find("voice-setup-accept").disabled(), None);
            window.click("voice-setup-accept", cx);
        })
        .unwrap();

        let saved_clips = settings_store.saved_clips.lock().unwrap();
        assert_eq!(
            saved_clips.len(),
            2,
            "both the original recording and the re-imported clip must have been accepted"
        );
        assert_ne!(
            saved_clips[0], saved_clips[1],
            "the second Accept must have saved the newly imported clip, not a stale copy of the first (recorded) clip"
        );
    }

    #[gpui_kit::test]
    fn a_failed_accept_keeps_the_pending_clip_available_for_a_retry(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Succeed(
            valid_clip_config(12.0),
        )));
        // The first `save_reference_voice_sample` call fails; the retry
        // (second call) succeeds.
        let settings_store = Arc::new(FakeSettingsStore::failing(1));
        let settings_store_dyn: Arc<dyn SettingsStore> = settings_store.clone();
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_capture_source(
                    settings_store_dyn.clone(),
                    capture_source.clone(),
                    true,
                    window,
                    cx,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("voice-setup-record", cx);
            window.click("voice-setup-stop", cx);
            window.click("voice-setup-accept", cx);
            window.render_frame(cx);

            assert!(
                window.try_find("voice-setup-play").is_some(),
                "a failed Accept must leave the pending clip (and Play) available for a retry"
            );
            assert_eq!(
                window.find("voice-setup-accept").disabled(),
                None,
                "Accept must still be enabled so the user can retry"
            );

            // Retry: this second call to the fake store succeeds.
            window.click("voice-setup-accept", cx);
            window.render_frame(cx);

            assert!(
                window.try_find("voice-setup-play").is_none(),
                "the pending clip is consumed once the retried Accept succeeds"
            );
        })
        .unwrap();

        assert_eq!(
            settings_store.saved_clips.lock().unwrap().len(),
            1,
            "exactly one save must have gone through, after the first failed attempt"
        );
    }

    #[gpui_kit::test]
    fn recording_auto_stops_at_the_maximum_duration_without_a_manual_stop(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        // Reports an elapsed time already at/over MAX_RECORDING_SECS from
        // the very first poll, so the `cx.spawn` watch loop's auto-stop
        // path — not a manual Stop click — is what ends the recording.
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Succeed(
            valid_clip_config(MAX_RECORDING_SECS),
        )));
        let settings_store: Arc<dyn SettingsStore> = Arc::new(FakeSettingsStore::default());
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_capture_source(
                    settings_store.clone(),
                    capture_source.clone(),
                    true,
                    window,
                    cx,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("voice-setup-record", cx);
        })
        .unwrap();

        // Give the 100ms-interval watch loop a chance to run its first tick
        // and call `stop_recording` on its own.
        cx.executor().advance_clock(Duration::from_millis(500));
        cx.run_until_parked();

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("voice-setup-play").is_some(),
                "hitting MAX_RECORDING_SECS must auto-stop and offer playback, \
                 without a manual Stop click"
            );
            assert_eq!(
                window.find("voice-setup-record").disabled(),
                None,
                "recording must have actually stopped, freeing Record again"
            );
        })
        .unwrap();
    }

    // Import coverage below drives the same fake-`SettingsStore`/real-click
    // pattern as Record/Stop/Accept above, swapping in a `FakeImportSource`
    // in place of a real native file dialog and `Task::ready` in place of a
    // real async round trip. `cx.run_until_parked()` after the Import click
    // lets `start_import`'s spawned task actually run to completion before
    // assertions.

    #[gpui_kit::test]
    fn valid_wav_import_offers_playback_and_accept(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_wav_guard, wav_path) = write_temp_file(&wav_bytes_for_duration(12.0), ".wav");
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Fail));
        let import_source = Arc::new(FakeImportSource::new(Some(wav_path)));
        let settings_store = Arc::new(FakeSettingsStore::default());
        let settings_store_dyn: Arc<dyn SettingsStore> = settings_store.clone();
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_capture_and_import_sources(
                    settings_store_dyn.clone(),
                    capture_source.clone(),
                    import_source.clone(),
                    true,
                    window,
                    cx,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("voice-setup-import", cx);
        })
        .unwrap();

        cx.run_until_parked();

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("voice-setup-play").is_some(),
                "a valid imported wav must offer inline playback before Accept"
            );
            assert_eq!(
                window.find("voice-setup-accept").disabled(),
                None,
                "Accept must be enabled once the imported clip validates"
            );
            window.click("voice-setup-accept", cx);
        })
        .unwrap();

        assert_eq!(
            settings_store.saved_clips.lock().unwrap().len(),
            1,
            "accepting an imported wav must save it exactly like a recording"
        );
    }

    #[gpui_kit::test]
    fn valid_mp3_import_converges_on_the_same_pending_clip_flow(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_mp3_guard, mp3_path) = write_temp_file(&mp3_bytes_for_duration(12.0), ".mp3");
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Fail));
        let import_source = Arc::new(FakeImportSource::new(Some(mp3_path)));
        let settings_store = Arc::new(FakeSettingsStore::default());
        let settings_store_dyn: Arc<dyn SettingsStore> = settings_store.clone();
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_capture_and_import_sources(
                    settings_store_dyn.clone(),
                    capture_source.clone(),
                    import_source.clone(),
                    true,
                    window,
                    cx,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("voice-setup-import", cx);
        })
        .unwrap();

        cx.run_until_parked();

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("voice-setup-play").is_some(),
                "a valid imported mp3 must decode and offer inline playback before Accept"
            );
            assert_eq!(window.find("voice-setup-accept").disabled(), None);
            window.click("voice-setup-accept", cx);
        })
        .unwrap();

        assert_eq!(
            settings_store.saved_clips.lock().unwrap().len(),
            1,
            "accepting an imported mp3 must converge on the same save path as wav/recording"
        );
    }

    #[gpui_kit::test]
    fn unsupported_format_import_is_rejected_and_blocks_accept(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_bad_guard, bad_path) = write_temp_file(b"this is not a wav or mp3 file", ".flac");
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Fail));
        let import_source = Arc::new(FakeImportSource::new(Some(bad_path)));
        let settings_store = Arc::new(FakeSettingsStore::default());
        let settings_store_dyn: Arc<dyn SettingsStore> = settings_store.clone();
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_capture_and_import_sources(
                    settings_store_dyn.clone(),
                    capture_source.clone(),
                    import_source.clone(),
                    true,
                    window,
                    cx,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("voice-setup-import", cx);
        })
        .unwrap();

        cx.run_until_parked();

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("voice-setup-play").is_none(),
                "an unsupported/corrupt import must not offer playback"
            );
            // Accept is disabled by `has_pending_clip`, and GPUI's real click
            // dispatch refuses disabled controls — clicking it must not reach
            // the store.
            window.click("voice-setup-accept", cx);
        })
        .unwrap();

        assert!(
            settings_store.saved_clips.lock().unwrap().is_empty(),
            "an unsupported import must never reach the store"
        );
    }

    #[gpui_kit::test]
    fn too_short_import_is_rejected_and_blocks_accept(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_short_guard, short_path) = write_temp_file(&wav_bytes_for_duration(2.0), ".wav");
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Fail));
        let import_source = Arc::new(FakeImportSource::new(Some(short_path)));
        let settings_store = Arc::new(FakeSettingsStore::default());
        let settings_store_dyn: Arc<dyn SettingsStore> = settings_store.clone();
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_capture_and_import_sources(
                    settings_store_dyn.clone(),
                    capture_source.clone(),
                    import_source.clone(),
                    true,
                    window,
                    cx,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("voice-setup-import", cx);
        })
        .unwrap();

        cx.run_until_parked();

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("voice-setup-play").is_none(),
                "an imported clip below MIN_RECORDING_SECS must not offer playback"
            );
            window.click("voice-setup-accept", cx);
        })
        .unwrap();

        assert!(
            settings_store.saved_clips.lock().unwrap().is_empty(),
            "a too-short imported clip must never reach the store"
        );
    }

    #[gpui_kit::test]
    fn too_long_import_is_rejected_and_blocks_accept(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_long_guard, long_path) =
            write_temp_file(&wav_bytes_for_duration(MAX_RECORDING_SECS + 5.0), ".wav");
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Fail));
        let import_source = Arc::new(FakeImportSource::new(Some(long_path)));
        let settings_store = Arc::new(FakeSettingsStore::default());
        let settings_store_dyn: Arc<dyn SettingsStore> = settings_store.clone();
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_capture_and_import_sources(
                    settings_store_dyn.clone(),
                    capture_source.clone(),
                    import_source.clone(),
                    true,
                    window,
                    cx,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("voice-setup-import", cx);
        })
        .unwrap();

        cx.run_until_parked();

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("voice-setup-play").is_none(),
                "an imported clip above MAX_RECORDING_SECS must not offer playback"
            );
            window.click("voice-setup-accept", cx);
        })
        .unwrap();

        assert!(
            settings_store.saved_clips.lock().unwrap().is_empty(),
            "a too-long imported clip must never reach the store"
        );
    }

    #[gpui_kit::test]
    fn cancelled_import_leaves_existing_pending_clip_untouched(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Succeed(
            valid_clip_config(12.0),
        )));
        let import_source = Arc::new(FakeImportSource::new(None));
        let settings_store: Arc<dyn SettingsStore> = Arc::new(FakeSettingsStore::default());
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_capture_and_import_sources(
                    settings_store.clone(),
                    capture_source.clone(),
                    import_source.clone(),
                    true,
                    window,
                    cx,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            // Establish prior state: a real pending clip from a recording.
            window.click("voice-setup-record", cx);
            window.click("voice-setup-stop", cx);
            window.render_frame(cx);
            assert!(window.try_find("voice-setup-play").is_some());

            // Cancelling the import dialog must be a true no-op.
            window.click("voice-setup-import", cx);
        })
        .unwrap();

        cx.run_until_parked();

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("voice-setup-play").is_some(),
                "cancelling the import dialog must leave the prior pending clip untouched"
            );
            assert_eq!(
                window.find("voice-setup-accept").disabled(),
                None,
                "Accept must stay enabled for the untouched pending clip"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn unreadable_import_path_is_rejected_and_blocks_accept(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        // A tempdir path that is never written to: the directory exists but
        // this specific file inside it never does, so `std::fs::read` hits
        // the same `Err` a vanished/permission-denied file would.
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let missing_path = dir.path().join("does-not-exist.wav");
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Fail));
        let import_source = Arc::new(FakeImportSource::new(Some(missing_path)));
        let settings_store = Arc::new(FakeSettingsStore::default());
        let settings_store_dyn: Arc<dyn SettingsStore> = settings_store.clone();
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_capture_and_import_sources(
                    settings_store_dyn.clone(),
                    capture_source.clone(),
                    import_source.clone(),
                    true,
                    window,
                    cx,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("voice-setup-import", cx);
        })
        .unwrap();

        cx.run_until_parked();

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("voice-setup-play").is_none(),
                "a file that can't be read must not offer playback"
            );
            // Accept is disabled by `has_pending_clip`, and GPUI's real click
            // dispatch refuses disabled controls — clicking it must not reach
            // the store.
            window.click("voice-setup-accept", cx);
        })
        .unwrap();

        assert!(
            settings_store.saved_clips.lock().unwrap().is_empty(),
            "a file that can't be read must never reach the store"
        );
    }

    #[gpui_kit::test]
    fn accept_and_play_are_inert_while_an_import_is_in_flight(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        // A prior pending clip — including one a failed Accept left behind
        // awaiting retry — must not become acceptable again just because a
        // new Import click is currently in flight.
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Succeed(
            valid_clip_config(12.0),
        )));
        let import_source = Arc::new(PendingImportSource);
        let settings_store = Arc::new(FakeSettingsStore::default());
        let settings_store_dyn: Arc<dyn SettingsStore> = settings_store.clone();
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_capture_and_import_sources(
                    settings_store_dyn.clone(),
                    capture_source.clone(),
                    import_source.clone(),
                    true,
                    window,
                    cx,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("voice-setup-record", cx);
            window.click("voice-setup-stop", cx);
            window.render_frame(cx);
            assert!(window.try_find("voice-setup-play").is_some());

            // Kick off an import that never resolves (simulating the window
            // between the click and the native picker returning).
            window.click("voice-setup-import", cx);
            window.render_frame(cx);

            // Accept and Play are disabled by `is_importing`, and GPUI's
            // real click dispatch refuses disabled controls — clicking them
            // must not reach the store or start playback.
            window.click("voice-setup-accept", cx);
            window.click("voice-setup-play", cx);
        })
        .unwrap();

        assert!(
            settings_store.saved_clips.lock().unwrap().is_empty(),
            "Accept must not reach the store while an import is still pending"
        );
    }

    #[gpui_kit::test]
    fn a_genuine_picker_failure_is_reported_and_leaves_no_pending_clip(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        // Distinguishes a real file-picker failure from a user cancel
        // (`Ok(None)`), which must stay a silent no-op.
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Fail));
        let import_source = Arc::new(FakeImportSource::failing());
        let settings_store: Arc<dyn SettingsStore> = Arc::new(FakeSettingsStore::default());
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_capture_and_import_sources(
                    settings_store.clone(),
                    capture_source.clone(),
                    import_source.clone(),
                    true,
                    window,
                    cx,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("voice-setup-import", cx);
        })
        .unwrap();

        cx.run_until_parked();

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("voice-setup-play").is_none(),
                "a genuine picker failure must not offer playback"
            );
        })
        .unwrap();
    }

    // Story 1.5: the first-run empty-state copy is driven purely by the
    // `has_active_sample` flag threaded through construction (never by a
    // separate persisted flag). The marker element below
    // ("voice-setup-first-run-empty-state") is only ever mounted by
    // `.when(is_first_run, ...)`, so its presence/absence is a real fact
    // about which copy branch rendered — mirroring this file's existing
    // pattern of asserting on element presence rather than on rendered text.

    #[gpui_kit::test]
    fn no_active_sample_at_construction_shows_the_first_run_empty_state(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Fail));
        let settings_store: Arc<dyn SettingsStore> = Arc::new(FakeSettingsStore::default());
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_capture_source(
                    settings_store.clone(),
                    capture_source.clone(),
                    false, // no active sample at construction
                    window,
                    cx,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .try_find("voice-setup-first-run-empty-state")
                    .is_some(),
                "constructing with no active sample must show the first-run empty state"
            );
            assert_eq!(
                window.find("voice-setup-record").disabled(),
                None,
                "Record must be the highlighted primary action in the empty state"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn an_existing_active_sample_at_construction_shows_the_normal_view(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Fail));
        let settings_store: Arc<dyn SettingsStore> = Arc::new(FakeSettingsStore::default());
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_capture_source(
                    settings_store.clone(),
                    capture_source.clone(),
                    true, // active sample already exists at construction
                    window,
                    cx,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .try_find("voice-setup-first-run-empty-state")
                    .is_none(),
                "constructing with an existing active sample must not show the first-run empty state"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn the_first_run_empty_state_never_reappears_after_accepting_a_sample_this_run(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Succeed(
            valid_clip_config(12.0),
        )));
        let settings_store: Arc<dyn SettingsStore> = Arc::new(FakeSettingsStore::default());
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_capture_source(
                    settings_store.clone(),
                    capture_source.clone(),
                    false, // first run: no active sample yet
                    window,
                    cx,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .try_find("voice-setup-first-run-empty-state")
                    .is_some(),
                "must start in the first-run empty state"
            );

            window.click("voice-setup-record", cx);
            window.click("voice-setup-stop", cx);
            window.click("voice-setup-accept", cx);
            window.render_frame(cx);

            assert!(
                window
                    .try_find("voice-setup-first-run-empty-state")
                    .is_none(),
                "the empty-state copy must not reappear for the rest of the run once a \
                 sample has just been accepted"
            );
        })
        .unwrap();
    }

    // Story 1.5: input device selection, via a real gpui-component `Select`
    // (not a hand-rolled control, per FR9). The selector is hidden whenever
    // no devices are enumerable (matching this view's pre-story behavior
    // exactly); otherwise `VoiceSetupView` owns an `Entity<SelectState<..>>`
    // and reacts to its `SelectEvent::Confirm` to persist the choice.
    // Simulating a real click-open-click-item popover flow isn't how
    // gpui-component's own test suite drives `Select` either — these tests
    // follow the same convention: mutate `SelectState` directly
    // (`set_selected_value`) and, to test the persistence glue specifically,
    // emit `SelectEvent::Confirm` directly on the entity.

    #[gpui_kit::test]
    fn no_enumerable_devices_hides_the_device_selector(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Fail));
        let settings_store: Arc<dyn SettingsStore> = Arc::new(FakeSettingsStore::default());
        let mut voice_setup = None;
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_capture_source(
                    settings_store.clone(),
                    capture_source.clone(),
                    true,
                    window,
                    cx,
                )
            });
            voice_setup = Some(view.clone());
            Root::new(view, window, cx)
        });
        let voice_setup = voice_setup.unwrap();

        cx.update_window(handle.into(), |_, _window, cx| {
            assert!(
                voice_setup.read(cx).mic_device_select.is_none(),
                "no devices enumerable must hide the device selector"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn an_enumerable_device_list_is_used_to_record_once_selected(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Succeed(
            valid_clip_config(12.0),
        )));
        let import_source = Arc::new(FakeImportSource::new(None));
        let input_device_source = Arc::new(FakeInputDeviceSource {
            devices: vec!["Mic A".to_string(), "Mic B".to_string()],
        });
        let settings_store: Arc<dyn SettingsStore> = Arc::new(FakeSettingsStore::default());
        let mut voice_setup = None;
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_all_sources(
                    settings_store.clone(),
                    capture_source.clone(),
                    import_source.clone(),
                    input_device_source.clone(),
                    true,
                    None, // no device selected yet — starts on "System default"
                    window,
                    cx,
                )
            });
            voice_setup = Some(view.clone());
            Root::new(view, window, cx)
        });
        let voice_setup = voice_setup.unwrap();

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let mic_device_select = voice_setup
                .read(cx)
                .mic_device_select
                .clone()
                .expect("enumerable devices must produce a device selector");

            mic_device_select.update(cx, |state, cx| {
                state.set_selected_value(&"Mic A".to_string(), window, cx);
            });

            window.click("voice-setup-record", cx);
            window.render_frame(cx);
            assert_eq!(
                *capture_source.last_requested_device.lock().unwrap(),
                Some("Mic A".to_string()),
                "Record must capture from the currently selected device"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn confirming_a_selection_persists_it_via_settings_store(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Fail));
        let import_source = Arc::new(FakeImportSource::new(None));
        let input_device_source = Arc::new(FakeInputDeviceSource {
            devices: vec!["Mic A".to_string(), "Mic B".to_string()],
        });
        let settings_store = Arc::new(FakeSettingsStore::default());
        let settings_store_dyn: Arc<dyn SettingsStore> = settings_store.clone();
        let mut voice_setup = None;
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_all_sources(
                    settings_store_dyn.clone(),
                    capture_source.clone(),
                    import_source.clone(),
                    input_device_source.clone(),
                    true,
                    None,
                    window,
                    cx,
                )
            });
            voice_setup = Some(view.clone());
            Root::new(view, window, cx)
        });
        let voice_setup = voice_setup.unwrap();

        cx.update_window(handle.into(), |_, _window, cx| {
            let mic_device_select = voice_setup
                .read(cx)
                .mic_device_select
                .clone()
                .expect("enumerable devices must produce a device selector");

            mic_device_select.update(cx, |_, cx| {
                cx.emit(SelectEvent::Confirm(Some("Mic B".to_string())));
            });
        })
        .unwrap();
        // `cx.subscribe`'s callback runs as a deferred effect, not inline
        // with the `emit` above — flush it before asserting on its result.
        cx.run_until_parked();

        assert_eq!(
            settings_store.saved_mic_devices.lock().unwrap().as_slice(),
            [Some("Mic B".to_string())],
            "confirming a selection must persist it via SettingsStore"
        );
    }

    #[gpui_kit::test]
    fn a_failed_persist_surfaces_an_error(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let capture_source = Arc::new(FakeCaptureSource::new(FakeOutcome::Fail));
        let import_source = Arc::new(FakeImportSource::new(None));
        let input_device_source = Arc::new(FakeInputDeviceSource {
            devices: vec!["Mic A".to_string()],
        });
        let settings_store: Arc<dyn SettingsStore> =
            Arc::new(FakeSettingsStore::failing_mic_device_save());
        let mut voice_setup = None;
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|cx| {
                VoiceSetupView::new_with_all_sources(
                    settings_store.clone(),
                    capture_source.clone(),
                    import_source.clone(),
                    input_device_source.clone(),
                    true,
                    None,
                    window,
                    cx,
                )
            });
            voice_setup = Some(view.clone());
            Root::new(view, window, cx)
        });
        let voice_setup = voice_setup.unwrap();

        cx.update_window(handle.into(), |_, _window, cx| {
            let mic_device_select = voice_setup
                .read(cx)
                .mic_device_select
                .clone()
                .expect("enumerable devices must produce a device selector");

            mic_device_select.update(cx, |_, cx| {
                cx.emit(SelectEvent::Confirm(Some("Mic A".to_string())));
            });
        })
        .unwrap();
        // `cx.subscribe`'s callback runs as a deferred effect, not inline
        // with the `emit` above — flush it before asserting on its result.
        cx.run_until_parked();

        cx.update(|cx| {
            assert!(
                voice_setup.read(cx).error_message.is_some(),
                "a failed persist must surface an error"
            );
        });
    }
}
