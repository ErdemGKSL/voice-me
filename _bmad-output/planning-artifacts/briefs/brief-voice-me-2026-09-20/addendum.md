# Addendum: voice-me

Supporting detail that doesn't belong in the tight brief but is worth keeping for the next stages (architecture, PRD).

## Research grounding (web research, 2026-09-20)

### Chatterbox-Multilingual V3
- Resemble AI, open source, MIT licensed (commercial use allowed).
- 0.5B-parameter Llama-backbone architecture, ~23-25 languages (English, Spanish, French, German, Japanese, Korean, Chinese, Arabic, Hindi, Turkish among others reported).
- Ships as a PyTorch model via Hugging Face/pip. Normal use requires Python + PyTorch.
- A separate "Chatterbox-Nano" variant targets CPU-only inference, reported ~3x realtime on 8 cores; the main multilingual model favors a CUDA/ROCm GPU.
- No official ONNX export or Rust/candle binding found at research time. Third-party server wrappers exist (e.g. `devnen/Chatterbox-TTS-Server`) but still run Python underneath.
- Sources: github.com/resemble-ai/chatterbox, resemble.ai (Chatterbox Multilingual V3 announcement), huggingface.co/ResembleAI/chatterbox

### Virtual microphone, per OS
- **Windows**: needs a WDM/kernel audio driver (the VB-Cable model) or a WASAPI-based virtual endpoint. Driver must be signed (EV certificate + WHQL/attestation signing) or users hit SmartScreen/driver-block friction. Highest-effort platform.
- **macOS**: a CoreAudio HAL plug-in / driver extension (BlackHole is the reference open-source implementation) installed under `/Library/Audio/Plug-Ins/HAL`, requires restarting `coreaudiod`. Distribution requires Apple notarization plus the user approving a system extension in Security settings.
- **Linux**: a PipeWire/PulseAudio null-sink (`pactl load-module module-null-sink ... media.class=Audio/Source/Virtual`) creates a virtual mic with no kernel driver and no signing — by far the lowest-friction platform.
- Implication: "single executable" is realistically "one app binary that installs/manages a signed driver or extension on first run" on Windows/macOS; Linux can be closer to literally driver-free.
- Sources: vb-audio.com/Cable, github.com/ExistentialAudio/BlackHole, wiki.archlinux.org/PipeWire, luke.hsiao.dev (PipeWire virtual mic writeup)

### GPUI
- Zed Industries' GPU-accelerated Rust UI framework, still pre-1.0 with breaking changes between releases.
- Zed itself runs on macOS, Linux, and Windows, but the Windows port has lagged noticeably ("everything is different on Windows" — The Register, Aug 2026).
- No built-in system tray or global hotkey support in upstream GPUI. A community fork, "Adabraka GPUI," reportedly adds tray/hotkey/daemon-mode — third-party, unverified for production use, worth a spike before relying on it.
- No native Python interop; calling into ML inference requires FFI (e.g. PyO3/embedded Python) or subprocess/IPC to a separate process.
- Sources: gpui.rs, github.com/zed-industries/zed (crates/gpui), theregister.com (Aug 2026 piece on Zed's Windows port)

### Competitive landscape
- **Voicemod / Voice.ai**: real-time voice-changing and cloning, routed through their own virtual audio device — closest analogs, but built around live voice transformation, not typed-text-to-speech.
- **VoxBooster**: markets "no kernel driver required" plus local AI voice cloning and a hotkey soundboard, claims sub-150ms latency — notable specifically because it claims to sidestep the driver-signing problem; worth investigating how, if this project hits the same wall.
- **Vozard, EaseUS VoiceWave, MorphVOX Pro, Clownfish**: preset-based voice changers/soundboards for Discord/OBS/games, generally not built around cloned-voice TTS from typed text.
- No tool found doing exactly "type text → your own cloned voice → virtual mic, positioned for gaming."
- Sources: voxbooster.com/blog/voicemod, topai.tools/alternatives/voicemod

## Architecture options considered (not decided — for bmad-architecture)

1. **Bundled/embedded Python sidecar (recommended starting point in the brief)** — GPUI shell manages a `python-build-standalone` runtime + Chatterbox as a local subprocess over IPC. Pro: no ONNX/Rust port needed, ships fastest. Con: larger download size, an extra moving process to manage/update, still "Python under the hood" even if invisible to the user.
2. **Full Rust-native inference (ONNX/candle port of Chatterbox)** — Pro: true single-binary, no Python anywhere, likely much smaller footprint. Con: no official export path exists yet; would be a real, unscoped engineering project on its own, likely out of reach for a first version of a hobby project.
3. **User-installed system Python, app just calls it** — Pro: simplest to build. Con: reintroduces exactly the "user has to set up dependencies manually" friction the brief explicitly wants to avoid; rejected direction, kept here for the record.

## OS rollout ordering (decided)

v1 targets Linux and Windows both; macOS is explicitly deferred past v1 (no notarization/system-extension work scoped yet). Development order: Linux first (least platform risk, validates the end-to-end flow), then Windows (signed driver work) — applied pragmatically, i.e. work follows whichever machine is actually in front of the developer at the time rather than a hard phase gate.

## Feature ideas — fuller brainstorm (unfiltered, for future prioritization)

Beyond the "Possible Future Features" shortlist in the brief:
- Text history/log of recently spoken lines, replayable
- Import/export of voice profiles (multiple reference voices, e.g. different "characters")
- Community-shared (opt-in) phrase packs, kept local-only unless explicitly shared
- Visual waveform/mic-level indicator while "speaking" so the user has feedback that output is live
- Keyboard-only operation throughout (no mouse needed) to fit the mid-game use case
- A "practice/preview" mode that plays the generated audio back to the user's own speakers before it's ever sent to the virtual mic, for the first few uses while trust in the voice clone builds
- Configurable profanity/safety filter, since output is voiced audio heard by others in real time
