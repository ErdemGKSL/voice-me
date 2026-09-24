# Epic 3 Context: Never Get Stuck on Setup

<!-- Compiled from planning artifacts. Edit freely. Regenerate with compile-epic-context if planning docs change. -->

## Goal

Make voice-me work on any machine without a terminal or a wiki. The app checks for the runtime components it needs, installs them with one click where it can, and names anything it can't install along with the manual steps. The user chooses which speech backend generates their voice, local or remote, and sets that backend's options, speech language and voice in its own Settings → Backend tab. The app also says plainly when the selected backend can't run on this machine. One artefact per OS carries every backend, so every story here depends on the backend: what counts as "required" or "ready" comes from the **selected** backend, never from a fixed list. The epic also adds the stock-voice backends: eSpeak NG / Windows speech engine, Piper (the new first-run default), Azure Neural TTS and Edge TTS.

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

- The Dependency Check runs on first run and on demand. It lists only what the selected backend needs: the ONNX Runtime, the execution-provider libraries, the weight variant, the virtual mic driver, the eSpeak NG / edge-tts programs and the Piper voices. The CPU backend never reports GPU libraries, and a GPU backend never reports the Q4 weights. A remote backend is ready when it has a key (if it needs one), the settings its provider requires, and a reachable provider. It has no file list.
- Provisioning downloads only the selected backend's assets: the CPU set (~1.56 GB: runtime plus Q4 weights, 354 MB) or the GPU set (provider libraries plus FP16 weights, 1.04 GB). Never both, and nothing for a remote backend. Large downloads show progress and can resume. A failure names what went wrong. A dependency that can't be installed automatically gets a manual-steps link instead of an Install button. voice-me never runs pip.
- Fail honestly and never substitute silently. A backend that can't run here is described in words: what it needs, what the machine has, and what selecting CPU would do, with that selection offered. The "active backend" shown to the user is what was actually **acquired at session build**, not what was selected. Selecting CPU on a machine with a good GPU is a normal state and shows an informational "CPU mode" badge.
- A missing dependency that blocks speech auto-opens Settings → Dependencies. The Prompt Overlay still opens but shows an inline notice instead of taking input. Once the dependency is resolved it works again without a restart.
- Backend, device, per-backend speech language and per-backend voice all persist through `SettingsStore`. A change applies to the next Speak Action with no restart. Switching backends never rewrites another backend's language. Core checks the language against the **selected** backend's set, not a global list. An existing `speech_language` value seeds the Local and DeepInfra choices on first load.
- **Unset backend**: Piper with the voice `tr_TR-fahrettin-medium` on an OS whose build has Piper (Linux now, Windows after 3.16). Otherwise CPU with no device preference. An explicit selection is never rewritten.
- Remote backends: the user must confirm a one-time disclosure per provider before any text leaves the machine. It states exactly what is sent. Core enforces it, not the adapter. The only request allowed before confirmation is a stock-voice provider's voice list, which carries the key alone. API keys are stored in plaintext in the settings TOML, the UI says so where the key is entered, and keys are never logged. Every request has a deadline. A failure is one notification naming the provider and the reason, and nothing is retried or re-sent automatically.
- Stock-voice backends (Piper, System voice, Azure, Edge TTS) are labelled "stock voice" and never receive the Reference Voice Sample. Settings does not auto-open for a missing sample while one is selected.
- Every generation or dependency failure reaches the user as a clear notification or in-app message. Never silence.

## Technical Decisions

- **Hexagonal boundaries**: `voice-me-core` owns `AppState`, `AppEvent`, the port traits and the single domain error enum. Adapters map their own errors into that enum at the boundary. No adapter calls another adapter.
- **Capability resolution happens in core.** `voice-me-deps` detects what the machine offers: which execution providers the runtime carries, a **list** of candidate devices per provider, which assets are present, and key presence. It reports only by emitting an `AppEvent`. Core reconciles that with the user's selection into one `AppState` value (backend + device + weight variant). `voice-me-tts-onnx` and `voice-me-tts-remote` read that value. Neither calls `voice-me-deps`, and it calls neither of them.
- **One ONNX Runtime with all providers.** CI builds it from source with `--use_cuda --use_webgpu --build_shared_lib` and mirrors it as a versioned, resumable asset. Switching local backends rebuilds the session; it never restarts the process. FP16 weights run on GPU, Q4 on CPU. Sessions are built once and held. Session build is slow (~90–110 s for Chatterbox on CPU), so warm-up and "still loading" states matter.
- **Asset sources**: this repo's GitHub Releases, plus other trusted static URLs at pinned revisions (for example Hugging Face for files too big to mirror). Assets are stored in the OS cache directory owned by `voice-me-deps`. Settings and the voice sample stay in `SettingsStore`.
- **Network egress**: only `voice-me-deps` and `voice-me-tts-remote` may open sockets. A CI allowlist enforces this. The `edge-tts` child process reaches Microsoft's service, and it is spawned only while Edge TTS is selected and its disclosure is confirmed.
- **Remote providers**: `voice-me-tts-remote` implements the same `TtsPort` with one `SpeechProvider` per vendor. Cloning providers (DeepInfra, fal.ai) upload the sample once, cached by (provider, sample hash), and the UI shows the sample is held there, with a delete action. Stock-voice providers (Azure: key plus region, SSML, `riff-24khz-16bit-mono-pcm`) never get the sample. No provider type reaches core or the UI. Edge TTS is a Remote entry with no key (`needs_api_key() == false`), but it is implemented in `voice-me-tts-edge`, not `voice-me-tts-remote`.
- **Child processes**: only `voice-me-espeak` spawns processes. It is the single bounded runner for `espeak-ng` (fixed name on PATH, text on stdin, no shell, 10 s deadline; `--ipa` for Piper, never `--ipa=3`) and `edge-tts` (PATH then `~/.local/bin`, `-f -`, stdout audio, 30 s deadline). No GPL engine code is linked.
- **Piper** runs its VITS graph in-process on `ort` with the CPU provider, using phoneme IDs from the voice's `phoneme_id_map`. Voices are stored at `<cache>/piper/<key>/` with a `voice.toml`. Voice catalogs are merged in order: voice-me's own, then rhasspy (pinned), then speaches-ai. The first source wins a duplicate. Downloads are checksum-verified.
- **Audio contract**: every backend returns the shared 24 kHz mono f32 buffer. Resampling or decoding to it happens inside the TTS adapter, and playback through the virtual mic is unchanged.
- A GPU device that loses its context mid-generation must surface as an error, not play the corrupted audio. This was observed on a Maxwell GPU under NVK.

## UX & Interaction Patterns

- Settings has these tabs: Voice, Hotkey, **Backend**, **Dependencies**, General, plus a **Piper voices** management tab.
- The Backend tab works in two steps: Local or Remote, then a `Select` of backends of that kind. Local: Piper (listed first), Chatterbox (CPU plus added runtime targets), System voice. Remote: DeepInfra, fal.ai, Azure, Edge TTS. Only the selected backend's options appear: language, voice, masked key with plaintext notice, region, remote sample state, GPU device (only for GPU backends; unset means "let the backend decide"). Read-only Selected/Active lines sit beneath.
- The Dependencies tab shows dependency rows (name, a `Badge` reading ready/missing/installing that always includes the word, Install or a manual-steps link) and a speech-blocking capability row that links to the Backend tab.
- The remote disclosure is a `Dialog` shown on the first Speak Action with that provider. Declining leaves the backend selected but unusable.
- Messages are specific and human, for example "Missing: speech model files. Download now?" and never "code 3". A device that has disappeared is reported by name, and the selection reverts to the default.
- Follow the gpui-kit components and Design Guides. The theme is dark only.

## Cross-Story Dependencies

- 3.1 feeds 3.2, 3.3 and 3.4. 3.5 (selection) drives which dependency rows 3.1 shows.
- 3.6 introduces `SpeechProvider` and the disclosure gate. 3.7, 3.14 and 3.17 build on it.
- 3.8 is needed only by the GPU backends (CUDA/WebGPU) and 3.9. The CPU backend must stay fully usable without it.
- 3.10 moves the backend UI into its own tab without changing any logic, so later stories add their options there. 3.11 adds a per-backend language to every backend, including the ones added afterwards.
- 3.12 sets the pattern for child processes and stock voices. 3.15 extracts `voice-me-espeak` and moves 3.12 onto it. 3.16 extends Piper to Windows and provisions eSpeak NG there. 3.17 reuses the runner.
- These stories build on Epic 2: `TtsPort`, the Speak Action pipeline, virtual mic playback and the Prompt Overlay. Windows manual verification (3.13, 3.16) waits until the Windows app can start (Story 2.8 and tray work).
