---
title: 'Spike — Background/Tray Presence Feasibility on GPUI/gpui-kit'
type: 'feature'
created: '2026-09-21'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
context: []
baseline_commit: '607db041e0c54caebcf63e3ad26494d3b4e446b6'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** `voice-me-tray-linux` and `voice-me-tray-windows` are empty `todo!()` stubs. Neither GPUI nor gpui-kit provide system tray support upstream (PRD Open Question 6), so it's unverified whether the app can show a tray icon at all before the rest of Epic 2 (tray-resident process, hotkey, overlay) is built on top of that assumption.

**Approach:** Spike a real tray icon + "Quit" menu item on Linux using the `gpui-tray` crate (native GPUI/gpui-kit integration, not the unverified "Adabraka GPUI" fork), verify it manually in this dev environment, and record the decision. Windows is deferred entirely — no Windows target/toolchain exists in this dev environment (see Decision below).

## Boundaries & Constraints

**Always:** Keep the chosen tray library entirely inside `voice-me-tray-linux`/`voice-me-tray-windows`; `voice-me-core` only gains whatever `TrayPort` signature change is needed to pass a GPUI context through, never a dependency on the tray crate itself. Record the decision (crate chosen, why, fallback) in `ARCHITECTURE-SPINE.md`'s Deferred section, resolving the GPUI/gpui-kit tray sub-question of PRD Open Question 6.

**Never:** Do not wire the tray into `voice-me-app`'s normal startup/lifecycle (tray-resident behavior, Settings/Quit menu contents) — that's Story 2.2. This spike only needs a standalone way to manually confirm a tray icon renders and Quit works. Do not implement the "Adabraka GPUI" fork path — it's git-only and unverified; not worth spiking when `gpui-tray` is a published, gpui-kit-compatible alternative.

**Decision:** This dev environment has no Windows Rust target or cross-compile toolchain installed, so `voice-me-tray-windows` cannot be built or run here. Windows is deferred entirely for this spike: leave `voice-me-tray-windows` as the existing `todo!()` stub, and record only the intended approach (same `gpui-tray` crate, same pattern as Linux, Windows backend via Shell_NotifyIconW) as a plan — not code — in the architecture decision. Story 2.1's Windows AC is explicitly deferred to a later pass once Windows access exists, not attempted now.

</frozen-after-approval>

## Code Map

- `crates/voice-me-tray-linux/src/lib.rs` -- current `todo!()` stub implementing `TrayPort`; replace with a real `gpui-tray`-backed implementation.
- `crates/voice-me-tray-windows/src/lib.rs` -- current `todo!()` stub; stays untouched for this spike (Windows deferred — see Intent).
- `crates/voice-me-core/src/ports.rs` -- `TrayPort::show(&self) -> Result<(), VoiceMeError>` has no way to pass a GPUI context; `gpui-tray`'s `Tray::builder()...build(cx)` needs `cx: &mut App`. The signature needs to gain a context parameter (exact type: whatever gpui-kit re-exports as its `App`/context type — check `voice-me-ui`'s existing gpui-kit imports for the established alias).
- `crates/voice-me-tray-linux/Cargo.toml` -- add `gpui-tray` dependency with the `gpui-kit` feature enabled (disable default `gpui` feature) and gpui-kit's own `gpui` re-export as the version source, not Zed's pinned git rev.
- `_bmad-output/planning-artifacts/architecture/architecture-voice-me-2026-09-20/ARCHITECTURE-SPINE.md` -- Deferred section's tray/hotkey bullet (line ~184): update to record the resolution for tray (crate chosen, Apache-2.0, published on crates.io, gpui-kit-native menu/action dispatch, Linux via StatusNotifierItem/DBusMenu+zbus, Windows via Shell_NotifyIconW) and the fallback (`tray-icon` by tauri-apps, mature/cross-platform, requires bridging its own event receiver) if `gpui-tray` proves broken.
- No existing crate currently depends on `voice-me-tray-linux`/`-windows` (`voice-me-app` doesn't wire tray yet) — this spike needs its own small verification path (an example binary in the tray crate, e.g. `crates/voice-me-tray-linux/examples/spike.rs`) rather than touching `voice-me-app`.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-core/src/ports.rs` -- add whatever context parameter `TrayPort::show` needs to satisfy `gpui-tray`'s `build(cx)` call, document why in a doc comment -- unblocks a real (non-`todo!()`) implementation
- [x] `crates/voice-me-tray-linux/Cargo.toml`, `.../src/lib.rs` -- add `gpui-tray` (features = ["gpui-kit"], default-features = false) and implement `TrayPort` for real: icon + tooltip + a menu with one "Quit" item that calls `cx.quit()`
- [x] `crates/voice-me-tray-linux/examples/spike.rs` -- minimal `gpui_kit::application().run(...)` that opens the tray via the new adapter, for manual verification
- [x] `_bmad-output/planning-artifacts/architecture/architecture-voice-me-2026-09-20/ARCHITECTURE-SPINE.md` -- update the Deferred bullet to record the resolution (Linux, verified) and the deferred Windows plan (same crate/pattern, not yet implemented) and fallback plan

**Acceptance Criteria:**
- Given the `voice-me-tray-linux` example spike is run on this machine, when it starts, then it registers a tray item on the session DBus (`org.kde.StatusNotifierItem-<pid>-1`, confirmed via `busctl`) with a one-item "Quit" menu, and sending that menu item the same `clicked` event a real host panel sends exits the process (confirmed via a direct `com.canonical.dbusmenu.Event` call — see Implementation Notes); full pixel-level visual confirmation in a real panel is deferred (see `deferred-work.md`), since this dev environment's screenshot tooling is sandboxed
- Given the chosen approach worked (or didn't), when the spec is done, then `ARCHITECTURE-SPINE.md`'s Deferred section names the chosen crate, why, the fallback if it breaks, and the deferred Windows plan — resolving the Linux half of PRD Open Question 6 and leaving Windows explicitly open
- Given `voice-me-tray-windows` has no real tray implementation, when the spec is done, then it still builds as part of the workspace (its `TrayPort::show` stays `todo!()`, only its signature was mechanically updated to match the changed `TrayPort` trait) and nothing depends on it having real behavior yet

## Implementation Notes

- `TrayPort::show` now takes `cx: &mut gpui_kit::App`; `voice-me-core` gained a `gpui-kit` (default-features = false) dependency solely for that context type, per the Always constraint (no dependency on `gpui-tray` itself outside the adapter crates).
- `LinuxTrayAdapter` implements `TrayPort` for real using `gpui-tray` 0.1.4 (`gpui-kit` feature, `default-features = false`): a 32x32 placeholder solid-circle RGBA icon, title/tooltip "voice-me", one "Quit" menu item wired to a GPUI action that calls `cx.quit()`. The live `Tray` handle is kept alive via a `GPUI` `Global` (`TrayHandle`) — without this, the handle would drop and the icon would vanish immediately after `show()` returns, since `Tray`'s `Drop` tears the native item down.
- **`voice-me-tray-windows` build fix (not in the original diff, added during verification):** the implementing subagent left `voice-me-tray-windows`'s stub with the old one-argument `show(&self)` signature, which failed `cargo check --workspace` with E0050 (trait now requires `cx: &mut gpui_kit::App`). Fixed by updating only the stub's signature (still `todo!()` body) and adding the same `gpui-kit` dependency to `voice-me-tray-windows/Cargo.toml` — this is a mechanical signature fix to keep the workspace compiling, not a Windows implementation; Windows remains deferred per the frozen Decision.
- **Verification performed:** `cargo check --workspace` passes. `cargo build -p voice-me-tray-linux --example spike` succeeds. Running `cargo run -p voice-me-tray-linux --example spike` on this machine: the process starts, stays alive with no error or panic, prints "voice-me tray spike is running; use the tray menu to quit.", and — confirmed via `busctl --user list` while the example was running — claims the well-known DBus name `org.kde.StatusNotifierItem-<pid>-1`, exactly the freedesktop StatusNotifierItem spec's registration pattern. This is real, protocol-level proof the adapter registers correctly.
- **Quit verified end to end (added during review triage):** introspected the running example's DBusMenu directly — `com.canonical.dbusmenu.GetLayout` on the advertised `/Menu` object path returned exactly one item, id `1`, `{enabled: true, label: 'Quit'}` — then sent it the same `com.canonical.dbusmenu.Event(1, "clicked", <0>, 0)` call a real host panel sends on a real click. The process exited immediately. This exercises the exact protocol path a physical click takes, not just the code wiring, and is stronger evidence than a manual click would have been in a screen-recording sense.
- **Not captured: pixel-level visual confirmation.** This GNOME session has the AppIndicator/KStatusNotifierItem extension installed and enabled, but this dev environment's screenshot tooling (`import`/ImageMagick) is sandboxed and refused to capture the display (consistent "missing an image filename" error regardless of output path or format), so I could not visually confirm the icon rendering in the panel. Given the confirmed DBus registration, confirmed Quit round-trip, and clean process lifecycle, I'm treating this as sufficient evidence the spike succeeded; a human glancing at the top panel while running the example is the one remaining confirmation step, tracked in `deferred-work.md`.
- Icon-generation math in `tray_icon()` fixed during review triage (edge-case-hunter finding): the circle was off-center by half a pixel (`dx = x - 15` on a 32-wide buffer, true center 15.5); now uses doubled coordinates (`dx = 2*x - 31`) so it's exactly centered without floating point.

## Spec Change Log

## Review Triage Log

- **sprint-status.yaml stale story status** — `high`/`medium`/`low`: `medium`. Verified: `2-1-...` was left at `in-progress` while the spec moved to `in-review`, diverging from every other story's convention of `review` at that point. Route: patch — fixed (set to `review`).
- **`ARCHITECTURE-SPINE.md` dropped hotkey-half tracking of PRD Open Question 6** — `medium`. Verified: the replaced Deferred bullet covered tray+hotkey feasibility together; the rewrite covers only tray, and no other bullet tracks hotkey feasibility as still open (line 187 only covers anti-cheat compatibility, a different sub-risk). Route: patch — fixed (added a dedicated hotkey-half bullet).
- **Stale `gpui-tray` version string in spec Design Notes ("v0.1.1")** — `low`. Verified: `Cargo.toml` correctly uses the `"0.1"` semver range (resolves to `0.1.4` per `Cargo.lock`, not a bug); only the Design Notes prose hardcoded a stale exact version. Route: patch — fixed (now states the range and the resolved version).
- **Quit menu action never actually exercised (verification-gap, filed disposition: patch)** — `medium`, pre-verified per that layer's evidence rules. Route: patch — fixed by performing the exact protocol-level click: `com.canonical.dbusmenu.Event(1, "clicked", ...)` against the running example's `/Menu` object, confirmed via the process exiting. Recorded in Implementation Notes.
- **Pixel-level visual tray-icon confirmation never captured (verification-gap, filed disposition: defer)** — `medium` (if the icon genuinely doesn't render, this AC is unmet), pre-verified/pre-dispositioned per that layer's evidence rules — genuinely blocked by this dev environment's sandboxed screenshot tooling, not by any code gap. Route: defer — appended to `deferred-work.md`.
- **Tray icon off-center by half a pixel in `tray_icon()`** (verification-gap "Other findings", verified like any other finding) — `low`. Verified: `dx = x - 15` on a 32-wide buffer is asymmetric around the true center (15.5). Cosmetic only, on an explicitly throwaway placeholder icon. Route: patch — fixed (doubled-coordinate centering).
- **`voice-me-core` now depends on `gpui-kit` for `TrayPort`'s context type, in tension with the hexagon's platform-agnostic framing** — `low`. Verified: true, and intentional — `gpui-tray`'s `build(cx)` requires the concrete `gpui_kit::App` type; there's no object-safe generic UI-context abstraction GPUI exposes to route around it, and `gpui-kit` is the app's fixed execution substrate (already a dependency of `voice-me-ui`), not a swappable adapter. Already disclosed in three places (the port's own doc comment, Implementation Notes, `ARCHITECTURE-SPINE.md`). Rejected: fix would require introducing a new opaque-context abstraction (more than a direct correction) for a concern unlikely to confuse a reader given the existing documentation.
- **`VoiceMeError::Other(String)` used for `gpui-tray` failures, "discarding structured error information"** — verified `false`. `grep` across the codebase shows `VoiceMeError::Other(String)` is the established, codebase-wide pattern for wrapping every external-library failure (`settings_store.rs`, `voice_setup.rs` for `cpal`/wav-encoding errors) — there is no typed-variant convention it contradicts. Rejected.
- **No automated tests added** — verified `low`. The frozen Intent explicitly scopes this story's verification as a manual spike ("verify it manually in this dev environment"), which is an intent-level exclusion, not just a spec-scope choice; the only realistically-unit-testable piece (`tray_icon()`'s pixel math) is explicitly throwaway placeholder code slated for replacement. Rejected: unlikely to matter given intent's own framing, and a new test file is more than a direct correction.
- **`LinuxTrayAdapter::show` has no re-entrancy guard (double-invocation would silently replace the global `Tray` handle and double-register the `Quit` action)** — verified `low`. Real edge case, but nothing in this diff calls `show` more than once (only the new example does, once), and Story 2.2 is where real lifecycle ownership gets designed. Rejected: unlikely to be hit today, and a guard is more than a direct correction.
- **AC wording claimed `voice-me-tray-windows` was left "untouched" when its `Cargo.toml`/`lib.rs` signature necessarily changed to match the new `TrayPort` trait** — verified true, but the fix is purely a spec-text correction. Rejected per the "fix is to edit this build's spec" rule — fixed directly as housekeeping (AC reworded to describe the mechanical signature-only change).

## Design Notes

`gpui-tray` v0.1 (resolved to 0.1.4 in `Cargo.lock`; Apache-2.0, github.com/kagenokeiyou/gpui-tray, also published as crates.io `gpui-tray`) is purpose-built for GPUI apps: tray menus are plain `gpui::MenuItem`s dispatching ordinary GPUI actions via `cx.on_action`, so no separate event-loop bridge is needed (unlike the generic `tray-icon` crate, which would need its own receiver pumped from somewhere). It explicitly documents a `gpui-kit` feature flag for GPUI Kit 0.6+ (we're on 0.6.4) that avoids the default feature's pinned git dependency on Zed's own `gpui`. First implementation step should be a trivial `cargo build -p voice-me-tray-linux --no-default-features --features gpui-kit` smoke test before writing real logic, since gpui-kit compatibility is claimed but not yet verified against this exact workspace.

Sketch from the crate's own example:
```rust
let tray = Tray::builder()
    .icon(icon)
    .title("voice-me")
    .tooltip("voice-me")
    .menu(build_menu) // returns [MenuItem::action("Quit", Quit)]
    .build(cx)?; // cx: &mut App
cx.on_action(|_: &Quit, cx| cx.quit());
```

## Verification

**Commands:**
- `cargo build -p voice-me-tray-linux --example spike` -- expected: builds cleanly
- `cargo check --workspace` -- expected: whole workspace still type-checks after the `TrayPort` signature change, including the untouched `voice-me-tray-windows` stub

**Manual checks (if no CLI):**
- Run `cargo run -p voice-me-tray-linux --example spike`: confirm it claims `org.kde.StatusNotifierItem-<pid>-1` via `busctl --user list`, and that sending its `/Menu` object a `com.canonical.dbusmenu.Event(1, "clicked", <0>, 0)` call exits the process (both done during implementation/review — see Implementation Notes). Full visual confirmation (icon actually visible in a real panel) is deferred — see `deferred-work.md`.
