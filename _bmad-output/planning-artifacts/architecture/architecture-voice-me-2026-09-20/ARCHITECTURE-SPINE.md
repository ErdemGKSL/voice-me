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

### AD-7 — Open source, GitHub-native CI/release, first-party dependency hosting

- **Binds:** FR-7 (dependency management), overall distribution
- **Prevents:** `voice-me-deps` pulling runtime assets from ad-hoc third-party URLs with no versioning or trust boundary
- **Rule:** GitHub Actions builds Linux and Windows separately (no macOS job in v1); binaries publish to GitHub Releases of this repo. Any remotely-fetched runtime asset that (a) is small enough for a GitHub Release asset and (b) voice-me is legally permitted to redistribute (the ONNX Runtime shared library, the Windows Virtual-Audio-Driver installer — both open source and small) is mirrored there — one trusted, versioned source `voice-me-deps` pulls from. Chatterbox's ONNX weights (AD-12) are covered by this rule as of 2026-09-21: they are MIT-licensed — therefore redistributable — and, in the variants v1 ships, every individual file is under GitHub Releases' 2 GB per-asset limit (`speech_encoder.onnx_data` 592 MB, `conditional_decoder.onnx_data` 534 MB, `embed_tokens.onnx_data` 68 MB, `language_model_q4.onnx_data` 354 MB / `language_model_fp16.onnx_data` 1.04 GB). The FP32 `language_model.onnx_data` (2.08 GB) is the one variant that does **not** fit as a single release asset and is therefore not mirrored — v1 ships FP16 (GPU) and Q4 (CPU) instead. `voice-me-deps`'s `DependencyProvisioningPort` still supports fetching from more than one trusted source (this repo's releases, and/or the Hugging Face origin `onnx-community/chatterbox-multilingual-ONNX`) rather than hard-coding "GitHub Releases only" — the origin stays a fallback, not the primary.

### AD-8 — No network egress outside `voice-me-deps`'s declared GitHub-Releases fetches

- **Binds:** all crates
- **Prevents:** any crate silently phoning home — a cloud fallback added to `voice-me-tts`, telemetry slipped into `voice-me-ui`, or any other undeclared network call — which would break the PRD's local-only/no-telemetry guarantee without anyone deciding it
- **Rule:** the only network calls in the whole app are `voice-me-deps` fetching versioned assets from its trusted sources (AD-7). Every other crate is offline: since AD-12 `voice-me-tts` performs no IPC and opens no socket at all — it reads model files from the cache directory `voice-me-deps` owns. The `ort` crate must be configured to load a locally provisioned ONNX Runtime (no download-at-build-or-run behaviour), and Hugging Face Hub clients (`hf-hub` and friends) are banned outside `voice-me-deps` by the same CI check. Enforced with a CI check (a `cargo-deny` ban list, or a workspace-wide `grep` for HTTP-client crates outside `voice-me-deps`'s `Cargo.toml`) rather than relying on code-review discipline alone — solo development is exactly the setting where a silent violation goes unnoticed without an automated check. Adding any other network call is an architectural change, not a local one — it revisits this AD.

### AD-9 — Hardware capability detection is core-owned, not adapter-to-adapter

- **Binds:** FR-5 (TTS generation), FR-7 (dependency check / GPU fallback)
- **Prevents:** `voice-me-tts` querying `voice-me-deps` directly for GPU availability, which would violate AD-1's no-adapter-to-adapter rule
- **Rule:** `voice-me-deps` detects which ONNX Runtime **execution provider** is usable (CUDA/TensorRT on NVIDIA, DirectML on Windows, CPU otherwise) as part of its Dependency Check and reports the result **only** by emitting it on the shared `AppEvent` channel (AD-3) — never by calling a `voice-me-core` use-case function directly. `voice-me-app` (the sole `AppEvent` receiver) routes that event into `voice-me-core`, which records it in `AppState` as one `InferenceBackend` value. `voice-me-tts` reads that value from `AppState`/its `TtsPort` call parameters and picks the matching execution provider **and** language-model weight variant (FP16 on GPU, Q4 on CPU — see AD-12); it never calls `voice-me-deps` itself, and `voice-me-deps` never calls `voice-me-core` directly either.

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

- **End-to-end latency on real hardware is still unmeasured.** AD-12 removes process-startup cost but not inference cost: the decode loop runs the 0.5 B backbone once per speech token (up to `max_new_tokens`). Whether Q4-on-CPU is usable-in-a-voice-chat or only technically-working, and how much FP16-on-CUDA buys, is the thing Story 2.5's spike must actually measure (PRD Open Question 1).
- **How the ONNX Runtime shared library and its execution providers get onto the user's machine.** `ort` with `load-dynamic` needs `libonnxruntime.so`/`onnxruntime.dll` plus, for GPU, the matching CUDA/cuDNN or DirectML libraries — that is now the bulk of what `voice-me-deps` provisions in place of a Python runtime (AD-7, AD-12). Which files, per OS, mirrored where, resolve before `voice-me-deps` is implemented.
- **Quantization quality.** Q4 on CPU is a size/speed choice, not a free one; its effect on cloned-voice fidelity versus FP16 is unverified. Compare in the Story 2.5 spike and record which variant each backend ships (AD-9).
- **Perth watermarking absent in v1 (AD-12).** Python-only today. Options if revisited: a Rust port, an ONNX export of the watermarker, or shipping without. Decide deliberately before v1 is published, rather than by omission.
- **Chatterbox-Nano as a CPU path.** FR-7/Story 3.3 originally named Nano as the no-GPU fallback; AD-12 makes the Q4 language-model variant the default CPU path instead (same model, same code path, no second engine). Nano stays a fallback-to-the-fallback only if Q4-on-CPU measures unusably slow **and** Nano has an ONNX export — neither checked.
- **Tray half of PRD Open Question 6 — resolved on Linux (Story 2.1).** `voice-me-tray-linux` uses the `gpui-tray` crate (v0.1, Apache-2.0, published on crates.io, github.com/kagenokeiyou/gpui-tray) instead of the unverified "Adabraka GPUI" fork: it's native to GPUI/gpui-kit (tray menus are plain `gpui::MenuItem`s dispatching ordinary GPUI actions via `cx.on_action`, no second event loop to bridge) and ships a `gpui-kit` feature flag matching gpui-kit 0.6+, avoiding its default feature's pinned git dependency on Zed's own `gpui`. `TrayPort::show` gained a `cx: &mut gpui_kit::App` parameter so adapters can build the native tray item and dispatch actions through it — `voice-me-core` now depends on `gpui-kit` for this context type only, never on `gpui-tray` itself, which stays confined to the `voice-me-tray-*` adapters (AD-2 holds). Verified via `crates/voice-me-tray-linux/examples/spike.rs`: the adapter runs without error and claims the well-known `org.kde.StatusNotifierItem-<pid>-1` DBus name per the freedesktop StatusNotifierItem spec — confirmed by `busctl --user list` while the example ran. Full pixel-level visual confirmation (icon actually rendering in a panel) wasn't captured in this dev environment (screenshot tooling here is sandboxed), but GNOME's AppIndicator extension is installed and enabled on this machine, and DBus-level registration succeeding is the operative proof the adapter works. Fallback if `gpui-tray` ever proves broken: `tray-icon` (tauri-apps, mature, actively maintained, cross-platform) — framework-agnostic, would need its own event-receiver pumped from somewhere in `voice-me-app`. **Windows deferred** (Story 2.1 explicit scope decision): this dev environment has no Windows Rust target or cross-compile toolchain installed, so `voice-me-tray-windows` cannot be built or run here. It stays a `todo!()` stub (signature updated to match the new `TrayPort::show(&self, cx: &mut gpui_kit::App)`). Intended approach when Windows access exists: the same `gpui-tray` crate, whose Windows backend uses `Shell_NotifyIconW` plus a hidden top-level window and Win32 menus — resolve and verify before implementing `voice-me-hotkey-windows`/other Windows adapters that assume tray presence.
- **Hotkey half of PRD Open Question 6 — still unresolved.** Only the tray half was in scope for Story 2.1 (above); whether GPUI/gpui-kit (or a hand-rolled per-OS shim) can deliver global-hotkey capture, including while a fullscreen game holds focus, remains unverified. Resolve before implementing `voice-me-hotkey-linux`/`voice-me-hotkey-windows` in Story 2.3.
- Windows `Virtual-Audio-Driver` control mechanism (named pipe vs. an alternative) — PRD Open Question 2; must resolve before `voice-me-audio-windows` is implemented.
- Anti-cheat compatibility of the chosen hotkey-capture approach — PRD Open Question 3; investigate before committing `voice-me-hotkey-windows`/`-linux`'s implementation.
- Exact tray UX per OS (icon, menu contents) — a UI/UX detail, not an architectural invariant.
- Auto-update mechanism — not needed for v1; revisit once v1 ships.
- macOS adapters (`voice-me-hotkey-macos`, `voice-me-audio-macos`) — deferred past v1; AD-2 already makes this additive.
- Exact settings TOML schema (field names/types) — an implementation detail once `voice-me-core` is scaffolded, bound only by AD-6's ownership rule.
- Exact shape of the in-house Tokio/GPUI bridge (AD-5) — small, low-risk, but not yet written; implemented against gpui-kit's re-exported `gpui` types, not Zed's own.
- Pinning policy inconsistency: `thiserror`/`anyhow` are pinned to exact patch versions while `tokio`/`tracing`/`directories` are left "latest stable" — both approaches carry equal staleness risk since no implementation date is committed yet; normalize at implementation time (either re-verify all five, or drop the specific patch numbers).
