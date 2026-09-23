---
title: 'Dependency Check and graceful overlay block (Stories 3.1 + 3.4)'
type: 'feature'
created: '2026-09-22'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
baseline_commit: '90341a9afcda31d4efa989b36774e88d9a116b33'
context: ['{project-root}/_bmad-output/implementation-artifacts/epic-3-context.md']
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Nothing in the app knows whether the assets it needs are actually on disk. `DepsAdapter::check` is `todo!()`, `AppEvent::DependencyCheckCompleted` is never sent, Settings has no Dependencies tab, and a missing ONNX Runtime or model file surfaces only as a generation failure notification after the user has already typed a line.

**Approach:** Give `voice-me-deps` a real backend-aware Dependency Check that reports, per dependency, ready/missing plus what would fix it, publish that report on the `AppEvent` channel, render it as a Settings → Dependencies tab, and have the Prompt Overlay open into a blocked state naming the specific blocker instead of accepting input — recovering without a restart once the dependency is resolved.

## Boundaries & Constraints

**Always:** Required dependencies are derived from the **selected** backend (`AppState.speech_backend`), never a fixed list. The check only reads the filesystem and environment — detection, never provisioning. Detection results reach `voice-me-core` via `AppEvent` only; `voice-me-tts` never calls `voice-me-deps`. Status is stated in words ("missing"), never by colour alone. Virtual-microphone status is asked of `voice-me-audio-linux`, never re-detected here. Every control is keyboard-operable using gpui-kit components.

**Never:** No downloading, installing, or network access of any kind (Story 3.2). No backend selector UI (3.5), no remote-provider readiness (3.6), no GPU device enumeration (3.9), no ONNX Runtime build work (3.8). Do not silently substitute a backend. Do not persist the dependency report to `settings.toml`.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| All present | CPU backend; runtime dylib resolvable and present, all Q4 model files present | Report lists every row `ready`; overlay accepts input normally | N/A |
| Missing weights | CPU backend; `language_model_q4.onnx` absent | Model-weights row `missing`, detail names the exact absent path | Overlay blocks with that blocker named |
| Runtime unresolvable | `ORT_DYLIB_PATH` unset and no runtime in the cache root | ONNX Runtime row `missing`, detail says where it is looked for | Overlay blocks |
| Runtime path stale | `ORT_DYLIB_PATH` set to a path that no longer exists | Row `missing`, detail names the configured path that is gone | Overlay blocks |
| Weights for another variant | Q4 files absent but FP16 present, CPU backend selected | Row `missing` — the FP16 files do not satisfy a Q4 selection, and no FP16 row is reported at all | Overlay blocks |
| Unresolvable cache root | No `HOME`, no `XDG_CACHE_HOME`, no `VOICE_ME_MODEL_CACHE` | Check returns `Err`; Dependencies tab shows the check itself failed, with the reason | `VoiceMeError` surfaced in the tab, not a panic |
| Resolve then recheck | A missing file is placed on disk, then "Check again" pressed | Rows re-render as `ready`; the next hotkey press opens a normal overlay | N/A, no restart |
| Virtual mic down (Linux) | PipeWire sink unavailable, engine assets all present | Virtual-microphone row `missing`; overlay still accepts input and sends `SpeakRequested` | Playback fails later via the existing `VirtualMicUnavailable` notification |
| Windows | Any backend, Windows host | No virtual-microphone row is reported at all | N/A |
| Startup with a gap | Startup check finds a missing dependency | Settings → Dependencies opens by itself, focused on that tab, once for this launch | N/A |
| Empty text still blocked | Overlay blocked; user presses Enter | Nothing is sent; no `SpeakRequested` reaches the channel | N/A |

## Decisions

1. **Virtual Microphone row:** one row on Linux only, its status derived by asking `voice-me-audio-linux` whether its sink is available — no second copy of PipeWire detection. No row at all on Windows until Story 2.8 lands.
2. **When the check runs:** automatically at startup in the background, and on demand from a "Check again" button. When the startup check finds anything missing, Settings → Dependencies opens by itself — once per launch, on every launch, while something is still missing. No persisted "already shown" flag.
3. **What blocks the overlay:** only the speech-engine dependencies (ONNX Runtime and the model weights). A missing virtual microphone is reported as missing but still lets the user type; playback then fails through the existing `VirtualMicUnavailable` notification path.
4. **Selected backend:** a read-only line above the rows stating the selected backend and what it implies ("Backend: CPU — Q4 weights, no device selection"). Story 3.5 replaces the line with a `Select`; no disabled control ships in the meantime.

</frozen-after-approval>

## Code Map

- `crates/voice-me-core/src/ports.rs:157-160` -- `DependencyProvisioningPort::check` — widen to take the selected `SpeechBackend` and an `AppEventSender`, mirroring `HotkeyPort::start_listening`'s shape at `:20`.
- `crates/voice-me-core/src/event.rs:11` -- `AppEvent::DependencyCheckCompleted` — give it the report payload. Enum is `#[non_exhaustive]` and derives `Debug, Clone, PartialEq, Eq`, so the payload must too.
- `crates/voice-me-core/src/state.rs:19-73` -- `SpeechExecutionTarget`, `SpeechWeights`, `SpeechBackend` (with `::CPU`) already exist and are the vocabulary to derive the required list from. `AppState` (`:80+`) is where the latest report hangs; it is **not** a `SettingsFile` field.
- `crates/voice-me-core/src/settings_store.rs:43-102` -- `SettingsFile` and `FileSettingsStore`; uses `directories::ProjectDirs::from("dev","voice-me","voice-me")` at `:80`. Do not add a dependency field here.
- `crates/voice-me-tts/src/sessions.rs:120-188` -- `ModelCache`: `from_env` (`VOICE_ME_MODEL_CACHE`, else a hand-rolled `XDG_CACHE_HOME`/`$HOME/.cache` resolver at `:182`), `required_files` (`:152`, explicitly "this *is* Story 3.2's provisioning list"), `check` (`:169`), layout `<root>/onnx/*.onnx` + `.onnx_data` + `<root>/tokenizer.json`. **Reuse this knowledge — do not fork a second copy of the file list.** `voice-me-deps` must not depend on `voice-me-tts`, so lift the shared path/filename vocabulary into a new `voice-me-core` module and have both sides use it; keep `ModelCache`'s public API intact.
- `crates/voice-me-tts/src/sessions.rs:205-224` -- `init_runtime`: resolves the dylib from an explicit path or `ORT_DYLIB_PATH` and returns `MissingRuntimeAsset` when absent. Same resolution rule the runtime row must report on.
- `crates/voice-me-core/src/error.rs:32` -- `VoiceMeError::MissingRuntimeAsset { path }` already exists; reuse rather than adding a new variant.
- `crates/voice-me-deps/src/lib.rs:1-12` -- `DepsAdapter`, the `todo!()` to replace. Cargo.toml depends only on `voice-me-core`; keep it that way (no HTTP client in this story).
- `crates/voice-me-app/src/main.rs:108-119` -- `resolved_speech_backend()` returns `SpeechBackend::CPU` with a comment naming Story 3.1; `current_state` (`:126+`) rebuilds `AppState` per action from the store, so the latest report must be held app-side and merged in.
- `crates/voice-me-app/src/main.rs:578,846-908` -- the sole `AppEvent` receiver loop; `HotkeyPressed` → `open_overlay` (`:731-770`), `SettingsRequested` → `open_settings`. New `DependencyCheckCompleted` arm lands here.
- `crates/voice-me-ui/src/settings.rs:26-27,81-101` -- `TabBar` with `VOICE_TAB=0` / `HOTKEY_TAB=1`; add the Dependencies tab here and a way to open Settings focused on it.
- `crates/voice-me-ui/src/prompt_overlay.rs:55,63,87,143-151` -- `body()` carries a comment reserving that spot for Story 3.4's inline notice; `InputState` is auto-focused, Enter sends `SpeakRequested` at `:101-111`. Only `dismissing: bool` exists as view state today.
- `crates/voice-me-ui/src/voice_setup.rs`, `hotkey.rs` -- the pattern to copy for a new settings sub-view (construction, state, rendering).
- `crates/voice-me-core/src/speak.rs:104-146` -- precondition ladder (`EmptyText`, language, `NoReferenceVoiceSample`) and the single-notification contract. Do **not** add a dependency precondition here; the overlay block is the gate for this story.
- `crates/voice-me-tests/src/lib.rs` -- cross-crate integration tests live here; per-module fakes are inline (`speak.rs:172+`, `settings.rs:120+`). Follow the inline-fake pattern.
- `crates/voice-me-i18n/src/lib.rs` -- scaffold only; UI strings are inline literals today. Keep new strings inline (Epic 4 owns i18n).

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-core/src/assets.rs` (new) -- own the shared asset vocabulary: cache-root resolution (`VOICE_ME_MODEL_CACHE`, else the OS cache dir), the required model-file list per `SpeechWeights`, and the ONNX Runtime dylib resolution rule -- so `voice-me-deps` and `voice-me-tts` agree on one list without `deps` depending on `tts`.
- [x] `crates/voice-me-core/src/state.rs` -- add `DependencyStatus`, `Dependency` (label, status, detail, whether it is automatable), and `DependencyReport` (the backend it was computed for + the rows); hang the latest report on `AppState` as non-persisted state -- the UI and the overlay gate both read one value.
- [x] `crates/voice-me-core/src/event.rs` -- carry the report on `DependencyCheckCompleted` -- the report crosses the boundary on the event channel and nowhere else.
- [x] `crates/voice-me-core/src/ports.rs` -- widen `DependencyProvisioningPort::check` to take the selected backend and an `AppEventSender` -- the check is backend-relative and reports by event.
- [x] `crates/voice-me-core/src/lib.rs` -- export the new module and types.
- [x] `crates/voice-me-tts/src/sessions.rs` -- delegate `ModelCache`'s root resolution and `required_files` to `voice-me-core::assets`, keeping the public API and behaviour unchanged -- one list, one source.
- [x] `crates/voice-me-deps/src/lib.rs` -- implement `check`: build the rows for the selected backend from `voice-me-core::assets`, stat each path, and send `DependencyCheckCompleted` -- filesystem only, no network.
- [x] `crates/voice-me-audio-linux/src/lib.rs` -- expose a cheap, non-mutating "is the virtual-microphone sink available here" query for `voice-me-deps` to call -- detection stays where the PipeWire knowledge already is.
- [x] `crates/voice-me-ui/src/dependencies.rs` (new) -- the Dependencies view: the read-only backend line, rows with name, the word ready/missing, and a detail line naming the exact path or fix; a "Check again" button; the check-failed state -- this is the single place the user learns what is missing.
- [x] `crates/voice-me-ui/src/settings.rs` -- add the Dependencies tab and let callers open Settings focused on it -- Story 3.4 needs to auto-open exactly this tab.
- [x] `crates/voice-me-ui/src/prompt_overlay.rs` -- add a blocked state: the overlay opens and is dismissible but renders the blocker notice in `body()` instead of accepting input, and sends no `SpeakRequested` -- Story 3.4's core behaviour.
- [x] `crates/voice-me-app/src/main.rs` -- run the check at startup, hold the latest report, merge it into `current_state`, handle `DependencyCheckCompleted` (opening Settings → Dependencies once per launch while anything is missing), and route a hotkey press blocked by a speech-engine gap to the blocked overlay -- the composition root owns wiring, not the adapters.
- [x] `crates/voice-me-deps/src/lib.rs` (tests) -- unit-test the I/O matrix rows against temporary directories: all-present, each missing asset, a stale configured runtime path, the wrong weight variant present, and an unresolvable cache root.
- [x] `crates/voice-me-tests/src/lib.rs` -- integration test: a report with a missing speech-engine dependency yields a blocked overlay decision and no `SpeakRequested`, and the same decision flips once the report comes back all-ready.

**Acceptance Criteria:**
- Given the app has started, when the Dependency Check runs, then Settings → Dependencies lists one row per dependency the selected backend actually needs, each with the word ready or missing.
- Given the CPU backend is selected, when the check runs, then no GPU provider library and no FP16 weight row appears in the report.
- Given a dependency is missing, when the user presses the global hotkey, then the Prompt Overlay opens showing an inline notice naming that specific blocker, refuses input, and Settings → Dependencies opens focused on the Dependencies tab.
- Given the overlay is blocked, when the user presses Escape, then it dismisses exactly as an unblocked overlay does.
- Given the missing dependency is then resolved and the check re-run, when the user presses the hotkey, then the overlay accepts input normally with no app restart.
- Given the check is run from Settings, when it completes, then the rows re-render from the new report without the user leaving the window.
- Given the startup check finds a missing dependency, when the app finishes launching, then Settings → Dependencies opens by itself once for that launch, and does so again on the next launch while the dependency is still missing.
- Given only the virtual microphone is missing, when the user presses the hotkey, then the overlay accepts input normally and the failure surfaces at playback rather than at typing time.

## Implementation Notes

- **`voice-me-core::assets` is the single asset vocabulary.** Cache-root
  resolution (`VOICE_ME_MODEL_CACHE`, else the OS cache dir — the unix
  branch is the old `voice-me-tts` resolver moved verbatim so `XDG_CACHE_HOME`
  with no `HOME` still works), the per-`SpeechWeights` file list, and the
  ONNX Runtime resolution rule now live there. `ModelCache::from_env`,
  `ModelCache::required_files`, `LanguageModel::file_name` and
  `tokenizer_path` all delegate to it with their public APIs unchanged.
- **`init_runtime` was widened to the same rule.** It previously failed
  outright when `ORT_DYLIB_PATH` was unset; it now falls back to
  `<cache root>/runtime/<libonnxruntime>` — the exact path the runtime row
  reports on. Without this the check could say "ready" about a library the
  engine would then refuse to find.
- **Blocked-overlay focus.** A blocked overlay renders no `Input`, so the
  `Escape` action bound in the `"Input"` key context is never dispatched.
  The frame takes focus in that shape and handles the raw key itself, which
  is what keeps Escape behaving identically to an unblocked overlay.
- **Failed check blocks too.** `overlay_blocker` treats
  `DependencyOutcome::Failed` (an unresolvable cache root, say) as a
  blocker naming the reason: the check failed on exactly the paths the
  engine would have read. `Pending` deliberately does not block — refusing
  a Speak Action because a background check has not landed yet would be a
  worse failure than the engine's own named error.
- **`DependencyKind` carries the blocking decision.** `Dependency` has the
  four fields the spec named plus `kind`, so "does this stop the Speak
  Action" is a match on a value rather than on prose.
- **`SettingsView::new` takes a `DependenciesTab` struct** rather than four
  more positional parameters — twelve arguments was a place for two of them
  to be swapped silently.
- **Matrix-audit additions (verification pass).** Three matrix rows had no
  running test: the startup auto-open rule, the Windows "no virtual-mic row"
  case, and the check-failed state of the tab. The auto-open condition was
  extracted from `main`'s closures into
  `should_auto_open_dependencies(already_auto_opened, anything_missing)` so
  Decision 2 is testable as a decision; the Windows case is a
  `cfg(not(target_os = "linux"))` test that runs on the Windows CI job; the
  failed state is a `DependenciesView` render test.
- **Review patch round.** All ten triaged patches were applied: the doubled
  blocker sentence, the "Loaded from" claim the check had not earned,
  `is_present`/`device_count` delegating to `virtual_microphone_available`,
  the empty-`VOICE_ME_MODEL_CACHE` asymmetry, `DependencyOutcome` moved into
  `voice-me-core` with `AppState.dependencies` carrying it (so a failed check
  is representable and the field has a reader), the guard that stops a repeat
  hotkey press dismissing its own blocked overlay, `check_again` moved off the
  UI thread behind a `Send + Sync` port, `cargo test --workspace` added to the
  Windows CI job, and the two missing tests — the "Check again" click and
  `init_runtime`'s cache-root fallback. The implementation agent was
  interrupted partway through; the `init_runtime` test, a `click_on` typo, and
  a `derivable_impls` clippy warning on `DependencyOutcome` were finished
  here.

## Spec Change Log

## Review Triage Log

Pass 1 (blind-hunter, edge-case-hunter, verification-gap).

| Verdict | Finding | Evidence |
|---|---|---|
| medium | `blocker_notice` doubles the prefix: "Missing: Speech model files (Q4). Missing: /cache/…" | Confirmed — `dependencies.rs` formats `"Missing: {label}. {detail}"` and `model_weights_row`'s detail already begins `"Missing: "`. The blocked overlay is the story's headline surface. → patch |
| medium | The ready runtime row says "Loaded from {path}" after only a `Path::exists()` | Confirmed at `deps/src/lib.rs:91`. Nothing is loaded until `init_runtime`; the epic's "truth over silent fallback" rule makes the wording matter. → patch |
| medium | `check_again` runs a PulseAudio round trip on the UI thread | Confirmed — `DependenciesView::check_again` calls `check` inline, which on Linux reaches `PulseSession::connect`, whose readiness loop (`pulse.rs:79-90`) has no timeout. The startup path uses `background_spawn` for exactly this reason. → patch |
| medium | A repeat hotkey press while blocked dismisses the overlay it just opened | Confirmed by construction: the press calls `open_dependencies` (activating Settings) before `open_overlay`; an already-open overlay dismisses on lost activation, so the second press leaves a dying window. The first press is fine — the overlay is created after. → patch |
| medium | The Windows "no virtual-microphone row" test never runs | Confirmed — `.github/workflows/ci.yml`'s `build-windows` job runs `cargo build --workspace` only; `cargo build` does not compile `#[cfg(test)]` code, and the Ubuntu job compiles the test out. → patch |
| medium | `DependencyOutcome` lives in `voice-me-ui` but is the composition root's gate type, and `AppState.dependencies` has no reader | Confirmed and grouped: one root cause. `overlay_blocker`/`current_state` take `&DependencyOutcome` from the view crate (AD-1 puts state vocabulary in core), and `AppState.dependencies` is written at `main.rs:190` and read nowhere — with `None` meaning both "not checked" and "check failed". Story 3.3 is the next reader and would meet it. Routed patch rather than bad_spec: the fix is a cross-crate move of a type the spec already placed in core's vocabulary, introducing no new concept. → patch |
| medium | `check_again`'s asking half is never exercised — no test clicks the button | Pre-verified by the verification-gap layer and confirmed: `a_check_that_could_not_run_says_so_with_its_reason` supplies `Failed` by hand and never calls the stub's `check`. Deleting the `on_click` leaves every test green. → patch |
| medium | `init_runtime`'s new cache-root fallback is pinned by no test | Pre-verified and confirmed: the only `init_runtime` test uses the explicit-path arm. The widened rule is the one thing keeping the "ready" row from lying, and it can drift back silently. → patch |
| low | `model_cache_root` does not filter an empty `VOICE_ME_MODEL_CACHE` while `resolve_runtime_dylib` filters an empty `ORT_DYLIB_PATH` | Confirmed; `VOICE_ME_MODEL_CACHE=""` yields a relative root and relative paths in every row. Unlikely, but the fix is a one-line direct correction symmetric with its neighbour. → patch |
| low | `virtual_microphone_available` is a verbatim copy of `is_present`, whose doc claims it is the same call | Confirmed — three copies of `PulseSession::connect()?.source_count(DEVICE_NAME)?` (`is_present`, `device_count`, the new free function). Direct correction: delegate. → patch |
| low | `check_again`'s `Err` path updates the view but not the composition root's held outcome | Real but unreachable in this story: `check` returns `Err` only when `model_cache_root()` fails, which depends on process environment that cannot change after launch — a startup check that succeeded means a later one cannot fail this way. The fix needs a new `AppEvent` variant. Rejected; flagged for Story 3.2, where provisioning adds reachable failure modes. |
| low | The auto-open one-shot can be consumed by a manual "Check again" that completes first | Real but harmless: pressing "Check again" requires Settings to already be open, which is the state auto-open exists to produce. Fix adds a parameter. Rejected. |
| low | Windows cache root moves from `%HOME%\.cache\voice-me` to `BaseDirs::cache_dir()` | Real behaviour change (the old resolver was not cfg-split), but no Windows install has a cache to lose — provisioning is Story 3.2 and Epic 2 shipped Linux-only. The new location is the correct one. Fix is a migration. Rejected. |
| low | `the_report_carries_the_backend_it_was_computed_for` opens a real audio-server connection from a unit test | Confirmed, and against the repo's `#[ignore = "needs a running audio server"]` convention. Harmless in practice: with no server `context.connect` fails fast and the row simply reports missing. The hang needs a server that connects and never becomes ready. Rejected; the underlying no-timeout loop is deferred below. |
| low | `settings_view_slot` is not cleared when `open_window` returns `Err` | Confirmed by reading the `Err` arm. Consequence is a view entity with no window receiving updates that render nowhere — no user-visible effect, and `open_window` failing is already treated as fatal-ish (logged, return). Rejected. |
| low | A blocked overlay left open does not update when the dependency is resolved | Confirmed: `set_dependency_outcome` reaches only `DependenciesView`. The acceptance criterion is about the *next* hotkey press, which works. Fix is a new push path into the overlay window. Rejected. |
| maybe-false | `required_model_files` assumes every graph has an `.onnx_data` sibling, and that list now gates typing | The rule is unchanged from `ModelCache::required_files` (pre-existing, moved verbatim) — only its consequence is new. Would be settled by listing a real provisioned cache and checking whether `embed_tokens.onnx_data` and `conditional_decoder.onnx_data` exist. → defer |
| low | `speech_engine_rows` ignores `backend.target`, so a GPU selection yields the CPU row set | True, and out of this story's scope by intent as well as by spec — Stories 3.3 and 3.8 own GPU backends, and no GPU backend can be selected until 3.5. Not caused by this change. → defer |
| false | Documents disagree: the spec says `in-review` while sprint-status says `in-progress` | These are different facts, not a contradiction: sprint-status tracks the story through the sprint, the spec frontmatter tracks the build workflow's own state, and each was correct when written. Sprint-status is synced to `review` at the end of this workflow. |

## Design Notes

The report is a value, not a service: `voice-me-deps` computes it, sends it once on the event channel, and holds nothing. The composition root keeps the latest one and merges it into the `AppState` it already rebuilds per action, so both the Dependencies tab and the overlay gate read the same value without a second source of truth.

`voice-me-core::assets` exists to break an otherwise unavoidable dependency: `voice-me-deps` needs the model-file list, that list lives in `voice-me-tts`, and `deps → tts` is the wrong direction (AD-9 keeps the two apart). Core already carries this kind of shared vocabulary — `SpeechExecutionTarget` exists precisely so core and `tts` can name the same thing without either importing the other. Core gains no `ort` dependency and no network: it owns path strings and filenames only.

## Verification

**Commands:**
- `cargo check --workspace` -- expected: clean, no new warnings.
- `cargo test -p voice-me-deps` -- expected: the I/O-matrix unit tests pass.
- `cargo test --workspace` -- expected: green, including the existing `voice-me-tts` and settings-store tests, which must be unaffected by the `ModelCache` delegation.

**Manual checks (if no CLI):**
- Launch the app with `VOICE_ME_MODEL_CACHE` pointed at an empty directory: Settings → Dependencies shows the model rows as missing with their exact paths, and a hotkey press opens the blocked overlay naming the blocker.
- Point it back at the provisioned cache and press "Check again": rows flip to ready and the next hotkey press opens a normal, typable overlay in the same session.
