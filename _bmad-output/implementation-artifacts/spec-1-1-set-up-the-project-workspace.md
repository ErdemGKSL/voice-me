---
title: 'Set Up the Project Workspace'
type: 'chore'
created: '2026-09-20'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
baseline_commit: '4dcb55a5b63a2da43a82ac02fa9dd98b77fe5b46'
context:
  - '{project-root}/_bmad-output/planning-artifacts/architecture/architecture-voice-me-2026-09-20/ARCHITECTURE-SPINE.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** The repository has no Rust code yet — every later story (Voice Setup, the Core Loop, Dependency Management, Localization) needs a workspace and crate structure to add code into, and none exists.

**Approach:** Scaffold a Cargo workspace with the crate layout fixed by the Architecture Spine's Structural Seed: one bin crate as composition root, one hub lib crate holding domain types and port traits, and empty stub lib crates for every adapter named in the spine (UI, per-OS hotkey/audio/tray, TTS, dependency provisioning, i18n), plus a black-box test crate. Add CI that builds the workspace on both target platforms.

## Boundaries & Constraints

**Always:**
- Crate set, names, and roles match the Architecture Spine's Structural Seed exactly (AD-2 for the per-OS split).
- `voice-me-core` depends on no other `voice-me-*` crate (AD-1); every adapter crate depends only on `voice-me-core`, never on a sibling adapter.
- `voice-me-core` declares the port traits (`HotkeyPort`, `VirtualMicPort`, `TrayPort`, `TtsPort`, `DependencyProvisioningPort`, `SettingsStore`) and the domain types `AppState`, `AppEvent`, and a `thiserror`-based `VoiceMeError` enum (AD-4), each with the minimal shape needed to compile — later stories extend them, this story does not implement behavior.
- `cargo build --workspace` succeeds on the current toolchain.
- GitHub Actions CI runs `cargo build --workspace` in two separate jobs (`ubuntu-latest`, `windows-latest`), no macOS job (AD-7).

**Never:**
- No adapter crate contains business logic beyond a compiling stub — implementations land in later stories/epics.
- No network call anywhere in this story.
- Do not add Tokio, gpui-kit, or any other real dependency to a crate before a story actually needs it — stub crates start with no dependencies beyond `voice-me-core` where applicable.

</frozen-after-approval>

## Code Map

- `Cargo.toml` (new) -- workspace root, `[workspace] members = [...]`
- `crates/voice-me-app/` (new) -- bin crate, composition root; `main.rs` just `fn main() {}` for now
- `crates/voice-me-core/` (new) -- lib crate; `src/lib.rs` re-exports `state.rs` (`AppState`), `event.rs` (`AppEvent`), `error.rs` (`VoiceMeError`, `thiserror`), `ports.rs` (the six port traits)
- `crates/voice-me-ui/`, `crates/voice-me-i18n/` (new) -- empty lib crates depending on `voice-me-core`
- `crates/voice-me-hotkey-linux/`, `crates/voice-me-hotkey-windows/`, `crates/voice-me-audio-linux/`, `crates/voice-me-audio-windows/`, `crates/voice-me-tray-linux/`, `crates/voice-me-tray-windows/`, `crates/voice-me-tts/`, `crates/voice-me-deps/` (new) -- empty lib crates depending on `voice-me-core`, each with one stub `impl` of its port left as `todo!()`
- `crates/voice-me-tests/` (new) -- lib crate depending on `voice-me-core`; one placeholder `#[test]` that asserts `true` so the crate is exercised by CI
- `.github/workflows/ci.yml` (new) -- two jobs, `cargo build --workspace` on `ubuntu-latest` and `windows-latest`

Repository currently has no `Cargo.toml`, no `crates/`, no `.github/` — this is a from-scratch scaffold. Local toolchain is `rustc 1.96.0` / `cargo 1.96.0`; edition 2024 has been supported since 1.85, so this is fine even though the Architecture Spine's Stack table names 1.98.1 as "current at authoring" — build against whatever toolchain is actually installed, don't block on the exact patch version.

## Tasks & Acceptance

**Execution:**
- [x] `Cargo.toml` -- create workspace manifest listing all 13 member crates -- fixes the dependency graph before any crate has content
- [x] `crates/voice-me-core/src/{lib,state,event,error,ports}.rs` -- define `AppState` (fields: `hotkey: Option<String>`, `reference_voice_sample: Option<std::path::PathBuf>`, `ui_language: String`, `selected_mic_device: Option<String>`), `AppEvent` (a small `#[non_exhaustive]` enum with at least `HotkeyPressed` and `DependencyCheckCompleted`), `VoiceMeError` (`thiserror`, at least `Io` and `Other(String)` variants), and the six port traits each with one representative method returning `Result<_, VoiceMeError>` -- gives every later crate something real to depend on
- [x] `crates/voice-me-hotkey-{linux,windows}/src/lib.rs`, `crates/voice-me-audio-{linux,windows}/src/lib.rs`, `crates/voice-me-tray-{linux,windows}/src/lib.rs`, `crates/voice-me-tts/src/lib.rs`, `crates/voice-me-deps/src/lib.rs` -- one struct per crate implementing its port trait with `todo!()` bodies -- proves the dependency direction compiles per AD-1/AD-2
- [x] `crates/voice-me-ui/src/lib.rs`, `crates/voice-me-i18n/src/lib.rs` -- empty lib crates depending on `voice-me-core`, no content yet
- [x] `crates/voice-me-app/src/main.rs` -- composition root that constructs nothing yet -- later epics wire real adapters in here
- [x] `crates/voice-me-tests/src/lib.rs` -- one placeholder test -- gives CI something to run against the workspace
- [x] `.github/workflows/ci.yml` -- two-job matrix, `cargo build --workspace` per job

**Acceptance Criteria:**
- Given the scaffolded workspace, when `cargo build --workspace` runs, then it succeeds with no errors
- Given the scaffolded workspace, when `cargo test --workspace` runs, then the placeholder test in `voice-me-tests` passes
- Given a fresh clone on either target OS, when GitHub Actions runs, then both the `ubuntu-latest` and `windows-latest` jobs pass

## Implementation Notes

## Spec Change Log

## Review Triage Log

- **false** — CI never runs `cargo test --workspace` (only `cargo build --workspace` in both jobs). The frozen Boundaries & Constraints explicitly specify "GitHub Actions CI runs `cargo build --workspace` in two separate jobs ... (AD-7)" with no mention of a test step — intent itself excludes this, so no gap exists. (blind-hunter, verification-gap — same claim)
- **false** — Placeholder test lives in `crates/voice-me-tests/src/lib.rs` rather than under `tests/`, so it isn't a strict black-box integration test. The spec's own Tasks line directs exactly this file: "`crates/voice-me-tests/src/lib.rs` -- one placeholder test." Spec-directed, not a defect. (blind-hunter)
- **false** — `voice-me-tests` depends on `voice-me-core` but the placeholder test doesn't reference it. The Code Map explicitly specifies this crate "depending on `voice-me-core`" ahead of later real tests; unused-but-declared path deps don't warn or break the build. Spec-directed. (blind-hunter)
- **defer** — Per-OS adapter crates (`voice-me-hotkey-{linux,windows}` etc.) have no `[target.'cfg(...)']` gating, so `cargo build --workspace` on either CI runner compiles both platforms' stub crates; this only works today because the stubs are OS-agnostic `todo!()` bodies. No bad outcome yet — verified `cargo build --workspace` succeeds on the current toolchain — but a later story adding a real Windows-only or Linux-only dependency to one of these crates would break the opposite-OS CI job. Real if realized, but not yet, and resolving it (target-specific manifests or a CI build matrix keyed to crate) is bigger than a direct correction — beyond this chore story's scope ("later stories extend them, this story does not implement behavior"). (blind-hunter)
- **false** — Spec status vs. `sprint-status.yaml` momentarily disagreeing (`in-review` vs `in-progress`) is workflow bookkeeping mid-run, not a code defect; both are reconciled by the Build workflow's final step. (blind-hunter)
- **false** — `Implementation Notes` / `Spec Change Log` sections being empty is expected: nothing to log yet, no deviation from spec occurred. (blind-hunter)
- **false** — `thiserror = "2.0.20"` is not an exact pin; Cargo's default bare-version syntax is a caret requirement (`^2.0.20`, allows `<3.0.0`), so patch/minor updates are already permitted. Claim about missing `[workspace.dependencies]` is a style preference with no named harm at this single-dependency stage. (blind-hunter)
- **low** — `.gitignore` covers `/target` but not common editor/OS artifacts (`.vscode/`, `.idea/`, `.DS_Store`). Real minor gap, fix is a direct addition with no public surface — routes to patch.
- **false** — `Cargo.toml` workspace member ordering is "ad hoc" rather than alphabetical/Code-Map order. No named harm to any caller or process; vague messiness claim, not a severity grade. (blind-hunter)

## Verification

**Commands:**
- `cargo build --workspace` -- expected: exits 0, no warnings about missing crates
- `cargo test --workspace` -- expected: 1 passed test (the `voice-me-tests` placeholder)
