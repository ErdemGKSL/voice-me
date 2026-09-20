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
