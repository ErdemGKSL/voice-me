//! `voice-me-tts-piper` — Piper voices, natural and instant (Story 3.15).
//!
//! A [`TtsPort`] over a Piper VITS voice: one ONNX graph run in-process on
//! the existing `ort`, CPU execution provider only. No Piper engine code and
//! no libespeak-ng is linked. The pipeline, fixture-verified against Piper
//! 1.8.0:
//!
//! 1. the text is split into clauses at `, : ; . ! ?`
//!    ([`phonemes::split_clauses`]);
//! 2. each clause is phonemized by `espeak-ng --ipa` (through
//!    `voice-me-espeak` on Linux and Windows — the [`Phonemizer`] here), its mark put
//!    back, and the result decomposed to NFD ([`phonemes::sentence_phonemes`]);
//! 3. each sentence becomes ids ([`phonemes::to_ids`]) and one run of the
//!    graph;
//! 4. each sentence's audio is peak-normalized and clipped, the sentences
//!    are joined, and the whole is resampled to AD-11's 24 kHz.
//!
//! One session is held per selected voice (AD-10) and rebuilt when the voice
//! changes. The ONNX Runtime library is committed through an injected
//! closure (AD-1: this crate does not depend on `voice-me-tts`, which owns
//! the once-per-process guard). It never downloads a runtime or a voice, and
//! opens no socket (AD-8).

pub mod config;
pub mod phonemes;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use ort::ep::ExecutionProviderDispatch;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::TensorRef;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use voice_me_core::{AudioBuffer, SAMPLE_RATE, TtsPort, VoiceMeError, assets};

pub use config::PiperConfig;

/// The engine's name, as every failure names it.
pub const ENGINE_LABEL: &str = "Piper";

/// Below this peak a sentence is silence, not something to amplify.
const SILENCE_PEAK: f32 = 1e-8;

/// The resampler's fixed input chunk, in frames.
const RESAMPLE_CHUNK: usize = 1024;

/// Asks for one clause's IPA phonemes in an `espeak-ng` voice, returning
/// what the program printed — or why not, in words.
pub trait Phonemizer: Send + Sync {
    fn phonemize(&self, espeak_voice: &str, clause: &str) -> Result<String, String>;
}

/// The real phonemizer: the `espeak-ng` program, through `voice-me-espeak`.
///
/// Story 3.16: by default the program is resolved on each call
/// (`voice_me_espeak::find_program()`, falling back to the bare name), so
/// an eSpeak NG installed after the engine was built — the Dependency
/// Check's Install on Windows — is picked up without a rebuild.
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[derive(Debug, Clone, Default)]
pub struct EspeakPhonemizer {
    /// A fixed program, or `None` to resolve it on each call.
    program: Option<std::ffi::OsString>,
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl EspeakPhonemizer {
    /// `espeak-ng` wherever `voice-me-espeak` finds it, on each call.
    pub fn new() -> Self {
        Self { program: None }
    }

    /// Another program in its place.
    pub fn with_program(program: impl Into<std::ffi::OsString>) -> Self {
        Self {
            program: Some(program.into()),
        }
    }

    /// The program this call runs.
    fn program(&self) -> std::ffi::OsString {
        self.program.clone().unwrap_or_else(|| {
            voice_me_espeak::find_program()
                .map(std::path::PathBuf::into_os_string)
                .unwrap_or_else(|| voice_me_espeak::PROGRAM.into())
        })
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl Phonemizer for EspeakPhonemizer {
    fn phonemize(&self, espeak_voice: &str, clause: &str) -> Result<String, String> {
        voice_me_espeak::ipa(&self.program(), espeak_voice, clause)
            .map_err(|error| format!("{} {error}", voice_me_espeak::PROGRAM))
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "windows")))]
mod espeak_phonemizer_tests {
    use super::*;

    /// Story 3.16: by default the program is resolved on each call, where
    /// `voice-me-espeak` finds it; a fixed one is used as given.
    #[test]
    fn the_default_program_is_resolved_per_call_and_a_fixed_one_is_kept() {
        assert_eq!(
            EspeakPhonemizer::new().program(),
            voice_me_espeak::find_program()
                .map(PathBuf::into_os_string)
                .unwrap_or_else(|| voice_me_espeak::PROGRAM.into())
        );
        assert_eq!(EspeakPhonemizer::with_program("x").program(), "x");
    }
}

/// What commits the ONNX Runtime library: the composition root passes the
/// one `voice-me-tts` guards, so the process still commits exactly one.
pub type RuntimeInit = Arc<dyn Fn() -> Result<(), VoiceMeError> + Send + Sync>;

/// The execution providers a voice's session is built with, and the notes
/// to add to a failed build — say, NVIDIA libraries that did not load
/// before the CUDA provider was registered.
pub struct SessionProviders {
    pub providers: Vec<ExecutionProviderDispatch>,
    pub failure_notes: Vec<String>,
}

impl SessionProviders {
    /// The CPU execution provider: Piper's default.
    pub fn cpu() -> Self {
        Self {
            providers: vec![ort::ep::CPU::default().build()],
            failure_notes: Vec::new(),
        }
    }
}

/// What picks a session's providers (spec-backend-engine-and-device-selects).
/// Called after [`RuntimeInit`], before each session build. AD-1: the
/// composition root injects `voice-me-tts`'s providers and its CUDA library
/// preload; this crate never depends on it.
pub type ProvidersInit = Arc<dyn Fn() -> Result<SessionProviders, VoiceMeError> + Send + Sync>;

/// A failure, naming Piper and the reason.
fn failure(reason: impl std::fmt::Display) -> VoiceMeError {
    VoiceMeError::SpeechEngine(format!("{ENGINE_LABEL} {reason}"))
}

/// The graph's inputs for one sentence.
#[derive(Debug, Clone, PartialEq)]
pub struct PiperInputs {
    /// `input`, shaped `[1, n]`.
    pub input: Vec<i64>,
    /// `input_lengths`, shaped `[1]`: `n`.
    pub input_lengths: [i64; 1],
    /// `scales`: `[noise_scale, length_scale, noise_w]`.
    pub scales: [f32; 3],
    /// `sid`, only for a multi-speaker voice.
    pub sid: Option<[i64; 1]>,
}

impl PiperInputs {
    pub fn new(ids: Vec<i64>, config: &PiperConfig) -> Self {
        Self {
            input_lengths: [ids.len() as i64],
            input: ids,
            scales: config.scales(),
            sid: config.speaker_id().map(|sid| [sid]),
        }
    }
}

/// Peak-normalize one sentence in place and clip it to ±1. A sentence whose
/// peak is below 1e-8 becomes all zeros.
pub fn normalize(samples: &mut [f32]) {
    let peak = samples
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    if peak < SILENCE_PEAK || !peak.is_finite() {
        samples.fill(0.0);
        return;
    }
    for sample in samples {
        *sample = (*sample / peak).clamp(-1.0, 1.0);
    }
}

/// Convert mono `samples` at `rate` to exactly [`SAMPLE_RATE`]. Copied
/// from `voice-me-tts-system-linux`'s `wav.rs`.
pub fn resample(samples: Vec<f32>, rate: u32) -> Result<Vec<f32>, String> {
    if rate == SAMPLE_RATE || samples.is_empty() {
        return Ok(samples);
    }
    let frames = samples.len();
    let mut resampler = Fft::<f32>::new(
        rate as usize,
        SAMPLE_RATE as usize,
        RESAMPLE_CHUNK,
        1,
        FixedSync::Input,
    )
    .map_err(|error| format!("{rate} Hz cannot be converted to {SAMPLE_RATE} Hz: {error}"))?;
    let input = InterleavedSlice::new(&samples, 1, frames)
        .map_err(|error| format!("its audio could not be wrapped: {error}"))?;
    let output = resampler
        .process_all(&input, frames, None)
        .map_err(|error| format!("resampling failed: {error}"))?;
    Ok(output.take_data())
}

/// The whole pipeline over one text, with the graph run abstracted as
/// `run` — what the tests drive with no model at all.
pub fn synthesize(
    text: &str,
    config: &PiperConfig,
    phonemizer: &dyn Phonemizer,
    mut run: impl FnMut(&PiperInputs) -> Result<Vec<f32>, VoiceMeError>,
) -> Result<AudioBuffer, VoiceMeError> {
    let clauses = phonemes::split_clauses(text);
    let mut ipa = Vec::with_capacity(clauses.len());
    for clause in &clauses {
        let printed = phonemizer
            .phonemize(&config.espeak.voice, &clause.text)
            .map_err(|reason| failure(format!("could not phonemize the text: {reason}")))?;
        ipa.push(printed);
    }

    let mut audio = Vec::new();
    for sentence in phonemes::sentence_phonemes(&clauses, &ipa) {
        let ids = phonemes::to_ids(&sentence, &config.phoneme_id_map);
        let mut samples = run(&PiperInputs::new(ids, config))?;
        normalize(&mut samples);
        audio.extend(samples);
    }
    if audio.is_empty() {
        return Err(failure("had nothing it could say for this text"));
    }
    let mut audio = resample(audio, config.audio.sample_rate).map_err(failure)?;
    // The resampler's ringing can overshoot a full-scale peak a little; the
    // buffer stays within ±1 as each sentence was.
    for sample in &mut audio {
        *sample = sample.clamp(-1.0, 1.0);
    }
    Ok(AudioBuffer::new(audio))
}

/// One loaded voice: its session and its config.
struct LoadedVoice {
    key: String,
    session: Session,
    config: PiperConfig,
}

/// The Piper [`TtsPort`] adapter.
pub struct PiperTts {
    root: PathBuf,
    /// The voice warm-up builds, when one is selected.
    warm_voice: Option<String>,
    phonemizer: Arc<dyn Phonemizer>,
    runtime_init: RuntimeInit,
    /// The providers each session is built with: CPU unless the root says
    /// otherwise.
    providers_init: ProvidersInit,
    /// The held session, keyed by voice. The mutex is the queue (AD-10):
    /// one generation at a time, the second waits.
    held: Mutex<Option<LoadedVoice>>,
    /// Mirrors "a session is held", readable without the lock.
    ready: AtomicBool,
}

impl PiperTts {
    /// The adapter over the voices under `root` (the cache root). `warm_voice`
    /// is the voice warm-up loads; `runtime_init` commits ONNX Runtime.
    /// Sessions run on the CPU unless [`Self::with_providers`] says
    /// otherwise.
    pub fn new(
        root: PathBuf,
        warm_voice: Option<String>,
        phonemizer: Arc<dyn Phonemizer>,
        runtime_init: RuntimeInit,
    ) -> Self {
        Self {
            root,
            warm_voice,
            phonemizer,
            runtime_init,
            providers_init: Arc::new(|| Ok(SessionProviders::cpu())),
            held: Mutex::new(None),
            ready: AtomicBool::new(false),
        }
    }

    /// Build each session with the providers `providers_init` returns — a
    /// GPU one for Piper on CUDA or WebGPU.
    pub fn with_providers(mut self, providers_init: ProvidersInit) -> Self {
        self.providers_init = providers_init;
        self
    }

    fn lock(&self) -> MutexGuard<'_, Option<LoadedVoice>> {
        self.held
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// The voice's two files, both present — or the first missing one.
    fn voice_files(&self, key: &str) -> Result<assets::PiperVoiceFiles, VoiceMeError> {
        let files = assets::piper_voice_files(&self.root, key)
            .ok_or_else(|| failure(format!("can't use the voice {key:?}: not a voice name")))?;
        for path in [&files.model, &files.config] {
            if !path.exists() {
                return Err(VoiceMeError::MissingRuntimeAsset { path: path.clone() });
            }
        }
        Ok(files)
    }

    /// Make `key` the held voice, building its session unless it already
    /// is — the one rebuild a voice change costs.
    fn ensure_voice<'a>(
        &self,
        held: &'a mut Option<LoadedVoice>,
        key: &str,
    ) -> Result<&'a mut LoadedVoice, VoiceMeError> {
        let files = self.voice_files(key)?;
        if held.as_ref().is_none_or(|loaded| loaded.key != key) {
            *held = None;
            self.ready.store(false, Ordering::SeqCst);
            (self.runtime_init)()?;
            let config = std::fs::read_to_string(&files.config)
                .map_err(|error| {
                    failure(format!(
                        "could not read {}: {error}",
                        files.config.display()
                    ))
                })
                .and_then(|json| {
                    PiperConfig::from_json(&json).map_err(|reason| {
                        failure(format!(
                            "could not read {}: {reason}",
                            files.config.display()
                        ))
                    })
                })?;
            let providers = (self.providers_init)()?;
            let session = build_session(&files.model, &providers.providers)
                .map_err(|error| with_notes(error, &providers.failure_notes))?;
            *held = Some(LoadedVoice {
                key: key.to_string(),
                session,
                config,
            });
            self.ready.store(true, Ordering::SeqCst);
        }
        Ok(held.as_mut().expect("just loaded"))
    }
}

/// A failed session build's error, with `notes` added after it.
fn with_notes(error: VoiceMeError, notes: &[String]) -> VoiceMeError {
    match error {
        VoiceMeError::SpeechEngine(reason) if !notes.is_empty() => {
            VoiceMeError::SpeechEngine(format!("{reason} ({})", notes.join("; ")))
        }
        error => error,
    }
}

/// One session on `providers`, from the model's path.
fn build_session(
    model: &Path,
    providers: &[ExecutionProviderDispatch],
) -> Result<Session, VoiceMeError> {
    let engine =
        |error: ort::Error| failure(format!("could not load {}: {error}", model.display()));
    Session::builder()
        .map_err(engine)?
        .with_execution_providers(providers)
        .map_err(|error| engine(error.into()))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|error| engine(error.into()))?
        .with_intra_threads(4)
        .map_err(|error| engine(error.into()))?
        .commit_from_file(model)
        .map_err(engine)
}

/// One run of the graph for one sentence.
fn run_graph(session: &mut Session, inputs: &PiperInputs) -> Result<Vec<f32>, VoiceMeError> {
    let engine = |what: &str, error: ort::Error| failure(format!("failed at {what}: {error}"));
    let input = TensorRef::from_array_view((
        vec![1_i64, inputs.input.len() as i64],
        inputs.input.as_slice(),
    ))
    .map_err(|error| engine("input", error))?;
    let lengths = TensorRef::from_array_view((vec![1_i64], &inputs.input_lengths[..]))
        .map_err(|error| engine("input_lengths", error))?;
    let scales = TensorRef::from_array_view((vec![3_i64], &inputs.scales[..]))
        .map_err(|error| engine("scales", error))?;
    let mut values = ort::inputs![
        "input" => input,
        "input_lengths" => lengths,
        "scales" => scales,
    ];
    if let Some(sid) = inputs.sid.as_ref() {
        let sid = TensorRef::from_array_view((vec![1_i64], &sid[..]))
            .map_err(|error| engine("sid", error))?;
        values.push((std::borrow::Cow::from("sid"), sid.into()));
    }
    let outputs = session
        .run(values)
        .map_err(|error| engine("the voice's graph", error))?;
    let (_, audio) = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|error| engine("its output", error))?;
    Ok(audio.to_vec())
}

impl TtsPort for PiperTts {
    /// Build the selected voice's session ahead of the first line. Needs no
    /// Reference Voice Sample; with no voice selected there is nothing to
    /// build.
    fn warm_up(&self) -> Result<(), VoiceMeError> {
        let Some(key) = self.warm_voice.as_deref() else {
            return Ok(());
        };
        let mut held = self.lock();
        self.ensure_voice(&mut held, key).map(|_| ())
    }

    fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    /// Speak `text` in the Piper `voice`. A stock voice: `reference_clip`
    /// is never read, and `language` is implied by the voice.
    fn generate(
        &self,
        text: &str,
        _reference_clip: Option<&Path>,
        _language: &str,
        voice: Option<&str>,
    ) -> Result<AudioBuffer, VoiceMeError> {
        if text.trim().is_empty() {
            return Err(VoiceMeError::EmptyText);
        }
        let Some(key) = voice else {
            return Err(failure("was given no voice to speak in"));
        };
        let mut held = self.lock();
        let loaded = self.ensure_voice(&mut held, key)?;
        let LoadedVoice {
            session, config, ..
        } = loaded;
        synthesize(text, config, self.phonemizer.as_ref(), |inputs| {
            run_graph(session, inputs)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;

    const FAHRETTIN: &str = include_str!("../tests/fixtures/tr_TR-fahrettin-medium.config.json");

    /// A phonemizer that answers from a table, and records what it was
    /// asked.
    struct TablePhonemizer {
        asked: StdMutex<Vec<(String, String)>>,
    }

    impl Phonemizer for TablePhonemizer {
        fn phonemize(&self, voice: &str, clause: &str) -> Result<String, String> {
            self.asked
                .lock()
                .unwrap()
                .push((voice.to_string(), clause.to_string()));
            Ok(match clause {
                "Merhaba Erdem" => "mˈɛrhaba ɛrdˈæm\n".to_string(),
                "bu yerel ve anında" => "bʊ jeɾˈæl vɛ anɯndˈa\n".to_string(),
                other => format!("{}\n", other.to_lowercase()),
            })
        }
    }

    fn table() -> TablePhonemizer {
        TablePhonemizer {
            asked: StdMutex::new(Vec::new()),
        }
    }

    #[test]
    fn each_sentence_runs_once_normalized_and_the_whole_is_24_khz() {
        let config = PiperConfig::from_json(FAHRETTIN).unwrap();
        let phonemizer = table();
        let mut runs = Vec::new();

        let audio = synthesize("Merhaba Erdem. Bu yerel!", &config, &phonemizer, |inputs| {
            runs.push(inputs.clone());
            // A quiet sentence: normalization brings its peak to 1.
            Ok((0..22_050)
                .map(|n| (n as f32 / 50.0).sin() * 0.25)
                .collect())
        })
        .unwrap();

        assert_eq!(runs.len(), 2, "one run per sentence");
        assert_eq!(runs[0].input_lengths, [runs[0].input.len() as i64]);
        assert_eq!(runs[0].scales, [0.667, 1.0, 0.8]);
        assert_eq!(runs[0].sid, None);
        assert_eq!(
            *phonemizer.asked.lock().unwrap(),
            vec![
                ("tr".to_string(), "Merhaba Erdem".to_string()),
                ("tr".to_string(), "Bu yerel".to_string())
            ]
        );
        let expected = 2 * 22_050 * 24_000 / 22_050;
        assert!(
            audio.len().abs_diff(expected) <= expected / 100,
            "{}",
            audio.len()
        );
        let peak = audio.samples().iter().fold(0.0_f32, |p, s| p.max(s.abs()));
        assert!(peak > 0.9 && peak <= 1.05, "{peak}");
    }

    #[test]
    fn a_multi_speaker_voice_is_run_as_speaker_zero() {
        let mut config = PiperConfig::from_json(FAHRETTIN).unwrap();
        config.num_speakers = 3;
        let mut sids = Vec::new();
        synthesize("Merhaba Erdem", &config, &table(), |inputs| {
            sids.push(inputs.sid);
            Ok(vec![0.5; 100])
        })
        .unwrap();
        assert_eq!(sids, vec![Some([0])]);
    }

    #[test]
    fn a_silent_sentence_becomes_zeros_and_a_loud_one_is_clipped() {
        let mut silent = vec![1e-9, -1e-9];
        normalize(&mut silent);
        assert_eq!(silent, vec![0.0, 0.0]);

        let mut loud = vec![0.5, -2.0, 1.0];
        normalize(&mut loud);
        assert_eq!(loud, vec![0.25, -1.0, 0.5]);
    }

    #[test]
    fn a_phonemizer_failure_names_piper_and_the_reason() {
        struct Broken;
        impl Phonemizer for Broken {
            fn phonemize(&self, _: &str, _: &str) -> Result<String, String> {
                Err("espeak-ng could not be started: not found".to_string())
            }
        }
        let config = PiperConfig::from_json(FAHRETTIN).unwrap();
        let error = synthesize("Merhaba", &config, &Broken, |_| Ok(vec![0.1]))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("Piper") && error.contains("espeak-ng could not be started"),
            "{error}"
        );
    }

    #[test]
    fn a_missing_voice_is_reported_by_path_before_any_runtime_is_loaded() {
        let root = tempfile::tempdir().unwrap();
        let tts = PiperTts::new(
            root.path().to_path_buf(),
            Some("tr_TR-fahrettin-medium".to_string()),
            Arc::new(table()),
            Arc::new(|| panic!("no runtime is loaded for a voice that is not there")),
        );

        let error = tts
            .generate("Merhaba", None, "tr_TR", Some("tr_TR-fahrettin-medium"))
            .unwrap_err();
        assert!(
            matches!(&error, VoiceMeError::MissingRuntimeAsset { path } if path.ends_with("model.onnx")),
            "{error:?}"
        );
        assert!(tts.warm_up().is_err());
        assert!(!tts.is_ready());
    }

    #[test]
    fn no_text_and_no_voice_are_refused_first() {
        let tts = PiperTts::new(
            PathBuf::from("/nonexistent"),
            None,
            Arc::new(table()),
            Arc::new(|| Ok(())),
        );
        assert!(matches!(
            tts.generate("  ", None, "tr_TR", Some("x")),
            Err(VoiceMeError::EmptyText)
        ));
        assert!(
            tts.generate("Merhaba", None, "tr_TR", None)
                .unwrap_err()
                .to_string()
                .contains("Piper")
        );
        // Nothing selected: warm-up has nothing to build.
        assert!(tts.warm_up().is_ok());
    }

    /// spec-backend-engine-and-device-selects: the injected providers are
    /// asked for after the runtime is committed, before the session is
    /// built, and their failure is the build's.
    #[test]
    fn the_injected_providers_are_asked_for_after_the_runtime() {
        let root = tempfile::tempdir().unwrap();
        let key = "tr_TR-fahrettin-medium";
        let files = assets::piper_voice_files(root.path(), key).unwrap();
        std::fs::create_dir_all(&files.dir).unwrap();
        std::fs::write(&files.model, b"not a graph").unwrap();
        std::fs::write(&files.config, FAHRETTIN).unwrap();

        let calls = Arc::new(StdMutex::new(Vec::new()));
        let (runtime_calls, provider_calls) = (calls.clone(), calls.clone());
        let tts = PiperTts::new(
            root.path().to_path_buf(),
            Some(key.to_string()),
            Arc::new(table()),
            Arc::new(move || {
                runtime_calls.lock().unwrap().push("runtime");
                Ok(())
            }),
        )
        .with_providers(Arc::new(move || {
            provider_calls.lock().unwrap().push("providers");
            Err(VoiceMeError::SpeechEngine(
                "the WebGPU provider is not available".to_string(),
            ))
        }));

        let error = tts.warm_up().unwrap_err().to_string();
        assert!(error.contains("WebGPU provider"), "{error}");
        assert_eq!(*calls.lock().unwrap(), vec!["runtime", "providers"]);
        assert!(!tts.is_ready());
    }

    #[test]
    fn failure_notes_follow_a_failed_build() {
        let error = with_notes(
            VoiceMeError::SpeechEngine("Piper could not load x".to_string()),
            &["libcudnn.so.9: not found".to_string()],
        )
        .to_string();
        assert!(error.contains("x (libcudnn.so.9: not found)"), "{error}");
        let untouched = with_notes(VoiceMeError::EmptyText, &["note".to_string()]);
        assert!(matches!(untouched, VoiceMeError::EmptyText));
    }
}
