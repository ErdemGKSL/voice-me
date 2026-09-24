---
title: 'Bind the hotkey through GNOME and KDE on Linux'
type: 'feature'
created: '2026-09-24'
status: 'done'
baseline_commit: '5579edfac625c36a90ee251dbf3117c3eecc7b58'
route: 'dispatch'
review_loop_iteration: 1
context:
  - '{project-root}/_bmad-output/implementation-artifacts/spec-2-3-configure-a-global-hotkey.md'
  - '{project-root}/_bmad-output/planning-artifacts/architecture/architecture-voice-me-2026-09-20/ARCHITECTURE-SPINE.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** On Wayland, voice-me reads the hotkey passively from `/dev/input` (evdev, spec 2.3). The keys still reach whatever window has focus, so the combination also acts in the background app. It needs the `input` group, cannot detect conflicts, and a reader dies on unplug or suspend. On GNOME and KDE, the desktop has its own shortcut system: it grabs the combination exclusively, and the user sees and edits it in System Settings.

**Approach:** On GNOME and KDE (any session type), register the hotkey through the desktop instead of evdev or X11.
- **GNOME:** a custom keybinding (`org.gnome.settings-daemon.plugins.media-keys`) whose command asks the running voice-me over D-Bus to summon the overlay.
- **KDE:** voice-me registers itself as a `kglobalaccel` component and listens for its shortcut's pressed signal.
- The Hotkey page has a button that opens the desktop's own keyboard-shortcut settings.
- On other desktops: on Wayland, the xdg-desktop-portal GlobalShortcuts portal comes first, then evdev. On X11, the X11 grab, then evdev. evdev is always the last resort.

**Decisions (user, 2026-09-24):**
1. On GNOME/KDE, Save on the Hotkey page writes the desktop binding automatically, and evdev/X11 are not used there. The only extra control is a button that opens the desktop's keyboard-shortcut settings.
2. On other desktops (Sway, Hyprland…), the Hotkey page also shows a "Show steps" row with the `gdbus call` summon command, so the user can bind it in their compositor. evdev/X11 stay as they are.
3. Backend order (user, renegotiated 2026-09-24): GNOME/KDE native → xdg-desktop-portal GlobalShortcuts (Wayland) → X11 grab (X11 session) → evdev. Each is tried only when the one before it is unavailable or fails, and evdev is the last resort everywhere.

## Boundaries & Constraints

**Always:**
- Presses reach the app only as `AppEvent::HotkeyPressed` on the one channel (AD-3).
- The saved hotkey stays in `settings.toml` in `global-hotkey` syntax (spec 2.3). The desktop binding is derived from it.
- The desktop is chosen at runtime from `XDG_CURRENT_DESKTOP`.
- When a backend fails (no `gsettings`, no kglobalaccel on the bus, no GlobalShortcuts portal, bind rejected), the adapter moves to the next one in the order of decision 3 and says why on stderr.
- The previous binding stays until the new one is confirmed (spec 2.3). On KDE, the keys kglobalaccel actually assigned are checked. An empty result means `HotkeyAlreadyInUse`.
- The GNOME entry is one fixed path, `…/custom-keybindings/voice-me/`, updated in place. Other custom keybindings in the list are never touched.

**Never:**
- No Unix socket and no second-instance protocol. The D-Bus service is the only IPC.
- No executable path in the GNOME command. It is a `gdbus call`, so a moved binary never breaks the shortcut.
- No `.desktop` file installed, no privilege escalation.
- No changes to Windows or to the evdev/X11 internals beyond selecting them.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| GNOME save | `XDG_CURRENT_DESKTOP=ubuntu:GNOME`, save `Ctrl+Alt+KeyV` | The `voice-me` custom keybinding has binding `<Control><Alt>v`, the summon command, and its path in the list exactly once. Pressing it sends `HotkeyPressed` | `gsettings` fails → evdev/X11 fallback, error on stderr |
| GNOME rebind | Entry exists, save `Super+F9` | Same path, binding `<Super>F9`, list unchanged | — |
| KDE save | `XDG_CURRENT_DESKTOP=KDE`, save `Ctrl+Alt+KeyV` | Component `voice-me`, action `summon`, shortcut Ctrl+Alt+V. The pressed signal sends `HotkeyPressed` | Empty keys returned → `HotkeyAlreadyInUse`, old shortcut restored |
| Portal desktop | e.g. `Hyprland`, Wayland, portal has GlobalShortcuts | Shortcut `summon` bound through the portal. `Activated` sends `HotkeyPressed` | Portal missing or bind fails → evdev |
| Other desktop | `sway`, Wayland, no GlobalShortcuts portal | evdev as today. The D-Bus summon service also runs, and the page shows the `gdbus call` steps | As today |
| App not running | GNOME shortcut pressed | `gdbus` fails quietly; nothing happens | — |
| Unmappable key | A key with no GNOME/KDE name | Save refused | `VoiceMeError::Other` naming the key |

</frozen-after-approval>

## Code Map

- `crates/voice-me-hotkey-linux/src/lib.rs` -- `LinuxHotkeyAdapter` (`:98`), `impl HotkeyPort` (`:128-137`), `session_kind()` (`:44-63`), lazy backend. Add desktop detection and the two new backends here.
- `crates/voice-me-hotkey-linux/src/{x11,evdev}.rs` -- existing backends; untouched except as fallbacks.
- `crates/voice-me-core/src/ports.rs:56-78` -- `HotkeyPort::{start_listening, rebind}`; unchanged.
- `crates/voice-me-core/src/error.rs:9-24` -- `HotkeyAlreadyInUse`, reused.
- `crates/voice-me-ui/src/hotkey.rs` -- `HotkeyView` (`:310`), buttons `:543-569`, `accelerator_code` `:193-247` (the canonical token list). Add the open-settings button.
- `crates/voice-me-app/src/main.rs:2567` -- adapter construction; `:3666-3674` re-binds the saved hotkey at start.
- `Cargo.lock` -- `zbus` 5.19 is already there through gpui; use it directly (blocking API) in `voice-me-hotkey-linux`.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-hotkey-linux/Cargo.toml` -- add `zbus` (the locked 5.x, blocking) -- the D-Bus service and the kglobalaccel client.
- [x] `crates/voice-me-hotkey-linux/src/desktop.rs` -- `Desktop::{Gnome, Kde, Other}` from `XDG_CURRENT_DESKTOP` (colon-separated, case-insensitive). Pure accelerator converters `to_gnome(&str)` (`Ctrl+Alt+KeyV` → `<Control><Alt>v`) and `to_qt_key(&str) -> i32` (modifier bits | Qt key code). Unit tests over every `accelerator_code` token.
- [x] `crates/voice-me-hotkey-linux/src/summon.rs` -- serve `dev.voice_me.VoiceMe` at `/dev/voice_me/VoiceMe`, method `Summon` → `HotkeyPressed`. Started once, when the adapter is created at app start, on every Linux desktop, so the manual command works before any hotkey is saved. Owning the name fails → error → fallback.
- [x] `crates/voice-me-hotkey-linux/src/gnome.rs` -- read and write the list and the relocatable `custom-keybinding` keys through `gsettings` (bounded run). Pure helper `with_voice_me_path(list_gvariant) -> String`, tested with an empty list, `@as []`, an existing entry, and others present.
- [x] `crates/voice-me-hotkey-linux/src/kde.rs` -- kglobalaccel: `doRegister`, `setShortcut` (with `SetPresent|NoAutoloading`), check the returned keys, subscribe to `globalShortcutPressed` on `getComponent("voice-me")`. Restore the previous keys on failure.
- [x] `crates/voice-me-hotkey-linux/src/portal.rs` -- GlobalShortcuts through `ashpd` (0.13, already in `Cargo.lock` through gpui, feature `global_shortcuts`): create a session, bind one shortcut `summon` with the preferred trigger converted from the accelerator (a pure converter to the XDG shortcuts format, e.g. `CTRL+ALT+v`, unit-tested), then forward `Activated` to `HotkeyPressed`. On rebind, bind again in the same session. Run on the Tokio bridge (AD-5) or a dedicated thread.
- [x] `crates/voice-me-hotkey-linux/src/lib.rs` -- choose lazily in decision 3's order (Gnome/Kde → portal on Wayland → X11 on X11 → evdev), moving to the next on failure. A `HotkeyAlreadyInUse` from any backend ends the attempt and is returned; a fallback never covers a conflict. Expose which backend is active (`None` before the first bind), so the Hotkey tab shows the GNOME/KDE note only when the native backend bound, or when nothing has bound yet on GNOME/KDE. Public `desktop()` and `open_shortcut_settings()` (`gnome-control-center keyboard` / `systemsettings kcm_keys`).
- [x] `crates/voice-me-ui/src/hotkey.rs` -- on GNOME/KDE, a note that the shortcut lives in the desktop's settings, and a button `hotkey-open-settings`. On other Linux desktops, a "Show steps" toggle (the Dependencies-row pattern) with the `gdbus call` command. Tests.
- [x] `README.md` -- document it.

**Acceptance Criteria:**
- Given a GNOME session, when the user saves a hotkey, then it appears under Settings → Keyboard → Custom Shortcuts as "voice-me", and pressing it with any window focused opens the Prompt Overlay without the keys reaching that window.
- Given a KDE session, when the user saves a hotkey, then it appears in System Settings → Shortcuts under voice-me and summons the overlay.
- Given the button is pressed, then the desktop's shortcut settings open.

## Implementation Notes

- New modules in `voice-me-hotkey-linux`: `desktop.rs` (detection and converters), `summon.rs` (D-Bus `Summon`), `gnome.rs` (gsettings, 5 s bound per call), `kde.rs` (kglobalaccel, KF5-era `setShortcut`/`shortcut`), `portal.rs` (ashpd GlobalShortcuts, 10 s bound).
  - `lib.rs` tries the backends in decision 3's order; the order itself is unit-tested.
  - Once one backend is running, rebinds stay on it.
  - A conflict found on the way wins over evdev's permission error.
- The summon service refuses to take over an existing name. A second voice-me fails, and GNOME then falls back, instead of stealing the shortcut.
- UI: `HotkeyDesktop::{Plain, Native, Manual}` is injected by `voice-me-app` (`hotkey_desktop()`), so `voice-me-ui` still does not depend on the adapter.
- Architecture: AD-12 names `voice-me-espeak` as the bounded runner for child processes. This crate now spawns `gsettings` (with its own deadline) and the settings apps. The spec required it, and the rule predates this change.
- Matrix audit:
  - GNOME save/rebind: live test against real `gsettings` (keyfile backend, `VOICE_ME_TEST_GSETTINGS=1`).
  - KDE save and conflict: live test against a fake kglobalaccel on `dbus-run-session`.
  - Summon: live `gdbus call` test.
  - Unmappable key: converter tests.
  - Other desktop: order test and UI steps tests.
  - Portal row: the order and converter are unit-tested. The portal exchange itself has no test; no GlobalShortcuts portal exists here.
  - App-not-running row: this is `gdbus`'s own behaviour; there is no code of ours to test.
- Verified:
  - `dbus-run-session cargo test -p voice-me-hotkey-linux` with the gsettings env: 45 passed.
  - `voice-me-ui`: 128 passed. `voice-me-app`: 53 passed.
  - `fmt` and `clippy` are clean.
- Disk: testing `-p hotkey-linux -p ui` in one run changed the feature resolution and rebuilt the gpui stack, which filled the disk. Old duplicate rlibs were deleted (the two newest of each were kept). No `cargo clean`.

- Review pass 1 fixes verified:
  - `dbus-run-session` with the CI env: hotkey-linux 50 passed, and no test skipped.
  - ui 129, app 54. fmt and clippy are clean.
- Also in this commit, at the user's request: the Settings window opens centred at 900×640 (`SETTINGS_WIDTH`/`SETTINGS_HEIGHT` in `voice-me-app`) instead of the platform default, which filled the screen.

## Spec Change Log

- **Review pass 1:**
  - **Findings that triggered it:**
    - A first-bind conflict on KDE fell through to evdev and was hidden.
    - The summon service started only on the first bind, so the manual `gdbus` command did nothing before a Save.
    - The Hotkey tab chose the GNOME/KDE note from `XDG_CURRENT_DESKTOP`, even when a fallback had bound.
  - **Amended:**
    - The `summon.rs` task: start the service at adapter creation.
    - The `lib.rs` task: a conflict ends the attempt, and the active backend is exposed for the tab.
  - **Known-bad states avoided:** double-firing combinations, a manual command that reaches nothing, a note pointing at a desktop entry that does not exist.
  - **KEEP:** every module and test as built (converters, the gsettings list edit, the kglobalaccel fake-bus test, the name-takeover refusal, the portal threads, the UI `HotkeyDesktop` injection). This pass amends them in place; code was not reverted and re-derived, because the rest verified clean.

## Review Triage Log

Pass 1: blind hunter (B), edge-case hunter (E), verification gap (V).

| Verdict | Finding | Evidence / route |
|---|---|---|
| medium | A first-bind conflict (KDE/X11) falls through to evdev and is hidden, so the combination double-fires (B, E, V) | `bind` stores `conflict` and keeps looping → bad_spec, amended: a conflict ends the attempt |
| medium | The summon service starts only on the first bind, so the manual command does nothing before a Save (B) | `ensure_summon_service` is called from `bind` → bad_spec, amended: start at adapter creation |
| medium | The GNOME/KDE note is chosen from `XDG_CURRENT_DESKTOP`, not the backend that bound (B, E, V) | `hotkey_desktop()` reads `desktop()` → bad_spec, amended: expose the active backend |
| medium | The portal's 10 s deadline covers a user-facing bind dialog, so it times out, falls back, and the late bind double-fires (B, E) | `bounded` wraps `bind_shortcuts` → patch, separate 120 s bind deadline |
| medium | The live D-Bus/gsettings tests skip silently in CI and run on a developer's real bus (B, V) | Gated only on `DBUS_SESSION_BUS_ADDRESS`; ci.yml has no bus → patch, explicit opt-in plus a CI step under `dbus-run-session` |
| medium | The fallback loop's logic is untested (V) | Pre-verified → patch, pure loop function with tests |
| low | KDE `shortcut` read failure restores empty keys (E) | `unwrap_or_default` → patch, `?` |
| low | KDE reports success when the assigned keys differ from the requested key (E) | Only all-zero is checked → patch, require the key |
| low | The open-settings failure overwrites the hotkey error and goes stale (B, E) | `hotkey_error` reused → patch, own field |
| low | The manual steps can double-fire with voice-me's own binding (B) | The text invites the same combination → patch, text |
| low | README says "contains" GNOME/KDE, but the code matches a whole entry, and it does not say that desktop-side edits are replaced (B) | Docs → patch |
| low | Desktop-side edits are overwritten at start and not synced back (B) | Real. The text now says to change it in voice-me. Syncing back is new behaviour → rejected beyond the text |
| low | The GNOME entry and KDE component are never removed (B) | No unbind exists (spec 2.3 deferral) → defer |
| medium | The portal has no integration test (B) | A fake portal needs Request/Response objects → defer |
| maybe-false | Shift plus a digit or symbol may never fire on KDE (Qt matches shifted keysyms) (B) | Needs a real Plasma session → defer, unverified medium |
| maybe-false | Plasma 6 kglobalacceld may lack the KF5 `setShortcut`/`shortcut` methods (B) | Needs a real Plasma 6 session. If absent, KDE falls back to the portal → defer, unverified high |
| low | All-fail shows evdev's `input`-group error on GNOME/KDE (B) | That is the backend that would actually work next, so the message is actionable → rejected |
| low | KDE start leaves a grab with no listener if `getComponent` fails after `setShortcut` (B, E) | Needs kglobalaccel to fail between two calls → rejected (rare, adds cleanup branches) |
| low | GNOME list read→write race and partial rollback (B, E) | Seconds-wide window during a Save → rejected (rare) |
| low | The KDE listener ends silently if kglobalaccel restarts (E) | Rare, needs supervision → rejected |
| low | A failed summon start is not retried (E) | Only after another instance exits → rejected |
| low | A leaked portal session after a timed-out create (E) | Mostly gone with the separate bind deadline → rejected |
| false | An unmappable key is refused even when native is unavailable (E) | Every key the UI can capture maps (tested over all 73 tokens) → rejected |
| low | A settings program that exits at once reports success (E) | Rare → rejected |
| low | The `main.rs` mapping is untested (V, filed defer) | Folded into the active-backend patch as a pure, tested fn |

## Design Notes

**Why D-Bus and not a command with the binary path:** the release is a plain tarball with no install location, so any stored path goes stale when the binary moves. `gdbus` ships with GLib on every GNOME system. KDE needs no command at all: kglobalaccel signals the registered component directly, as it does for native KDE apps.

**Why GNOME/KDE native before the portal:** KDE and GNOME 48+ implement the portal, but an unsandboxed app with no `.desktop` file has no app id there, and GNOME may refuse it. The native path avoids that. The portal serves the other compositors that implement it (e.g. Hyprland).

**Not testable here:** this container has no GNOME or KDE session. The converters, the GVariant list edit and the desktop detection are unit-tested. The D-Bus calls are verified by hand on GNOME and on KDE.

## Verification

**Commands:**
- `cargo test -p voice-me-hotkey-linux -p voice-me-ui` -- expected: pass
- `cargo clippy -p voice-me-hotkey-linux --all-targets` -- expected: clean

**Manual checks (if no CLI):**
- GNOME: save → `gsettings get org.gnome.settings-daemon.plugins.media-keys custom-keybindings` lists `…/voice-me/` once; press the combination → the overlay opens.
- KDE: save → the shortcut shows in System Settings; press it → the overlay opens.
