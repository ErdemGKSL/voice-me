//! The persistent half of the Virtual Microphone (spec-2-7 Decision 2): a
//! drop-in config file that makes the audio server create the device at
//! login, whether or not voice-me is running.
//!
//! This is the only thing voice-me writes outside its own directories, so
//! it is deliberately one file under one well-known name, with an
//! uninstall that removes exactly that file and nothing else.
//!
//! **It is a `pipewire-pulse` drop-in, not a `pipewire` one, and that is
//! load-bearing.** The obvious form — a `context.objects` entry under
//! `pipewire.conf.d` creating a `support.null-audio-sink` — produces a node
//! that looks perfect (right name, right `media.class`, an `input_MONO`
//! port and a `capture_MONO` port) and that no PulseAudio client can play
//! to: the name does not resolve, so the stream lands on the default sink
//! and comes out of the speakers instead, silently. Measured during
//! spec-2-7: the same tone, captured off the device, peaked at 0 through a
//! `context.objects` node and at 32768 through a `pulse.cmd` one. The
//! difference is `pulse.module.id` — a device that pipewire-pulse created
//! is one pipewire-pulse can address by name.

use std::path::{Path, PathBuf};

use voice_me_core::VoiceMeError;

/// The device's node name — what other applications match on when the user
/// picks voice-me as their microphone.
pub const DEVICE_NAME: &str = "voice-me";

/// The sink [`crate::LinuxVirtualMicAdapter::play`] writes into, and the
/// thing [`DEVICE_NAME`] is a remap of.
///
/// Two nodes rather than one, because a single `Audio/Source/Virtual`
/// null-sink is *not addressable by name from another process*: it is
/// published as a source, playback streams resolve names against sinks, and
/// pipewire-pulse answers a name it cannot resolve by substituting the
/// default sink — so the generated line comes out of the speakers. Measured
/// after spec-2-7 shipped: `paplay --device=voice-me` into such a node
/// captures silence, and only the process that had just loaded the module
/// could address it, which is why the spike's own binary appeared to work.
/// A plain sink resolves by name from anywhere; the remap turns its monitor
/// back into an ordinary-looking microphone, so the user still sees one
/// clean input device and nothing named "Monitor of".
pub const SINK_NAME: &str = "voice-me-sink";

/// What the user actually reads in a microphone list.
pub const DEVICE_DESCRIPTION: &str = "voice-me (Virtual Microphone)";

/// The pipewire-pulse drop-in directory, relative to the user's config
/// directory. See the module docs for why it is this one and not
/// `pipewire/pipewire.conf.d`.
const PIPEWIRE_PULSE_CONF_D: &str = "pipewire/pipewire-pulse.conf.d";

/// Our file inside it. Named for voice-me so an uninstall — or a puzzled
/// user — can tell at a glance who put it there.
const CONF_FILE_NAME: &str = "voice-me.conf";

/// The module that backs the device, and the arguments that shape it, in
/// the one place both the drop-in and the runtime load read them from —
/// otherwise a device created at login and one created by `install` could
/// quietly differ.
pub const NULL_SINK_MODULE: &str = "module-null-sink";

/// The module that turns the sink's monitor into a real capture source.
pub const REMAP_SOURCE_MODULE: &str = "module-remap-source";

/// The sink half: an ordinary null-sink, deliberately *without*
/// `media.class=Audio/Source/Virtual`, so it stays a real sink that any
/// process can address by name. The description is single-quoted because
/// the drop-in embeds these arguments inside a double-quoted string.
pub fn null_sink_arguments() -> String {
    format!(
        "sink_name={SINK_NAME} channel_map=mono \
         node.description='voice-me (speech output)'"
    )
}

/// The microphone half: the sink's monitor, republished under
/// [`DEVICE_NAME`] as a source applications list like any other input.
pub fn remap_source_arguments() -> String {
    format!(
        "source_name={DEVICE_NAME} master={SINK_NAME}.monitor channel_map=mono \
         source_properties=device.description='{DEVICE_DESCRIPTION}'"
    )
}

/// The drop-in's contents: tell pipewire-pulse to load the same two modules
/// `install` loads, every time it starts. Order matters — the remap needs a
/// monitor to attach to.
pub fn conf_contents() -> String {
    format!(
        "# Created by voice-me. Delete this file (or run voice-me's uninstall)\n\
         # to remove the Virtual Microphone.\n\
         pulse.cmd = [\n\
         \x20   {{ cmd = \"load-module\" args = \"{NULL_SINK_MODULE} {}\" }}\n\
         \x20   {{ cmd = \"load-module\" args = \"{REMAP_SOURCE_MODULE} {}\" }}\n\
         ]\n",
        null_sink_arguments(),
        remap_source_arguments()
    )
}

/// Where the drop-in lives under `config_dir`.
pub fn conf_path(config_dir: &Path) -> PathBuf {
    config_dir.join(PIPEWIRE_PULSE_CONF_D).join(CONF_FILE_NAME)
}

/// Write the drop-in, creating the PipeWire directories if the user has
/// never had any (a fresh account has no `pipewire-pulse.conf.d` at all).
///
/// Overwrites an existing file rather than checking first: the contents are
/// generated, so an older voice-me's version is exactly what should be
/// replaced, and rewriting identical bytes is what makes install idempotent.
pub fn write_conf(config_dir: &Path) -> Result<PathBuf, VoiceMeError> {
    let path = conf_path(config_dir);
    let parent = path.parent().expect("conf_path always has a parent");

    std::fs::create_dir_all(parent).map_err(|error| {
        VoiceMeError::VirtualMicUnavailable(format!(
            "could not create {}: {error}",
            parent.display()
        ))
    })?;
    std::fs::write(&path, conf_contents()).map_err(|error| {
        VoiceMeError::VirtualMicUnavailable(format!("could not write {}: {error}", path.display()))
    })?;

    Ok(path)
}

/// Remove the drop-in. Returns whether there was one to remove, so an
/// uninstall on a machine that never installed is a quiet `Ok(false)`
/// rather than an error.
pub fn remove_conf(config_dir: &Path) -> Result<bool, VoiceMeError> {
    let path = conf_path(config_dir);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(VoiceMeError::VirtualMicUnavailable(format!(
            "could not remove {}: {error}",
            path.display()
        ))),
    }
}

/// The user's config directory (`XDG_CONFIG_HOME`, else `~/.config`) — the
/// same resolution `voice-me-core`'s settings store uses, via the same
/// crate, so the two never disagree about where "config" is.
pub fn user_config_dir() -> Result<PathBuf, VoiceMeError> {
    directories::BaseDirs::new()
        .map(|dirs| dirs.config_dir().to_path_buf())
        .ok_or_else(|| {
            VoiceMeError::VirtualMicUnavailable(
                "could not resolve the user's config directory".to_string(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The drop-in is the whole persistence mechanism, and nothing in CI
    /// can start an audio server to find out whether it is accepted — so
    /// the properties the device depends on are pinned here.
    #[test]
    fn the_drop_in_loads_both_halves_in_the_order_they_depend_on() {
        let conf = conf_contents();

        let sink = conf
            .find(NULL_SINK_MODULE)
            .expect("the drop-in has to create the sink");
        let remap = conf
            .find(REMAP_SOURCE_MODULE)
            .expect("the drop-in has to republish the monitor as a microphone");
        assert!(
            sink < remap,
            "the remap attaches to the sink's monitor, so a drop-in that loads it \
             first silently yields no microphone"
        );

        assert!(
            conf.contains(&format!("sink_name={SINK_NAME}")),
            "playback addresses the sink by name"
        );
        assert!(
            conf.contains(&format!("source_name={DEVICE_NAME}")),
            "the microphone the user picks has to carry the plain device name"
        );
        assert!(
            !conf.contains("media.class=Audio/Source/Virtual"),
            "a node published straight as a virtual source is not addressable by name \
             from another process — playback lands on the default sink, i.e. the \
             user's speakers"
        );
        assert!(conf.contains("pulse.cmd"));
    }

    /// The one state the whole adapter is built to avoid: two nodes called
    /// `voice-me`. The remap owns that name, so the sink must not also
    /// claim it.
    #[test]
    fn the_two_halves_do_not_share_a_name() {
        assert_ne!(SINK_NAME, DEVICE_NAME);
        assert!(
            !null_sink_arguments().contains(&format!("sink_name={DEVICE_NAME} ")),
            "the sink must not take the microphone's name"
        );
    }

    /// The login-time device and the one `install` creates for the current
    /// session must be the same device, so both read the arguments from
    /// here. A drift would mean a user who installed and then rebooted
    /// found a differently-shaped microphone.
    #[test]
    fn the_drop_in_embeds_exactly_the_runtime_module_arguments() {
        assert!(conf_contents().contains(&null_sink_arguments()));
    }

    /// The directory is the whole discovery of spec-2-7: a drop-in under
    /// `pipewire.conf.d` creates a device no PulseAudio client can play to,
    /// so the generated line comes out of the speakers. Asserted against the
    /// literal path rather than against `conf_path` itself, which would hold
    /// for any directory name.
    #[test]
    fn the_drop_in_goes_in_the_pipewire_pulse_directory() {
        let path = conf_path(Path::new("/config"));

        assert_eq!(
            path,
            Path::new("/config/pipewire/pipewire-pulse.conf.d/voice-me.conf"),
            "a `pipewire.conf.d` drop-in produces an unaddressable device"
        );
    }

    /// `DEVICE_DESCRIPTION` is what the user reads in a microphone list, and
    /// it travels into the drop-in inside two layers of quoting. A character
    /// that breaks that quoting yields a config the audio server rejects at
    /// login — silently, hours later.
    #[test]
    fn the_description_survives_into_the_drop_in() {
        assert!(!DEVICE_DESCRIPTION.contains(['"', '\'', '\\']));
        assert!(conf_contents().contains(DEVICE_DESCRIPTION));
    }

    #[test]
    fn install_creates_the_pipewire_directories_when_the_user_has_none() {
        let temp = tempfile::tempdir().expect("temp dir");
        let config_dir = temp.path().join("config");

        let written = write_conf(&config_dir).expect("write");

        assert_eq!(written, conf_path(&config_dir));
        assert_eq!(std::fs::read_to_string(&written).unwrap(), conf_contents());
    }

    #[test]
    fn installing_twice_leaves_one_file_with_current_contents() {
        let temp = tempfile::tempdir().expect("temp dir");

        write_conf(temp.path()).expect("first write");
        std::fs::write(
            conf_path(temp.path()),
            "stale contents from an older voice-me",
        )
        .unwrap();
        write_conf(temp.path()).expect("second write");

        let dir = conf_path(temp.path()).parent().unwrap().to_path_buf();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        assert_eq!(
            std::fs::read_to_string(conf_path(temp.path())).unwrap(),
            conf_contents()
        );
    }

    #[test]
    fn uninstall_removes_the_file_and_is_quiet_when_there_is_nothing_to_remove() {
        let temp = tempfile::tempdir().expect("temp dir");

        assert!(!remove_conf(temp.path()).expect("uninstall with nothing installed"));

        write_conf(temp.path()).expect("write");
        assert!(remove_conf(temp.path()).expect("uninstall"));
        assert!(!conf_path(temp.path()).exists());
    }

    /// A config directory that cannot be created has to name the path it
    /// failed on: "virtual microphone unavailable" with no path is
    /// unactionable, and this is the one install failure a user can fix.
    #[test]
    fn an_unwritable_config_directory_names_the_path() {
        let temp = tempfile::tempdir().expect("temp dir");
        let blocked = temp.path().join("config");
        std::fs::write(&blocked, "a file where the directory should be").unwrap();

        let error = write_conf(&blocked).expect_err("a file cannot contain directories");

        let message = error.to_string();
        assert!(
            message.contains(&blocked.display().to_string()),
            "error must name the path: {message}"
        );
    }
}
