---
title: Voice Me icon and interface refresh
type: feature
created: '2026-09-26'
status: done
route: dispatch
review_loop_iteration: 0
baseline_commit: 7b8c5576f12bc9a2fc3345cb316360ff605c9c78
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** The app still uses placeholder tray dots, cannot reflect speech activity there, and packs five Settings tabs into its title bar. Voice setup is visible even when the chosen engine does not use that page.

**Approach:** Build a waveform identity with five native tray states, move Settings navigation into a left sidebar beneath a centered branded title, and polish the prompt overlay without changing its fast entry flow.

## Boundaries & Constraints

**Always:** Keep the dark GPUI Kit theme and violet accent. Show Voice in the sidebar only for Chatterbox; show the reference-sample recorder within Speech for remote cloning. Keep OS-appropriate window controls, speech notifications, first-run access to sample recording, and current speak results. Tray states are starting, ready, generating, playing, and attention needed; icons and tooltips must communicate state without relying on color alone. Track overlapping speech work; playing starts only after the playback queue lock is acquired. Commit and push after verification.

**Never:** Add a tray submenu, make the overlay wait for speech, or clean the Cargo build.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|---------------|----------------------------|----------------|
| Startup | Dependency check pending | Starting icon and tooltip | Failed check becomes attention |
| Speech | One or more lines generating or playing | Playing outranks generating; completion of one line preserves other active state | Failed attempt shows attention until next attempt or successful readiness check |
| Backend switch | Chatterbox to another engine while Voice page selected | Voice nav disappears and Speech opens | Recorder remains available within Speech for cloning remote providers |
| Blocked | Current speech blocker | Attention icon and informative tooltip | Existing notification and Settings detail remain authoritative |

</frozen-after-approval>

## Code Map

- `crates/voice-me-core/src/ports.rs`, `speak.rs` — tray contract and generation/playback boundary; retain existing speak entry point.
- `crates/voice-me-tray-linux/src/lib.rs`, `crates/voice-me-tray-windows/src/lib.rs` — native gpui-tray handles and placeholder pixels; gpui-tray supports `set_icon` and `set_tooltip`.
- `crates/voice-me-app/src/main.rs` — event receiver, background work, tray registration, settings composition and asset source.
- `crates/voice-me-ui/src/settings.rs` — title bar, five tabs, view routing; `backend.rs` supplies current selection and Piper navigation.
- `crates/voice-me-ui/src/prompt_overlay.rs` — one-field overlay, existing dismissal/focus contract.

## Tasks & Acceptance

**Execution:**
- [x] `assets/` and platform packaging — create waveform vector, tray variants, PNG exports and ICO.
- [x] `crates/voice-me-core/src/ports.rs`, `speak.rs` — expose tray visual state and optional phase reporting while preserving speak behavior.
- [x] Both tray adapters — load state assets, update icon and tooltip, preserve menu/actions.
- [x] `crates/voice-me-app/src/main.rs` — derive readiness, track concurrent speech work and push visual updates on the GPUI thread.
- [x] `crates/voice-me-ui/src/settings.rs` — implement centered title and left navigation, contextual Voice and Piper management, remote cloning recorder.
- [x] `crates/voice-me-ui/src/prompt_overlay.rs` — refine hierarchy and spacing without adding steps.

**Acceptance Criteria:**
- Given a ready app, when a line generates and then plays, the tray shows each phase and returns to ready after completion.
- Given overlapping lines, when one completes, the tray still reflects the other active line.
- Given a non-Chatterbox backend, when Settings opens, Voice is absent; a cloning remote has recorder access in Speech.
- Given minimum Settings width, when navigating by keyboard, controls and focus remain usable.

## Implementation Notes

- Native tray state uses shared `TrayActivity` precedence. Dependency reports carry the exact selected backend so stale same-device reports cannot reset the icon or readiness.
- Linux ships a desktop entry and hicolor icons with matching `app_id`; Windows embeds the ICO through a resource build script.

## Spec Change Log

## Review Triage Log

- `medium` — readiness refresh could leave Ready visible; fixed with a tray checking state that preserves the hotkey's held blocker.
- `medium` — a stale backend report could override current readiness; fixed by matching exact backend selection before accepting it.
- `medium` — check failure could mask active speech; fixed through the shared tray precedence rule.
- `medium` — panel refresh could close the remote recorder; fixed by preserving it while selection stays the same.
- `low` — first-run sample copy implied all engines need a sample; corrected to describe cloning engines.
- `high` — app icon exports were unused; added Linux desktop integration and Windows executable resource, and rendered the authored mark in the title.
- `medium` — full-width title could cover window controls; constrained its hit area and added bounds checks.
- `medium` — sidebar reduced content space at minimum width; added a content-width and keyboard-path check.
- `low` — attention tooltip could overflow native limits; replaced it with a short state message.
- `maybe-false` — Windows native tray remains unexercised on this Linux host; Windows adapter checks and resource compilation pass, and a Windows runtime check is still needed on Windows.
- `medium` — app speech completion was tested only through a map; shared `TrayActivity` transitions now have overlapping-work and completion tests.
- `medium` — title centering was checked only by presence; the rendered test now asserts its center and clearance from controls.

## Verification

**Commands:**
- `cargo fmt --check` — formatting clean.
- `cargo check -p voice-me-app` — affected Linux workspace compiles incrementally.
- `cargo test -p voice-me-core -p voice-me-ui -p voice-me-tray-linux` — behavior tests pass.
- `cargo test -p voice-me-app` — tray work and readiness transitions pass.
- `cargo check -p voice-me-tray-windows` — adapter code checks on Linux.
- `desktop-file-validate assets/linux/voice-me.desktop` and `x86_64-w64-mingw32-windres voice-me.rc` — launcher and Windows resource validate.
