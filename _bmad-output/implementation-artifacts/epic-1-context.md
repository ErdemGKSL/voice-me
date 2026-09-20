# Epic 1 Context: Voice Identity Setup

<!-- Compiled from planning artifacts. Edit freely. Regenerate with compile-epic-context if planning docs change. -->

## Goal

Let the user establish their Reference Voice Sample — the short clip of their own voice, recorded in-app or imported from a file, that the TTS engine uses to clone their voice for every generated line later on. The sample must be saved locally, selectable as active, and replaceable at any time from Settings, with no restart required. This epic is also where the Cargo workspace and crate skeleton get scaffolded, since no starter template exists — every later epic builds on the structure stood up here.

## Stories

- Story 1.1: Set Up the Project Workspace
- Story 1.2: Record a Reference Voice Sample
- Story 1.3: Import an Existing Audio File as the Reference Voice Sample
- Story 1.4: Replace My Reference Voice Sample
- Story 1.5: First-Run Voice Setup Prompt

## Requirements & Constraints

- A recorded or imported clip must be saved locally and become the active Reference Voice Sample; it must be re-recordable/replaceable at any time from Settings, repeatably, without restarting the app.
- Minimum/maximum reference-clip length and supported import formats are not yet specified by product — they must be set from the TTS engine's (Chatterbox-Multilingual V3) own documented recommendations during implementation, not invented. Whatever bounds are chosen must be enforced with a clear in-app message on violation (never silent failure).
- An unsupported import format or an out-of-bounds clip must show a clear in-app message rather than failing silently.
- Inline playback before accepting is required for both recorded and imported clips.
- On first launch with no Reference Voice Sample yet, Voice Setup must auto-open to an empty state ("Record your voice to get started") with a single primary action; this auto-open must never fire again once a sample has been accepted.
- Standard UI elements (buttons, inputs, the recorder, dialogs) must be gpui-kit components per the gpui-kit Design Guides — not hand-rolled — checked before considered done (FR9, cross-cutting acceptance bar for every story in this epic).

## Technical Decisions

- Hexagonal/Ports-and-Adapters: `voice-me-core` holds domain types, `AppState`, `AppEvent`, and every port trait; it depends on no other `voice-me-*` crate. Only `voice-me-core` use-case functions mutate `AppState`.
- Workspace to scaffold in this epic's first story: `voice-me-app` (bin, composition root), `voice-me-core`, `voice-me-ui` (gpui-kit), `voice-me-i18n`, `voice-me-tests`, plus empty stub crates `voice-me-hotkey-{linux,windows}`, `voice-me-audio-{linux,windows}`, `voice-me-tray-{linux,windows}`, `voice-me-tts`, `voice-me-deps`. `voice-me-core` gets stub `AppState`, `AppEvent`, and empty definitions for `HotkeyPort`, `VirtualMicPort`, `TrayPort`, `TtsPort`, `DependencyProvisioningPort`, `SettingsStore`. The workspace must build with `cargo build`, and CI must run separate Linux and Windows GitHub Actions build jobs from the start.
- Settings (one TOML file, OS config directory) and the Reference Voice Sample (audio file, OS data directory) both live behind the `SettingsStore` port, implemented inside `voice-me-core` itself (not a separate adapter crate) — resolved via the `directories` crate. `AppState`'s active Reference Voice Sample field is a `PathBuf` into that data directory, not an ID with a separate manifest/index.
- Adapters — including `voice-me-ui`'s recorder — hand recorded/imported audio to `voice-me-core` to store; they never write to the data directory themselves and never read settings except through `AppState`.
- One `thiserror` domain error enum lives in `voice-me-core`; adapters map their own errors into it at the port boundary; `anyhow` aggregates only at the composition root (`voice-me-app`).
- No network egress is permitted from any crate touched in this epic (recording/importing/storing a local file involves no network call at all).
- `voice-me-ui` is the crate that will hold the Voice Setup view, recorder, and related gpui-kit components.

## UX & Interaction Patterns

- Voice Setup lives in Settings → Voice; on first run it auto-opens directly (no tray-icon-only start) to an empty state with copy "Record your voice to get started" and a single primary action.
- Voice recorder component: Record/Stop controls with a Recording indicator (primary-violet dot/waveform accent) shown only while capture is active — this is the one place the accent color appears outside a primary button/focus ring. After Stop, inline playback lets the user hear the clip before accepting.
- Import is an alternative entry point via the standard OS file picker, converging on the same playback/accept/re-record actions as a live recording.
- Re-record/re-import from Settings → Voice works the same way as first-time setup and can be repeated indefinitely; the new clip replaces the old as active only once accepted.
- Visual system: dark theme only (v1), gpui-kit theme tokens inherited wholesale except the primary accent (`#7C6AFF` violet, used here for the recording indicator) — no second accent color, no raw hex values in application code.

## Cross-Story Dependencies

- Story 1.1 (workspace scaffold) is a prerequisite for all other stories in this epic and for every later epic — it establishes the crates and port traits everything else is built into.
- Stories 1.2 and 1.3 both converge on the same "active Reference Voice Sample" state and playback/accept flow in `voice-me-core`/`voice-me-ui`; Story 1.4 (replace) builds directly on whichever of them ships first.
- Story 1.5 (first-run prompt) depends on Stories 1.2–1.4 existing, since it auto-opens the same Voice Setup UI and its "never fires again" condition depends on a sample having been accepted.
- Epic 2's Story 2.6 (Generate Speech) depends on this epic's Reference Voice Sample existing and being accessible via `AppState`.
