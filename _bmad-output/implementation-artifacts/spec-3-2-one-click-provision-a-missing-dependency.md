---
title: 'One-click provisioning of a missing dependency (Story 3.2)'
type: 'feature'
created: '2026-09-23'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
baseline_commit: '8fbb8bd02079f607a0265ca7b782834fb7bbd477'
context: ['{project-root}/_bmad-output/implementation-artifacts/epic-3-context.md']
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Settings → Dependencies (Story 3.1) can say that the ONNX Runtime or the speech model files are missing, but the only fix is the hand-run `curl`/`tar` recipe in `README.md`. That breaks the epic's promise that nobody has to open a terminal, and it gives no progress for a 1.5 GB download.

**Approach:** Give `voice-me-deps` a provisioning half. An Install action on a missing, automatable Dependency row downloads exactly the selected backend's assets into the deps-owned cache. The download reports progress, can resume, and is checked against pinned SHA-256s. When it finishes, the Dependency Check runs again, so the row turns "ready" and the overlay unblocks without a restart.

## Boundaries & Constraints

**Always:** The assets fetched come from the selected `SpeechBackend`, through the same `voice-me-core::assets` list the check and the engine already use. Every source is a versioned, immutable URL. Each file is verified against a SHA-256 pinned in code before it takes its final name, and it only takes that name through an atomic rename from a `.part` file, so a half-written file is never reported as "ready". Progress, completion and failure travel on the `AppEvent` channel (AD-3). Downloads run on Tokio through the existing AD-5 bridge and never on the UI thread. Error messages name the file and the reason, for example "Download of speech_encoder.onnx_data failed: connection reset (412 MB of 591 MB kept, Install resumes from there)". Status is always given in words: "installing", plus a figure like "412 MB of 1.56 GB".

**Never:** No network code in any crate except `voice-me-deps` (AD-8), and no Hugging Face Hub client crate: plain HTTPS GETs only. No backend selector (3.5), remote provider (3.6), GPU runtime build or mirror (3.8), or device selection (3.9). No extra weight variants: Q4 stays the CPU set, and q4f16 is not added. No change to `speak.rs`'s precondition ladder. No blocking setup wizard. Never delete a file the user configured through `ORT_DYLIB_PATH`.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Fresh CPU install | CPU backend, empty cache; Install on the model row | Fetches only the 9 Q4-set files, with the row showing "installing" and bytes done/total. When done, the check re-runs, the row turns "ready" and the overlay accepts input | N/A |
| Partial set | 7 of 9 files present and verified | Fetches only the 2 missing files; total bytes counts only those | N/A |
| Resume | A `.part` of 300 MB exists from an interrupted run | Request is sent with `Range: bytes=<len>-`; a 206 response appends; progress starts at 300 MB | A 200 response (server ignored the range) restarts that file from zero |
| Corrupt download | Finished bytes hash differently from the pinned SHA-256 | The `.part` file is deleted and the final file is never created | Row shows "speech_encoder.onnx_data did not match its expected checksum; Install will fetch it again" |
| Network drop | Connection fails midway | The `.part` file is kept and the row returns to "missing" | Message names the file, the reason, and the bytes kept |
| Disk full / unwritable | Write fails | The `.part` file is kept | Message names the directory and the OS reason |
| Runtime install (Linux x64) | Runtime row missing, `ORT_DYLIB_PATH` unset | The pinned ONNX Runtime archive is fetched and verified; only the real shared library is extracted to `<cache>/runtime/libonnxruntime.so` (atomic rename) | Extraction failure is named |
| Runtime path configured | `ORT_DYLIB_PATH` points at a path that does not exist | Row stays manual (as in 3.1): no Install button, and inline steps are shown | N/A |
| Non-automatable row | `automatable == false` | Instead of Install, a "Show steps" toggle expands 2–4 short steps inline. No external link | N/A |
| Virtual mic missing (Linux) | Sink not loaded, audio server reachable | Install runs `LinuxVirtualMicAdapter::install()` (idempotent, no network), then re-checks | Audio-server error shown on the row |
| Double click / concurrent | Install is pressed while that row is already installing | Ignored, because Install is disabled while the row is installing | N/A |

## Decisions

1. **Runtime scope:** the ONNX Runtime can be installed automatically on Linux x64 only. On other targets the runtime row is manual, with short steps. Model weights and their SHA-256 pins are the same on every OS.
2. **"Manual-steps link"** is an inline expandable list inside the row. It never links to external docs (epic: no external docs).
3. **A GPU runtime has no source until Story 3.8.** The asset plan is keyed by `SpeechBackend`, so FP16 weights are fetched when FP16 is selected. A GPU backend's runtime row is manual, saying that its runtime is not yet available.
4. **Sources today are pinned upstream origins (answered 2026-09-23).** No GitHub Release mirror exists yet. Weights come from `huggingface.co/onnx-community/chatterbox-multilingual-ONNX/resolve/452d3f434aa592098f1eedac9099f33642ab2da5/...`, and the Linux runtime from Microsoft's `onnxruntime-linux-x64-1.28.2.tgz` GitHub release. Both sit in one source table in `voice-me-deps` holding URL, size and SHA-256 per asset, so a future mirror (Story 3.8) only changes URLs. This story creates no releases and uploads nothing.
5. **Scope kept whole (answered 2026-09-23)** even though the spec is over the token target.

</frozen-after-approval>

## Code Map

- `crates/voice-me-core/src/assets.rs` -- the single asset vocabulary (`model_cache_root`, `required_model_files`, `resolve_runtime_dylib`, `bundled_runtime_dylib`). The provisioning plan is built on top of these functions. Do not fork the list.
- `crates/voice-me-core/src/ports.rs:164-181` -- `DependencyProvisioningPort` (`Send + Sync`, `check(backend, events)`). Add `provision(kind, backend, events)` alongside it. The port stays sync: the app dispatches it through `tokio_bridge`, and the adapter drives its async I/O on the Tokio `Handle` it is given (or via `Handle::current()` inside the blocking task).
- `crates/voice-me-core/src/event.rs:9-35` -- `AppEvent` (`#[non_exhaustive]`, `Clone + PartialEq + Eq`). Add `ProvisioningProgress { kind, done_bytes, total_bytes }` and `ProvisioningFinished { kind, result: Result<(), String> }`. The failure crossing on the channel also resolves the 3.1 deferred item "failures raised outside the event channel diverge".
- `crates/voice-me-core/src/state.rs:83-167` -- `DependencyStatus` (`Ready | Missing`, `label()`), `DependencyKind`, `Dependency` (`automatable`, `.manual()`). Add an `Installing { done, total }` status, or keep install progress as UI-side state. Pick one so that the overlay gate (`overlay_blocker`) still treats "installing" as blocked. Rows may carry a `manual_steps: Vec<String>`.
- `crates/voice-me-core/src/tokio_bridge.rs` -- the AD-5 bridge (`TokioRuntime::install`, `spawn_blocking`). Reuse it; do not build a second runtime.
- `crates/voice-me-deps/src/lib.rs` -- `DepsAdapter`, `runtime_row`, `model_weights_row`, `virtual_microphone_row`, and the `EnvGuard` test helper (`:230+`). Put the new provisioning code in its own modules (for example `provision.rs`, `sources.rs`), keeping `lib.rs` as the port impl. Every manual row gets steps.
- `crates/voice-me-deps/Cargo.toml` -- add `reqwest` (rustls, stream, no default features), `tokio`, `sha2`, `flate2` + `tar` (Linux runtime archive), and `futures`, which is already a dev-dependency.
- `crates/voice-me-audio-linux/src/lib.rs:96` -- `LinuxVirtualMicAdapter::install()` is idempotent, so the Virtual Microphone's Install action reuses it.
- `crates/voice-me-ui/src/dependencies.rs` -- `DependenciesView` (`row()` `:89`, manual marker `:131`, `check_again` `:75`, test stub `CountingDepsPort` `:278`). Add an Install button on missing, automatable rows, a gpui-kit progress bar with the "installing" label and byte figure, and the "Show steps" toggle.
- `crates/voice-me-app/src/main.rs:770-900,1154-1180` -- holds `deps_port`, `dependency_outcome`, `settings_view_slot`, and the `AppEvent` loop. Add handlers for the new events: forward progress to the view, and on `ProvisioningFinished` re-run the check through the existing background path (`:1086`).
- `README.md:79-120` -- the manual `curl` recipe. Replace it with "use Settings → Dependencies → Install", keeping the pinned revision as a reference.
- Hugging Face tree at revision `452d3f434aa592098f1eedac9099f33642ab2da5`: every graph does have an `.onnx_data` sibling, so the 3.1 deferred doubt is settled. The LFS `oid` in `https://huggingface.co/api/models/onnx-community/chatterbox-multilingual-ONNX/tree/<rev>/onnx` is the SHA-256 to pin. Take `tokenizer.json`'s hash from the root tree the same way.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-core/src/{event,ports,state}.rs` -- add the provisioning events, the `provision` port method, and the install status/steps field -- this is the contract the adapter, app and UI share.
- [x] `crates/voice-me-deps/src/sources.rs` -- a pinned source table: per asset, its URL, size and SHA-256, plus the plan function `(backend, cache root) → assets still missing` -- this puts backend relativity and pinning in one place.
- [x] `crates/voice-me-deps/src/provision.rs` -- a resumable ranged download to `.part`, then SHA-256 verification, then an atomic rename. Progress events are throttled to at most about 10 per second per row. The runtime archive is extracted to `bundled_runtime_dylib`. Errors map into `VoiceMeError` with file-specific text.
- [x] `crates/voice-me-deps/src/lib.rs` -- implement `provision` for each kind (weights, runtime, virtual mic), and attach manual steps to non-automatable rows.
- [x] `crates/voice-me-ui/src/dependencies.rs` -- the Install button, the installing state with a progress bar and bytes, the "Show steps" toggle, and failure text on the row.
- [x] `crates/voice-me-app/src/main.rs` -- dispatch `provision` through `tokio_bridge`, route the progress and finished events to the view, and re-run the check after finishing.
- [x] `crates/voice-me-deps` tests -- drive the matrix against a local in-process HTTP server (for example a `tokio` `TcpListener` serving fixed bytes that honours `Range`) with small fake assets and a test-injected source table. Cover: fresh, partial, resume (206), range ignored (200), checksum mismatch, a connection dropped midway, and backend relativity (a Q4 plan never contains FP16 files).
- [x] `crates/voice-me-ui/src/dependencies.rs` tests -- clicking Install calls the port once, and Install is absent on manual rows, which show steps instead.
- [x] `README.md` -- replace the curl recipe.

**Acceptance Criteria:**
- Given a missing automatable row, when Install is clicked, then that row alone shows "installing" with a figure that grows, and when it finishes the row reads "ready" and the next hotkey press opens a typable overlay, with no restart.
- Given the app is quit mid-download, when Install is clicked in a later launch, then the download continues from the bytes already on disk.
- Given the CPU backend, when provisioning completes, then no FP16 file and no GPU provider library exist in the cache.

## Implementation Notes

- **Install progress lives beside the report, not in it.** `DependencyStatus` stays `Ready | Missing`. The composition root holds a `HashMap<DependencyKind, RowProvisioning>`, so a row being installed still reads "missing" to the overlay gate until the re-run check has seen the files.
- **`provision` is dispatched from `DependenciesView::install`** through `tokio_bridge::spawn_blocking_on`, following the precedent 3.1's `check_again` set. The root learns about each install only from `AppEvent`s.
- **Adapter guard against duplicate Installs:** `DepsAdapter` keeps an in-flight set so a second Install on the same row returns at once, sends nothing, and never starts a second writer on one `.part` file.
- **`block_on`** reuses the AD-5 multi-thread runtime when the call is running on its blocking pool, and otherwise builds a current-thread runtime for that one call (as in tests).
- **Deferred doubt settled:** every graph at the pinned revision has an `.onnx_data` sibling (Hugging Face tree API, 2026-09-23).
- **Review of the edge-case table (verification pass):** the "Virtual mic missing" row had no covering test. The installer is now injectable (`DepsAdapter::with_virtual_mic_installer`), and `installing_the_virtual_microphone_runs_the_installer_and_reports_its_end` covers both success and failure.
- **Download client:** `reqwest` without the `stream` feature (`Response::chunk` needs none), rustls TLS.

## Spec Change Log

## Review Triage Log

Pass 1 (blind-hunter, edge-case-hunter, verification-gap).

| Verdict | Finding | Evidence |
|---|---|---|
| medium | A panicked or never-started provisioning job leaves the root map on a stale `Installing`; every later check re-imposes it and Install stays disabled until restart | Confirmed: `DependenciesView::install`'s error arm sets `Failed` only in the view, and `replace_provisioning` on the next report overwrites it with the root's entry. Grouped with the next three rows under one cause: install state has two owners. → patch (the view reports click and non-report through the existing events) |
| low | Clicking Install and then a check landing before the first progress event wipes the row's installing state and re-enables Install | Confirmed: the view records `Installing{0,0}` locally only. The window is milliseconds long, because `ProgressReporter::for_plan` sends at once. Same group → patch |
| low | After an `Ok` finish, a failed re-run check leaves the view on "installing" | Confirmed: the `Err` branch pushes only `set_dependency_outcome`. It is barely reachable (a check fails only on an unresolvable cache root), but the fix is one line. → patch |
| low | A Settings window opened between an `Ok` finish and the re-run check shows Install enabled; an older check landing in that gap flashes the row | Confirmed but a sub-second window. The fix would need a new "finishing" state. Rejected |
| medium | The provisioning-event handling in `main.rs` has no test (verification-gap, pre-verified) | Deleting the re-check, `replace_provisioning`, or inverting the `retain` all pass the suite. → patch (extract pure functions and test them) |
| medium | Recovery branches (416, wrong `Content-Range`, oversize `.part`, complete `.part`, pre-placed archive) never run (verification-gap, pre-verified) | The test server has no behaviour that reaches them. → patch |
| medium | The production multi-thread `block_on` path is never exercised (verification-gap, pre-verified) | Every deps test runs without a runtime; the one UI test uses a counting stub. → patch |
| low | No test builds the view with a non-empty provisioning map (verification-gap, pre-verified) | Dropping `.with_provisioning(..)` passes every test. → patch |
| medium | A body longer than `asset.size` is written without bound and pushes progress past 100% | Confirmed: the chunk loop has no bound, and the checksum runs only at the end. → patch |
| low | `write_error` says "0 B of X kept, Install resumes from there" when nothing was kept | Confirmed: `kept_sentence` is always appended. A direct correction. → patch |
| low | An empty plan sends a spurious `ProvisioningProgress{0,0}` | Confirmed: the reporter's constructor sends before the `is_empty` return. A direct correction. → patch |
| low | The model revision is duplicated in `MODEL_BASE_URL` and `MODEL_REVISION` | Confirmed. A direct correction. → patch |
| low | Files already on disk (for example from the old curl recipe) are never size- or hash-checked | Real, but those files are the same pinned bytes, and the check (3.1) applies the same presence rule. The fix adds branches in both check and plan. Rejected |
| low | The SHA-256 pass re-reads each file after download with no progress | Real but about a second per 600 MB. Hashing while writing is a redesign. Rejected |
| low | No free-space check before download | The matrix covers disk-full as a named failure, which works. Rejected |
| low | No cancel or pause | Not in the intent or the ACs; a feature request. Rejected |
| low | The in-flight guard is per-process, so two processes can write one `.part` | Real but needs two voice-me instances on one cache; the checksum catches it. Rejected |
| low | The client does not set `https_only` | Every pinned URL is HTTPS, and integrity is pinned by SHA-256. Enforcing it needs a test-only flag. Rejected |
| low | Extraction runs with no status text | Extracting 24 MB takes under a second. Rejected |
| low | `sprint-status.yaml` `last_updated` moved backwards | Cosmetic. Corrected when status is synced at the end of this workflow. |
| false | The AC says no FP16 file exists in the cache after a CPU install, but existing FP16 files are left | The AC concerns what provisioning fetches; `a_cpu_provision_never_fetches_fp16_files_or_gpu_libraries` pins that. |
| low | The task says `main.rs` dispatches `provision`, but the view does | A deviation in placement only, following 3.1's `check_again` precedent. Its one named harm (the root not knowing about a click) is the two-owners group above, which is patched. Rejected |

## Verification

**Commands:**
- `cargo check --workspace` -- expected: clean, no new warnings.
- `cargo test -p voice-me-deps` -- expected: the matrix tests pass, with no external network access.
- `cargo test --workspace` -- expected: green.

**Manual checks:**
- Set `VOICE_ME_MODEL_CACHE` to an empty directory and click Install on both rows. Expect progress to be visible, an interruption and re-click to resume, both rows to end "ready", and speech to be generated in the same session.
