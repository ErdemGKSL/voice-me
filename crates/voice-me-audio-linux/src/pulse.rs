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
use voice_me_core::VoiceMeError;

use crate::config::{DEVICE_NAME, NULL_SINK_MODULE, null_sink_arguments};

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

    /// Connect to a named server instead of the user's default one.
    ///
    /// Exists so "there is no audio server" can be provoked in a test by
    /// pointing at a socket that cannot exist, rather than by mutating
    /// `PULSE_SERVER` process-wide — which is racy the moment the crate
    /// has two tests.
    pub(crate) fn connect_to(server: Option<&str>) -> Result<Self, VoiceMeError> {
        let mut mainloop =
            Mainloop::new().ok_or_else(|| unavailable("could not create a PulseAudio mainloop"))?;
        let mut context = Context::new(&mainloop, CLIENT_NAME)
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

    /// Create the device for this session. Returns the module index.
    pub(crate) fn load_null_sink(&mut self) -> Result<u32, VoiceMeError> {
        let index = Rc::new(RefCell::new(None));
        let sink = Rc::clone(&index);

        let operation = self.context.introspect().load_module(
            NULL_SINK_MODULE,
            &null_sink_arguments(),
            move |module_index| *sink.borrow_mut() = Some(module_index),
        );

        self.wait(&operation)?;

        match *index.borrow() {
            // The C API reports failure as the invalid-index sentinel
            // rather than through an error channel, so this is the only
            // place that can turn a failed load into a domain error.
            Some(index) if index != u32::MAX => Ok(index),
            _ => Err(unavailable(format!(
                "the audio server refused to create the Virtual Microphone ({NULL_SINK_MODULE} \
                 {})",
                null_sink_arguments()
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
        let marker = format!("sink_name={DEVICE_NAME}");

        let operation = self
            .context
            .introspect()
            .get_module_info_list(move |result| {
                if let libpulse_binding::callbacks::ListResult::Item(module) = result
                    && module.name.as_deref() == Some(NULL_SINK_MODULE)
                    && module
                        .argument
                        .as_deref()
                        .is_some_and(|argument| argument.contains(marker.as_str()))
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
