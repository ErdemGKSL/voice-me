//! `voice-me-audio-linux` — the Linux `VirtualMicPort` adapter.
//!
//! The Virtual Microphone is a PipeWire/PulseAudio null-sink published as
//! a virtual *source*, so every other application lists it as a microphone.
//! It needs no kernel driver, no elevated privileges and no `sudo`
//! (spec-2-7 verified this end to end on PipeWire 1.6).
//!
//! Two halves, per spec-2-7's decisions:
//!
//! * **Decision 1** — the PulseAudio client API is the mechanism.
//!   WirePlumber silently ignores a playback client's `target` when the
//!   target is an `Audio/Source/Virtual` node and routes it to the default
//!   sink instead: the audio comes out of the speakers and the Virtual
//!   Microphone stays silent. Addressing the device *by name* through the
//!   Pulse API is what sidesteps that, and it keeps working on a host
//!   running classic PulseAudio rather than PipeWire.
//! * **Decision 2** — the device is installed once and persists. A drop-in
//!   under the user's PipeWire config makes the audio server create it at
//!   login, so a game or Discord keeps it selected between sessions;
//!   [`install`] also creates it for the current session, since the drop-in
//!   only takes effect when the audio server next starts.
//!
//! Story 2.9 wires this into the Speak Action; nothing here reaches into
//! `voice-me-app`.

// This crate *is* the Linux capability (AD-2): it links against the system
// `libpulse`, which does not exist on a Windows runner, so the whole body
// is gated rather than the workspace being made unbuildable there.
#![cfg(target_os = "linux")]

mod config;
mod pulse;

use std::path::PathBuf;

use libpulse_binding::sample::{Format, Spec};
use voice_me_core::{AudioBuffer, SAMPLE_RATE, VirtualMicPort, VoiceMeError};

pub use config::{DEVICE_DESCRIPTION, DEVICE_NAME};

use config::SINK_NAME;
use pulse::PulseSession;

/// What the audio server shows this stream as, next to the device.
const STREAM_NAME: &str = "generated speech";

/// The application name the playback stream registers under.
///
/// Deliberately *not* [`DEVICE_NAME`]: pipewire-pulse names the stream's
/// own node after the application, so an application called `voice-me`
/// publishes a second node called `voice-me` beside the device — the exact
/// duplicate-name state [`LinuxVirtualMicAdapter::play`] refuses to play
/// into.
const APP_NAME: &str = "voice-me speech";

/// Linux `VirtualMicPort` adapter.
///
/// Holds no connection: each [`VirtualMicPort::play`] opens a playback
/// stream, writes the buffer and closes. A Speak Action happens once every
/// few tens of seconds at most, and a stream held open between them would
/// keep the device busy — and be one more thing to rebuild after a suspend
/// or a server restart — for no gain.
pub struct LinuxVirtualMicAdapter {
    /// The user's config directory. Injectable so tests never touch a real
    /// one, mirroring `SettingsStore::with_dirs` in `voice-me-core`.
    config_dir: PathBuf,
}

impl LinuxVirtualMicAdapter {
    /// Build an adapter against the user's real config directory.
    pub fn new() -> Result<Self, VoiceMeError> {
        Ok(Self {
            config_dir: config::user_config_dir()?,
        })
    }

    /// Build an adapter against an explicit config directory.
    pub fn with_config_dir(config_dir: PathBuf) -> Self {
        Self { config_dir }
    }

    /// Create the Virtual Microphone: write the drop-in so it comes back at
    /// every login, and load the module so the device exists right now.
    ///
    /// Idempotent — it always ends with exactly one device under
    /// [`DEVICE_NAME`], never two. Returns the path of the drop-in it wrote.
    ///
    /// It unloads *every* module it finds before loading one, rather than
    /// leaving an existing device alone. Two devices sharing a name is the
    /// one state that must never survive an install: the name then resolves
    /// ambiguously and a playback stream asking for it is silently routed
    /// to the default sink — the generated line comes out of the speakers,
    /// which for this product is the worst possible failure. Measured
    /// during spec-2-7, and it is how every mysterious silence in that
    /// spike turned out to have been produced.
    pub fn install(&self) -> Result<PathBuf, VoiceMeError> {
        // The live device first, the drop-in last: a failure here must not
        // leave behind a file that conjures the device at the next login
        // after install told the user it had failed.
        let mut session = PulseSession::connect()?;
        for index in session.loaded_null_sinks()? {
            session.unload_module(index)?;
        }
        session.load_device()?;

        config::write_conf(&self.config_dir)
    }

    /// Remove everything [`Self::install`] created: the drop-in, so it does
    /// not come back at the next login, and the live device if one is
    /// loaded.
    ///
    /// Quiet when nothing is installed — an uninstall on a clean machine is
    /// a no-op, not a failure.
    pub fn uninstall(&self) -> Result<(), VoiceMeError> {
        config::remove_conf(&self.config_dir)?;

        // No audio server means no live device to remove, which is the
        // state uninstall is trying to reach — so it is done, not failed.
        // The drop-in above is the half that matters on a headless box.
        let Ok(mut session) = PulseSession::connect() else {
            return Ok(());
        };
        for index in session.loaded_null_sinks()? {
            session.unload_module(index)?;
        }

        Ok(())
    }

    /// Whether the Virtual Microphone is currently present as a capture
    /// source — i.e. whether another application can select it right now.
    pub fn is_present(&self) -> Result<bool, VoiceMeError> {
        virtual_microphone_available()
    }

    /// How many devices carry [`DEVICE_NAME`]. One is correct; two would
    /// mean an install duplicated the Virtual Microphone, which is the
    /// failure a user notices as two identical entries in Discord.
    pub fn device_count(&self) -> Result<usize, VoiceMeError> {
        device_count()
    }
}

/// Whether the Virtual Microphone is available on this machine right now —
/// the Dependency Check's question (Story 3.1, Decision 1).
///
/// A free function rather than a method because the caller has nothing to
/// construct: `voice-me-deps` wants an answer, not an adapter, and building
/// one would mean resolving a config directory it has no use for. It is
/// also deliberately the *same* call [`LinuxVirtualMicAdapter::is_present`]
/// makes — PipeWire detection is not re-implemented anywhere else, here or
/// in `voice-me-deps`.
///
/// Cheap and non-mutating: one short-lived connection, one source
/// enumeration, no module loaded and no file written. `Err` carries why the
/// audio server could not be asked, which is itself the answer the user
/// needs to read.
pub fn virtual_microphone_available() -> Result<bool, VoiceMeError> {
    Ok(device_count()? > 0)
}

/// How many devices carry [`DEVICE_NAME`]. One is correct; two would mean
/// an install duplicated the Virtual Microphone, which is the failure a
/// user notices as two identical entries in Discord.
///
/// The one enumeration in this crate: both the adapter's methods and the
/// Dependency Check's question go through it, so "the same call" is a fact
/// the compiler enforces rather than a claim in a doc comment.
fn device_count() -> Result<usize, VoiceMeError> {
    PulseSession::connect()?.source_count(DEVICE_NAME)
}

impl VirtualMicPort for LinuxVirtualMicAdapter {
    fn play(&self, audio: &AudioBuffer) -> Result<(), VoiceMeError> {
        // Nothing to play is not a failure, and opening a stream to write
        // zero bytes would leave `drain` waiting on a buffer that never
        // arrives.
        if audio.is_empty() {
            return Ok(());
        }

        // Refuse before opening a stream unless there is exactly one device
        // under this name. PulseAudio does not fail a playback stream whose
        // requested device it cannot resolve — it silently substitutes the
        // default sink, so a missing or duplicated Virtual Microphone would
        // send the user's line out of their speakers instead of into their
        // voice chat. For this product that is not a degraded outcome, it is
        // the outcome the product exists to prevent, so it is checked rather
        // than hoped for.
        let mut session = PulseSession::connect_as(APP_NAME)?;
        match session.source_count(DEVICE_NAME)? {
            1 => {}
            0 => {
                return Err(VoiceMeError::VirtualMicUnavailable(format!(
                    "there is no `{DEVICE_NAME}` device — the Virtual Microphone has not been \
                     installed on this machine"
                )));
            }
            duplicates => {
                return Err(VoiceMeError::VirtualMicUnavailable(format!(
                    "there are {duplicates} devices called `{DEVICE_NAME}` — playback would be \
                     routed to the speakers instead; remove the extras and reinstall"
                )));
            }
        }

        // AD-11: the buffer is 24 kHz mono f32 and stays that way. The
        // sample spec *declares* that to the server, which resamples to
        // whatever the device runs at (48 kHz, typically) — writing a
        // resampler here would re-implement what every stream already gets.
        let spec = Spec {
            format: Format::F32le,
            rate: SAMPLE_RATE,
            channels: 1,
        };
        debug_assert!(spec.is_valid());

        // `play_to` does not return until the server has drained the
        // buffer, which is why this call belongs on the blocking pool
        // (AD-5) — and it refuses to play at all unless the stream landed
        // on the device this adapter asked for.
        session.play_to(SINK_NAME, STREAM_NAME, &spec, &to_le_bytes(audio.samples()))
    }
}

/// `f32` samples as the little-endian bytes `Format::F32le` promises.
///
/// Explicit rather than a `bytemuck`-style cast: this is the one place the
/// declared sample format and the in-memory layout have to agree, and the
/// cost is one copy of a few hundred kilobytes per utterance.
fn to_le_bytes(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(size_of_val(samples));
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The empty-buffer short-circuit is the one `play` path that reaches a
    /// conclusion without an audio server, so it is also the only one CI
    /// can execute. It matters: `drain` on a stream nothing was written to
    /// blocks, and Story 2.9 hands over whatever generation produced.
    #[test]
    fn playing_an_empty_buffer_does_nothing_and_succeeds() {
        let adapter = LinuxVirtualMicAdapter::with_config_dir(PathBuf::from("/nonexistent"));

        assert!(adapter.play(&AudioBuffer::default()).is_ok());
    }

    #[test]
    fn samples_are_written_as_little_endian_f32() {
        let bytes = to_le_bytes(&[1.0f32, -1.0]);

        assert_eq!(bytes.len(), 8);
        assert_eq!(&bytes[0..4], &1.0f32.to_le_bytes());
        assert_eq!(&bytes[4..8], &(-1.0f32).to_le_bytes());
    }

    /// Whatever the adapter plays has to be addressed to the same device
    /// the drop-in creates; a mismatch is silent (the stream lands on the
    /// default sink and the user hears it through their speakers).
    #[test]
    fn playback_targets_the_installed_device() {
        // `play` writes into the sink; the drop-in has to create that same
        // sink, or a persisted device would have nothing to play into.
        assert!(config::conf_contents().contains(&format!("sink_name={SINK_NAME}")));
    }
}
