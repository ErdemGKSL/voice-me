---
title: 'Drag the Settings window by its tab strip, with window controls per OS'
type: 'feature'
created: '2026-09-24'
status: 'done'
route: 'oneshot'
review_loop_iteration: 0
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** The Settings window opens with `WindowOptions::default()`. Where the compositor gives it no server-side title bar (GNOME on Wayland), it has no way to be moved and no minimize, maximize or close buttons.

**Approach:** Make the tab strip the window's title bar. Holding the left button on its empty background and dragging moves the window, and the strip carries window controls in the form each OS expects: Windows draws minimize, maximize and close at the right; macOS keeps its native traffic lights at the left; Linux draws its own buttons only when the window is client-decorated, so a desktop that already draws a native title bar never gets a second set. Tabs stay clickable and never start a move. Build it on gpui-component's `TitleBar` rather than new window-control code.

</frozen-after-approval>

## Implementation Notes

- `crates/voice-me-ui/src/settings.rs`: the `TabBar` sits in gpui-component's `TitleBar`, wrapped in an `occlude()` div so the tabs are not part of the drag area. On Windows an ancestor `WindowControlArea::Drag` would otherwise answer `HTCAPTION` over the tabs (they do not block the mouse), and on Linux pressing a tab would arm the title bar's move-on-drag. Tabs are the small pill variant: no background or bottom border of their own, and 24px tall in the 34px bar. `SettingsView::window_options()` is `TitleBar::window_options()` plus a 720×480 minimum size, so the tabs never push the controls out of the window. `set_on_close` holds what the Linux close button runs.
- `crates/voice-me-app/src/main.rs`: the Settings window opens with those options. On Linux, `TitleBar`'s close button would call `remove_window()`, which skips `on_window_should_close`. That would leave the window and view slots set, and the next "Open Settings" would open nothing. Both paths now share one `forget_window` closure.
- Decision: on Linux the window keeps the decorations gpui negotiates (server-side where the compositor offers them), as the design guides ask for native decorations. On such desktops `TitleBar` hides its own buttons, and the tab strip is a second, draggable bar under the native one. On GNOME Wayland the window is client-decorated and gets our buttons.
- Not verified visually: this container has no Vulkan driver, so the app cannot open a window here. Checked with `cargo check` and the 120 `voice-me-ui` tests, including the tab-click routing tests.

## Review Triage Log

- low (rejected): no new tests. Tab switching through real clicks is already covered by the settings routing tests, which pass with the tabs inside the title bar. The Linux close button renders only under client decorations, which the test platform does not report, and the fix would be more than a simple correction.
- medium (patched): the window controls could be pushed off-screen in a narrow window. Added a 720×480 `window_min_size`.
- medium (patched): the tab strip's styling clashed with the title bar (two backgrounds and two bottom borders). Switched to small pill tabs.
- low (accepted by design): on server-decorated Linux desktops the strip is a second bar under the native one. See the decision above.
- medium (deferred): the window has no title. It predates this change and is recorded in deferred-work.md.
- low (rejected): cleanup on close depends on calling `set_on_close`. There is one caller, and on other platforms the handler is unused by design.
- false: "the spec is incomplete". The one-shot route writes only Intent and Implementation Notes by design.
- false: "an unrelated deferred entry is bundled in". It is committed on its own.

