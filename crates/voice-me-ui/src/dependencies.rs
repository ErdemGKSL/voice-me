//! Settings → Dependencies (Story 3.1): the one place the user learns what
//! the app needs and what it cannot find.
//!
//! The view owns no detection of its own. It renders the latest
//! [`DependencyReport`] the composition root handed it, and "Check again"
//! asks `DependencyProvisioningPort` for a fresh one — which comes back the
//! same way every other report does, on the `AppEvent` channel (AD-3), and
//! is pushed in through [`DependenciesView::set_outcome`]. There is one
//! report in the process and the overlay gate reads the same one.
//!
//! Status is stated in words. The `missing` next to a row is text, never a
//! colour with a meaning attached to it.

use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _,
    alert::Alert,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, TestSupportExt as _, Window, div, px,
};
use voice_me_core::{
    AppEventSender, Dependency, DependencyKind, DependencyOutcome, DependencyProvisioningPort,
    SpeechBackend, SpeechExecutionTarget, SpeechWeights,
};

/// The Dependencies tab.
pub struct DependenciesView {
    deps: Arc<dyn DependencyProvisioningPort>,
    events: AppEventSender,
    backend: SpeechBackend,
    outcome: DependencyOutcome,
}

impl DependenciesView {
    pub fn new(
        deps: Arc<dyn DependencyProvisioningPort>,
        events: AppEventSender,
        backend: SpeechBackend,
        outcome: DependencyOutcome,
    ) -> Self {
        Self {
            deps,
            events,
            backend,
            outcome,
        }
    }

    /// Replace what the tab shows. Called by the composition root when a
    /// `DependencyCheckCompleted` arrives — including the one the button
    /// below asked for, which is why the rows re-render without the user
    /// leaving the window.
    pub fn set_outcome(&mut self, outcome: DependencyOutcome, cx: &mut Context<Self>) {
        self.outcome = outcome;
        cx.notify();
    }

    /// Run the check again, off the UI thread.
    ///
    /// Not a synchronous call, even though it is only a handful of `stat`s
    /// and one audio-server query: that query connects to PipeWire, and an
    /// unresponsive audio server would freeze the Settings window on the
    /// click. The startup check runs on the same executor for the same
    /// reason.
    ///
    /// A check that *ran* reports itself by event, and reaches this view
    /// through [`Self::set_outcome`]. Only a check that could not run at
    /// all is handled here.
    fn check_again(&mut self, cx: &mut Context<Self>) {
        let deps = self.deps.clone();
        let events = self.events.clone();
        let backend = self.backend;
        let check = cx.background_spawn(async move { deps.check(backend, events) });
        cx.spawn(async move |this, cx| {
            let Err(error) = check.await else { return };
            let _ = this.update(cx, |this, cx| {
                this.set_outcome(DependencyOutcome::Failed(error.to_string()), cx);
            });
        })
        .detach();
    }

    fn row(dependency: &Dependency, cx: &mut Context<Self>) -> AnyElement {
        let missing = dependency.status.is_missing();

        v_flex()
            .id(row_marker(dependency.kind))
            .test_support()
            .gap_1()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(dependency.label.clone()),
                    )
                    // The word, not a colour: "missing" has to be readable.
                    .child(
                        div()
                            .id(status_marker(dependency.kind, missing))
                            .test_support()
                            .px_2()
                            .py(px(1.))
                            .rounded_md()
                            .bg(cx.theme().muted)
                            .text_size(px(12.))
                            .text_color(if missing {
                                cx.theme().danger
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(dependency.status.label()),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child(dependency.detail.clone()),
            )
            // Story 3.2 turns this into an Install button; until then the
            // row still has to be honest about which half it is in.
            .when(missing && !dependency.automatable, |el| {
                el.child(
                    div()
                        .id(manual_marker(dependency.kind))
                        .test_support()
                        .text_size(px(12.))
                        .text_color(cx.theme().muted_foreground)
                        .child("voice-me cannot fix this one for you."),
                )
            })
            .into_any_element()
    }
}

impl Render for DependenciesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let outcome = self.outcome.clone();

        v_flex()
            .id("dependencies-surface")
            .test_support()
            .size_full()
            .p_6()
            .gap_4()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(div().text_lg().child("Dependencies"))
            // Decision 4: a read-only line, not a disabled control. Story
            // 3.5 replaces it with a real `Select`.
            .child(
                div()
                    .id("dependencies-backend")
                    .test_support()
                    .text_color(cx.theme().muted_foreground)
                    .child(backend_summary(self.backend)),
            )
            .map(|el| match &outcome {
                DependencyOutcome::Pending => el.child(
                    div()
                        .id("dependencies-pending")
                        .test_support()
                        .text_color(cx.theme().muted_foreground)
                        .child("Checking…"),
                ),
                DependencyOutcome::Failed(reason) => el
                    .child(Alert::error(
                        "dependencies-check-failed-alert",
                        format!("The dependency check could not run: {reason}"),
                    ))
                    .child(div().id("dependencies-check-failed").test_support()),
                DependencyOutcome::Ready(report) => {
                    // A plain loop rather than `map`: the rows borrow `cx`
                    // for the theme, which a closure cannot hand back out.
                    let mut rows = Vec::with_capacity(report.dependencies.len());
                    for dependency in &report.dependencies {
                        rows.push(Self::row(dependency, cx));
                    }
                    el.child(v_flex().gap_4().children(rows))
                }
            })
            .child(
                Button::new("dependencies-check-again")
                    .primary()
                    .label("Check again")
                    .on_click(cx.listener(|this, _, _window, cx| this.check_again(cx))),
            )
    }
}

/// Decision 4's read-only line: which backend the rows below were derived
/// from, and what that choice implies.
pub fn backend_summary(backend: SpeechBackend) -> String {
    let target = match backend.target {
        SpeechExecutionTarget::Cpu => "CPU",
        SpeechExecutionTarget::WebGpu => "WebGPU",
    };
    let weights = match backend.weights {
        SpeechWeights::Q4 => "Q4",
        SpeechWeights::Fp16 => "FP16",
        SpeechWeights::Fp32 => "FP32",
    };
    let device = match backend.device {
        Some(id) => format!("device {id}"),
        None => "no device selection".to_string(),
    };
    format!("Backend: {target} — {weights} weights, {device}")
}

/// The sentence the Prompt Overlay shows when `dependency` is what stops
/// the Speak Action.
///
/// Lives here, beside the tab that shows the same row, so the overlay and
/// Settings never name the same blocker two different ways.
///
/// The row's own detail already says what is wrong ("Missing: <path>",
/// "Not found at <path>"), so this only puts the row's name in front of it
/// — restating "missing" here would make the notice say it twice.
pub fn blocker_notice(dependency: &Dependency) -> String {
    format!("{} — {}", dependency.label, dependency.detail)
}

fn row_marker(kind: DependencyKind) -> &'static str {
    match kind {
        DependencyKind::OnnxRuntime => "dependency-row-onnx-runtime",
        DependencyKind::ModelWeights => "dependency-row-model-weights",
        DependencyKind::VirtualMicrophone => "dependency-row-virtual-microphone",
    }
}

fn status_marker(kind: DependencyKind, missing: bool) -> &'static str {
    match (kind, missing) {
        (DependencyKind::OnnxRuntime, true) => "dependency-missing-onnx-runtime",
        (DependencyKind::OnnxRuntime, false) => "dependency-ready-onnx-runtime",
        (DependencyKind::ModelWeights, true) => "dependency-missing-model-weights",
        (DependencyKind::ModelWeights, false) => "dependency-ready-model-weights",
        (DependencyKind::VirtualMicrophone, true) => "dependency-missing-virtual-microphone",
        (DependencyKind::VirtualMicrophone, false) => "dependency-ready-virtual-microphone",
    }
}

fn manual_marker(kind: DependencyKind) -> &'static str {
    match kind {
        DependencyKind::OnnxRuntime => "dependency-manual-onnx-runtime",
        DependencyKind::ModelWeights => "dependency-manual-model-weights",
        DependencyKind::VirtualMicrophone => "dependency-manual-virtual-microphone",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{TestAppContext, component::Root, px, size};
    use voice_me_core::{AppEvent, VoiceMeError};

    use super::*;

    /// Counts the checks it was asked for, and can be told to fail the way
    /// an unresolvable cache root does.
    #[derive(Default)]
    struct CountingDepsPort {
        checks: AtomicUsize,
        fails_with: Option<String>,
    }

    impl DependencyProvisioningPort for CountingDepsPort {
        fn check(
            &self,
            _backend: SpeechBackend,
            _events: AppEventSender,
        ) -> Result<(), VoiceMeError> {
            self.checks.fetch_add(1, Ordering::SeqCst);
            match self.fails_with.as_ref() {
                Some(reason) => Err(VoiceMeError::Other(reason.clone())),
                None => Ok(()),
            }
        }
    }

    /// Opens the tab over `port` and clicks "Check again".
    fn click_check_again(cx: &mut TestAppContext, port: Arc<CountingDepsPort>) -> bool {
        cx.update(gpui_kit::init);
        let (event_tx, _event_rx) = futures::channel::mpsc::unbounded::<AppEvent>();
        let deps: Arc<dyn DependencyProvisioningPort> = port.clone();

        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|_| {
                DependenciesView::new(
                    deps.clone(),
                    event_tx.clone(),
                    SpeechBackend::CPU,
                    DependencyOutcome::Pending,
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("dependencies-check-again", cx);
        })
        .unwrap();
        // The check runs off the UI thread; let it land.
        cx.run_until_parked();

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.try_find("dependencies-check-failed").is_some()
        })
        .unwrap()
    }

    /// Without this, deleting the button's `on_click` leaves every other
    /// test green: nothing else exercises the asking half of the tab.
    #[gpui_kit::test]
    fn check_again_asks_the_port_for_a_fresh_report(cx: &mut TestAppContext) {
        let port = Arc::new(CountingDepsPort::default());

        let failed = click_check_again(cx, port.clone());

        assert_eq!(
            port.checks.load(Ordering::SeqCst),
            1,
            "the button's whole job is to ask for a new check"
        );
        assert!(
            !failed,
            "a check that ran reports by event — the tab keeps waiting, it does not claim failure"
        );
    }

    /// The other half: a check that could not run at all is the one result
    /// that never arrives by event, so the tab has to show it itself.
    #[gpui_kit::test]
    fn a_check_that_cannot_run_is_shown_in_the_tab(cx: &mut TestAppContext) {
        let port = Arc::new(CountingDepsPort {
            checks: AtomicUsize::new(0),
            fails_with: Some("could not resolve a cache directory".to_string()),
        });

        assert!(
            click_check_again(cx, port.clone()),
            "the reason has nowhere else to surface"
        );
        assert_eq!(port.checks.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_backend_line_states_the_selection_and_what_it_implies() {
        assert_eq!(
            backend_summary(SpeechBackend::CPU),
            "Backend: CPU — Q4 weights, no device selection"
        );
    }

    /// A check that could not run at all has to *say so* in the tab —
    /// silently showing "Checking…" forever would hide the one thing the
    /// user needs to read.
    #[gpui_kit::test]
    fn a_check_that_could_not_run_says_so_with_its_reason(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::test::TestWindowExt as _;
        use gpui_kit::{AppContext as _, component::Root, px, size};
        use voice_me_core::VoiceMeError;

        struct FailingDeps;

        impl DependencyProvisioningPort for FailingDeps {
            fn check(
                &self,
                _backend: SpeechBackend,
                _events: AppEventSender,
            ) -> Result<(), VoiceMeError> {
                Err(VoiceMeError::Other(
                    "could not resolve a cache directory".to_string(),
                ))
            }
        }

        cx.update(gpui_kit::init);
        let (event_tx, _event_rx) = futures::channel::mpsc::unbounded();
        let handle = cx.open_window(size(px(640.), px(480.)), |window, cx| {
            let view = cx.new(|_| {
                DependenciesView::new(
                    Arc::new(FailingDeps),
                    event_tx.clone(),
                    SpeechBackend::CPU,
                    DependencyOutcome::Failed("could not resolve a cache directory".to_string()),
                )
            });
            Root::new(view, window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("dependencies-check-failed").is_some(),
                "the tab states that the check itself failed"
            );
            assert!(
                window.try_find("dependencies-pending").is_none(),
                "and stops claiming it is still checking"
            );
        })
        .unwrap();
    }

    #[test]
    fn the_blocker_notice_names_the_row_and_its_detail() {
        let notice = blocker_notice(&Dependency::missing(
            DependencyKind::ModelWeights,
            "Speech model files (Q4)",
            "Missing: /cache/onnx/language_model_q4.onnx",
        ));

        assert!(notice.contains("Speech model files (Q4)"), "{notice}");
        assert!(notice.contains("language_model_q4.onnx"), "{notice}");
        assert_eq!(
            notice.matches("Missing").count(),
            1,
            "the notice says what is wrong once, not once per half: {notice}"
        );
    }
}
