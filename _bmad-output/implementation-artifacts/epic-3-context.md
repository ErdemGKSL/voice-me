# Epic 3 Context: Never Get Stuck on Setup

<!-- Compiled from planning artifacts. Edit freely. Regenerate with compile-epic-context if planning docs change. -->

## Goal

The app finds its own missing runtime dependencies and fixes them with one click from inside its UI. Anything it cannot fix automatically is named, with short manual steps. The user chooses which backend speaks for them in a dedicated Settings → Backend tab: local Chatterbox (CPU/CUDA/WebGPU), an instant local system voice, or a remote provider with their own key. Each backend has its own options and speech language. The app is honest about whether that backend can actually run on this machine. One executable per OS carries every backend, so every story here depends on the **selected** backend rather than on a build variant. For a user who installed no toolchain and reads no docs, this is the difference between voice-me working and not working.

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

- The dependency check runs on first run and on demand. It needs no terminal and no external docs. The required list comes from the selected backend: the CPU backend never asks for GPU provider libraries or FP16 weights, and a GPU backend never asks for Q4 weights. A remote backend is ready when it has a key and a reachable provider, not when files are present.
- Provisioning fetches only what the selected backend needs. For CPU that is about 1.56 GB: the core runtime plus the Q4 weights. For a GPU backend it is the provider-enabled runtime plus the FP16 weights. It never fetches both sets, and fetches nothing for a remote backend. Large downloads report progress and can resume. A failure names the file and the reason. A dependency that cannot be automated shows inline manual steps instead of an Install button, never a link to external docs.
- **The CPU runtime is only the core ONNX Runtime shared library, with no execution-provider libraries.** It is the floor and is always shipped. It must stay fully usable whether or not the GPU runtime (Story 3.8) exists. Model weights and their pins are the same on every OS. Only the runtime library differs per platform: `libonnxruntime.so` on Linux, `onnxruntime.dll` on Windows.
- Fail honestly (NFR4). A backend that cannot run here is described in words, together with what selecting CPU would do. The app never silently switches to another backend. "CPU mode" is an informational badge, not an error. Every generation, provisioning or check failure reaches the user as a specific notification or row message.
- No installer or setup wizard (NFR6). Runtime assets are installed by the app on first run, and a finished install unblocks the overlay without a restart.
- Remote backends: before the first request, the app tells the user what is sent and asks once per provider, and core enforces this. The key is stored as plaintext in settings, the UI says so, and the key is never logged. Failures name the provider and are never re-sent automatically. Stock-voice backends never receive the Reference Voice Sample.
- Speech language is set per backend. Local Chatterbox offers only languages that need no Python normalization (Turkish, English). Remote and system backends offer their own language sets. The UI language is independent of the speech language.

## Technical Decisions

- **Runtime assets are hosted on this repo's GitHub Releases (AD-7).** Any runtime asset that is small enough and legally redistributable should be mirrored as a versioned GitHub Release asset. This includes the ONNX Runtime shared library and the Windows virtual-audio driver installer. Provisioning must support more than one trusted source. An asset may come from any stable, versioned, resumable HTTPS URL: the pinned upstream origin such as Microsoft's ONNX Runtime GitHub release or Hugging Face at a pinned revision, or this repo's mirror. The right choice is a URL that cannot change.
- **Current sources, until the mirror exists:** weights come from `onnx-community/chatterbox-multilingual-ONNX` at a pinned Hugging Face revision. The runtime comes from Microsoft's official ONNX Runtime 1.28.2 release asset. All sources live in one table in `voice-me-deps`, with URL, size and **SHA-256 pinned in code** for each asset, so a later mirror only changes URLs. Each file downloads to `.part`, is checked against its SHA-256, and gets its final name only by an atomic rename. From an archive, only the real shared library is extracted, into the deps-owned cache path the core asset vocabulary resolves.
- **Network egress is limited to two crates (AD-8):** `voice-me-deps` (asset fetches from trusted sources) and `voice-me-tts-remote` (the provider the user selected). A CI allowlist enforces this. It uses plain HTTPS GETs through `reqwest` with rustls, and no Hugging Face Hub client. `voice-me-tts-onnx`, and both system-voice crates, open no sockets. `ort` uses `load-dynamic`: the runtime is provisioned locally and resolved at run time (for example through `ORT_DYLIB_PATH` or the bundled cache path). It is never downloaded at build or run time.
- **One runtime carries every provider (AD-7, Story 3.8).** `ort` loads exactly one ONNX Runtime library per process. So the GPU path is a single CI build from source (`--use_cuda --use_webgpu --build_shared_lib`), matched to the pinned `ort` rc (2.0.0-rc.13) and mirrored as a versioned, resumable release asset. With that runtime installed, switching between CPU, CUDA and WebGPU rebuilds the session without restarting the process. Until 3.8 lands, a GPU backend's runtime row is manual and says its runtime is not available yet. Windows GPU (DirectML/D3D12) is still unresolved.
- **Backend resolution happens in core (AD-9).** It combines the user's persisted selection with the capability `voice-me-deps` detects. Deps reports only through `AppEvent` and never calls a TTS adapter. Adapters read the resolved backend, device and weight variant (FP16 on GPU, Q4 on CPU) from `AppState`. The UI shows what the session **actually acquired** when it was built, not what was requested. If nothing is selected, the backend is Piper where this OS's build has it (Linux; Windows from 3.16), otherwise CPU. An explicit selection is never rewritten.
- Settings and the key go through `SettingsStore`, in one TOML file. Runtime and weight files live in a separate OS cache directory that only `voice-me-deps` writes (AD-6).
- `voice-me-tts-remote` holds one `SpeechProvider` per vendor. Cloning providers upload the sample once and cache its id by (provider, sample hash). Stock-voice providers such as Azure take a voice name instead (AD-13). All backends return the shared 24 kHz mono f32 buffer (AD-11).
- eSpeak NG runs as a child process: found by fixed name on PATH, text passed on stdin, no shell, 10 s deadline. This is the only exception to the no-child-process rule. The Windows system voice uses WinRT `SpeechSynthesizer` writing to a stream, never to the speaker.
- **Piper (2026-09-24, `sprint-change-proposal-2026-09-24-piper.md`).** `voice-me-tts-piper` runs each voice's VITS ONNX graph on the existing `ort` with the CPU execution provider only; no Piper engine code (`piper1-gpl`, GPL-3.0; `piper-phonemize`) is linked. Phonemes come from the eSpeak NG program via `--ipa` (never `--ipa=3`: its U+200D ties are not in `phoneme_id_map`). The CLI drops clause punctuation, so split the text at `, . ! ? ; :`, phonemize each clause, and put the mark's id back after it. Ids are BOS `^`, a pad `_` after each id, EOS `$`; scales come from the voice JSON. Output is 22 050 Hz and is resampled to 24 kHz. The espeak-ng runner moves into one shared crate, `voice-me-espeak`, used by both system-linux and piper. Voices come from a curated, SHA-256-pinned table in `voice-me-deps`, fetched from `rhasspy/piper-voices` at a pinned revision and not mirrored, because licences vary (Turkish `tr_TR-dfki-medium` is CC BY-NC-SA 4.0); each voice's licence is shown in the picker.
- Never run a clean build. Build incrementally.

## UX & Interaction Patterns

- Settings → Dependencies: one row per dependency, each with a status badge (ready / missing / installing), plus Install with a progress bar, or inline "Show steps". Status is never shown by color alone. A blocking missing dependency opens this tab automatically. The overlay still opens, but shows an inline notice instead of taking input.
- Settings → Backend has two steps: Local or Remote, then a backend. Only that backend's options are shown: added runtimes, API key with the plaintext notice, sample state, GPU device, speech language and voice. The Selected and Active lines and the "CPU mode" badge are here too. Stock-voice backends are labelled "stock voice".

## Cross-Story Dependencies

- Stories 3.1, 3.2, 3.3 and 3.4 share one asset vocabulary in core, used by the check, the provisioning plan and the engine. Do not fork it.
- Story 3.8 blocks the GPU backends (3.9, and CUDA/WebGPU provisioning in 3.2). It does not block CPU. When 3.8 lands, it should only swap URLs in the pinned source table.
- Stories 3.10 and 3.11 depend on 3.5, 3.6 and 3.7. Stories 3.12–3.16 add entries to the Backend tab. 3.15 (Piper, next up) extracts `voice-me-espeak` from 3.12's crate.
- Windows items depend on Epic 2's Windows tray and hotkey work and on Story 2.8 (Windows virtual mic control surface, still unresolved) before they can be verified by hand.
