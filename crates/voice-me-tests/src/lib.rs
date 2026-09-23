//! Black-box integration tests against the workspace's lib crates.

#[test]
fn placeholder() {
    assert!(true);
}

#[cfg(test)]
mod file_settings_store_reference_voice_sample {
    //! Story 1.2 acceptance: "Given no Reference Voice Sample exists, when I
    //! Record, speak, Stop, and Accept, then the clip is saved via
    //! `SettingsStore` and a fresh `FileSettingsStore::load` reports it as
    //! the active Reference Voice Sample."

    use voice_me_core::{FileSettingsStore, SettingsStore};

    #[test]
    fn save_then_fresh_load_round_trips_the_active_sample() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();

        let store = FileSettingsStore::with_dirs(
            config_dir.path().to_path_buf(),
            data_dir.path().to_path_buf(),
        );
        assert_eq!(
            store.load().unwrap().reference_voice_sample,
            None,
            "no sample exists yet"
        );

        let wav_bytes = b"RIFF....WAVEfmt fake pcm data for the round trip".to_vec();
        let state_after_save = store.save_reference_voice_sample(&wav_bytes).unwrap();
        let saved_path = state_after_save
            .reference_voice_sample
            .clone()
            .expect("save_reference_voice_sample must report the new active sample");
        assert_eq!(std::fs::read(&saved_path).unwrap(), wav_bytes);

        // A brand new `FileSettingsStore` (simulating a fresh process/run)
        // against the same directories still reports it as active.
        let fresh_store = FileSettingsStore::with_dirs(
            config_dir.path().to_path_buf(),
            data_dir.path().to_path_buf(),
        );
        let reloaded = fresh_store.load().unwrap();
        assert_eq!(reloaded.reference_voice_sample, Some(saved_path));
    }

    #[test]
    fn accepting_a_new_recording_replaces_the_previously_active_one() {
        let config_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let store = FileSettingsStore::with_dirs(
            config_dir.path().to_path_buf(),
            data_dir.path().to_path_buf(),
        );

        let first_clip = b"first accepted clip".to_vec();
        let state1 = store.save_reference_voice_sample(&first_clip).unwrap();
        let path = state1.reference_voice_sample.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), first_clip);

        let second_clip = b"second accepted clip replaces the first".to_vec();
        let state2 = store.save_reference_voice_sample(&second_clip).unwrap();
        let path2 = state2.reference_voice_sample.unwrap();

        assert_eq!(
            path, path2,
            "same fixed filename, not a new id/manifest (AD-6)"
        );
        assert_eq!(std::fs::read(&path2).unwrap(), second_clip);
    }
}

#[cfg(test)]
mod recording_duration_rule {
    //! Pure-function unit test for the 5s/60s reference-clip duration rule
    //! (Boundaries & Constraints: "hard minimum of 5s and maximum of 60s").

    use voice_me_ui::{
        MAX_RECORDING_SECS, MIN_RECORDING_SECS, recording_meets_minimum_duration, should_auto_stop,
    };

    #[test]
    fn constants_match_the_spec() {
        assert_eq!(MIN_RECORDING_SECS, 5.0);
        assert_eq!(MAX_RECORDING_SECS, 60.0);
    }

    #[test]
    fn shorter_than_five_seconds_is_rejected() {
        assert!(!recording_meets_minimum_duration(0.0));
        assert!(!recording_meets_minimum_duration(2.0));
        assert!(!recording_meets_minimum_duration(4.99));
    }

    #[test]
    fn five_seconds_or_longer_is_accepted() {
        assert!(recording_meets_minimum_duration(5.0));
        assert!(recording_meets_minimum_duration(12.0));
        assert!(recording_meets_minimum_duration(60.0));
        assert!(
            recording_meets_minimum_duration(90.0),
            "the max is enforced by auto-stop, not by this predicate"
        );
    }

    #[test]
    fn auto_stop_triggers_at_sixty_seconds_not_before() {
        assert!(!should_auto_stop(0.0));
        assert!(!should_auto_stop(59.9));
        assert!(should_auto_stop(60.0));
        assert!(should_auto_stop(75.0));
    }
}

#[cfg(test)]
mod dependency_report_gates_the_prompt_overlay {
    //! Stories 3.1 + 3.4 acceptance, end to end across three crates: a
    //! report from `voice-me-core`'s vocabulary decides which shape of
    //! `voice-me-ui`'s Prompt Overlay a hotkey press opens, and a blocked
    //! one never reaches the Speak Action.
    //!
    //! The composition root's own wiring is asserted in `voice-me-app`;
    //! what is proven here is that the value and the view agree — the
    //! report's `speech_engine_blocker` is exactly what the overlay refuses
    //! input over.

    use futures::channel::mpsc;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{AppContext as _, TestAppContext, component::Root, px, size};
    use voice_me_core::{AppEvent, Dependency, DependencyKind, DependencyReport, SpeechBackend};
    use voice_me_ui::{PromptOverlayView, blocker_notice};

    fn engine_rows(weights_missing: bool) -> Vec<Dependency> {
        vec![
            Dependency::ready(
                DependencyKind::OnnxRuntime,
                "ONNX Runtime",
                "Loaded from /rt",
            ),
            if weights_missing {
                Dependency::missing(
                    DependencyKind::ModelWeights,
                    "Speech model files (Q4)",
                    "Missing: /cache/onnx/language_model_q4.onnx",
                )
            } else {
                Dependency::ready(
                    DependencyKind::ModelWeights,
                    "Speech model files (Q4)",
                    "All 9 files present in /cache.",
                )
            },
        ]
    }

    /// Opens whichever overlay `report` implies, types a line, presses
    /// Enter, and reports what reached the channel.
    fn press_hotkey_and_speak(cx: &mut TestAppContext, report: &DependencyReport) -> Vec<AppEvent> {
        cx.update(gpui_kit::init);
        let (event_tx, mut event_rx) = mpsc::unbounded::<AppEvent>();
        let blocker = report.speech_engine_blocker().map(blocker_notice);

        let handle = cx.open_window(size(px(560.), px(84.)), |window, cx| {
            let view = cx.new(|cx| match blocker.clone() {
                Some(blocker) => PromptOverlayView::blocked(event_tx.clone(), blocker, window, cx),
                None => PromptOverlayView::new(event_tx.clone(), window, cx),
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.input("hello there", cx);
            window.press("enter", cx);
        })
        .unwrap();

        let mut sent = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            sent.push(event);
        }
        sent
    }

    #[gpui_kit::test]
    fn a_missing_speech_engine_dependency_blocks_and_then_unblocks(cx: &mut TestAppContext) {
        let blocked = DependencyReport::new(SpeechBackend::CPU, engine_rows(true));

        assert!(
            press_hotkey_and_speak(cx, &blocked).is_empty(),
            "a blocked overlay refuses input — nothing reaches the Speak Action"
        );

        // The missing file is placed on disk and the check re-run: the same
        // decision flips, with no restart in between.
        let resolved = DependencyReport::new(SpeechBackend::CPU, engine_rows(false));

        assert_eq!(
            press_hotkey_and_speak(cx, &resolved),
            vec![AppEvent::SpeakRequested {
                text: "hello there".to_string()
            }],
            "the overlay accepts input normally once the report comes back all-ready"
        );
    }

    /// The same report, read the way Settings reads it: a gap is still
    /// *reported*, it just does not gate typing unless it is the engine's.
    #[gpui_kit::test]
    fn a_virtual_microphone_gap_is_reported_but_never_blocks(cx: &mut TestAppContext) {
        let mut rows = engine_rows(false);
        rows.push(Dependency::missing(
            DependencyKind::VirtualMicrophone,
            "Virtual Microphone",
            "The device is not loaded.",
        ));
        let report = DependencyReport::new(SpeechBackend::CPU, rows);

        assert!(report.has_missing(), "the row is still missing");
        assert!(report.speech_engine_blocker().is_none());
        assert_eq!(
            press_hotkey_and_speak(cx, &report),
            vec![AppEvent::SpeakRequested {
                text: "hello there".to_string()
            }],
            "playback is what fails, later, through VirtualMicUnavailable"
        );
    }
}
