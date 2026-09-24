# Sprint Change Proposal — Edge TTS through the `edge-tts` program (Linux)

**Date:** 2026-09-24
**Author:** Developer (implementation plan), for Erdem (product owner)
**Mode:** Batch
**Builds on:** Story 3.12 (eSpeak NG, the child-process pattern), Story 3.14 (Azure, the stock-voice list and disclosure), `sprint-change-proposal-2026-09-24-piper.md` (the shared runner)
**Status:** Proposed — awaiting Erdem's approval of E1–E7. No PRD, architecture, epics or sprint-status edits are applied yet.

---

## 1. Issue Summary

**Trigger.** Erdem asked for an **Edge TTS** backend:

- It is for **Linux only** for now. Windows is not needed.
- It uses the **`edge-tts` command-line program**.
- It is **always listed and selectable**, whether or not `edge-tts` is installed.
- If the program is missing, selecting it works, but speech is **blocked by a dependency error** that asks the user to install edge-tts.

**Category.** A new requirement from the stakeholder.

**The gap it fills.** Edge TTS speaks with Microsoft's neural voices, the same ones Azure has (`tr-TR-EmelNeural`, `tr-TR-AhmetNeural`, …). It needs **no API key and no account**, and it downloads no model. The trade-offs:

- Compared with **Azure** (3.14): the same voices, without a key.
- Compared with **Piper** (3.15): more voices and languages, but online only.
- Compared with **eSpeak NG** (3.12): it sounds natural rather than robotic.

**Evidence, checked 2026-09-24 in the cloud dev container** (`edge-tts` 7.2.8, installed with `pip` into a scratch venv and used only as a measuring tool):

- **Text can go in on stdin.** `edge-tts -f -` reads the text from stdin (`util.py`: `if args.file in ("-", "/dev/stdin"): args.text = sys.stdin.read()`). The text never needs to be on argv, so Story 3.12's "text on stdin, never argv" rule still holds.
- **Audio can come out on stdout.** With no `--write-media` (or with `--write-media -`), the audio goes to stdout. The "writing to a terminal" prompt only fires when stdin *and* stdout are TTYs, and voice-me pipes both.
- **Output format:** always `audio-24khz-48kbitrate-mono-mp3`. It is already AD-11's 24 kHz mono, so it only needs MP3 decoding, not resampling. `symphonia` with its `mp3` feature is already in the workspace (`voice-me-tts/Cargo.toml`).
- **Voice list:** `edge-tts --list-voices` prints a fixed-width table with a header and a `---` rule (columns `Name  Gender  ContentCategories  VoicePersonalities`). There are 323 voices, including `tr-TR-AhmetNeural Male` and `tr-TR-EmelNeural Female`. This is the same `ShortName` form that Azure's `StockVoice` list already uses.
- **Network.** `edge-tts` opens a WebSocket to `speech.platform.bing.com` (Microsoft's Edge "Read Aloud" service, which is undocumented). In this container, synthesis failed with `CERTIFICATE_VERIFY_FAILED` because of the sandbox's TLS proxy, so **synthesis was not measured here**. The voice list did work. On failure, the program exits with status 1 and prints a Python traceback to stderr, and the last line holds the reason.
- **Licence:** `edge-tts` is LGPL-3.0. voice-me runs it as a separate program and never links or ships it (the AD-12 exception, as for `espeak-ng`).

---

## 2. Impact Analysis

| # | Item | Status | Finding |
|---|---|---|---|
| 1.1–1.3 | Trigger and evidence | [x] | See §1 |
| 2.1 | Current epic | [x] | Epic 3 can still be completed. It gains one story |
| 2.2 | Epic-level changes | [!] | New **Story 3.17** (Edge TTS on Linux). No story is renumbered |
| 2.3–2.4 | Other epics | [N/A] | Epic 1: stock-voice backends already skip the first-run voice-setup prompt (3.12). Epic 4 adds the Turkish copy of the new strings |
| 2.5 | Order | [?] | Proposed: after 3.15 Piper (E7), because both touch the shared process runner |
| 3.1 | PRD | [!] | FR-5 (stock-voice list), FR-7 (checked components), FR-10 (remote backends), §6.1, Glossary |
| 3.2 | Architecture | [!] | AD-7 (backend table), **AD-8 (egress: a child process that talks to the network)**, AD-12 (a second fixed program), Structural seed, Capability map |
| 3.3 | UX | [!] | A Remote entry "Edge TTS — free, online (stock voice)", its disclosure, and a missing-program state row |
| 3.4 | Other | [!] | `sprint-status.yaml`, `epic-3-context.md`, CI (`ci.yml`: install `edge-tts` for the parser smoke test) |
| 4.1 | Direct adjustment | Viable | Effort: Medium (roughly the size of 3.12, reusing 3.14's voice list). Risk: Medium, because the service is unofficial |
| 4.2–4.3 | Rollback / MVP review | Not needed | Additive. No existing behaviour changes |

---

## 3. Recommended Approach — decisions

Decisions E1–E3 are from Erdem's request. E4–E7 are recommended with this plan and need approval.

- **E1 — Linux only.** The adapter is built under `cfg(target_os = "linux")`. On other OSes the entry is listed, and its capability row says "Edge TTS isn't available on this OS yet". That follows the fal.ai / System voice precedent in `capability.rs`. No engine is built there.
- **E2 — Always listed and selectable.** Selecting Edge TTS never checks for the program. Presence is a Dependency Check question, not a selection question.
- **E3 — A missing program is a blocking dependency error, with manual steps only.** It is a speech-blocking row (Story 3.4), so the overlay is blocked with the row's message.
  - The copy is "edge-tts is not installed. Please install it: `pipx install edge-tts`". It gains a Turkish copy in Epic 4: "edge-tts kurulu değil. Lütfen kurun: `pipx install edge-tts`".
  - There is no Install button. voice-me does not run pip for the user.
- **E4 — It is a *Remote* backend, not a Local one.** The text leaves the machine for Microsoft, so it goes in the Remote group, and it needs the same **one-time disclosure** as the other providers (Story 3.6), but **no API key**. It is a stock voice, so it never receives the Reference Voice Sample, and the first-run prompt and Settings auto-open stay quiet (as for 3.12 and 3.14).
  - *Rejected alternative:* listing it under Local because the program runs locally. That would mislead the user about where their text goes, and it would break the "no network call until the user picks a remote backend" promise (FR-10).
- **E5 — Text on stdin, audio on stdout, and nothing typed on argv.** The argv is exactly `--voice=<voice id> -f - --write-media -`. The voice id comes from the listed voices, and the `=` form stops a voice id from being read as a flag. There is no shell, and `edge-tts` is resolved on `PATH` and then in `~/.local/bin` (see E6).
- **E6 — The program is found in `PATH` or in `~/.local/bin`.** `pipx` and `pip install --user` put the program in `~/.local/bin`. A desktop-launched app often lacks that directory on its `PATH`, so the lookup also checks `$HOME/.local/bin/edge-tts`. The ready row names the path it found.
- **E7 — One runner, two programs.** Piper's plan (3.15) moves the bounded runner (stdin in, stdout on its own thread, deadline, kill and reap) out of `voice-me-tts-system-linux/src/process.rs`. This plan makes that runner **program-agnostic**, taking the program, args, stdin bytes and deadline.
  - If 3.15 lands first, its `voice-me-espeak` crate keeps the eSpeak wrappers, and the generic `run` moves to a crate named `voice-me-process`. If 3.17 lands first, this story does the extraction itself.
  - Either way, **one crate spawns processes**, and AD-12's exception names exactly two programs: `espeak-ng` and `edge-tts`.

---

## 4. Technical Plan

### 4.1 New crate — `crates/voice-me-tts-edge/` (Linux-gated, workspace member)

`#![cfg(target_os = "linux")]`, as in `voice-me-tts-system-linux`. Its deps are `voice-me-core`, the shared runner (E7), and `symphonia` (`mp3` feature, the same version as `voice-me-tts`).

- **Constants:**
  - `PROGRAM = "edge-tts"`
  - `ENGINE_LABEL = "Edge TTS"`
  - `SPEECH_DEADLINE = 30 s`. This is network-bound and matches Azure's `AzureDeadlines::speech`.
  - `VOICES_DEADLINE = 15 s`, matching Azure's `voices`.
- **`pub fn find_program() -> Option<PathBuf>`** — `PATH`, then `$HOME/.local/bin/edge-tts` (E6). It must be executable.
- **`pub fn speak_args(voice_id: &str) -> Vec<OsString>`** — exactly `["--voice=<id>", "-f", "-", "--write-media", "-"]`.
- **`pub fn list_voices() -> Result<Vec<StockVoice>, VoiceMeError>`** — runs `edge-tts --list-voices` with a null stdin and `VOICES_DEADLINE`, then parses the result with a pure function:
  - Skip the header line and the `---` rule. Split each row on runs of whitespace. `Name` is the first field and `Gender` the second.
  - **The locale** is `Name` minus its last `-` segment: `tr-TR-EmelNeural` → `tr-TR`, `zh-CN-liaoning-XiaobeiNeural` → `zh-CN-liaoning`, `iu-Cans-CA-SiqiniqNeural` → `iu-Cans-CA`.
  - **The display name** is the last segment with `Neural` / `MultilingualNeural` removed, then the gender: "Emel (Female)".
  - **The language label** is the locale itself, since the program lists no locale names. A small table for `tr-TR` / `en-US` / `en-GB` is optional, as polish.
  - Blank or malformed rows are skipped.
  - A list with **zero** voices is an error ("Edge TTS listed no voices").
- **`EdgeTts` implementing `TtsPort`** — `warm_up` does nothing and `is_ready` is true. `generate(text, None, language, Some(voice))` does the following:
  1. Run the program with `speak_args(voice)`, the UTF-8 text on stdin, and `SPEECH_DEADLINE`.
  2. Decode the stdout MP3 with symphonia to mono f32. If the rate is not 24 000 Hz, which should not happen, resample it. Reuse the `resample` copy from 3.12's `wav.rs`, or move it into the shared crate.
  3. Map failures to `VoiceMeError::SpeechEngine("Edge TTS …")` with a short reason:
     - A non-zero exit gives the **last non-empty stderr line**, trimmed to 200 chars. That line is the Python exception, for example `ClientConnectorError: Cannot connect to host speech.platform.bing.com:443`.
     - A timeout gives "took longer than 30 s", and the child is killed and reaped.
     - Empty or undecodable stdout gives "returned no audio".
     - `ErrorKind::NotFound` at spawn gives "edge-tts is not installed. Please install it: pipx install edge-tts". The binary can vanish after the check.
  4. `generate` with a `None` voice returns a refusal, but core resolves a voice before it gets here (§4.2).
- **Tests (pure, no network):**
  - Parse a captured `--list-voices` fixture, checked into `tests/fixtures/list-voices-7.2.8.txt`. Check the header and rule are skipped, the locales above (including the 3- and 4-segment ones), the names and the gender.
  - Check the argv never contains the text: hostile text like `--voice=x`, `; rm -rf ~` and `$(…)` goes to stdin only.
  - Decode a small checked-in MP3 fixture and check it comes out at 24 kHz with the expected length.
  - Use a fake program through a test-only constructor that overrides the program path (the 3.12 pattern) for:
    - the deadline kill,
    - a non-zero exit turned into the last stderr line,
    - empty stdout turned into "returned no audio",
    - a missing program turned into the install message.
  - Check `find_program` finds a fake executable in a temporary `HOME/.local/bin`, with `PATH` empty.
- **Integration test** `tests/edge_tts.rs` (Linux-gated):
  - It skips itself when `edge-tts` is absent.
  - It runs `--list-voices` and asserts that `tr-TR-EmelNeural` is listed.
  - Synthesis needs Microsoft's service, so it also skips unless `VOICE_ME_EDGE_TTS_ONLINE=1` is set. CI never depends on an unofficial online service.

### 4.2 Core — `crates/voice-me-core/src/`

- **`state.rs`:**
  - Add `RemoteProvider::EdgeTts`: `serde` name `edge_tts`, label "Edge TTS", added to `ALL` after Azure, and `is_stock_voice() = true`.
  - Add `RemoteProvider::needs_api_key()`. It is `false` only for `EdgeTts`. Every place that checks for a key today (capability rows, the Backend tab's key field, `speak`'s key refusal) asks this first, so Edge TTS never shows a key field or a "no key" row.
- **`LanguageBackend::Remote(EdgeTts)`:**
  - `has_voice_list() = true`, the dynamic list shared with Azure and the System voice.
  - `requires_voice() = false`. Unlike Azure, an unset voice falls back to the language's first listed voice, as the System voice does.
  - The default language is `tr-TR`.
  - Its voice list lives in `AppState` beside Azure's. It is not persisted.
- **`settings_store.rs`** — `edge_tts` in `[speech_languages]` and `[speech_voices]`, following the 3.11/3.12 pattern. Saving a new language clears the stored voice.
- **`speak.rs`** — the `Remote(EdgeTts)` branch:
  - Check the disclosure (3.6).
  - Skip the key check (`needs_api_key`).
  - Skip the sample check (stock voice).
  - Resolve the language and voice against the listed voices, using `resolve_stock_voice`, refusing by name and never substituting.
  - Pass `None` and the voice id.
- **`DependencyKind::EdgeTtsProgram`** — `blocks_speech() = true` and manual steps only. Its UI slug is `edge-tts-program`.

### 4.3 Dependencies — `crates/voice-me-deps/`

- **`capability.rs` + `lib.rs`:**
  - When `Remote(EdgeTts)` is selected on Linux, add the **Edge TTS row**:
    - **Found:** Ready, naming the path.
    - **Missing:** **blocking**, with the title "edge-tts is not installed" and the step "Please install it: `pipx install edge-tts`".
    - The steps then list per-distro ways to get `pipx`, from `/etc/os-release` (reuse 3.12's `os_release.rs`):
      - arch → `sudo pacman -S python-pipx`
      - debian/ubuntu → `sudo apt install pipx`
      - fedora → `sudo dnf install pipx`
      - opensuse → `sudo zypper install python3-pipx`
      - otherwise → "or `pip install --user edge-tts`"
  - There is no Install action, so `provision` returns the same "manual steps" error as `SystemVoiceEngine`.
  - On other OSes the selection gets the `cannot_run` row "Edge TTS isn't available on this OS yet" (E1).
  - There is no key row and no engine row (`local_target()` is `None`).
- `voice-me-deps` depends on `voice-me-tts-edge` (Linux only) for `find_program`, following the 3.12 precedent.

### 4.4 UI — `crates/voice-me-ui/`

- **`backend.rs`:**
  - The Remote `Select` lists "Edge TTS — free, online (stock voice)" after Azure.
  - Selecting it is **always allowed**, whether or not the program is installed (E2).
  - There is no API-key field.
  - It shows the Stock voice tag, the speech-language `Select` fed from the Edge voice list, and a voice `Select` when a language has more than one voice (the 3.12 widgets and ids, with the provider's own list).
  - A failed voice listing shows beside the speech language: "Couldn't list Edge TTS voices: …". A missing program is the Dependencies row and is not repeated here.
- **Disclosure copy:** "Edge TTS sends the text you type to Microsoft's Edge Read Aloud service. It is free, needs no account, and is not an official Microsoft API: it may stop working at any time." Confirmed once, as in 3.6.
- **`dependencies.rs`:** the `edge-tts-program` slug, and the manual-steps row with a copy button for the command.
- **Overlay:** nothing new. The speech-blocking row already blocks it with the row's message (3.4).

### 4.5 App wiring — `crates/voice-me-app/`

- `build_engine`:
  - `Remote(EdgeTts)` builds `voice-me-tts-edge` under `cfg(target_os = "linux")`, and `unavailable` everywhere else.
  - It does not go through `voice-me-tts-remote`: no reqwest and no key.
- `run_check` refreshes the Edge voice list in the background when Edge TTS is selected **and** the program is found (like `refresh_system_voices`), and merges it into `AppState` and the panel.
- `resolve_backend` maps it to the CPU placeholder, like the other remote and stock backends. The ONNX warm-up never starts for it.
- `Cargo.toml` adds the crate as a Linux-only dependency, and it becomes a workspace member.

### 4.6 CI — `.github/workflows/ci.yml`

- `build-ubuntu` runs `pipx install edge-tts==7.2.8` (pinned) so the `--list-voices` smoke test runs for real. The online synthesis test stays off (§4.1).

---

## 5. Detailed Change Proposals (to apply on approval)

### 5.1 PRD — `prd.md`

- **Glossary.** Add:
  > **Edge TTS** — Microsoft's Edge "Read Aloud" neural voices, reached through the separately installed `edge-tts` program; free and keyless, but online and unofficial; a stock voice.
- **FR-5.** Add "Edge TTS (Linux)" to the stock-voice backend list.
- **FR-7.** Add "the `edge-tts` program when Edge TTS is selected (manual install)" to the checked components.
- **FR-10.** Remote backends: add
  > Edge TTS, which needs no key and sends only the typed text, after the same one-time disclosure.
- **§6.1 In Scope.** Add "Edge TTS on Linux via the `edge-tts` program".
- **§6.2 Out of Scope.** Add:
  > Edge TTS on Windows; installing `edge-tts` for the user; rate/pitch/volume controls.

### 5.2 Architecture — `ARCHITECTURE-SPINE.md`

- **AD-7 table.** Add a row:
  > | Edge TTS | none (no ONNX) | none — the `edge-tts` program is installed by the user (`pipx`) | `voice-me-tts-edge`; Linux only |
- **AD-8.** Append:
  > The `edge-tts` child process (Story 3.17) reaches `speech.platform.bing.com`. It is spawned only while Edge TTS is selected and its disclosure is confirmed. voice-me's own code opens no socket for it, so `voice-me-tests`' egress allowlist is unchanged.
- **AD-12.** Widen the exception to two fixed programs, `espeak-ng` and `edge-tts`, with one runner crate (E7).
- **Structural seed.** Add:
  > `voice-me-tts-edge/  # lib: TtsPort adapter - the edge-tts program (Linux), MP3 via symphonia`

### 5.3 UX — `EXPERIENCE.md`

- **Backend selector, Remote group.** Add "Edge TTS — free, online (stock voice)", with no key field.
- **New state row:**
  > | edge-tts not installed | Dependencies, overlay blocked | "edge-tts is not installed. Please install it: `pipx install edge-tts`", with the distro's pipx command and a copy button. |

### 5.4 Epics — `epics.md`

Append after 3.16:

```
### Story 3.17: Speak With Edge TTS on Linux

As Erdem,
I want to pick Microsoft's free Edge voices through the edge-tts program,
So that I get natural neural voices in many languages without an API key or a model download.

**Acceptance Criteria:**

**Given** Remote → Edge TTS is selectable in Settings → Backend on every OS, whether or not `edge-tts` is installed
**When** it is selected on Linux and `edge-tts` is not on PATH or in ~/.local/bin
**Then** a speech-blocking Dependencies row says "edge-tts is not installed. Please install it: pipx install edge-tts" with the distro's pipx command, and the overlay is blocked (Story 3.4)
**And** with the program present and the disclosure confirmed, a Speak Action runs `edge-tts --voice=<id> -f - --write-media -` with the text on stdin (no shell, 30 s deadline), decodes the 24 kHz mono MP3 and plays it through the Virtual Microphone (AD-11, AD-12)
**And** the speech languages and voices come from `edge-tts --list-voices` (default language tr-TR, default voice the language's first); an unlisted language or voice is refused by name
**And** it is a stock voice: no Reference Voice Sample, no API key, the Stock voice tag
**And** a failure (exit ≠ 0, timeout, no audio, program gone) is one notification naming Edge TTS and the reason
**And** on non-Linux the selection's capability row says it isn't available on this OS yet
```

### 5.5 Sprint status — `sprint-status.yaml`

After `3-16-speak-with-piper-on-windows`, add:

```yaml
  3-17-speak-with-edge-tts-on-linux: backlog  # sprint change 2026-09-24 (edge-tts)
```

---

## 6. I/O & Edge-Case Matrix (for the story spec)

| Scenario | Input / State | Expected | Error handling |
|---|---|---|---|
| Select, not installed | Edge TTS selected, no `edge-tts` | Selection saved. Blocking "edge-tts is not installed" row. Overlay blocked | N/A |
| Installed via pipx, GUI PATH | Program only in `~/.local/bin` | Row Ready with that path. Speak works | N/A |
| Speak | `tr-TR`, voice unset, disclosure confirmed | `--voice=tr-TR-AhmetNeural` (first listed), text on stdin, 24 kHz buffer played | N/A |
| Pick voice | Emel | `SetSpeechVoice(Remote(EdgeTts), Some("tr-TR-EmelNeural"))`. Used on the next Speak | Save failure shown inline |
| No disclosure | Not yet confirmed | Refused before any process is spawned (3.6) | One notification |
| Offline / service down | Exit 1, traceback | Nothing played | One notification: "Edge TTS: ClientConnectorError: …" |
| Slow network | More than 30 s | Child killed and reaped | One notification: "took longer than 30 s" |
| Program removed after check | Spawn `NotFound` | Nothing played | One notification with the install message. The next check blocks |
| Hostile text | `--voice=x ; $(rm …)` | Spoken literally. The argv never contains it | N/A |
| Other OS | Windows | Listed. `cannot_run` "not available on this OS yet" | N/A |

---

## 7. Risks

- **The service is unofficial.** Microsoft can change or block the Read Aloud endpoint. `edge-tts` has had to follow such changes (the `Sec-MS-GEC` token, 2024). voice-me pins no version at run time. The user updates with `pipx upgrade edge-tts`. The disclosure says it may stop working.
- **Terms of use.** The service is meant for the Edge browser. voice-me does not bundle, install or call it itself, but it does automate it. This is acceptable for a free hobby app, to be revisited if voice-me is monetized (the same note as the Piper voice licence).
- **The `--list-voices` format.** The table layout is the program's, and it has changed across major versions. The parser is pinned by a fixture, and CI runs the pinned version. A format change shows as "Couldn't list Edge TTS voices", not a crash.
- **Latency is not measured.** The container's TLS proxy blocked the WebSocket. Expect roughly network round trip plus about 0.5–1 s for a short line, to be measured on the dev machine during the story.

---

## 8. Implementation Handoff

**Scope: Moderate.** One new story, one new crate, one widened exception (AD-12) and one AD-8 note.

| Who | Responsibility |
|---|---|
| Erdem | Approve E1–E7, especially **E4 (Remote + disclosure)** and the story order (E7) |
| Developer (correct-course) | Apply §5.1–§5.5 and `epic-3-context.md` |
| Developer (`bmad-build`) | Build 3.17: crate → core → deps → UI → app → CI, in that order, each with its tests |

**Success criteria:**

- On a Linux machine **without** `edge-tts`, Edge TTS can be selected. Dependencies shows the blocking "please install edge-tts" row, and the overlay is blocked.
- After `pipx install edge-tts` and a re-check, with no other change, a Turkish line is heard through the Virtual Microphone.
- `cargo test --workspace` and `cargo check --workspace --all-targets` pass on Linux and Windows CI. `voice-me-tests`' egress allowlist is unchanged.
- Only the one runner crate spawns processes, and the typed text is never on any argv.
