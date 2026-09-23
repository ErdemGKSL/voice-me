---
title: 'Selectable backends, API keys, and honesty when the selected backend cannot run here (Story 3.3, with 3.5 and 3.6 key management)'
type: 'feature'
created: '2026-09-23'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
baseline_commit: '43ca8d54c7df65c9018defd069288d1fb599f14b'
context: ['{project-root}/_bmad-output/implementation-artifacts/epic-3-context.md']
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Today the backend is hard-coded to CPU (`resolved_speech_backend()`). The user cannot add a GPU-capable ONNX Runtime, cannot choose a backend, and cannot enter a provider API key. Nothing checks whether this machine can run a given choice either: a CUDA selection with no usable NVIDIA GPU would fail only at the first Speak Action, with an engine error. The Dependencies tab also prints what was *selected* as if it were what is running, which is the claim AD-9 forbids.

**Approach:** Settings → Dependencies gets four things:
1. **Add runtime…** opens a file picker for an ONNX Runtime library. The app finds out which execution providers the library supports, and each supported provider becomes a local backend entry.
2. A backend `Select` lists the bundled CPU runtime, the entries from each added runtime, and the two remote providers. The choice is persisted.
3. An **API keys** section with a masked field for DeepInfra and for fal.ai.
4. Checks on whether the selected backend can run here. The Dependency Check probes the hardware and the key, and when the selection cannot run it shows a speech-blocking row: the reason in words, what the CPU backend would do instead, and a **Use CPU backend** button.

The backend line shows two separate facts: *Selected*, and *Active* (what the speech engine actually acquired when it built its session).

## Boundaries & Constraints

**Always:**
- **Where capability comes from.** Capability reaches core only inside the `DependencyReport` on `AppEvent` (AD-3/AD-9). The TTS crate never calls deps, and deps never calls TTS.
- **Where "Active" comes from.** "Active" comes only from the TTS adapter's report of the session it built.
- **CPU is a normal choice.** CPU selected on a machine with a good GPU shows only an informational "CPU mode" badge, never a warning (UX-DR18).
- **Capability misses block speech.** A capability miss blocks the overlay through Story 3.4's existing gate.
- **Probes never crash or hang the check.** A missing driver library means "not found"; the driver's own error text is shown as-is.
- **Weights follow the target.** CPU uses Q4 weights. CUDA and WebGPU use FP16 (AD-12), so provisioning still fetches only the selected backend's variant.
- **API keys.** Keys are stored in plaintext in the settings file. The UI says so next to the key fields. Keys never appear in logs or `Debug` output.
- **Only three places touch the network.** Network code stays in `voice-me-deps`. No remote request is made in this story.

**Never:**
- **No silent fallback.** A GPU or remote selection is never quietly run on CPU. It fails, naming the reason.
- **Nothing is downloaded for a GPU runtime.** Downloading a GPU runtime is Story 3.8. No remote speech generation (Story 3.6).
- **No GPU device picker** (Story 3.9).
- **An added library is never loaded into the main process just to probe it.**

## Decisions

1. **Local backends come from libraries the user adds (answered 2026-09-23).** **Add runtime…** opens a native file picker (`cx.prompt_for_paths`) for a `.so`/`.dll`. The type is auto-detected only, with no user override. The app re-executes itself as `voice-me --probe-runtime <path>` in a helper process (10 s timeout). The helper loads the library through `ort` and prints which of `CPUExecutionProvider`, `CUDAExecutionProvider` and `WebGpuExecutionProvider` are available. The library then yields one entry per available GPU provider, plus a CPU entry when it reports no GPU provider at all. A file that fails to load is refused with the helper's reason. The bundled runtime (`ORT_DYLIB_PATH`, or the cache copy) is always listed as the built-in "CPU (bundled runtime)" entry. A selected added library takes priority over `ORT_DYLIB_PATH`. Added runtimes can be removed.
2. **Switching library needs a restart (answered 2026-09-23).** `ort` commits one library per process. The root records which library it committed. If the new selection uses a different library than the committed one, the selection is saved at once, and the tab says "Takes effect after restart" with a **Restart now** button. That button relaunches `current_exe()` and quits. If no library has been committed yet, or the new selection uses the same library, the switch applies to the next Speak Action with no restart: the root rebuilds the TTS adapter.
3. **Remote providers can be selected now (answered 2026-09-23).** DeepInfra and fal.ai appear in the `Select`. Choosing one gives a blocking capability row. With no key, the row says "DeepInfra has no API key — add one under API keys" and offers Use CPU. With a key, the row says "Remote generation through DeepInfra arrives in a later voice-me release" and offers Use CPU. Story 3.6 replaces the second sentence with real generation.
4. **API keys: DeepInfra and fal.ai (answered 2026-09-23).** Each has its own masked `Input` (`mask_toggle`) with Save and Remove. They are stored independently.
5. **The CUDA floor is compute capability 6.0,** per spec-2-5 Decision 1. It is one constant, and Story 3.8 may revisit it.
6. **Size:** the full spec is kept as-is, knowing it is above the token target (answered 2026-09-23).

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| CPU, GPU present | CPU selected, good NVIDIA GPU | "CPU mode" badge, no capability row, not blocked | N/A |
| CUDA, no driver | CUDA entry selected, no `libcuda` | Row "can't run here: No NVIDIA driver found." + the CPU sentence + Use CPU backend; overlay blocked | N/A |
| CUDA, old GPU | Compute capability 5.0 found | Row names the device and "5.0 < 6.0 required" | N/A |
| WebGPU, no adapter | No Vulkan loader, or only a CPU-type adapter (llvmpipe) | Row names what was found | N/A |
| Add runtime: CUDA build | Picked a library whose probe reports CUDA and CPU | One entry, "CUDA (<file name>)", appears in the Select | N/A |
| Add runtime: bad file | Picked a file that is not an ONNX Runtime library | Nothing is added | Inline error with the helper's reason |
| Probe hangs | The helper is silent for more than 10 s | Helper killed, nothing added | "Probing <file> timed out" |
| Switch, same library | Committed the bundled library; select another entry backed by the same library | Applies on the next Speak Action, no restart | N/A |
| Switch, other library | Committed library A; select an entry backed by library B | Saved; "Takes effect after restart" + Restart now | Relaunch failure is shown inline |
| Remote, no key | DeepInfra selected, key empty | Blocking row pointing at API keys + Use CPU | N/A |
| Key saved | Enter a key and click Save | Persisted; the field shows it masked; the check re-runs | A save error is shown inline |
| Use CPU clicked | Any capability row | Selection = bundled CPU, persisted; the check re-runs; the row goes away | Save error on the row |
| Active states | No session built / build OK / build failed | "Active: not started yet" / "Active: CUDA — FP16 weights" / "Active: none — <engine error>" | N/A |

</frozen-after-approval>

## Code Map

- `crates/voice-me-core/src/state.rs:15-72` -- `SpeechExecutionTarget`: add `Cuda`. Add a persisted `BackendSelection { Local { runtime: Option<PathBuf> /* None = bundled */, target }, Remote(RemoteProvider) }`, plus `RemoteProvider { DeepInfra, FalAi }` with a display label. `SpeechBackend` stays the resolved value and gains a `for_target` constructor (the target implies the weights). `DependencyKind`: add `BackendCapability` (`blocks_speech`). `AppState`: add `backend_selection`, `local_runtimes: Vec<LocalRuntime { path, targets }>`, `api_keys` with a redacted `Debug` impl, and `active_backend: ActiveBackend { NotStarted, Acquired(SpeechBackend), Failed(String) }`, which is not persisted.
- `crates/voice-me-core/src/settings_store.rs:43-200` -- `SettingsFile`: add the selection, the runtimes and the keys, each with a serde default so old files still load. Add `save_backend_selection`, `save_local_runtimes`, and `save_api_key(provider, Option<&str>)`, in the pattern of `save_hotkey`.
- `crates/voice-me-core/src/event.rs` -- `AppEvent::SpeechSessionBuilt { result: Result<SpeechBackend, String> }`.
- `crates/voice-me-core/src/assets.rs` -- `resolve_runtime_dylib` gains the selected library's override (Decision 1).
- `crates/voice-me-core/src/ports.rs:180` -- `check` takes a `CheckRequest { backend: SpeechBackend, selection: BackendSelection, has_api_key: bool }` in place of a bare `SpeechBackend`. Update the stubs in `voice-me-ui` (`dependencies.rs:576,732`; `settings.rs:221`).
- `crates/voice-me-deps/src/capability.rs` (new) -- a `GpuProbe` trait, injected like `with_virtual_mic_installer`. CUDA uses `libloading` on `libcuda.so.1`/`nvcuda.dll` and calls `cuInit`, `cuDeviceGetCount`/`Name`/`Attribute` (75/76). Vulkan uses `ash` with the `loaded` feature (`ash` is already in `Cargo.lock`), where a `PHYSICAL_DEVICE_TYPE_CPU` device does not count. It also builds `capability_row(request, probe)`, which returns `None` for CPU.
- `crates/voice-me-deps/src/lib.rs:177,301,397` -- put the capability row first in `check`. `gpu_runtime_unavailable` names CUDA. A selected added library is reported through `runtime_row` like any other path.
- `crates/voice-me-tts/src/sessions.rs:74-120` -- `ExecutionTarget::Cuda` → `ort::ep::CUDA` with `.error_on_failure()`. Add `pub fn probe_runtime(path) -> Result<Vec<SpeechExecutionTarget>, VoiceMeError>` (`init_from(path)`, then each EP's `is_available()`). `init_runtime` takes the resolved path.
- `crates/voice-me-tts/src/lib.rs:112-240` -- `execution_target_for` maps every target and never falls back. The adapter takes an optional `AppEventSender` and sends `SpeechSessionBuilt` from `build_sessions`, whether the build succeeds or fails.
- `crates/voice-me-app/src/main.rs:136,248,575,1076,1240-1275,1406` -- replace `resolved_speech_backend()` with a resolution from `state.backend_selection`. Add the `--probe-runtime` early-exit mode at the top of `main`. Hold the TTS adapter in a swappable slot. Record the committed library. Handle `SpeechSessionBuilt`. Wire the view's callbacks (select, add or remove runtime, save key, use CPU, restart), each of which saves through `SettingsStore` and then re-runs the check.
- `crates/voice-me-ui/src/dependencies.rs:440-488` -- replace `backend_summary` with a gpui-kit `Select`, the Selected and Active lines, the CPU-mode `Badge`, the restart line and button, **Add runtime…** with a list of added runtimes (each with Remove), and the API keys section with its plaintext notice. The capability row's action is **Use CPU backend**. The view calls root-supplied callbacks and never touches `SettingsStore`. Read the gpui-kit design guides before laying this out.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-core/src/{state,event,settings_store,assets,ports}.rs` -- add the selection, runtimes, keys, `ActiveBackend`, `SpeechSessionBuilt`, `CheckRequest`, and the `Cuda` target, with settings round-trip tests and a test that `Debug` redacts keys.
- [x] `crates/voice-me-deps/src/capability.rs` + `lib.rs` -- add the probes and the capability row; test every matrix row with a fake probe.
- [x] `crates/voice-me-tts/src/{sessions,lib}.rs` -- add the honest CUDA target, `probe_runtime`, and the `SpeechSessionBuilt` report; test that an unreachable target reports `Failed` and never CPU.
- [x] `crates/voice-me-app/src/main.rs` -- add the probe mode, the resolution, the adapter swap, restart detection, and the event handling; extract the decisions as pure functions (`needs_restart`, `resolve_backend`) and test them.
- [x] `crates/voice-me-ui/src/dependencies.rs` -- add the selector, the Selected/Active lines, the badge, runtimes, keys, and Use CPU; UI tests cover the capability row with its button and the callback firing once, a masked key saved through the callback, and the restart line appearing only when needed.
- [x] `crates/voice-me-deps/Cargo.toml` -- add `libloading` and `ash` (`loaded`).
- [x] `_bmad-output/implementation-artifacts/deferred-work.md` -- add an entry for D3D12-only Windows adapters (no toolchain to verify them).

**Acceptance Criteria:**
- Given CUDA is selected on this machine (GM107, nouveau), when the app starts, then Dependencies opens by itself saying "No NVIDIA driver found." with Use CPU, and the hotkey opens a blocked overlay naming the same reason.
- Given that row, when Use CPU is clicked, then the row disappears, the badge reads "CPU mode", and after the next Speak Action "Active" reads "CPU — Q4 weights".
- Given any state, "Active" never names a target the engine did not build a session on.
- Given the CPU backend, when provisioning runs, then no FP16 file is fetched.

## Implementation Notes

## Spec Change Log

## Review Triage Log

Pass 1 (blind-hunter, edge-case-hunter, verification-gap).

| Verdict | Finding | Evidence |
|---|---|---|
| medium | A late `SpeechSessionBuilt` from a replaced adapter overwrites "Active" (blind, edge ×2, verification-gap other) | Confirmed: the old adapter keeps a clone of the event sender, its warm-up can run 86–110 s, and the handler writes "Active" unconditionally. Breaks the "Active" acceptance criterion. The event is new in this diff, so tagging it adds no outside surface. → patch (a generation number per engine) |
| medium | `warmed_up` is never reset, so an engine rebuilt by a switch is never warmed (blind, edge) | Confirmed: the flag is set once in the check handler and not cleared in `apply_selection`. → patch |
| medium | The warm-up condition is untested (verification-gap, pre-verified) | Dropping `runnable &&` passes every test. → patch (pure `should_warm_up` + test), grouped with the row above |
| medium | Settings file holding plaintext keys is written 0644 (blind) | Confirmed: `fs::write` with the default umask, so any local user can read the keys, contradicting the notice. → patch (0600 on Unix) |
| medium | `from_state`'s runtime resolution is untested (verification-gap, pre-verified; blind) | Its only caller is `main.rs`; the replacement test exercises `new`/`with_runtime` only. → patch |
| medium | Check again and Install forwarding the panel's request is untested (verification-gap, pre-verified) | `CountingDepsPort` ignores its arguments. → patch |
| medium | `build_engine`'s refusal for a remote selection is untested (verification-gap, pre-verified) | No test calls `build_engine`. → patch |
| low | The CUDA row passes on any device, but the engine builds on device 0 (blind, edge) | Confirmed for multi-NVIDIA machines; the fix is a direct correction. → patch |
| low | Ignored `cuDeviceGet`/`cuDeviceGetAttribute` return codes show as "compute capability 0.0" (blind, edge) | Confirmed; a direct correction. → patch |
| low | The Select keeps showing a choice whose save failed (blind, edge) | Confirmed: `panel_stale` is only set when the selection, keys or runtimes change. A one-line correction. → patch |
| low | An `Err` from `try_wait` orphans the probe helper (edge) | Confirmed; a direct correction. → patch |
| low | A warm-up in flight can commit a library after the restart decision (blind, edge) | Real, but `init_runtime` runs within milliseconds of the build starting, so the window is milliseconds. The fix needs in-flight state. Rejected |
| low | A check for the previous selection, landing after a switch, can start a warm-up (edge) | Same millisecond window; the fix adds a guard on state not demonstrated. Rejected |
| low | The CUDA provider also needs cuDNN/cuBLAS, which the row does not check (blind) | Real for a user-added CUDA library without cuDNN, but the engine then fails honestly and "Active" names the error. The fix is a new probe. Rejected (worth revisiting with Story 3.8) |
| low | A hung probe leaks a thread on every check (blind, edge) | Needs a driver that hangs; the fix adds process-wide state. Rejected |
| low | Paths are compared as raw strings, not canonicalized (blind, edge) | Both sides come from the same `resolve_runtime_dylib`; a mismatch needs the bundled library added by another spelling. Rejected |
| low | Unreadable newer settings are dropped on the next save; `api_keys` is not lenient (blind) | Needs a downgrade or a hand-edited file. Rejected |
| low | The remote Speak notice ignores whether a key exists, and the rebuild on key save does nothing (blind) | Reachable only before the first check lands, when the capability row starts blocking. Harmless. Rejected |
| low | Restart waits a flat 1 s off Linux; a slow old instance can still hold the tray (blind, edge) | Windows is unverifiable here, and its startup is already broken (existing deferred entry). Rejected |
| low | Identical labels for two added libraries with the same file name and target (edge) | The runtimes list shows full paths; the fix needs disambiguation logic. Rejected |
| low | Removing a runtime while the same path is being probed makes it reappear (edge) | Needs a remove click during a sub-second probe. Rejected |
| low | `make_panel` hides a settings load failure (edge) | A failed load also breaks every other settings read; rare. Rejected |
| low | A selection missing from the choices leaves the Select blank (edge) | Only a hand-edited file reaches it; removing a runtime in the UI resets the selection. Rejected |
| low | The old report stays in force until the check after a switch lands (verification-gap other) | A window of seconds; the engine fails honestly rather than falling back. Rejected |
| low | The drain threads can block if a grandchild holds the pipe (edge) | The helper is voice-me itself, which spawns nothing. Rejected |
| low | `gpu_targets_refuse_to_fall_back_to_cpu` asserts on `ort`'s Debug text (blind) | Brittle only across `ort` upgrades; the test fails loudly if it changes. Rejected |
| low | The acceptance criteria's overlay and rendered-"Active" paths have no end-to-end test (blind) | `speech_engine_blocker` and `blocks_speech` are both tested; `overlay_blocker` is unchanged. Rejected |
| low | `probe_runtime` without `dynamic-runtime` reports the static runtime (blind) | Only reachable in the never-shipped `webgpu-probe` spike build. Rejected |
| false | Missing macOS Vulkan portability flag (blind) | voice-me targets Linux and Windows only; there is no macOS adapter crate. |
| false | Non-adjacent duplicate targets duplicate Select entries (edge) | `probe_runtime` pushes CPU, CUDA and WebGPU at most once each, in order. |
| false | Warm-up no longer runs when the check could not run (edge) | A check fails only on an unresolvable cache root, and then `from_state` fails the same way, so there is no engine to warm. |
| false | `entries()` drops the CPU provider of a GPU library (edge) | This is frozen Decision 1: "one entry per available GPU provider, plus a CPU entry when it reports no GPU provider at all". |
| false | Planning files are inconsistent (spec status vs sprint status, empty notes) (blind) | Sprint status is synced at the end of this workflow; Implementation Notes are written there too. |

Pass 1 (blind-hunter, edge-case-hunter, verification-gap).

| Verdict | Finding | Evidence |
|---|---|---|
| medium | A replaced engine's late `SpeechSessionBuilt` overwrites "Active" (blind, edge, verification-gap) | Confirmed: every adapter shares `event_tx`, and the handler writes "Active" with no check of which engine sent it. A CPU warm-up finishing after a switch to DeepInfra reads "Active: CPU — Q4". → patch (a per-engine generation filter inside `main.rs`, no public surface) |
| medium | `warmed_up` is never reset, so any engine built after a switch is never warmed up (blind, edge) | Confirmed: it is set once and never cleared; the first Speak Action after Use CPU pays the whole 86–110 s build. → patch |
| medium | A check for the *previous* selection landing after a switch can start warm-up on an engine that cannot run (edge) | Confirmed: the handler reads only `runnable` from whatever report arrived. Same cause as the row above (the warm-up gate does not know about the current selection). → patch |
| medium | The warm-up gate has no test (verification-gap, pre-verified) | Deleting `runnable &&` passes the suite. Same group → patch (extract `should_warm_up` and test it) |
| low | A warm-up that is still in `init_runtime` makes `committed_runtime()` read `None` at a switch (blind, edge) | Real, but the window is the few milliseconds between a build starting and `init_from` returning, and the result is an honest "restart voice-me" engine error. The fix would add in-flight state. Rejected |
| low | The CUDA row passes if *any* device is capable, but the engine uses device 0 (blind, edge) | Confirmed on multi-GPU machines. A direct correction (judge the first device). → patch |
| low | `cuDeviceGet`/`cuDeviceGetAttribute` return codes are ignored, so a failed call reads "0.0 < 6.0" (blind, edge) | Confirmed. A direct correction (return the driver's error). → patch |
| low | Hung probe threads pile up on every check (blind, edge) | Real only with a driver that hangs; the fix adds process-wide state. Rejected. The hard-coded "5 seconds" in the messages is a direct correction → patch |
| medium | CUDA "can run here" ignores cuDNN/cuBLAS, which the CUDA provider also needs (blind) | Real: a good driver with no cuDNN passes, and the first build fails (honestly, as "Active: none"). A new probe is beyond the spec's checks. → defer |
| false | WebGPU on macOS needs `VK_KHR_portability_enumeration` (blind) | The workspace targets Linux and Windows only; there are no macOS adapter crates. |
| medium | Settings holding API keys are written with the default umask, i.e. world-readable (blind) | Confirmed: `fs::write` leaves 0644, which contradicts the notice's "readable by anyone with access to your account". → patch (0600 on Unix) |
| low | Unreadable newer-version selection or runtimes are dropped on the next save (blind) | Real, but needs a downgrade after a newer version wrote the file. Rejected |
| low | `api_keys` is not lenient, so a malformed table makes the whole file unreadable (blind) | Confirmed. A direct correction with the existing helper pattern. → patch |
| low | After a failed save the `Select` keeps showing the unsaved choice (blind, edge) | Confirmed: the pushed panel's selection is unchanged, so the controls are never resynced. A direct fix. → patch |
| low | Paths are compared without canonicalising (blind, edge) | Both sides of `needs_restart` come from the same resolver with the same input, so the default case cannot differ. Only an added file that is the bundled one under another spelling can. Rejected |
| low | Saving a key rebuilds the engine for a remote selection, but `build_engine` never reads the key (blind) | Confirmed dead code. A direct deletion. → patch |
| low | `probe_runtime` in a non-`dynamic-runtime` build reports the static runtime's providers (blind) | Only the never-shipped `webgpu-probe` spike build. Rejected |
| low | The `from_state` resolution path is untested since the old test was removed (blind, verification-gap, pre-verified) | Switching `from_state` to pass `None` passes the suite. → patch (test) |
| low | The provider test asserts on `ort`'s `Debug` text (blind) | Fragile only on an `ort` bump, which the test would announce. Rejected |
| low | Restart waits a flat 1 s off Linux (blind, edge) | Windows cannot start today anyway (the tray is `todo!()`, deferred-work). Rejected |
| low | No test that a capability row reaches the overlay, or of the rendered "Active" text (blind) | `speech_engine_blocker` returning the capability row is asserted in the deps tests, and `overlay_blocker` is unchanged. Rejected |
| false | Planning files disagree (status, empty notes) (blind) | The status is synced in step 5; the fix edits the spec. |
| low | `try_wait` erroring leaves the probe helper orphaned (edge) | Confirmed. A direct correction (kill before returning). → patch |
| low | A grandchild holding the helper's pipes blocks the drain join (edge) | ONNX Runtime spawns no processes. Rejected |
| low | Waiting more than 10 s for the old instance, then claiming the tray (edge) | Only a hung old instance. Rejected |
| low | `settings_store.load()` failing in `make_panel` shows CPU silently (edge) | A settings read that fails at runtime after a successful startup. Rejected |
| low | Removing a runtime while its probe is in flight makes it reappear (edge) | Needs a remove during the ≤10 s probe of the same file. Rejected |
| low | A persisted selection missing from the choices leaves the `Select` blank (edge) | Hand-edited files only; the check still reports honestly. Rejected |
| false | Duplicate non-adjacent probe targets list an entry twice (edge) | The helper prints targets in the fixed order cpu, cuda, webgpu, each at most once. |
| low | Two added runtimes with the same file name have identical labels (edge) | Only two added libraries offering the same target. Rejected |
| false | Warm-up no longer runs when the check fails (edge) | The check fails only on an unresolvable cache root, where the engine cannot build either. |
| false | `entries()` drops CPU for a GPU runtime (edge) | Decision 1 says exactly that. |
| medium | `check_again` and Install forward the panel's request with no test (verification-gap, pre-verified) | The fake port ignores its arguments. → patch (record and assert) |
| medium | `build_engine`'s refusal to build CPU for a remote selection has no test (verification-gap, pre-verified) | Deleting the early return passes the suite. → patch (test) |
| low | The previous report gates speech until the new check lands after a switch (verification-gap other) | The new engine fails honestly in that window. Rejected |

## Verification

**Commands:**
- `cargo check --workspace` -- expected: clean, no new warnings.
- `cargo test --workspace` -- expected: green, needing no GPU (the probes are injected).

**Manual checks:**
- Add the bundled `libonnxruntime.so` through **Add runtime…**. Expect one "CPU (libonnxruntime.so)" entry.
- Select a CUDA entry and restart. Expect the capability row.
- Save a DeepInfra key. Expect `settings.toml` to hold it and no log line to show it.
