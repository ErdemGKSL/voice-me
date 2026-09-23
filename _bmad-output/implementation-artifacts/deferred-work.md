- source_spec: `_bmad-output/implementation-artifacts/spec-1-1-set-up-the-project-workspace.md`
  summary: Per-OS adapter crates (voice-me-hotkey-linux/windows, voice-me-audio-linux/windows, voice-me-tray-linux/windows) have no target-specific gating, so a later story adding a real OS-only dependency to one of them would break `cargo build --workspace` on the opposite-OS CI runner.
  evidence: Verified `cargo build --workspace` currently succeeds on this toolchain because every adapter crate is still an OS-agnostic `todo!()` stub; the risk is latent, triggered only once a future story adds a genuinely OS-specific dependency (e.g. a Windows-only crate) without adding `[target.'cfg(...)']` gating or excluding it from the opposite-OS CI job.
- source_spec: `_bmad-output/implementation-artifacts/spec-1-5-first-run-voice-setup-prompt.md`
  summary: `voice-me-app`'s composition root has no test coverage at all, including the mapping from a loaded `AppState` to `has_active_sample`/`selected_mic_device` that Story 1.5 added — a regression there (e.g. an inverted condition) would ship with `cargo test --workspace` fully green.
  evidence: Verified `voice-me-app` has no test file and no `[dev-dependencies]` in its `Cargo.toml`; every `voice-me-ui`/`voice-me-tests` test passes these two values as literals rather than deriving them from a loaded `AppState`, so the one site that actually performs the derivation (`crates/voice-me-app/src/main.rs`) is untested. Pre-existing gap in kind — the crate had zero test coverage before this story, closing it means standing up its first test harness, out of proportion to this story's scope.
- source_spec: `_bmad-output/implementation-artifacts/spec-1-5-first-run-voice-setup-prompt.md`
  summary: The "falls back to the OS default if the selected device is no longer present" behavior in `start_capture` (crates/voice-me-ui/src/voice_setup.rs) has no automated test — only verified by code inspection.
  evidence: Confirmed the fallback logic itself is correct by reading `start_capture`'s `named_device.or_else(|| host.default_input_device())`. Exercising it in a test requires faking `cpal`'s `Host`/device enumeration, which `CaptureSource` doesn't expose — it only abstracts the whole `start()` call, not `cpal` enumeration inside the concrete `CpalCaptureSource`. This is the same pre-existing untested-cpal-integration boundary `start_capture` already had (its OS-default-unavailable path was never unit tested either); closing it needs a new fakeable host-abstraction trait.
- source_spec: `_bmad-output/implementation-artifacts/spec-2-1-spike-background-tray-presence-feasibility-on-gpui-gpui-kit.md`
  summary: `LinuxTrayAdapter`'s tray icon has never been visually confirmed rendering in a real system tray panel — only its DBus-level registration and Quit round-trip were confirmed.
  evidence: Verified via `busctl --user list` that the running example claims `org.kde.StatusNotifierItem-<pid>-1` on the session bus, and via a direct `com.canonical.dbusmenu.Event(1, "clicked", ...)` call that its "Quit" menu item correctly exits the process — both protocol-level, not visual. This dev environment's screenshot tooling (`import`/ImageMagick) refused to capture the X11/Wayland display (consistent "missing an image filename" error regardless of output path/format used), so the icon's actual on-screen rendering in GNOME's AppIndicator panel (installed and enabled on this machine) was never seen. A human glancing at the top panel while running `cargo run -p voice-me-tray-linux --example spike` would settle this.
- source_spec: `_bmad-output/implementation-artifacts/spec-2-2-run-as-a-background-tray-resident-process.md`
  summary: The duplicate-window guard (`window_slot`/`activate_window` in `crates/voice-me-app/src/main.rs`) and the whole tray-triggered "Settings…" reopen path are unexercised by any automated test and could not be manually verified end-to-end in this sandboxed environment either.
  evidence: Searched `voice-me-tests` and the whole repo for `SettingsRequested`/`window_slot`/`activate_window`/`on_window_should_close` — no test references any of them; `voice-me-app` has no test file at all. Manual D-Bus verification confirmed the tray-only-to-first-open path (no window → "Settings…" click → window opens), but once a window is open, further dbusmenu-triggered clicks stopped reaching any GPUI action handler in this sandboxed Wayland session (traced to `Window::dispatch_action`'s `cx.defer`-based dispatch never getting a frame/effects-flush for an unfocused, script-driven window) — so the actual "second click activates instead of duplicating" behavior was never observed running. Closing this needs either a GPUI headless test harness for window activation or genuine window-manager interaction (real focus/frame-callback delivery, or actual mouse clicks), neither available in this repo/environment today.
- source_spec: `_bmad-output/implementation-artifacts/spec-2-2-run-as-a-background-tray-resident-process.md`
  summary: `voice-me-tray-windows`'s `todo!()` body now panics at process startup on any Windows build, since `crates/voice-me-app/src/main.rs` calls `WindowsTrayAdapter::show` unconditionally — a real regression from before this story (Windows previously opened the Voice Setup window fine with no tray at all).
  evidence: Confirmed by reading `main.rs`'s `#[cfg(target_os = "windows")]` branch, which calls `WindowsTrayAdapter.show(cx, event_tx.clone())` unconditionally at startup, and `crates/voice-me-tray-windows/src/lib.rs`, whose `show` body is still `todo!()`. No Windows toolchain exists in this dev environment or any CI to build, run, or guard against this; the rest of Epic 2's Windows support (hotkey, audio, virtual-mic adapters) is still `backlog`, so the platform is non-functional regardless of this specific panic. `voice-me-tray-windows` staying a deferred `todo!()` stub is the explicit precedent set by spec-2-1.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-3-configure-a-global-hotkey.md`
  summary: `voice-me-hotkey-windows`'s `todo!()` bodies are now reachable at runtime — a Windows build panics at startup with a saved hotkey, and on every Hotkey-tab Save.
  evidence: Verified real. `main.rs` calls `start_listening` whenever a saved hotkey exists and `HotkeyView::save` calls `rebind`; both are `todo!()` on Windows. Deferred on spec-2-2's precedent for the identical `voice-me-tray-windows` case — no Windows toolchain exists here or in CI to build or verify a guard against, and every Windows adapter in this epic is still a stub. Smallest future fix: return `Err(VoiceMeError::Other(...))` instead of `todo!()`, which the UI's existing `HotkeyIssueKind::Other` path already renders.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-3-configure-a-global-hotkey.md`
  summary: The Wayland/evdev backend enumerates `/dev/input` once at startup — a keyboard connected later is never read, and a reader thread that dies is never replaced.
  evidence: Verified real and acknowledged in `evdev.rs`'s own comments, which name unplug and suspend as causes for a thread exiting. The hotkey can therefore stop working silently after a suspend/resume cycle, with only a stderr line. A missing capability rather than a defect in what was built; closing it needs an inotify watch on `/dev/input` plus reader-thread supervision.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-3-configure-a-global-hotkey.md`
  summary: A `Matcher` is created per input device, so a combination whose modifiers and trigger key arrive on two different event nodes would never match.
  evidence: Unverified (`maybe-false`); would be `medium` if true. `is_keyboard` admits only devices reporting `KEY_A`, which normally also report their own modifiers, but some laptops and USB combo receivers expose several `eventN` nodes. Settling it needs hardware that splits modifiers from the main key matrix; a shared modifier set across matchers would fix it if confirmed.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-3-configure-a-global-hotkey.md`
  summary: X11 `rebind`'s register-before-unregister ordering, and the composition root's startup re-activation of a saved hotkey, are both unexercised by any automated test.
  evidence: Verified by search — no test executes `X11Backend::rebind`, and `voice-me-app` has no test target. Reversing the rebind order would leave a user with no working hotkey after a rejected Save while the UI blames a conflict; deleting the startup block would stop hotkeys surviving a restart. Both stay green today. The first needs a real X display plus a second grab-holding client; the second needs the startup sequence extracted into a library crate — the same gap Story 2.2's tray wiring already carries.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-3-configure-a-global-hotkey.md`
  summary: A configured hotkey can be changed but never cleared — no UI path calls `save_hotkey(None)`, so the "None set" empty state is unreachable once a hotkey exists.
  evidence: Verified real. `SettingsStore::save_hotkey` accepts and persists `None` and is tested for it, and `HotkeyView` renders a `NO_HOTKEY_LABEL` state, but the view offers only Change / Cancel / Save. Rejected from this story as `low`: Story 2.3's intent is "assign and change", not remove, and a Clear control is new public UI surface. Worth adding alongside the Settings shell's later sections, together with an `unbind` on `HotkeyPort`.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-4-summon-type-and-dismiss-the-prompt-overlay.md`
  summary: The composition root's `open_overlay` — one-at-a-time re-summon and stale-handle recovery — has no automated test.
  evidence: Deleting the slot-clearing recovery in `crates/voice-me-app/src/main.rs` makes the hotkey work exactly once per launch while `cargo test --workspace` stays green. `voice-me-app` is a binary crate with no test target and the repo has no window-server harness; the same gap Stories 2.2 and 2.3 already carry.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-4-summon-type-and-dismiss-the-prompt-overlay.md`
  summary: The Prompt Overlay's blur dismissal has no "has been activated at least once" guard, so it may self-close the instant it appears under GNOME/Wayland focus-stealing prevention.
  evidence: `cx.observe_window_activation` dismisses on any `!window.is_window_active()`; a newly mapped window can arrive unfocused under Wayland. Would be high severity if real. Settled by running the built binary in a GNOME/Wayland session and pressing the hotkey — one of this story's own manual checks.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-4-summon-type-and-dismiss-the-prompt-overlay.md`
  summary: CI never runs `cargo test`, so no pipeline executes any of the workspace's tests.
  evidence: Both jobs in `.github/workflows/ci.yml` end at `cargo build --workspace`; `grep -rn "cargo test" .github/workflows/*` returns nothing. Repo-wide and pre-existing, but it is what lets a behavioral regression merge green.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-4-summon-type-and-dismiss-the-prompt-overlay.md`
  summary: With `QuitMode::Explicit` set, a tray-registration failure now leaves the process running with neither tray nor window once the fallback Settings window is closed.
  evidence: `crates/voice-me-app/src/main.rs` falls back to `open_settings` when `TrayPort::show` fails; under the old quit-on-empty default, closing that window ended the process, and it no longer does. Rare path, but the right behavior (quit, retry the tray, or warn) is a product decision rather than a mechanical fix.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-5-spike-in-process-chatterbox-inference-on-onnx-runtime.md`
  summary: The CPU language-model variant is settled on FP32 pending one cheap check — `language_model_q4f16` (304 MB) was never tested and would save 1.7 GB of download if it holds FP32's quality.
  evidence: Post-review measurements on the same prompt: Q4 59 tokens / 2.32 s / 19.13 s total, FP32 57 tokens / 2.24 s / 22.59 s total. The user judged FP32 audibly better, by a margin they described as not large. FP32's cost is ~18% end to end, not the 3.9x the language-model ratio suggests, because `conditional_decoder` dominates both and is indifferent to the weight variant. The 2.08 GB size is no longer an obstacle: AD-7 was revised to allow an oversized asset to be fetched by direct static URL from the MIT-licensed Hugging Face origin instead of being mirrored on GitHub Releases, which Story 3.2's resumable ranged download handles either way. So this is now a quality question only, and q4f16 is the one untested point on the curve.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-5-spike-in-process-chatterbox-inference-on-onnx-runtime.md`
  summary: FP16 was never re-measured after the repetition-penalty fix, so its recorded token count and audio length (61 tokens / 2.40 s) are from the buggy run.
  evidence: The fix changed Q4 from 51 to 59 tokens and FP32 from 61 to 57 on the same prompt; FP16 ran under the same bug and was not repeated. Its timings stand (the penalty costs nothing measurable) but its token accounting does not. Low priority: FP16 on CPU measured 361 ms/token, 12.9x slower than Q4, so it is not a shipping candidate for the CPU variant regardless.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-5-spike-in-process-chatterbox-inference-on-onnx-runtime.md`
  summary: Three WebGPU runs left core dumps on exit (Dawn cleanup after `VK_ERROR_DEVICE_LOST`), and `tests/reference_containers.rs` only covers the mp3/flac/ogg matrix row when `VOICE_ME_TEST_CLIPS` is set, so `cargo test --workspace` does not exercise it on its own.
  evidence: The core dumps affect only the off-by-default `webgpu-probe` build and not anything CI compiles. The container test was run manually against ffmpeg re-encodes of the user's own clip and passed; making it unconditional needs committed binary fixtures, which the crate deliberately avoids.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-6-generate-speech-from-the-prompt-overlay-text.md`
  summary: A permanently failed session build re-shows the "still getting ready, your line will be spoken" notice on every Speak Action, promising something that will not happen.
  evidence: `is_ready()` stays false when the build fails, and `speak_inner` notifies on `!is_ready()`. The failure notification does follow it, so the user is not left misled — but the smallest honest fix needs a "a build has already failed" signal on `TtsPort`, which is new public surface rather than a patch.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-6-generate-speech-from-the-prompt-overlay-text.md`
  summary: The Speak Action queue is unordered and unbounded — rapid hotkey presses can be generated out of the order typed, and each waiter parks a Tokio blocking-pool thread.
  evidence: `SessionSlot` serializes on a `std::sync::Mutex`, which makes no FIFO guarantee. Neither the spec's matrix nor the epic AC promises ordering (both say "queued, not concurrent"), so this is a quality gap rather than a deviation; settling it means a FIFO ticket queue and an in-flight cap.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-6-generate-speech-from-the-prompt-overlay-text.md`
  summary: `voice-me-notify-linux`'s only test sets `DBUS_SESSION_BUS_ADDRESS` process-wide through `unsafe { set_var }`, which becomes racy the moment that crate gains a second test.
  evidence: The SAFETY comment asserts a single-threaded body; cargo's harness is multi-threaded, and the assertion holds today only because the crate has exactly one test. Settling it means a scoped env guard or serialized execution.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-7-spike-linux-virtual-microphone-via-pipewire.md`
  summary: A playback stream opened from the cargo *test* binary is routed to the default sink instead of the Virtual Microphone, while the identical call from an ordinary binary is routed correctly — cause unknown.
  evidence: Reproduced repeatedly with exactly one `voice-me` device present and the stream's own node carrying `target.object = voice-me`; `pw-link` shows it linked to `alsa_output…analog-stereo`. Ruled out: duplicate device nodes, a concurrent recording client (in-process and external both), the stream's application name, and WirePlumber's saved stream state (`~/.local/state/wireplumber/stream-properties` holds volumes only). The only differing stream property is `application.process.binary`. Worked around by making `mic-spike` a `src/bin` so `tests/virtual_mic.rs` drives a real binary; the workaround is sound, but the underlying routing behaviour is unexplained and could bite any future in-test playback.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-7-spike-linux-virtual-microphone-via-pipewire.md`
  summary: The classic-PulseAudio (non-PipeWire) host is entirely unverified — the persistent drop-in does not apply there at all.
  evidence: `~/.config/pipewire/pipewire-pulse.conf.d/` is meaningless to a real PulseAudio daemon, so on such a host only the runtime module load would create the device and it would vanish at logout; persistence would need `~/.config/pulse/default.pa` instead. No classic-PulseAudio machine was available. The matrix row says as much; closing it needs such a host, or a decision that PipeWire is the only supported Linux audio stack.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-7-spike-linux-virtual-microphone-via-pipewire.md`
  summary: `install` tears down and recreates the device on every call, which drops any capture stream another application already had open on it.
  evidence: Unloading every matching module before loading one is what guarantees a single, addressable device, and it is the right trade at voice-me's startup. But a game or Discord already capturing from `voice-me` when voice-me starts loses that stream and has to re-select the device. A narrower fix would reload only when the device is actually unaddressable, which needs a way to detect that — the very thing this spike could not do without playing audio and listening to it.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-7-spike-linux-virtual-microphone-via-pipewire.md`
  summary: Nothing bounds how long a PulseAudio operation can block, so a server that accepts the socket but never answers would hang a Speak Action forever with no error and no speech.
  evidence: `PulseSession::connect` and `PulseSession::wait` both loop on `mainloop.iterate(true)` and escape only on operation completion or a `Failed`/`Terminated` context; a context that stays `Ready` with an operation stuck `Running` never exits. `play` is documented as running on the AD-5 blocking pool, so the thread parks indefinitely. Never observed — settling it means either reproducing a wedged server or adding a deadline to both loops, which is real machinery rather than a guard.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-7-spike-linux-virtual-microphone-via-pipewire.md`
  summary: `play`'s "exactly one device" check is a time-of-check/time-of-use window — the device could vanish between the check and the stream opening, and the audio would then go to the speakers.
  evidence: `play` calls `source_count` on its own short-lived `PulseSession`, then opens `Simple::new` on a second connection. Closing the window needs a way to ask an open stream which device it actually got, which `libpulse-simple` does not offer; the full API's `pa_stream_get_device_name` would, at the cost of dropping the simple API that made this adapter small.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-7-spike-linux-virtual-microphone-via-pipewire.md`
  summary: A failed `load_null_sink` leaves the machine with no Virtual Microphone at all, because `install` unloads every existing module before loading a fresh one.
  evidence: The unload-all-then-load sequence in `LinuxVirtualMicAdapter::install` is what guarantees a single addressable device, and the failure is reported rather than swallowed — but a user who had a working device and a failing install ends up worse off than before. Restoring it means capturing the previous module's arguments and replaying them on failure.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-9-play-generated-audio-through-the-virtual-microphone.md`
  summary: The epic's "heard on both Linux and Windows" criterion is met on Linux only — a Windows Speak Action generates the utterance and then notifies that it has nowhere to play it.
  evidence: `WindowsVirtualMicAdapter::play` now returns `VoiceMeError::VirtualMicUnavailable` naming Story 2.8 instead of `todo!()`, so the process no longer panics on the first Speak Action; the actual driver control surface is Story 2.8's, and the wiring above it is deliberately identical to Linux's. This is spec-2-9 Decision 2, taken by the human — a recorded gap, not an oversight. No Windows toolchain exists here to build or run it either.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-9-play-generated-audio-through-the-virtual-microphone.md`
  summary: The startup ensure's *dispatch* is untested — that it runs at all, that it runs off GPUI's main thread, and that its outcome is logged rather than propagated. The decision it dispatches is covered.
  evidence: `ensure_device` in `crates/voice-me-app/src/main.rs` is behind a private `VirtualMicInstaller` trait with four passing tests (already present, absent then installed, unreachable server, failing install), so Decision 1's sequencing and its never-fatal policy are pinned. What no test reaches is the `cx.background_spawn(...)` call site and `report_ensure`: whether the task is actually spawned, whether it stays off the main thread, and whether a `Failed` outcome only logs. Those need a GPUI harness the repo does not have — `voice-me-app` has no test that starts an `App`.

- source_spec: `_bmad-output/implementation-artifacts/spec-2-9-play-generated-audio-through-the-virtual-microphone.md`
  summary: A Windows build panics at launch, before a Speak Action can reach the virtual-microphone error spec-2-9 Decision 2 added — `WindowsTrayAdapter::show` is still `todo!()` and `main.rs` calls it unconditionally.
  evidence: Found by the spec-2-9 review and verified: `crates/voice-me-tray-windows/src/lib.rs` returns `todo!()` from `show`, and `crates/voice-me-app/src/main.rs` calls `WindowsTrayAdapter.show(cx, …)` with no fallback on the startup path. Decision 2 made `WindowsVirtualMicAdapter::play` honest so a Windows user would get a notification rather than a crash; that is true of the virtual-mic path and moot in practice, because the process aborts long before any line is typed. Pre-existing (spec-2-1 deferred Windows for want of a toolchain), so not this story's to fix — but Story 2.8 should not be judged complete while the app cannot start on Windows at all.

- source_spec: `_bmad-output/implementation-artifacts/spec-3-1-3-4-dependency-check-and-overlay-block.md`
  summary: Confirm every ONNX graph really has an `.onnx_data` sibling, now that the required-file list gates whether the user can type at all.
  evidence: `voice-me-core::assets::required_model_files` unconditionally pairs each of the four graphs with a `<name>.onnx_data`, a rule moved verbatim from `ModelCache::required_files`. Under Story 3.1 a false "missing" no longer fails at generation time — it blocks the Prompt Overlay. Settled by listing a real provisioned cache and checking whether the small graphs (`embed_tokens`, `conditional_decoder`) carry external data at all.

- source_spec: `_bmad-output/implementation-artifacts/spec-3-1-3-4-dependency-check-and-overlay-block.md`
  summary: `PulseSession::connect_with`'s readiness loop has no timeout, so an audio server that connects but never becomes ready hangs its caller forever.
  evidence: `crates/voice-me-audio-linux/src/pulse.rs:79-90` iterates the mainloop until `Ready`, `Failed` or `Terminated`, with no deadline. Pre-existing, but the Dependency Check gives it new callers — including, before this review's patch, the UI thread.

- source_spec: `_bmad-output/implementation-artifacts/spec-3-1-3-4-dependency-check-and-overlay-block.md`
  summary: The dependency row set is derived from the backend's weight variant only, never its execution target, so a GPU selection would report the CPU row set.
  evidence: `speech_engine_rows(root, backend.weights)` in `voice-me-deps` never reads `backend.target`. Harmless today — no GPU backend can be selected until Story 3.5 — and provider-library rows belong to Stories 3.3/3.8.

- source_spec: `_bmad-output/implementation-artifacts/spec-3-1-3-4-dependency-check-and-overlay-block.md`
  summary: When Story 3.2 adds provisioning, `DependencyProvisioningPort` failures raised outside the event channel will diverge between the Dependencies tab and the overlay gate.
  evidence: `DependenciesView::check_again` writes a `Failed` outcome into the view only; the composition root's held outcome is updated solely from `AppEvent`. Unreachable in Story 3.1 (the one `Err` source depends on process environment that cannot change after launch), but provisioning adds reachable failures. The fix is an `AppEvent` variant carrying the failure.

- source_spec: `_bmad-output/implementation-artifacts/spec-3-3-be-honest-when-the-selected-backend-cant-run-here.md`
  summary: On Windows, a WebGPU selection on a machine with a D3D12 adapter but no Vulkan loader is reported as "can't run here", because the capability probe only asks Vulkan.
  evidence: `crates/voice-me-deps/src/capability.rs` probes WebGPU capability through `ash`'s runtime-loaded Vulkan loader only (`vulkan-1.dll` on Windows). Dawn's WebGPU backend on Windows prefers D3D12, so a D3D12-only adapter (no Vulkan ICD installed) is a real, runnable machine that the row would block. A DXGI adapter enumeration (skipping WARP / software adapters, the D3D12 analogue of excluding `PHYSICAL_DEVICE_TYPE_CPU`) is the fix; there is no Windows toolchain here to build or verify it.

- source_spec: `_bmad-output/implementation-artifacts/spec-3-3-be-honest-when-the-selected-backend-cant-run-here.md`
  summary: The CUDA capability row says "can run here" on a machine with a good NVIDIA driver and GPU but no CUDA runtime, cuBLAS or cuDNN, which ONNX Runtime's CUDA provider needs at registration.
  evidence: `crates/voice-me-deps/src/capability.rs` checks only the driver (`libcuda`) and compute capability, and `probe_runtime`'s `is_available()` reports only that the provider was compiled in. The first session build then fails with an engine error (honestly — "Active: none — …", no CPU fallback), which is the late failure Story 3.3 set out to prevent. The fix is a probe that dlopens `libcudart`/`libcublas`/`libcudnn` (or the ORT CUDA provider shared library) — best settled together with Story 3.8, which decides which CUDA libraries ship alongside the all-provider runtime.

- source_spec: `_bmad-output/implementation-artifacts/spec-3-6-generate-through-a-remote-provider.md`
  summary: Nothing tests that the overlay's confirm callback in `main()` actually persists the selected provider's disclosure, so a regression there would lock DeepInfra behind a repeating confirm loop.
  evidence: The callback is built inline in `main()` (`save_disclosure_confirmed` has no other non-test caller). The view tests use a counter closure, and `disclosure_needed` and the store round-trip are tested separately. Closing the gap means extracting a `disclosure_confirm_callback(store, provider)` helper and testing it against a temp-dir `FileSettingsStore`.
- source_spec: `_bmad-output/implementation-artifacts/spec-3-10-give-backends-their-own-settings-tab.md`
  summary: The tts-remote no-key error still says "add one under Settings → Dependencies"; since Story 3.10 it should say Settings → Backend.
  evidence: `crates/voice-me-tts-remote/src/lib.rs:235`; the capability row (`voice-me-deps/src/capability.rs:312`) already says Settings → Backend, so the two surfaces now disagree. It is reached only if the key vanishes between the Dependency Check and the Speak Action. Spec-3-10's frozen Never excluded `voice-me-tts-remote`; the test at `voice-me-tts-remote/src/tests.rs:503` checks only "no API key".
- source_spec: `_bmad-output/implementation-artifacts/spec-3-11-choose-the-speech-language-per-backend.md`
  summary: The composition root's `backend_actions` dispatch (including 3.11's `SetSpeechLanguage` arm) lives inline in `fn main` and no test drives it.
  evidence: The verification-gap review of 3.11 showed that dropping the arm's error removal or its `push_panel` would pass CI. Every other `BackendAction` arm has the same gap. Fixing it means pulling the dispatch into a testable function.
