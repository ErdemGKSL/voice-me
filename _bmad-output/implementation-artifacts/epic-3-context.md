# Epic 3 Context: Never Get Stuck on Setup

<!-- Compiled from planning artifacts. Edit freely. Regenerate with compile-epic-context if planning docs change. -->

## Goal

Make voice-me self-sufficient and honest about setup. The app detects the runtime dependencies it needs, provisions them with one click where it can, and gives manual steps where it can't. The user picks the speech backend in Settings → Backend: Local (Piper, Chatterbox on CPU/CUDA/WebGPU, or the instant system voice) or Remote (DeepInfra, fal.ai, Azure Neural TTS, Edge TTS). Each backend keeps its own options and speech language. The app always tells the truth about whether the selected backend can run on this machine. One executable per OS carries every backend, so every story here depends on the backend that is selected, not on a build variant. A user should never need a terminal, a wiki, or guesswork to get speaking.

## Stories

- Story 3.1: Detect Missing Dependencies
- Story 3.2: One-Click Provision a Missing Dependency
- Story 3.3: Be Honest When the Selected Backend Can't Run Here
- Story 3.4: Block the Overlay Gracefully on a Missing Dependency
- Story 3.5: Choose Which Backend Generates Speech
- Story 3.6: Generate Through a Remote Provider
- Story 3.7: Add fal.ai as a Second Provider
- Story 3.8: Build and Mirror the All-Provider ONNX Runtime
- Story 3.9: Choose Which GPU Runs Generation
- Story 3.10: Give Backends Their Own Settings Tab
- Story 3.11: Choose the Speech Language per Backend
- Story 3.12: Speak Instantly With eSpeak NG on Linux
- Story 3.13: Speak Instantly With the Windows Speech Engine
- Story 3.14: Generate Through Azure Neural TTS
- Story 3.15: Speak Naturally and Instantly With Piper on Linux
- Story 3.16: Speak With Piper on Windows
- Story 3.17: Speak With Edge TTS on Linux

## Requirements & Constraints

- **Dependencies follow the selected backend.** The required list is derived from the selected backend. CPU never reports GPU provider libraries. A GPU backend never reports Q4 weights. A remote backend's readiness means a key and a reachable provider, not files. Checks run on first run and on demand, and need no terminal.
- **Asset sets per backend:**
  - CPU: core ONNX Runtime plus `language_model_q4`, about 1.56 GB in total.
  - GPU: provider libraries plus `language_model_fp16`, never both sets.
  - Piper: the shared CPU runtime plus about 63 MB per voice.
  - Remote: nothing.
  - Model weights (354 MB to 1.04 GB) download with progress and can resume.
- **Fail honestly.** A backend that can't run here is named in words, with what selecting CPU would do and a one-click CPU selection. There is no silent substitution. The UI shows the backend that was actually acquired at session build, not what was requested. Choosing CPU on a machine with a GPU is a normal state ("CPU mode" badge), not an error.
- **Blocking behaviour.** While a required dependency is missing, pressing the hotkey auto-opens Settings → Dependencies naming the blocker. The overlay still opens but shows an inline notice and accepts no input. Once the blocker is resolved, the overlay works normally again without a restart.
- **Selection.** Backend, per-backend speech language, stock voice and GPU device persist through `SettingsStore`. Changes apply to the next Speak Action without a restart; a local switch rebuilds the session. When nothing is selected, the default is Piper with `tr_TR-fahrettin-medium` where this OS has Piper, otherwise CPU with no device preference. An explicit selection is never rewritten. A GPU device that has disappeared is reported and the default is used. A device that loses its context mid-generation is an error, and its audio is never played.
- **Remote providers:**
  - Before the first request to each provider, a one-time disclosure states what is sent (text, language tag, and the sample for cloning providers, or the voice for stock-voice providers). Core enforces it, not the adapter.
  - The only request allowed before the disclosure is a stock-voice provider's voice-list fetch, sent with the key alone.
  - Keys are stored in plaintext in settings, with a notice where the key is entered, and are never logged. Azure also stores its region.
  - A cloning provider uploads the sample once, keyed by (provider, sample hash), so re-recording invalidates it. The UI shows that the sample is held on the provider and offers a way to delete it.
  - Failures produce one notification naming the provider and the reason. There is no automatic retry that re-sends data.
- **Stock-voice backends** (Piper, system voice, Azure, Edge TTS) are labelled "stock voice" and never receive the Reference Voice Sample. The local ones open no socket and need no disclosure. While Piper is selected, Settings does not auto-open for a missing voice sample.
- **Speech languages are per backend.** Core validates each Speak Action's language against the selected backend's own list:
  - Local Chatterbox: Turkish and English only.
  - DeepInfra: its 23 languages.
  - eSpeak NG, Windows voices, Piper and Edge TTS: their own lists.
  - An existing `speech_language` value seeds the Local and DeepInfra choices on first load.
  - Speech language is independent of the UI language (Epic 4).
- Any failure (generation, provisioning or dependency check) reaches the user as a specific in-app message or OS notification naming the backend or dependency.

## Technical Decisions

- **Hexagonal boundaries:**
  - `voice-me-deps` owns detection, provisioning and the cache directory. It reports capability, including a list of candidate GPU devices, only by emitting `AppEvent`.
  - `voice-me-core` reconciles the user's selection with the detected capability into one `AppState` value: active backend, device and weight variant.
  - TTS adapters read `AppState`. They never call `voice-me-deps`, and `voice-me-deps` never calls them.
- **Network egress** is allowed only in `voice-me-deps` and `voice-me-tts-remote`, enforced by a CI allowlist. `hf-hub` and similar clients are banned elsewhere. The `edge-tts` child process reaches the network itself; it is spawned only while Edge TTS is selected and its disclosure is confirmed.
- **Asset sources:**
  - Runtime and weights are mirrored on this repo's GitHub Releases, versioned and resumable.
  - Assets too large to mirror use a pinned static URL, such as Hugging Face at a pinned revision.
  - Piper voices are not mirrored. They come from three catalogs merged in order: voice-me's `piper-voices/catalog.json` (SHA-256), `rhasspy/piper-voices` at a pinned revision (MD5), and `speaches-ai` (LFS SHA-256 / Git blob SHA-1). Each file is checked against its source's digest.
  - Voices are cached at `<cache>/piper/<key>/` with a `voice.toml`.
  - The VB-CABLE driver is not redistributable and is always fetched from VB-Audio.
  - eSpeak NG for Windows comes from its official release, with the URL and SHA-256 pinned.
- **One ONNX Runtime per process.** CI builds it from source with `--use_cuda --use_webgpu --build_shared_lib`, so CPU, CUDA and WebGPU all live in one library and switching between them is a session rebuild. `voice-me-tts-onnx` removes the `webgpu-probe` feature and its build-time download. CPU must stay usable without this build.
- **Backend crates, all behind `TtsPort`:**
  - `voice-me-tts-onnx`: Chatterbox, FP16 on GPU, Q4 on CPU.
  - `voice-me-tts-remote`: one `SpeechProvider` per vendor. Cloning providers are DeepInfra and fal.ai; stock-voice providers are Azure and Edge TTS. No provider-specific type reaches core or UI.
  - `voice-me-tts-piper`: VITS on `ort`, CPU execution provider only, built once per voice. Phonemes come from `espeak-ng --ipa`, never `--ipa=3`.
  - `voice-me-tts-system-linux`, `voice-me-tts-system-windows` (WinRT `SpeechSynthesizer`, never to the speaker), and `voice-me-tts-edge`.
- **Child processes.** Only `voice-me-espeak` spawns processes: `espeak-ng` and `edge-tts`. Each is resolved by fixed name (`edge-tts` also in `~/.local/bin`), text goes on stdin, no shell is used, and deadlines are 10 s (espeak) or 30 s (edge-tts). No GPL engine code is linked.
- **Audio format.** Every backend returns the shared 24 kHz mono f32 buffer. Adapters resample or decode internally: Azure requests `riff-24khz-16bit-mono-pcm`; Edge TTS returns MP3, decoded with `symphonia`.
- Errors are mapped to the single domain error enum at the adapter boundary. Generation runs on `spawn_blocking`, with one generation in flight at a time.

## UX & Interaction Patterns

- **Settings → Backend:**
  - Two steps: Local or Remote, then a backend.
  - Only the selected backend's options appear: speech language, voice picker, key and region, remote sample state, GPU device selector.
  - Read-only Selected and Active lines plus the "CPU mode" badge.
  - Piper is listed first. Edge TTS is always selectable and has no key field.
- **Settings → Dependencies** holds only dependency rows (name, a status badge that uses words, "Install" or a manual-steps link) and a speech-blocking capability row that links to the Backend tab.
- **Manual-steps rows:**
  - "eSpeak NG is not installed" with the distro's command. It is shared by the system voice and Piper.
  - "edge-tts is not installed. Please install it: pipx install edge-tts". voice-me never runs pip.
  - A Windows language with no installed voice.
- **Settings → Piper voices** lists voices, filterable by language and by search. Each row offers Download (with a progress bar), Delete and Use. A catalog that fails shows as one inline line while the others still appear. Catalogs are fetched only when the tab is opened or refreshed.
- The remote disclosure is a `Dialog`. Declining leaves the backend selected but unusable, never switched.
- Use gpui-kit components and follow the Design Guides. Status is never shown by color alone.

## Cross-Story Dependencies

- 3.1 → 3.2 → 3.3/3.4 form the dependency pipeline. 3.5 supplies the selection they derive from.
- 3.10 moves the backend UI into its own tab. 3.7, 3.9, 3.11 and 3.12–3.17 build on that tab.
- 3.6 creates the `SpeechProvider` abstraction that 3.7 and 3.14 extend. Edge TTS (3.17) reuses 3.14's stock-voice list and disclosure.
- 3.8 gates the GPU backends and 3.9.
- 3.12 sets the child-process pattern. 3.15 moves it into `voice-me-espeak`, which 3.17 reuses. 3.16 depends on 3.15.
- Windows stories (3.13, 3.16) need a CI compile and passing unit tests. Manual checks wait until the Windows app can start (Epic 2, Story 2.8 and the tray).
- Relies on Epic 2's `TtsPort`, the Virtual Microphone, notifications and the Prompt Overlay. Epic 4 adds the Turkish strings for the new copy.
