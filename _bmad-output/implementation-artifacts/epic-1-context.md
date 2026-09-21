# Epic 1 Context: Voice Identity Setup

<!-- Compiled from planning artifacts. Edit freely. Regenerate with compile-epic-context if planning docs change. -->

## Goal

Users can record their own voice directly in the app, or import an existing audio file, and use it as their Reference Voice Sample — saved locally, selectable as active, and replaceable at any time from settings. This is the foundation every later Speak Action depends on (no TTS generation is possible without an active Reference Voice Sample), and it's a complete, demonstrable capability on its own: record or import, hear it back, done. Epic 1's first story also stands up the Cargo workspace and crate skeleton per the Architecture Spine's Structural Seed, since no starter template exists to adopt — this is a from-scratch hexagonal workspace.

## Stories

- Story 1.1: Set Up the Project Workspace
- Story 1.2: Record a Reference Voice Sample
- Story 1.3: Import an Existing Audio File as the Reference Voice Sample
- Story 1.4: Replace My Reference Voice Sample
- Story 1.5: First-Run Voice Setup Prompt

## Requirements & Constraints

- A recorded or imported clip must be saved locally and become selectable as the active Reference Voice Sample; the user can re-record or re-import to replace it at any time from Settings, repeatably, without restarting the app.
- Minimum/maximum clip length and supported import formats are not yet specified — they are enforced with a clear in-app message when violated, with exact bounds to be set during implementation from Chatterbox's own guidance (open item, not user-specified).
- An unsupported import format or an out-of-bounds clip must show a clear in-app message rather than failing silently.
- Inline playback of the captured/imported clip must be available before it is accepted as active.
- On first launch, with no Reference Voice Sample yet, the app must walk the user straight into Voice Setup rather than dropping to a bare tray icon; once a sample is accepted, this auto-open must never fire again.
- UI must be built from gpui-kit components (not hand-rolled GPUI primitives) wherever a suitable one exists, and follow the gpui-kit Design Guides for spacing, typography, color, density, and interaction states — checked before any story is considered done (FR9, cross-cutting acceptance bar for every story in this epic).

## Technical Decisions

- Hexagonal/ports-and-adapters paradigm: `voice-me-core` holds domain types, `AppState`, `AppEvent`, and every port trait (including `SettingsStore`); it depends on no other `voice-me-*` crate.
- Workspace structural seed (Story 1.1): member crates `voice-me-app` (bin, composition root), `voice-me-core`, `voice-me-ui` (gpui-kit), `voice-me-i18n`, `voice-me-tests`, plus empty stub crates for `voice-me-hotkey-{linux,windows}`, `voice-me-audio-{linux,windows}`, `voice-me-tray-{linux,windows}`, `voice-me-tts`, `voice-me-deps`. `voice-me-core` contains stub `AppState`, `AppEvent`, and empty port trait definitions. Workspace must build with `cargo build`; GitHub Actions CI runs separate Linux and Windows build jobs (no macOS).
- `SettingsStore` is the one port implemented inside `voice-me-core` itself, not a separate adapter crate, since local file I/O via the `directories` crate doesn't vary by OS. Adapters (including `voice-me-ui`'s recorder) hand recorded/imported audio to `voice-me-core` to store — they never write to the data directory themselves, and they read settings only through `AppState`.
- Storage layout: one TOML settings file at the OS config directory; Reference Voice Sample audio files at the OS data directory (both resolved via `directories`). `AppState`'s active Reference Voice Sample field is a `PathBuf` into that data directory — not an ID with a separate manifest/index. No IDs or timestamps are needed at this altitude (single-user, single-machine).
- Large remotely-fetched runtime assets (Python runtime, model weights, driver installer) are explicitly out of `SettingsStore`'s scope — owned by `voice-me-deps` in a separate cache directory. Not relevant to this epic's own storage, but don't conflate the two.
- One `thiserror` domain error enum lives in `voice-me-core`; adapters map their own errors at the boundary.
- No network egress is permitted outside `voice-me-deps` — recording/import/storage in this epic is entirely local.
- `tracing` is initialized once centrally in `voice-me-app`; no crate sets up its own logger.

## UX & Interaction Patterns

- Inherit gpui-kit/gpui-component theme tokens wholesale; override only the primary accent color (`#7C6AFF`, violet) and overlay corner radius — neither is central to this epic except via the Recording indicator, the one place accent color appears outside a primary button.
- Dark theme only for v1; no light theme or switcher.
- Voice recorder component (Settings → Voice): Record/Stop with a Recording indicator (primary-violet dot/waveform) shown while capture is active, then inline playback before Accept. Import is an alternative entry via a standard system file picker, converging on the same accept/re-record actions as recording.
- First run with no Reference Voice Sample: Settings → Voice auto-opens to an empty state — "Record your voice to get started" — with a single primary action; no other Settings tabs are shown to distract from this blocking prerequisite.
- Settings is one window with four sections (Voice, Hotkey, Dependencies, General) via gpui-kit `Tabs`; this epic only touches the Voice section.
- Accessibility: Settings follows standard focus order and visible focus rings; all controls (record/stop, import, accept) must be keyboard-operable.

## Cross-Story Dependencies

- Story 1.1 (workspace scaffold) must land before any other story in this epic — it establishes the crates and stub types the rest build on.
- Stories 1.2 and 1.3 (record, import) both converge on the same accept/replace mechanism used by Story 1.4; Story 1.4 depends on at least one of them existing first.
- Story 1.5 (first-run prompt) depends on Stories 1.2/1.3/1.4 existing, since it auto-opens the same Voice Setup UI and its "never fires again" behavior depends on an active Reference Voice Sample being detectable via `AppState`.
- This epic is a hard prerequisite for Epic 2's Speak Action: Story 2.6 (Generate Speech) requires an active Reference Voice Sample to exist.
