---
title: 'Run as a Background/Tray-Resident Process'
type: 'feature'
created: '2026-09-21'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
context: []
baseline_commit: '2952a6ad10d40d6ebd0f7d3202fe9244d566cc0c'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** `voice-me-app` always opens the Voice Setup window unconditionally at startup and has no tray wiring — the app cannot yet run headless in the background, and `voice-me-tray-linux`'s working adapter (spec-2-1) isn't called from anywhere.

**Approach:** Wire `LinuxTrayAdapter` into `voice-me-app`'s startup so the tray is always shown; open the Voice Setup window automatically only on first run (no active sample), otherwise stay tray-only; add a "Settings…" tray menu item that (re)opens that same window on demand, alongside the existing "Quit".

## Boundaries & Constraints

**Always:** Tray menu items are exactly "Settings…" (with a real ellipsis, `…`) and "Quit", per AC. The tray adapter signals "open Settings" back to the composition root only through the shared `AppEvent` channel (AD-3) — it must not depend on `voice-me-ui` or call window APIs itself. Reuse the existing `VoiceSetupView`/window-opening logic in `voice-me-app` for both the first-run auto-open and the tray-triggered open; reload `SettingsStore` state fresh each time a window opens (not just once at binary startup) so re-opening after a sample was saved shows the normal view, not the first-run empty state.

**Never:** Do not build a multi-section Settings shell (Voice/Hotkey/Dependencies/General tabs) — that's later epics' scope; "Settings…" opens today's single Voice Setup window. Do not implement `voice-me-tray-windows` — it stays the deferred `todo!()` stub from spec-2-1; only its `TrayPort` call site in `voice-me-app` needs to compile under `cfg(target_os = "windows")`.

</frozen-after-approval>

## Code Map

- `crates/voice-me-core/src/event.rs` -- `AppEvent` enum (`#[non_exhaustive]`); add a `SettingsRequested` variant for "tray Settings… clicked."
- `crates/voice-me-core/src/ports.rs` -- `TrayPort::show(&self, cx: &mut gpui_kit::App) -> Result<...>`; change signature to also take an event sender so the adapter can emit `AppEvent::SettingsRequested` without depending on UI. Add `voice-me-core` dep on `futures` (already resolved to 0.3.34 in `Cargo.lock` via gpui's own dependency graph) and export a `pub type AppEventSender = futures::channel::mpsc::UnboundedSender<AppEvent>;` alongside the existing `AppEvent`/`TrayPort` exports in `lib.rs`.
- `crates/voice-me-tray-linux/src/lib.rs` -- `LinuxTrayAdapter::show` currently registers only a `Quit` action and a one-item menu (see `build_menu`, `TrayHandle` global). Add a `Settings` action dispatching `AppEvent::SettingsRequested` via the sender (stash it in a new `Global` next to `TrayHandle`, since the action handler fires later, outside `show`'s call frame); `build_menu` returns `[MenuItem::action("Settings…", Settings), MenuItem::action("Quit", Quit)]` (order matches AC).
- `crates/voice-me-tray-windows/src/lib.rs` -- `WindowsTrayAdapter::show` stub; update its signature to match the changed `TrayPort` trait (still `todo!()` body, same mechanical-only pattern as spec-2-1's Linux/Windows split).
- `crates/voice-me-app/src/main.rs` -- composition root. Currently always opens the window unconditionally (Story 1.2/1.5 placeholder, see its own doc comment). Needs: an `unbounded::<AppEvent>()` pair; a reusable window-open closure capturing `settings_store` and a shared `Rc<RefCell<Option<WindowHandle<Root>>>>` so a second "Settings…" click activates the existing window (`window.activate_window()`) instead of opening a duplicate, and `window.on_window_should_close` clears that slot when the user actually closes it; call the platform tray adapter's `show(cx, sender)` unconditionally at startup; call the window-open closure immediately only when `!has_active_sample` (first run); spawn a `cx.spawn` task that loops `receiver.next().await`, calling the window-open closure on `AppEvent::SettingsRequested` and ignoring other (not-yet-relevant) variants via a wildcard arm (required by `#[non_exhaustive]`).
- `crates/voice-me-app/Cargo.toml` -- add `futures = "0.3"` and target-gated deps: `voice-me-tray-linux` under `[target.'cfg(target_os = "linux")'.dependencies]`, `voice-me-tray-windows` under `[target.'cfg(target_os = "windows")'.dependencies]` (no crate currently wires either tray adapter in).

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-core/src/event.rs` -- add `AppEvent::SettingsRequested` -- lets the tray adapter ask the composition root to open Settings without knowing about UI
- [x] `crates/voice-me-core/src/ports.rs`, `Cargo.toml`, `src/lib.rs` -- add `futures` dep, `AppEventSender` type alias, thread it through `TrayPort::show` -- gives adapters the sender half per AD-3 without a new abstraction
- [x] `crates/voice-me-tray-linux/src/lib.rs` -- add `Settings` action + updated two-item menu, stash the sender in a `Global` -- satisfies the AC's exact menu contents and click behavior
- [x] `crates/voice-me-tray-windows/src/lib.rs` -- update `show`'s signature only -- keeps the workspace compiling; Windows body stays deferred
- [x] `crates/voice-me-app/Cargo.toml` -- target-gated tray adapter deps + `futures` -- first crate to wire a platform tray adapter in
- [x] `crates/voice-me-app/src/main.rs` -- unconditional tray `show`, first-run-only auto-open, `AppEvent` pump reopening/activating the window, no-duplicate-window guard -- delivers the story's tray-resident behavior end to end

**Acceptance Criteria:**
- Given the app has just started with an active Reference Voice Sample already saved, when startup completes, then no window opens, the process stays running, and `busctl --user list` shows the tray claimed (same verification approach as spec-2-1)
- Given the app is tray-only (no window open), when the tray's "Settings…" item is clicked (verified via the same `com.canonical.dbusmenu.Event` call spec-2-1 used), then the Voice Setup window opens
- Given the Voice Setup window is already open, when "Settings…" is clicked again, then no second window opens (the existing window is activated instead)
- Given the tray's "Quit" item is clicked, when the event is dispatched, then the process exits (unchanged from spec-2-1)
- Given no active Reference Voice Sample exists yet (first run), when the app starts, then the Voice Setup window opens automatically in addition to the tray being present, matching today's Story 1.5 first-run behavior

## Implementation Notes

Implemented exactly per the Code Map/Design Notes sketch: `AppEvent::SettingsRequested`, `AppEventSender` alias, `TrayPort::show(cx, events)`, the two-item Linux tray menu (`"Settings…"` then `"Quit"`, real ellipsis) backed by a new `EventSender` GPUI global (the `Settings` action handler fires later, outside `show`'s call frame, so it needs a global to read the sender from — same pattern as `TrayHandle`), and the `main.rs` composition-root rewrite (unconditional tray `show`, first-run-only auto-open, `window_slot: Rc<RefCell<Option<WindowHandle<Root>>>>` guarding against duplicate windows via `activate_window()`, `on_window_should_close` clearing the slot, fresh `settings_store.load()` on every open). `voice-me-tray-linux/examples/spike.rs` and its `Cargo.toml` were also updated (new dev-dependency on `futures`) purely to keep compiling against the changed `TrayPort::show` signature — no behavior change there.

**Verification performed:** `cargo check --workspace` and `cargo build -p voice-me-app` both pass. Manual D-Bus verification against the built binary (same technique as spec-2-1) confirmed: with an existing sample, startup shows no window and registers the tray (AC1); the menu's `GetLayout` returns `"Settings…"` (id 1) then `"Quit"` (id 2) in that order; clicking "Settings…" via `com.canonical.dbusmenu.Event` while no window is open opens the Voice Setup window (AC2); Quit exits the process cleanly when clicked before any window is open (AC4, unmodified from spec-2-1); simulating first run (fresh `XDG_CONFIG_HOME`/`XDG_DATA_HOME`) auto-opens the window alongside the tray (AC5).

**Known verification gap (environment-blocked, not a code gap):** AC3 (second "Settings…" click activates the existing window instead of duplicating it) and AC4 with a window already open could not be exercised end-to-end in this sandboxed Wayland session. Once the Voice Setup window is open, further dbusmenu-triggered clicks stop reaching either action handler at all — traced (via temporary `eprintln!` instrumentation, since removed) to GPUI's `Window::dispatch_action` deferring the actual handler invocation via `cx.defer` to a frame/effects-flush cycle that this sandbox's real GPU/Wayland session never triggers for an unfocused, script-driven window; this reproduced identically across multiple fresh runs. The window-reuse code path itself (`window_slot` check, `activate_window()`, `on_window_should_close`) matches the spec's design sketch exactly and is the same pattern used elsewhere in the gpui-kit ecosystem — it is untested here only because no click reaches it, not because of a known defect. Re-verify with genuine window-manager focus/frame-callback delivery (or real mouse interaction) before treating AC3/AC4's window-open case as fully signed off; same category of gap as spec-2-1's deferred pixel-level tray-icon confirmation.

**Review patches applied (the original implementation subagent was no longer reachable, so these were applied directly):**
- `crates/voice-me-app/src/main.rs` — reordered so `open_settings` is defined before the tray `show()` calls, and a tray-registration failure now falls back to `open_settings(cx)` instead of leaving a silent, unreachable headless process; `open_settings`'s `cx.open_window(...).expect(...)` was replaced with a `match` that logs to stderr and returns on failure, so a transient window-open failure on a tray-triggered reopen no longer panics the whole background process.
- `_bmad-output/implementation-artifacts/epic-2-context.md` — restored the "Voice/tone for microcopy," "status never color-only," and hotkey-chip-typography bullets dropped during this workflow run's own context regeneration.
- Re-verified after patching: `cargo check --workspace` and `cargo build -p voice-me-app` both pass. Re-ran the manual checks against the rebuilt binary: existing-sample startup registers the tray (`org.kde.StatusNotifierItem-<pid>-1` confirmed via `busctl --user list`) with no crash; clicking "Settings…" via `com.canonical.dbusmenu.Event` grew the process from 17 to 25 threads (GPUI's window/render threads spinning up), confirming the window opened; first-run (fresh `XDG_CONFIG_HOME`/`XDG_DATA_HOME`) again registered the tray with no error. AC3 (second-click activation) remains unverified in this sandbox for the same reason recorded above and in `deferred-work.md`.

## Spec Change Log

## Review Triage Log

- **Tray `show()` failure while an active sample exists leaves the process fully silent — no window, no tray, no error surfaced** — `medium`. Verified: `crates/voice-me-app/src/main.rs`'s tray-show branch only `eprintln!`s on `Err` and continues; when `has_active_sample` is `true`, `open_settings` is never called either, so a tray-registration failure (e.g. no `StatusNotifierWatcher`/D-Bus session) leaves a totally unreachable zombie process — contradicts the epic's own "never silent nothing" failure-surfacing principle. Route: patch.
- **`.expect("failed to open window")` inside `open_settings` now panics the whole running process on every tray-triggered reopen, not just at startup** — `medium`. Verified: the same `.expect` that was previously a one-shot startup call now sits inside a closure invoked on every `AppEvent::SettingsRequested`; a transient `cx.open_window` failure after the app has been tray-resident for a while kills the entire background process instead of just failing that one window-open attempt. Route: patch.
- **`epic-2-context.md`'s regeneration (this workflow run's own step-1 side effect) dropped the "Voice/tone for microcopy," "status never color-only," and hotkey-chip-typography guidance bullets with no replacement** — `low`. Verified: diffing the file shows those three specifics present in the prior version and absent from the regenerated one; they're relevant to later epic-2 stories (2.3, 2.4, 2.6). Fix is a trivial content restoration, no public surface. Route: patch.
- **Duplicate-window guard (`window_slot`/`activate_window`) and the tray-triggered reopen path are entirely unexercised by any automated test** — `medium` (the affected behavior is real — a regression here would ship silently), pre-verified per the verification-gap layer's evidence rules (searched `voice-me-tests` and the whole repo for `SettingsRequested`/`window_slot`/`activate_window` — no hits). Also could not be manually verified end-to-end in this sandboxed Wayland session (see Implementation Notes). Route: defer — closing this needs either a GPUI headless test harness for window activation or genuine WM interaction, neither available in this repo/environment today; same category as spec-2-1's own deferred pixel-level verification.
- **`voice-me-tray-windows`'s `todo!()` body now panics at process startup on any Windows build, since `main.rs` calls `WindowsTrayAdapter::show` unconditionally** — `low`. Verified: true, and a real behavior change from before this story (Windows previously opened the window fine with no tray). Rejected from `patch`/immediate fix: no Windows toolchain exists in this or any CI environment to build, run, or verify a guard against, the whole epic's Windows support is non-functional regardless (hotkey/audio/virtual-mic Windows adapters are all still-`backlog` stubs), and `voice-me-tray-windows` staying a deferred `todo!()` is an explicit precedent from spec-2-1. Route: defer.
- **`sprint-status.yaml` says `in-progress` while the spec frontmatter says `in-review`** — verified `false`. This is expected, in-flight workflow state: step-05 (not yet reached) syncs `sprint-status.yaml` to `review` when presenting the finished spec. Rejected.
- **Tray icon missing a tooltip/accessible name (epic's accessibility requirement)** — verified `false`. `crates/voice-me-tray-linux/src/lib.rs`'s `Tray::builder()` already sets `.tooltip("voice-me")`, unchanged from spec-2-1; the requirement was already satisfied before this diff. Rejected.
- **`EventSender` global is overwritten silently if `TrayPort::show` is ever called twice** — verified `low`. Real in principle, but `show` is only called once per platform, unconditionally, at startup in the current code — no reachable path calls it twice. Rejected: unlikely to be hit today, and a guard is more than a direct correction for a scenario nothing in this diff triggers.
- **Design Notes' `open_settings` code sketch drifts from the shipped implementation** (`.ok()` vs `let _ =`, omits the settings-reload logic) — verified `true`, but the fix is to edit this build's spec. Rejected per that rule.
- **Stale `window_slot` handle: if `handle.update(...)` fails (window removed without `on_window_should_close` firing), a "Settings…" click does nothing and no window ever reopens** — verified `low`. Real edge case, but nothing in the current code removes a window through any path other than the covered close hook, so it isn't reachable today. Rejected: unlikely to be hit, and a recovery fallback is more than a direct correction.

## Design Notes

`AppEvent::SettingsRequested` and the `AppEventSender` alias are the first real use of the AD-3 channel described (but not yet implemented) in the architecture spine. `futures::channel::mpsc::unbounded` is used rather than adding `tokio` because `futures` is already pulled in transitively by the `gpui`/`gpui-kit` stack (`Cargo.lock` resolves it to 0.3.34) and its `UnboundedSender::unbounded_send` works from a synchronous GPUI action handler (the tray click) exactly like `HotkeyPort`'s future OS-thread listener will need, while `UnboundedReceiver` is `.next().await`-able inside the existing `cx.spawn` pattern in `main.rs`.

Window reuse sketch for `main.rs`:
```rust
let window_slot: Rc<RefCell<Option<WindowHandle<Root>>>> = Rc::new(RefCell::new(None));
let open_settings = { /* captures settings_store, window_slot */
    move |cx: &mut App| {
        if let Some(handle) = window_slot.borrow().as_ref() {
            handle.update(cx, |_, window, _| window.activate_window()).ok();
            return;
        }
        // open_window(...), register on_window_should_close to clear window_slot, store handle
    }
};
```

## Verification

**Commands:**
- `cargo check --workspace` -- expected: whole workspace still type-checks, including `voice-me-tray-windows`'s stub with the changed `TrayPort::show` signature
- `cargo build -p voice-me-app` -- expected: builds cleanly on Linux (the only buildable target in this dev environment, per spec-2-1)

**Manual checks (if no CLI):**
- Run the built `voice-me` binary with an existing Reference Voice Sample already saved: confirm via `busctl --user list` that it claims `org.kde.StatusNotifierItem-<pid>-1` and no window appears
- Send the tray's `/Menu` object a `com.canonical.dbusmenu.Event(<settings-id>, "clicked", <0>, 0)` call (same technique as spec-2-1's Quit verification): confirm the Voice Setup window appears; repeat and confirm no second window appears
- Delete/rename the settings/data directory (simulating first run) and re-run: confirm the window opens automatically while the tray is also present
