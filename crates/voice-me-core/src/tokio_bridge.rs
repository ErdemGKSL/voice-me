//! The AD-5 Tokio ↔ GPUI bridge.
//!
//! GPUI runs its own executor on the main thread; ONNX Runtime inference is
//! a long, purely blocking CPU job that must never touch it. AD-5 answers
//! that with a Tokio runtime living alongside GPUI, and this module is the
//! *one* place that owns it — a GPUI [`Global`] holding a
//! [`tokio::runtime::Handle`], plus a `spawn_blocking` wrapper whose return
//! value is a plain `Send + 'static` future, which is exactly what
//! `cx.background_spawn(..)` accepts.
//!
//! Keeping it in `voice-me-core` (rather than in `voice-me-tts`, the first
//! caller) is deliberate: playback, dependency provisioning and microphone
//! capture all need the same bridge, and two runtimes in one process would
//! be two thread pools competing for four cores.
//!
//! `voice-me-core` already depends on `gpui-kit` (default features off) for
//! `TrayPort`'s context type — see `ports.rs` — so the `Global` costs no new
//! dependency edge.

use std::future::Future;

use gpui_kit::{App, Global};
use tokio::runtime::{Handle, Runtime};

use crate::error::VoiceMeError;

/// The Tokio runtime, held as a GPUI global so it outlives the call that
/// installed it.
///
/// It stores the `Runtime` itself and not merely a `Handle`: a `Handle` is
/// just a reference, and dropping the owning `Runtime` would shut the
/// worker and blocking pools down underneath every in-flight job.
pub struct TokioRuntime {
    handle: Handle,
    /// `None` when this process did not create the runtime (tests, or an
    /// embedder that already has one). The handle is then borrowed.
    _owned: Option<Runtime>,
}

impl Global for TokioRuntime {}

impl TokioRuntime {
    /// Build a multi-threaded Tokio runtime and install it as the global.
    ///
    /// Call once, from the composition root, inside
    /// `gpui_kit::application().run(..)`.
    pub fn install(cx: &mut App) -> Result<(), VoiceMeError> {
        let runtime = Runtime::new()?;
        let handle = runtime.handle().clone();
        cx.set_global(Self {
            handle,
            _owned: Some(runtime),
        });
        Ok(())
    }

    /// Adopt an existing runtime's handle instead of creating one.
    pub fn from_handle(handle: Handle) -> Self {
        Self {
            handle,
            _owned: None,
        }
    }

    /// The handle, for callers that want [`spawn_blocking_on`] directly.
    pub fn handle(&self) -> &Handle {
        &self.handle
    }
}

/// Run `work` on Tokio's blocking pool, from inside GPUI.
///
/// The returned future is `Send + 'static`, so the caller awaits it from a
/// `cx.background_spawn(..)` task and then hops back to the main thread with
/// `cx.update(..)` to touch any GPUI state — the GPUI half of AD-5. The
/// future is created eagerly (the work is already queued when this returns),
/// so dropping it cancels nothing that has started.
///
/// Panics inside `work` surface as [`VoiceMeError::Other`] rather than
/// unwinding into GPUI's executor.
pub fn spawn_blocking<F, T>(
    cx: &App,
    work: F,
) -> impl Future<Output = Result<T, VoiceMeError>> + Send + 'static
where
    F: FnOnce() -> Result<T, VoiceMeError> + Send + 'static,
    T: Send + 'static,
{
    spawn_blocking_on(cx.global::<TokioRuntime>().handle(), work)
}

/// [`spawn_blocking`] against an explicit handle, for code that has no
/// `App` in reach (tests, and the example spike's setup path).
pub fn spawn_blocking_on<F, T>(
    handle: &Handle,
    work: F,
) -> impl Future<Output = Result<T, VoiceMeError>> + Send + 'static
where
    F: FnOnce() -> Result<T, VoiceMeError> + Send + 'static,
    T: Send + 'static,
{
    let join = handle.spawn_blocking(work);
    async move {
        match join.await {
            Ok(result) => result,
            Err(error) => Err(VoiceMeError::Other(format!(
                "background work did not finish: {error}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spawn_blocking_result_comes_back() {
        let runtime = Runtime::new().unwrap();
        let bridge = TokioRuntime::from_handle(runtime.handle().clone());

        // Awaited from a plain executor rather than from within Tokio, the
        // way GPUI's `background_spawn` task awaits it.
        let value = futures::executor::block_on(spawn_blocking_on(bridge.handle(), || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            Ok(6563_u32)
        }))
        .unwrap();

        assert_eq!(value, 6563);
    }

    #[test]
    fn a_failure_inside_the_blocking_job_comes_back_as_a_domain_error() {
        let runtime = Runtime::new().unwrap();
        let bridge = TokioRuntime::from_handle(runtime.handle().clone());

        let result: Result<(), _> =
            futures::executor::block_on(spawn_blocking_on(bridge.handle(), || {
                Err(VoiceMeError::Other("inference failed".to_string()))
            }));

        assert!(
            matches!(result, Err(VoiceMeError::Other(message)) if message == "inference failed")
        );
    }

    #[test]
    fn a_panicking_job_is_reported_rather_than_unwinding_into_gpui() {
        let runtime = Runtime::new().unwrap();
        let bridge = TokioRuntime::from_handle(runtime.handle().clone());

        let result: Result<(), _> =
            futures::executor::block_on(spawn_blocking_on(bridge.handle(), || panic!("boom")));

        assert!(result.is_err());
    }
}
