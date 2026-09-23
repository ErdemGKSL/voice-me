---
title: 'Install the ONNX Runtime on Windows'
type: 'feature'
created: '2026-09-23'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
context: []
baseline_commit: '3d5dd35ddd660e123fc538a9333a0c227a98935d'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** On Windows, Settings → Dependencies shows the ONNX Runtime row as manual ("Download ONNX Runtime 1.28.2 … copy its onnxruntime.dll into …"), because Story 3.2 Decision 1 pinned an automatic runtime source for Linux x64 only.

**Approach:** Give Windows x64 the same one-click Install: pin Microsoft's `onnxruntime-win-x64-1.28.2.zip` in the source table, and teach runtime extraction to read a `.zip` as well as a `.tgz`. Install then downloads, verifies, extracts only `onnxruntime.dll` into the cache, and re-runs the check, exactly like Linux.

## Boundaries & Constraints

**Always:** Same guarantees as Story 3.2: versioned immutable URL, pinned size + SHA-256, `.part` then verify then atomic rename, progress on the `AppEvent` channel, resume via `Range`, and the archive is deleted after a successful extraction. Only the one library entry is extracted (never headers, `.lib`, `.pdb` or `onnxruntime_providers_shared.dll`). The archive format is chosen by the archive's file extension, so both extractors are testable on every OS. Never replace a runtime the user configured through `ORT_DYLIB_PATH`.

**Never:** No Windows arm64 or x86 source (x64 only, the machine class this repo targets). No GPU/DirectML runtime (Story 3.8). No GitHub Release mirror. No network code outside `voice-me-deps` (AD-8). No UI change: the Install button, progress and re-check already exist for automatable rows.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Fresh install (Windows x64) | CPU backend, `ORT_DYLIB_PATH` unset, no runtime in cache | Row is missing with Install; Install fetches the zip (78.6 MB progress), verifies, extracts `<cache>/runtime/onnxruntime.dll`, deletes the zip, row turns ready | N/A |
| Zip extraction | archive with the library plus other entries | Only the named entry is written, via `.part` + rename | Missing entry → "Extraction of onnxruntime.dll from <zip> failed: the archive has no <entry>" |
| Tgz still works | Linux fixture archive | Unchanged behavior | Unchanged |
| Corrupt / interrupted download | hash mismatch or network drop | Same as 3.2 (`.part` removed or kept, row message names the file) | Same as 3.2 |
| Configured path missing | `ORT_DYLIB_PATH` set to a missing file | Row stays manual, no Install | N/A |
| GPU backend selected | CUDA/WebGPU | Row stays manual ("ships with a later release") | N/A |

</frozen-after-approval>

## Code Map

- `crates/voice-me-deps/src/sources.rs` -- `pinned_runtime()` (L226-251): the Linux x64 entry, then a `None` for every other target. Add a `cfg(all(target_os = "windows", target_arch = "x86_64"))` entry. Relative path `runtime/onnxruntime-win-x64-1.28.2.zip`, URL `https://github.com/microsoft/onnxruntime/releases/download/v1.28.2/onnxruntime-win-x64-1.28.2.zip`, size `78_620_837`, sha256 `c4eedd29489d5feca21866d054638416f3655bf6b18851b3b6b85c8313e95c35` (GitHub's release digest; the same API returns the Linux pin already in code). Library entry `onnxruntime-win-x64-1.28.2/lib/onnxruntime.dll`. Narrow the `None` cfg to match. Update the doc comments at L24 and L161-162 that say "Linux x64 only".
- `crates/voice-me-deps/src/provision.rs` -- `extract_runtime_library` (L338-400) reads tar.gz only. Split out the part that writes the entry (the `.part` write, the unix-only chmod, the rename). Dispatch on the archive name: `.zip` goes to a zip reader that takes the entry by exact name and requires a file, not a directory; anything else goes to the existing tar path. Keep the error texts.
- `crates/voice-me-deps/Cargo.toml` -- add `zip` (current release, `default-features = false`, features `["deflate"]`) next to `flate2`/`tar`, with a comment in the file's style. It is an unconditional dependency, so the zip tests run on Linux CI too.
- `crates/voice-me-deps/src/lib.rs` -- `provision_runtime` (L154-201) and the manual `runtime_row` branch (L448-462) need no logic change: `installable` already follows `sources.runtime.is_some()`. Update only the "Linux x64 only" comments at L154 and L451.
- `crates/voice-me-deps/src/provision_tests.rs` -- `fake_runtime_archive` (L607) plus `a_runtime_install_extracts_only_the_real_library_into_the_cache` (L643) show the fixture pattern (`Fixture::with_extra_files`, pinned fake hashes). Add a zip twin: a fake `.zip` holding the library plus a decoy (`onnxruntime_providers_shared.dll`, a header), installed through `fixture.provision(DependencyKind::OnnxRuntime)`. Add a zip missing-entry test as well.
- `README.md` L84-86 -- the runtime bullet names Linux x64 only; add the Windows x64 zip.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-deps/Cargo.toml` -- add `zip` -- zip extraction
- [x] `crates/voice-me-deps/src/provision.rs` -- format dispatch and the shared entry writer -- extract from `.zip` and `.tgz`
- [x] `crates/voice-me-deps/src/sources.rs` -- Windows x64 pin and comments -- the row becomes installable on Windows
- [x] `crates/voice-me-deps/src/lib.rs`, `README.md` -- comment and doc updates -- no stale "Linux only" claims
- [x] `crates/voice-me-deps/src/provision_tests.rs` (and `sources.rs` tests) -- zip install, zip missing entry, and a Windows-only assertion that `Sources::pinned().runtime` is `Some` with a `.zip` archive and an `onnxruntime.dll` entry -- cover the matrix

**Acceptance Criteria:**
- Given Windows x64 with no runtime in the cache, when Settings → Dependencies opens with the CPU backend, then the ONNX Runtime row shows Install instead of manual steps
- Given that row, when Install is pressed in the real app, then the zip downloads with progress, `<cache>/runtime/onnxruntime.dll` exists afterwards (hash-verified archive, archive deleted), and the row turns ready without a restart

## Implementation Notes

- `zip` is pinned at `8.6` (the latest stable; crates.io's newest is `9.0.0-pre3`, a pre-release), `default-features = false, features = ["deflate"]`.
- The Windows pin's size and SHA-256 were re-checked against the GitHub releases API for `v1.28.2` and match. The archive was not downloaded, so the entry path `onnxruntime-win-x64-1.28.2/lib/onnxruntime.dll` is still unconfirmed; the manual check covers it.
- The zip reader treats a directory or symlink entry with the library's name as absent, so the error is the same "the archive has no <entry>" as the tar path. An extra test covers the directory case.
- Manual check, run in step 3 with the user's approval: a throwaway example (since deleted) called the real `DepsAdapter::provision(OnnxRuntime, CPU, …)` with the pinned sources, into a temporary `VOICE_ME_MODEL_CACHE`, with `ORT_DYLIB_PATH` unset. What it showed:
  - The 78,620,837-byte zip downloaded with 14 progress events and passed the pinned SHA-256 check.
  - The entry path `onnxruntime-win-x64-1.28.2/lib/onnxruntime.dll` is correct, and `runtime/` held only `onnxruntime.dll` afterwards (zip deleted).
  - `ProvisioningFinished { result: Ok(()) }` was sent.
  - The DLL loads, and `OrtGetApiBase` resolves to a non-null pointer.
  - The GUI click itself was not run; this is the same code path, minus the button.
- Matrix audit: zip install, zip missing entry and the zip directory entry have new tests. Tgz, corrupt/interrupted download, a configured path and a GPU backend are covered by the existing 3.2 tests, which pass unchanged. Fresh install on Windows x64 is covered by `windows_x64_pins_the_zip_runtime_and_its_dll` plus the real run above.

## Spec Change Log

## Review Triage Log

Pass 1 (blind-hunter, edge-case-hunter, verification-gap):

- **The zip symlink guard is untested** (verification-gap, blind). `medium`, pre-verified: weakening it to `is_dir()` passes every test. Route: patch (a symlink fixture test). The explicit `is_symlink()` is redundant with zip 8.6's `is_file()`, but it is harmless and states intent, so it is kept.
- **A failed rename leaves `<name>.part` behind** (blind, edge). `low`. Verified in `write_entry`: only the copy failure removes it. On Windows it is reachable when antivirus holds the fresh DLL. The fix is a direct one-liner. Route: patch.
- **The `lib.rs` Decision 1 doc line is about 95 chars** (blind). `low`, cosmetic. The fix is a direct re-wrap. Route: patch.
- **The runtime row checks only that the file exists, not that it loads; the Windows build needs the MSVC runtime (VC++ Redistributable)** (blind, edge, edge claim). `medium`. Pre-existing: a manually copied DLL, or a Linux library with a missing dependency, reaches the same existence-only check. This change only makes the Windows path easier to reach. This PC has the redistributable (winget installed it with uv), so the real run loaded fine. Route: defer.
- **The Windows pin compiles and tests only on Windows** (blind). `low`. The `build-windows` CI job compiles and runs it, and restructuring the table is more than a direct correction. Rejected.
- **The Windows pin test checks only the shape, not size or self-consistency, and nothing checks the real archive** (blind, verification-gap). `low`. The real archive was downloaded and verified this session (size, SHA-256, entry path, DLL load). An automated check would need 78 MB of network in an offline suite, and the Linux pin has none either. Rejected.
- **Unknown archive extensions fall back to the tar reader** (blind). `low`. The only sources are `.tgz` and `.zip`, so no reachable input triggers it. Rejected.
- **No corrupt/truncated zip test; no size cap on the extracted entry** (blind). `false`. The archive is SHA-256-verified against the pin before extraction starts, so a corrupt or oversized archive never reaches `extract_zip_entry`.
- **A directory or symlink entry is reported as "the archive has no …"** (edge). `low`. This matches the tar path's existing rule and the spec's recorded choice. Rejected.
- **Windows arm64 still sees manual steps with no hint** (blind). `low`. It is out of scope by intent (x64 only). Rejected.
- **The spec and epic-context changes are missing from the reviewed diff** (blind). `false`. They were deliberately excluded: the review layers see code only, and the claims file goes to the edge-case layer alone.

## Verification

**Commands:**
- `cargo test -p voice-me-deps` -- expected: all pass, including the new zip tests
- `cargo build --workspace` and `cargo test --workspace --no-fail-fast` -- expected: success on Windows

**Manual checks (if no CLI):**
- Run `voice-me.exe` with an empty `<cache>/runtime` and `ORT_DYLIB_PATH` unset, open Settings → Dependencies, and press Install on the ONNX Runtime row. Confirm the progress, then `onnxruntime.dll` in the cache, then the row turns ready. This really downloads 78.6 MB from github.com/microsoft/onnxruntime; it is the feature itself. If the entry path differs, the row names the missing entry. Then fix the pin from the real archive listing.
