//! A minimal blocking wrapper over the PulseAudio client API — enough to
//! ask whether the Virtual Microphone exists, and to create or remove it
//! for the *current* session (spec-2-7 Decision 2: the drop-in only takes
//! effect when the audio server next starts, so install has to do both).
//!
//! It is deliberately synchronous. Every caller is either an install/
//! uninstall step or runs behind the AD-5 bridge, so a standard mainloop
//! driven to completion is simpler and easier to reason about than a
//! threaded one with a callback surface nobody needs.

use std::cell::RefCell;
use std::rc::Rc;

use libpulse_binding::context::{Context, FlagSet as ContextFlagSet, State as ContextState};
use libpulse_binding::mainloop::standard::{IterateResult, Mainloop};
use libpulse_binding::operation::{Operation, State as OperationState};
use libpulse_binding::proplist::Proplist;
use libpulse_binding::sample::Spec;
use libpulse_binding::stream::{FlagSet as StreamFlagSet, SeekMode, State as StreamState, Stream};
use voice_me_core::VoiceMeError;

use crate::config::{
    DEVICE_NAME, NULL_SINK_MODULE, REMAP_SOURCE_MODULE, null_sink_arguments, remap_source_arguments,
};

/// The name voice-me identifies itself by in the audio server's client list.
const CLIENT_NAME: &str = "voice-me";

fn unavailable(message: impl Into<String>) -> VoiceMeError {
    VoiceMeError::VirtualMicUnavailable(message.into())
}

/// A connected PulseAudio client session.
pub(crate) struct PulseSession {
    mainloop: Mainloop,
    context: Context,
}

impl PulseSession {
    /// Connect to the user's audio server.
    ///
    /// The common failure here is that there is no audio server at all — a
    /// CI runner, a headless box — so it is reported as a domain error
    /// naming that, never a panic.
    pub(crate) fn connect() -> Result<Self, VoiceMeError> {
        Self::connect_to(None)
    }

    /// Connect under a different client name.
    ///
    /// Playback uses this: pipewire-pulse names a stream's node after its
    /// application, so a client called `voice-me` would publish a second
    /// node under the device's own name — the duplicate state
    /// [`crate::LinuxVirtualMicAdapter::play`] refuses to play into.
    pub(crate) fn connect_as(client_name: &str) -> Result<Self, VoiceMeError> {
        Self::connect_with(None, client_name)
    }

    /// Connect to a named server instead of the user's default one.
    ///
    /// Exists so "there is no audio server" can be provoked in a test by
    /// pointing at a socket that cannot exist, rather than by mutating
    /// `PULSE_SERVER` process-wide — which is racy the moment the crate
    /// has two tests.
    pub(crate) fn connect_to(server: Option<&str>) -> Result<Self, VoiceMeError> {
        Self::connect_with(server, CLIENT_NAME)
    }

    fn connect_with(server: Option<&str>, client_name: &str) -> Result<Self, VoiceMeError> {
        let mut mainloop =
            Mainloop::new().ok_or_else(|| unavailable("could not create a PulseAudio mainloop"))?;
        let mut context = Context::new(&mainloop, client_name)
            .ok_or_else(|| unavailable("could not create a PulseAudio context"))?;

        context
            .connect(server, ContextFlagSet::NOFLAGS, None)
            .map_err(|error| unavailable(format!("no audio server to connect to: {error}")))?;

        loop {
            iterate(&mut mainloop)?;
            match context.get_state() {
                ContextState::Ready => break,
                ContextState::Failed | ContextState::Terminated => {
                    return Err(unavailable(
                        "the audio server refused the connection — is PipeWire or PulseAudio running?",
                    ));
                }
                _ => {}
            }
        }

        Ok(Self { mainloop, context })
    }

    /// How many capture sources carry this name — one is correct, two
    /// would mean install duplicated the device.
    pub(crate) fn source_count(&mut self, name: &str) -> Result<usize, VoiceMeError> {
        let count = Rc::new(RefCell::new(0usize));
        let sink = Rc::clone(&count);
        let wanted = name.to_string();

        let operation = self
            .context
            .introspect()
            .get_source_info_list(move |result| {
                if let libpulse_binding::callbacks::ListResult::Item(source) = result
                    && source.name.as_deref() == Some(wanted.as_str())
                {
                    *sink.borrow_mut() += 1;
                }
            });

        self.wait(&operation)?;
        Ok(*count.borrow())
    }

    /// Create the device for this session: the sink first, then the remap
    /// that republishes its monitor as the microphone. Order matters — the
    /// remap needs a monitor to attach to.
    pub(crate) fn load_device(&mut self) -> Result<(), VoiceMeError> {
        self.load_module(NULL_SINK_MODULE, &null_sink_arguments())?;
        self.load_module(REMAP_SOURCE_MODULE, &remap_source_arguments())?;
        Ok(())
    }

    fn load_module(&mut self, name: &str, arguments: &str) -> Result<u32, VoiceMeError> {
        let index = Rc::new(RefCell::new(None));
        let sink = Rc::clone(&index);

        let operation =
            self.context
                .introspect()
                .load_module(name, arguments, move |module_index| {
                    *sink.borrow_mut() = Some(module_index)
                });

        self.wait(&operation)?;

        match *index.borrow() {
            // The C API reports failure as the invalid-index sentinel
            // rather than through an error channel, so this is the only
            // place that can turn a failed load into a domain error.
            Some(index) if index != u32::MAX => Ok(index),
            _ => Err(unavailable(format!(
                "the audio server refused to create the Virtual Microphone ({name} {arguments})"
            ))),
        }
    }

    /// The indices of every runtime module backing our device. Matched on
    /// the module's own arguments, so a null-sink some other application
    /// loaded is never touched.
    ///
    /// Plural on purpose: two modules under the same `sink_name` publish
    /// two nodes with the same name, and that is the one state this adapter
    /// must never be in — see [`crate::LinuxVirtualMicAdapter::play`].
    pub(crate) fn loaded_null_sinks(&mut self) -> Result<Vec<u32>, VoiceMeError> {
        let found = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&found);
        // Matched on the names voice-me gives its own nodes, so a
        // null-sink or remap belonging to some other application is never
        // touched. Both spellings are checked: `sink_name=` covers the
        // playback half (and, as a prefix, the single node earlier versions
        // installed under the bare device name), `source_name=` the
        // microphone half. Missing an older module would leave two sources
        // called `voice-me`, which is the ambiguous state that routes
        // speech to the speakers.
        let sink_marker = format!("sink_name={DEVICE_NAME}");
        let source_marker = format!("source_name={DEVICE_NAME}");

        let operation = self
            .context
            .introspect()
            .get_module_info_list(move |result| {
                if let libpulse_binding::callbacks::ListResult::Item(module) = result
                    && matches!(
                        module.name.as_deref(),
                        Some(NULL_SINK_MODULE) | Some(REMAP_SOURCE_MODULE)
                    )
                    && module.argument.as_deref().is_some_and(|argument| {
                        argument.contains(sink_marker.as_str())
                            || argument.contains(source_marker.as_str())
                    })
                {
                    sink.borrow_mut().push(module.index);
                }
            });

        self.wait(&operation)?;
        Ok(found.borrow().clone())
    }

    /// Remove a runtime module by index.
    pub(crate) fn unload_module(&mut self, index: u32) -> Result<(), VoiceMeError> {
        let succeeded = Rc::new(RefCell::new(false));
        let sink = Rc::clone(&succeeded);

        let operation = self
            .context
            .introspect()
            .unload_module(index, move |success| *sink.borrow_mut() = success);

        self.wait(&operation)?;

        if *succeeded.borrow() {
            Ok(())
        } else {
            Err(unavailable(format!(
                "the audio server refused to remove the Virtual Microphone (module {index})"
            )))
        }
    }

    /// Write `data` into the device called `device`, and do not return
    /// until the server has drained it.
    ///
    /// Uses the asynchronous stream API rather than the simple one for one
    /// reason: `PA_STREAM_DONT_MOVE`. Without it, PulseAudio's
    /// `module-stream-restore` overrides the device this stream explicitly
    /// asks for with whichever device this application name last used, and
    /// once a single playback has fallen back to the speakers it is pinned
    /// there for good — the generated line comes out of the user's
    /// headphones, forever, with `target.object` still reading `voice-me`.
    /// The simple API cannot pass stream flags at all.
    ///
    /// The device is then confirmed rather than assumed: the server is
    /// asked which device the stream actually reached, and anything but the
    /// expected one is a domain error instead of audible speech. This is
    /// the guard that turns the worst failure this product has — speech the
    /// user typed *because* they cannot speak aloud, played out loud — into
    /// a notification.
    pub(crate) fn play_to(
        &mut self,
        device: &str,
        stream_name: &str,
        spec: &Spec,
        data: &[u8],
    ) -> Result<(), VoiceMeError> {
        // Opt this stream out of the session manager's saved routing.
        //
        // WirePlumber's `restore-stream` policy remembers the device a
        // stream last used, keyed on its application name, and applies it
        // at connect time — overriding the device asked for below. One
        // playback that fell back to the speakers is therefore enough to
        // pin every later utterance to them permanently, with the stream
        // still carrying `target.object = voice-me`. `state.restore-target`
        // switches that off for this stream; `state.restore-props` goes
        // with it, because a volume or mute restored from some earlier
        // session would silence the line just as effectively.
        let mut properties = Proplist::new()
            .ok_or_else(|| unavailable("could not create a stream property list"))?;
        for (key, value) in [
            ("state.restore-target", "false"),
            ("state.restore-props", "false"),
        ] {
            properties
                .set_str(key, value)
                .map_err(|_| unavailable(format!("could not set the stream property `{key}`")))?;
        }

        let mut stream =
            Stream::new_with_proplist(&mut self.context, stream_name, spec, None, &mut properties)
                .ok_or_else(|| unavailable("could not create a playback stream"))?;

        stream
            .connect_playback(Some(device), None, StreamFlagSet::DONT_MOVE, None, None)
            .map_err(|error| {
                unavailable(format!(
                    "could not open `{device}` for playback: {error} — has the Virtual                      Microphone been installed?"
                ))
            })?;

        loop {
            match stream.get_state() {
                StreamState::Ready => break,
                StreamState::Failed | StreamState::Terminated => {
                    return Err(unavailable(format!(
                        "the audio server refused a playback stream on `{device}`"
                    )));
                }
                _ => {}
            }
            iterate(&mut self.mainloop)?;
        }

        match stream.get_device_name() {
            Some(reached) if reached == device => {}
            Some(elsewhere) => {
                return Err(unavailable(format!(
                    "the audio server put this stream on `{elsewhere}` instead of \
                     `{device}` — refusing to play, because that device is audible"
                )));
            }
            None => {
                return Err(unavailable(
                    "the audio server would not say which device this stream reached \
                     — refusing to play rather than risk the speakers",
                ));
            }
        }

        let mut written = 0;
        while written < data.len() {
            let writable = stream.writable_size().unwrap_or(0);
            if writable == 0 {
                iterate(&mut self.mainloop)?;
                continue;
            }

            let end = (written + writable).min(data.len());
            stream
                .write(&data[written..end], None, 0, SeekMode::Relative)
                .map_err(|error| unavailable(format!("could not write to `{device}`: {error}")))?;
            written = end;
        }

        let operation = stream.drain(None);
        self.wait(&operation)?;
        let _ = stream.disconnect();
        Ok(())
    }

    /// Drive the mainloop until `operation` finishes, failing rather than
    /// spinning forever if the server drops the connection mid-operation.
    fn wait<T: ?Sized>(&mut self, operation: &Operation<T>) -> Result<(), VoiceMeError> {
        loop {
            match operation.get_state() {
                OperationState::Running => {}
                OperationState::Done => return Ok(()),
                OperationState::Cancelled => {
                    return Err(unavailable("the audio server cancelled the request"));
                }
            }

            iterate(&mut self.mainloop)?;

            match self.context.get_state() {
                ContextState::Failed | ContextState::Terminated => {
                    return Err(unavailable("lost the connection to the audio server"));
                }
                _ => {}
            }
        }
    }
}

impl Drop for PulseSession {
    fn drop(&mut self) {
        // Leaving a context connected at drop makes libpulse complain on
        // stderr; there is nothing to report here, so it is a plain
        // teardown.
        self.context.disconnect();
    }
}

fn iterate(mainloop: &mut Mainloop) -> Result<(), VoiceMeError> {
    match mainloop.iterate(true) {
        IterateResult::Success(_) => Ok(()),
        IterateResult::Quit(_) => Err(unavailable("the PulseAudio mainloop quit unexpectedly")),
        IterateResult::Err(error) => {
            Err(unavailable(format!("PulseAudio mainloop error: {error}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No audio server has to reach the caller as a domain error — that is
    /// CI's situation and a headless user's, and nothing here may panic on
    /// it. Provoked by pointing at a socket that cannot exist, so the test
    /// gives the same answer on a machine that *does* run an audio server.
    #[test]
    fn no_audio_server_is_a_domain_error_rather_than_a_panic() {
        let result = PulseSession::connect_to(Some("unix:/nonexistent/voice-me-pulse"));

        assert!(
            matches!(result, Err(VoiceMeError::VirtualMicUnavailable(_))),
            "a missing audio server must be reportable, not fatal"
        );
    }
}
