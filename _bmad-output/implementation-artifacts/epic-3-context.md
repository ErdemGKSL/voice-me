# Epic 3 Context: Never Get Stuck on Setup

<!-- Compiled from planning artifacts. Edit freely. Regenerate with compile-epic-context if planning docs change. -->

## Goal

Nobody should hit a cryptic crash because a runtime library, model file, driver or key is missing. This epic makes the app detect what the *selected* backend needs, fix what it can with one click, and name in plain words anything it can't. It also gives the user the choice that makes this meaningful: which backend speaks for them. Backends are local (Chatterbox in the user's cloned voice on CPU/CUDA/WebGPU, or an instant stock system voice: eSpeak NG on Linux, the Windows speech engine on Windows) or remote with the user's own key (cloning providers DeepInfra and fal.ai, and Azure Neural TTS with stock voices). All of this is configured in a dedicated Settings → Backend tab, where each backend keeps its own speech language. The app stays honest throughout about what was selected, what can run here, and what was actually acquired.

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

## Requirements & Constraints

- **Backend-aware, never variant-aware.** One executable per OS carries every backend. Required dependencies derive from the selected backend: CPU never reports GPU provider libs, a GPU backend never reports Q4 weights, a remote backend's readiness is a key (and for Azure a region and voice) plus a reachable provider, and the system voice's readiness is the engine plus an installed voice for the chosen language. The old download-time variant model was reversed and must not be implemented.
- **Truth over silent fallback.** The UI reports what was actually acquired at session build, never just what was selected. A backend that can't run here is named in words, with what selecting CPU would do and a one-click way to do it. Never substitute a backend, voice or language silently. CPU on a good-GPU machine is a normal state ("CPU mode" badge), not an error.
- **Self-sufficient.** No terminal or external docs. Non-automatable items (e.g. `espeak-ng`, a system package; a missing Windows language voice) get short manual steps instead of Install. Resolving a blocker restores normal behaviour without restart.
- **Provisioning scale.** Fetch only the selected backend's assets: about 1.56 GB for CPU (runtime + 354 MB Q4 LM), or GPU provider libs + 1.04 GB FP16 LM; nothing for remote or system-voice backends. Weight downloads show progress and resume.
- **Remote disclosure.** Before any text leaves the machine, a one-time per-provider confirmation, enforced by core, states what is sent. Cloning providers get the text, language tag and Reference Voice Sample. Azure gets the text, language and voice name, and the dialog says speech will be in a Microsoft voice, not the user's. The only request allowed before that is Azure's voice list, which carries the key alone.
- **Keys.** Plaintext in settings TOML with a notice at the entry field; redacted from logs. Azure's region is stored beside its key. Provider failures (no/rejected key, timeout, provider error) produce one notification naming the provider and reason, with no automatic re-send.
- **Speech language is per backend**, independent of the UI language. Local Chatterbox: `tr`, `en` only. DeepInfra: its model's 23 languages. fal.ai: decided in 3.7. Azure: the locales in its voice list. System voice: the engine's own list, with a voice picker when a language has several voices. Core validates the Speak Action's language against the selected backend's set, not a global list.

## Technical Decisions

- **Hexagonal boundaries.** Core owns `AppState`, ports, the backend-resolution logic and the disclosure gate. `voice-me-deps` reports capability only via `AppEvent`. TTS adapters read the resolved backend, device and weight variant from `AppState` and never call `voice-me-deps`.
- **Persisted selection (through `SettingsStore`).** Backend, GPU device (unset = let backend decide), and per-backend speech language plus voice for stock-voice backends. Switching backends never rewrites another backend's choice. Unset backend = CPU. Migration: an existing single `speech_language` seeds the Local and DeepInfra entries. Azure defaults to `tr-TR` with no voice (a speech-blocking state). Cover migration with a round-trip test.
- **One ONNX Runtime, every provider.** `ort` loads exactly one `libonnxruntime` per process. CI builds it from source with CUDA and WebGPU and mirrors it on this repo's GitHub Releases, so switching local backends is a session rebuild. The `webgpu-probe` feature and its build-time download go away. CPU stays usable without it.
- **`voice-me-tts-remote`.** One `SpeechProvider` per vendor, in two shapes. *Cloning* providers upload the sample once, keyed by (provider, sample hash), reference it by id, and show a user-visible "held on provider" state with delete. *Stock-voice* providers (Azure) take a voice name and never touch the sample. No provider-specific type reaches core or UI. Azure details: `POST https://{region}.tts.speech.microsoft.com/cognitiveservices/v1` with `Ocp-Apim-Subscription-Key`, SSML (`<voice name>` + `xml:lang`), output `riff-24khz-16bit-mono-pcm` (no resampling). The voice list comes from `GET …/voices/list`, fetched on demand when the picker opens and cached per session.
- **System voices are separate per-OS crates** behind `TtsPort`, wired in the composition root. Core gets a `SystemVoice` local backend kind that `resolve_backend` must not treat as an ONNX target.
  - `voice-me-tts-system-linux` runs `espeak-ng -v <voice> --stdout` as a child process. The binary is found by fixed name on PATH, text goes on stdin (never argv), no shell, 10 s deadline. It is not linked, because eSpeak NG is GPL-3.0. It parses WAV at 22 050 Hz and resamples to 24 kHz. Voices come from parsing `espeak-ng --voices`. This is the only exception to "no child process", which still binds Chatterbox.
  - `voice-me-tts-system-windows` uses WinRT `SpeechSynthesizer::SynthesizeTextToStreamAsync` (never SAPI `Speak`, which plays to the speaker), on a thread owning its COM apartment. Voices come from `AllVoices`. Scope is compile and unit tests on CI's Windows job; manual verification waits on the Windows tray.
- **Audio contract.** Every backend yields the shared 24 kHz mono f32 buffer.
- **Network egress.** Only `voice-me-deps` and `voice-me-tts-remote` may open sockets, enforced by a CI allowlist. Azure lives in `voice-me-tts-remote`, so the allowlist is unchanged. System-voice crates are offline.
- **Generation failure.** A GPU losing its context mid-generation is an error. Its corrupted audio must never play.

## UX & Interaction Patterns

- Settings has five tabs: Voice, Hotkey, Backend, Dependencies, General.
- **Settings → Backend is two-step.** Choose Local or Remote first, then a backend. Local has "Chatterbox — your voice" (CPU plus added runtime targets) and "System voice — instant". Remote has DeepInfra, fal.ai and Azure. Only the selected backend's options appear: speech language `Select`, masked key + plaintext notice, Azure region + voice picker ("Speech will be in this Microsoft voice, not yours."), remote sample state, GPU device `Select` (hidden for CPU/remote). Below everything, read-only Selected / Active lines and the "CPU mode" badge. Stock-voice backends are tagged "stock voice" wherever they are selected.
- **Settings → Dependencies** keeps only the dependency rows (name, ready/missing/installing badge with words, Install or manual steps) and the speech-blocking capability row, which links to the Backend tab.
- **Blocking states** (missing dependency, no key, Azure with no voice, eSpeak NG not installed, no system voice for the language) auto-open Settings → Dependencies when the hotkey is pressed. The overlay still opens but shows an inline notice instead of accepting input.
- Failures reach the user as OS notifications naming the short reason. The overlay never reappears automatically.

## Cross-Story Dependencies

- Stories 3.1–3.6 are built (in review). Story 3.10 moves their UI into the Backend tab with no logic change.
- Build order: 3.10, then 3.11, 3.12, 3.13 and 3.14, then 3.7, 3.8 and 3.9. Story 3.11 needs 3.10's tab. 3.12–3.14 need 3.10 and 3.11 (per-backend language and voice pickers). 3.7 and 3.9 target the Backend tab. 3.9's GPU choice and real CUDA/WebGPU switching depend on 3.8's runtime.
- 3.12–3.14 reuse 3.4's speech-blocking gate for their missing-engine, voice, key or region states. 3.14 extends 3.6's `SpeechProvider` abstraction with the stock-voice shape.
- Epic 4 (UI language) must stay independent of per-backend speech language.
- The Windows system voice (3.13) can't be manually verified until Story 2.8 and the Windows tray are done.
