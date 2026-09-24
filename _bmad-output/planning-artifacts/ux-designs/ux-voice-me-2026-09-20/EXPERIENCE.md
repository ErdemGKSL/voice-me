---
name: voice-me
status: final
sources:
  - ../../specs/spec-voice-me/SPEC.md
  - ../../specs/spec-voice-me/glossary.md
  - ../architecture/architecture-voice-me-2026-09-20/ARCHITECTURE-SPINE.md
created: 2026-09-20
updated: 2026-09-20
---

# voice-me — Experience Spine

## Foundation

Desktop utility, Linux and Windows in v1 (macOS deferred), built with gpui-kit on GPUI. Single-user, single-machine, no accounts — there is exactly one "workspace." `DESIGN.md` is the visual identity reference; this spine is the experience. The product is invisible by default: it lives in the system tray and surfaces only as the Prompt Overlay (a `[gpui-kit-design-guides]` "utility window" shell — one focused task, short fixed action path) or the Settings window (a "single workspace" shell — one title bar, one primary view, no navigation sidebar needed at this scope).

## Information Architecture

| Surface | Reached from | Purpose |
|---|---|---|
| Prompt Overlay | Global hotkey (configurable) | Type a line, speak it, gone — the entire product's reason to exist |
| Settings — Voice | Tray menu → Settings | Record/import Reference Voice Sample |
| Settings — Backend | Tray menu → Settings | Local/Remote → backend; that backend's options: speech language, key, region, voice, sample state, GPU device |
| Settings — Hotkey | Tray menu → Settings | Capture and confirm the global hotkey |
| Settings — Dependencies | Tray menu → Settings, or auto-opened on a blocking Dependency Check failure | Status of runtime dependencies, one-click provisioning |
| Settings — General | Tray menu → Settings | UI language (Turkish/English) |
| Tray menu | System tray icon | Open Settings, quit |

Settings is one window with the five sections above as tabs or a simple in-window nav (gpui-kit `Tabs`) — not four separate windows. No sidebar workspace shell is warranted at this scope (five sections, one user, no growth expected). → Composition reference: none rendered this run (Fast path, creative tools skipped); spine text is authoritative until a mock exists.

## Voice and Tone

Microcopy. Brand posture lives in `DESIGN.md.Brand & Style` — quiet, gets out of the way.

| Do | Don't |
|---|---|
| "Recording…" | "We're capturing your voice! 🎙️" |
| "Couldn't generate speech. Try again." | "Oops! Something went wrong with the TTS engine." |
| "Hotkey already in use by {app}." | "Error: hotkey conflict detected." |
| "Missing: speech model files. Download now?" | "Dependency check failed (code 3)." |

## Component Patterns

Behavioral. Visual specs live in `DESIGN.md.Components`.

| Component | Use | Behavioral rules |
|---|---|---|
| Prompt Overlay | Global hotkey (anywhere, including in-game) | Appears centered or near cursor `[ASSUMPTION: centered on the active/focused monitor]`, single `Input` pre-focused so typing starts immediately. `Enter` closes it instantly and fires the Speak Action (CAP-4/CAP-5) — the overlay does not wait for generation. `Escape`, or losing focus, closes it and discards the text without speaking. |
| Hotkey capture field | Settings → Hotkey | Click "Change," press the desired combination, it's captured live and rendered as a hotkey chip for confirmation before saving. Already-in-use combinations are rejected inline, not after Save. |
| Voice recorder | Settings → Voice | Record button starts capture with the recording indicator (`DESIGN.md`); stop button ends it; a short playback control lets the user hear it back before accepting. Import is a standard file picker as an alternative entry point, same accept/re-record actions after either path. |
| Dependency row | Settings → Dependencies | One row per dependency: name, status (`Badge`: ready / missing / installing), and a one-click "Install" action when missing and automatable, or a short manual-steps link when not. |
| Backend selector | Settings → Backend, top | Two steps: a Local/Remote choice, then a `Select` of that kind's backends (Local: "Piper — natural, instant", tagged "stock voice", listed first and the first-run default where available; "Chatterbox — your voice" with the bundled CPU and each added runtime's targets, and "System voice — instant" — eSpeak NG on Linux, Windows voices on Windows, tagged "stock voice"; Remote: DeepInfra, fal.ai, Azure, and "Edge TTS — free, online" — Azure and Edge TTS tagged "stock voice"; Edge TTS has no key field and is always selectable, installed or not). Below it, only the selected backend's options. Beneath all, read-only, what the selection **actually acquired** at session build, which is not always what was selected (AD-9). |
| Speech language selector | Settings → Backend, per backend | A `Select` of the languages the selected backend supports; each backend remembers its own. Independent of the UI language. |
| Azure voice picker | Settings → Backend, Azure selected | Region `Input`, then a `Select` of the voices for the chosen locale, fetched on demand with the key (the one pre-disclosure request, AD-13). States plainly: "Speech will be in this Microsoft voice, not yours." |
| API key field | Settings → Backend, when a remote backend is selected | A masked `Input` for the provider key, with a plain line stating the key is stored in the settings file in plaintext (AD-13). |
| Remote sample state | Settings → Backend, when a cloning remote backend is configured | States that the Reference Voice Sample has been uploaded to that provider and is held there, with a delete action (FR-10). |
| GPU device selector | Settings → Backend, when a GPU backend is selected | A `Select` listing the devices the Dependency Check found this backend can drive, by name. Unset means "let the backend decide". Hidden when the CPU or a remote backend is selected, where there is nothing to choose. |
| Remote disclosure dialog | First Speak Action after selecting a remote provider | A `Dialog` naming the provider and stating exactly what is sent — the typed text, the language tag, and the Reference Voice Sample — confirmed once per provider before the first request. Declining leaves the backend selected but unusable, not silently switched. |
| Tray menu | System tray | Two items: "Settings…", "Quit". No status submenu at this scope — Settings itself is the place to check state. |
| UI language selector | Settings → General | A `Select` of Turkish/English. Applying a new value re-renders every open surface immediately — no restart, no confirmation dialog. Independent of each backend's speech language set in Settings → Backend; changing one never changes the other. |

## State Patterns

| State | Surface | Treatment |
|---|---|---|
| Idle (nothing summoned) | Tray only | No window visible; tray icon is the only presence. |
| Overlay open, composing | Prompt Overlay | Input focused, empty or mid-type; no other chrome. |
| Overlay dismissed, generating | (overlay already closed) | No blocking UI — this happens after the overlay is gone. If generation takes longer than expected, a brief OS-native notification/toast reports it's still working, not silence. `[ASSUMPTION: exact latency threshold pending PRD Open Question 1]` |
| Speak succeeded | (no surface) | Audio plays through the Virtual Microphone; no confirmation UI needed — the played audio is the confirmation. |
| Speak failed | OS-native notification | "Couldn't generate speech. {short reason}." This notification, originating from voice-me itself, *is* the "clear failure" surface (SPEC CAP-5) — no overlay reappears automatically, and the user re-invokes the hotkey to retry. |
| First run / no Reference Voice Sample yet | Settings → Voice (auto-opened) | Empty state: "Record your voice to get started," single primary action, no other Settings tabs distract from this blocking prerequisite. |
| Dependency missing (blocking) | Settings → Dependencies (auto-opened) | Named dependency, status `missing`, one-click Install where possible. The Prompt Overlay still opens on hotkey press but shows an inline notice instead of accepting input until resolved. |
| CPU backend selected | Settings → Backend | Informational `Badge`: "CPU mode" — not an error and not a fallback, just the selected backend. Generation works, slower. |
| Selected backend unavailable here | Settings → Dependencies (speech-blocking row, linking to Settings → Backend) | Named in words, not a bare error: what the selected backend needs, what this machine has, and what selecting the CPU backend would do — with that selection one click away. Never a silent substitution (AD-9). |
| Remote backend selected, no key | Settings → Backend | The backend row states that a key is required and where to get one; the Speak Action is blocked by Story 3.4's rules rather than failing at the provider. |
| edge-tts not installed | Dependencies; the overlay is blocked | "edge-tts is not installed. Please install it: `pipx install edge-tts`", with the distro's pipx command. No Install button — voice-me never runs pip (Story 3.17). |
| System voice unavailable | Settings → Backend, and a speech-blocking row in Dependencies | Linux: "eSpeak NG is not installed" with the distro install command; either OS: "No system voice for Turkish" with the OS's steps to add one. Never a silent switch to another voice or language. |
| Piper voice not downloaded | Settings → Backend, and a speech-blocking row in Dependencies | "No Piper voice installed" (Install downloads the default voice, fahrettin — Turkish, 63 MB, CC0) or "The selected voice … is not installed" (Install downloads it when its source is known), each with a progress bar. The Backend tab's Piper pickers list installed voices only, with "Manage voices". |
| Piper voices tab | Settings → Piper voices | Search and a language filter above a list of every voice from three catalogs (voice-me's own, `rhasspy/piper-voices`, speaches-ai; first source wins a duplicate). Each row: name, language, quality, size, source, licence ("see model card" when none is declared) and a status word (not installed / downloading / installed / in use), with Download (progress bar), Delete and Use. A catalog that cannot be read is one inline line; the others still list. The catalogs are fetched when the tab is opened or Refresh is pressed, never at startup. |
| Piper phonemizer missing | Dependencies | The same "eSpeak NG is not installed" row as System voice, noting that Piper needs it to read text. |
| Azure selected, no voice | Settings → Backend | "Pick a voice for Azure" inline; the Speak Action is blocked by Story 3.4's rules. |
| Selected GPU no longer present | Settings → Backend | The selector reports the missing device by name and reverts to the default rather than failing silently. |
| GPU lost mid-generation | OS-native notification | Surfaced as a generation failure. The audio produced by a device that lost its context is corrupted (measured on Maxwell/NVK, Story 2.5) and must never be played. |
| Hotkey conflict | Settings → Hotkey | Inline error at the capture field: "Already used by {app}." Previous working hotkey stays active until a new one is confirmed. |

## Interaction Primitives

**Hotkey-first, everywhere else mouse-first.** The one interaction that must work flawlessly from anywhere, including inside a fullscreen game, is the global hotkey. Everything else (Settings) is an ordinary desktop app — no special keyboard discipline beyond standard tab order.

- **Configurable global hotkey** (default unset — first-run setup requires picking one) — summons the Prompt Overlay.
- `Enter` — inside the Prompt Overlay, fires the Speak Action and closes it.
- `Escape` — inside the Prompt Overlay, cancels and closes it without speaking.
- No other shortcuts are in scope for v1 — no Preset Phrase hotkeys (non-goal, see SPEC.md), no in-overlay command mode.

**Mouse:** Settings is entirely mouse-operable as a fallback; no action requires the keyboard except typing the line itself.

**Banned for v1:** any second global hotkey, an in-overlay menu or autocomplete, drag-and-drop anywhere, a resizable Prompt Overlay (fixed comfortable width per `DESIGN.md`).

## Accessibility Floor

Behavioral. Visual contrast lives in `DESIGN.md` (inherits gpui-kit's theme, verified at AA).

- The Prompt Overlay must be operable with keyboard alone from summon to dismissal — this is definitionally true given the product only has keyboard input, but it must never silently require a mouse click to focus the input after appearing.
- Settings follows gpui-kit's standard focus order, visible focus rings, and keyboard operability for every control (tab capture, record/stop, install buttons).
- Icon-only controls (tray icon, any toolbar icon in Settings) carry a tooltip and accessible name per the Design Guides.
- Status is never color-only: the "missing" dependency `Badge` carries the word "missing," not just a color change; "CPU mode" is a label, not an icon alone.

## Responsive & Platform

Not a responsive-web surface; this is fixed-purpose desktop software with two real platform variables:

| Platform | Behavior |
|---|---|
| Linux | Hotkey chip renders `Ctrl`/`Alt`/`Shift`/`Super` in platform-native naming; tray icon uses the desktop environment's native status-icon convention. |
| Windows | Hotkey chip renders `Ctrl`/`Alt`/`Shift`/`Win`; tray icon sits in the Windows notification area. |

The Prompt Overlay is a fixed comfortable width (not user-resizable) per `DESIGN.md`'s "utility window, short fixed action path" — there is no responsive breakpoint behavior to define at this scope. Settings has a documented minimum window size (small, five-tab content) but is otherwise not a layout concern worth detailing further at this stage.

## Key Flows

### Flow 1 — Erdem calls out a line mid-match without speaking

1. Erdem is mid-game, teammates on voice chat, can't speak out loud right now.
2. He presses his configured hotkey. The Prompt Overlay appears immediately, input already focused.
3. He types "sağ tarafa geçin."
4. He presses `Enter`. The overlay vanishes instantly — no waiting, no spinner.
5. **Climax:** about a second later, his cloned voice says the line through the Virtual Microphone. His teammates hear it exactly like a normal callout — he never had to alt-tab, open a window, or wait on the interface. The game never lost focus.

Failure: generation takes unusually long → a brief OS notification says it's still working. Failure: generation errors → OS notification names the problem; no overlay reappears, Erdem just presses the hotkey again to retry.

### Flow 2 — Erdem sets up his voice for the first time

1. First launch. No Reference Voice Sample exists yet, so Settings → Voice opens automatically instead of the app going straight to the tray.
2. Empty state: "Record your voice to get started." He clicks Record, speaks a short sample, clicks Stop.
3. He plays it back inline, decides it's good, clicks Accept.
4. Settings → Hotkey is the natural next stop: he clicks "Change," presses `Alt+Shift+Space`, sees the chip confirm the combination isn't taken elsewhere, and saves.
5. **Climax:** he closes Settings entirely. The app drops to the tray — no window, no chrome, just the icon — and is now, silently, ready for the hotkey he just set. The whole setup took under a few minutes and never asked him to open a terminal.

Failure: a required dependency (e.g. the speech model files, or the ONNX Runtime library) isn't provisioned yet → Settings → Dependencies opens instead, named specifically, with a one-click Install; Voice Setup resumes once it completes.
