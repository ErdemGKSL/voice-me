---
title: 'Build and mirror the all-provider ONNX Runtime'
type: 'feature'
created: '2026-09-24'
status: 'in-progress'
baseline_commit: '698ae2924706fdfcad381a49f78ea1048e39b0f5'
route: 'dispatch'
review_loop_iteration: 0
context:
  - '{project-root}/_bmad-output/implementation-artifacts/epic-3-context.md'
  - '{project-root}/_bmad-output/implementation-artifacts/spec-3-3-be-honest-when-the-selected-backend-cant-run-here.md'
  - '{project-root}/_bmad-output/implementation-artifacts/spec-3-2-one-click-provision-a-missing-dependency.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** The bundled ONNX Runtime is Microsoft's CPU build, so the CUDA and WebGPU backends only work with a library the user adds by hand. Microsoft ships no WebGPU build at all. Switching backends needs a restart, and `voice-me-tts`'s `webgpu-probe` feature downloads a runtime at build time (an AD-8 violation).

**Approach:** Build ONNX Runtime 1.28.2 in CI from source, with `--use_cuda --use_webgpu --build_shared_lib`, for Linux x64 and Windows x64. Mirror it as a versioned GitHub Release on this repo. Provision it through `voice-me-deps` like any other asset. One library then carries CPU, WebGPU and CUDA, so switching between them is a session rebuild, not a restart. Remove `webgpu-probe`.

**Decisions (user, 2026-09-24):**
1. The CUDA backend's NVIDIA libraries (CUDA 12 runtime, cuBLAS/cuBLASLt, cuFFT, cuDNN 9) are downloaded by voice-me with one click. The user does not install them.
2. The build workflow is added to the repo and triggered from this session. When it succeeds, its assets' sizes and SHA-256 are pinned in `voice-me-deps`.

**Decisions (agent; the user delegated the rest):**
3. The NVIDIA libraries come from NVIDIA's own wheels on PyPI (`nvidia-cuda-runtime-cu12`, `nvidia-cublas-cu12`, `nvidia-cufft-cu12`, `nvidia-cudnn-cu12`). Each is pinned by URL, size and SHA-256. They are fetched with resume like every asset and only their shared libraries are extracted. They are not re-hosted: they are too large for a clean mirror, and PyPI is a stable, versioned source (AD-7: "assets too large to mirror use a pinned static URL").
4. The release is split in two per OS:
   - a **core** archive: `onnxruntime` plus `onnxruntime_providers_shared`, with WebGPU built in. It replaces Microsoft's CPU archive for everyone.
   - a **CUDA provider** archive: `onnxruntime_providers_cuda`. It is fetched only when a CUDA backend is selected, together with the NVIDIA wheels.

   CPU and WebGPU users never download the CUDA parts.
5. CUDA 12.8 with architectures `60;70;75;80;86;89;90;120` plus PTX. This keeps the existing compute-capability floor of 6.0.
6. The tag is `onnxruntime-1.28.2-voiceme.1`. The workflow runs on `workflow_dispatch` only, never on push, and it never touches the rolling `dev-main` release.
7. Until the release is pinned, the sources keep Microsoft's CPU archive, and the GPU rows keep today's manual text. The CPU backend works throughout.

## Boundaries & Constraints

**Always:**
- Every downloaded file is verified against its pinned size and SHA-256 before it takes its final name, and downloads resume (spec 3-2).
- The NVIDIA libraries are loaded from `<cache>/runtime/cuda/` before the CUDA provider is registered: dlopen with RTLD_GLOBAL on Linux, and the DLL directory added on Windows. The user's PATH and LD_LIBRARY_PATH are never changed.
- A runtime the user added under Local runtimes, or `ORT_DYLIB_PATH`, still wins over the bundled one (spec 3-3). Its restart rule is unchanged.

**Never:**
- No build-time or run-time download outside `voice-me-deps` (AD-8). `ort/download-binaries` disappears from every manifest.
- No other OS or architecture. No TensorRT, no DirectML.
- No change to how Piper uses the runtime, beyond picking up the new core library.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| CPU user | Fresh install, CPU backend | Install fetches only the core archive; CPU works | Download or verify failure → row error (existing) |
| WebGPU | WebGPU selected, core installed | Generation on WebGPU from the same library; no restart when switching from CPU | Provider unavailable → existing capability row |
| CUDA | CUDA selected, NVIDIA driver present | Install also fetches the CUDA provider archive and the 4 NVIDIA wheels, and extracts only their `.so`/`.dll`; CUDA runs without a restart | A missing piece is its own row; Install fetches the rest |
| Switch | CPU → CUDA → WebGPU → CPU in one run | Each switch rebuilds sessions only | — |
| Before pinning | Release not yet pinned | Sources as today (Microsoft CPU); GPU rows say a later release | — |
| Added runtime | User-added library | Behaves exactly as today | — |

</frozen-after-approval>

## Code Map

- `.github/workflows/onnxruntime.yml` (new) -- `workflow_dispatch`, a matrix of `ubuntu-22.04` (glibc floor, as in dev-release) and `windows-latest`.
  - Free runner disk first. Install the CUDA 12.8 toolkit and cuDNN 9 for the build.
  - `build.sh`/`build.bat --config Release --build_shared_lib --parallel --use_cuda --cuda_home … --cudnn_home … --use_webgpu --skip_tests --cmake_extra_defines CMAKE_CUDA_ARCHITECTURES=…` at tag `v1.28.2`.
  - Pack the two archives per OS, then `gh release create onnxruntime-1.28.2-voiceme.1` with assets and `SHA256SUMS.txt`. If the release exists, clobber-upload.
- `crates/voice-me-deps/src/sources.rs:26,176,271-323` -- `RUNTIME_VERSION`, `RuntimeArchive`, the per-OS archives.
  - Add a `library_entries` list: the core carries 2 libraries.
  - Add a `cuda_runtime: Option<RuntimeArchive>` and `nvidia_wheels: Vec<ArchiveLibs>` (each wheel with the member paths to extract).
  - Keep the Microsoft archive as the fallback until pinned (decision 7).
- `crates/voice-me-deps/src/provision.rs:391-421` -- `extract_runtime_library` extracts one entry; generalise it to a list. Wheels are zip files.
- `crates/voice-me-deps/src/lib.rs:385-420,749-861` -- `provision_runtime` / `gpu_runtime_unavailable` / `runtime_row`: a CUDA backend gets the CUDA provider plus NVIDIA rows, and Install fetches them. Retire the "later voice-me release" GPU text once pinned. Deferred item :187 (Piper Install refused on a GPU selection) is fixed in passing.
- `crates/voice-me-core/src/assets.rs:32-196` -- the cache layout: `runtime/` (core + providers), `runtime/cuda/` (NVIDIA libs). `resolve_runtime_dylib` stays the entry point.
- `crates/voice-me-tts/Cargo.toml:21-28,61-85` + `src/sessions.rs:98-114,202-285` -- delete `webgpu-probe` and the `not(dynamic-runtime)` `init_runtime`. Before registering CUDA, preload the NVIDIA libraries from `runtime/cuda/`. Switching targets on the committed bundled library is a session rebuild.
- `crates/voice-me-app/src/main.rs:541-600,3563-3600` -- `needs_restart`: switching among CPU, CUDA and WebGPU on the bundled library never needs a restart. Added libraries keep the rule.
- `crates/voice-me-deps/src/capability.rs:29-31` -- the CUDA floor comment: 6.0 kept (decision 5).

## Tasks & Acceptance

**Execution:**
- [ ] `.github/workflows/onnxruntime.yml` -- the build and release workflow (decisions 4–6).
- [ ] `crates/voice-me-deps/src/{sources,provision,lib}.rs` -- multi-library archives, the CUDA provider archive, the NVIDIA wheels (`None` until pinned), the rows and Install. Tests use injected sources and fake archives (a zip wheel with `nvidia/cudnn/lib/libcudnn.so.9`).
- [ ] `crates/voice-me-core/src/assets.rs` -- the `runtime/cuda` layout helpers and tests.
- [ ] `crates/voice-me-tts/{Cargo.toml,src/sessions.rs}` -- remove `webgpu-probe`, add the NVIDIA preload, switch without a restart.
- [ ] `crates/voice-me-app/src/main.rs` -- `needs_restart` for the bundled library, and tests.
- [ ] After the first successful workflow run: pin the URLs, sizes and SHA-256 of the 4 release assets (2 per OS) and the 4 NVIDIA wheels per OS in `sources.rs`. Switch the default runtime to the core archive.
- [ ] `README.md` + `ARCHITECTURE-SPINE.md` stack row -- the mirrored runtime and the NVIDIA wheels.

**Acceptance Criteria:**
- Given the workflow is dispatched, then it publishes `onnxruntime-1.28.2-voiceme.1` with 4 archives and `SHA256SUMS.txt`.
- Given a Linux or Windows machine with an NVIDIA driver, when the user selects CUDA and presses Install, then everything CUDA needs is fetched in one click, and generation runs on CUDA without a restart.
- Given `cargo tree`, then no crate enables `ort/download-binaries`.

## Implementation Notes

## Spec Change Log

## Review Triage Log

## Verification

**Commands:**
- `cargo test -p voice-me-deps`, `-p voice-me-core`, `-p voice-me-tts`, `-p voice-me-app` (one per run) -- expected: pass
- `grep -r download-binaries crates/` -- expected: none

**Manual checks:**
- GitHub Actions: the workflow run is green and the release has its assets.
- On a CUDA machine: select CUDA, Install, speak.
