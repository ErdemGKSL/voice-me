---
title: 'Summon, Type, and Dismiss the Prompt Overlay'
type: 'feature'
created: '2026-09-21'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
context: []
baseline_commit: '76fbc1b7a792106e2f09a3c7aae4975fa047b46b'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** `AppEvent::HotkeyPressed` reaches the composition root and does nothing but `println!` (`main.rs:167`). There is no Prompt Overlay anywhere in the app — the product's entire interaction surface (press hotkey → type a line → Enter) is missing, and Stories 2.6 and 2.9 both hang off the Speak Action this story must emit.

**Approach:** Add a `PromptOverlayView` in `voice-me-ui` — a borderless, always-on-top, fixed-width window holding one pre-focused gpui-kit `Input` and nothing else — summoned by the composition root on `AppEvent::HotkeyPressed`. `Enter` closes it immediately and emits a new `AppEvent::SpeakRequested { text }` (logged only until Story 2.6); `Escape` or window deactivation closes it and discards the text.

## Boundaries & Constraints

**Always:** Dismissal on `Enter` is synchronous and unconditional (AD-10) — the view waits on nothing before closing, and the Speak Action leaves only via the shared `AppEvent` channel (AD-3), never a port call from the view. Exactly one overlay exists at a time: a press while one is open activates it instead of opening a second. One `Input`, no other chrome (UX-DR4), `rounded.overlay` 12px radius, gpui-kit popover-family elevation, `lg`/`xl` padding, keyboard-operable from summon to dismissal with no click (UX-DR20). Only a short fade/scale-in on summon and fade-out on dismiss (UX-DR22). The body must be structured so Epic 3's Story 3.4 can swap the `Input` for an inline notice without reshaping the view.

**Never:** No TTS, generation, notifications, or playback — Stories 2.6/2.9 own those; the `SpeakRequested` arm logs and nothing more. No second hotkey, in-overlay menu, autocomplete, command mode, resize handle, or scrim dimming. Dismissal never quits the app — tray residency (Story 2.2) is untouched. Do not modify `voice-me-hotkey-*`, `ports.rs`, or the Settings shell's tabs.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Summon | Hotkey pressed, no overlay open | Overlay opens centered, `Input` focused and empty | Window-open failure logs to stderr; app stays tray-resident |
| Type and speak | Overlay open, "hello there" typed, `Enter` | Closes at once; one `AppEvent::SpeakRequested { text: "hello there" }` | N/A |
| Empty `Enter` | Overlay open, `Input` empty, `Enter` | Closes; no `SpeakRequested` emitted | N/A |
| Escape | Overlay open, text typed, `Escape` | Closes, text discarded, no event | N/A |
| Focus loss | Overlay open, another window activated | Closes, text discarded, no event | N/A |
| Re-summon | Overlay already open, hotkey pressed again | Existing window activated; no second window; text kept | N/A |
| Speak then re-summon | `Enter` sent, hotkey pressed again | Fresh overlay with an empty `Input` | N/A |

**Decisions taken (human, 2026-09-21):**

1. **Always-on-top is a two-tier capability, mirroring Story 2.3's hotkey backends.** X11 uses `WindowKind::PopUp` — a true always-on-top, taskbar-less window. Wayland has no equivalent in this GPUI version (`PopUp` falls through to a plain xdg_toplevel, and the compositor also decides placement, ignoring "centered"), so a Wayland session gets a plain focused window that may sit below a fullscreen game. `WindowKind::LayerShell` is deliberately **not** used: GNOME/Mutter — this project's own dev session — does not implement `zwlr_layer_shell_v1`, so it would add a second unverifiable path for no gain here. The difference is documented in the README, not worked around.

</frozen-after-approval>

## Code Map

- `crates/voice-me-core/src/event.rs` -- add `SpeakRequested { text: String }` to `AppEvent`. Enum is `#[non_exhaustive]`, derives `Clone, PartialEq, Eq`; `main.rs`'s wildcard arm keeps other matches compiling.
- `crates/voice-me-ui/src/prompt_overlay.rs` (new) -- the view. Own `input: Entity<InputState>` (`gpui_kit::component::input::{Input, InputState, InputEvent}`; `InputState::new(window, cx)` is already single-line), a `FocusHandle`, and the `AppEventSender`. Focus at construction: `self.input.update(cx, |s, cx| s.focus(window, cx))`. **Enter:** `cx.subscribe_in(&input, window, ..)` matching `InputEvent::PressEnter { .. }` — gpui-base already binds `enter` in the `"Input"` key context, so add no keybinding. **Escape:** `.on_action(cx.listener(..))` for the re-exported `gpui_kit::component::input::Escape` on the wrapper div (a raw `on_key_down` loses to the inner `"Input"` context). **Blur:** `cx.observe_window_activation(window, ..)`, dismiss when `!window.is_window_active()`; keep the returned `Subscription`. Read text via `self.input.read(cx).value()`. Reuse `hotkey.rs` idioms: `div().id(..).test_support()` markers (`hotkey.rs:499`), `track_focus` (`:472`), `focus_handle` accessor (`:453`).
- `crates/voice-me-ui/src/lib.rs` -- export `PromptOverlayView`.
- `crates/voice-me-app/src/main.rs` -- add an `overlay_slot: Rc<RefCell<Option<WindowHandle<Root>>>>` beside `window_slot` (line 75) and an `open_overlay` closure shaped like `open_settings` (lines 78-130), `on_window_should_close` clearing the slot included. `WindowOptions` per Design Notes. Replace the `HotkeyPressed` log arm (line 167) with `open_overlay`; add a log-only `SpeakRequested { text }` arm.
- `crates/voice-me-ui/src/hotkey.rs` -- read-only test-harness reference (`gpui_kit::test::TestWindowExt`, `TestAppContext`, `component::Root`, `px`/`size`).
- `README.md` -- the Wayland/X11 overlay capability difference.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-core/src/event.rs` -- add `AppEvent::SpeakRequested { text: String }` -- the Speak Action's only route out of the UI (AD-3/AD-10)
- [x] `crates/voice-me-ui/src/prompt_overlay.rs`, `src/lib.rs` -- the view: focused `Input`, Enter/Escape/blur dismissal, fade/scale animation, Epic-3-ready body
- [x] `crates/voice-me-ui/src/prompt_overlay.rs` -- tests for every I/O matrix row reachable without a compositor (summon focus, Enter emits once and closes, empty Enter emits nothing, Escape discards, blur discards), asserting on a drained `AppEvent` receiver
- [x] `crates/voice-me-app/src/main.rs` -- `open_overlay` + slot, `HotkeyPressed`/`SpeakRequested` arms, per-session `WindowOptions`
- [x] `README.md` -- document that always-on-top over a fullscreen window holds on X11 but not on GNOME/Wayland

**Acceptance Criteria:**
- Given the app is tray-resident with a hotkey configured, when the hotkey is pressed, then the overlay appears with its `Input` focused and accepts typing with no click, and no Settings window opens as a side effect
- Given the overlay is open, when it is dismissed by any of the three routes, then the app stays tray-resident and the next press summons a fresh, empty overlay
- Given the overlay is open under X11, when a fullscreen window from another application holds focus, then the overlay is drawn above it
- Given `cargo test --workspace`, when the suite runs, then the overlay tests pass with no compositor, X server, or `/dev/input` access

## Implementation Notes

**The overlay's dismissal forced a quit-mode decision.** gpui's default
`QuitMode` quits the process when the last window closes on non-macOS. In a
tray-resident session the overlay is normally the *only* window, so the first
`Enter` or `Escape` would have killed the app — breaking both this story's
"dismissal never quits" boundary and Story 2.2's tray residency. The
composition root now builds the application with `QuitMode::Explicit`, which
is what `voice-me-tray-linux/examples/spike.rs` already does; the tray's
"Quit" item calls `cx.quit()` itself. **Caveat:** this also means that if the
tray fails to register, the fallback Settings window no longer quits the
process when closed, leaving it unreachable. That degraded path was not in
scope here and is worth a follow-up.

**Dismissal is two-phase, and the tests reflect that.** `Enter` sends
`AppEvent::SpeakRequested` and flips `dismissing` synchronously; the window
itself is removed `DISMISS_MS` (100ms) later, after the fade-out frame has
rendered, because `Window::remove_window` is immediate and would otherwise
make the fade impossible. `dismissing` also makes every route idempotent —
a second `Enter` during the fade-out cannot speak twice (asserted).

**`Enter` arrives as a deferred effect.** It reaches the view as an
`InputState` event through `cx.subscribe_in`, and GPUI flushes subscription
effects when the enclosing `update_window` returns — not mid-closure. The
tests therefore press the key in one `update_window` and assert the
`prompt-overlay-dismissing` marker in the next frame (`Harness::is_dismissing`).
`Escape` (an `on_action` listener) and blur run synchronously and do not need
this, but use the same helper for consistency.

**Whitespace.** The line is trimmed before the emptiness check and before
being sent, so `Enter` on a field holding only spaces dismisses without
speaking, and no leading/trailing junk reaches TTS. Both halves are asserted.

**`track_focus` on the frame is a focus trap.** GPUI registers a mouse-down
listener on any focusable element that moves focus to its tracked handle, so
a click on the overlay's *padding* would have taken focus off the `Input` and
silently ended typing. The frame claims the left mouse-down first (bubble
handlers run in reverse registration order) and re-focuses the `Input`.

**`WindowKind` is chosen per session, not per platform.** `overlay_window_kind()`
reuses `voice_me_hotkey_linux::session_kind()` (the public helper — that crate
was not modified) and returns `PopUp` on X11, `Normal` on Wayland. Asking for
`PopUp` under Wayland would not fail, it would simply behave like `Normal`;
naming `Normal` keeps the code honest about what that session actually gets.

**Stale overlay handles.** The view closes itself with `remove_window`, which
never runs `on_window_should_close`, so `overlay_slot` routinely holds a
handle to a window that is already gone. `open_overlay` treats a failing
`handle.update` as exactly that, clears the slot and opens a fresh overlay —
which is what gives "speak, then re-summon" an empty `Input` for free.

**Not verified here (needs a live session):** the three manual checks in
Verification — always-on-top over a fullscreen X11 window, the two-press
single-window behavior against a real window server, and the informal
hotkey-to-caret timing against the ~200ms NFR1 budget. The window is created
per press (per Design Notes); no measurement was taken, so the NFR1 budget is
still unconfirmed.

## Spec Change Log

## Review Triage Log

**Routed to patch**

- **The app quits when the overlay removes its window** — `high`. Verified: `main()` never sets a quit mode, and gpui's `QuitMode::Default` is quit-on-empty off macOS (`gpui-pre-0.3.5/src/app.rs:1955-1963`); nothing in `gpui_kit::application()`, `voice-me-tray-linux`, or `gpui-tray` overrides it, and the tray opens no window. In a normal tray-resident session the overlay is the only window, so the first `Enter`/`Escape` ends the process — contradicting this story's frozen boundary and Story 2.2. Pre-existing for the Settings window, but this story makes window removal routine. `voice-me-tray-linux/examples/spike.rs:17` already uses `QuitMode::Explicit`. Route: patch.
- **A click on the overlay's padding steals focus from the `Input`** — `medium`. Verified: `.track_focus` sets `focusable`, and gpui registers a mouse-down listener transferring focus to the tracked handle (`div.rs:2770-2783`). Clicking the surround leaves the field silently dead to typing and to `Enter`. Route: patch.
- **`speak`'s `trim()` is untested** — `medium`, pre-verified by the verification-gap layer. Deleting the trim keeps the suite green; the event's own doc comment promises trimmed text. Route: patch.
- **`overlay_window_kind()` is untested** — `medium`, pre-verified. Swapping its arms silently loses X11 always-on-top — acceptance criterion 3 — with a green build and suite. Route: patch, via a pure `overlay_window_kind_for`, mirroring `session_kind_for`.
- **README promises speech that does not happen, and states unverified X11 behavior as fact** — `low`, but the fix is a direct correction of a user-facing document. Route: patch.
- **The focus test leaves a live dismiss timer and an un-removed window** — `low`, direct correction, and the file was being patched anyway. Route: patch.

**Rejected**

- **A hotkey press during the 100ms fade-out is swallowed** — verified `low`, rejected. Real: the window is alive but `dismissing` for `DISMISS_MS`, so `open_overlay` activates a window that then vanishes and the press produces no overlay. But it requires re-summoning within a tenth of a second of dismissing, nothing is lost, and the next press works. Every fix needs new view→composition-root signalling (a new `AppEvent` variant or a constructor callback) for a window of time a human is unlikely to hit.
- **Blur has no "was active at least once" guard** — verified `maybe-false`; see deferred work, where it is recorded with the severity it would carry if true.
- **`PressEnter { secondary, shift }` is matched with `..`, so Shift+Enter speaks** — verified `low`, rejected. The field is single-line with no multi-line affordance, so "any Enter speaks" is a defensible reading; the fix adds a branch to encode a policy nothing asked for.
- **`InputState::escape` may consume Escape in some states** — verified `low`, rejected. The states that consume it (context menu, multi-cursor, inline completion) need surface this overlay does not have: one `Input`, no menu, no completions.
- **The window is a fixed 84px tall, so Epic 3's inline notice could clip** — verified `low`, rejected. Story 3.4 owns that surface and will size the window it needs; guarding it now means designing for a notice that does not exist.
- **The drive-by removal of `let _ =` on `cx.update` drops error visibility** — verified `false`. `AsyncApp::update` returns `R`, not `Result`, in this version (`async_context.rs:165`); the binding discarded nothing and clippy is clean without it.
- **Spec status and `sprint-status.yaml` disagree, and `baseline_commit` is no longer HEAD** — verified `false`. Both are the workflow's normal in-flight state: sprint status syncs to `review` at presentation, and `baseline_commit` deliberately pins where this story started (an unrelated architecture-docs commit landed on `main` meanwhile).

**Routed to defer**

- **`open_overlay`'s one-at-a-time and stale-handle bookkeeping has no automated test** — `medium`, pre-verified, deferred. Deleting the slot-clearing recovery would make the hotkey work exactly once per launch with a green suite. `voice-me-app` is a binary crate with no test target and no window-server harness — the same gap Stories 2.2 and 2.3 already carry for the composition root.
- **Blur dismissal has no "has been activated at least once" guard** — `maybe-false`, would be `high` if true (the overlay would self-close the instant it appears). Under GNOME/Wayland focus-stealing prevention a newly mapped window can arrive unfocused, and `observe_window_activation` would dismiss it immediately. Settled by running the built binary in a GNOME/Wayland session and pressing the hotkey — one of this story's own manual checks.
- **CI never runs `cargo test`** — `medium`, deferred. Both jobs in `.github/workflows/ci.yml` stop at `cargo build --workspace`, so every behavioral guarantee this story adds is enforced only by local runs. Repo-wide and pre-existing.

## Design Notes

`WindowOptions`: `titlebar: None`, `is_resizable`/`is_movable`/`is_minimizable: false`, `window_decorations: Some(WindowDecorations::Client)` (required for borderless on Linux), `window_background: Transparent`, `focus: true`, `window_bounds: Some(WindowBounds::centered(size(px(560.), px(84.)), cx))`. 560px is this spec's choice for the "fixed comfortable width" the design leaves unspecified. `kind` carries the Open Question's outcome; detect the session the way `voice-me-hotkey-linux` already does (`WAYLAND_DISPLAY`/`XDG_SESSION_TYPE`).

Animation: `div` has **no** transform/scale in this GPUI version (`Transformation` is svg-only), so the scale-in is faked by interpolating offset/size alongside `Styled::opacity` inside `.with_animation("prompt-in", Animation::new(Duration::from_millis(120)).with_easing(ease_out_quint()), ..)`. Fade-out needs two phases because `window.remove_window()` is immediate: set a `dismissing` flag, `cx.notify()`, then `cx.spawn` a `background_executor().timer(~100ms)` before removing. Keep the total under ~140ms.

The window is created per press rather than kept hidden and re-shown — simpler, and it gives the empty-`Input` re-summon for free. If measurement shows creation blows the ~200ms NFR1 budget, record the number in Implementation Notes rather than silently redesigning.

## Verification

**Commands:**
- `cargo check --workspace` -- expected: workspace type-checks, including the new `AppEvent` variant against existing matches
- `cargo test --workspace` -- expected: new overlay tests pass alongside existing suites
- `cargo build -p voice-me-app` -- expected: builds cleanly on Linux

**Manual checks (if no CLI):**
- Run the built binary in this GNOME/Wayland session with a hotkey configured: press it, confirm a focused caret, type, press `Enter`, confirm the window closes instantly and `SpeakRequested` logs the typed text
- Press the hotkey twice without dismissing: exactly one window exists
- Press `Escape`, and separately click another window while the overlay is open: both discard the text with nothing logged
- Time summon informally (hotkey → caret visible) against the ~200ms NFR1 target; record the observation in Implementation Notes
- Under an X11 session, confirm the overlay draws above another app's fullscreen window, and that the tray still registers (`busctl --user list`) after dismissal
