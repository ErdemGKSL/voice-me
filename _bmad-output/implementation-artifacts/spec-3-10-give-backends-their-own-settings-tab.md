---
title: 'Give backends their own Settings tab (Story 3.10)'
type: 'refactor'
created: '2026-09-23'
status: 'done'
route: 'dispatch'
review_loop_iteration: 1
baseline_commit: '1493cce2a275c8d12027a8112ebe64de296073c7'
context: ['{project-root}/_bmad-output/implementation-artifacts/epic-3-context.md']
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Settings → Dependencies now holds everything about the backend: the `Select`, the Selected/Active lines, added runtimes, both API keys, and the remote sample state, all around the dependency rows. It is no longer "one primary view" (UX-DR7), and 3.7, 3.9, 3.11 and 3.14 would each add more to it.

**Approach:** Add a Settings → **Backend** tab (`BackendView`), placed between Hotkey and Dependencies. It is two-step: a Local/Remote choice, then a `Select` of that kind's backends. Below that it shows only the selected backend's options, then the Selected/Active lines, the CPU-mode tag and the restart line. Dependencies keeps the dependency rows, Check again, and the capability row, which gains an **Open Backend tab** link. This moves UI only; `BackendAction`, `BackendPanel` and the composition root's handling stay as they are.

## Boundaries & Constraints

**Always:**
- **Both views use the same channels as today.** They are fed the same `BackendPanel` and ask through the same `BackendActions`. Neither view touches `SettingsStore`. `SettingsView::set_backend_panel` pushes the panel to both views, because Dependencies still needs `check_request` and the Capability error.
- **Only existing backends are listed.** Local: the bundled CPU entry and each added runtime's entries (`backend_choices`). Remote: `RemoteProvider::ALL` (DeepInfra, fal.ai). The System voice and Azure join when 3.12, 3.13 and 3.14 build them.
- **Options per kind.**
  - Local: the added-runtimes list with **Add runtime…** and Remove.
  - Remote: that provider's masked key field with Save and Remove, the plaintext notice, and "Voice sample on …" with delete. The sample line appears only for `SAMPLE_HOLDING_PROVIDERS`.
- **Unchanged from 3.3/3.6:** element ids, the key-redacting `Debug`, the CPU-mode tag (UX-DR18), restart only when `restart_pending` is set, and inline errors next to their control. Existing UI tests move with their sections and keep passing.
- **The Use CPU backend button stays on the capability row**, as in Story 3.3. The new link sits beside it.
- The layout follows the gpui-kit design guides.

**Never:**
- No change to `voice-me-core`, `voice-me-deps` or `voice-me-tts-remote`, and no new `BackendAction` variant. The one exception is Decision 3's wording.
- No per-backend language (3.11), System voice (3.12/3.13), Azure (3.14), or GPU device picker (3.9).
- No General tab.

## Decisions

1. **Flipping Local ↔ Remote only changes the view (answered 2026-09-23).** The saved selection is unchanged. The second `Select` lists the new kind's backends with none chosen, with the placeholder "Choose a local backend" or "Choose a remote backend". No options are shown, and Selected/Active still name the saved backend. Nothing is saved until an entry is picked. When the panel's saved selection changes, the kind follows it. Decision 2 carves out two exceptions.
2. **Two things stay reachable whichever backend is saved (answered 2026-09-23, after review pass 1).** The saved backend's options stay as in Decision 1, with two exceptions:
   - Whenever the Local kind is shown, the added-runtimes list and **Add runtime…** are shown, even if a Remote backend is saved. That is how local entries come to exist.
   - Whenever the Remote kind is shown, every provider that is *not* the saved one but holds the Reference Voice Sample or has a saved key gets one short line. The line reads "<Provider>: key saved · voice sample held", naming only what applies, with **Remove key** and/or **Delete from <Provider>**. It uses the same `BackendAction`s, and its errors and Deleting… state appear inline on that line.

   Entering a new key still means picking that provider first.
3. **The remote capability row names the Backend tab (answered 2026-09-23, after review pass 1).** `voice-me-deps/src/capability.rs:312` changes from "add one under API keys." to "add one in Settings → Backend.", along with its test at `:648`. Nothing else in `voice-me-deps` changes.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Open Backend tab | Saved selection is Local CPU | Kind = Local; `Select` shows CPU; runtimes shown; no key field; CPU-mode tag | N/A |
| Open with remote saved | DeepInfra saved | Kind = Remote; DeepInfra's key field, notice, sample line; no runtimes | N/A |
| Pick a backend | `Select` confirm on another entry of the current kind | `BackendAction::Select` once; re-confirming the current entry sends nothing | Save failure → Selection error inline, `Select` resyncs to saved |
| Flip kind | Toggle Local→Remote while CPU is saved | Kind = Remote; `Select` empty with placeholder; no options; Selected still says CPU; no action sent (Decision 1) | N/A |
| Capability row | Dependencies shows a can't-run row | Use CPU backend → `UseCpu` once; Open Backend tab switches to the Backend tab | N/A |
| Blocked overlay | `show_dependencies()` | Still lands on Dependencies | N/A |
| Local shown, remote saved | DeepInfra saved; kind flipped to Local | Runtimes list and **Add runtime…** are shown; no key field (Decision 2) | Probe errors are shown inline |
| Sample held elsewhere | CPU saved; DeepInfra holds the sample; kind = Remote | A DeepInfra line with **Delete from DeepInfra** → `DeleteRemoteSample` once | Its error and Deleting… state are shown on that line |
| Key saved elsewhere | DeepInfra saved; fal.ai has a key; kind = Remote | A fal.ai line with **Remove key** → `SaveApiKey(FalAi, None)` once | Its error is shown on that line |
| No key, remote saved | DeepInfra saved with no key | Capability row: "DeepInfra has no API key — add one in Settings → Backend." (Decision 3) | N/A |

</frozen-after-approval>

## Code Map

- `crates/voice-me-ui/src/dependencies.rs:736-1033` -- `backend_section`, `runtimes_section`, `api_keys_section`, `remote_samples_section`, `SAMPLE_HOLDING_PROVIDERS`, `provider_slug`, `choices`/`BackendChoice`, `API_KEY_STORAGE_NOTICE`, the `BackendAction`/`BackendArea`/`BackendPanel` types (60-160), and `add_runtime`/`save_key`/`sync_controls` and the `Select`/key-input state (236-392) all move to the new `backend.rs`. `row()`'s capability branch (651-672) stays and gains the link. `Render` (1053) drops the four backend sections and keeps the heading, rows and Check again. `DependenciesView` keeps `panel` only for `check_request` and the Capability error.
- `crates/voice-me-ui/src/dependencies.rs:1660-2038` -- backend tests: `recording_actions`, `cuda_panel` and `open_backend_tab`, plus the CPU-mode, restart, key, sample, delete-error and choose-backend tests. They move to `backend.rs` and open `BackendView`. `a_capability_row_offers_use_cpu…` and `check_again_and_install_forward…` stay in Dependencies.
- `crates/voice-me-ui/src/settings.rs` -- add `BACKEND_TAB = 2` and move `DEPENDENCIES_TAB` to 3. Add the "Backend" tab, a `backend: Entity<BackendView>` built from `DependenciesTab.backend`/`.actions` (the struct stays, with its doc updated), and a subscription to a `DependenciesView` event that switches tab. `set_backend_panel` updates both views. Update the tab-routing tests at 193-411.
- `crates/voice-me-ui/src/lib.rs` -- re-export the moved types from `backend`, keeping every public name the same so `voice-me-app` compiles unchanged.
- `crates/voice-me-deps/src/capability.rs:312,648` -- Decision 3's wording and its test. Nothing else changes there.
- **Pass 1 code:** `/tmp/claude-1000/-home-erdem-Documents-GitHub-voice-me/31cd560f-24a9-4878-9259-fe47deee52f0/scratchpad/keep-3-10-pass1.patch` (`git apply` it onto the baseline). It met every pass-1 check. Re-apply it, then add Decisions 2 and 3 and the pass-1 patch findings in the Review Triage Log. Those are the tests for a flipped kind surviving an unrelated push and for rebuilding items when runtimes change, a label for the kind `RadioGroup`, and dropping `DependenciesView::new`'s unused `_cx`.
- gpui-kit 0.6.4 provides `radio::RadioGroup` and `button::ButtonGroup` for the kind choice (`~/.cargo/registry/src/index.crates.io-*/gpui-component-0.6.4/src/`).

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-ui/src/backend.rs` (new) -- `BackendView`: the kind choice, a kind-filtered `Select`, the per-kind options, Selected/Active/CPU-mode/restart, and the moved types. Tests: the moved backend tests, plus: remote saved → kind Remote with only that provider's key field; local saved → no key field and runtimes shown; flipping the kind sends no action and shows no options (Decision 1); the Decision 2 rows of the matrix; a flipped kind survives an unrelated panel push; an added runtime appears in the Local `Select`.
- [x] `crates/voice-me-ui/src/dependencies.rs` -- strip the backend sections. Add the **Open Backend tab** button (`backend-open-tab`) on the capability row, emitting an event. Test: the click emits once.
- [x] `crates/voice-me-deps/src/capability.rs` -- Decision 3's wording and its test.
- [x] `crates/voice-me-ui/src/settings.rs` + `lib.rs` -- the new tab, index shifts, the event subscription, and the panel pushed to both views. Tests: the fourth tab routes to Dependencies, the third to Backend, and the capability link switches to Backend.

**Acceptance Criteria:**
- Given Settings is open, when the tabs render, then they read Voice, Hotkey, Backend, Dependencies.
- Given the Dependencies tab, then no backend `Select`, runtime list, key field or sample line appears there.
- Given `cargo test -p voice-me-ui -p voice-me-tests` and `cargo check -p voice-me-app`, then both pass with no change to `voice-me-app`, and `cargo test -p voice-me-deps` passes.

## Implementation Notes

- Pass 2 re-applied `keep-3-10-pass1.patch` on the baseline, then added Decision 2 in `BackendView::options_section`: the Local kind always shows `runtimes_section`; the Remote kind shows the saved provider's key/sample sections (only when that provider is saved) plus `other_provider_line` for every other provider holding a key or a sample. New ids: `backend-other-{slug}`, `backend-other-state-{slug}`, `backend-other-remove-key-{slug}`, `backend-other-delete-sample-{slug}`, `backend-other-error-{slug}` (ApiKey and RemoteSample errors for that provider, joined). Existing ids are unchanged.
- `RadioGroup` in gpui-component 0.6.4 has no accessibility label, so the kind choice gets a visible heading, "Where speech is generated" (`backend-kind-heading`), styled like the second step's label.
- `backend-runtime-{index}` rows gained `.test_support()` so tests can find them; the id is unchanged.
- `DependenciesView::new` lost its unused `cx` parameter; callers are `settings.rs` and the tests only (`voice-me-app` does not call it).
- Decision 3: `capability.rs` wording changed, and its test renamed to `a_remote_provider_with_no_key_points_at_the_backend_tab`.
- Mutation-checked: replacing the `selection_changed` guard with `true` fails `a_flipped_kind_survives_an_unrelated_panel_push`; dropping `items_stale |= runtimes_changed` fails `an_added_runtime_appears_in_the_local_select`.

## Spec Change Log

## Review Triage Log

Pass 1 (blind-hunter, edge-case-hunter, verification-gap). Code parked as `keep-3-10-pass1.patch` in the session scratchpad; the loopback is an intent gap.

| Verdict | Finding | Evidence |
|---|---|---|
| medium | A held DeepInfra sample can be deleted only while DeepInfra is the saved backend (blind) | Confirmed: `options_section` renders `remote_sample_section` only for the saved remote provider. Before this change the line and Delete were always shown. → intent_gap IG1 |
| medium | A saved key can be seen or removed only while its provider is saved (blind) | Confirmed: `api_key_section` is reached only through `BackendSelection::Remote(provider)`. → intent_gap IG1 |
| medium | A remote provider must be saved before its key can be entered (blind) | Confirmed: Decision 1 hides options until a pick, and a pick sends `Select`. Selecting DeepInfra with no key raises the "no API key" blocker first. → intent_gap IG1 |
| medium | **Add runtime…** is reachable only while a Local backend is saved (blind) | Confirmed: `runtimes_section` is shown only when `showing_saved_kind()` and the selection is Local. The spec's own manual check ("pick DeepInfra … then add a runtime") runs into this. → intent_gap IG1 |
| low | Errors and in-flight states of hidden sections (probing, key, sample errors, Deleting…) vanish on a flip (blind, edge) | Confirmed: nothing renders them while the kind is flipped. Same root cause as IG1 → grouped |
| medium | The remote capability row still says "add one under API keys" (verification-gap other) | Confirmed at `voice-me-deps/src/capability.rs:312`, pinned by the test at `:648`, and reaching the overlay's notice through `blocker_notice`. That section no longer exists. The fix edits `voice-me-deps`, which the frozen Never forbids. → intent_gap IG2 |
| medium | A flipped kind is not tested against a later panel push with the same selection (verification-gap) | Pre-verified: dropping the `selection_changed` guard passes the suite. → patch |
| medium | Rebuilding the Local `Select` items after a runtime change is not tested (verification-gap, blind) | Pre-verified: dropping `items_stale \|= runtimes_changed` passes the suite. → patch |
| low | The Local/Remote `RadioGroup` has no heading or accessible name (blind) | Confirmed: the second step has "Local backend"/"Remote backend", the first has nothing. A direct addition → patch |
| low | `DependenciesView::new` keeps an unused `_cx` parameter (blind) | Confirmed; a direct deletion → patch |
| low | A Selection error follows the user across flips, and resyncs the `Select` on every push (blind, edge) | The resync-while-error behaviour is carried over from 3.3 unchanged. Across a flip the resync keeps the other kind's `Select` empty, which is correct. Scoping the error adds state. Rejected |
| low | Unsaved text in a hidden key field survives, or is reset by another provider's save (blind, edge) | The reset-all-inputs-on-any-saved-key-change logic is 3.3's, unchanged. Hiding needs a new reset branch. Rejected |
| low | A file-picker error written into `self.panel.errors` is wiped by the next push (edge) | Pre-existing in 3.3's `add_runtime`, moved verbatim. Rejected |
| maybe-false | A saved selection absent from the choices leaves the `Select` on its placeholder (edge) | Would need the root to keep a selection whose runtime was removed. Even if true it would only be low. Rejected |
| low | Copy repeats "{Provider} API key" in heading and placeholder; tests pick key inputs by index (blind) | Cosmetic; the indexing is 3.3's. Rejected |
| false | No public `show_backend()` (blind) | No caller in this story needs one. The only route in, the capability link, works and is tested. |
| low | `.id("backend-section")` now wraps only the status lines; `error_line` lives in `backend.rs` (blind) | No named harm beyond naming. Rejected |

Pass 2 (blind-hunter, edge-case-hunter, verification-gap), after the Decision 2/3 loopback. No pass-1 row recurred at the same location.

| Verdict | Finding | Evidence |
|---|---|---|
| medium | Deleting… on the other-provider line is never asserted (verification-gap, blind) | Pre-verified: dropping `.disabled(deleting)` passes the suite. → patch |
| medium | The disclosure says the sample can be deleted "in Settings → Dependencies" (verification-gap other) | Confirmed at `prompt_overlay.rs:82`. Deletion now lives only on the Backend tab. → patch |
| low | The fal.ai unavailable reason says "Choose a local backend under Settings → Dependencies." (verification-gap other) | Confirmed at `voice-me-app/src/main.rs:448`. The backend `Select` is on the Backend tab now. A direct copy fix; copy only, root handling unchanged. → patch |
| low | The tts-remote no-key error says "under Settings → Dependencies" (verification-gap other) | Confirmed at `voice-me-tts-remote/src/lib.rs:235`. It is reached only if the key disappears between the check and the Speak Action, because the capability row blocks first. The frozen Never excludes `voice-me-tts-remote`. → defer |
| low | No test for a provider with both a key and a sample; the state text is unasserted (blind) | Confirmed; only one-fact cases are tested. → patch |
| low | The flip test reads the model, not the rendered Selected line (blind) | Confirmed. → patch (if the harness can read text) |
| low | The held-sample test's doc promises a text check it doesn't make (blind) | Confirmed. → patch |
| low | The blocker says "Settings → Backend" but the overlay opens Dependencies (blind) | By design: the matrix keeps `show_dependencies()`, and the capability row there has **Open Backend tab**. One click. Rejected |
| low | Tab labels are untested; only the routing is (blind) | Literal strings, seen on first open. Rejected |
| low | Typing survives an unrelated push, and the Add runtime/probing/Remove flows, are untested (blind) | 3.3 behaviour moved verbatim, not changed here. Rejected |
| low | A Restart error renders only while `restart_pending` (blind) | Pre-existing in 3.3's `backend_section`. Rejected |
| false | Dropping the panel push to Dependencies would go unnoticed (blind) | `a_backend_panel_reaches_both_tabs` asserts the capability error, which comes from that push. |
| low | `backend-remote-samples` lacks `.test_support()` (blind) | No test needs it. Rejected |
| low | **Open Backend tab** neither focuses nor scrolls to the key field (blind) | That adds behaviour beyond the intent. Rejected |
| low | The kind `RadioGroup` has no accessible name (blind) | gpui-component 0.6.4's `RadioGroup` has no label API. The visible heading is the available fix and is in place. Rejected |
| low | `DependenciesTab` now also builds the Backend tab (blind) | A rename touches `voice-me-app` for no user harm. Rejected |
| low | Another provider's **Remove key** resets the saved provider's unsaved typed key (edge) | The reset-all-on-any-key-change logic is 3.3's, unchanged. The fix adds per-provider diffing. Rejected |
| low | A flipped kind persists, so **Open Backend tab** can land on the other kind (edge) | Real, but it needs a flip, a tab switch, then the link. The fix adds a reset path. Rejected |
| false | fal.ai holding a sample while saved is shown nowhere (edge) | fal.ai cannot hold a sample before Story 3.7 (`SAMPLE_HOLDING_PROVIDERS`), so the state is unreachable. |
| false | An error for a provider with neither key nor sample is never shown (edge) | A failed Remove leaves the key and a failed Delete leaves the sample, so the line still renders. Key saves happen only for the saved provider, which has its own section. |

## Verification

**Commands:**
- `cargo test -p voice-me-ui -p voice-me-tests -p voice-me-deps` -- expected: all pass
- `cargo check -p voice-me-app && cargo clippy -p voice-me-ui --all-targets` -- expected: clean
- `cargo fmt --check -p voice-me-ui` -- expected: no diff in changed files

**Manual checks:**
- Run the app, open Settings → Backend, and flip Local/Remote. Pick DeepInfra and save a key, then add a runtime. Confirm that Dependencies shows only its rows.
