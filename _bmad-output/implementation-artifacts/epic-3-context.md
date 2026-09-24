# Epic 3 Context: Never Get Stuck on Setup

<!-- Compiled from planning artifacts. Edit freely. Regenerate with compile-epic-context if planning docs change. -->

## Goal

The app finds its own missing runtime dependencies and fixes them with one click from inside its UI. Anything it cannot fix automatically is named, with short manual steps. The user picks the backend that speaks for them in a dedicated Settings → Backend tab. The local choices are Piper neural stock voices, Chatterbox in the user's cloned voice (CPU/CUDA/WebGPU), and an instant system voice. The remote choices are DeepInfra, fal.ai and Azure, each with the user's own key. Each backend has its own options and speech language, and the app says plainly whether that backend can actually run on this machine. One executable per OS carries every backend, so every story here depends on the **selected** backend, not on a build variant. For a user who installed no toolchain and reads no docs, this decides whether voice-me works at all.

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

## Requirements & Constraints

- The dependency check runs on first run and on demand, with no terminal and no external docs. The required list comes from the selected backend. CPU Chatterbox never asks for GPU provider libraries or FP16 weights, and a GPU backend never asks for Q4 weights. Piper needs the shared CPU runtime, the selected voice and `espeak-ng`, and nothing from Chatterbox. A remote backend is ready when it has its key (plus region and voice for Azure), not when files are present.
- Provisioning fetches only what the selected backend needs. CPU Chatterbox needs about 1.56 GB (core runtime plus Q4 weights). Piper needs about 72 MB (one ~63 MB voice plus the 8.7 MB runtime). Remote backends need nothing. Large downloads show progress and can resume. A failure names the file and the reason. A dependency that cannot be automated shows inline manual steps instead of an Install button.
- The CPU runtime is the core ONNX Runtime shared library alone, with no execution-provider libraries. It is always shipped and must work whether or not the GPU runtime exists. Only the runtime library differs per OS (`libonnxruntime.so` on Linux, `onnxruntime.dll` on Windows).
- Failures are honest. A backend that cannot run here is described in words, together with what selecting CPU would do. The app never silently switches to another backend. "CPU mode" is an informational badge. Every generation, provisioning or check failure reaches the user as one specific notification or row message that names the backend or provider.
- There is no installer. A finished install unblocks the overlay without a restart.
- Remote: before the first request, the app shows once per provider what will be sent, and core enforces this. Azure's voice list, which is sent with the key alone, is the only request allowed before that confirmation. Keys are stored as plaintext in settings, the UI says so, and keys are never logged. Failed requests are never re-sent automatically. Stock-voice backends (Piper, system voice, Azure) never receive the Reference Voice Sample and are labelled "stock voice".
- Speech language is set per backend. Local Chatterbox offers only Turkish and English. Every other backend offers its own set: Piper offers the locales in its voice table, and system and remote backends offer their engine's or provider's languages. The UI language is independent of the speech language.

## Technical Decisions

- **Asset sources.** Any asset may come from a stable, versioned, resumable HTTPS URL: this repo's GitHub Releases mirror, or a pinned upstream such as Microsoft's ONNX Runtime 1.28.2 release or a Hugging Face repo at a pinned revision. All sources sit in one table in `voice-me-deps`, with the URL, size and SHA-256 pinned in code for each. Each download goes to `.part`, is checked against its SHA-256, and gets its final name only by an atomic rename. From an archive, only the real shared library is extracted, into the deps-owned cache.
- **Network egress.** Only `voice-me-deps` and `voice-me-tts-remote` may open sockets, and a CI allowlist enforces this. They use plain `reqwest`/rustls, with no Hugging Face Hub client. `voice-me-tts-onnx`, `voice-me-tts-piper`, `voice-me-espeak` and both system-voice crates open no sockets. `ort` (2.0.0-rc.13) uses `load-dynamic` and loads a runtime provisioned locally. It never downloads one at build or run time.
- **One runtime for every provider (3.8).** A process loads exactly one ONNX Runtime library. The GPU path is therefore a single CI build from source (`--use_cuda --use_webgpu --build_shared_lib`), mirrored as a release asset. Switching between CPU, CUDA and WebGPU then rebuilds the session without restarting the process. Until 3.8 lands, a GPU backend's runtime row is manual. Windows GPU (DirectML/D3D12) is unresolved.
- **Backend resolution is in core.** Core combines the persisted selection with the capability that deps detects. Deps reports only through `AppEvent` and never calls an adapter. Adapters read the resolved backend, device and weight variant (FP16 on GPU, Q4 on CPU) from `AppState`. The UI shows what the session actually acquired, not what was requested. **If nothing is selected, the backend is Piper where this OS's build has it (Linux; Windows from 3.16), otherwise CPU Chatterbox.** An explicit selection is never rewritten.
- Settings and keys go through `SettingsStore` (one TOML file; Azure's region is stored beside its key). Runtime, weight and voice files live in an OS cache directory that only `voice-me-deps` writes to.
- **Remote.** `voice-me-tts-remote` has one `SpeechProvider` per vendor, and no provider-specific type reaches core or UI. Cloning providers (DeepInfra, fal.ai) upload the sample once and cache its id by (provider, sample hash). Stock-voice providers (Azure) take a voice name. Azure requests SSML with `riff-24khz-16bit-mono-pcm`. Every backend returns the shared 24 kHz mono f32 buffer.
- **eSpeak NG** is the only child process allowed. It is run in exactly one crate, `voice-me-espeak`: looked up by fixed name on PATH, text on stdin, no shell, 10 s deadline. 3.15 extracts this crate from `voice-me-tts-system-linux`, which then uses it with unchanged behaviour. The Windows system voice uses WinRT `SpeechSynthesizer` and writes to a stream, never to the speaker.
- **Piper.** `voice-me-tts-piper` runs each voice's VITS ONNX graph in-process on the existing `ort`, CPU execution provider only. No Piper engine code (`piper1-gpl`, `piper-phonemize`) and no libespeak-ng is linked. Phonemes come from `espeak-ng --ipa`. Never use `--ipa=3`, because its U+200D ties are not in `phoneme_id_map`. The CLI drops clause punctuation, so the adapter splits the text at `, . ! ? ; :`, phonemizes each clause, and puts the mark's id back after it. Ids run BOS `^`, a pad `_` after each id, then EOS `$`. The inputs are `input`, `input_lengths` and `scales`, with the scales taken from the voice JSON. The output is 22 050 Hz and is resampled to 24 kHz. The session is built once per selected voice and held (a rebuild takes about 1.3 s). Voices come from a curated table in `voice-me-deps` (voice id, locale, quality, URL at a pinned `rhasspy/piper-voices` revision, size, SHA-256 of both files, licence), with `tr_TR-dfki-medium` first. Voices are not mirrored because their licences vary; the Turkish voice is CC BY-NC-SA 4.0. There is no GPU path and no live `voices.json` fetch. A test fixture pins the reference phoneme sequence for the Turkish lines.
- Never run a clean build. Build incrementally.

## UX & Interaction Patterns

- Settings → Dependencies has one row per dependency, each with a status badge (ready / missing / installing), then either Install with a progress bar or inline "Show steps". Status is never shown by color alone. A missing dependency that blocks speech opens this tab automatically. The overlay still opens, but shows an inline notice instead of taking input. The capability row that blocks speech links to the Backend tab.
- Settings → Backend: choose Local or Remote first, then a backend. The Local list has "Piper — natural, instant" first, then "Chatterbox — your voice" and "System voice — instant". The Remote list has DeepInfra, fal.ai and Azure. Only the selected backend's options are shown: added runtimes, GPU device, API key with the plaintext notice, region, sample state, speech language and voice. The Piper voice picker shows each voice's size and licence. Azure's picker says the speech will be in a Microsoft voice. The Selected and Active lines and the "CPU mode" badge live here. While a stock-voice backend is selected, a missing Reference Voice Sample does not open Settings automatically.

## Cross-Story Dependencies

- 3.1–3.4 share one asset vocabulary in core, used by the check, the provisioning plan and the engine. Do not fork it.
- 3.8 blocks the GPU backends (3.9, and CUDA/WebGPU provisioning in 3.2) but never CPU or Piper. When it lands, only the pinned URLs should change.
- 3.10/3.11 build on 3.5–3.7. 3.12–3.16 add entries to the Backend tab. 3.12 and 3.14 are built.
- Build order: **3.15 Piper on Linux next**, then 3.13, 3.16, then 3.7, 3.8 and 3.9.
- Windows stories (3.13, 3.16) must compile and pass their unit tests on CI's Windows job. They cannot be checked by hand until Epic 2's Windows tray and hotkey work and Story 2.8 (the Windows virtual mic) are done. In 3.16, deps provisions eSpeak NG on Windows with one click from its official release (pinned URL and SHA-256).
