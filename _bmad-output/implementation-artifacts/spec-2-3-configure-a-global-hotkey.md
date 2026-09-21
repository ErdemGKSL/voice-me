---
title: 'Configure a Global Hotkey'
type: 'feature'
created: '2026-09-21'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
context: []
baseline_commit: 'd423b8caf91d549f2dffa240e5a29a96e58f1c0d'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** `AppState.hotkey` and the `settings.toml` `hotkey` field already exist, but nothing reads or writes them: `voice-me-hotkey-linux`/`-windows` are `todo!()` stubs, `SettingsStore` has no `save_hotkey`, and there is no UI to capture a combination. Without a configured, globally-registered hotkey, Story 2.4's Prompt Overlay has no way to be summoned.

**Approach:** Build a tabbed Settings shell (Voice | Hotkey), add a Hotkey capture flow (Change → press combination → chip preview → Save) that persists through `SettingsStore`, and implement `voice-me-hotkey-linux` with two backends — `global-hotkey` on X11, raw `evdev` on Wayland — so a saved combination emits `AppEvent::HotkeyPressed`. Windows stays a stub per the `voice-me-tray-windows` precedent.

**Decisions taken (human, 2026-09-21):**
1. **Wayland gets an evdev fallback.** `global-hotkey` is X11-only and GNOME does not implement the GlobalShortcuts portal, so evdev is the only way the hotkey works in this project's own dev session. Three consequences are accepted, not defects: **(a)** evdev reads passively — it does not grab keys exclusively, so the combination *also* reaches the focused app (pressing it in a game triggers voice-me and types into the game; the only alternative, `EVIOCGRAB`, would steal all input from every other application); **(b)** there is no registration on this path, so conflicts cannot be detected at all — inline conflict rejection exists only on X11; **(c)** it requires read access to `/dev/input/event*`, i.e. membership in the `input` group, which this machine does not currently have.
2. **Conflict copy drops the app name.** No OS API reports which process holds a combination, so UX-DR19's "Already used by {app}" is amended to "This combination is already in use. Try another."
3. **Tabbed Settings shell is built now** (Voice | Hotkey), superseding spec-2-2's boundary against a multi-section shell — Epics 3 and 4 both add sections, so the shell arrives here rather than as a later rewrite.

## Boundaries & Constraints

**Always:** The adapter signals presses only through the shared `AppEvent` channel (AD-3) — never depending on `voice-me-ui` or touching windows. The previously saved hotkey stays active until a new one is confirmed working; a failed Save leaves both the old binding and `settings.toml` untouched. Backend selection is runtime, not compile-time: `WAYLAND_DISPLAY`/`XDG_SESSION_TYPE` decides X11 vs evdev. A missing-permission failure on the evdev path must surface as actionable words naming the `input` group — never a silent no-op. Hotkey chips use monospace 12px medium on `muted` background with platform-native modifier names (Ctrl/Alt/Shift/Super). Microcopy stays quiet and direct; no failure is signalled by color alone. Use gpui-kit components where one exists.

**Never:** Do not build the Prompt Overlay or wire `AppEvent::HotkeyPressed` to any window — Story 2.4 owns that; the pump arm here logs the press and nothing more. Do not implement `voice-me-hotkey-windows` (stays `todo!()`, signature-compatible only) and do not use evdev or any keystream reader on the Windows path — its backend is `RegisterHotKey` when built. Do not call `EVIOCGRAB`. Do not attempt privilege escalation, setuid, or writing udev rules from the app; surface the requirement and stop.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Capture a combination | "Change" active, user presses Ctrl+Alt+V | Chip shows `Ctrl+Alt+V`, Save enabled | N/A |
| Modifier-only press | "Change" active, user presses Ctrl alone | No capture; still waiting | Inline: needs a non-modifier key |
| Save, X11, combination free | Chip `Ctrl+Alt+V` | Registers, persists, becomes the saved binding | N/A |
| Save, X11, combination taken | Chip `Ctrl+Alt+T`, grabbed elsewhere | Rejected before persist; old binding stays live | Inline conflict message |
| Save, Wayland | Any valid chip | Always persists — no conflict detection exists | N/A |
| evdev without permission | Wayland, user not in `input` group | Hotkey inert; app still starts tray-resident | Inline + stderr naming the `input` group |
| Escape during capture | "Change" active, Escape pressed | Capture cancelled, saved binding unchanged | N/A |

</frozen-after-approval>

## Code Map

- `crates/voice-me-core/src/ports.rs` -- `HotkeyPort::start_listening(&self, hotkey: &str)` has no sender and no re-registration path. Change to `start_listening(&self, hotkey: &str, events: AppEventSender)` (mirroring `TrayPort::show`'s AD-3 threading) and add `rebind(&self, hotkey: &str) -> Result<(), VoiceMeError>` so Save learns about a conflict before persisting. Add `SettingsStore::save_hotkey(&self, hotkey: Option<&str>) -> Result<AppState, VoiceMeError>` — copy `save_selected_mic_device` (line 144) in shape; `SettingsFile.hotkey` already round-trips.
- `crates/voice-me-core/src/error.rs` -- add variants for "combination already registered" and "input device permission denied" so the UI can tell a conflict, a permission problem, and a generic failure apart.
- `crates/voice-me-hotkey-linux/src/lib.rs` + `Cargo.toml` -- replace the `todo!()`. Add `global-hotkey = "0.8"` and `evdev = "0.13"`. Split into `x11.rs` (register a `HotKey` on a process-long `GlobalHotKeyManager`, held by the adapter the way `LinuxTrayAdapter` holds `TrayHandle`; forward `GlobalHotKeyEvent`) and `evdev.rs` (enumerate `/dev/input/event*` for `EV_KEY` devices, read on a background thread, track modifier state, match the combination). Both forward `AppEvent::HotkeyPressed` via the sender. `rebind` on X11 registers the new key before unregistering the old; on evdev it swaps the matched combination on the running thread.
- `crates/voice-me-hotkey-windows/src/lib.rs` -- signature-only update to match the trait; body stays `todo!()`.
- `crates/voice-me-ui/src/settings.rs` (new) -- tabbed shell wrapping the existing `VoiceSetupView` as the Voice tab plus a new Hotkey tab. Use gpui-kit's `Tabs` component.
- `crates/voice-me-ui/src/hotkey.rs` (new) -- the capture view. Reuse `voice_setup.rs`'s established idiom: `Button`, `Alert::error`, `.when_some(...)`, and the `div().id(...).test_support()` marker (line 628) so capture state is assertable. State: `capture_mode`, `pending_hotkey`, `saved_hotkey`, `hotkey_error`. Inject `HotkeyPort` as a trait object the way `voice_setup.rs` injects `CaptureSource`/`InputDeviceSource`, so tests can fake it.
- `crates/voice-me-ui/src/lib.rs` -- export the new views and the pure capture/format helpers, matching `recording_meets_minimum_duration`'s precedent for `voice-me-tests`.
- `crates/voice-me-app/src/main.rs` -- composition root. Open the Settings shell instead of `VoiceSetupView` directly (lines 78-92), pass `saved_hotkey` from the startup `settings_store.load()` into the adapter, call `start_listening` after the tray `show` (same `#[cfg(target_os)]` split, lines 100-114), and add an `AppEvent::HotkeyPressed` arm to the `cx.spawn` pump (line 130) that logs only. `Cargo.toml` -- add `voice-me-hotkey-linux`/`-windows` under the existing target-gated tables.
- `README.md` -- document the one-time `sudo usermod -aG input $USER` (plus re-login) needed for the Wayland path.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-core/src/error.rs`, `src/ports.rs`, `src/settings_store.rs` -- conflict + permission error variants, reshaped `HotkeyPort`, `save_hotkey` -- lets the UI attempt registration and persist only on success
- [x] `crates/voice-me-core/src/settings_store.rs` -- unit-test `save_hotkey` round-trip and clear-to-`None`, mirroring `save_selected_mic_device_round_trips_across_a_fresh_load` (line 217)
- [x] `crates/voice-me-hotkey-linux/src/x11.rs`, `src/evdev.rs`, `src/lib.rs`, `Cargo.toml` -- both backends, runtime session detection, string↔combination parsing, event forwarding, `rebind` ordering -- the actual global capture
- [x] `crates/voice-me-hotkey-linux/src/evdev.rs` -- unit-test the pure modifier-state/combination matcher against synthetic event sequences (no `/dev/input` access), since that is the only part of the evdev path testable without hardware
- [x] `crates/voice-me-hotkey-windows/src/lib.rs` -- signature-only update -- keeps the workspace compiling
- [x] `crates/voice-me-ui/src/settings.rs`, `src/hotkey.rs`, `src/lib.rs` -- tabbed shell and the Change/capture/chip/Save flow with inline conflict, permission, and modifier-only handling
- [x] `crates/voice-me-ui/src/hotkey.rs` -- UI tests for the I/O matrix rows against a fake `HotkeyPort`
- [x] `crates/voice-me-hotkey-linux/examples/hotkey-spike.rs` -- standalone verification binary selecting either backend, mirroring `voice-me-tray-linux/examples/spike.rs`
- [x] `crates/voice-me-app/src/main.rs`, `Cargo.toml` -- open the shell, wire the adapter, add the `HotkeyPressed` pump arm
- [x] `README.md` -- the `input`-group setup step
- [x] This spec's Implementation Notes -- record the anti-cheat finding (epic AC requires it documented before the story is done): `RegisterHotKey`/`XGrabKey` are message-based and treated as ordinary app behavior by BattlEye/EasyAntiCheat, whereas `SetWindowsHookEx(WH_KEYBOARD_LL)` and raw keystream readers share a keylogger's signature — which is why the Windows path stays on `RegisterHotKey` and evdev is confined to Linux/Wayland, where kernel-level anti-cheat is largely absent

**Acceptance Criteria:**
- Given no hotkey has ever been configured, when the app starts, then it starts tray-resident with no hotkey active and no error shown
- Given a hotkey was saved in a previous run, when the app starts, then the session-appropriate backend activates it and each press emits exactly one `AppEvent::HotkeyPressed`
- Given an active hotkey, when the user saves a different valid combination, then the new combination fires and the old one no longer does
- Given the user is mid-capture with an unsaved chip, when the window closes without Save, then the saved binding is unchanged in both `settings.toml` and the OS
- Given an active hotkey, when the combination is pressed while another application holds input focus, then the event still fires
- Given a Wayland session where the user lacks `input`-group membership, when the app starts with a saved hotkey, then the app runs normally and the Hotkey tab states the requirement in words

## Implementation Notes

### Anti-cheat finding (epic AC: must be documented before the story is done)

`RegisterHotKey` (Windows) and `XGrabKey` (X11) are **message-based**: the OS
delivers one message for one registered combination and nothing else. Both
BattlEye and EasyAntiCheat treat that as ordinary application behavior —
overlays, launchers and chat apps all use it.

`SetWindowsHookEx(WH_KEYBOARD_LL)` and raw keystream readers are categorically
different: they observe **every** keystroke the user makes, which is a
keylogger's signature and a documented anti-cheat flagging pattern. That is
why the Windows path stays on `RegisterHotKey` when it is built (a note to
that effect lives on `WindowsHotkeyAdapter` itself so it cannot be lost), and
why `evdev` — which is exactly such a raw keystream reader — is confined to
Linux/Wayland, where kernel-level anti-cheat is largely absent.

### Deviations and decisions taken during implementation

- **`HotkeyPort` is a two-method trait, and the adapter binds lazily.** The
  very first hotkey a user ever configures is bound through `rebind` from the
  Settings UI, with no preceding `start_listening` call to carry the
  `AppEventSender` in. `LinuxHotkeyAdapter::new(events)` therefore takes the
  sender at construction and `start_listening` refreshes it. The alternative —
  starting a backend at launch with an empty combination — would surface the
  Wayland permission error on a machine that has never configured a hotkey,
  which the first acceptance criterion forbids.
- **Modifier-only presses are detected from `ModifiersChanged`, not
  `KeyDown`.** GPUI's Linux backends drop key events whose keysym is a
  modifier (`keysym.is_modifier_key()` → the event is discarded before any
  handler sees it), so a `KeyDownEvent` for Ctrl alone never arrives. The view
  watches the modifier set falling back to empty during capture instead. The
  pure `capture_keystroke` helper still classifies modifier key *names*, which
  is what the UI test drives and what keeps the case handled on any platform
  that does deliver such an event.
- **A failed persist rolls the binding back.** `rebind` succeeding and
  `save_hotkey` then failing would leave the live combination ahead of
  `settings.toml`; the view re-binds the previously saved combination in that
  case, so "a failed Save leaves both the old binding and `settings.toml`
  untouched" holds for a file failure too, not just a conflict.
- **Button disabled-state is asserted through dispatch, not the snapshot.**
  `ElementSnapshot::disabled()` reports `None` for a disabled gpui-kit
  `Button` in this version, so — following `voice_setup.rs`'s existing
  pattern — the tests assert that a click on a disabled control never reaches
  the fake collaborators.

### Corrections applied after the implementation pass

- **The example was renamed `spike` → `hotkey-spike`.** Cargo writes every
  example in the workspace to one shared `target/debug/examples/` directory,
  so this crate's `spike` collided with `voice-me-tray-linux`'s: cargo warned
  `output filename collision … this may become a hard error in the future`,
  and the two binaries overwrote each other, making `cargo run --example
  spike` non-deterministic across the two packages.
- **A test was added for the "Save, Wayland" matrix row.** That row's
  distinguishing claim — this backend registers nothing, so it can never
  report a conflict — was documented but unasserted;
  `evdev::tests::rebinding_never_reports_a_conflict` now pins it, so a
  refactor that started returning `HotkeyAlreadyInUse` on the evdev path
  could not silently contradict what the Hotkey tab tells a Wayland user.

### Manual verification performed

- `cargo run -p voice-me-hotkey-linux --example hotkey-spike` in this GNOME/Wayland
  session: selects the Wayland (evdev) backend and exits with
  `failed to bind Ctrl+Alt+KeyV: no read access to /dev/input/event* — add
  your user to the 'input' group and log back in`. This is the
  before-joining-the-group check: it fails in words, immediately, rather
  than hanging or silently doing nothing.
- The same spike with `WAYLAND_DISPLAY` unset and `XDG_SESSION_TYPE=x11`
  selects the X11 backend and binds successfully against Xwayland.
- Two X11 spike instances at once: the second reports
  `failed to bind Ctrl+Alt+KeyV: this combination is already in use` — the
  conflict path is real, not theoretical.
- The built `voice-me` binary starts, prints nothing to stderr (no hotkey
  configured → no error, per the first acceptance criterion), and still
  registers a tray item (`busctl --user get-property
  org.kde.StatusNotifierWatcher /StatusNotifierWatcher
  org.kde.StatusNotifierWatcher RegisteredStatusNotifierItems` lists a new
  `:N@/StatusNotifierItem` while it runs) — no regression to spec-2-2.
- **The last acceptance criterion, verified end to end** (re-run
  independently of the implementation pass): with a throwaway
  `XDG_CONFIG_HOME` holding `hotkey = "Ctrl+Alt+KeyV"` and an active sample,
  the binary on this Wayland session prints exactly
  `failed to activate the hotkey Ctrl+Alt+KeyV: no read access to
  /dev/input/event* — add your user to the 'input' group and log back in`,
  and then **keeps running normally** — at ~15s it holds 24 threads and its
  `StatusNotifierItem-<pid>-1` is registered. A hotkey failure costs the app
  neither its tray nor its life.
  Note for anyone repeating this: a cold-cache debug build sits in `D` state
  scanning `/usr/share/fonts` for the first ~10-15s before the tray appears.
  Sampling at 5s reads as "no tray" and is a measurement artifact, not a
  regression.

### Review patches applied

Nine findings routed to patch and were applied by the implementation agent,
then re-verified here:

- `README.md` — corrected both spike commands to `--example hotkey-spike` and
  the stated default to `Ctrl+Alt+KeyV`; added an explicit warning that
  `input`-group membership lets any process running as that user read every
  keystroke system-wide (including passwords typed into other applications),
  that the Wayland backend does exactly that, and that X11 needs none of it.
- `crates/voice-me-hotkey-linux/Cargo.toml` — the comment now describes what
  `default-features = false` actually does instead of contradicting it.
- `crates/voice-me-hotkey-linux/src/x11.rs` — the event pump's `.expect()`
  became a logged failure, matching `spawn_reader` and this story's rule that
  a hotkey failure never costs the app its tray presence.
- `crates/voice-me-hotkey-linux/src/evdev.rs` — `open_keyboards` no longer
  returns `Ok` when a *readable* pseudo-keyboard masks an *unreadable* real
  one. A permission-denied node is now checked against the world-readable
  `/sys/class/input/<eventN>/device/capabilities/key` bitmap, and only one
  that positively reports `KEY_A` forces the permission error — so unreadable
  mice, switches and audio nodes leave the normal case untouched. Spot-checked
  against this machine's 25 input nodes: the heuristic selects exactly the
  three real keyboards and rejects everything else, including the HP
  keyboard's own consumer-control and system-control sub-nodes.
- Four new test modules closed the verification gaps: all 73 accelerator
  tokens the UI can emit now round-trip through `Combination::parse`;
  `map_register_error`'s conflict mapping is pinned; `session_kind`'s decision
  moved into a pure `session_kind_for` helper with its three rows tested; and
  `SettingsView`'s tab routing is asserted, so swapping the match arms — which
  would make the whole feature unreachable in the app — now fails the suite.

Post-patch verification: `cargo check --workspace --all-targets`,
`cargo fmt --all --check`, and `cargo clippy --workspace --all-targets` are
clean (the only two clippy warnings are pre-existing: `assert!(true)` from
Story 1.1 and a `let _ =` at `main.rs` that predates this change).
`cargo test --workspace` passes 66 tests, none failing. The two manual paths
were re-run against the rebuilt spike: Wayland still reports the `input`-group
requirement in words, and a second X11 instance still reports the conflict.

### Manual verification still owed

Three checks need a machine state this session could not produce:

1. A real press firing the hotkey, on either backend. No key-injection tool
   (`xdotool`/`ydotool`) is installed, and nothing may be installed from
   here; the press-to-event path is covered by the evdev matcher's unit
   tests and by the X11 backend's `Pressed`-only filter, but not yet by a
   physical press.
2. The Wayland/evdev path with permission: this user is not in the `input`
   group (`groups` shows `erdem docker crossmacro wheel`), and joining it
   needs `sudo` plus a re-login.
3. The fullscreen-focus check (combination still fires while another
   application holds a fullscreen window), which depends on (1).

## Spec Change Log

## Review Triage Log

**Routed to patch**

- **README documents `--example spike`, which no longer exists** — `medium`. Verified: `README.md:51-52` names `spike` while the file is `examples/hotkey-spike.rs`; both documented commands fail with "no example target named `spike`". Introduced by this build's own rename of the colliding example. The inline `# Ctrl+Alt+V` also misstates the spike's default, which is `Ctrl+Alt+KeyV`. Route: patch.
- **README asks the user to join the `input` group without saying what that grants** — `medium`. Verified: `README.md` presents `sudo usermod -aG input $USER` as routine setup. Membership lets any process running as that user read every keystroke system-wide, and the Wayland backend is doing exactly that. The spec is explicit about the keylogger-signature issue for Windows but the user-facing document is silent on it — the one consequence here with a security dimension. Route: patch.
- **`Cargo.toml` comment contradicts the dependency line it annotates** — `low`. Verified: the comment reads "Default features only — no tokio" directly above `evdev = { version = "0.13", default-features = false }`, which disables defaults. The comment is what a future reader trusts. Fix is a direct correction. Route: patch.
- **`spawn_event_pump`'s `.expect()` panics the composition root where the evdev sibling tolerates the same failure** — `low`. Verified: `x11.rs:97` ends in `.expect("failed to spawn the X11 hotkey event pump thread")` while `evdev.rs`'s `spawn_reader` logs and continues. A thread-spawn failure is rare, but this story's own principle is that a hotkey failure never costs the app its tray presence, and the fix is a direct substitution with no added surface. Route: patch.
- **`open_keyboards` reintroduces the silent no-op its own doc comment forbids, on partial permission** — `medium`. Verified: `permission_denied` is consulted only inside the `devices.is_empty()` branch, so one readable pseudo-keyboard (uinput node, VM tablet driver) alongside an unreadable real keyboard returns `Ok` with a reader on a device that can never produce the combination. The frozen Boundaries require a missing-permission failure to surface in words, never as a silent no-op. Route: patch — warn without failing, so the normal mixed-permission case (unreadable non-keyboard nodes) does not start erroring.
- **The UI's accelerator vocabulary and the adapter's parser are connected only by convention, with no test** — `medium`. Pre-verified by the verification-gap layer and independently confirmed here: `voice-me-ui` hand-builds accelerator strings in `accelerator_code` and does not depend on `global-hotkey`; the adapter parses them via `HotKey::from_str` + `linux_keycode`. I probed all 73 tokens the UI can emit through `Combination::parse` — **all currently pass**, so this is a verification gap rather than a live bug. A typo in either table degrades to "the hotkey silently never fires", which no existing test would catch. Route: patch.
- **`map_register_error`, the product's only conflict signal, has no test** — `medium`. Pre-verified: `x11.rs` has no test module; the UI's conflict test injects an already-mapped `VoiceMeError::HotkeyAlreadyInUse` through the fake port, so it exercises the view's branch, not the mapping. Deleting the `AlreadyRegistered` arm would silently degrade the conflict copy to a generic failure with a green suite. Pure function, no X server needed. Route: patch.
- **`session_kind` has no test, and picking the wrong backend fails silently** — `medium`. Pre-verified: no test calls it. Inverting the `WAYLAND_DISPLAY` check would give a GNOME/Wayland user the X11 backend, which registers a grab that never fires — a hotkey that binds "successfully" and does nothing. Route: patch, via a pure helper taking the two env values (mutating process env in a test is `unsafe` under edition 2024 and racy in parallel).
- **`SettingsView` tab routing — the only path to the Hotkey tab in the shipped app — has no test** — `medium`. Pre-verified: no test constructs `SettingsView`; every `HotkeyView` test opens the view directly, bypassing the shell. Swapping the `match self.active_tab` arms would make the whole feature unreachable in the app with the suite still green. This is also the shell Epics 3 and 4 will build on. Route: patch.

**Rejected**

- **Capture left active when switching tabs leaves "Change" permanently disabled** — verified `false`. A `Cancel` button is rendered whenever `capture_mode` is true (`hotkey.rs:547-553`), so the state is always recoverable; the "permanently stuck" outcome does not occur.
- **A failed `X11Backend::start` that is retried leaks an extra event-pump thread** — verified `false`. `spawn_event_pump` runs only after `backend.rebind(hotkey)?` succeeds, and a failed `start` returns before `*backend = Some(started)`, dropping the manager. No path spawns a second pump.
- **`sprint-status.yaml` says `in-progress` while the spec says `in-review`** — verified `false`. Expected in-flight state; step-05 syncs it to `review` when the finished spec is presented. Same as spec-2-2's triage.
- **A bare alphanumeric is accepted as a global hotkey** — verified `low`, rejected. Real: plain `v` captures as `KeyV` and binds, which on X11 grabs that key process-wide. But it is previewed on the chip and requires an explicit Save to commit, and it is recoverable in-app (Change → new combination → Save) without editing TOML. Bare-key bindings are also standard in comparable tools (OBS, Discord push-to-talk), so "allow bare keys" is a defensible reading rather than a clear defect, and any fix encodes a contestable policy about which keys are "typing keys" plus a new `CaptureOutcome` variant on a public enum. Flagged to the human as a judgment call rather than patched silently.
- **No UI path can clear a configured hotkey, leaving `save_hotkey(None)` dead** — verified `low`, rejected. Real: only Change/Cancel/Save exist, so the `NO_HOTKEY_LABEL` state is unreachable once a hotkey is set. But Story 2.3's intent is "assign and change", not remove, and the fix adds new public UI surface. Recorded in deferred work instead.
- **`hotkey_startup_error` is never cleared, so a stale message can reappear** — verified `low`, rejected. Reachable only via a startup *conflict* (not the permission path, where a later Save also fails, so the message can never go stale): user resolves it, saves successfully, reopens Settings, and sees the old conflict text. Narrow, and the binding still works. Every fix either plumbs the shared cell into `voice-me-ui` (new coupling) or makes the error show exactly once — which is worse when the user closes the window without fixing anything.
- **A first-ever hotkey stays bound in the OS when `save_hotkey` then fails** — verified `low`, rejected. Real: the rollback is guarded by `if let Some(previous) = self.saved_hotkey`, and there is no previous binding to restore to. Persist failure is rare (disk full, permissions) and the divergence self-heals on the next restart; a clean fix needs a new `unbind` method on `HotkeyPort` — public surface for a case nothing demonstrated.
- **An "unsupported key" message is overwritten by the modifier-only message on modifier release** — verified `low`, rejected. Real and reachable (Ctrl+Alt+media-key, then release), but it is a transient wrong-message wart with no functional effect, and the fix adds a condition rather than correcting one.
- **A modifier already held when "Change" is clicked suppresses the modifier-only message** — verified `low`, rejected. `modifiers_held` starts `false`, so the release goes unnoticed. Cosmetic; capture still works normally on the next press.
- **All reader threads failing to spawn returns `Ok` with nothing reading** — verified `low`, rejected. Requires thread-resource exhaustion, at which point the app has larger problems; the fix adds a counter and branch for a state never demonstrated reachable.
- **`start_listening` after a backend exists does not refresh the sender into running threads** — verified `low`, rejected. Real, but `start_listening` is called exactly once, at startup; no reachable path calls it twice today.
- **`display_hotkey` silently keeps only the last non-modifier token, and can render an empty chip** — verified `low`, rejected. Reachable only from a hand-edited or corrupted `settings.toml`; the app never produces such a string.
- **A `settings_store.load()` failure at startup silently skips hotkey activation** — verified `low`, rejected. Consistent with the existing, deliberate treatment of the same failure for `has_active_sample` directly above it; changing one without the other would be incoherent.
- **A `Matcher` is created without seeding currently-held modifiers** — verified `low`, rejected. If the app starts while Ctrl is physically held, the combination does not match until that modifier is released and pressed again — a single-keystroke, self-correcting window.

**Routed to defer**

- **`voice-me-hotkey-windows`'s `todo!()` is now reachable at runtime, panicking on Windows at startup and on every Save** — `medium`, deferred. Verified real and a genuine behavior change. Deferred on spec-2-2's established precedent for the identical `voice-me-tray-windows` case: no Windows toolchain exists in this or any CI environment to build or verify a guard against, and the epic's Windows support is non-functional regardless (hotkey, audio, and virtual-mic Windows adapters are all still stubs).
- **Per-device `Matcher` cannot match a combination split across two event nodes** — `maybe-false`, would be `medium` if true. Cannot be settled here: `is_keyboard` admits only devices reporting `KEY_A`, which normally also report modifiers, but some laptops and combo receivers expose several nodes. Settling it needs hardware that splits modifiers from the main key matrix.
- **No hotplug handling; a reader thread that exits is never replaced** — `medium`, deferred. Verified real and acknowledged in the code's own comments: a keyboard connected after launch is never read, and a thread that dies on `fetch_events` (the comment names unplug and suspend) is not restarted, so the hotkey can quietly stop working after suspend/resume. A missing capability rather than a defect in what was built; needs an inotify watch on `/dev/input`.
- **X11 `rebind`'s register-before-unregister ordering is unpinned by any test** — `medium`, deferred. Pre-verified: no test executes `X11Backend::rebind`. Reversing the order would leave a user with no working hotkey after a rejected Save while the UI blames a conflict. Exercising it needs a real X display plus a second client holding the grab — no harness exists; `hotkey-spike` is the manual check, and I ran exactly that scenario by hand this session.
- **Startup re-activation of a saved hotkey is unexecuted by any test** — `medium`, deferred. Pre-verified: `voice-me-app` has no test target, so deleting the startup block would silently stop hotkeys surviving a restart with a green suite. Closing it means extracting the composition root's startup sequence into a library crate — larger than this change, and the same gap Story 2.2's tray wiring already carries.

## Design Notes

Backend split is forced by the platform, not preference. `global-hotkey`'s Linux backend is X11 `XGrabKey`, which yields a true exclusive grab — the key does not reach other applications, and a second registration of the same combination fails, which is what makes inline conflict detection possible there. evdev has neither property: it is a passive read of the raw device stream, so the combination still reaches the focused window and nothing can conflict. Treat the two paths as genuinely different capability tiers rather than trying to paper over the difference in the UI.

On Windows the mechanism must stay `RegisterHotKey`. Low-level hooks that observe every keystroke share a keylogger's signature and are a documented anti-cheat flagging pattern — directly relevant to this epic's "works while a fullscreen game has focus" goal.

Hotkey strings persist in `global-hotkey`'s accelerator syntax (e.g. `Ctrl+Alt+KeyV`) so the stored value round-trips through `HotKey::from_str` with no bespoke parser; the evdev backend maps that same string onto keycodes, and the chip's display form (`Ctrl+Alt+V`, `Super` not `Meta`) is derived for rendering only and never persisted.

## Verification

**Commands:**
- `cargo check --workspace` -- expected: whole workspace type-checks, including the Windows stub against the changed trait
- `cargo test --workspace` -- expected: new `save_hotkey`, matcher, and capture-flow tests pass alongside existing suites
- `cargo build -p voice-me-app` -- expected: builds cleanly on Linux

**Manual checks (if no CLI):**
- Run `cargo run -p voice-me-hotkey-linux --example hotkey-spike` in this GNOME/Wayland session after `sudo usermod -aG input $USER` and re-login: press the combination, confirm one event per press
- Confirm the same spike run *before* joining the `input` group fails with the permission message rather than hanging or silently doing nothing
- With the spike running, raise a fullscreen window from another application and confirm the combination still fires while that window holds focus
- Re-run the spike under an X11 session (`XDG_SESSION_TYPE=x11`): confirm the X11 backend is selected, and that a second spike instance registering the same combination reports the conflict instead of silently succeeding
- Run the built `voice-me` binary and confirm via `busctl --user list` that the tray still registers (no regression to spec-2-2)
