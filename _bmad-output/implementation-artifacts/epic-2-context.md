# Epic 2 Context: Speak Without Speaking (Core Loop)

<!-- Compiled from planning artifacts. Edit freely. Regenerate with compile-epic-context if planning docs change. -->

## Goal

Deliver the product's entire reason to exist, end to end: press a global hotkey from anywhere — including inside a fullscreen game — get a minimal always-on-top overlay, type a line, hit Enter, and a moment later hear that line spoken in the user's own cloned voice through a virtual microphone that other apps (Discord, games, Zoom) pick up as real mic input. This epic carries two foundational technical risks that are de-risked early via spikes rather than a separate epic: (1) neither GPUI nor gpui-kit provide system tray or global-hotkey support upstream, and (2) the Chatterbox TTS sidecar's packaging/IPC and the Windows virtual-mic driver's control surface are both unverified. Both must be resolved before the rest of the loop is built on top of them.

## Stories

- Story 2.1: Spike — Background/Tray Presence Feasibility on GPUI/gpui-kit
- Story 2.2: Run as a Background/Tray-Resident Process
- Story 2.3: Configure a Global Hotkey
- Story 2.4: Summon, Type, and Dismiss the Prompt Overlay
- Story 2.5: Spike — Chatterbox Sidecar Packaging and IPC
- Story 2.6: Generate Speech from the Prompt Overlay Text
- Story 2.7: Spike — Linux Virtual Microphone via PipeWire
- Story 2.8: Spike — Windows Virtual Microphone Control Surface
- Story 2.9: Play Generated Audio Through the Virtual Microphone

## Requirements & Constraints

- The app must run tray-resident with no visible window needed to respond to the hotkey; dismissing the Prompt Overlay never stops the app from listening for the next hotkey press.
- A single global hotkey, user-assignable and changeable, must summon the Prompt Overlay from anywhere on Linux and Windows, including while a fullscreen/borderless-fullscreen game holds input focus. Assigning a combination already in use elsewhere must be surfaced, never silently dropped.
- The Prompt Overlay must appear and be ready for keystrokes within roughly 200ms of the hotkey press (unmeasured target, no hard baseline yet). Enter closes it immediately and fires the Speak Action without waiting on generation; Escape or focus loss closes it and discards the text without speaking.
- On a Speak Action, typed text + active Reference Voice Sample + selected speech language go to the TTS Engine (Chatterbox-Multilingual V3) via the Sidecar Process. A generation failure must surface a clear failure indication, never silence. No hard end-to-end latency target exists yet (Enter → audible playback) — it must feel usable in live voice chat, to be measured during implementation.
- Generated audio must play out through the Virtual Microphone device (not the default speaker) on both Linux and Windows, timed to the Speak Action; the user's real physical microphone must be unaffected.
- Whether global hotkey capture during fullscreen games risks anti-cheat conflicts (BattlEye, EasyAntiCheat) is unresolved and must be investigated and documented before the hotkey-capture implementation is locked in — this is a done-criterion for Story 2.3, not just a nice-to-have.
- The app must be local-only: no cloud calls, no accounts, no telemetry. The TTS sidecar's IPC (like every component except the dependency-provisioning adapter) is loopback-only — no network egress.
- All standard UI elements use gpui-kit components per the gpui-kit Design Guides; the Prompt Overlay is the one custom-built surface (no stock gpui-kit equivalent fits its contract).

## Technical Decisions

- Hexagonal/ports-and-adapters: `voice-me-core` owns `AppState`, `AppEvent`, and every port trait (`HotkeyPort`, `VirtualMicPort`, `TrayPort`, `TtsPort`) and depends on no adapter crate. Adapters never depend on each other — all cross-adapter effects flow through `AppState`/`AppEvent`.
- Per-OS capabilities are separate crates, not `cfg`-gated modules: `voice-me-hotkey-{linux,windows}`, `voice-me-audio-{linux,windows}`, `voice-me-tray-{linux,windows}`, each implementing the matching core port trait. `voice-me-app` selects the right crates per target via Cargo `[target.'cfg(...)'.dependencies]`.
- Single `AppState`/single `AppEvent` channel: only `voice-me-core` use-case functions mutate `AppState`. Every adapter holds only the `AppEvent` sender; `voice-me-app` is the sole receiver and is the one place that pumps events into core use-cases and forwards results to `voice-me-ui`.
- One `thiserror` domain error enum in `voice-me-core`; each adapter maps its own failures (IPC error, driver error, I/O error) into it at the port boundary; `anyhow` aggregates only at the `voice-me-app` composition root.
- Tokio runs alongside GPUI's own executor via one in-house bridge (reimplementing Zed's `gpui_tokio` pattern — a `Global` holding a `tokio::runtime::Handle` plus a `Tokio::spawn` wrapping `cx.background_spawn`), owned by `voice-me-core`, shared by `voice-me-tts` and `voice-me-deps`. This is *not* a direct dependency on Zed's own `gpui_tokio` crate (not independently publishable / would drag in a second `gpui` instance).
- Speak Action sequencing is a hard contract: dismissing the Prompt Overlay on Enter is synchronous and unconditional — `voice-me-ui` never waits on `TtsPort::generate`. Generation runs as a Tokio task dispatched immediately after dismissal; its result reaches the UI later via `AppEvent`. Sidecar Process lifecycle (start, health-check, crash-restart) is owned entirely inside `voice-me-tts` and never leaks into core or UI as a process-management detail — only as the domain error enum.
- One shared audio buffer type crosses the `TtsPort` → `VirtualMicPort` boundary — never a raw byte slice or a per-adapter struct. Exact format (sample rate, bit depth, channel layout) is not yet fixed; it depends on what Chatterbox actually outputs (to be resolved in Story 2.5) and what each `voice-me-audio-*` backend expects. Any format conversion happens inside the `voice-me-audio-*` adapter.
- Hardware capability detection (GPU vs CPU-only) is core-owned: `voice-me-deps` reports it only via `AppEvent`; `voice-me-tts` reads it from `AppState`/`TtsPort` call parameters and never calls `voice-me-deps` directly.
- No network egress outside `voice-me-deps`'s declared GitHub-Releases fetches — enforced by a CI check (cargo-deny ban list or a workspace grep for HTTP-client crates), not review discipline alone.
- Windows virtual-mic driver candidate: `VirtualDrivers/Virtual-Audio-Driver` release `25.7.14` (MIT + MS-PL, SignPath-signed — a real working signing path, no EV cert needed). Its named-pipe/IPC control surface is **unconfirmed** as a standard feature of the public release; Story 2.8 exists specifically to verify this before `voice-me-audio-windows` is built on the assumption. Fallback paths if unconfirmed: other scriptable interface (registry config, CLI, COM/WinRT), or forking the driver.
- Linux virtual mic: PipeWire/PulseAudio null-sink, no driver/kernel module, no elevated privileges required (to be confirmed in Story 2.7).
- IPC framing between `voice-me-tts` and the Python sidecar (stdio JSON-lines vs. a local socket) is an implementation detail hidden behind `TtsPort` — either choice satisfies the architecture invariants; decide during Story 2.5.
- Foundational unresolved risk: neither GPUI nor gpui-kit provide tray or global-hotkey support upstream; the unofficial "Adabraka GPUI" fork claims to but is unverified. This is isolated behind the `voice-me-tray-*`/`voice-me-hotkey-*` adapter crates so the resolution doesn't ripple elsewhere — Story 2.1 exists to resolve it (hand-rolled per-OS shim, the fork, or another approach) before the rest of the epic builds on it.

## UX & Interaction Patterns

- **Prompt Overlay**: custom composition (not a stock gpui-kit surface) — borderless, always-on-top, one `Input` and nothing else, built from gpui-kit's popover-family elevation/shadow treatment plus a dedicated rounder corner radius (12px, vs. gpui-kit's default control radius elsewhere), fixed non-resizable comfortable width. Appears with the `Input` pre-focused so typing starts immediately (assumption: centered on the active/focused monitor). Only a short fade/scale-in on summon and fade-out on dismiss — no other animation anywhere in the app. Must be operable keyboard-only from summon to dismissal, with no extra click to focus the input.
- **Hotkey chip**: monospace "shortcut" typography (12px, medium weight — reserved exclusively for hotkey display, never prose), `muted` background. Used in Settings → Hotkey and during live capture. Renders platform-native modifier names (Linux: Ctrl/Alt/Shift/Super; Windows: Ctrl/Alt/Shift/Win).
- **Hotkey capture field**: clicking "Change" captures the next key combination live, renders it as a hotkey chip for confirmation before Save. An already-in-use combination is rejected inline at the capture field (naming the conflicting app), before Save — not after. The previously-working hotkey stays active until a new one is confirmed.
- **Tray menu**: exactly two items, "Settings…" and "Quit" — no status submenu. Tray icon follows each OS's native status-icon area convention (Windows notification area; Linux DE-native convention).
- **Generation feedback**: no blocking UI while TTS generates (the overlay is already closed by then). If generation is unusually slow, a brief OS-native notification reports it's still working. On failure, an OS-native notification names the short reason — this notification *is* the "clear failure" surface; no overlay reappears automatically, the user just re-invokes the hotkey to retry.
- Voice/tone for microcopy: quiet, direct, no exclamation marks or cutesy framing (e.g. "Hotkey already in use by {app}." not "Error: hotkey conflict detected."; "Couldn't generate speech. Check the sidecar and try again." not "Oops! Something went wrong.").
- Status is never color-only — e.g. any failure/conflict messaging carries words, not just color.

## Cross-Story Dependencies

- Story 2.1 (tray spike) must resolve before Story 2.2 (tray-resident process) is built on its findings.
- Story 2.2 (tray presence) and Story 2.3 (hotkey configured) are both prerequisites for Story 2.4 (Prompt Overlay summon/dismiss).
- Story 2.5 (Chatterbox sidecar spike) must resolve — including defining the shared audio buffer type — before Story 2.6 (speech generation) is implemented.
- Story 2.4 (Speak Action triggered) and Epic 1 (an active Reference Voice Sample must exist) are both prerequisites for Story 2.6.
- Stories 2.7 (Linux virtual mic spike) and 2.8 (Windows virtual mic spike) must resolve before Story 2.9 (playback through the Virtual Microphone) is built, since the Windows control-surface outcome in particular may force a fallback approach.
- Story 2.6 (generated audio exists) and Stories 2.7/2.8 (a Virtual Microphone is available) are both prerequisites for Story 2.9.
- Epic 3 (dependency detection/provisioning) governs what happens when a Speak Action is attempted with a missing dependency (blocking-overlay behavior) — not built in this epic, but Story 2.6's read of GPU/CPU capability from `AppState` depends on Epic 3's `voice-me-deps` emitting that state via `AppEvent`.
