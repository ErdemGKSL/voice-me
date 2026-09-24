//! The Prompt Overlay (Story 2.4): the product's whole interaction surface
//! — press the hotkey, type a line, press `Enter`.
//!
//! Ready to type, the view is a pill-shaped prompt bar holding one `Input`,
//! a voice icon and an Enter hint, and nothing else (UX-DR4). It never calls a
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
//!
//! Story 3.6 adds a *confirm-first* shape (Decision 1). The first time a
//! remote provider is about to be used, the overlay opens naming the
//! provider and the three things that would be sent to it, with **Send to
//! <provider>** (`Enter`) and **Cancel** (`Escape`). Confirming asks the
//! composition root to record the confirmation, and the same overlay then
//! becomes the ordinary text input; cancelling dismisses it and records
//! nothing. Core's own gate stays the enforcement — this is only where the
//! user sees and answers the question.
//!
//! Story 3.14: what is listed depends on the provider ([`DisclosureText`]):
//! Azure is sent the text, the language and the voice name — never a
//! sample — and speaks in a Microsoft voice, not the user's.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Root, Sizable as _, ThemeStyled as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    kbd::Kbd,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, AppContext as _, Context, Entity, FocusHandle,
    Focusable, FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, Keystroke,
    MouseButton, MouseDownEvent, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, TestSupportExt as _, Window, div, ease_out_quint, px,
};
use voice_me_core::{AppEvent, AppEventSender, RemoteProvider};

/// The prompt bar's height, which is also its window's: the bar fills the
/// window, with nothing around it. A window geometry, so a pixel value.
pub const PROMPT_BAR_HEIGHT: f32 = 56.;

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

/// What the confirm-first shape says leaves the machine — exactly the
/// three things the adapter sends, and nothing else.
pub const DISCLOSURE_ITEMS: [&str; 3] = [
    "the text you type",
    "the language tag of your speech language",
    "your Reference Voice Sample (uploaded once, then kept on their servers until you delete it \
     in Settings → Backend or record a new sample)",
];

/// The closing line of Edge TTS's disclosure (Story 3.17).
pub const EDGE_TTS_DISCLOSURE_NOTE: &str = "Edge TTS sends the text to Microsoft's Edge Read Aloud service. It is free, needs no \
     account, and is not an official API: it may stop working at any time.";

/// What the confirm-first shape lists for one provider: exactly what
/// leaves the machine, and — for a stock-voice provider — a closing line
/// saying whose voice the speech will be in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisclosureText {
    pub items: Vec<String>,
    pub note: Option<String>,
}

impl DisclosureText {
    /// The disclosure for `provider`, whose speech language is `language`
    /// and, for a stock-voice provider, whose voice is `voice`.
    ///
    /// A cloning provider gets [`DISCLOSURE_ITEMS`]. Azure (Story 3.14, D2)
    /// gets the typed text, the language and the voice name, and says the
    /// speech will be in a Microsoft voice, not the user's. Edge TTS
    /// (Story 3.17) lists the same three and says what its service is.
    pub fn for_provider(provider: RemoteProvider, language: &str, voice: Option<&str>) -> Self {
        match provider {
            RemoteProvider::Azure => Self {
                items: vec![
                    "the text you type".to_string(),
                    format!("the speech language: {}", language.trim()),
                    format!("the voice name: {}", voice.unwrap_or("none chosen")),
                ],
                note: Some(
                    "Speech will be in this Microsoft voice, not yours. Your Reference Voice \
                     Sample is never sent."
                        .to_string(),
                ),
            },
            // Story 3.17: the same three items as Azure — no key, no
            // sample — and what the service is.
            RemoteProvider::EdgeTts => Self {
                items: vec![
                    "the text you type".to_string(),
                    format!("the speech language: {}", language.trim()),
                    format!("the voice name: {}", voice.unwrap_or("none chosen")),
                ],
                note: Some(EDGE_TTS_DISCLOSURE_NOTE.to_string()),
            },
            RemoteProvider::DeepInfra | RemoteProvider::FalAi => Self {
                items: DISCLOSURE_ITEMS
                    .iter()
                    .map(|item| item.to_string())
                    .collect(),
                note: None,
            },
        }
    }
}

/// Called once when the user confirms the disclosure (Story 3.6).
pub type ConfirmDisclosure = Rc<dyn Fn(&mut App)>;

/// The confirm-first state: which provider, what it is sent, and who
/// records the answer.
struct Disclosure {
    provider: SharedString,
    text: DisclosureText,
    on_confirm: ConfirmDisclosure,
}

/// The borderless prompt window's contents.
pub struct PromptOverlayView {
    input: Entity<InputState>,
    focus_handle: FocusHandle,
    events: AppEventSender,
    /// The missing dependency this overlay opened blocked on, if any
    /// (Story 3.4). `None` is the ordinary typeable overlay.
    blocker: Option<SharedString>,
    /// The confirm-first state (Story 3.6), until it is answered.
    disclosure: Option<Disclosure>,
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

    /// Build the confirm-first shape (Story 3.6, Decision 1): name
    /// `provider` and what would be sent to it (`text`, per provider since
    /// Story 3.14). **Send to <provider>** / `Enter` calls `on_confirm`
    /// once and turns this into the ordinary overlay; **Cancel** / `Escape`
    /// dismisses it and records nothing.
    pub fn confirm_disclosure(
        events: AppEventSender,
        provider: impl Into<SharedString>,
        text: DisclosureText,
        on_confirm: ConfirmDisclosure,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::build(events, None, window, cx);
        view.disclosure = Some(Disclosure {
            provider: provider.into(),
            text,
            on_confirm,
        });
        // As in the blocked shape: no `Input` is rendered yet, so the frame
        // holds focus and sees `Enter` and `Escape` itself.
        window.focus(&view.focus_handle, cx);
        view
    }

    /// The user confirmed: record it through the root, once, and become
    /// the ordinary overlay.
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dismissing {
            return;
        }
        let Some(disclosure) = self.disclosure.take() else {
            return;
        };
        (disclosure.on_confirm)(cx);
        // The confirm-first card needed a taller window; the bar is its own
        // height, and would otherwise stretch into a tall pill.
        let width = window.viewport_size().width;
        window.resize(gpui_kit::size(width, px(PROMPT_BAR_HEIGHT)));
        self.input.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
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
            disclosure: None,
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
        if self.blocker.is_some() || self.disclosure.is_some() {
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

    /// The inline notice that replaces the `Input` when a dependency the
    /// speech engine needs is missing (Story 3.4).
    fn blocked_body(&self, blocker: SharedString, cx: &mut Context<Self>) -> AnyElement {
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

impl PromptOverlayView {
    /// The confirm-first body: who, exactly what, and the two answers.
    fn disclosure_body(
        &self,
        provider: &SharedString,
        text: &DisclosureText,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let items = text.items.iter().map(|item| {
            div()
                .text_size(px(12.))
                .text_color(cx.theme().popover_foreground)
                .child(format!("• {item}"))
        });
        let note = text.note.clone().map(|note| {
            div()
                .id("prompt-overlay-disclosure-note")
                .test_support()
                .text_size(px(12.))
                .text_color(cx.theme().muted_foreground)
                .child(note)
        });
        v_flex()
            .id("prompt-overlay-disclosure")
            .test_support()
            .gap_1()
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(cx.theme().popover_foreground)
                    .child(format!("Speech will be generated by {provider}.")),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("voice-me sends {provider} only:")),
            )
            .children(items)
            .children(note)
            .child(
                h_flex()
                    .pt_1()
                    .gap_2()
                    .child(
                        Button::new("prompt-overlay-disclosure-confirm")
                            .primary()
                            .small()
                            .label(format!("Send to {provider}"))
                            .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))),
                    )
                    .child(
                        Button::new("prompt-overlay-disclosure-cancel")
                            .ghost()
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| this.dismiss(window, cx))),
                    ),
            )
            .into_any_element()
    }
}

impl PromptOverlayView {
    /// The blocked and confirm-first shapes: a card with room for a few
    /// lines of text and, for the disclosure, its two buttons.
    fn card(&self, body: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
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
            .child(body)
            .into_any_element()
    }

    /// Whether the overlay is ready to take a line: not blocked, and not
    /// asking to confirm a provider first.
    fn is_prompt(&self) -> bool {
        self.blocker.is_none() && self.disclosure.is_none()
    }

    /// The prompt shape: the overlay is the bar itself — a pill holding a
    /// voice icon, the one `Input`, and the Enter hint. Its edge takes the
    /// focus-ring colour while the input has focus (DESIGN.md's overlay
    /// focus emphasis). No shadow: the bar fills its window, which would
    /// clip one into a haze in the corners around the rounded ends.
    fn prompt_bar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let focused = self.input.focus_handle(cx).is_focused(window);
        h_flex()
            .id("prompt-overlay-bar")
            .test_support()
            // The window's height, never more than the bar's own: a window
            // whose client area comes out shorter than asked still holds
            // the whole bar, and the confirm-first window, taller until the
            // compositor applies its resize, still gets a bar, not a pill
            // stretched to its height.
            .size_full()
            .max_h(px(PROMPT_BAR_HEIGHT))
            .px_5()
            .gap_3()
            .bg(cx.theme().popover)
            .text_color(cx.theme().popover_foreground)
            .border_1()
            .border_color(if focused {
                cx.theme().ring
            } else {
                cx.theme().border
            })
            .rounded_full_style(cx)
            .child(Icon::new(IconName::AudioLines).text_color(cx.theme().muted_foreground))
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.input)
                        .appearance(false)
                        .text_size(px(16.))
                        .text_color(cx.theme().popover_foreground),
                ),
            )
            // What Enter does here is speak the line; the key is named in
            // the platform's own notation.
            .children(Keystroke::parse("enter").ok().map(|enter| {
                Kbd::new(enter)
                    .appearance(false)
                    .text_color(cx.theme().muted_foreground)
            }))
            .into_any_element()
    }
}

impl PromptOverlayView {
    /// The overlay window's `Root`.
    ///
    /// Not bordered: on Linux, `Root` wraps a client-decorated window in
    /// `window_border`, which reserves a 20px shadow margin on every side
    /// and draws its own frame — on a 56px window that leaves a 16px strip
    /// for the bar, framed by a square. The overlay draws its own edge.
    ///
    /// Transparent: `Root` otherwise fills the whole window with the theme
    /// background, which shows as a square behind the pill's rounded ends
    /// (and behind the card's corners).
    pub fn root(view: Entity<Self>, window: &mut Window, cx: &mut Context<Root>) -> Root {
        Root::new(view, window, cx)
            .bordered(false)
            .bg(gpui_kit::transparent_black())
    }
}

impl Focusable for PromptOverlayView {
    fn focus_handle(&self, _cx: &gpui_kit::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PromptOverlayView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dismissing = self.dismissing;
        let is_prompt = self.is_prompt();

        let surface = match (self.disclosure.as_ref(), self.blocker.clone()) {
            (Some(disclosure), _) => {
                let body = self.disclosure_body(&disclosure.provider, &disclosure.text, cx);
                self.card(body, cx)
            }
            (None, Some(blocker)) => {
                let body = self.blocked_body(blocker, cx);
                self.card(body, cx)
            }
            (None, None) => self.prompt_bar(window, cx),
        };

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
                let key = event.keystroke.key.as_str();
                if (this.blocker.is_some() || this.disclosure.is_some()) && key == "escape" {
                    this.dismiss(window, cx);
                } else if this.disclosure.is_some() && key == "enter" {
                    this.confirm(window, cx);
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
                    if this.blocker.is_some() || this.disclosure.is_some() {
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
                .when(!is_prompt, |el| el.p(px(SURFACE_INSET_END)))
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
                    move |el, delta| {
                        // The bar fills its window, so it only fades in; a
                        // card closes its inset as it does.
                        if is_prompt {
                            return el.opacity(delta);
                        }
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
        let handle = cx.open_window(size(px(560.), px(PROMPT_BAR_HEIGHT)), |window, cx| {
            let view = cx.new(|cx| PromptOverlayView::new(event_tx.clone(), window, cx));
            PromptOverlayView::root(view, window, cx)
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
            PromptOverlayView::root(view, window, cx)
        });
        Harness {
            handle,
            events: event_rx,
        }
    }

    /// Ready to type, the overlay is the bar itself: it fills its window
    /// once summoned, with no card or inset around it, and holds the input
    /// and the Enter hint. A blocked overlay keeps its card instead.
    #[gpui_kit::test]
    fn the_prompt_shape_is_a_bar_filling_its_window(cx: &mut TestAppContext) {
        let harness = open(cx);
        // Let the summon fade finish.
        cx.executor()
            .advance_clock(Duration::from_millis(SUMMON_MS + 50));
        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let bar = window.find("prompt-overlay-bar").bounds();
            assert_eq!(bar.origin, point(px(0.), px(0.)), "nothing around the bar");
            assert_eq!(bar.size, size(px(560.), px(PROMPT_BAR_HEIGHT)));
            assert!(window.try_find("prompt-overlay-surface").is_none());
        })
        .unwrap();

        let blocked = open_blocked(cx, "ONNX Runtime is missing.");
        cx.update_window(blocked.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("prompt-overlay-surface").is_some());
            assert!(window.try_find("prompt-overlay-bar").is_none());
        })
        .unwrap();
    }

    /// A window whose client area comes out shorter than the bar asked for
    /// (window chrome the platform kept) still holds the whole bar: it
    /// shrinks to fit rather than running off the bottom.
    #[gpui_kit::test]
    fn the_bar_fits_a_window_shorter_than_asked(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (event_tx, _event_rx) = mpsc::unbounded::<AppEvent>();
        let handle = cx.open_window(size(px(560.), px(40.)), |window, cx| {
            let view = cx.new(|cx| PromptOverlayView::new(event_tx.clone(), window, cx));
            PromptOverlayView::root(view, window, cx)
        });
        cx.executor()
            .advance_clock(Duration::from_millis(SUMMON_MS + 50));
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let bar = window.find("prompt-overlay-bar").bounds();
            assert_eq!(bar.size, size(px(560.), px(40.)), "{bar:?}");
        })
        .unwrap();
    }

    /// The Story 3.6 shape, with a counter standing in for the root's
    /// "record the confirmation" callback.
    fn open_disclosure(cx: &mut TestAppContext) -> (Harness, Rc<std::cell::Cell<usize>>) {
        cx.update(gpui_kit::init);
        let (event_tx, event_rx) = mpsc::unbounded::<AppEvent>();
        let confirmed = Rc::new(std::cell::Cell::new(0));
        let on_confirm: ConfirmDisclosure = {
            let confirmed = confirmed.clone();
            Rc::new(move |_cx| confirmed.set(confirmed.get() + 1))
        };
        let handle = cx.open_window(size(px(560.), px(176.)), |window, cx| {
            let view = cx.new(|cx| {
                PromptOverlayView::confirm_disclosure(
                    event_tx.clone(),
                    "DeepInfra",
                    DisclosureText::for_provider(RemoteProvider::DeepInfra, "tr", None),
                    on_confirm.clone(),
                    window,
                    cx,
                )
            });
            PromptOverlayView::root(view, window, cx)
        });
        (
            Harness {
                handle,
                events: event_rx,
            },
            confirmed,
        )
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
                window.try_find("prompt-overlay-bar").is_some(),
                "the overlay draws its one bar"
            );
            assert!(
                window.try_find("prompt-overlay-dismissing").is_none(),
                "and is not dismissing yet"
            );
            // No click anywhere: text goes straight to whatever holds focus.
            window.input("hello ", cx);
            // A click on the bar outside the Input (on its voice icon) must
            // not steal focus from the Input —
            // GPUI transfers focus to any focusable element that is clicked,
            // and the frame is focusable.
            window.click_at(
                "prompt-overlay-bar",
                point(px(24.), px(PROMPT_BAR_HEIGHT / 2.)),
                cx,
            );
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

    /// Decision 1: the provider and what is sent are named before
    /// anything else; Enter confirms exactly once, and the same overlay
    /// then takes the line.
    #[gpui_kit::test]
    fn enter_confirms_the_disclosure_once_then_the_overlay_takes_the_line(cx: &mut TestAppContext) {
        let (mut harness, confirmed) = open_disclosure(cx);

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("prompt-overlay-disclosure").is_some());
            assert!(
                window
                    .try_find("prompt-overlay-disclosure-confirm")
                    .is_some()
            );
            // Typed before confirming: there is no Input to receive it.
            window.input("too early", cx);
            window.press("enter", cx);
        })
        .unwrap();
        assert_eq!(confirmed.get(), 1);
        assert!(harness.drain().is_empty(), "confirming speaks nothing");

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("prompt-overlay-disclosure").is_none());
            // The card's taller window asks to shrink to the bar, and the
            // bar is its own height either way: the same pill a fresh
            // overlay shows, never a stretched one.
            assert_eq!(window.bounds().size.height, px(PROMPT_BAR_HEIGHT));
            let bar = window.find("prompt-overlay-bar").bounds();
            assert_eq!(bar.size.height, px(PROMPT_BAR_HEIGHT), "{bar:?}");
            window.input("merhaba", cx);
            window.press("enter", cx);
        })
        .unwrap();

        assert_eq!(confirmed.get(), 1, "confirmed once, not per Enter");
        assert_eq!(
            harness.drain(),
            vec![AppEvent::SpeakRequested {
                text: "merhaba".to_string()
            }]
        );
    }

    #[gpui_kit::test]
    fn the_send_button_confirms_once(cx: &mut TestAppContext) {
        let (mut harness, confirmed) = open_disclosure(cx);

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("prompt-overlay-disclosure-confirm", cx);
        })
        .unwrap();
        cx.run_until_parked();

        assert_eq!(confirmed.get(), 1);
        assert!(
            !harness.is_dismissing(cx),
            "it becomes the input, not closed"
        );
        assert!(harness.drain().is_empty());
    }

    /// Cancel records nothing and closes, like any other dismissal.
    #[gpui_kit::test]
    fn escape_cancels_the_disclosure_and_records_nothing(cx: &mut TestAppContext) {
        let (mut harness, confirmed) = open_disclosure(cx);

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press("escape", cx);
            window.press("enter", cx);
        })
        .unwrap();

        assert!(harness.is_dismissing(cx));
        assert_eq!(confirmed.get(), 0, "a dismissed overlay confirms nothing");
        assert!(harness.drain().is_empty());

        settle(cx);
        assert!(harness.window_is_gone(cx));
    }

    #[gpui_kit::test]
    fn the_cancel_button_records_nothing(cx: &mut TestAppContext) {
        let (harness, confirmed) = open_disclosure(cx);

        cx.update_window(harness.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("prompt-overlay-disclosure-cancel", cx);
        })
        .unwrap();
        cx.run_until_parked();

        assert!(harness.is_dismissing(cx));
        assert_eq!(confirmed.get(), 0);
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

    /// Story 3.14 (D2): Azure's disclosure names the text, the language and
    /// the voice, and says the speech is a Microsoft voice; DeepInfra's
    /// still names the sample.
    #[test]
    fn the_disclosure_items_are_per_provider_and_name_azures_voice() {
        let azure =
            DisclosureText::for_provider(RemoteProvider::Azure, "tr-TR", Some("tr-TR-EmelNeural"));
        assert_eq!(azure.items.len(), 3);
        assert!(azure.items[0].contains("text you type"));
        assert!(azure.items[1].contains("tr-TR"));
        assert!(azure.items[2].contains("tr-TR-EmelNeural"));
        assert!(
            !azure.items.iter().any(|item| item.contains("Sample")),
            "Azure is never sent the sample: {azure:?}"
        );
        assert!(
            azure
                .note
                .as_deref()
                .unwrap()
                .contains("Microsoft voice, not yours")
        );

        let deepinfra = DisclosureText::for_provider(RemoteProvider::DeepInfra, "tr", None);
        assert_eq!(
            deepinfra.items,
            DISCLOSURE_ITEMS.map(str::to_string).to_vec()
        );
        assert_eq!(deepinfra.note, None);
    }

    /// Story 3.17: Edge TTS lists the text, the language and the voice,
    /// never a sample or a key, and says what its service is.
    #[test]
    fn the_edge_tts_disclosure_names_the_voice_and_the_service() {
        let edge = DisclosureText::for_provider(
            RemoteProvider::EdgeTts,
            "tr-TR",
            Some("tr-TR-AhmetNeural"),
        );
        assert_eq!(
            edge.items,
            vec![
                "the text you type".to_string(),
                "the speech language: tr-TR".to_string(),
                "the voice name: tr-TR-AhmetNeural".to_string(),
            ]
        );
        assert_eq!(
            edge.note.as_deref(),
            Some(
                "Edge TTS sends the text to Microsoft's Edge Read Aloud service. It is free, \
                 needs no account, and is not an official API: it may stop working at any time."
            )
        );
    }
}
