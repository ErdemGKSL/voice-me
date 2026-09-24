---
title: 'Choose where the Prompt Overlay appears vertically'
type: 'feature'
created: '2026-09-24'
status: 'done'
baseline_commit: 'ec41f90cde949b8019469829e5a9441a100994ff'
route: 'dispatch'
review_loop_iteration: 1
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** The Prompt Overlay always opens centred on the screen. The user wants to choose how high it appears.

**Approach:** Settings gets a horizontal slider, "Overlay position", from 0% to 100% with a default of 50%. It sets the overlay's vertical position on the screen: 0% is the top, 100% the bottom, and 50% is today's centred placement. The value is saved in `settings.toml` and used the next time the overlay opens.

**Decisions (user delegated all choices, 2026-09-24):**
1. The slider lives on the Hotkey tab, under the hotkey, as its own "Overlay position" section. The minimum Settings width fits exactly five tabs, so a sixth tab would overflow.
2. The percentage places the window within the primary display's *visible* area (taskbar and panels excluded). `y = visible.top + (visible.height − window height) × pct`, and the window stays horizontally centred. At 0% the window's top touches the visible top; at 100% its bottom touches the visible bottom.
3. It is saved when the slider is released, not on every drag step. The label shows the live percentage while dragging.
4. On Wayland the overlay is a normal toplevel, and compositors choose its position themselves. The setting then may have no effect. The slider's note says so only on Wayland sessions.

## Boundaries & Constraints

**Always:**
- Only `SettingsStore` touches `settings.toml` (AD-6). The value is lenient on load: unreadable or missing → 50, out of range → clamped to 0–100.
- The overlay reads the value at each open (it already reloads state then).

**Never:**
- No change to the overlay's size, width or horizontal centring.
- No new Settings tab.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Default | No key in settings.toml | 50 → centred vertically in the visible area | — |
| Top | 0 | Window top = visible top | — |
| Bottom | 100 | Window bottom = visible bottom | — |
| Bad value | `overlay_position = "x"` or `250` | Loads as 50 / clamped to 100; the rest of the file still loads | — |
| Save | Slider released at 30 | settings.toml `overlay_position = 30`; the next overlay open uses 30% | Save failure shows on the tab |

</frozen-after-approval>

## Code Map

- `crates/voice-me-core/src/settings_store.rs:35-98` -- `SettingsFile` top-level keys. Copy `azure_region`'s `#[serde(default, deserialize_with = "lenient", skip_serializing_if = …)]`. `read_settings_file` (:436-499) is where load-time clamping goes. `build_state` (~:515-550). `save_hotkey` (:606-611) is the save pattern. Round-trip tests from :819.
- `crates/voice-me-core/src/ports.rs:355-442` -- `SettingsStore`. Add `save_overlay_position(u8) -> Result<AppState, VoiceMeError>` and update every implementor:
  - `voice-me-core/src/settings_store.rs:563`
  - `voice-me-ui/src/voice_setup.rs:955`
  - `voice-me-ui/src/hotkey.rs:800`
  - `voice-me-ui/src/settings.rs:350`
  - `voice-me-app/src/main.rs:1276`
- `crates/voice-me-core/src/state.rs:1181,1244` -- `AppState.overlay_position: u8` (default 50) and a pure `overlay_origin_y(visible_top, visible_height, window_height, pct) -> f32` helper with tests.
- `crates/voice-me-ui/src/hotkey.rs` -- `HotkeyView` already holds `settings_store`. Add the section with `gpui_kit::component::slider::{Slider, SliderState, SliderEvent}`:
  - `SliderState::new().min(0.).max(100.).step(1.).default_value(v)`;
  - subscribe to `SliderEvent::Change` (live label) and `Release` (save);
  - pass the saved value and a `wayland: bool` in, the way `saved_hotkey` is passed (`settings.rs` `SettingsView::new`, `main.rs:3588-3634`).
- `crates/voice-me-app/src/main.rs:3750-3772` -- the overlay `WindowOptions`. Replace `WindowBounds::centered(size, cx)` with `WindowBounds::Windowed(Bounds { origin, size })`:
  - `cx.primary_display()` → `visible_bounds()`; `x` = centre − w/2; `y` from the core helper;
  - fall back to `centered` when there is no display.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-core/src/{settings_store,ports,state}.rs` -- the key, clamping, save, `AppState` field, helper. Tests for the matrix rows.
- [x] `crates/voice-me-ui/src/{hotkey,settings,voice_setup}.rs` -- the slider section (`hotkey-overlay-position`, `-slider`, `-value`, `-note`, `-error`), the fakes, and UI tests: it shows the saved value, and Release saves.
- [x] `crates/voice-me-app/src/main.rs` -- the positioned bounds at overlay open, the value passed to Settings, the Wayland flag, and the fake store.
- [x] `README.md` -- one line under the Hotkey/overlay section.

**Acceptance Criteria:**
- Given the slider is released at 20%, when the overlay is next summoned (X11 or Windows), then it opens horizontally centred with its top 20% of the way down the free vertical space.

## Implementation Notes

## Spec Change Log

## Review Triage Log

Pass 1: blind hunter (B), edge-case hunter (E), verification gap (V).

| Verdict | Finding | Evidence / route |
|---|---|---|
| medium | A failed save leaves the label and slider on an unsaved value (B, E) | `self.overlay_position` is set before the store call and never reverted → patch |
| medium | Accessibility Increment/Decrement changes are never saved; the label goes stale (B, E, V) | gpui-base `set_value` emits no event → patch, observe the slider |
| low | `x` is not clamped on a display narrower than 560 px (B, E) | Direct fix → patch |
| low | The new test took `overlay_gate`'s doc comment (B) | Direct correction → patch |
| low | The fallback doc says "centred" but gpui gives (0,0); other displays are not tried (B, E) | → patch, try `cx.displays()` first and fix the doc |
| low | The "50 = as before" doc is wrong: it now centres in the visible area (B, E) | Decision 2 chose the visible area → patch the wording |
| low | The same value is clamped four times; `try_from(..).ok()` silently falls back (B) | → patch, one load clamp and one save clamp |
| low | Exact float compare in the window-level test (B) | → patch, integer-exact values |
| low | Confirming a disclosure shrinks the window, but its top stays, so the bar drifts from the chosen spot (B, E, V) | The resize happens in `prompt_overlay.rs` without the position; repositioning needs a window-move path → defer |
| low | On X11, gpui's `visible_bounds` equals the full display, so 0% and 100% can sit under panels (E) | gpui-pre 0.3.5 overrides it only on Windows → defer (upstream) |
| medium | The composition root's use of the saved position is untested (V, filed defer) | No harness for `main()`'s closures → defer |
| low | No test at the minimum Settings size for the now-scrolling tab (B) | Cosmetic risk → rejected |

## Verification

**Commands:**
- `cargo test -p voice-me-core`, `cargo test -p voice-me-ui`, `cargo test -p voice-me-app` (one per run) -- expected: pass
