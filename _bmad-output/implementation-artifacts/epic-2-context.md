# Epic 2 Context: Speak Without Speaking (Core Loop)

<!-- Compiled from planning artifacts. Edit freely. Regenerate with compile-epic-context if planning docs change. -->

## Goal

Press a global hotkey from anywhere — including inside a fullscreen game — get a minimal overlay, type a line, hit Enter, and a moment later hear that line spoken in your own cloned voice through a virtual microphone other apps treat as input. This is the product's whole reason to exist, delivered end to end: tray-resident presence, hotkey configuration, the Prompt Overlay, in-process speech generation, and playback into a Virtual Microphone. Two technical risks live inside the epic — GPUI/gpui-kit's missing tray and global-hotkey support, and the ONNX inference loop plus the per-OS virtual-mic control surfaces — handled by spike stories sequenced early rather than by splitting the epic, since both belong to one inseparable user outcome.

## Stories

- Story 2.1: Spike — Background/Tray Presence Feasibility on GPUI/gpui-kit
- Story 2.2: Run as a Background/Tray-Resident Process
- Story 2.3: Configure a Global Hotkey
- Story 2.4: Summon, Type, and Dismiss the Prompt Overlay
- Story 2.5: Spike — In-Process Chatterbox Inference on ONNX Runtime
- Story 2.6: Generate Speech from the Prompt Overlay Text
- Story 2.7: Spike — Linux Virtual Microphone via PipeWire
- Story 2.8: Spike — Windows Virtual Microphone Control Surface
- Story 2.9: Play Generated Audio Through the Virtual Microphone

## Requirements & Constraints

- The app stays resident between uses: dismissing the Overlay never quits it. It remains reachable through a tray icon whose menu has exactly two items, "Settings…" and "Quit" — no status submenu — following each OS's native status-icon convention.
- One configurable global hotkey summons the Overlay from anywhere, including while a fullscreen or borderless-fullscreen game holds input focus, on Linux and Windows. An in-use combination is rejected inline before Save, naming the conflicting app; the previous working hotkey stays active until a new one is confirmed. Anti-cheat risk (BattlEye, EasyAntiCheat) of the chosen capture approach must be investigated and documented before the hotkey story is done.
- Prompt Overlay: borderless, always-on-top, fixed non-resizable width, one single-line input and nothing else, pre-focused on appear. Target roughly 200 ms from press to ready for keystrokes (an aim, not a gate). Enter closes it immediately and fires the Speak Action without waiting on generation; Escape or focus loss closes it and discards the text.
- A Speak Action passes the typed text, the active Reference Voice Sample, and the selected speech language to the in-process engine; speech language is independent of UI language. v1 speech languages are only those needing no Python-only text normalization — Turkish and English in; Chinese, Japanese, Hebrew, Korean out.
- Generation failure surfaces as an OS-native notification naming a short reason — never silence, and the Overlay never reappears on its own. Unusually slow generation gets a brief "still working" notification; no blocking UI, since the Overlay is already gone.
- Generated audio plays out the Virtual Microphone, not the default speaker, on both OSes, timed to the Speak Action; the user's real microphone is untouched.
- Measured reality (dev machine, CPU): session construction is 86–110 s and a ~2 s utterance costs ~20 s — a 0.10× real-time factor. Live voice chat is out of reach as it stands, so the asynchronous Speak Action must do real work: a warm-up path plus a visible "still loading"/"generating" state is required, not just lazy first-use construction. Whether ~20 s per utterance is acceptable at all is an open product decision.
- Accessibility: the Overlay is fully keyboard-operable from summon to dismissal with no extra click; icon-only controls carry tooltips and accessible names; status is never color-only.
- UI uses gpui-kit components wherever a suitable one exists, checked against the gpui-kit Design Guides before any story here is done.

## Technical Decisions

- Hexagonal: `voice-me-core` owns domain types, the single `AppState`, and every port trait (`HotkeyPort`, `VirtualMicPort`, `TrayPort`, `TtsPort`) and depends on no adapter crate. Per-OS capabilities are separate crates, not `cfg` modules; no adapter depends on a sibling — cross-adapter effects flow only through `AppState`/`AppEvent`. Only core use-cases mutate `AppState`; `voice-me-app` is the sole `AppEvent` receiver. One `thiserror` domain enum in core; adapters map their own failures at the port boundary; `anyhow` only at the composition root.
- **TTS is in-process (AD-12)** — no Python, no child process, no IPC. `voice-me-tts` loads four ONNX graphs (`speech_encoder`, `embed_tokens`, `language_model`, `conditional_decoder`) plus `tokenizer.json` via `ort`, with the generation loop (language tag, tokenize, encode reference once, KV-cached greedy decode with repetition penalty, decode to waveform) ported to Rust. `ort` uses `load-dynamic` so the ONNX Runtime library comes from the provisioned cache, never the build.
- **Speak sequencing and session lifecycle (AD-10).** Overlay dismissal is synchronous and unconditional; the UI never awaits `TtsPort::generate`. The four sessions are built once and held for the process lifetime (confirmed emphatically by the measurements above); exactly one generation runs at a time, a second Speak Action queues rather than running concurrently. All of it internal to `voice-me-tts`; failures reach core only as the domain error enum.
- **Backend is a build variant + a detected capability + a user choice (AD-7, AD-9).** v1 ships one artefact per backend variant (`cpu`, `cuda`, `local-webgpu`) picked at download time; the variant is a compile-time bound the binary reports about itself. `voice-me-deps` detects usable providers and candidate devices and reports **only** via `AppEvent`; core resolves and records the active backend (plus selected device) in `AppState`; `voice-me-tts` reads that and never calls `voice-me-deps`. Conflicts resolve toward telling the truth in words — never a silent CPU fallback — and what the UI claims must come from what was actually acquired, not what was requested.
- **Weight variant:** FP32 is the intended CPU default (user judged it audibly better; ~18 % end-to-end cost over Q4, since the vocoder dominates and ignores the weight variant). FP16-on-CPU is the worst option. GPU/FP16 via CUDA remains the design but is untested — no capable hardware on the dev machine; WebGPU over Vulkan measured far slower than CPU and, on one device, corrupted audio after a lost device. A `local-webgpu` build must treat a device lost mid-inference as a surfaced error, never as a buffer to play.
- **Shared audio buffer (AD-11).** One core-defined buffer crosses `TtsPort::generate` → `VirtualMicPort::play` — never a byte slice or per-adapter struct. Fixed at 24 000 Hz mono f32, straight out of `conditional_decoder` with no TTS-side conversion; any conversion to what the driver wants (typically 48 kHz) happens inside `voice-me-audio-*`. Reference clips are decoded and resampled to the same format before encoding.
- **Tokio/GPUI bridge (AD-5).** One in-house reimplementation of Zed's `gpui_tokio` pattern, core-owned and shared; never a dependency on Zed's workspace-internal crate and never reinvented per adapter. Generation runs on `spawn_blocking`; GPUI's own spawn is for UI-side async only.
- **Tray:** Linux resolved — `voice-me-tray-linux` uses the `gpui-tray` crate (native to GPUI, no second event loop), verified by DBus StatusNotifierItem registration; `TrayPort::show` takes a `cx: &mut gpui_kit::App`, the one reason core depends on gpui-kit. Fallback is `tray-icon`. Windows is deferred as a `todo!()` stub (no Windows toolchain available), intended via the same crate's `Shell_NotifyIconW` backend.
- **Hotkey feasibility is still unresolved on both OSes** — settle it before either hotkey adapter is implemented.
- **Virtual mic, Linux — resolved and verified.** `voice-me-audio-linux` publishes a PipeWire/PulseAudio null-sink as a virtual *source* named `voice-me` and plays the AD-11 buffer into it through the **PulseAudio client API** (`libpulse`), not the PipeWire-native API: WirePlumber ignores a PipeWire-native client's target when that target is a virtual source, so a native `play` lands on the speakers. No kernel driver, no elevated privileges, no network. The buffer is handed to the server as 24 kHz mono f32 and the server resamples — that server-side resample is the concrete form AD-11's "conversion happens in the audio adapter" takes on Linux; no resampler is hand-written. Persistence is a one-time user-level `pipewire-pulse` drop-in so the device survives logout with voice-me not running; install also loads it for the current session, and uninstall removes both. The recorded fallback (if the Pulse path ever fails) is owning the node with the `pipewire` crate and creating the link explicitly.
- **Three Linux virtual-mic traps, all of which silently route speech to the speakers** — the worst failure this product has, since the user typed precisely because they cannot speak aloud. (a) A `pipewire.conf.d` `context.objects` node looks right but no Pulse client can address it; the drop-in must be a `pipewire-pulse` one. (b) Two devices sharing the name make the name resolve ambiguously to the default sink — install must remove every pre-existing match first, and play must refuse unless exactly one device carries the name. (c) Opening a stream against a *missing* device succeeds and plays to the default sink, so an explicit existence check before playing is what turns "not installed" into a domain error instead of audible speech. Any new playback path must re-prove these, not assume them.
- **Virtual mic, Windows — unresolved.** `VirtualDrivers/Virtual-Audio-Driver` 25.7.14; its named-pipe/IPC control surface from an unsigned app is unconfirmed — confirm it or pick a documented fallback before implementing the adapter. Blocked on Windows machine access.
- OS-native notifications (slow generation, generation failure) go through a core-owned `NotificationPort`, like every other outward effect — never emitted directly from `voice-me-tts` or the UI.
- Settings (hotkey binding, active sample path, UI language, device selection) live in one TOML behind `SettingsStore` inside core; adapters never touch it. Large runtime assets live in a cache directory owned solely by `voice-me-deps`, fetched from a mirrored release or, when too large to mirror, a pinned static origin URL.
- No network egress outside `voice-me-deps`'s declared fetches; the inference engine opens no socket. Enforced by a CI check.

## UX & Interaction Patterns

- Idle is a tray icon and nothing else. Overlay open is an input and nothing else. Success needs no confirmation UI — the audio arriving on the Virtual Microphone is the confirmation.
- The Overlay is a custom composition, not a stock gpui-kit surface: popover-family elevation, the overlay-specific 12 px radius, dark theme only, centered on the active monitor, short fade/scale-in and fade-out and no other animation anywhere in the app. No language switcher, menu, autocomplete, command mode, second hotkey, drag-and-drop, or resizing.
- Hotkey capture (Settings → Hotkey): click "Change", press the combination, it renders live as a hotkey chip for confirmation before Save; conflicts show inline at the field. Chips use the reserved 12 px medium monospace shortcut style on a `muted` background, never used for prose, with platform-native modifier names (Super on Linux, Win on Windows).
- Microcopy is quiet and direct — "Couldn't generate speech. Try again.", not cheerful apology.
- Blocking missing-dependency state (owned by Epic 3, accommodated here): the Overlay still opens on hotkey press but shows an inline notice instead of accepting input.

## Cross-Story Dependencies

- Story 2.2 consumes 2.1's tray decision (`gpui-tray` on Linux; Windows stubbed).
- Story 2.4 needs 2.2 (tray presence) and 2.3 (a configured hotkey).
- Story 2.6 needs 2.4 (Speak Action), Epic 1 (an active Reference Voice Sample), and 2.5 (ported generation loop, measured latency, build-once confirmation, Tokio bridge, shared audio buffer).
- Story 2.9 needs 2.6 (audio exists) and a per-OS virtual-mic adapter: 2.7 is done and proven on Linux (install/play/uninstall plus a device-presence guard), so 2.9's Linux half is wiring that adapter behind `VirtualMicPort` and into the Speak Action; the Windows half waits on 2.8.
- Story 2.7's install/uninstall of the Linux virtual device is a dependency-provisioning surface Epic 3 will present as a dependency row — Epic 3 should drive it, not reimplement it.
- Story 2.5's runtime-asset list (ONNX Runtime library plus the nine model files per backend) is a direct input to Epic 3's provisioning work; Story 2.6 reads backend capability from `AppState`, which Epic 3's dependency check populates, and Story 3.4's notice renders inside the Overlay built in 2.4.
- All Windows-side work here (tray, hotkey, virtual mic) is blocked on access to a Windows machine and toolchain.
