---
title: 'Build and mirror the all-provider ONNX Runtime'
type: 'feature'
created: '2026-09-24'
status: 'in-review'
baseline_commit: '698ae2924706fdfcad381a49f78ea1048e39b0f5'
route: 'dispatch'
review_loop_iteration: 1
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
- [x] `crates/voice-me-deps/src/{sources,provision,lib}.rs` -- multi-library archives, the CUDA provider archive, the NVIDIA wheels (`None` until pinned), the rows and Install. Tests use injected sources and fake archives (a zip wheel with `nvidia/cudnn/lib/libcudnn.so.9`).
- [x] `crates/voice-me-core/src/assets.rs` -- the `runtime/cuda` layout helpers and tests.
- [x] `crates/voice-me-tts/{Cargo.toml,src/sessions.rs}` -- remove `webgpu-probe`, add the NVIDIA preload, switch without a restart.
- [x] `crates/voice-me-app/src/main.rs` -- `needs_restart` for the bundled library, and tests.
- [ ] After the first successful workflow run: pin the URLs, sizes and SHA-256 of the 4 release assets (2 per OS) and the 4 NVIDIA wheels per OS in `sources.rs`. Switch the default runtime to the core archive.
- [x] `README.md` + `ARCHITECTURE-SPINE.md` stack row -- the mirrored runtime and the NVIDIA wheels.

**Acceptance Criteria:**
- Given the workflow is dispatched, then it publishes `onnxruntime-1.28.2-voiceme.1` with 4 archives and `SHA256SUMS.txt`.
- Given a Linux or Windows machine with an NVIDIA driver, when the user selects CUDA and presses Install, then everything CUDA needs is fetched in one click, and generation runs on CUDA without a restart.
- Given `cargo tree`, then no crate enables `ort/download-binaries`.

## Implementation Notes

- **Pinning is data only.** `sources.rs` holds `VOICEME_{LINUX,WINDOWS}_{CORE,CUDA}` (`ReleaseArchive`: dir, extension, `pin: Option<Pin>`, libraries under `<dir>/lib/`) and `NVIDIA_WHEELS_{LINUX,WINDOWS}` (`NvidiaWheel`: package, version, `pin: Option<WheelPin>`, member paths). Filling the `pin` fields switches `Sources::pinned()` to the core archive (`runtime_all_providers = true`) and enables CUDA (`cuda_installable()`, which needs the core, the CUDA archive and all 4 wheels pinned). If the workflow's `*-contents.txt` lists more libraries in the core (Dawn, `dxil.dll`, `dxcompiler.dll`), add them to its `libraries`.
- **Wheel members were read from the published wheels** (central directory over HTTP ranges): cudart 12.8.90, cuBLAS 12.8.4.1 (`libcublasLt` + `libcublas`, not `libnvblas`), cuFFT 11.3.3.83 (not `cufftw`), cuDNN 9.8.0.87 (all 8 libraries).
- **Stale runtime.** Install writes `runtime/installed-from.txt` with the core archive's release URL (which names the tag) and pinned SHA-256, so a re-pin under the same file names is detected. Once pinned, a runtime without it (Microsoft's CPU build) or with another stamp stays ready for CPU/Piper, but is a missing, installable runtime row ("an older or CPU-only build") for WebGPU/CUDA; its CUDA provider row reads missing too. A current runtime missing any library its archive ships is missing for every target. The old CUDA provider is removed only after the new core extracted.
- **NVIDIA records.** `runtime/cuda/installed-from.txt` holds one line per installed wheel (stamp + library names). A line not matching the current pins makes the NVIDIA row missing, and Install clears the folder before fetching. The engine preloads only the libraries that record names.
- **Archives on disk** are re-verified against the pin (`is_verified`) before a download is skipped; a mismatch is deleted and fetched again.
- **Extraction** moves each existing library to `<name>.old`, renames the new ones in, and restores every backup on any failure; backups are deleted only on success.
- **Windows staging.** When the composition root reports the bundled runtime loaded in this process (`DepsAdapter::with_runtime_in_use`), Windows extracts the new core and provider into `runtime/staged/` (record written last). `voice_me_deps::apply_staged_runtime` moves a complete staged set into place first thing in `main`; the GPU runtime and CUDA provider rows say a restart finishes the install. Linux replaces in place.
- **One Install, every piece.** Install on the runtime, CUDA provider or NVIDIA row fetches every missing piece in one plan with one progress figure on the clicked row. A runtime-install mutex makes a second click wait and then find nothing left to fetch. Extraction is all-or-nothing per archive (`.part` files renamed only once every library was found).
- **Restart rule.** Switching CPU/CUDA/WebGPU on the bundled library is a session rebuild. The one exception is `voice_me_tts::replaced_runtime_needs_restart` (bundled && GPU && replaced on disk since loaded), or a staged update with a GPU selection: `needs_restart` then says restart, a GPU session build on it fails naming the restart, and a finished runtime Install sets "Restart now" at once.
- **NVIDIA preload** (`sessions::preload_cuda_libraries`) runs only for CUDA on the bundled runtime; it loads the libraries the NVIDIA record names from `runtime/cuda/` (RTLD_NOW|RTLD_GLOBAL on Linux; on Windows `LoadLibraryExW` by full path with `LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS`, the process search path untouched), retrying until a pass loads nothing more, and appends any that failed to the provider's error.
- `voice-me-tts` loses both the `webgpu-probe` and `dynamic-runtime` features; `load-dynamic` is on the `ort` dependency itself. `ureq` left `Cargo.lock`.
- Deferred item (Piper Install refused on a GPU selection) fixed: a Piper selection provisions the CPU runtime.

## Spec Change Log

## Review Triage Log

Pass 1: blind hunter (B), edge-case hunter (E), verification gap (V). The code is behaviour-neutral until pinning (decision 7), but every finding below applies the moment the release is pinned.

| Verdict | Finding | Evidence / route |
|---|---|---|
| high | Windows cannot replace or delete a loaded `onnxruntime.dll` or CUDA provider, so the CPU→GPU upgrade fails there (B, E, V) | `rename` over a mapped DLL is refused → patch, staged set in `runtime/staged/` moved into place at the next start |
| medium | `extract_libraries` is not all-or-nothing in its rename phase (B, E, V) | No rollback of earlier renames → patch, backups plus restore, and a test |
| medium | The CUDA provider is deleted before the core extraction succeeds (B) | Order in `provision_runtime` → patch |
| medium | Core completeness checks only the main library (B, E) | `bundled_runtime_dylib().exists()` → patch, all `library_entries` |
| medium | The CUDA provider row reads ready beside a stale core (B, E, V) | → patch |
| medium | The stale check compares the file name only, so a re-pinned tag is never detected (B, E) | → patch, marker with SHA-256 and tag |
| medium | An existing archive is not re-verified after a pin change (E) | → patch |
| medium | NVIDIA sonames stay the same across versions: no re-download, and stale libraries get preloaded (B) | → patch, a `runtime/cuda` marker and preload of the pinned list only |
| medium | No restart prompt after an Install that replaces the loaded runtime; the app's and the engine's "replaced" rules differ (B, E) | → patch, one rule plus a prompt on ProvisioningFinished |
| medium | "Replaced" detection has only a vacuous test; the preload decision and the error merge are untested (B, V) | → patch, a serialized test and pure helpers |
| low | `SetDllDirectoryW` changes the process-wide DLL search path (B, E) | → patch, `LoadLibraryExW` with a per-load search flag |
| low | CPU Install with no source errors even when nothing is missing; dead branch (B, E) | → patch |
| low | The stale message wrongly says an older voice-me build has no provider (B) | → patch, reworded |
| maybe-false | The CUDA provider or cuDNN may also need cuRAND or NVRTC (E) | Settled at pinning from the build's `linux-x64-ldd.txt` → pin step |
| maybe-false | Dawn and DXC files are missing from the core tables (B) | Settled at pinning from `*-contents.txt` → pin step |
| low | Download-all-then-extract doubles peak disk use (B) | Rare disk exhaustion; restructuring the plan adds complexity → rejected |
| low | Size+mtime identity is weak on coarse-timestamp filesystems (B) | Mostly superseded by the SHA marker; rare → rejected |
| low | No test for the `runtime_install` mutex (B) | → rejected |

## Verification

**Commands:**
- `cargo test -p voice-me-deps`, `-p voice-me-core`, `-p voice-me-tts`, `-p voice-me-app` (one per run) -- expected: pass
- `grep -r download-binaries crates/` -- expected: none

**Manual checks:**
- GitHub Actions: the workflow run is green and the release has its assets.
- On a CUDA machine: select CUDA, Install, speak.
