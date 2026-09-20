# Reconciliation: PRD vs. Brief (voice-me, 2026-09-20)

Compared `brief.md` + brief `addendum.md` against `prd.md` + PRD `addendum.md`. The PRD correctly turns most of the brief's narrative into FRs/scope/Open Questions (latency, anti-cheat risk, driver-signing candidate, Chatterbox-Nano-style fallback intent, dev sequencing, macOS deferral, Preset Phrase deferral). The gaps below are things in the brief that would silently disappear if someone only ever read the PRD going forward.

## Gaps found

- **No FR (or Non-Goal, or Open Question) for system-tray / background presence.**
  - *Brief:* States "voice-me sits quietly in the background" as core UX, and names "background tray presence" as an explicit required architecture piece — GPUI having "no built-in system tray... both are required here" is called out as a top-4 Key Risk.
  - *PRD:* FR-2 through FR-7 cover hotkey, overlay, TTS, playback, dependency check, and localization, but there is no requirement anywhere for a tray icon, background-run behavior, or how the user quits/reopens settings when no window is open. It's implied by "runs quietly" language in §1 Vision but never made a testable requirement.
  - *Fix:* Add a small FR (or fold into FR-2) for system-tray presence / background operation with a settings/quit entry point, so it isn't left to be inferred at implementation time.

- **GPUI's tray/hotkey gap and pre-1.0 status (a top brief risk) isn't carried into the PRD's Open Questions or Assumptions Index.**
  - *Brief:* Lists this as one of four Key Risks & Unknowns, explicitly flags the "Adabraka GPUI" fork as an unverified dependency to validate early.
  - *PRD:* Open Questions covers anti-cheat (Q3), driver control mechanism (Q2), latency (Q1), clip length (Q4), and sidecar packaging (Q5) — but never mentions the GPUI tray/hotkey plumbing gap or the fork evaluation, even though FR-2 (global hotkey) and the missing tray requirement both depend on it.
  - *Fix:* Add an Open Question (or Assumptions Index line) noting GPUI has no upstream tray/hotkey support and that the Adabraka fork (or a hand-built shim) needs validation before FR-2/tray work is locked in.

- **Chatterbox-Nano fallback is named in the brief but genericized in the PRD.**
  - *Brief:* Specifically names "Chatterbox-Nano" as the CPU-oriented fallback variant when no GPU is present.
  - *PRD:* FR-6's consequence only says the app "functions in a reduced/CPU-only mode when GPU acceleration isn't available" — the specific model name is dropped, so a future implementer (or the architecture doc) has no paper trail pointing to Chatterbox-Nano specifically vs. just running the main model slower on CPU.
  - *Fix:* Update FR-6's consequence (or the Assumptions Index) to name Chatterbox-Nano as the intended CPU fallback path.

- **Brief's "if shared publicly, organic adoption/stars" success criterion has no counterpart in PRD Success Metrics.**
  - *Brief:* Explicitly lists organic adoption/stars from other gamers as an `[ASSUMPTION]` success criterion if the project is ever shared publicly, tying back to the Vision section's "share it as a small open-source tool" aspiration.
  - *PRD:* §7 Success Metrics (SM-1 through SM-3, SM-C1) is entirely single-user/hobby-scale and doesn't mention public sharing at all, even as an explicitly-deferred/non-metric note.
  - *Fix:* Add one line to Success Metrics (or Vision) noting public-sharing adoption is an out-of-v1-scope aspirational signal, not a tracked metric, so the intent isn't silently lost.

- **"Practice/preview" playback-to-own-speakers idea (tied to a real trust/UX risk) isn't mentioned anywhere in the PRD.**
  - *Brief addendum:* Calls out a preview mode that plays generated audio back to the user's own speakers before first sending it to the virtual mic, specifically "for the first few uses while trust in the voice clone builds" — a concrete idea addressing a real risk (bad clone quality going live to teammates unheard-by-the-user-first).
  - *PRD:* FR-4/FR-5 handle failure indication for generation errors but say nothing about clone-quality trust or a preview path; the idea also isn't listed among the (mostly-dropped) future features.
  - *Fix:* Either add a short note under FR-4/FR-5 Notes or the Assumptions Index flagging preview-before-live-send as a considered-but-deferred mitigation, so the reasoning isn't lost if it resurfaces later.
