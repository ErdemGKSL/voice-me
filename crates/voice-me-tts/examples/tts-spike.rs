//! Manual verification for spec-2-5: generates one utterance with
//! Chatterbox-Multilingual running **in-process** on ONNX Runtime, times
//! each stage, and writes a wav to listen to.
//!
//! It runs inside `gpui_kit::application().run(..)` on purpose. A plain
//! `fn main` would prove the engine works but nothing about AD-5 — the
//! whole point is that generation is dispatched from GPUI's main thread
//! onto Tokio's blocking pool, awaited from `cx.background_spawn`, and the
//! result comes back without the UI thread ever blocking.
//!
//! Named `tts-spike` rather than `spike` because cargo writes every example
//! in the workspace into one shared `target/debug/examples/` directory, so
//! two examples called `spike` would overwrite each other's binary (see
//! `hotkey-spike.rs`'s own note).
//!
//! ```bash
//! export ORT_DYLIB_PATH=~/.cache/voice-me/onnxruntime/onnxruntime-linux-x64-1.28.2/lib/libonnxruntime.so
//! cargo run --release -p voice-me-tts --example tts-spike -- tr "Merhaba, bugün nasılsın?"
//! cargo run --release -p voice-me-tts --example tts-spike -- en "Hey, I'll be right back." --variant fp32
//! ```
//!
//! Arguments: `<language> <text> [--variant q4|fp16|fp32] [--ep cpu|webgpu:N]
//! [--reference PATH] [--out PATH] [--verbose-placement]`.

use std::path::PathBuf;

use gpui_kit::{App, QuitMode};
use voice_me_core::{AudioBuffer, VoiceMeError, tokio_bridge};
use voice_me_tts::generate::{GenerationOutcome, GenerationSettings};
use voice_me_tts::sessions::{ExecutionTarget, LanguageModel, ModelCache, Sessions, init_runtime};

struct Args {
    language: String,
    text: String,
    variant: LanguageModel,
    target: ExecutionTarget,
    reference: Option<PathBuf>,
    out: Option<PathBuf>,
    verbose_placement: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut positional: Vec<String> = Vec::new();
    let mut variant = LanguageModel::Q4;
    let mut target = ExecutionTarget::Cpu;
    let mut reference = None;
    let mut out = None;
    let mut verbose_placement = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--variant" => {
                let value = args.next().ok_or("--variant needs a value")?;
                variant = LanguageModel::parse(&value)
                    .ok_or_else(|| format!("unknown variant {value}; try q4, fp16 or fp32"))?;
            }
            "--ep" => {
                let value = args.next().ok_or("--ep needs a value")?;
                target = match value.as_str() {
                    "cpu" => ExecutionTarget::Cpu,
                    other => match other.strip_prefix("webgpu:") {
                        Some(id) => ExecutionTarget::WebGpu {
                            device_id: id
                                .parse()
                                .map_err(|_| format!("bad device id in {other}"))?,
                        },
                        None => return Err(format!("unknown execution provider {other}")),
                    },
                };
            }
            "--reference" => {
                reference = Some(PathBuf::from(
                    args.next().ok_or("--reference needs a path")?,
                ))
            }
            "--out" => out = Some(PathBuf::from(args.next().ok_or("--out needs a path")?)),
            "--verbose-placement" => verbose_placement = true,
            other => positional.push(other.to_string()),
        }
    }

    if positional.len() < 2 {
        return Err(
            "usage: tts-spike <language> <text> [--variant q4|fp16|fp32] \
                    [--ep cpu|webgpu:N] [--reference PATH] [--out PATH] [--verbose-placement]"
                .to_string(),
        );
    }

    Ok(Args {
        language: positional[0].clone(),
        text: positional[1].clone(),
        variant,
        target,
        reference,
        out,
        verbose_placement,
    })
}

/// The Reference Voice Sample the app itself would use, per AD-6.
fn default_reference() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/share/voice-me/reference_voice_sample.wav"))
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };

    // Tray-resident quit semantics do not apply here, but this example has
    // no window at all, so the app must be told to keep running until the
    // work is done and it quits itself.
    gpui_kit::application()
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx: &mut App| {
            // AD-5: one Tokio runtime alongside GPUI, installed as a global
            // by the composition root — here, this example.
            if let Err(error) = tokio_bridge::TokioRuntime::install(cx) {
                eprintln!("could not start the Tokio runtime: {error}");
                cx.quit();
                return;
            }

            // Everything below this point runs on GPUI's main thread only
            // to *dispatch*. The inference itself is on the blocking pool.
            let work = tokio_bridge::spawn_blocking(cx, move || run(args));

            cx.spawn(async move |cx| {
                let outcome = work.await;
                cx.update(|cx| {
                    match outcome {
                        Ok(report) => println!("{report}"),
                        Err(error) => eprintln!("\nfailed: {error}"),
                    }
                    cx.quit();
                });
            })
            .detach();

            println!("dispatched generation onto the Tokio blocking pool; GPUI is not blocked.");
        });
}

/// The blocking half: everything from here down runs off the main thread.
fn run(args: Args) -> Result<String, VoiceMeError> {
    let cache = ModelCache::from_env()?;
    let reference_path = args
        .reference
        .or_else(default_reference)
        .ok_or_else(|| VoiceMeError::Other("no reference clip and no HOME".to_string()))?;

    println!("model cache:   {}", cache.root().display());
    println!("reference:     {}", reference_path.display());
    println!("variant:       {:?}", args.variant);
    println!("provider:      {}", args.target.label());

    // Report the whole provisioning list up front on the first failure —
    // that list is Story 3.2's input, so it has to be precise.
    if let Err(error @ VoiceMeError::MissingRuntimeAsset { .. }) = cache.check(args.variant) {
        eprintln!("\n{error}\n\nthe full file list this run needs:");
        for path in cache.required_files(args.variant) {
            let mark = if path.exists() { "ok     " } else { "MISSING" };
            eprintln!("  {mark} {}", path.display());
        }
        return Err(error);
    }

    init_runtime(None)?;

    let decode_started = std::time::Instant::now();
    let reference = voice_me_tts::reference::load_reference_clip(&reference_path)?;
    let decode = decode_started.elapsed();
    println!(
        "reference decoded to {:.2} s @ {} Hz mono in {:?}",
        reference.duration().as_secs_f64(),
        reference.sample_rate(),
        decode
    );

    let build_started = std::time::Instant::now();
    let mut sessions = Sessions::build(&cache, args.variant, args.target, args.verbose_placement)?;
    let build = build_started.elapsed();
    println!("sessions built in {build:?}");

    let outcome = voice_me_tts::generate::generate(
        &mut sessions,
        &args.text,
        &args.language,
        &reference,
        GenerationSettings::default(),
    )?;

    let out = args.out.unwrap_or_else(|| {
        std::env::temp_dir().join(format!(
            "voice-me-{}-{}-{}.wav",
            args.language,
            format!("{:?}", args.variant).to_lowercase(),
            args.target.label().replace(':', "-")
        ))
    });
    write_wav(&out, &outcome.audio)?;

    Ok(report(&outcome, build, &out))
}

fn report(
    outcome: &GenerationOutcome,
    build: std::time::Duration,
    out: &std::path::Path,
) -> String {
    format!(
        "\n--- spec-2-5 measurement ---\n\
         session build:    {:>9.2?}   (once, held per AD-10)\n\
         speech_encoder:   {:>9.2?}\n\
         embed_tokens:     {:>9.2?}\n\
         language_model:   {:>9.2?}   {} tokens, {:.1} ms/token{}\n\
         cond_decoder:     {:>9.2?}\n\
         utterance total:  {:>9.2?}\n\
         audio:            {:>9.2?}   ({} samples, realtime factor {:.2}x)\n\
         wav:              {}\n",
        build,
        outcome.encode,
        outcome.embed,
        outcome.decode_loop,
        outcome.tokens,
        outcome.ms_per_token(),
        if outcome.stopped_on_token {
            ""
        } else {
            " — hit max_new_tokens without a stop token"
        },
        outcome.vocode,
        outcome.total,
        outcome.audio.duration(),
        outcome.audio.len(),
        outcome.realtime_factor(),
        out.display()
    )
}

/// 16-bit PCM, because that is what every audio player opens without
/// comment. The engine's own buffer stays f32 (AD-11) — this conversion
/// exists only so a human can listen to the result.
fn write_wav(path: &std::path::Path, audio: &AudioBuffer) -> Result<(), VoiceMeError> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: audio.sample_rate(),
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).map_err(|error| {
        VoiceMeError::Other(format!("could not write {}: {error}", path.display()))
    })?;
    for &sample in audio.samples() {
        let clamped = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        writer
            .write_sample(clamped)
            .map_err(|error| VoiceMeError::Other(format!("could not write a sample: {error}")))?;
    }
    writer
        .finalize()
        .map_err(|error| VoiceMeError::Other(format!("could not finalize the wav: {error}")))?;
    Ok(())
}
