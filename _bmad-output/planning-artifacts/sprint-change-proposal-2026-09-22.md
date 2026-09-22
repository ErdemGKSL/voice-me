---
title: "Sprint Change Proposal — Runtime-Selectable Backends and Remote Generation"
status: proposed
created: 2026-09-22
trigger_story: "3.1 Detect Missing Dependencies (planning)"
scope: major
---

# Sprint Change Proposal — Runtime-Selectable Backends and Remote Generation

## 1. Issue Summary

**What triggered it.** During `bmad-build` planning for Story 3.1 (Detect Missing Dependencies) on 2026-09-22, the spec asked how the build variant should be expressed in code. The product owner answered by replacing the model: all local backends are built together as optional Rust adapter crates and selected from the UI, and a remote-API backend (DeepInfra `ensembleAI/chatterbox-multilingual` as the default, fal.ai second) ships alongside them.

**Issue type:** strategic pivot — a product-owner direction change, not a technical failure or a misread requirement.

**Why it could not be absorbed into the story.** Story 3.1's every acceptance criterion derives the required-dependency list from "the running build's variant". That variant is created by AD-7, bounded by AD-9, and the remote backend it now sits beside is explicitly forbidden by AD-8 pending PRD Open Question 7. Writing the spec would have meant implementing against three architecture decisions the direction had just reversed.

**Evidence.**
- `ARCHITECTURE-SPINE.md` AD-7: "v1 does not ship one executable that discovers its backend at run time — it ships a matrix of backend variants and the user picks one when downloading" (2026-09-21).
- AD-8: network egress confined to `voice-me-deps`; the remote-generation exception recorded as "the one declared exception, and it is not yet granted", blocked on PRD Open Question 7.
- AD-9: the build variant as the first of three inputs and "the outer bound … a compile-time constant".
- `epics.md` Epic 3 scope note and Stories 3.1, 3.2, 3.3, 3.5 — all variant-aware in their acceptance criteria.
- Code state: nothing implements the variant model. `resolved_speech_backend()` returns a hardcoded `SpeechBackend::CPU`, `DepsAdapter::check` is `todo!()`, `AppEvent::DependencyCheckCompleted` falls into a wildcard arm. No rollback is required.
- Technical constraint discovered while assessing the change: execution providers are ONNX Runtime **C** libraries and `ort` commits exactly one `libonnxruntime` per process, so "all backends in one binary" is true of the Rust adapters but requires a deliberate decision about the runtime underneath them.

## 2. Impact Analysis

### Epic impact

| Epic | Impact |
| --- | --- |
| Epic 1 — Voice Setup | None. All stories `review`/`done`. |
| Epic 2 — Core loop | None. `TtsPort` was designed to absorb a second adapter; Story 2.5's measurements stand. Story 2.8 (Windows virtual mic) is untouched. |
| **Epic 3 — Never Get Stuck on Setup** | **Rewritten.** Scope note replaced; Stories 3.1, 3.2, 3.3 reworded backend-aware; 3.4 rekeyed to the selected backend; old 3.5 renumbered 3.9; four new stories added (backend selection, DeepInfra, fal.ai, the CI runtime build). |
| Epic 4 — Localization | None. |

### Story impact

No story is deleted and none is rolled back. Epic 3 grows from five stories to nine.

### Artifact conflicts

- **PRD** — §1 Vision, §2.2 Non-Users, §5 Non-Goals, §6.1/§6.2 MVP scope, FR-5, FR-7, Open Question 7, §9 Assumptions Index. New FR-10.
- **Architecture** — AD-7 (rewritten), AD-8 (rewritten), AD-9 (rewritten), new AD-13, Stack table, Capability → Architecture Map.
- **UX** — `EXPERIENCE.md` Component Patterns and State Patterns tables; `DESIGN.md` component list.
- **Other** — CI (`cargo-deny`/grep allowlist re-scoped to two crates; a new from-source ONNX Runtime build job), `README.md` provisioning section, `sprint-status.yaml`.

### Technical impact

- New crates: `voice-me-tts-remote`; `voice-me-tts` renamed `voice-me-tts-onnx`.
- New workspace dependencies, confined by AD-8: `reqwest` (rustls), `serde_json`.
- `AppState` gains a persisted backend selection; `SpeechExecutionTarget` gains CUDA and a remote-provider case.
- Standing CI responsibility: an ONNX Runtime built from source with CUDA and WebGPU providers, mirrored per AD-7 — the heaviest infrastructure item in this change.

## 3. Recommended Approach

**Hybrid: Direct Adjustment of all planning artifacts, plus an explicit MVP scope revision.**

- *Direct Adjustment* is viable because nothing is built yet on the model being replaced — this is a documentation change followed by an unchanged implementation cadence.
- *Rollback* is not applicable: no shipped code depends on the variant axis.
- *MVP review* is genuinely required, because remote generation is added to v1 rather than reshuffled within it.

**Effort:** medium-high on planning artifacts, low risk to existing code, high effort on the new CI runtime build. **Timeline:** Epic 3 roughly doubles in size (five stories to nine, including one infrastructure story).

**Risks recorded, accepted by the product owner:**
1. **The local-only promise is dropped as a headline claim.** It was the brief's differentiator. Mitigated by per-backend documentation and a local default.
2. **The Reference Voice Sample leaves the machine** on the remote path and is cached on provider infrastructure by id. Mitigated by one-time disclosure before the first request and a delete path.
3. **The API key is stored in plaintext** in the settings TOML. Mitigated by a notice at the point of entry and log redaction; the OS keyring remains a later port-level change.
4. **Two providers in v1** doubles the remote-path story count against one provider's worth of user-visible value.

## 4. Detailed Change Proposals

All eight proposals below were approved individually on 2026-09-22.

### 4.1 PRD

**P1 — the local-only claim.** §1 Vision rewritten to "local by default, remote if you pick it, always visible which". §2.2 Non-Users: cloud/hybrid inference removed as an exclusion, multi-device sync retained. §5 Non-Goals rewritten: no voice-me server, no voice-me account, no telemetry ever; local-only dropped as a headline claim as of 2026-09-22, with per-backend behavior documented in FR-10. §6.2's cloud-inference exclusion deleted.

**P2 — FR-5, FR-7, new FR-10.** FR-5 targets "the selected speech backend" and drops the superseded Sidecar Process reference; its failure consequence names which backend failed and why. FR-7 becomes backend-aware: per-backend asset lists, only the selected backend's gaps block a Speak Action, remote readiness is a key plus reachability, and the app reports what was acquired rather than what was requested. New FR-10 (§4.6) covers backend selection, the local default, one-time disclosure before the first byte leaves, upload-once-reference-by-id sample handling with a delete path, the plaintext-key notice, and no-restart switching.

**P3 — Open Question 7 and MVP scope.** OQ7 answered 2026-09-22: remote is a selected backend in v1; text and Reference Voice Sample are sent, the sample cached remotely by id; disclosure confirmed once per provider; local-only rewritten rather than narrowed; providers DeepInfra (default) and fal.ai. §6.1 gains backend selection and remote generation; §6.2 gains stock-voice-only providers and any voice-me-hosted proxy or shared keys as exclusions. §9's stale CPU-fallback assumption replaced with AD-9's no-silent-substitution rule.

### 4.2 Architecture

**P4 — AD-7** retitled "one artefact per OS carrying every backend". Ships one executable per OS with every backend compiled in; Cargo features exclude backends only from development builds, never define a shipped product; variant names leave release artefact names; the asset table becomes per backend. Adds: CI builds ONNX Runtime from source with `--use_cuda --use_webgpu --build_shared_lib` and mirrors that single distribution, so switching local backends is a session rebuild rather than a process restart, and `webgpu-probe` stops being an AD-8 violation.

**P5 — AD-8** retitled "Network egress is confined to two named crates, and nothing else". Exactly `voice-me-deps` and `voice-me-tts-remote` may open a socket; the CI ban-list becomes an allowlist of those two manifests and fails on a third; what the remote adapter may send is bounded by FR-10 (text, language tag, Reference Voice Sample) and excludes telemetry and machine information; provider-side retention is disclosed, not managed. The 2026-09-21 "not yet granted" paragraph is superseded by PRD OQ7.

**P6 — AD-9** retitled "The active backend is a user choice and a detected capability, resolved in core". Two inputs: the persisted user selection (backend, and device for GPU backends; unset means the CPU backend) and detected capability from `voice-me-deps` via `AppEvent` only. The compile-time outer bound is replaced by what the provisioned runtime actually carries. The truth-over-silent-fallback rule is retained and restated for selection, with Story 2.5's `provider: webgpu:0` case kept as the cautionary precedent.

**P7 — new AD-13**, "The remote backend is one port, many providers, with credentials and disclosure owned by core": one `TtsPort` implementation with a `SpeechProvider` trait per vendor; sample uploaded once per provider and referenced by id, keyed by (provider, sample hash); plaintext key storage behind `SettingsStore` with its consequence stated and log redaction; disclosure enforced by the core use-case rather than the adapter; bounded request deadlines and no automatic re-send of audio data. Stack table gains `reqwest`, `serde_json`, DeepInfra and fal.ai, and the ONNX Runtime rows point at the single CI-built distribution. Capability Map updates FR-5 and FR-7 and adds an FR-10 row. `voice-me-tts` is renamed `voice-me-tts-onnx`.

### 4.3 Epics

**P8 — Epic 3.** Scope note replaced with the backend-aware version. Rewrites: 3.1 (per-backend required lists, selected backend stated, GPU candidates when a GPU backend is selected, remote readiness as key presence), 3.2 (provisions only the selected backend's assets; resumable-download and manual-steps clauses retained verbatim), 3.3 retitled "Be Honest When the Selected Backend Can't Run Here", 3.4 rekeyed to the selected backend's blocker, and old 3.5 renumbered **3.9** with its variant framing removed.

New stories:
- **3.5 Choose Which Backend Generates Speech** — selector in Settings, persisted, effective on the next Speak Action with no restart.
- **3.6 Generate Through a Remote Provider (DeepInfra)** — the adapter behind `TtsPort`, key entry with its plaintext notice, the one-time disclosure gate, upload-once-reference-by-id with a delete path.
- **3.7 Add fal.ai as a Second Provider** — exercises the `SpeechProvider` abstraction with no change above `TtsPort`.
- **3.8 Build and Mirror the All-Provider ONNX Runtime** — the from-source CI build AD-7 now depends on; blocks the GPU backends, not the CPU one.

### 4.4 UX

`EXPERIENCE.md`: the "Build variant line" row becomes a **Backend selector** showing what was actually acquired; the GPU device selector is gated on a selected GPU backend rather than a GPU build; new rows for a masked **API key field**, a **first-use disclosure dialog**, and **remote sample state with a delete action**. State rows: "Running the `cpu` variant" → "CPU backend selected"; "GPU variant, no usable device" → "Selected backend unavailable here"; new "Remote backend selected, no key". `DESIGN.md` adds masked `Input` to the as-is component list.

### 4.5 Sprint status

Epic 3's five keys are replaced by the nine above, all `backlog`. Epics 1, 2 and 4 are untouched.

## 5. Implementation Handoff

**Scope classification: Major** — three architecture decisions rewritten, one added, a PRD open question answered, MVP scope grown, and an epic restructured.

| Recipient | Responsibility |
| --- | --- |
| Product Manager (`bmad-agent-pm`) | Owns the PRD edits in §4.1 — the promise rewrite, FR-5/FR-7/FR-10, OQ7, MVP scope. |
| Solution Architect (`bmad-agent-architect`) | Owns AD-7, AD-8, AD-9, AD-13, Stack and Capability Map in §4.2, and the CI allowlist re-scope. |
| Product Owner / Developer | Owns the Epic 3 rewrite and `sprint-status.yaml` in §4.3 and §4.5, then re-plans Story 3.1 with `bmad-build` against the corrected documents. |
| UX (`bmad-agent-ux-designer`) | Owns the `EXPERIENCE.md` / `DESIGN.md` updates in §4.4. |

**Success criteria.**
1. No planning artifact still derives behavior from a download-time build variant.
2. AD-8 names exactly two network-capable crates and the CI check enforces that list.
3. PRD Open Question 7 is answered, and no document still claims local-only without qualification.
4. Epic 3's nine stories are in `sprint-status.yaml` as `backlog`, and Story 3.1 re-plans cleanly with no reference to a variant.
5. Epics 1, 2 and 4, and all shipped code, remain untouched by this change.
