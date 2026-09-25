---
title: 'Choose the engine first, then the device it runs on'
type: 'feature'
created: '2026-09-25'
status: 'draft'
route: 'dispatch'
review_loop_iteration: 0
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
- [ ] `crates/voice-me-core/src/{state,settings_store}.rs` -- the Piper target, engine and device choices, persistence and compatibility. Tests for the matrix rows Engine list, Device list, Old file and Engine switch.
- [ ] `crates/voice-me-deps/src/{capability,lib}.rs` -- Piper GPU rows and Install. Tests: Piper+CUDA rows include the CUDA provider and NVIDIA rows and exclude weights; Piper+CPU is unchanged.
- [ ] `crates/voice-me-tts-piper/` -- injectable providers, CPU by default for existing callers and tests.
- [ ] `crates/voice-me-ui/src/backend.rs` (+ `dependencies.rs` test) -- the two selects and their UI tests: engine list, device list shown or hidden, device Confirm saves, engine switch keeps the device.
- [ ] `crates/voice-me-app/src/main.rs` -- the wiring above and the Piper-GPU `is_gpu_selection` test.
- [ ] `EXPERIENCE.md`, `README.md` -- the selector rows.

**Acceptance Criteria:**
- Given Settings → Backend on Local, when the user opens the backend `Select`, then it lists engines without device suffixes, and a device `Select` appears under Piper or Chatterbox.
- Given Piper on WebGPU with the all-provider runtime installed, when the user speaks, then Piper's session is built with the WebGPU provider.

## Implementation Notes

## Spec Change Log

## Review Triage Log

## Verification

**Commands:**
- `cargo test -p voice-me-core`, `-p voice-me-deps`, `-p voice-me-tts-piper`, `-p voice-me-ui`, `-p voice-me-app` (one crate per run; disk is tight) -- expected: pass
- `cargo clippy -p <each changed crate> --all-targets` -- expected: no new warnings
