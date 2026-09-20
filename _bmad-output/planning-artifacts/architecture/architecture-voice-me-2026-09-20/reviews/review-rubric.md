---
title: Good-Spine Checklist Review — voice-me Architecture Spine
reviewed: architecture-voice-me-2026-09-20/ARCHITECTURE-SPINE.md
against: prds/prd-voice-me-2026-09-20/prd.md
date: 2026-09-20
calibration: hobby/solo-project build-substrate — rigor expectations set accordingly, not platform/enterprise
---

# Review: voice-me Architecture Spine vs. Good-Spine Checklist

## Overall Verdict: ADEQUATE

This is a solid first-draft spine for a solo hobby project — the hexagonal paradigm is coherent, all nine ADs are individually well-reasoned, every PRD FR is mapped to owning crates/ADs, and the Deferred section shows real judgment about what genuinely doesn't need deciding yet (tray UX, auto-update, exact TOML schema). It correctly resists over-engineering for a one-developer, single-machine app.

It falls short of STRONG on two concrete points that matter at this altitude: (1) the port-trait taxonomy that AD-1/AD-2 depend on is incomplete/inconsistent (TrayPort and an implicit deps-adapter port aren't listed where the hexagon's ports are enumerated), and (2) the single most central data flow in the whole app — generated audio moving from the TTS adapter to the platform audio adapter — has no settled contract, which is exactly the kind of cross-crate divergence point this document exists to close. Neither is fatal; both are cheap to fix before crate scaffolding starts, which is why this lands at ADEQUATE rather than THIN.

## Dimension Verdicts

| Dimension | Verdict | Notes |
| --- | --- | --- |
| Fixes real divergence points for the level below, misses none | **THIN** | Misses two real ones: the port-trait list is incomplete (Finding 1), and the TTS→VirtualMic audio hand-off has no data contract (Finding 2). |
| Every AD's Rule is enforceable and prevents its stated divergence | **ADEQUATE** | Eight of ten ADs are cleanly enforceable by structure/code review. AD-5's Deferred bridging choice undermines its own enforceability (Finding 3); AD-8's "inviolable" network rule has no enforcement mechanism named (Finding 6). |
| Nothing under Deferred could let two units diverge | **THIN** | The AD-5 bridging-approach item is deferred to "whoever implements AD-5," but AD-5 binds two separate adapter crates (`voice-me-tts`, `voice-me-deps`) — left as-is, each could pick a different bridging approach, recreating the divergence AD-5 was written to prevent (Finding 3). |
| Named tech is verified-current (stated as verified/sourced, not vague) | **THIN** | Only the GPUI row and AD-5's `[ADOPTED]` note carry any verification framing. `thiserror 2.0.20`, `anyhow 1.0.103`, `gpui-kit 0.6.4`, `Rust 1.98.1, edition 2024` are stated as flat fact with no indication they were checked at authoring time (Finding 5). |
| Covers the driving PRD's capabilities | **STRONG** | All of FR-1..FR-9 appear in the Capability→Architecture Map with an owning crate and governing AD(s). Five of six PRD Open Questions (OQ2, OQ3, OQ6 directly; OQ5 indirectly via AD-7's download-not-bundle approach) are picked up in Deferred. OQ1 (latency) and OQ4 (clip-length bounds) are correctly left out as product/measurement questions, not architecture. |
| Every owned dimension is decided/deferred/open — esp. operational/environmental envelope | **ADEQUATE** | Deployment & environments, CI, and infra/provider strategy (GitHub-only) are explicitly stated and appropriately light for hobby scope — this is the dimension the prompt most wanted checked, and it's genuinely present. Two adjacent infra risks are left silent rather than flagged: GitHub Releases' asset-size/hosting feasibility for multi-GB model weights, and the model-weights redistribution-legality risk being hedged inline instead of tracked (Findings 4, 7). |

## Findings

### Finding 1 — Port-trait taxonomy is incomplete and internally inconsistent
- **Severity:** High
- **Location:** Design Paradigm section (port trait list) vs. AD-2 vs. AD-9; Structural Seed (`voice-me-tray-linux`, `voice-me-tray-windows`, `voice-me-deps`)
- **What it means:** The Design Paradigm intro enumerates the port traits as `HotkeyPort, VirtualMicPort, TtsPort, SettingsStore`. AD-2 then requires a `TrayPort` for the tray adapters that isn't in that list. Separately, `voice-me-deps` is a driven adapter under AD-1's blanket rule ("every adapter crate depends on `voice-me-core` for its port trait") but no port trait is ever named for it (no `DependencyPort` or equivalent), even though AD-9 describes it reporting GPU-capability results back to core. Because the spine is the thing meant to keep independently-built crates consistent, an incomplete port list is a real divergence risk: whoever implements the tray or deps adapters has to invent the missing trait's shape themselves, with no canonical definition to converge on.

### Finding 2 — No data contract for the TTS→VirtualMic audio hand-off
- **Severity:** High
- **Location:** AD-3 (cross-adapter communication rule); Capability→Architecture Map rows for FR-5/FR-6
- **What it means:** AD-3 says all cross-adapter effects flow through `AppState`/`AppEvent`, but never specifies the shape of the one payload that matters most: the generated audio itself, produced by `voice-me-tts` and consumed by `voice-me-audio-{linux,windows}`. Format (WAV bytes vs. raw PCM vs. temp-file path), sample rate/bit depth/channel count, and whether it truly travels as an `AppEvent` variant (audio bytes inside an enum sent over a channel) or some other core-owned handoff are all undecided and unmentioned even in Deferred. This is the single most central data flow in the app (FR-5 → FR-6, the whole point of the product), and it's exactly the kind of contract this altitude of document should pin down or explicitly defer — right now it's simply silent.

### Finding 3 — AD-5's Deferred bridging decision can itself cause the divergence AD-5 prevents
- **Severity:** Medium
- **Location:** AD-5 Rule; Deferred, last bullet ("Whether `voice-me` depends directly on Zed's `gpui_tokio` crate... — a call for whoever implements AD-5")
- **What it means:** AD-5's stated purpose is to stop "each async-needing adapter picking a different... runtime" bridging approach. But AD-5 binds two separate crates (`voice-me-tts` for FR-5, `voice-me-deps` for FR-7), and the Deferred item hands the gpui_tokio-vs-in-house choice to "whoever implements AD-5" as if there's one implementer and one decision point. If `voice-me-tts` and `voice-me-deps` are built at different times (plausible solo-dev sequencing) without this being pinned as a single workspace-level decision, they could land on different bridging strategies — the exact outcome AD-5 exists to prevent. Fix: state explicitly that this choice is made once, centrally (e.g., in `voice-me-app`'s composition root or recorded in `voice-me-core`), not independently per adapter crate.

### Finding 4 — GitHub Releases hosting feasibility for model weights is unexamined
- **Severity:** Medium
- **Location:** AD-7 Rule; "Deployment & environments" paragraph
- **What it means:** AD-7 commits to mirroring every remotely-fetched runtime asset — including Chatterbox model weights — on this repo's own GitHub Releases as "one trusted, versioned source." GitHub Releases assets carry real per-file and practical hosting limits, and voice-cloning TTS model weights are often multi-gigabyte. The spine states the hosting strategy as settled without checking it against this constraint. If the weights don't fit, AD-7's entire "first-party hosting" premise needs a fallback (e.g., Git LFS, a different release host) that isn't mentioned anywhere, including Deferred.

### Finding 5 — Precise dependency versions stated without verification framing
- **Severity:** Low
- **Location:** Stack table (`thiserror 2.0.20`, `anyhow 1.0.103`, `gpui-kit 0.6.4`, `Rust 1.98.1, edition 2024`)
- **What it means:** The checklist asks whether named tech is stated as verified/sourced rather than vague. Most rows in the Stack table give exact patch versions as flat fact, with no "verified as of [date]" or sourcing note — contrast this with the GPUI row (explains *why* crates.io `0.2.2` is rejected) and AD-5 (`[ADOPTED] — confirmed as Zed's own pattern`), which do carry that framing. Rows that instead say "latest stable — pin at implementation time" (tokio, tracing, directories) are honest about not having verified a specific version; the exact-pinned rows give no comparable signal that they were checked rather than recalled.

### Finding 6 — AD-8's "inviolable" network-egress rule has no enforcement mechanism
- **Severity:** Low
- **Location:** AD-8 Rule
- **What it means:** AD-8 is framed in the strongest terms in the whole document ("Adding any other network call is an architectural change, not a local one"), but the Rule only states the constraint — it names no mechanism (CI check, `cargo-deny` ban list on HTTP-client crates in non-`voice-me-deps` crates, etc.) to actually catch a violation. For a solo developer, self-discipline plus code review may be enough in practice, but the rule's own severity of framing implies it deserves more than a purely social enforcement path, especially since a violation (e.g., an ad-hoc telemetry call) is precisely the silent kind of drift a solo dev might not notice in their own code.

### Finding 7 — Model-weight redistribution legality is hedged inline instead of tracked
- **Severity:** Low
- **Location:** AD-7 Rule ("Chatterbox model weights if redistributable")
- **What it means:** AD-7's whole hosting strategy assumes the Chatterbox-Multilingual V3 weights can legally be re-hosted on this project's own GitHub Releases. That assumption is flagged with a bare "if redistributable" inside the Rule's prose rather than being carried as an explicit Open Question or Deferred item alongside the PRD's own already-tracked risks (e.g., OQ6's GPUI tray/hotkey risk gets a full Deferred entry with a resolution path; this comparable foundational risk gets a four-word hedge). If the weights turn out not to be redistributable, AD-7 has no stated fallback (e.g., have the app download weights directly from the model's own origin instead of mirroring).

## Summary by Severity

- High: 2 (Findings 1, 2)
- Medium: 2 (Findings 3, 4)
- Low: 3 (Findings 5, 6, 7)
- Total: 7
