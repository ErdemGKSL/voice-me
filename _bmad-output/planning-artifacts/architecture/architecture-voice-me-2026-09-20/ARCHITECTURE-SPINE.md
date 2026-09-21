---
name: 'voice-me'
type: architecture-spine
purpose: build-substrate
altitude: feature
paradigm: 'Hexagonal / Ports-and-Adapters'
scope: 'voice-me v1 - the whole product per prd.md: Voice Setup, background/tray operation, global hotkey + Prompt Overlay, TTS via in-process Chatterbox-Multilingual V3 ONNX inference, Virtual Microphone output on Linux and Windows, in-app dependency management, TR/EN UI, gpui-kit look & feel'
status: final
created: '2026-09-20'
updated: '2026-09-21'
binds: ['FR-1', 'FR-2', 'FR-3', 'FR-4', 'FR-5', 'FR-6', 'FR-7', 'FR-8', 'FR-9']
sources: ['_bmad-output/planning-artifacts/prds/prd-voice-me-2026-09-20/prd.md', '_bmad-output/planning-artifacts/prds/prd-voice-me-2026-09-20/addendum.md']
companions: []
---

# Architecture Spine — voice-me

## Design Paradigm

**Hexagonal / Ports-and-Adapters.** `voice-me-core` is the hexagon: platform-agnostic domain types, `AppState`, `AppEvent`, and the full set of port traits — `HotkeyPort`, `VirtualMicPort`, `TrayPort`, `TtsPort`, `DependencyProvisioningPort`, and `SettingsStore`. Everything else is an adapter plugged into a port — driving adapters trigger the app (the UI, per-OS hotkey listeners); driven adapters are what the app calls out to (virtual mic playback, tray presence, the in-process TTS engine, dependency provisioning). `SettingsStore` is the one exception: its implementation lives inside `voice-me-core` itself rather than a separate adapter crate (see AD-6) since local file I/O via `directories` doesn't vary by OS the way hotkey/audio/tray do. No adapter depends on another adapter; all cross-adapter communication passes through `voice-me-core`.

```mermaid
graph TD
  app[voice-me-app] --> core[voice-me-core]
  app --> ui[voice-me-ui]
  app --> hkl[voice-me-hotkey-linux]
  app --> hkw[voice-me-hotkey-windows]
  app --> aul[voice-me-audio-linux]
  app --> auw[voice-me-audio-windows]
  app --> trl[voice-me-tray-linux]
  app --> trw[voice-me-tray-windows]
  app --> tts[voice-me-tts]
  app --> deps[voice-me-deps]

  ui --> core
  ui --> i18n[voice-me-i18n]
  hkl --> core
  hkw --> core
  aul --> core
  auw --> core
  trl --> core
  trw --> core
  tts --> core
  deps --> core

  tests[voice-me-tests] -.exercises.-> core
  tests -.exercises.-> ui
  tests -.exercises.-> tts
  tests -.exercises.-> deps
```

## Invariants & Rules

### AD-1 — Hexagonal paradigm; core has zero adapter dependencies

- **Binds:** all crates
- **Prevents:** business logic leaking into a platform-specific or UI crate, and adapters reaching into each other directly
- **Rule:** `voice-me-core` depends on no other `voice-me-*` crate. Every adapter crate depends on `voice-me-core` for its port trait and domain types, never on a sibling adapter. All cross-adapter effects flow through `AppState`/`AppEvent` (AD-3).

### AD-2 — Per-OS adapters are separate crates, not `cfg`-gated modules

- **Binds:** FR-2 (background/tray), FR-3 (hotkey), FR-6 (virtual mic)
- **Prevents:** one crate accumulating both platforms' dependencies, and Linux-only work pulling in Windows-only build requirements
- **Rule:** each OS-specific capability ships as its own crate — `voice-me-hotkey-{linux,windows}`, `voice-me-audio-{linux,windows}`, and `voice-me-tray-{linux,windows}` (system tray APIs differ per OS just as much as hotkey/audio, and neither GPUI nor gpui-kit provide one — see Deferred) — each implementing the matching `voice-me-core` port trait (`HotkeyPort`, `VirtualMicPort`, `TrayPort`). `voice-me-app` selects the right crates per target via `[target.'cfg(...)'.dependencies]` in `Cargo.toml`. A future macOS adapter set is additive, not a rewrite.

### AD-3 — Single `AppState`, single `AppEvent` channel

- **Binds:** FR-1, FR-2, FR-3, FR-4, FR-5, FR-7, FR-8
- **Prevents:** divergent per-adapter copies of settings/state, and each adapter inventing its own callback or polling mechanism to reach the UI
- **Rule:** `voice-me-core` owns the one `AppState` (hotkey binding, active Reference Voice Sample, UI language, selected Virtual Microphone device). No adapter mutates it directly — only through `voice-me-core` use-case functions. Every adapter signals the app through one shared `AppEvent` enum over one channel; every adapter (including `voice-me-deps`, per AD-9) holds only the sender half. `voice-me-app` (the composition root) owns the single receiver and is the one place that pumps `AppEvent`s into `voice-me-core`'s use-case functions and forwards UI-relevant results to `voice-me-ui`, which turns them into GPUI entity updates via `cx.spawn`. No adapter, including `voice-me-ui`, holds or reads the receiver directly.

### AD-4 — One domain error type; adapters map into it at the boundary

- **Binds:** all crates
- **Rule:** `voice-me-core` defines a `thiserror`-based error enum. Each adapter converts its own failures (inference/session error, driver error, I/O error) into that enum at the port boundary. `voice-me-app` aggregates with `anyhow` at the composition root. No adapter-specific error type crosses a crate boundary.
- **Prevents:** ad-hoc error types leaking into the UI or into other adapters.

### AD-5 — Tokio runs alongside GPUI's own executor, bridged deliberately

- **Binds:** FR-5 (TTS generation), FR-7 (dependency provisioning)
- **Prevents:** each async-needing adapter picking a different (or no) runtime, assuming GPUI's own executor is Tokio-compatible, and — since AD-12 moved inference in-process — a multi-second synchronous ONNX run blocking the UI or an async executor thread
- **Rule:** `voice-me-tts` (ONNX inference, AD-12) and `voice-me-deps` (HTTP downloads) run their work on Tokio. `voice-me-deps`' downloads are genuinely async I/O; `voice-me-tts`' generation is CPU/GPU-bound and therefore runs on `tokio::task::spawn_blocking` (one in-flight generation at a time — see AD-10), never directly on an async worker. GPUI's own `cx.spawn`/`cx.background_spawn` is reserved for UI-side async work. The bridge between them is **one** small in-house reimplementation of Zed's `gpui_tokio` pattern (a `Global` holding a `tokio::runtime::Handle`, plus a `Tokio::spawn` wrapping `cx.background_spawn`), owned by `voice-me-core` and exposed to both `voice-me-tts` and `voice-me-deps` as a single shared utility — not reinvented per adapter. **Not** a direct dependency on Zed's `gpui_tokio` crate: it's a workspace-internal path dependency inside `zed-industries/zed`, not independently publishable, and pulling it in would drag a second, type-incompatible `gpui` instance alongside the one `gpui-kit` already pins (see Stack). `[ADOPTED pattern, in-house implementation]` — the bridging *technique* is confirmed as Zed's own; the *crate* is not reusable as-is.

### AD-6 — Settings and user assets live in core-owned locations, never touched directly by adapters

- **Binds:** FR-1, FR-3, FR-8
- **Prevents:** an adapter reading or writing the settings file (or the Reference Voice Sample audio) directly and drifting from what `voice-me-core` believes is true; two designs disagreeing on whether the active Reference Voice Sample is a path or an ID+manifest
- **Rule:** one TOML settings file at the OS config directory, and Reference Voice Sample audio files at the OS data directory — both resolved via the `directories` crate — behind the `SettingsStore` port, implemented inside `voice-me-core` itself (not a separate adapter crate — see Design Paradigm). `AppState`'s active Reference Voice Sample field is a `PathBuf` into that data directory, not an ID with a separate manifest/index. Adapters (including `voice-me-ui`'s recorder) hand recorded/imported audio to `voice-me-core` to store; they never write to the data directory themselves and read settings only through `AppState`. Large remotely-fetched runtime assets (the ONNX Runtime shared library and its execution providers, the ONNX model weights, the Windows driver installer) are **out of `SettingsStore`'s scope** — they live in a separate OS *cache* directory (also via `directories`), owned and written only by `voice-me-deps` behind `DependencyProvisioningPort`, never through `SettingsStore`.

### AD-7 — Open source, GitHub-native CI/release, first-party dependency hosting, one release artefact per backend variant

- **Binds:** FR-7 (dependency management), overall distribution
- **Prevents:** `voice-me-deps` pulling runtime assets from ad-hoc third-party URLs with no versioning or trust boundary
- **Rule:** GitHub Actions builds Linux and Windows separately (no macOS job in v1); binaries publish to GitHub Releases of this repo. **v1 does not ship one executable that discovers its backend at run time — it ships a matrix of backend variants and the user picks one when downloading** (product-owner direction, 2026-09-21). The variant axis is the inference backend, crossed with the OS axis:

  | Variant | ONNX Runtime it needs | Language-model weights | Notes |
  | --- | --- | --- | --- |
  | `cpu` | `onnxruntime-linux-x64-1.28.2.tgz` core library, 8.7 MB, no provider libs | `language_model_q4` (354 MB) | The floor; always buildable, always shipped, the only variant Story 2.5 verified |
  | `cuda` | `onnxruntime-linux-x64-gpu_cuda12` (404 MB) or `gpu_cuda13` (230 MB), incl. `libonnxruntime_providers_cuda.so` | `language_model_fp16` (1.04 GB) | Needs an sm_60-or-newer NVIDIA GPU; unverified — no such hardware on the dev machine |
  | `local-webgpu` | **No Microsoft release asset exists** — requires ONNX Runtime built from source with `--use_webgpu --build_shared_lib`, mirrored here per this AD | `language_model_q4` or `_fp16` per device `shaderFloat16` | Vendor-neutral GPU path over Vulkan/D3D12; the build is CI's job, not the user's |
  | remote (later) | none | none | A second `TtsPort` adapter, not an `ort` configuration. Gated on the AD-8 decision below |

  A variant name is part of the release artefact's name so a downloaded binary is self-describing, and the binary reports its own variant in Settings — a user who downloaded the wrong one must be able to see that, not just fail. The variant fixes which `ort` features are compiled in; it does **not** fix which device is used within that backend (see AD-9). Any remotely-fetched runtime asset that (a) is small enough for a GitHub Release asset and (b) voice-me is legally permitted to redistribute (the ONNX Runtime shared library, the Windows Virtual-Audio-Driver installer — both open source and small) is mirrored there — one trusted, versioned source `voice-me-deps` pulls from. Chatterbox's ONNX weights (AD-12) are covered by this rule as of 2026-09-21: they are MIT-licensed — therefore redistributable — and, in the variants v1 ships, every individual file is under GitHub Releases' 2 GB per-asset limit (`speech_encoder.onnx_data` 592 MB, `conditional_decoder.onnx_data` 534 MB, `embed_tokens.onnx_data` 68 MB, `language_model_q4.onnx_data` 354 MB / `language_model_fp16.onnx_data` 1.04 GB). The FP32 `language_model.onnx_data` (2.08 GB) is the one variant that does **not** fit as a single release asset and is therefore not mirrored — v1 ships FP16 (GPU) and Q4 (CPU) instead. `voice-me-deps`'s `DependencyProvisioningPort` still supports fetching from more than one trusted source (this repo's releases, and/or the Hugging Face origin `onnx-community/chatterbox-multilingual-ONNX`) rather than hard-coding "GitHub Releases only" — the origin stays a fallback, not the primary.

### AD-8 — No network egress outside `voice-me-deps`'s declared fetches, in every locally-inferring variant

- **Binds:** all crates
- **Prevents:** any crate silently phoning home — a cloud fallback added to `voice-me-tts`, telemetry slipped into `voice-me-ui`, or any other undeclared network call — which would break the PRD's local-only/no-telemetry guarantee without anyone deciding it
- **Rule:** the only network calls in the whole app are `voice-me-deps` fetching versioned assets from its trusted sources (AD-7). Every other crate is offline: since AD-12 `voice-me-tts` performs no IPC and opens no socket at all — it reads model files from the cache directory `voice-me-deps` owns. The `ort` crate must be configured to load a locally provisioned ONNX Runtime (no download-at-build-or-run behaviour), and Hugging Face Hub clients (`hf-hub` and friends) are banned outside `voice-me-deps` by the same CI check. Enforced with a CI check (a `cargo-deny` ban list, or a workspace-wide `grep` for HTTP-client crates outside `voice-me-deps`'s `Cargo.toml`) rather than relying on code-review discipline alone — solo development is exactly the setting where a silent violation goes unnoticed without an automated check. Adding any other network call is an architectural change, not a local one — it revisits this AD.
- **The planned remote-generation variant is the one declared exception, and it is not yet granted.** A backend that generates speech through a third-party API (product-owner direction, 2026-09-21) necessarily sends the user's typed text, and probably their Reference Voice Sample, off the machine. That is a change to what this product promises, not merely to where a computation runs, so it is blocked on an explicit PRD decision — see PRD Open Question 7 — covering which variants may reach the network, what the UI discloses before anything is sent, and whether the local-only claim is rewritten or scoped to the local variants. Until that decision exists, the rule above holds without exception. When it is granted, the permission belongs to a named remote `TtsPort` adapter in its own variant, never to `voice-me-tts`, and the CI check must keep failing any HTTP-client dependency that appears anywhere else.

### AD-9 — The active backend is a build variant, a detected capability and a user choice, resolved in core

- **Binds:** FR-5 (TTS generation), FR-7 (dependency check / GPU fallback)
- **Prevents:** `voice-me-tts` querying `voice-me-deps` directly for GPU availability, which would violate AD-1's no-adapter-to-adapter rule; and — since AD-7 made the backend a download-time choice — the three things that now decide which device runs inference silently disagreeing, so a user on a `cuda` build with no NVIDIA GPU gets an inference-engine error instead of a sentence telling them they downloaded the wrong variant
- **Rule:** three inputs decide which device runs inference, and `voice-me-core` is where they are reconciled — no adapter resolves this on its own.
  1. **The build variant (AD-7)** is fixed at compile time and is the outer bound: a `cpu` build has no GPU execution provider compiled in and cannot acquire one at run time. It is a compile-time constant the binary can report about itself.
  2. **Detected capability** is still `voice-me-deps`' job, exactly as before: as part of its Dependency Check it reports what this machine actually offers — which execution providers are usable, and, for GPU variants, the **list of candidate devices** rather than a single verdict, since a machine can have several and Story 2.5 found `ort::ep::WebGPU::with_device_id` does not choose one on its own. It reports **only** by emitting on the shared `AppEvent` channel (AD-3), never by calling a `voice-me-core` use-case directly.
  3. **The user's selection**, persisted through `SettingsStore` (AD-6) like any other setting: on a GPU variant the user picks which of the detected devices to use. Unset means "let the backend decide", which is the only behaviour a `cpu` build ever has.

  `voice-me-app` routes the detection event into `voice-me-core`, which records the resolved outcome in `AppState` as one value — the active backend plus, where it applies, the selected device. `voice-me-tts` reads that and picks the execution provider **and** the language-model weight variant (FP16 on GPU, Q4 on CPU — see AD-12). It never calls `voice-me-deps`, and `voice-me-deps` never calls `voice-me-core`.

  **Conflicts resolve toward telling the truth, never toward a silent fallback.** A GPU variant on a machine with no usable device, or a persisted device that has since disappeared, is a state the UI names in words — including which variant this binary is and which one would fit — rather than quietly running on CPU and leaving the user to wonder why it is slow. Story 2.5 is the cautionary case: a misconfigured build reported `provider: webgpu:0` while running entirely on CPU, and only ONNX Runtime's node-placement log revealed it. Whatever the UI claims about the active backend must come from what was actually acquired, not from what was requested.

### AD-10 — Speak Action sequencing and inference-session lifecycle are explicit contracts

- **Binds:** FR-4 (Prompt Overlay dismiss), FR-5 (TTS generation)
- **Prevents:** the Prompt Overlay's "closes instantly on Enter" guarantee silently depending on how fast TTS happens to run, and ONNX session lifetime/warm-up logic leaking into `voice-me-core` or `voice-me-ui`
- **Rule:** dismissing the Prompt Overlay on Enter is synchronous and unconditional — `voice-me-ui` never waits on `TtsPort::generate`. Generation runs as a Tokio blocking task (AD-5) dispatched immediately after dismissal; its result reaches the UI later via `AppEvent` (AD-3). Model lifecycle is owned entirely inside `voice-me-tts`: the four `ort` sessions (AD-12) are built once, lazily, on the first generation or on an explicit warm-up, then held for the process lifetime — they are never rebuilt per Speak Action, since session construction is the multi-second cost and per-utterance inference is not. A second Speak Action arriving while one is in flight is queued, not run concurrently (one set of sessions, bounded VRAM/RAM). Failures surface to `voice-me-core` only as the domain error enum (AD-4), never as an inference-engine detail.

### AD-11 — One shared audio buffer type crosses the TTS→Virtual Microphone boundary

- **Binds:** FR-5 (TTS generation), FR-6 (Virtual Microphone playback)
- **Prevents:** `voice-me-tts` and `voice-me-audio-{linux,windows}` independently assuming a sample rate, bit depth, or channel layout, and only discovering the mismatch as garbled audio at runtime
- **Rule:** `voice-me-core` defines one audio buffer type that `TtsPort::generate` returns and `VirtualMicPort::play` accepts. As of AD-12 its values are known and fixed: **32-bit float samples, 24 000 Hz, mono** — Chatterbox's `S3GEN_SR` output, taken straight from `conditional_decoder.onnx` with no conversion on the TTS side. Neither port trait uses a raw byte slice or a per-adapter-defined struct; any format conversion (24 kHz mono f32 to whatever the virtual mic driver expects — typically 48 kHz) happens inside the `voice-me-audio-*` adapter, never by the two adapters agreeing informally.

### AD-12 — TTS inference runs in-process on ONNX Runtime; no Python, no sidecar process

- **Binds:** FR-5 (TTS generation), FR-7 (dependency check / provisioning)
- **Supersedes:** the "bundled-Python Sidecar Process + local IPC" approach in prd.md/addendum.md (PRD Open Question 5), superseded 2026-09-21 — see the Superseded section below for why.
- **Prevents:** shipping and supervising a second runtime (CPython + PyTorch + CUDA wheels, ~3-6 GB and a per-OS packaging problem of its own) for what is four inference graphs and a tokenizer; an IPC framing decision, a process-supervision state machine, and a startup-ordering problem that exist only because the engine was out-of-process
- **Rule:** `voice-me-tts` implements `TtsPort` by running Chatterbox-Multilingual V3's ONNX export **in the app's own process** via the `ort` crate (ONNX Runtime bindings). It spawns no child process, opens no socket, and links no Python. The model contract is exactly the five artefacts `voice-me-deps` provisions from `onnx-community/chatterbox-multilingual-ONNX` (MIT):

  | Artefact | Role |
  | --- | --- |
  | `speech_encoder.onnx` (+`_data`, 592 MB) | Reference Voice Sample → conditioning embedding, prompt tokens, speaker x-vector, prompt features |
  | `embed_tokens.onnx` (+`_data`, 68 MB) | text/speech token ids + position ids + `exaggeration` → input embeddings |
  | `language_model.onnx` (+`_data`) | 0.5 B Llama backbone, KV-cached autoregressive speech-token generation; `_fp16` (1.04 GB) on GPU, `_q4` (354 MB) on CPU |
  | `conditional_decoder.onnx` (+`_data`, 534 MB) | speech tokens + speaker embedding/features → 24 kHz mono f32 waveform (AD-11) |
  | `tokenizer.json` | HF tokenizer, loaded with the `tokenizers` crate |

  The generation loop (prepend the `[<lang>]` tag, tokenize, encode the reference clip once, run the KV-cached decode loop with repetition penalty 1.2 and greedy argmax until `STOP_SPEECH_TOKEN` 6562 or `max_new_tokens`, then decode to waveform) is ported to Rust from the reference implementation in the model card — it is arithmetic over tensors, not model code, and no part of it requires Python.
- **Language scope consequence:** v1 speech languages are limited to those needing **no** Python-only text normalization. Turkish and English — the languages this product exists for — need only the language tag. Chinese (`pkuseg` + Cangjie), Japanese (`pykakasi`), Hebrew (`dicta-onnx`) and Korean (Jamo decomposition; the only one that is trivially portable) are **excluded from v1** rather than dragging a Python runtime back in for four locales. This narrows the SPEC's "speech languages Chatterbox supports natively" non-goal boundary; enabling one of them later is additive work inside `voice-me-tts`, not an architectural change.
- **Watermarking consequence:** Resemble's optional Perth watermarker is a Python library with no ONNX/Rust equivalent, so v1 emits un-watermarked audio. Recorded as a deliberate decision, not an oversight — see Deferred.

## Consistency Conventions

| Concern | Convention |
| --- | --- |
| Naming | Crates: kebab-case, `voice-me-` prefixed. Types: PascalCase. Modules/files: snake_case. Standard Rust convention throughout — no project-specific scheme. |
| Data & formats | Settings: single TOML file (AD-6). Errors: one domain enum per AD-4. No IDs/timestamps needed at this altitude — single-user, single-machine, no multi-record domain data. |
| State & cross-cutting | Mutation only through `voice-me-core` use cases (AD-3). Logging via `tracing`, initialized once in `voice-me-app` — no crate sets up its own logger or log-level scheme. |

## Stack

*Every version below was checked against crates.io/docs.rs/the Rust blog on 2026-09-20 — re-verify before use if implementation starts significantly later.*

| Name | Version |
| --- | --- |
| Rust | 1.98.1, edition 2024 |
| gpui-kit | 0.6.4 (crates.io) — the app's only dependency on GPUI; never add `gpui` directly |
| GPUI | not a direct dependency — arrives transitively via gpui-kit's own pin, currently the third-party `gpui-pre@0.3.5` crates.io snapshot ("snapshot of zed@d89e9c2"), not the official `gpui` crate (crates.io `0.2.2`, confirmed stale vs. zed's git `main` — zed-industries/zed#46486) and not a live git dependency. Bumping GPUI means bumping gpui-kit, not editing a rev in this repo. |
| thiserror | 2.0.20 |
| anyhow | 1.0.103 |
| tokio | latest stable — pin at implementation time |
| tracing | latest stable — pin at implementation time |
| directories | latest stable — pin at implementation time |
| ort (ONNX Runtime bindings, AD-12) | 2.0.0-rc.12 — re-verify at implementation time; `load-dynamic` so the runtime library is provisioned by `voice-me-deps`, never downloaded by the build |
| ONNX Runtime | 1.x matching the pinned `ort` rc; execution providers: CUDA (NVIDIA), DirectML (Windows), CPU fallback (AD-9) |
| tokenizers (HF) | latest stable — pin at implementation time; loads the model's `tokenizer.json` |
| ndarray | latest stable — tensor plumbing for the AD-12 generation loop |
| symphonia + rubato | latest stable — decode the Reference Voice Sample (wav/mp3/flac/ogg) and resample it to 24 kHz mono f32 for `speech_encoder.onnx` |

## Structural Seed

```text
voice-me/
  Cargo.toml                  # [workspace] members
  crates/
    voice-me-app/             # bin: composition root - GPUI entry, Tokio+gpui_tokio bridge setup, tracing init, per-OS adapter wiring
    voice-me-core/            # lib: domain types, AppState, AppEvent, port traits (HotkeyPort, VirtualMicPort, TtsPort, SettingsStore), domain error enum
    voice-me-ui/               # lib: gpui-kit views - Prompt Overlay, settings, voice setup, tray menu
    voice-me-hotkey-linux/     # lib: HotkeyPort adapter for Linux
    voice-me-hotkey-windows/   # lib: HotkeyPort adapter for Windows
    voice-me-audio-linux/      # lib: VirtualMicPort adapter - PipeWire/PulseAudio null-sink
    voice-me-audio-windows/    # lib: VirtualMicPort adapter - controls the signed Virtual-Audio-Driver
    voice-me-tray-linux/       # lib: TrayPort adapter - Linux system tray
    voice-me-tray-windows/     # lib: TrayPort adapter - Windows system tray
    voice-me-tts/               # lib: TtsPort adapter - in-process ONNX Runtime (ort) sessions + Chatterbox generation loop (AD-12)
    voice-me-deps/              # lib: dependency detection/provisioning (ONNX Runtime lib + EPs, model weights, virtual-mic driver), pulls from this repo's GitHub Releases
    voice-me-i18n/              # lib: TR/EN string catalogs
    voice-me-tests/             # test-only: black-box integration tests against the lib crates
```

**Deployment & environments.** Two environments only: developer machine (build + run) and end-user machine (run only) — no server/cloud tier. CI is GitHub Actions with separate `ubuntu-latest` and `windows-latest` jobs (no macOS job in v1); release artifacts and remotely-fetched runtime assets both live on this repo's GitHub Releases (AD-7). No auto-update mechanism is decided for v1 (see Deferred).

## Capability → Architecture Map

| Capability / Area | Lives in | Governed by |
| --- | --- | --- |
| FR-1 Voice Setup | voice-me-ui, voice-me-core | AD-3, AD-6 |
| FR-2 Background/tray operation | voice-me-app, voice-me-tray-linux, voice-me-tray-windows | AD-1, AD-2, AD-3 |
| FR-3 Global hotkey configuration | voice-me-hotkey-linux, voice-me-hotkey-windows | AD-1, AD-2 |
| FR-4 Prompt Overlay summon/type/dismiss | voice-me-ui | AD-1, AD-3, AD-10 |
| FR-5 TTS generation (in-process ONNX) | voice-me-tts | AD-1, AD-4, AD-5, AD-8, AD-9, AD-10, AD-11, AD-12 |
| FR-6 Playback through the Virtual Microphone | voice-me-audio-linux, voice-me-audio-windows | AD-1, AD-2, AD-11 |
| FR-7 Dependency Check and provisioning | voice-me-deps | AD-4, AD-5, AD-7, AD-8, AD-9, AD-12 |
| FR-8 Turkish/English UI | voice-me-ui, voice-me-i18n | — |
| FR-9 gpui-kit look & feel | voice-me-ui | external: gpui-kit-design-guides |

## Superseded

- **Bundled-Python Sidecar Process + local IPC → in-process ONNX Runtime (AD-12), 2026-09-21.** The original plan (prd.md, addendum.md, PRD Open Question 5) assumed Chatterbox could only be reached through its reference PyTorch implementation, and therefore through a bundled CPython sidecar talking to `voice-me-tts` over local IPC. That assumption no longer holds: Resemble's multilingual V3 checkpoint is published as a complete ONNX export (`onnx-community/chatterbox-multilingual-ONNX`, MIT) covering the whole pipeline — reference-voice encoding included — and the surrounding pipeline is plain tensor arithmetic that ports to Rust directly. Removing the sidecar removes, in one move: the Python-packaging risk that PRD Open Question 5 existed to track, the IPC-framing decision, sidecar supervision/restart logic (AD-10), the multi-GB CPython+PyTorch payload from `voice-me-deps`' provisioning surface, and process-startup latency from the Speak Action path. What it costs is the AD-12 port of the generation loop and the v1 language-scope narrowing recorded there. `TtsPort` is unchanged — this was always meant to be invisible behind the port, and it is.

## Deferred

- **DIRECTION (product owner, 2026-09-21): ship several build variants, not one binary — and let the user choose.** Releases become a matrix of backend profiles (`cpu`, `local-webgpu`, `cuda`, and later a remote-API one) built by CI as separate artefacts on this repo's GitHub Releases, rather than a single executable that detects everything at run time. The user picks the variant that matches their machine when they download, and the UI follows: a `local-webgpu` build lets the user pick *which* GPU in the system to use, since Story 2.5 found `ort::ep::WebGPU::with_device_id` does not choose an adapter on its own. Eventually generation over remote APIs is added as a further option. Not yet designed — this bullet records the direction so the decisions below are read in its light. **It reopens four things:**
  1. **AD-7** currently describes one binary per OS. It needs a build-variant matrix, and a rule for which runtime assets each variant implies — `cpu` wants the 8.7 MB core runtime and `language_model_q4`, `cuda` wants `onnxruntime-linux-x64-gpu_cuda1x` plus `language_model_fp16`, `local-webgpu` wants a WebGPU-enabled runtime that does not exist as a Microsoft release asset at all.
  2. **`local-webgpu` requires building ONNX Runtime from source** (`--use_webgpu --build_shared_lib`) and mirroring it per AD-7. Story 2.5 judged that not worth it for a measurement; as a shipping variant it becomes necessary work, and it is also what lets `voice-me-tts`'s `webgpu-probe` feature stop being a measurement tool that violates AD-8 and become a real profile that honours `load-dynamic`.
  3. **AD-9 becomes a three-way decision, not pure detection.** Today the backend is detected by `voice-me-deps` and recorded in `AppState`. With build variants the backend is partly fixed at build time and partly chosen by the user in Settings, so AD-9's rule needs to say how a build-time profile, a detected capability, and a user selection combine — and what happens when the user picks a GPU the build cannot drive. The detection machinery is still needed (to populate the GPU list and to catch "this build wants CUDA, this machine has none"); what changes is that it no longer has the last word.
  4. **Remote-API generation contradicts a stated product promise.** AD-8 forbids network egress outside `voice-me-deps` specifically to protect the PRD's local-only/no-telemetry guarantee, and AD-12 puts inference in-process. A remote backend is a second `TtsPort` adapter — architecturally additive, and `TtsPort` was designed to absorb exactly this — but it sends the user's typed text and possibly their voice sample to a third party. That is a **product** decision the PRD has to make explicitly, not an architecture detail: which variants can talk to the network, what is disclosed in the UI, and whether the local-only claim is rewritten or scoped to the local variants. Do not implement it as a quiet exception to AD-8.

  Impact beyond the ADs: Epic 3's Dependency Check and provisioning (Stories 3.1-3.4) become variant-aware rather than one list; Story 3.3's "degrade to CPU-only" changes meaning when CPU-only is its own download; and the Settings UI gains a backend/GPU surface the UX documents do not describe yet. Worth running `bmad-correct-course` before Epic 3 is planned in detail.

- **End-to-end latency on real hardware — MEASURED (Story 2.5, 2026-09-21).** AD-12 is viable: Chatterbox-Multilingual V3 runs in-process on ONNX Runtime from Rust, with no Python and no sidecar, and produces audible cloned Turkish and English speech. On this machine (i7-7700HQ, 4C/8T, 15 GB, no usable GPU — see the next bullet), with `"Merhaba, bugün nasılsın?"` and the 31 s Reference Voice Sample:

  | Variant / provider | Session build | `language_model` | `conditional_decoder` | Utterance total (2.0 s of audio) |
  | --- | --- | --- | --- | --- |
  | **Q4 / CPU** | 86–91 s | 28.0 ms/token | 17.6 s | **20.5 s** |
  | FP32 / CPU | 93.6 s | 107.9 ms/token | 19.8 s | 28.1 s |
  | FP16 / CPU | 88.1 s | 361.0 ms/token | 18.5 s | 42.1 s |
  | FP16 / WebGPU (Intel HD 630) | 110.0 s | 344.4 ms/token | 49.1 s | 73.8 s |

  Three consequences. **(a) AD-10's build-once/hold rule is confirmed, emphatically** — session construction is 86–110 s against a ~20 s utterance, so rebuilding per Speak Action would be absurd; it is also large enough that Story 2.6 needs a warm-up path and a "still loading" state, not just lazy construction on first use. **(b) Q4 is the CPU variant, by a factor of 3.9× over FP32 and 12.9× over FP16** — and, after a decode-loop fix found in Story 2.5's review, its token stream tracks FP32's closely (59 vs 57 tokens on the same prompt), so the speed is not being bought with a visibly different reading — FP16's explicit `Cast` nodes make it the *worst* CPU option, which AD-9's "FP16 on GPU, Q4 on CPU" split already anticipated. **(c) The token loop is not the bottleneck: `conditional_decoder` is,** at 17.6 s of a 20.5 s utterance (86 %). The vocoder's cost scales with the *concatenated* token sequence — the ~150 cropped reference tokens plus the generated ones — so it is roughly constant per utterance rather than proportional to what was said, and a short line costs nearly as much as a long one. Any future latency work belongs there (batching CFM timesteps, a smaller `n_timesteps`, or an alternative vocoder), not in the decode loop.

  **Real-time factor is 0.10×, so this is not usable in a live voice chat as it stands.** It is usable for the PRD's actual interaction — type a line, it is spoken a beat later — only with the Speak Action's asynchrony (AD-10) doing real work and a visible "generating" state. Whether ~20 s per utterance is acceptable at all is a product decision this spike deliberately does not make.
- **How the ONNX Runtime shared library and its execution providers get onto the user's machine — RESOLVED for Linux/CPU (Story 2.5).** The complete runtime-asset list for the shipping path is: (1) `libonnxruntime.so` from `onnxruntime-linux-x64-1.28.2.tgz` (8.7 MB, ships the core library and **no** provider libraries), resolved at run time from `ORT_DYLIB_PATH`; and (2) nine model files in the cache directory — `tokenizer.json` at its root, then under `onnx/`: `speech_encoder.onnx` + `.onnx_data` (592 MB), `embed_tokens.onnx` + `.onnx_data` (68 MB), `language_model_q4.onnx` + `.onnx_data` (354 MB), `conditional_decoder.onnx` + `.onnx_data` (534 MB). 1.56 GB total. Each `.onnx_data` records its location as a bare relative filename, so it must sit beside its `.onnx` and the session must be built from a path, never from bytes. `ModelCache::required_files` in `voice-me-tts` is that list in code, and a missing file is reported as `VoiceMeError::MissingRuntimeAsset` naming the absolute path — Story 3.2 provisions exactly these. **No execution-provider library is needed for the CPU path**, which is what this dev machine can run and therefore all Story 2.5 could verify. This is *not* a decision that voice-me ships CPU-only: CUDA remains the intended GPU path (AD-9) and is deferred, not dropped — it simply cannot be exercised on this hardware (next bullet). Story 3.2 should treat the CPU list as the one confirmed set of files and leave room for a CUDA execution-provider set alongside it: `onnxruntime-linux-x64-gpu_cuda12-1.28.2.tgz` (404 MB) and `…gpu_cuda13…` (230 MB) carry `libonnxruntime_providers_cuda.so`/`_shared.so`, and the GPU path additionally wants `language_model_fp16` (1.04 GB) instead of `_q4`. Those sizes and filenames are from the release listings, not from a run. Windows/DirectML remains unresolved.
- **Quantization quality — MEASURED, pending a listening check (Story 2.5).** Q4, FP16 and FP32 were each generated from the user's own Reference Voice Sample for the same Turkish line; the wavs are at `~/.local/share/voice-me/spike-2-5/`. Objectively the variants diverge: greedy argmax turns small logit differences into different token streams, so Q4 produced 51 tokens (2.00 s) where FP32 and FP16 both produced 61 (2.40 s) — FP16 and FP32 agree with each other, Q4 does not. That is a different *reading* of the same sentence, not necessarily a worse one, and no objective metric separates them; the audible verdict is the user's and is not yet recorded here. **`conditional_decoder` bakes in an unseeded `randn_like` for its flow-matching prior, so no two runs are byte-identical regardless of variant** — quality can only ever be judged by listening, never by comparison against a golden wav.
- **GPU inference — NOT MEASURABLE on this dev machine; deferred, not dropped (Story 2.5).** AD-9's CUDA path stays the intended GPU backend and is still expected to be added; Story 2.5 simply had no hardware to verify it on, so it was skipped rather than eliminated. Nothing below is evidence against CUDA on a supported GPU — only against these two devices. **CUDA:** the Quadro M1200 is GM107 = sm_50; ONNX Runtime 1.28.2's prebuilt CUDA floor is sm_60 with no PTX target below `120-virtual` to JIT from, so a correct install would still fail at run with "no kernel image is available" — and Arch ships only the Turing+ `nvidia-open` driver besides. **WebGPU over Vulkan runs and is dramatically slower than the CPU:** on the same line and the same binary, 24.9 ms/token on CPU against 311 ms/token on the Intel HD 630 (Mesa ANV) and 1174 ms/token on the Quadro (NVK), with the vocoder 3.2× and 14× slower respectively. This is a genuine GPU run, not a silent fallback: ORT's node-placement log puts 183 of `language_model_q4`'s 189 nodes on `WebGpuExecutionProvider` — all 30 `GroupQueryAttention`, all 151 `MatMulNBits`, all 60 `SkipSimplifiedLayerNormalization` — leaving only six attention-mask shape ops on CPU, so the `com.microsoft` contrib ops are *not* the obstacle; the hardware is. The Quadro additionally loses its Vulkan device (`VK_ERROR_DEVICE_LOST` from NVK) partway through `conditional_decoder` on anything longer than a single word, and the audio it does produce is **audibly corrupted** (confirmed by listening, 2026-09-21) — so NVK-on-Maxwell fails on correctness, not merely on speed. A `local-webgpu` variant must treat a device lost mid-inference as an error to surface, never as a buffer to play. Two practical findings for any future GPU work: `ort::ep::WebGPU::with_device_id` had no observable effect (Dawn selects the adapter itself; only `VK_DRIVER_FILES` restricting the Vulkan loader actually chose a device), and `.error_on_failure()` guards EP *registration* only — node placement must be read from ORT's verbose log. **Consequence for AD-9:** its rule stands unchanged and its CUDA/FP16 half remains the plan — it is simply unverified, because no backend on this machine can run it. Treat "FP16 on GPU" as designed-but-untested rather than either confirmed or refuted, and measure it on the first machine with an sm_60-or-newer NVIDIA GPU, or on Windows/DirectML.
- **`voice-me-tts`'s `webgpu-probe` feature is a measurement tool, not a shipping path.** Reaching WebGPU at all required turning `load-dynamic` *off* — Cargo features are additive, and with `load-dynamic` enabled ort-sys skips its download entirely and silently builds a CPU-only binary that still reports "webgpu" on the command line. The feature layout therefore puts `load-dynamic` behind a default `dynamic-runtime` feature so `--no-default-features --features webgpu-probe` means what it says. That build downloads pyke's Dawn-bundling distribution and statically links it — the opposite of AD-8 — and is never built in CI. An architecture-clean WebGPU path would need ONNX Runtime built from source with `--use_webgpu --build_shared_lib` and mirrored per AD-7. On this hardware's numbers alone that would not be worth it — but the build-variant direction above makes it required work for a `local-webgpu` profile, whose audience is other people's GPUs rather than this Kaby Lake iGPU and a Maxwell Quadro. When that happens, this feature stops being a measurement tool that violates AD-8 and becomes a shipping profile that honours `load-dynamic`.
- **Perth watermarking absent in v1 (AD-12).** Python-only today. Options if revisited: a Rust port, an ONNX export of the watermarker, or shipping without. Decide deliberately before v1 is published, rather than by omission.
- **Chatterbox-Nano as a CPU path.** FR-7/Story 3.3 originally named Nano as the no-GPU fallback; AD-12 makes the Q4 language-model variant the default CPU path instead (same model, same code path, no second engine). Nano stays a fallback-to-the-fallback only if Q4-on-CPU measures unusably slow **and** Nano has an ONNX export. Story 2.5 checked the first half: Q4-on-CPU works and is the fastest path available here, but at a 0.10× real-time factor — and the cost is concentrated in `conditional_decoder`, which Nano shares, so a smaller *language model* would not move the number much. Whether Nano has an ONNX export is still unchecked, and on this evidence it is the wrong lever anyway.
- **Tray half of PRD Open Question 6 — resolved on Linux (Story 2.1).** `voice-me-tray-linux` uses the `gpui-tray` crate (v0.1, Apache-2.0, published on crates.io, github.com/kagenokeiyou/gpui-tray) instead of the unverified "Adabraka GPUI" fork: it's native to GPUI/gpui-kit (tray menus are plain `gpui::MenuItem`s dispatching ordinary GPUI actions via `cx.on_action`, no second event loop to bridge) and ships a `gpui-kit` feature flag matching gpui-kit 0.6+, avoiding its default feature's pinned git dependency on Zed's own `gpui`. `TrayPort::show` gained a `cx: &mut gpui_kit::App` parameter so adapters can build the native tray item and dispatch actions through it — `voice-me-core` now depends on `gpui-kit` for this context type only, never on `gpui-tray` itself, which stays confined to the `voice-me-tray-*` adapters (AD-2 holds). Verified via `crates/voice-me-tray-linux/examples/spike.rs`: the adapter runs without error and claims the well-known `org.kde.StatusNotifierItem-<pid>-1` DBus name per the freedesktop StatusNotifierItem spec — confirmed by `busctl --user list` while the example ran. Full pixel-level visual confirmation (icon actually rendering in a panel) wasn't captured in this dev environment (screenshot tooling here is sandboxed), but GNOME's AppIndicator extension is installed and enabled on this machine, and DBus-level registration succeeding is the operative proof the adapter works. Fallback if `gpui-tray` ever proves broken: `tray-icon` (tauri-apps, mature, actively maintained, cross-platform) — framework-agnostic, would need its own event-receiver pumped from somewhere in `voice-me-app`. **Windows deferred** (Story 2.1 explicit scope decision): this dev environment has no Windows Rust target or cross-compile toolchain installed, so `voice-me-tray-windows` cannot be built or run here. It stays a `todo!()` stub (signature updated to match the new `TrayPort::show(&self, cx: &mut gpui_kit::App)`). Intended approach when Windows access exists: the same `gpui-tray` crate, whose Windows backend uses `Shell_NotifyIconW` plus a hidden top-level window and Win32 menus — resolve and verify before implementing `voice-me-hotkey-windows`/other Windows adapters that assume tray presence.
- **Hotkey half of PRD Open Question 6 — still unresolved.** Only the tray half was in scope for Story 2.1 (above); whether GPUI/gpui-kit (or a hand-rolled per-OS shim) can deliver global-hotkey capture, including while a fullscreen game holds focus, remains unverified. Resolve before implementing `voice-me-hotkey-linux`/`voice-me-hotkey-windows` in Story 2.3.
- Windows `Virtual-Audio-Driver` control mechanism (named pipe vs. an alternative) — PRD Open Question 2; must resolve before `voice-me-audio-windows` is implemented.
- Anti-cheat compatibility of the chosen hotkey-capture approach — PRD Open Question 3; investigate before committing `voice-me-hotkey-windows`/`-linux`'s implementation.
- Exact tray UX per OS (icon, menu contents) — a UI/UX detail, not an architectural invariant.
- Auto-update mechanism — not needed for v1; revisit once v1 ships.
- macOS adapters (`voice-me-hotkey-macos`, `voice-me-audio-macos`) — deferred past v1; AD-2 already makes this additive.
- Exact settings TOML schema (field names/types) — an implementation detail once `voice-me-core` is scaffolded, bound only by AD-6's ownership rule.
- **Exact shape of the in-house Tokio/GPUI bridge (AD-5) — RESOLVED (Story 2.5).** `voice_me_core::tokio_bridge`: a `TokioRuntime` GPUI `Global` owning the `Runtime` itself (not merely a `Handle` — dropping the owner would shut the blocking pool down under every in-flight job), plus `spawn_blocking(cx, work)` returning a plain `Send + 'static` future for `cx.background_spawn`. Panics inside the job come back as a domain error rather than unwinding into GPUI's executor. Exercised end to end by `crates/voice-me-tts/examples/tts-spike.rs`, which dispatches a ~20 s inference from inside `gpui_kit::application().run(..)` without blocking the main thread. Built against gpui-kit's re-exported `gpui` types, not Zed's own.
- Pinning policy inconsistency: `thiserror`/`anyhow` are pinned to exact patch versions while `tokio`/`tracing`/`directories` are left "latest stable" — both approaches carry equal staleness risk since no implementation date is committed yet; normalize at implementation time (either re-verify all five, or drop the specific patch numbers).
