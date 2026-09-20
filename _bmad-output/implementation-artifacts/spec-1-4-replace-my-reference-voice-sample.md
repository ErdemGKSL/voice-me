---
title: 'Replace My Reference Voice Sample'
type: 'feature'
created: '2026-09-20'
status: 'done'
route: 'oneshot'
review_loop_iteration: 0
baseline_commit: 'b05c372976ca5c139f2fb6e20e9814a1f8dd925d'
context:
  - '{project-root}/_bmad-output/planning-artifacts/architecture/architecture-voice-me-2026-09-20/ARCHITECTURE-SPINE.md'
  - '{project-root}/_bmad-output/implementation-artifacts/epic-1-context.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Story 1.4's AC (an active Reference Voice Sample can be re-recorded or re-imported from Settings → Voice, replacing the old one on Accept, repeatable without restarting) is already implemented — Story 1.2 built replace-on-Accept for recording and covers it with a full UI-flow test (`re_recording_after_accept_replaces_the_previously_saved_clip`), and Story 1.3 built the equivalent for import with a store-level replace test (`re_recording_replaces_the_previously_active_sample`) — but no test drives the full UI click flow for the *import* side of replace after an existing accepted sample, so that half of the AC is unverified at the level the other stories were held to.

**Approach:** Add one `voice-me-ui` UI-integration test mirroring the existing `re_recording_after_accept_replaces_the_previously_saved_clip` pattern, but with the second cycle going through Import instead of Record, confirming Accept replaces the first (recorded) clip with the second (imported) one and that both flows can alternate without restarting the app. No production code changes are expected; if the test surfaces a real defect, fix it minimally in `voice_setup.rs`.

</frozen-after-approval>

## Implementation Notes

- Confirmed by reading `crates/voice-me-core/src/settings_store.rs` and `crates/voice-me-ui/src/voice_setup.rs` that replace-on-Accept was already fully implemented (fixed-filename overwrite in `FileSettingsStore::save_reference_voice_sample`, no production code change needed).
- Added `re_importing_after_accept_replaces_the_previously_saved_clip` in `crates/voice-me-ui/src/voice_setup.rs`, mirroring the existing `re_recording_after_accept_replaces_the_previously_saved_clip` test: record+Accept, then Import+Accept, asserting both clips landed in the fake store (2 saved clips) and that Accept re-enables after the first Accept. This is the one AC-relevant path (re-import after an existing active sample, driven through the real click flow) that no existing test covered — re-record-after-accept was already covered by Story 1.2, and import-replace was only covered at the `FileSettingsStore` unit level by Story 1.3.
- No `## Code Map` / `## Tasks & Acceptance` sections were needed (oneshot route) — the whole change is this one test addition.
- Review pass (see Review Triage Log): added `baseline_commit`/`context` frontmatter fields to match sibling specs' convention; strengthened the new test with an `assert_ne!` on the two saved clips' bytes, so it actually proves the second Accept saved the newly imported clip rather than merely firing twice.

**Verification performed:**
- `cargo test --workspace` — exits 0, 16 tests in `voice-me-ui` (was 15), including the new test.
- `cargo clippy --workspace --all-targets` — clean except the pre-existing unrelated `voice-me-tests::placeholder` warning.
- `cargo fmt --check` — clean.

## Spec Change Log

## Review Triage Log

- **patch** — the new test only asserted `saved_clips.len() == 2`, which would still pass if the second Accept re-saved a stale copy of the first (recorded) clip instead of the newly imported one, since `FakeSettingsStore` just pushes onto a `Vec`. Fix: added `assert_ne!(saved_clips[0], saved_clips[1], ...)` to prove the second saved payload actually differs. (blind-hunter)
- **patch** — the new spec's frontmatter omitted `baseline_commit` and left `context: []` empty, while both sibling specs (1-2, 1-3) record a `baseline_commit` and populate `context` with the architecture spine and epic context doc. Fix: added both fields to match. (blind-hunter)
- **false** — flagged that `sprint-status.yaml` and the spec's own `status` field were left at `in-progress` despite the work being verified-complete. This is expected at review time: the workflow's Finalize Spec step (which runs after review triage) is what sets `status: done` and syncs `sprint-status.yaml` to `review` — both happen immediately after this log entry, in the same run. (blind-hunter)
- **false** — flagged that the new spec file was untracked/not yet committed alongside the code and `sprint-status.yaml` changes. The workflow's Commit step runs after Finalize Spec and stages/commits all of them together in one commit — nothing was meant to be committed yet at review time. (blind-hunter)
