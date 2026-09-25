---
title: 'Choose the engine first, then the device it runs on'
type: 'feature'
created: '2026-09-25'
status: 'done'
route: 'dispatch'
baseline_commit: 'a50ccc9d967814d53bfb620fca6450a93544ddbc'
review_loop_iteration: 1
context:
  - '{project-root}/_bmad-output/implementation-artifacts/spec-3-8-build-and-mirror-the-all-provider-onnx-runtime.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** The Local backend `Select` mixes engines and devices. Chatterbox appears once per runtime and target ("CPU (bundled runtime)", "CUDA (libonnxruntime.so)"), while Piper is a single CPU-only entry. Piper can run on CUDA or WebGPU too, through the all-provider runtime from Story 3.8.

**Approach:** The Local `Select` lists engines only: Piper, Chatterbox and System voice. Picking an ONNX engine (Piper or Chatterbox) shows a second `Select` under it for the device: CPU, CUDA or WebGPU on the bundled runtime, plus, for Chatterbox, each added runtime's targets. Piper gains a CUDA/WebGPU target end to end: the saved selection, dependency rows, Install, and session.

**Decisions (user delegated, 2026-09-25):**
1. This supersedes Piper sprint-change decision P6 ("CPU only").
2. Device entries use short labels: "CPU", "CUDA", "WebGPU". An added runtime's entry reads "CUDA — libonnxruntime.so".
3. The bundled CUDA and WebGPU entries are listed only when the bundled runtime has every provider (`runtime_all_providers`, i.e. once 3.8 is pinned). Until then the bundled runtime offers CPU only.
4. Switching engine keeps the current device when the new engine offers it, otherwise CPU. Piper runs on the bundled runtime only.
5. The Local/Remote radio and the Remote list stay as they are. The GPU device-index picker (Story 3.9) stays out.

## Boundaries & Constraints

**Always:**
- Old `settings.toml` files load unchanged. `kind="piper"` without a target is Piper on CPU. A new file read by an old build falls back leniently, as today.
- A Piper GPU selection gets the same runtime, CUDA provider and NVIDIA library rows, Install, and restart rule as Chatterbox. It never gets the model-weights row.
- AD-1: `voice-me-tts-piper` does not depend on `voice-me-tts`. The execution providers and CUDA library preload are injected, like `runtime_init` is today.

**Never:**
- No change to remote backends, voices, languages or Piper voice management.
- No new settings tab.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Engine list | Local kind | Engine `Select`: Piper, Chatterbox, System voice | — |
| Device list | Chatterbox, all-provider runtime, one added CUDA runtime | CPU, CUDA, WebGPU, "CUDA — libonnxruntime.so" | — |
| No device | System voice or a remote backend | No device `Select` | — |
| Piper on CUDA | Piper + CUDA selected, NVIDIA driver | Rows: runtime, CUDA provider, NVIDIA libraries, voice; Install fetches them; speech runs on CUDA | Missing piece → its own row |
| Old file | `kind="piper"` | Piper, device CPU | — |
| Engine switch | Chatterbox+WebGPU → Piper | Piper+WebGPU | Target not offered → CPU |

</frozen-after-approval>

## Code Map

- `crates/voice-me-core/src/state.rs:696-888` -- `BackendSelection` becomes `Piper { target }`. Update `local_target`, `is_cpu`, `label` and `default`, and split `backend_choices` into engine choices plus `device_choices(engine, runtimes, all_providers)`. `LocalRuntime::entries` stays; it feeds the added runtimes' device entries.
- `crates/voice-me-core/src/settings_store.rs:280-322` -- `SelectionFile::Piper { #[serde(default)] target }`, plus a round-trip test and an old-file test.
- `crates/voice-me-ui/src/backend.rs:427-494,861-915` -- `choices()` returns engines. Add `backend-device-select`, shown for Piper and Chatterbox; its Confirm sends `BackendAction::Select` with the combined selection. Update the tests listed at 1693-2192 and 2644, and `dependencies.rs:1206`.
- `crates/voice-me-deps/src/capability.rs:297-386` -- the Piper arm gets `cuda_row`/`webgpu_row` like `Local`.
- `crates/voice-me-deps/src/lib.rs:250-259,799-891,1082-1103` -- drop the CPU override for Piper. `piper_rows` passes Piper's backend to `runtime_row` and adds the CUDA provider and NVIDIA rows. Reuse `speech_engine_rows`' CUDA branch, but not `model_weights_row`.
- `crates/voice-me-tts-piper/src/lib.rs:249-337` -- `PiperTts::new` takes the providers or a session-setup closure, replacing the hard-wired `ort::ep::CPU` at :326. Add `ort`'s `cuda`/`webgpu` features in `Cargo.toml`.
- `crates/voice-me-tts/src/sessions.rs:76-115,305` -- the `ExecutionTarget::providers()` and `preload_cuda_libraries` the app injects into Piper.
- `crates/voice-me-app/src/main.rs:509-855,2261,3774-3800` -- update `resolve_backend`, `is_gpu_selection` (Piper GPU now counts) and `build_piper` (pass the target's providers and preload), and pass the engine/device state to the Backend tab.
- `_bmad-output/planning-artifacts/ux-designs/ux-voice-me-2026-09-20/EXPERIENCE.md:53` and `README.md` -- describe the engine and device selects.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-core/src/{state,settings_store}.rs` -- the Piper target, engine and device choices, persistence and compatibility. Tests for the matrix rows Engine list, Device list, Old file and Engine switch.
- [x] `crates/voice-me-deps/src/{capability,lib}.rs` -- Piper GPU rows and Install. Tests: Piper+CUDA rows include the CUDA provider and NVIDIA rows and exclude weights; Piper+CPU is unchanged.
- [x] `crates/voice-me-tts-piper/` -- injectable providers, CPU by default for existing callers and tests.
- [x] `crates/voice-me-ui/src/backend.rs` (+ `dependencies.rs` test) -- the two selects and their UI tests: engine list, device list shown or hidden, device Confirm saves, engine switch keeps the device.
- [x] `crates/voice-me-app/src/main.rs` -- the wiring above and the Piper-GPU `is_gpu_selection` test.
- [x] `EXPERIENCE.md`, `README.md` -- the selector rows.

**Acceptance Criteria:**
- Given Settings → Backend on Local, when the user opens the backend `Select`, then it lists engines without device suffixes, and a device `Select` appears under Piper or Chatterbox.
- Given Piper on WebGPU with the all-provider runtime installed, when the user speaks, then Piper's session is built with the WebGPU provider.

## Implementation Notes

- `BackendSelection::Piper { target }` (+ `PIPER_CPU`). `local_target()` now returns Piper's target too, so `resolve_backend`, `is_gpu_selection`, `selection_library` and the deps capability/runtime rows treat Piper's device like Chatterbox's; `is_chatterbox()` marks the one engine with model files. Piper on CPU is `is_cpu()`, so the Backend tab shows the "CPU mode" tag beside "stock voice".
- Core: `LocalEngine` (Piper, Chatterbox, SystemVoice), `engine_choices()`, `device_choices(engine, runtimes, all_providers)`, `switch_engine(current, engine, runtimes, all_providers)`, `BackendSelection::{engine, device_label, on_cpu}`. `label()` (the *Selected* line) reads "<engine> · <device>". `backend_choices` takes `all_providers` and lists each engine's devices.
- Settings: `SelectionFile::Piper { #[serde(default)] target }`. Always written; an old build reads it as Piper (serde ignores the extra field of an internally tagged unit variant).
- Piper crate: `PiperTts::with_providers(ProvidersInit)` returning `SessionProviders { providers, failure_notes }`, CPU by default; called after `RuntimeInit`. `ort` gains `cuda`/`webgpu`.
- `voice_me_tts::session_providers(root, runtime, backend)` shares Chatterbox's restart rule and NVIDIA preload (`prepare_target`); the app injects it into Piper.
- UI: the backend `Select` value is `BackendEntry::{Engine, Remote}`; `backend-device-select` (value `BackendSelection`) is shown for Piper/Chatterbox under the Local kind. `BackendPanel.bundled_all_providers` comes from `Sources::pinned().runtime_all_providers`.
- "Use CPU backend" now selects the saved engine on CPU (Piper stays Piper).

## Spec Change Log

## Review Triage Log

Pass 1: blind hunter (B), edge-case hunter (E), verification gap (V).

| Verdict | Finding | Evidence / route |
|---|---|---|
| medium | `session_providers` test checks only `len()==1`; a target-blind implementation passes (V) | Pre-verified gap → patch: downcast per target |
| medium | "Use CPU backend keeps Piper" lives inline in `main()`, untested (V, B) | Pre-verified gap → patch: extract `use_cpu_selection`, test |
| low | Piper+WebGPU capability arm and rows untested (V, B) | Pre-verified gap → patch: add a WebGPU case |
| low | `backend_choices` public but unused outside core tests (B) | Direct deletion → patch: `#[cfg(test)]`, drop the re-export |
| low | `BackendEntry::of` panics with `unreachable!` on the render path (B) | Direct correction → patch: exhaustive match |
| low | Over-long comment line in `voice-me-tts-piper/Cargo.toml` (B) | Direct correction → patch |
| medium | A stale Chatterbox-GPU report can pass `for_current_selection` for Piper on the same device (E) | The guard compares `report.backend` only; the same race already existed for Chatterbox CPU → Piper CPU → defer (pre-existing) |
| medium | Piper never sends `SpeechSessionBuilt`, so the "acquired" line never shows its device (B) | Pre-existing since Story 3.15; now matters for GPU → defer |
| low | UseCpu falls back to `BUNDLED_CPU` when the settings load fails (B, E) | Real only when the settings file is unreadable, which is unlikely; the old behaviour → rejected |
| low | A saved GPU device missing from the device list leaves the `Select` on its placeholder (B, E) | Reachable only through a hand-edited or newer file while the all-provider runtime is pinned; the fix adds a branch → rejected |
| false | Piper on GPU builds on a CPU-only runtime with only a late error (E) | `runtime_row` for a GPU target reports the runtime missing, so speech is blocked before warm-up |
| false | CUDA/WebGPU listed based on the pinned runtime, not the installed one (B) | By design (3.8): the rows show missing and Install replaces the CPU runtime |
| low | `BackendSelection::on_cpu` vs `LocalEngine::on_cpu` differ for the System voice (B) | UseCpu only arises from a GPU capability row, never for the System voice; the doc states the behaviour → rejected |
| low | Downgrade compatibility (`kind="piper"` + `target`) untested (B) | serde's internally tagged unit variant ignores extra keys, and the load is lenient anyway → rejected |
| false | Bare "CUDA" beside "CUDA — libonnxruntime.so" is ambiguous (B) | Frozen decision 2 sets these labels |

## Verification

**Commands:**
- `cargo test -p voice-me-core`, `-p voice-me-deps`, `-p voice-me-tts-piper`, `-p voice-me-ui`, `-p voice-me-app` (one crate per run; disk is tight) -- expected: pass
- `cargo clippy -p <each changed crate> --all-targets` -- expected: no new warnings
