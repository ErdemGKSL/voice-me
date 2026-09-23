//! `voice-me-deps` — the `DependencyProvisioningPort` adapter.
//!
//! Story 3.1's half only: **detection**, never provisioning. This crate
//! reads the filesystem and the environment, builds one
//! [`DependencyReport`] for the selected backend, sends it on the shared
//! `AppEvent` channel (AD-3) and holds nothing afterwards. Story 3.2 adds
//! the downloading half; until then there is no HTTP client here and no
//! network access of any kind.
//!
//! Two rules shape everything below:
//!
//! * **Backend-relative.** The row list is derived from the selected
//!   [`SpeechBackend`], never from a fixed list — a CPU selection reports
//!   no GPU provider library and no FP16 weights, and FP16 files sitting in
//!   the cache do not satisfy a Q4 selection.
//! * **One list.** The model filenames come from `voice-me-core::assets`,
//!   the same module `voice-me-tts`'s `ModelCache` reads, so the check and
//!   the engine cannot disagree about what "provisioned" means. `deps`
//!   never depends on `tts`.

use std::path::Path;

use voice_me_core::{
    AppEvent, AppEventSender, Dependency, DependencyKind, DependencyProvisioningPort,
    DependencyReport, SpeechBackend, SpeechWeights, VoiceMeError, assets,
};

/// `DependencyProvisioningPort` adapter.
///
/// Stateless: every call re-reads the world, because the whole point of
/// "Check again" is that the answer changed since last time.
#[derive(Debug, Default, Clone, Copy)]
pub struct DepsAdapter;

impl DepsAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl DependencyProvisioningPort for DepsAdapter {
    fn check(&self, backend: SpeechBackend, events: AppEventSender) -> Result<(), VoiceMeError> {
        // The one failure that is not a report: with no cache directory to
        // resolve there is no path to state anything about, so the check
        // itself failed and the Dependencies tab says so with this reason.
        let root = assets::model_cache_root()?;

        let report = DependencyReport::new(
            backend,
            speech_engine_rows(&root, backend.weights)
                .into_iter()
                .chain(virtual_microphone_row())
                .collect(),
        );

        // A closed receiver means the app is shutting down; there is
        // nowhere to report that to, and nothing this adapter could do
        // about it.
        let _ = events.unbounded_send(AppEvent::DependencyCheckCompleted { report });
        Ok(())
    }
}

/// The rows a missing one of which blocks the Prompt Overlay (Decision 3):
/// the ONNX Runtime library, and the model files the selected weight
/// variant needs.
///
/// Pure apart from `ORT_DYLIB_PATH` and `Path::exists`, and takes the cache
/// root rather than resolving it, so the whole I/O matrix can be driven
/// against temporary directories.
pub fn speech_engine_rows(root: &Path, weights: SpeechWeights) -> Vec<Dependency> {
    vec![runtime_row(root), model_weights_row(root, weights)]
}

/// The ONNX Runtime row, reporting on exactly the rule the engine will
/// apply when it loads the library: `ORT_DYLIB_PATH` if set, otherwise the
/// cache root's own copy.
///
/// The two missing cases are deliberately different sentences. "You
/// configured a path and it is not there" is a stale setting the user has
/// to fix themselves; "nothing is configured and nothing is in the cache"
/// is a provisioning gap Story 3.2 will be able to fill with one click.
fn runtime_row(root: &Path) -> Dependency {
    let resolved = assets::resolve_runtime_dylib(root);
    let path = resolved.path.display();

    if resolved.path.exists() {
        return Dependency::ready(
            DependencyKind::OnnxRuntime,
            RUNTIME_LABEL,
            // What was actually established is that a file is there — the
            // library has not been loaded, and saying so would be a claim
            // this check never made.
            format!("Found at {path}."),
        );
    }

    if resolved.configured {
        Dependency::missing(
            DependencyKind::OnnxRuntime,
            RUNTIME_LABEL,
            format!(
                "{} is set to {path}, and nothing is there. Point it at an ONNX Runtime library, \
                 or unset it to use the one in the cache.",
                assets::RUNTIME_DYLIB_ENV
            ),
        )
        // Nothing can be downloaded to satisfy a path the user chose: the
        // fix is to correct or clear the variable.
        .manual()
    } else {
        Dependency::missing(
            DependencyKind::OnnxRuntime,
            RUNTIME_LABEL,
            format!(
                "Not found at {path}. {} is unset, so that is where voice-me looks.",
                assets::RUNTIME_DYLIB_ENV
            ),
        )
    }
}

/// The model-files row for the *selected* weight variant.
///
/// One row rather than nine, because the nine files are one artefact set as
/// far as the user is concerned — but the detail still names an exact
/// absolute path, since "some model files are missing" is unactionable and
/// a path is not.
fn model_weights_row(root: &Path, weights: SpeechWeights) -> Dependency {
    let required = assets::required_model_files(root, weights);
    let missing: Vec<_> = required
        .iter()
        .filter(|path| !path.exists())
        .cloned()
        .collect();

    let label = format!("Speech model files ({})", weights_label(weights));

    match missing.first() {
        None => Dependency::ready(
            DependencyKind::ModelWeights,
            label,
            format!(
                "All {} files present in {}.",
                required.len(),
                root.display()
            ),
        ),
        Some(first) => {
            let detail = if missing.len() == 1 {
                format!("Missing: {}", first.display())
            } else {
                format!(
                    "Missing {} of {} files, starting with {}",
                    missing.len(),
                    required.len(),
                    first.display()
                )
            };
            Dependency::missing(DependencyKind::ModelWeights, label, detail)
        }
    }
}

/// How the selected weight variant is named to the user.
fn weights_label(weights: SpeechWeights) -> &'static str {
    match weights {
        SpeechWeights::Q4 => "Q4",
        SpeechWeights::Fp16 => "FP16",
        SpeechWeights::Fp32 => "FP32",
    }
}

const RUNTIME_LABEL: &str = "ONNX Runtime";
const VIRTUAL_MIC_LABEL: &str = "Virtual Microphone";

/// The Virtual Microphone row — Linux only (Decision 1).
///
/// The status is *asked of* `voice-me-audio-linux` rather than detected
/// again here: that crate already owns every line of PipeWire knowledge in
/// this workspace, and a second copy of it would be a second thing to be
/// wrong. Windows gets no row at all until Story 2.8 gives the Windows
/// driver a control surface worth reporting on.
#[cfg(target_os = "linux")]
fn virtual_microphone_row() -> Option<Dependency> {
    let row = match voice_me_audio_linux::virtual_microphone_available() {
        Ok(true) => Dependency::ready(
            DependencyKind::VirtualMicrophone,
            VIRTUAL_MIC_LABEL,
            format!(
                "Other applications can select \"{}\" as their microphone.",
                voice_me_audio_linux::DEVICE_DESCRIPTION
            ),
        ),
        Ok(false) => Dependency::missing(
            DependencyKind::VirtualMicrophone,
            VIRTUAL_MIC_LABEL,
            "The device is not loaded. voice-me installs it at startup; if it keeps disappearing, \
             the audio server may have been restarted."
                .to_string(),
        ),
        // Not a failed check: the audio server being unreachable *is* the
        // answer, and the reason is the useful half of it.
        Err(error) => Dependency::missing(
            DependencyKind::VirtualMicrophone,
            VIRTUAL_MIC_LABEL,
            format!("Could not ask the audio server: {error}"),
        )
        .manual(),
    };
    Some(row)
}

#[cfg(not(target_os = "linux"))]
fn virtual_microphone_row() -> Option<Dependency> {
    None
}

#[cfg(test)]
mod tests {
    //! The story's I/O & Edge-Case Matrix, driven against temporary
    //! directories.
    //!
    //! `ORT_DYLIB_PATH` is process-global, so the rows that depend on it are
    //! serialised behind one mutex and always restore what they found — a
    //! stray variable left behind would silently change what every later
    //! test in this process sees.

    use std::path::Path;
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use voice_me_core::DependencyStatus;

    use super::*;

    /// Holds the lock and restores every variable it touched.
    ///
    /// One type rather than one per variable because `std::sync::Mutex` is
    /// not reentrant: a test that wanted both `ORT_DYLIB_PATH` and
    /// `VOICE_ME_MODEL_CACHE` would deadlock against itself.
    struct EnvGuard {
        previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
        _guard: MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn new() -> Self {
            static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
            let guard = LOCK
                .get_or_init(|| Mutex::new(()))
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self {
                previous: Vec::new(),
                _guard: guard,
            }
        }

        fn remember(&mut self, key: &'static str) {
            if !self.previous.iter().any(|(seen, _)| *seen == key) {
                self.previous.push((key, std::env::var_os(key)));
            }
        }

        fn set(mut self, key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            self.remember(key);
            // Every test that touches the environment holds this same lock,
            // and `Drop` puts back what was there.
            unsafe { std::env::set_var(key, value) };
            self
        }

        fn unset(mut self, key: &'static str) -> Self {
            self.remember(key);
            unsafe { std::env::remove_var(key) };
            self
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.previous.drain(..) {
                match value {
                    Some(value) => unsafe { std::env::set_var(key, value) },
                    None => unsafe { std::env::remove_var(key) },
                }
            }
        }
    }

    /// The common starting point: no configured runtime path, so the check
    /// falls back to the cache root's own copy.
    fn no_configured_runtime() -> EnvGuard {
        EnvGuard::new().unset(assets::RUNTIME_DYLIB_ENV)
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"not really a model, but it is on disk").unwrap();
    }

    /// A cache root with every file the given variant needs, plus the
    /// runtime library where the check looks for it with no env var set.
    fn provisioned(weights: SpeechWeights) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for path in assets::required_model_files(dir.path(), weights) {
            touch(&path);
        }
        touch(&assets::bundled_runtime_dylib(dir.path()));
        dir
    }

    fn row(rows: &[Dependency], kind: DependencyKind) -> &Dependency {
        rows.iter()
            .find(|dependency| dependency.kind == kind)
            .unwrap_or_else(|| panic!("no {kind:?} row in {rows:?}"))
    }

    #[test]
    fn a_fully_provisioned_cpu_cache_reports_every_row_ready() {
        let _env = no_configured_runtime();
        let dir = provisioned(SpeechWeights::Q4);

        let rows = speech_engine_rows(dir.path(), SpeechWeights::Q4);

        assert!(
            rows.iter().all(|row| row.status == DependencyStatus::Ready),
            "{rows:?}"
        );
    }

    #[test]
    fn an_absent_language_model_names_the_exact_path() {
        let _env = no_configured_runtime();
        let dir = provisioned(SpeechWeights::Q4);
        let weights = assets::graph_file(
            dir.path(),
            assets::language_model_file_name(SpeechWeights::Q4),
        );
        std::fs::remove_file(&weights).unwrap();

        let rows = speech_engine_rows(dir.path(), SpeechWeights::Q4);
        let model = row(&rows, DependencyKind::ModelWeights);

        assert_eq!(model.status, DependencyStatus::Missing);
        assert!(
            model.detail.contains(&weights.display().to_string()),
            "the absent path is the only actionable thing to say: {}",
            model.detail
        );
    }

    /// The external-data sibling is as required as the graph, and ONNX
    /// Runtime's own message for an absent one names a bare filename that
    /// is useless to whoever has to go fetch it.
    #[test]
    fn an_absent_external_data_file_is_missing_too() {
        let _env = no_configured_runtime();
        let dir = provisioned(SpeechWeights::Q4);
        let data =
            assets::graph_file(dir.path(), "speech_encoder.onnx").with_extension("onnx_data");
        std::fs::remove_file(&data).unwrap();

        let rows = speech_engine_rows(dir.path(), SpeechWeights::Q4);

        assert_eq!(
            row(&rows, DependencyKind::ModelWeights).status,
            DependencyStatus::Missing
        );
    }

    #[test]
    fn an_absent_tokenizer_is_missing_too() {
        let _env = no_configured_runtime();
        let dir = provisioned(SpeechWeights::Q4);
        std::fs::remove_file(assets::tokenizer_file(dir.path())).unwrap();

        let rows = speech_engine_rows(dir.path(), SpeechWeights::Q4);

        assert_eq!(
            row(&rows, DependencyKind::ModelWeights).status,
            DependencyStatus::Missing
        );
    }

    #[test]
    fn an_unresolvable_runtime_says_where_it_looked() {
        let _env = no_configured_runtime();
        let dir = provisioned(SpeechWeights::Q4);
        std::fs::remove_file(assets::bundled_runtime_dylib(dir.path())).unwrap();

        let rows = speech_engine_rows(dir.path(), SpeechWeights::Q4);
        let runtime = row(&rows, DependencyKind::OnnxRuntime);

        assert_eq!(runtime.status, DependencyStatus::Missing);
        assert!(
            runtime.detail.contains(
                &assets::bundled_runtime_dylib(dir.path())
                    .display()
                    .to_string()
            ),
            "{}",
            runtime.detail
        );
        assert!(
            runtime.detail.contains(assets::RUNTIME_DYLIB_ENV),
            "the rule itself has to be stated, or the path looks arbitrary: {}",
            runtime.detail
        );
    }

    #[test]
    fn a_stale_configured_runtime_path_names_the_configured_path() {
        let dir = provisioned(SpeechWeights::Q4);
        let stale = dir.path().join("moved-away").join("libonnxruntime.so");
        let _env = EnvGuard::new().set(assets::RUNTIME_DYLIB_ENV, &stale);

        let rows = speech_engine_rows(dir.path(), SpeechWeights::Q4);
        let runtime = row(&rows, DependencyKind::OnnxRuntime);

        assert_eq!(runtime.status, DependencyStatus::Missing);
        assert!(
            runtime.detail.contains(&stale.display().to_string()),
            "the path the user configured is the one that is wrong: {}",
            runtime.detail
        );
        assert!(
            !runtime.automatable,
            "nothing can be downloaded to satisfy a path the user chose"
        );
    }

    #[test]
    fn a_configured_runtime_that_exists_is_ready() {
        let dir = provisioned(SpeechWeights::Q4);
        let elsewhere = dir.path().join("elsewhere").join("libonnxruntime.so");
        touch(&elsewhere);
        let _env = EnvGuard::new().set(assets::RUNTIME_DYLIB_ENV, &elsewhere);

        let rows = speech_engine_rows(dir.path(), SpeechWeights::Q4);

        assert_eq!(
            row(&rows, DependencyKind::OnnxRuntime).status,
            DependencyStatus::Ready
        );
    }

    /// The backend-relative rule, at its sharpest: FP16 files on disk do
    /// not make a Q4 selection ready, and no FP16 row is reported at all.
    #[test]
    fn another_variants_weights_do_not_satisfy_the_selected_one() {
        let _env = no_configured_runtime();
        let dir = provisioned(SpeechWeights::Fp16);

        let rows = speech_engine_rows(dir.path(), SpeechWeights::Q4);
        let model = row(&rows, DependencyKind::ModelWeights);

        assert_eq!(model.status, DependencyStatus::Missing);
        assert!(
            model.detail.contains("language_model_q4.onnx"),
            "{}",
            model.detail
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row.kind == DependencyKind::ModelWeights)
                .count(),
            1,
            "the unselected variant is not a dependency, so it is not a row: {rows:?}"
        );
        assert!(
            !rows.iter().any(|row| row.label.contains("FP16")),
            "{rows:?}"
        );
    }

    /// Decision 3: a machine with no speech engine blocks; the same machine
    /// with only its Virtual Microphone gone does not.
    #[test]
    fn only_speech_engine_rows_block_the_overlay() {
        let blocked = DependencyReport::new(
            SpeechBackend::CPU,
            vec![Dependency::missing(
                DependencyKind::ModelWeights,
                "Speech model files (Q4)",
                "Missing: /cache/onnx/language_model_q4.onnx",
            )],
        );
        assert_eq!(
            blocked.speech_engine_blocker().map(|row| row.kind),
            Some(DependencyKind::ModelWeights)
        );

        let mic_only = DependencyReport::new(
            SpeechBackend::CPU,
            vec![
                Dependency::ready(DependencyKind::OnnxRuntime, "ONNX Runtime", "ok"),
                Dependency::ready(
                    DependencyKind::ModelWeights,
                    "Speech model files (Q4)",
                    "ok",
                ),
                Dependency::missing(
                    DependencyKind::VirtualMicrophone,
                    "Virtual Microphone",
                    "not loaded",
                ),
            ],
        );
        assert!(mic_only.has_missing(), "it is still reported as missing");
        assert!(
            mic_only.speech_engine_blocker().is_none(),
            "but the user can still type — playback is what fails, later"
        );
    }

    /// With no `HOME`, no `XDG_CACHE_HOME` and no override there is no path
    /// to say anything about, so the check itself fails — as a domain
    /// error the tab can print, never as a panic.
    #[test]
    #[cfg(not(target_os = "windows"))]
    fn an_unresolvable_cache_root_fails_the_check_rather_than_panicking() {
        let _env = EnvGuard::new()
            .unset(assets::CACHE_ROOT_ENV)
            .unset("XDG_CACHE_HOME")
            .unset("HOME");

        let error = assets::model_cache_root().expect_err("nothing to resolve a cache root from");

        assert!(
            error.to_string().contains("cache directory"),
            "the reason has to reach the Dependencies tab: {error}"
        );
    }

    /// Decision 1's other half: Windows gets no Virtual Microphone row at
    /// all until Story 2.8 gives the driver a control surface worth
    /// reporting on. Runs on the Windows CI job; on Linux the row above is
    /// what is exercised instead.
    #[test]
    #[cfg(not(target_os = "linux"))]
    fn no_virtual_microphone_row_is_reported_off_linux() {
        assert!(virtual_microphone_row().is_none());
    }

    #[test]
    fn the_report_carries_the_backend_it_was_computed_for() {
        let dir = provisioned(SpeechWeights::Q4);
        let _env = EnvGuard::new()
            .unset(assets::RUNTIME_DYLIB_ENV)
            .set(assets::CACHE_ROOT_ENV, dir.path());
        let (tx, mut rx) = futures::channel::mpsc::unbounded();

        DepsAdapter::new().check(SpeechBackend::CPU, tx).unwrap();

        let AppEvent::DependencyCheckCompleted { report } =
            rx.try_recv().expect("the check reports by event")
        else {
            panic!("the check sends exactly one kind of event");
        };
        assert_eq!(report.backend, SpeechBackend::CPU);
        assert!(
            report
                .dependencies
                .iter()
                .any(|row| row.kind == DependencyKind::ModelWeights)
        );
    }
}
