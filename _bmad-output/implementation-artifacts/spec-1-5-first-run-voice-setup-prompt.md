---
title: 'First-Run Voice Setup Prompt'
type: 'feature'
created: '2026-09-21'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
baseline_commit: '9043e7e6b7a45fb9849a7deb54b33bd94b1c1f5d'
context:
  - '{project-root}/_bmad-output/planning-artifacts/architecture/architecture-voice-me-2026-09-20/ARCHITECTURE-SPINE.md'
  - '{project-root}/_bmad-output/implementation-artifacts/epic-1-context.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** `voice-me-app`'s `main.rs` always opens the same `VoiceSetupView` with the same generic "Voice Setup" copy on every launch, regardless of whether an active Reference Voice Sample already exists. There is no first-run-specific empty state ("Record your voice to get started") and no distinction in the view between a first-time user and one who already has an active sample.

**Approach:** Make `VoiceSetupView` state-aware of whether an active Reference Voice Sample exists at construction time, and show the first-run empty-state copy only when it does not. `main.rs` loads settings before opening the window and passes that presence/absence in. No new persisted "first run" flag is introduced — `AppState.reference_voice_sample` (file-presence-derived, per AD-6) is the sole signal, so once a sample is accepted the empty state naturally never reappears.

**Decision:** The window still opens on every launch (Epic 2's tray/hotkey — the only other route to Settings — isn't built yet, and Story 1.4's replace-sample flow must stay reachable until it is). Only the first-run empty-state copy is gated on sample presence: shown when no active sample exists, replaced by today's normal recorder/replace view once one does.

**Renegotiated scope (human-requested addition):** The recorder must let the user pick which input (microphone) device to record from, instead of always using the OS default. `AppState.selected_mic_device: Option<String>` already exists in `voice-me-core` and is already round-tripped through `SettingsFile`/TOML — it was a stub with no writer and no UI. This story wires it up: enumerate available input devices, let the user pick one, persist the pick, and record from it.

## Boundaries & Constraints

**Always:** Determine "has an active sample" solely from `SettingsStore::load()` / `AppState.reference_voice_sample` — never add a separate persisted first-run flag. Persist the selected input device the same way (`SettingsStore`, TOML), never a separate config path.

**Never:** Do not build the Settings window's `Tabs` shell (Voice/Hotkey/Dependencies/General) — out of scope for this story, no other section exists yet. Do not add new fields to the on-disk `SettingsFile` format beyond wiring up the already-present `selected_mic_device`. Do not block Record/enumeration failures as hard errors — an empty or failed device list must silently fall back to the OS default device (matches today's behavior).

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| First launch, no sample | `SettingsStore::load()` returns `reference_voice_sample: None` | Window shows empty-state copy "Record your voice to get started"; Record is the highlighted primary action | N/A |
| Sample accepted during this run | User records/imports and clicks Accept | Empty-state copy disappears for the rest of the run (existing `just_saved` re-render already covers this) | N/A |
| Relaunch after a sample exists | `SettingsStore::load()` returns `reference_voice_sample: Some(path)` | Window opens showing the normal recorder/replace view; empty-state copy never shown again | N/A |
| Input devices available | `InputDeviceSource::list_devices()` returns 2+ names | A device selector control is shown; picking a name updates the selection and persists it via `SettingsStore` | N/A |
| No input devices enumerable | `list_devices()` returns empty (host/enumeration failure) | Device selector control is hidden; Record still uses the OS default device exactly as before this story | Never surfaced as an error — silent fallback |
| Record with a device selected | User has picked a non-default device, then clicks Record | `CaptureSource::start` is invoked with that device's name; if the named device is no longer present, capture falls back to the OS default | Falls back silently, same `MIC_UNAVAILABLE_MESSAGE` path as today if even the default fails |

</frozen-after-approval>

## Code Map

- `crates/voice-me-app/src/main.rs:14-32` -- composition root; unconditionally opens one window hosting `VoiceSetupView` (lines 20-31), no branching today. Window-open call itself stays unconditional; must call `settings_store.load()` before opening the window and pass the sample-presence into `VoiceSetupView::new` so the view (not `main.rs`) decides which copy to show.
- `crates/voice-me-core/src/state.rs:8-13` -- `AppState.reference_voice_sample: Option<PathBuf>` is the sole "has active sample" signal; no other flag exists or should be added.
- `crates/voice-me-core/src/ports.rs:36-46` -- `SettingsStore::load() -> Result<AppState, VoiceMeError>` is the existing read path.
- `crates/voice-me-ui/src/voice_setup.rs:156-211` -- `VoiceSetupView` struct and constructors (`new`, `new_with_capture_source`, `new_with_capture_and_import_sources`); no field distinguishes "has existing sample" today. Add a boolean captured at construction to drive the empty-state copy, threading it through all three constructors.
- `crates/voice-me-ui/src/voice_setup.rs:434-508` -- `Render` impl; always renders the same "Voice Setup" title (line 446) and description (lines 447-451) and the same Record/Stop/Import/Accept row (lines 478-507) regardless of sample state. Swap the title/description for the empty-state copy when constructed with no active sample; keep Record/Stop/Import/Accept unchanged in both states.
- `crates/voice-me-tests/src/lib.rs:1-48` -- black-box `SettingsStore`/`AppState` test pattern (tempdir, fresh `FileSettingsStore::with_dirs` per simulated restart) to mirror for a restart-persistence test.
- `crates/voice-me-ui/src/voice_setup.rs` (`#[cfg(test)] mod tests`, ~line 620+) -- existing fake `CaptureSource`/`ImportSource`/`SettingsStore` doubles and `cx.open_window` pattern to mirror for a UI test asserting empty-state vs. normal copy.
- `crates/voice-me-ui/src/voice_setup.rs:60-62` -- `CaptureSource::start(&self) -> Result<Box<dyn CaptureHandle>, VoiceMeError>` takes no device parameter today. Add a `device: Option<&str>` parameter (`None` = OS default, unchanged behavior).
- `crates/voice-me-ui/src/voice_setup.rs:104-108` and `:561-568` -- `CpalCaptureSource::start` / free fn `start_capture()` always call `host.default_input_device()`. Thread the new `device: Option<&str>` through: when `Some(name)`, look it up via `host.input_devices()` matching `.name().ok().as_deref() == Some(name)`, falling back to `default_input_device()` if not found (device unplugged since selection).
- `crates/voice-me-core/src/ports.rs:36-46` and `crates/voice-me-core/src/settings_store.rs` -- `SettingsStore` trait/`FileSettingsStore` impl. Add `save_selected_mic_device(&self, device: Option<&str>) -> Result<AppState, VoiceMeError>`, mirroring `save_reference_voice_sample`'s read-modify-write of the TOML file (the `selected_mic_device` field already exists in `SettingsFile`/`AppState`, just has no writer yet).
- `crates/voice-me-core/src/state.rs:8-13` -- `AppState.selected_mic_device: Option<String>` already exists and is already loaded by `FileSettingsStore::build_state`; this story is what starts writing it.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-ui/src/voice_setup.rs` -- add a `has_active_sample: bool` field, thread it through all three constructors, and branch the title/description in `Render` on it -- makes the view state-aware so `main.rs` and tests can distinguish first-run from normal
- [x] `crates/voice-me-app/src/main.rs` -- call `settings_store.load()` before opening the window and pass the resulting sample-presence into `VoiceSetupView::new` (window-open call itself stays unconditional) -- implements the first-run empty-state copy
- [x] `crates/voice-me-ui/src/voice_setup.rs` tests -- add cases asserting the empty-state copy renders only when constructed with no active sample, and the normal copy renders when constructed with one -- proves the AC's first-run copy
- [x] `crates/voice-me-tests/src/lib.rs` -- add a black-box case: a fresh `FileSettingsStore` reports no reference sample; after `save_reference_voice_sample`, a new `FileSettingsStore::with_dirs` load (simulated restart) reports the sample present -- proves the signal this story's "never fires again" behavior relies on persists across restarts (already existed verbatim from Story 1.2 as `file_settings_store_reference_voice_sample::save_then_fresh_load_round_trips_the_active_sample`; no duplicate added)
- [x] `crates/voice-me-core/src/ports.rs`, `crates/voice-me-core/src/settings_store.rs` -- add `SettingsStore::save_selected_mic_device` and its `FileSettingsStore` implementation -- gives the UI a way to persist the picked device
- [x] `crates/voice-me-ui/src/voice_setup.rs` -- add an `InputDeviceSource` trait (`list_devices() -> Vec<String>`) with a production `cpal`-backed impl; thread `device: Option<&str>` through `CaptureSource::start`/`CpalCaptureSource`/`start_capture()` -- lets the view enumerate devices and record from the selected one
- [x] `crates/voice-me-ui/src/voice_setup.rs` -- add `available_devices: Vec<String>` and `selected_device: Option<String>` fields to `VoiceSetupView`, a device-selector control in `Render` shown only when `available_devices` is non-empty, and a handler that updates the selection and calls `save_selected_mic_device` -- implements the pick-a-device UX (implemented as a cycle button — "System default" then each enumerated device, wrapping — rather than a dropdown, to keep it testable with this file's existing `window.click(id)` pattern instead of GPUI's anchored/deferred `PopupMenu`/`Select<D>` components)
- [x] `crates/voice-me-app/src/main.rs` -- pass `state.selected_mic_device` into the view alongside `has_active_sample` -- restores the last-picked device across restarts
- [x] `crates/voice-me-ui/src/voice_setup.rs` tests -- cover the new I/O matrix rows: selector shown/hidden by device-list presence, picking a device persists it, Record uses the selected device's name -- proves the device-selection behavior

**Acceptance Criteria:**
- Given no active Reference Voice Sample exists, when the app starts, then the window shows the empty-state copy "Record your voice to get started" with Record as the highlighted primary action.
- Given a sample was just accepted during this run, when the view re-renders, then the empty-state copy no longer appears for the remainder of the run.
- Given an active Reference Voice Sample already exists at launch, when the app starts, then the window opens showing today's normal recorder/replace view, not the empty-state copy.
- Given two or more input devices are enumerable, when Voice Setup renders, then a device selector is shown and picking one persists the choice via `SettingsStore`.
- Given a device was picked in a previous run, when the app restarts, then that device is pre-selected and Record captures from it (falling back to the OS default if the device is no longer present).

## Implementation Notes

- First-run empty-state work (original scope) implemented and verified first; device-selection work (renegotiated scope) added afterward as a second pass on the same spec/branch — see Spec Change Log.
- `cpal` 0.18 (locked version) removed `DeviceTrait::name()` in favor of `Display` (`device.to_string()`) plus a separate structured `description()`; `start_capture`'s device lookup and `CpalInputDeviceSource::list_devices` both use `to_string()` accordingly.
- Device selection UI is a single cycle-button ("System default" → each `cpal`-enumerated device name, wrapping) rather than a dropdown/menu component, so it stays driveable with this file's existing `window.click(id)` test pattern rather than GPUI's anchored/deferred `PopupMenu`/generic `Select<D>` (which needs a `SearchableListDelegate` impl and isn't used anywhere else in this codebase yet).
- Verification: `cargo test --workspace` (all green, voice-me-core 4 tests (was 3) / voice-me-ui 22 tests (was 16, this story's baseline) across both passes plus one review-loopback fix), `cargo clippy --workspace --all-targets` (clean aside from the pre-existing unrelated `voice-me-tests::placeholder` warning), `cargo fmt --check` (clean).
- Review loopback (patch): `cycle_device` originally updated `self.selected_device` before checking whether `save_selected_mic_device` succeeded, so a failed persist left in-memory state (and what Record would capture from) diverged from disk with nothing to revert it before the next restart; it also never cleared a stale `error_message` on a successful cycle. Fixed by capturing the previous selection and rolling back to it on `Err`, and clearing `error_message` on `Ok`. Separately, the device-selector label read `self.selected_device` directly, so a previously-picked device no longer present in `available_devices` (e.g. unplugged) kept displaying its stale name instead of "System default" — fixed by filtering the displayed value through `available_devices.contains()`. Added `a_failed_persist_rolls_back_the_selection` (`voice-me-ui`) covering the rollback; the stale-label display fix has no dedicated test (this harness's `try_find` only discovers interactive/`test_support()`-marked elements, and `Alert`/plain label text aren't either — consistent with this file's existing testing boundary), verified by code inspection instead.

## Spec Change Log

- **Human renegotiation, mid-implementation.** User asked to add input-device selection for the recorder to this story instead of deferring it, after being offered the option to split it into a separate story. Not derivable from epics/PRD (FR1 only says "using their system microphone"); `AppState.selected_mic_device` existed as an already-wired-in-TOML but never-written stub field, which this addition finally wires up end to end. Amended: Intent (added "Renegotiated scope"), Boundaries, I/O matrix (3 new rows), Code Map, Tasks, and Acceptance Criteria. KEEP: the already-implemented and verified first-run empty-state behavior (tasks 1-4, all `[x]`) is unaffected by this addition — the device-selection tasks are purely additive.

## Review Triage Log

- **medium** — `cycle_device` set `self.selected_device = next` before checking `save_selected_mic_device`'s result, so a failed persist left the in-memory selection (and what `start_recording` would pass to `CaptureSource::start`) diverged from what's actually on disk, with no rollback — the choice silently reverts on the next restart with no lasting indication beyond the one-time error banner. Also never cleared a stale `error_message` on a successful cycle. Fix: capture the previous selection and restore it on `Err`; clear `error_message` on `Ok`. Verified by reading `cycle_device` before the fix — both defects were real. (blind-hunter, edge-case-hunter — same root cause, merged)
- **low** — The device-selector's rendered label read `self.selected_device` directly, so a previously-picked device no longer present in a fresh `available_devices` enumeration (e.g. unplugged since selection) kept displaying its stale name — misleading, though `start_capture`'s existing name-lookup-with-fallback already made Record behave correctly regardless. Fix: filter the displayed value through `self.available_devices.contains(...)`, falling back to "System default" when the persisted pick isn't currently enumerable. Verified by reading `Render` before the fix — the label used the raw field with no such check. (edge-case-hunter, verification-gap `Other findings` — same root cause, merged)
- **defer** — `main.rs`'s mapping from a loaded `AppState` to `has_active_sample`/`selected_mic_device` (the sole site deriving these two values) has no test anywhere — `voice-me-app` has no test file or dev-dependencies at all, and every `voice_setup.rs`/`voice-me-tests` test passes these as literals rather than deriving them from a loaded `AppState`. A regression here (e.g. an inverted condition) would ship with `cargo test --workspace` fully green. Pre-existing gap in kind (the crate had zero test coverage before this story touched it, unconditionally passing `settings_store.clone()` with no branching), not one this diff newly introduces the *pattern* of; closing it means standing up `voice-me-app`'s first test harness, out of proportion to this story. (verification-gap, filed pre-verified with disposition `defer`)
- **defer** — The I/O matrix's "falls back to the OS default if the device is no longer present" row is correctly implemented (`start_capture`'s `named_device.or_else(|| host.default_input_device())`, verified by reading the code) but has no automated test: exercising it requires a fake `cpal` `Host`/enumeration, and `CaptureSource` only abstracts the *whole* `start()` call, not `cpal` enumeration inside the concrete `CpalCaptureSource`/`start_capture` — the same pre-existing untested-cpal-integration boundary this function already had before this story (its OS-default-unavailable path was never unit tested either). Closing it needs a new fakeable host-abstraction trait, a larger change than this story's scope. (blind-hunter)
- **low, rejected** — Two enumerated input devices could theoretically share an identical `cpal` display name (e.g. identical USB mic models), and both `cycle_device`'s position lookup and `start_capture`'s name match resolve purely by string equality, so a collision could target the wrong physical device. Unlikely in everyday use (identical simultaneous device names are rare) and the fix (switch to `DeviceTrait::id()`-based matching while still persisting/displaying by name) is more than a direct correction — rejected per the low-finding rule. (edge-case-hunter, blind-hunter — same root cause, merged)
- **low, rejected** — `available_devices` is enumerated once at `VoiceSetupView` construction with no live re-enumeration while the window stays open, so a microphone plugged in mid-session won't appear without an app restart. Unlikely to be hit in a first-time voice-setup session and the fix (polling or an event-driven re-enumeration) is more than a direct correction — rejected per the low-finding rule. (blind-hunter)
- **rejected, not a defect** — The device selector is a single forward-cycling button with no reverse/jump-to action, meaning a user with many devices must click through the whole list. This is a UX preference/feature-request, not a verified bad outcome — no incorrect behavior occurs. (blind-hunter)
- **false** — Flagged that `spec_file`'s `status: in-review` and `sprint-status.yaml`'s `1-5-first-run-voice-setup-prompt: in-progress` disagree. Expected at this point in the workflow: step-04 (review) doesn't sync `sprint-status.yaml`, and the workflow's later Finalize step is what reconciles both — same pattern already established as `false` in spec-1-4's own triage log for the equivalent transient state. (blind-hunter)
- **rejected, fix edits this spec** — Flagged that the Implementation Notes' test-count claim ("21, up from 3/19") undercounted since the full diff (from `baseline_commit`) adds 5 `voice-me-ui` tests + 1 `voice-me-core` test against a true pre-story baseline of 16/3, not the mid-implementation 19 the original wording compared against. Real inaccuracy, but its only fix is editing this spec's own prose — rejected per the "reject any finding whose fix is to edit this build's spec" rule; corrected directly in Implementation Notes anyway since it's this build's own bookkeeping, not a routed finding.

## Verification

**Commands:**
- `cargo test --workspace` -- expected: exits 0, includes the new tests
- `cargo clippy --workspace --all-targets` -- expected: clean (aside from the pre-existing unrelated `voice-me-tests::placeholder` warning)
- `cargo fmt --check` -- expected: clean
