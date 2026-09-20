# Reconciliation Check — UX Spines vs SPEC/glossary/PRD

**Scope:** DESIGN.md + EXPERIENCE.md (ux-voice-me-2026-09-20) against SPEC.md, glossary.md, and PRD UX-relevant detail (FR-2, FR-3, FR-7, FR-8).

**Verdict:** Substantially consistent — no non-goal violations, no invented glossary synonyms, no constraint contradictions found. Two real gaps and one worth-flagging tension.

## CAP-1..9 coverage

| CAP | Covered? | Where |
|---|---|---|
| CAP-1 Voice Setup | Yes | "Voice recorder" component pattern; Flow 2 |
| CAP-2 Background/tray | Yes | Idle state row; Tray menu row; IA table |
| CAP-3 Global hotkey config | Yes | "Hotkey capture field" pattern; "Hotkey conflict" state |
| CAP-4 Prompt Overlay summon/type/dismiss | Yes | "Prompt Overlay" pattern; Flow 1 |
| CAP-5 TTS generation | Yes | "Speak succeeded"/"Speak failed" states — see Gap 2 below on "in-app" wording |
| CAP-6 Playback via Virtual Microphone | Yes | "Speak succeeded" state |
| CAP-7 Dependency Check/provisioning | Yes | "Dependency row" pattern; "Dependency missing" and "No GPU available" states |
| CAP-8 Turkish/English UI | Partial | IA table lists "Settings — General ... UI language" only — see Gap 1 |
| CAP-9 Modern/clean visual design | Yes (DESIGN.md's role, not EXPERIENCE's) | DESIGN.md defers to gpui-kit Design Guides throughout |

## Gaps

1. **CAP-8 / FR-8 "no restart" and language-independence behavior is not stated anywhere in EXPERIENCE.md.** SPEC's success criterion is explicit: "Switching UI language takes effect without a restart; UI language and speech language can differ." EXPERIENCE.md's Component Patterns and State Patterns tables have no row for the language switcher at all — it only appears as a one-line IA table cell ("Settings — General ... UI language (Turkish/English)"). Nothing documents the live-switch behavior or the UI/speech-language independence as a UX contract, even though FR-8's consequences (live switch, independent from TTS language) are exactly the kind of behavioral detail this table format is meant to hold for every other capability.

2. **CAP-5's "clear in-app failure" wording is in tension with EXPERIENCE's OS-notification-only failure path.** SPEC says a generation failure must "surface a clear in-app failure, never silent nothing." EXPERIENCE.md's "Speak failed" state routes exclusively through an OS-native notification ("No overlay reappears automatically"), not anything in-app, because by CAP-4 design the overlay is already gone before generation finishes. This is a defensible design given the fire-and-forget overlay, but it's a literal mismatch with SPEC's "in-app" phrasing that's worth a one-line reconciliation (e.g. updating SPEC's wording to "in-app or OS-native" or EXPERIENCE explicitly noting the deliberate substitution) rather than leaving it implicit.

## Non-goal / constraint contradictions

None found. Specifically checked and clear:
- No Preset Phrase feature added — EXPERIENCE explicitly lists it as banned/non-goal.
- No live/continuous voice-changing implied anywhere.
- No macOS-specific UX described.
- Dark-theme-only, single accent color, no raw hex outside tokens — consistent with "gpui-kit components, not raw GPUI primitives" constraint. The Prompt Overlay's "custom composition" is explicitly built from gpui-kit's popover treatment + Input, not raw GPUI, so this doesn't violate the constraint despite the "not a stock gpui-kit surface" phrasing.
- No cloud/telemetry-implying behavior (OS-native notifications are local, not a network call).

## Glossary term usage

Consistent throughout for all five required terms — Reference Voice Sample, Prompt Overlay, Speak Action, Virtual Microphone, Dependency Check — no invented synonyms found in either file.

One minor, likely-acceptable deviation: the microcopy example "Couldn't generate speech. Check the sidecar and try again." uses lowercase "sidecar" rather than the glossary's formal "Sidecar Process." This is user-facing copy (deliberately informal per the Voice and Tone table), not a spec/contract reference, so it's not flagged as a real violation — noting it only in case the author wants strict terminology even in microcopy.

## PRD UX-relevant detail not covered by SPEC alone

- FR-2 (background/tray) and FR-3 (hotkey conflict) are both fully represented in EXPERIENCE.md, matching PRD's testable consequences.
- FR-7's CPU-mode degradation is represented via the "No GPU available" state row ("Badge: CPU mode ... not an error").
- FR-8 is the one PRD item with a real gap — see Gap 1 above.

## Summary

- **Gap count: 2** (CAP-8/FR-8 live-switch + independence behavior undocumented; CAP-5 "in-app" vs OS-notification wording tension).
- No CAP is entirely missing a surface/component/state.
- No non-goal or constraint contradictions.
- Glossary usage is clean.
