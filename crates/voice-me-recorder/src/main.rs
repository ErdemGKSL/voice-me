//! `voice-me-recorder` — reads a Turkish sentence at you, saves what you say,
//! and exports a zip the Colab notebook trains a Piper voice from.
//!
//! ```text
//! voice-me-recorder [--dir <folder>] [--sentences <id|text csv>] [--import <zip>]
//! ```
//!
//! Keys: `Space` record / stop, `P` play, `←` `→` previous / next,
//! `Enter` next sentence without a recording, `Delete` remove the take.
//! Every take is written to disk the moment it stops, so closing the
//! window loses nothing; **Yedekle** writes a zip another PC can
//! **İçe aktar** to carry on from the same sentence.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IndexPath, Root,
    alert::Alert,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    select::{Select, SelectEvent, SelectState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, Focusable as _, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _,
    Window, WindowBounds, WindowOptions, div, px, size,
};
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink};
use voice_me_recorder::audio;
use voice_me_recorder::export;
use voice_me_recorder::sentences::{self, Sentence};
use voice_me_recorder::session::{self, Session};

/// A take is cut off here.
const MAX_TAKE: Duration = Duration::from_secs(30);

/// A live microphone capture.
struct ActiveCapture {
    _stream: cpal::Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    rate: u32,
    channels: u16,
    started: Instant,
}

/// Every input device by name, and the default one's name.
fn input_devices() -> (Vec<String>, Option<String>) {
    let host = cpal::default_host();
    let names = host
        .input_devices()
        .map(|devices| devices.map(|d| d.to_string()).collect())
        .unwrap_or_default();
    (names, host.default_input_device().map(|d| d.to_string()))
}

/// The input format with the highest sample rate (up to 48 kHz), preferring
/// float over 16-bit and mono or stereo: the OS default is often a
/// telephone-band 16 kHz when a Bluetooth headset is the default device.
fn best_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig, String> {
    let default = device
        .default_input_config()
        .map_err(|e| format!("Mikrofon ayarı okunamadı: {e}"))?;
    let best = device
        .supported_input_configs()
        .into_iter()
        .flatten()
        .filter(|range| range.channels() <= 2)
        .filter(|range| matches!(range.sample_format(), cpal::SampleFormat::F32 | cpal::SampleFormat::I16))
        .map(|range| {
            let rate = 48_000.clamp(range.min_sample_rate(), range.max_sample_rate());
            range.with_sample_rate(rate)
        })
        .max_by_key(|config| {
            (
                config.sample_rate(),
                config.sample_format() == cpal::SampleFormat::F32,
            )
        });
    Ok(match best {
        Some(best) if best.sample_rate() > default.sample_rate() => best,
        _ => default,
    })
}

impl ActiveCapture {
    /// `device`: a name from [`input_devices`]; the system default when
    /// `None` or no longer plugged in.
    fn start(device: Option<&str>) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = device
            .and_then(|name| host.input_devices().ok()?.find(|d| d.to_string() == name))
            .or_else(|| host.default_input_device())
            .ok_or("Mikrofon bulunamadı")?;
        let config = best_config(&device)?;
        let (format, rate, channels) = (config.sample_format(), config.sample_rate(), config.channels());
        let stream_config: cpal::StreamConfig = config.into();
        let samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let err_fn = |_err: cpal::Error| {};
        let push = |samples: &Arc<Mutex<Vec<f32>>>| {
            let samples = samples.clone();
            move |chunk: &mut dyn Iterator<Item = f32>| {
                if let Ok(mut buffer) = samples.lock() {
                    buffer.extend(chunk);
                }
            }
        };
        let stream = match format {
            cpal::SampleFormat::F32 => {
                let push = push(&samples);
                device.build_input_stream(
                    stream_config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| push(&mut data.iter().copied()),
                    err_fn,
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                let push = push(&samples);
                device.build_input_stream(
                    stream_config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        push(&mut data.iter().map(|s| *s as f32 / i16::MAX as f32))
                    },
                    err_fn,
                    None,
                )
            }
            cpal::SampleFormat::U16 => {
                let push = push(&samples);
                device.build_input_stream(
                    stream_config,
                    move |data: &[u16], _: &cpal::InputCallbackInfo| {
                        push(&mut data.iter().map(|s| (*s as f32 - 32768.0) / 32768.0))
                    },
                    err_fn,
                    None,
                )
            }
            other => return Err(format!("Desteklenmeyen mikrofon biçimi: {other:?}")),
        }
        .map_err(|e| format!("Mikrofon açılamadı: {e}"))?;
        stream.play().map_err(|e| format!("Mikrofon başlatılamadı: {e}"))?;
        Ok(Self { _stream: stream, samples, rate, channels, started: Instant::now() })
    }

    /// The loudest sample of roughly the last 60 ms.
    fn level(&self) -> f32 {
        let window = (self.rate as usize * usize::from(self.channels)) / 16;
        self.samples.lock().map_or(0.0, |buffer| {
            let from = buffer.len().saturating_sub(window);
            buffer[from..].iter().fold(0.0_f32, |peak, s| peak.max(s.abs()))
        })
    }

    fn finish(self) -> (Vec<f32>, u32, u16) {
        let samples = self.samples.lock().map(|b| b.clone()).unwrap_or_default();
        (samples, self.rate, self.channels)
    }
}

#[derive(Clone)]
enum Status {
    Info(SharedString),
    Success(SharedString),
    Error(SharedString),
}

struct RecorderView {
    sentences: Vec<Sentence>,
    session: Session,
    /// Seconds of each recorded sentence.
    seconds: HashMap<String, f32>,
    index: usize,
    capture: Option<ActiveCapture>,
    playback: Option<MixerDeviceSink>,
    auto_advance: bool,
    busy: bool,
    mic_select: Option<Entity<SelectState<Vec<String>>>>,
    mic: Option<String>,
    sync_speaker: bool,
    status: Status,
    speaker_input: Entity<InputState>,
    focus_handle: FocusHandle,
}

fn clip_seconds(path: &std::path::Path) -> Option<f32> {
    // 44-byte header, then 16-bit mono samples.
    let length = std::fs::metadata(path).ok()?.len();
    Some(length.saturating_sub(44) as f32 / 2.0 / audio::TARGET_RATE as f32)
}

fn format_duration(seconds: f32) -> String {
    let total = seconds.round() as u64;
    format!("{} sa {:02} dk {:02} sn", total / 3600, total % 3600 / 60, total % 60)
}

impl RecorderView {
    fn new(
        sentences: Vec<Sentence>,
        session: Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let seconds: HashMap<String, f32> = session
            .recorded()
            .into_iter()
            .filter_map(|id| Some((id.clone(), clip_seconds(&session.wav_path(&id))?)))
            .collect();
        let speaker = session.speaker();
        let speaker_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Konuşmacı adı")
                .default_value(speaker)
        });
        let (devices, default_device) = input_devices();
        let mic = session.mic().filter(|name| devices.contains(name)).or(default_device);
        let mic_select = if devices.is_empty() {
            None
        } else {
            let index = mic.as_ref().and_then(|name| devices.iter().position(|d| d == name)).map(IndexPath::new);
            let state = cx.new(|cx| SelectState::new(devices, index, window, cx));
            cx.subscribe(&state, |this, _state, event, cx| {
                let SelectEvent::Confirm(value) = event;
                this.mic = value.clone();
                this.status = match this.session.set_mic(value.as_deref()) {
                    Ok(()) => Status::Success(format!("Mikrofon: {}", value.as_deref().unwrap_or("varsayılan")).into()),
                    Err(error) => Status::Error(error.into()),
                };
                cx.notify();
            })
            .detach();
            Some(state)
        };
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let mut view = Self {
            sentences,
            session,
            seconds,
            index: 0,
            capture: None,
            playback: None,
            auto_advance: true,
            busy: false,
            mic_select,
            mic,
            sync_speaker: false,
            status: Status::Info("Space ile kaydı başlat, bitince tekrar Space.".into()),
            speaker_input,
            focus_handle,
        };
        view.index = view.next_unrecorded_from(0).unwrap_or(0);
        view
    }

    fn current(&self) -> Option<&Sentence> {
        self.sentences.get(self.index)
    }

    fn is_recorded(&self, id: &str) -> bool {
        self.seconds.contains_key(id)
    }

    fn next_unrecorded_from(&self, from: usize) -> Option<usize> {
        (from..self.sentences.len())
            .chain(0..from.min(self.sentences.len()))
            .find(|&i| !self.is_recorded(&self.sentences[i].id))
    }

    fn goto(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.capture.is_some() || self.sentences.is_empty() {
            return;
        }
        self.index = index.min(self.sentences.len() - 1);
        self.playback = None;
        cx.notify();
    }

    fn next(&mut self, cx: &mut Context<Self>) {
        self.goto(self.index + 1, cx);
    }

    fn previous(&mut self, cx: &mut Context<Self>) {
        self.goto(self.index.saturating_sub(1), cx);
    }

    fn next_open(&mut self, cx: &mut Context<Self>) {
        match self.next_unrecorded_from(self.index + 1) {
            Some(index) => self.goto(index, cx),
            None => {
                self.status = Status::Success("Tüm cümleler kaydedildi. Zip'i dışa aktarabilirsin.".into());
                cx.notify();
            }
        }
    }

    fn toggle_record(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if self.capture.is_some() {
            self.stop_record(cx);
        } else {
            self.start_record(cx);
        }
    }

    fn start_record(&mut self, cx: &mut Context<Self>) {
        if self.current().is_none() {
            return;
        }
        self.playback = None;
        match ActiveCapture::start(self.mic.as_deref()) {
            Ok(capture) => {
                self.status = if capture.rate < 44_100 {
                    Status::Error(format!("Mikrofon yalnızca {} Hz veriyor (Bluetooth kulaklık olabilir); kalite düşer. Kablolu/dahili mikrofon seç.", capture.rate).into())
                } else {
                    Status::Info("Kayıt yapılıyor… bitince Space.".into())
                };
                self.capture = Some(capture);
                cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(Duration::from_millis(60)).await;
                        let Ok(still) = this.update(cx, |view, cx| {
                            let over = view
                                .capture
                                .as_ref()
                                .is_some_and(|c| c.started.elapsed() > MAX_TAKE);
                            if over {
                                view.stop_record(cx);
                            }
                            cx.notify();
                            view.capture.is_some()
                        }) else {
                            break;
                        };
                        if !still {
                            break;
                        }
                    }
                })
                .detach();
            }
            Err(error) => self.status = Status::Error(error.into()),
        }
        cx.notify();
    }

    fn stop_record(&mut self, cx: &mut Context<Self>) {
        let Some(capture) = self.capture.take() else { return };
        let (raw, rate, channels) = capture.finish();
        let Some(sentence) = self.current().cloned() else { return };
        let result = audio::prepare(&raw, rate, channels).and_then(|(clip, quality)| {
            if quality.too_short() {
                return Err("Kayıt çok kısa, tekrar dene.".to_string());
            }
            if quality.too_quiet() {
                return Err("Ses çok kısık, mikrofona yaklaşıp tekrar dene.".to_string());
            }
            let wav = audio::wav_bytes(&clip)?;
            self.session.save(&sentence.id, &wav)?;
            Ok((quality, clip.len() as f32 / audio::TARGET_RATE as f32))
        });
        match result {
            Ok((quality, seconds)) => {
                self.seconds.insert(sentence.id.clone(), seconds);
                let snr = quality.snr_db.map_or(String::new(), |snr| format!(", SNR {snr:.0} dB"));
                self.status = if quality.clipped() {
                    Status::Error("Kaydedildi ama ses patlamış (clipping); mikrofon seviyesini düşürüp yeniden kaydet (Sil + Space).".into())
                } else if quality.noisy() {
                    Status::Error(format!("Kaydedildi ama ortam gürültülü{snr}; daha sessiz bir yerde yeniden kaydet.").into())
                } else {
                    Status::Success(format!("Kaydedildi ({seconds:.1} sn{snr}).").into())
                };
                if self.auto_advance && !quality.clipped() && !quality.noisy() {
                    self.next_open(cx);
                }
            }
            Err(error) => self.status = Status::Error(error.into()),
        }
        cx.notify();
    }

    fn play(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.current().map(|s| s.id.clone()) else { return };
        let Some(bytes) = self.session.load(&id) else {
            self.status = Status::Error("Bu cümle için kayıt yok.".into());
            cx.notify();
            return;
        };
        let played = DeviceSinkBuilder::open_default_sink()
            .map_err(|e| format!("Ses çıkışı açılamadı: {e}"))
            .and_then(|sink| {
                let decoder = Decoder::new(Cursor::new(bytes)).map_err(|e| format!("Kayıt çözülemedi: {e}"))?;
                sink.mixer().add(decoder);
                Ok(sink)
            });
        match played {
            Ok(sink) => self.playback = Some(sink),
            Err(error) => self.status = Status::Error(error.into()),
        }
        cx.notify();
    }

    fn delete_take(&mut self, cx: &mut Context<Self>) {
        if self.capture.is_some() {
            return;
        }
        if let Some(id) = self.current().map(|s| s.id.clone()) {
            self.playback = None;
            self.session.delete(&id);
            if self.seconds.remove(&id).is_some() {
                self.status = Status::Info("Kayıt silindi.".into());
            }
        }
        cx.notify();
    }

    fn save_speaker(&mut self, cx: &mut Context<Self>) {
        let name = self.speaker_input.read(cx).value().trim().to_string();
        self.status = match self.session.set_speaker(&name) {
            Ok(()) => Status::Success(format!("Konuşmacı: {}", self.session.speaker()).into()),
            Err(error) => Status::Error(error.into()),
        };
        cx.notify();
    }

    fn export_dir(&self) -> PathBuf {
        self.session.dir().join("exports")
    }

    /// `backup`: the resumable zip instead of the training zip.
    fn export(&mut self, backup: bool, cx: &mut Context<Self>) {
        if self.busy || self.capture.is_some() {
            return;
        }
        self.save_speaker(cx);
        self.busy = true;
        self.status = Status::Info("Zip hazırlanıyor…".into());
        let next = self.current().map(|s| s.id.clone()).unwrap_or_default();
        let (session, sentences, dir) =
            (self.session.clone(), self.sentences.clone(), self.export_dir());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if backup {
                        export::export_backup(&session, &sentences, &dir, &next)
                    } else {
                        export::export_zip(&session, &sentences, &dir)
                    }
                })
                .await;
            this.update(cx, |view, cx| {
                view.busy = false;
                view.status = match result {
                    Ok(summary) => {
                        if let Some(parent) = summary.path.parent() {
                            cx.reveal_path(parent);
                        }
                        Status::Success(
                            format!(
                                "{} cümle, {} → {}{}",
                                summary.count,
                                format_duration(summary.seconds as f32),
                                summary.path.display(),
                                if summary.skipped > 0 {
                                    format!(" ({} uygunsuz kayıt atlandı)", summary.skipped)
                                } else {
                                    String::new()
                                }
                            )
                            .into(),
                        )
                    }
                    Err(error) => Status::Error(error.into()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn pick_import(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.capture.is_some() {
            return;
        }
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("İçe aktar".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = picked.await
                && let Some(path) = paths.pop()
            {
                this.update(cx, |view, cx| view.import(path, cx)).ok();
            }
        })
        .detach();
    }

    fn import(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.busy = true;
        self.status = Status::Info("İçe aktarılıyor…".into());
        let session = self.session.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { export::import_zip(&session, &path) })
                .await;
            this.update(cx, |view, cx| {
                view.busy = false;
                match result {
                    Ok(imported) => {
                        view.seconds = view
                            .session
                            .recorded()
                            .into_iter()
                            .filter_map(|id| Some((id.clone(), clip_seconds(&view.session.wav_path(&id))?)))
                            .collect();
                        view.sync_speaker = true;
                        let resume_at = imported
                            .next_id
                            .as_deref()
                            .and_then(|id| view.sentences.iter().position(|s| s.id == id));
                        if let Some(next) = resume_at {
                            view.index = next;
                        } else if let Some(open) = view.next_unrecorded_from(0) {
                            view.index = open;
                        }
                        view.status = Status::Success(
                            format!(
                                "{} kayıt eklendi, {} zaten vardı, {} atlandı.",
                                imported.added, imported.kept, imported.skipped
                            )
                            .into(),
                        );
                    }
                    Err(error) => view.status = Status::Error(error.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

impl Render for RecorderView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.sync_speaker) {
            let speaker = self.session.speaker();
            self.speaker_input
                .update(cx, |state, cx| state.set_value(speaker, window, cx));
        }
        let recording = self.capture.is_some();
        let total = self.sentences.len();
        let done = self.seconds.len();
        let recorded_seconds: f32 = self.seconds.values().sum();
        let current = self.current().cloned();
        let has_take = current.as_ref().is_some_and(|s| self.is_recorded(&s.id));
        let level = self.capture.as_ref().map_or(0.0, ActiveCapture::level).min(1.0);
        let busy = self.busy;
        let fraction = if total == 0 { 0.0 } else { done as f32 / total as f32 };
        let theme = cx.theme().clone();

        let status = match self.status.clone() {
            Status::Info(text) => Alert::info("status", text),
            Status::Success(text) => Alert::success("status", text),
            Status::Error(text) => Alert::error("status", text),
        };

        v_flex()
            .id("recorder")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                // Typing a speaker name must not trigger the shortcuts.
                if this.speaker_input.read(cx).focus_handle(cx).is_focused(window) {
                    if matches!(event.keystroke.key.as_str(), "enter" | "escape") {
                        this.save_speaker(cx);
                        window.focus(&this.focus_handle, cx);
                    }
                    return;
                }
                match event.keystroke.key.as_str() {
                    "space" => this.toggle_record(cx),
                    "p" => this.play(cx),
                    "right" => this.next(cx),
                    "left" => this.previous(cx),
                    "enter" => this.next_open(cx),
                    "delete" => this.delete_take(cx),
                    _ => {}
                }
            }))
            .size_full()
            .p_6()
            .gap_4()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(div().text_lg().child("Ses kaydedici"))
                    .child(div().w(px(220.)).child(Input::new(&self.speaker_input)))
                    .child(Button::new("save-speaker").label("Adı kaydet").on_click(
                        cx.listener(|this, _, _, cx| this.save_speaker(cx)),
                    )),
            )
            .when_some(self.mic_select.clone(), |el, select| {
                el.child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child("Mikrofon:")
                        .child(div().w(px(420.)).child(Select::new(&select).placeholder("Sistem varsayılanı").disabled(recording))),
                )
            })
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_color(theme.muted_foreground).child(format!(
                        "{done} / {total} cümle kaydedildi · {}",
                        format_duration(recorded_seconds)
                    )))
                    .child(
                        div().h(px(6.)).w_full().rounded_full().bg(theme.muted).child(
                            div().h_full().w(gpui_kit::relative(fraction)).rounded_full().bg(theme.primary),
                        ),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .justify_center()
                    .gap_3()
                    .p_6()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .child(div().text_color(theme.muted_foreground).child(format!(
                        "Cümle {} / {}  ·  {}",
                        self.index + 1,
                        total,
                        if has_take { "kaydedildi" } else { "henüz kayıt yok" }
                    )))
                    .child(div().text_2xl().child(
                        current.map_or_else(|| "Cümle listesi boş.".to_string(), |s| s.text),
                    )),
            )
            .when(recording, |el| {
                el.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(div().size(px(10.)).rounded_full().bg(theme.danger))
                        .child("Kayıt yapılıyor")
                        .child(
                            div().h(px(8.)).w(px(240.)).rounded_full().bg(theme.muted).child(
                                div().h_full().w(gpui_kit::relative((level * 3.0).min(1.0))).rounded_full().bg(theme.success),
                            ),
                        ),
                )
            })
            .child(status)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("prev")
                            .label("← Önceki")
                            .disabled(recording)
                            .on_click(cx.listener(|this, _, _, cx| this.previous(cx))),
                    )
                    .child(
                        Button::new("record")
                            .primary()
                            .label(if recording { "Durdur (Space)" } else { "Kaydet (Space)" })
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_record(cx))),
                    )
                    .child(
                        Button::new("play")
                            .label("Dinle (P)")
                            .disabled(recording || !has_take)
                            .on_click(cx.listener(|this, _, _, cx| this.play(cx))),
                    )
                    .child(
                        Button::new("delete")
                            .label("Sil (Del)")
                            .disabled(recording || !has_take)
                            .on_click(cx.listener(|this, _, _, cx| this.delete_take(cx))),
                    )
                    .child(
                        Button::new("next")
                            .label("Sonraki →")
                            .disabled(recording)
                            .on_click(cx.listener(|this, _, _, cx| this.next(cx))),
                    )
                    .child(
                        Button::new("next-open")
                            .label("Sıradaki boş (Enter)")
                            .disabled(recording)
                            .on_click(cx.listener(|this, _, _, cx| this.next_open(cx))),
                    )
                    .child(
                        Button::new("auto")
                            .label(if self.auto_advance { "Otomatik ilerle: açık" } else { "Otomatik ilerle: kapalı" })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.auto_advance = !this.auto_advance;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("export")
                            .primary()
                            .label("Eğitim zip'i oluştur")
                            .disabled(busy || recording || done == 0)
                            .on_click(cx.listener(|this, _, _, cx| this.export(false, cx))),
                    )
                    .child(
                        Button::new("backup")
                            .label("Yedekle (devam için)")
                            .disabled(busy || recording || done == 0)
                            .on_click(cx.listener(|this, _, _, cx| this.export(true, cx))),
                    )
                    .child(
                        Button::new("import")
                            .label("İçe aktar")
                            .disabled(busy || recording)
                            .on_click(cx.listener(|this, _, _, cx| this.pick_import(cx))),
                    ),
            )
            .child(div().text_sm().text_color(theme.muted_foreground).child(format!(
                "Kayıt klasörü: {}",
                self.session.dir().display()
            )))
    }
}

fn main() {
    let mut dir = session::default_dir();
    let mut sentences_path: Option<PathBuf> = None;
    let mut import_path: Option<PathBuf> = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--dir") => dir = args.next().map(PathBuf::from).unwrap_or(dir),
            Some("--sentences") => sentences_path = args.next().map(PathBuf::from),
            Some("--import") => import_path = args.next().map(PathBuf::from),
            _ => {}
        }
    }

    let sentences = match sentences_path {
        Some(path) => {
            let csv = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                eprintln!("could not read {}: {e}", path.display());
                std::process::exit(2);
            });
            sentences::order_for_coverage(sentences::parse(&csv))
        }
        None => sentences::bundled(),
    };
    let session = Session::open(&dir).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2);
    });
    if let Some(path) = import_path {
        match export::import_zip(&session, &path) {
            Ok(imported) => eprintln!("imported: {imported:?}"),
            Err(error) => eprintln!("import failed: {error}"),
        }
    }

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            let options = WindowOptions {
                app_id: Some("voice-me-recorder".into()),
                window_bounds: Some(WindowBounds::centered(size(px(1040.), px(680.)), cx)),
                ..Default::default()
            };
            cx.open_window(options, move |window, cx| {
                let view = cx.new(|cx| RecorderView::new(sentences, session, window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("failed to open the recorder window");
            cx.activate(true);
        });
}
