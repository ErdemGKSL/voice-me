//! The Prompt Overlay (Story 2.4): the product's whole interaction surface
//! — press the hotkey, type a line, press `Enter`.
//!
//! The view owns one `Input` and nothing else (UX-DR4). It never calls a
//! port: confirming the line sends `AppEvent::SpeakRequested` on the shared
//! channel (AD-3) and closes immediately, waiting on nothing (AD-10).
//! Generation, playback and notifications belong to Stories 2.6/2.9,
//! downstream of that event.
//!
//! Three routes out, all of which close the window and leave the app
//! tray-resident: `Enter` (speaks), `Escape` (discards) and losing window
//! activation (discards).
//!
//! Story 3.4 adds a fourth shape rather than a fourth route: the overlay
//! can open *blocked*. It still appears on the hotkey press and still
//! dismisses exactly as an unblocked one does, but it renders an inline
//! notice naming the missing dependency in place of the `Input`, and there
//! is nothing to type and nothing to send. The composition root decides
//! which shape to open; the view is told, and never asks `voice-me-deps`
//! anything itself.

use std::time::Duration;

use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, ThemeStyled as _, v_flex};
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, AppContext as _, Context, Entity, FocusHandle,
    Focusable, FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, ParentElement as _, Render, SharedString, Styled as _, Subscription,
    TestSupportExt as _, Window, div, ease_out_quint, px,
};
use voice_me_core::{AppEvent, AppEventSender};

/// Placeholder copy for the one and only field.
const PLACEHOLDER: &str = "Type what you want to say…";

/// How long the summon animation runs.
const SUMMON_MS: u64 = 120;

/// How long the fade-out runs before the window is actually removed.
///
/// `Window::remove_window` is immediate, so the only way to show a fade-out
/// at all is to render the faded frame first and remove the window after
/// this delay. Kept short deliberately: `Enter` must feel instantaneous, and
/// the `AppEvent` has already been sent by the time this starts.
const DISMISS_MS: u64 = 100;

/// The padding around the overlay surface at the start of the summon
/// animation, shrinking to [`SURFACE_INSET_END`]. `div` has no transform in
/// this GPUI version (`Transformation` is svg-only), so the "scale-in" is
/// this inset closing while the opacity rises.
const SURFACE_INSET_START: f32 = 12.;
const SURFACE_INSET_END: f32 = 6.;

/// What the overlay leads with when it is blocked: the same word Settings →
/// Dependencies uses, so the two surfaces cannot describe one gap two ways.
const BLOCKED_HEADLINE: &str = "voice-me can't speak yet";

/// The second line, pointing at the one place the whole gap is explained.
const BLOCKED_HINT: &str = "Settings → Dependencies has the details.";

/// The borderless prompt window's contents.
pub struct PromptOverlayView {
    input: Entity<InputState>,
    focus_handle: FocusHandle,
    events: AppEventSender,
    /// The missing dependency this overlay opened blocked on, if any
    /// (Story 3.4). `None` is the ordinary typeable overlay.
    blocker: Option<SharedString>,
    /// Set once a dismissal route has fired. The window is still up for
    /// [`DISMISS_MS`] while the fade-out renders, so this both drives that
    /// frame and makes every dismissal route idempotent.
    dismissing: bool,
    _subscriptions: Vec<Subscription>,
}

impl PromptOverlayView {
    /// Build the view with the `Input` already focused, so the overlay is
    /// typeable the moment it appears with no click (UX-DR20).
    pub fn new(events: AppEventSender, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::build(events, None, window, cx)
    }

    /// Build the blocked shape (Story 3.4): the overlay opens and dismisses
    /// normally but refuses input, naming `blocker` inline.
    ///
    /// `blocker` is a finished sentence rather than a dependency value: the
    /// wording belongs to `crate::dependencies::blocker_notice`, next to the
    /// tab that shows the same row, and this view's job is to display it.
    pub fn blocked(
        events: AppEventSender,
        blocker: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::build(events, Some(blocker.into()), window, cx)
    }

    fn build(
        events: AppEventSender,
        blocker: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(PLACEHOLDER));

        let subscriptions = vec![
            // `Enter` inside an `Input` is already bound by gpui-base in the
            // `"Input"` key context, and surfaces here as an event rather
            // than as a keybinding this view has to add.
            cx.subscribe_in(
                &input,
                window,
                |this, _input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.speak(window, cx);
                    }
                },
            ),
            // Activating any other window discards the line. Dismissal never
            // quits the app — the tray (Story 2.2) is untouched.
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() {
                    this.dismiss(window, cx);
                }
            }),
        ];

        let focus_handle = cx.focus_handle();
        if blocker.is_some() {
            // Nothing to type into, so focus goes to the frame instead —
            // which is also what puts `Escape` in reach: the `Input`'s key
            // context is not in the tree when the `Input` is not rendered,
            // so the frame's own key handler is the one that sees it.
            window.focus(&focus_handle, cx);
        } else {
            input.update(cx, |state, cx| state.focus(window, cx));
        }

        Self {
            input,
            focus_handle,
            events,
            blocker,
            dismissing: false,
            _subscriptions: subscriptions,
        }
    }

    /// The Speak Action. Sends the typed line and closes, in that order,
    /// without waiting on anything downstream (AD-10). An empty line is a
    /// plain dismissal: there is nothing to say.
    fn speak(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dismissing {
            return;
        }

        // A blocked overlay has no `Input` rendered, so this is
        // belt-and-braces — but it is the promise the story makes, and it
        // is cheaper to state it than to rely on the view tree to enforce
        // it: nothing is sent while a speech-engine dependency is missing.
        if self.blocker.is_some() {
            return;
        }

        let text = self.input.read(cx).value().trim().to_string();
        if !text.is_empty() {
            // A send failure means the composition root's receiver is gone,
            // i.e. the app is shutting down — closing is still the right
            // thing to do, so this is not surfaced.
            let _ = self
                .events
                .unbounded_send(AppEvent::SpeakRequested { text });
        }

        self.begin_dismiss(window, cx);
    }

    /// Close and discard whatever was typed: `Escape`, or losing activation.
    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dismissing {
            return;
        }
        self.begin_dismiss(window, cx);
    }

    fn begin_dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismissing = true;
        cx.notify();

        let timer = cx
            .background_executor()
            .timer(Duration::from_millis(DISMISS_MS));
        cx.spawn_in(window, async move |_this, cx| {
            timer.await;
            let _ = cx.update(|window, _| window.remove_window());
        })
        .detach();
    }

    /// Everything inside the overlay surface: the one `Input`, or — when a
    /// dependency the speech engine needs is missing — the inline notice
    /// that replaces it (Story 3.4).
    fn body(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(blocker) = self.blocker.clone() else {
            return Input::new(&self.input)
                .appearance(false)
                .text_size(px(16.))
                .text_color(cx.theme().popover_foreground)
                .into_any_element();
        };

        v_flex()
            .id("prompt-overlay-blocked")
            .test_support()
            .gap_1()
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(cx.theme().popover_foreground)
                    .child(BLOCKED_HEADLINE),
            )
            // The specific blocker, in words, rather than a generic
            // "dependency missing" the user would have to go and decode.
            .child(
                div()
                    .id("prompt-overlay-blocker")
                    .test_support()
                    .text_size(px(13.))
                    .text_color(cx.theme().popover_foreground)
                    .child(blocker),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child(BLOCKED_HINT),
            )
            .into_any_element()
    }
}

impl Focusable for PromptOverlayView {
    fn focus_handle(&self, _cx: &gpui_kit::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PromptOverlayView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dismissing = self.dismissing;
        let body = self.body(cx);

        let surface = v_flex()
            .id("prompt-overlay-surface")
            .test_support()
            .size_full()
            .justify_center()
            // `lg`/`xl` padding, and the overlay's own 12px radius rather
            // than the theme's general one.
            .px(px(20.))
            .py(px(16.))
            .rounded(px(12.))
            // The popover family's surface, edge and shadow — the elevation
            // every floating gpui-kit surface shares.
            .popover_style(cx)
            .child(body);

        let frame = div()
            .id("prompt-overlay")
            .test_support()
            .track_focus(&self.focus_handle)
            // `Escape` is bound in the inner `"Input"` context, which
            // propagates it; a raw `on_key_down` here would never see it.
            .on_action(cx.listener(|this, _: &Escape, window, cx| this.dismiss(window, cx)))
            // A blocked overlay renders no `Input`, so the `Escape` action
            // above — bound in the `"Input"` key context — is never
            // dispatched. The frame holds focus in that shape and sees the
            // raw key itself, so Escape dismisses exactly as it does in an
            // unblocked overlay.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.blocker.is_some() && event.keystroke.key == "escape" {
                    this.dismiss(window, cx);
                }
            }))
            // `track_focus` makes the frame focusable, and GPUI then
            // transfers focus to it on any mouse-down inside it — which
            // would take focus off the `Input` the moment someone clicks the
            // overlay's padding, silently ending their ability to type.
            // Claiming the click first (bubble handlers run in reverse
            // registration order, so this runs before GPUI's transfer) and
            // marking it handled keeps focus where it belongs.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    window.prevent_default();
                    if this.blocker.is_some() {
                        // Keep focus on the frame, which is what `Escape`
                        // is reaching this view through.
                        return;
                    }
                    this.input.update(cx, |state, cx| state.focus(window, cx));
                }),
            )
            .size_full()
            .child(surface);

        if dismissing {
            // Fade-out, rendered for `DISMISS_MS` before the window is
            // removed. The marker lets tests observe the dismissal without
            // a compositor.
            frame
                .child(div().id("prompt-overlay-dismissing").test_support())
                .p(px(SURFACE_INSET_END))
                .with_animation(
                    "prompt-out",
                    Animation::new(Duration::from_millis(DISMISS_MS)).with_easing(ease_out_quint()),
                    |el, delta| el.opacity(1. - delta),
                )
                .into_any_element()
        } else {
            frame
                .with_animation(
                    "prompt-in",
                    Animation::new(Duration::from_millis(SUMMON_MS)).with_easing(ease_out_quint()),
                    |el, delta| {
                        let inset =
                            SURFACE_INSET_START + (SURFACE_INSET_END - SURFACE_INSET_START) * delta;
                        el.opacity(delta).p(px(inset))
                    },
                )
                .into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    //! Every row of the story's I/O & Edge-Case Matrix that is reachable
    //! without a compositor, driven through real keystrokes against a real
    //! `AppEvent` channel — no X server, no `/dev/input`, no window manager.
    //!
    //! The two rows that are not here — "Summon" opening a window and
    //! "Re-summon" activating it instead of opening a second — live in the
    //! composition root's `open_overlay`, which needs a real window server
    //! to mean anything. They are covered by the story's manual checks.

    use futures::channel::mpsc;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{TestAppContext, VisualTestContext, WindowHandle, component::Root, point, size};
    use voice_me_core::AppEvent;

    use super::*;

    struct Harness {
        handle: WindowHandle<Root>,
        events: mpsc::UnboundedReceiver<AppEvent>,
    }

    impl Harness {
        /// Everything the overlay has sent so far, in order.
        fn drain(&mut self) -> Vec<AppEvent> {
            let mut events = Vec::new();
            while let Ok(event) = self.events.try_recv() {
                events.push(event);
            }
            events
        }

        /// Render one frame and report whether the overlay is on its way
        /// out.
        ///
        /// A separate frame from the keystroke on purpose: `Enter` arrives
        /// as an `InputState` event, and GPUI flushes subscription effects
        /// when the enclosing `update_window` returns, not mid-closure.
        fn is_dismissing(&self, cx: &mut TestAppContext) -> bool {
            cx.update_window(self.handle.into(), |_, window, cx| {
                window.render_frame(cx);
                window.try_find("prompt-overlay-dismissing").is_some()
            })
            .expect("the window is still up during the fade-out")
        }

        /// True once the window the overlay closed itself is actually gone.
        fn window_is_gone(&self, cx: &mut TestAppContext) -> bool {
            cx.update_window(self.handle.into(), |_, _, _| ()).is_err()
        }
    }

    fn open(cx: &mut TestAppContext) -> Harness {
        cx.update(gpui_kit::init);
        let (event_tx, event_rx) = mpsc::unbounded::<AppEvent>();
        let handle = cx.open_window(size(px(560.), px(84.)), |window, cx| {
            let view = cx.new(|cx| PromptOverlayView::new(event_tx.clone(), window, cx));
            Root::new(view, window, cx)
        });
        Harness {
            handle,
            events: event_rx,
        }
    }

    /// The Story 3.4 shape: summoned while a speech-engine dependency is
    /// missing.
    fn open_blocked(cx: &mut TestAppContext, blocker: &str) -> Harness {
        cx.update(gpui_kit::init);
        let (event_tx, event_rx) = mpsc::unbounded::<AppEvent>();
        let blocker = blocker.to_string();
        let handle = cx.open_window(size(px(560.), px(84.)), |window, cx| {
            let view = cx.new(|cx| {
                PromptOverlayView::blocked(event_tx.clone(), blocker.clone(), window, cx)
            });
            Root::new(view, window, cx)
        });
        Harness {
            handle,
            events: event_rx,
        }
    }

    /// Let the fade-out timer elapse so the window is actually removed.
    fn settle(cx: &mut TestAppContext) {
        cx.executor()
            .advance_clock(Duration::from_millis(DISMISS_MS * 2));
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn the_input_is_focused_on_summon_so_typing_needs_no_click(cx: &mut TestAppContext) {
        let mut harness = open(cx);

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("prompt-overlay-surface").is_some(),
                "the overlay draws its one surface"
            );
            assert!(
                window.try_find("prompt-overlay-dismissing").is_none(),
                "and is not dismissing yet"
            );
            // No click anywhere: text goes straight to whatever holds focus.
            window.input("hello ", cx);
            // A click on the overlay's padding (the frame's top-left corner,
            // clear of the Input) must not steal focus from the Input —
            // GPUI transfers focus to any focusable element that is clicked,
            // and the frame is focusable.
            window.click_at("prompt-overlay", point(px(2.), px(2.)), cx);
            window.input("there", cx);
            window.press("enter", cx);
        })
        .unwrap();

        assert!(harness.is_dismissing(cx));
        assert_eq!(
            harness.drain(),
            vec![AppEvent::SpeakRequested {
                text: "hello there".to_string()
            }],
            "both halves reached the channel, so the Input held focus throughout"
        );

        settle(cx);
        assert!(harness.window_is_gone(cx));
    }

    #[gpui_kit::test]
    fn the_spoken_line_is_trimmed(cx: &mut TestAppContext) {
        let mut harness = open(cx);

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.input("  hello there  ", cx);
            window.press("enter", cx);
        })
        .unwrap();

        assert_eq!(
            harness.drain(),
            vec![AppEvent::SpeakRequested {
                text: "hello there".to_string()
            }],
            "surrounding whitespace never reaches TTS"
        );

        settle(cx);
        assert!(harness.window_is_gone(cx));
    }

    #[gpui_kit::test]
    fn a_whitespace_only_line_closes_without_speaking(cx: &mut TestAppContext) {
        let mut harness = open(cx);

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.input("   ", cx);
            window.press("enter", cx);
        })
        .unwrap();

        assert!(harness.is_dismissing(cx));
        assert!(
            harness.drain().is_empty(),
            "spaces are nothing to say — the same as an empty field"
        );

        settle(cx);
        assert!(harness.window_is_gone(cx));
    }

    #[gpui_kit::test]
    fn enter_emits_the_speak_action_once_and_closes(cx: &mut TestAppContext) {
        let mut harness = open(cx);

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.input("hello there", cx);
            // Twice: a second Enter while the overlay is dismissing must not
            // speak again.
            window.press("enter", cx);
            window.press("enter", cx);
        })
        .unwrap();

        assert!(
            harness.is_dismissing(cx),
            "the view starts closing on the Enter itself — it waits on nothing"
        );
        assert_eq!(
            harness.drain(),
            vec![AppEvent::SpeakRequested {
                text: "hello there".to_string()
            }],
            "exactly one Speak Action, no matter how many times Enter is pressed"
        );

        settle(cx);
        assert!(
            harness.window_is_gone(cx),
            "the window is gone once the fade-out has run"
        );
    }

    #[gpui_kit::test]
    fn enter_on_an_empty_input_closes_without_speaking(cx: &mut TestAppContext) {
        let mut harness = open(cx);

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press("enter", cx);
        })
        .unwrap();

        assert!(harness.is_dismissing(cx));
        assert!(
            harness.drain().is_empty(),
            "there is nothing to say, so nothing is sent"
        );

        settle(cx);
        assert!(harness.window_is_gone(cx));
    }

    #[gpui_kit::test]
    fn escape_discards_the_text_and_closes(cx: &mut TestAppContext) {
        let mut harness = open(cx);

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.input("never mind", cx);
            window.press("escape", cx);
        })
        .unwrap();

        assert!(harness.is_dismissing(cx));
        assert!(
            harness.drain().is_empty(),
            "Escape discards — the typed line never leaves the view"
        );

        settle(cx);
        assert!(harness.window_is_gone(cx));
    }

    #[gpui_kit::test]
    fn a_blocked_overlay_names_the_blocker_instead_of_accepting_input(cx: &mut TestAppContext) {
        let mut harness = open_blocked(cx, "Speech model files (Q4) — Missing: /c/lm.onnx");

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("prompt-overlay-blocked").is_some(),
                "the overlay still opens — it just cannot be typed into"
            );
            assert!(
                window.try_find("prompt-overlay-blocker").is_some(),
                "and it names the specific blocker, not `dependency missing`"
            );
            // There is no Input to receive this, and Enter must send
            // nothing whether or not anything was typed.
            window.input("hello there", cx);
            window.press("enter", cx);
        })
        .unwrap();

        assert!(
            harness.drain().is_empty(),
            "a blocked overlay never reaches the Speak Action"
        );
    }

    /// The other half of "blocked, not broken": every way out of an
    /// ordinary overlay still works.
    #[gpui_kit::test]
    fn a_blocked_overlay_dismisses_on_escape_like_any_other(cx: &mut TestAppContext) {
        let mut harness = open_blocked(cx, "ONNX Runtime — Not found at /c/libort.so");

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press("escape", cx);
        })
        .unwrap();

        assert!(harness.is_dismissing(cx));
        assert!(harness.drain().is_empty());

        settle(cx);
        assert!(harness.window_is_gone(cx));
    }

    #[gpui_kit::test]
    fn an_unblocked_overlay_shows_no_notice(cx: &mut TestAppContext) {
        let harness = open(cx);

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("prompt-overlay-blocked").is_none());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn losing_window_activation_discards_the_text_and_closes(cx: &mut TestAppContext) {
        let mut harness = open(cx);

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.input("never mind", cx);
            window.activate_window();
        })
        .unwrap();
        cx.run_until_parked();

        // Another window taking focus, as far as this process can tell.
        VisualTestContext::from_window(harness.handle.into(), cx).deactivate_window();

        assert!(
            harness.is_dismissing(cx),
            "losing activation dismisses the overlay"
        );
        assert!(
            harness.drain().is_empty(),
            "blur discards — nothing is spoken"
        );

        settle(cx);
        assert!(harness.window_is_gone(cx));
    }
}
