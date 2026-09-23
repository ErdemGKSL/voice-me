# Epic 3 Context: Never Get Stuck on Setup

<!-- Compiled from planning artifacts. Edit freely. Regenerate with compile-epic-context if planning docs change. -->

## Goal

Nobody should hit a cryptic crash because a runtime library, a model file, or a driver isn't there. This epic makes the app detect what it needs, fix what it can with one click from inside its own UI, and name in plain words anything it can't fix. It also gives the user the choice that makes all of this meaningful: which backend generates their speech — a local CPU, CUDA, or WebGPU path, or a remote speech API with their own key — with the app honest at every point about which backend is selected, whether that backend can actually run on this machine, and what was really acquired versus what was asked for.

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

## Requirements & Constraints

**Backend-aware, never variant-aware.** One artefact per OS carries every backend; the user picks one inside the app. Every dependency question in this epic is therefore relative to the *selected* backend: the CPU backend never reports a missing GPU provider library, a GPU backend never reports weights it doesn't use, and a remote backend's readiness is an API key plus a reachable provider rather than a file list. (A download-time build-variant model appears in older planning text; it was reversed on 2026-09-22 and must not be implemented.)

**Truth over silent fallback.** What the UI reports as the active backend must be what was actually acquired at session build, never what was selected. A selected backend that can't run here — no supported NVIDIA GPU, no usable Vulkan/D3D12 adapter, no API key, a GPU that has vanished — is named in words, together with what selecting the CPU backend would do and a one-click way to select it. Substituting another backend quietly is forbidden. Selecting CPU on a machine with a good GPU is a normal, non-error state.

**Self-sufficiency.** Dependency check, provisioning, and recovery all happen in-app: no terminal, no external docs, no blocking upfront setup wizard. Failures name what went wrong specifically. Anything not automatable gets short manual steps instead of an Install button. Resolving a dependency restores normal behavior without an app restart.

**Asset scale.** Provisioning fetches only the selected backend's assets — roughly a 1.56 GB CPU set (small core runtime plus the 354 MB Q4 language model alongside the shared encoder/decoder/embedding graphs), or a GPU backend's provider libraries plus the 1.04 GB FP16 language model; nothing at all for a remote backend. The large weight downloads must report progress and be resumable rather than looking frozen.

**Remote path constraints.** Only the typed text, the language tag, and the Reference Voice Sample may leave the machine — no telemetry, no usage stats, nothing about the machine. The sample is uploaded once per provider and referenced by id afterwards, keyed so re-recording invalidates it; its presence on provider infrastructure is user-visible state with a delete action. A full disclosure of what is sent and to whom is confirmed once per provider before the first request. The API key lives in the settings file in plaintext; the UI says so where it is entered, and the key is redacted from logs. Provider failures are one clear notification naming provider and reason, with no automatic re-send of audio.

**Scope boundary.** No voice-me server, no account, no telemetry ever; no voice-me-hosted proxy or shared keys; stock-voice-only providers are excluded.

## Technical Decisions

- **Ports and ownership.** Dependency detection and provisioning live in `voice-me-deps` behind `DependencyProvisioningPort`. Backend selection is a persisted setting through `SettingsStore` (owned inside `voice-me-core`). Detected capability flows from `voice-me-deps` to `voice-me-core` on the shared `AppEvent` channel only — never a direct call into a TTS adapter, and never a TTS adapter querying `voice-me-deps`. Core reconciles selection plus capability into a single resolved value in `AppState` (active backend, device where applicable, implied weight variant); the TTS adapters read that.
- **Two TTS adapters, one port.** `voice-me-tts-onnx` (CPU/CUDA/WebGPU over one ONNX Runtime; renamed from `voice-me-tts`) and `voice-me-tts-remote` both implement `TtsPort`, so the Speak Action path never branches on backend. Inside the remote adapter, one `SpeechProvider` trait per vendor owns endpoint, auth, request/response shape and error mapping — no provider-specific type may surface above `TtsPort`. Each provider carries its own key and its own uploaded-sample id independently. Requests are deadline-bounded; failures map to the single domain error enum and reach the user through `NotificationPort`.
- **Disclosure is enforced by the core use-case**, not by adapter courtesy: core refuses to call a provider until that provider's one-time confirmation is recorded in settings. Declining leaves the backend selected but unusable, never silently switched.
- **Network egress is confined to exactly two crates**, `voice-me-deps` and `voice-me-tts-remote`. The CI check becomes an allowlist of those two manifests and fails on a third; Hugging Face Hub clients stay banned outside `voice-me-deps`. `voice-me-tts-onnx` opens no socket and reads model files from the deps-owned cache directory; `ort` uses `load-dynamic` against a provisioned runtime, never downloading at build or run time.
- **Storage split.** Settings and Reference Voice Sample audio live in core-owned config/data directories behind `SettingsStore`. Large fetched runtime assets (ONNX Runtime library and providers, model weights, the Windows driver installer) live in a separate OS cache directory owned exclusively by `voice-me-deps` — outside `SettingsStore`'s scope.
- **One runtime, every provider.** `ort` commits exactly one `libonnxruntime` per process, so CI must build ONNX Runtime from source with CUDA and WebGPU enabled and mirror that single distribution as a versioned, resumable asset. That makes switching local backends a session rebuild rather than a process relaunch, and retires the `webgpu-probe` feature and its build-time download. Provisioning sources must be trusted, versioned, resumable URLs — this repo's GitHub Releases where mirroring is legal and the asset fits, the pinned upstream MIT origin otherwise.
- **HTTP stack** for both network-capable crates is `reqwest` with rustls, plus `serde_json` for provider bodies.

## UX & Interaction Patterns

- Everything in this epic surfaces in **Settings → Dependencies**, reachable from the tray menu and auto-opened when a blocking dependency failure occurs.
- **Dependency row:** name, a ready/missing/installing badge, and either a one-click Install action or a short manual-steps link. Status is never color-only — the word "missing" appears, not just a color.
- **Backend selector** sits above the dependency rows: a `Select` of the backends this binary carries plus each configured remote provider, with a read-only line beneath stating what the selection actually acquired at session build. Selecting re-derives the dependency rows immediately.
- **GPU device selector** appears only when a GPU backend is selected, listing detected devices by name; unset means "let the backend decide"; a device that is gone is reported by name and reverts to the default.
- **API key field** is a masked `Input` shown when a remote backend is selected, with a plain line stating plaintext storage. **Remote sample state** states the sample is held on the provider and offers a delete action.
- **Remote disclosure dialog** fires on the first Speak Action after selecting a remote provider, naming the provider and exactly what is sent.
- **Blocked overlay:** the Prompt Overlay still opens on hotkey press but shows an inline notice instead of accepting input, while Settings → Dependencies auto-opens naming the specific blocker.
- Microcopy is quiet and specific — "Missing: speech model files. Download now?", not "Dependency check failed (code 3)". Every control is keyboard-operable with visible focus, per gpui-kit's design guides; use gpui-kit components rather than hand-rolled primitives.

## Cross-Story Dependencies

- **3.1 is the foundation:** 3.2, 3.3, 3.4 and 3.9 all consume the per-backend required-asset lists and candidate-device list it produces.
- **3.5 precedes the honesty and provisioning rules in practice** — everything else keys off "the selected backend" — but 3.1 can land first with the selection defaulting to CPU (unset means CPU with no device preference).
- **3.6 establishes the `SpeechProvider` abstraction that 3.7 exercises**; 3.7 must require no change above `TtsPort`.
- **3.8 blocks the GPU backends only.** CUDA and WebGPU are menu entries that cannot work until the all-provider runtime is built and mirrored; the CPU backend stays fully usable while 3.8 is unstarted, and 3.9 is only meaningful once a GPU backend can actually be selected.
- **From Epic 2:** `TtsPort`, `AppState`/`AppEvent`, `NotificationPort`, `SettingsStore`, the shared 24 kHz mono f32 audio buffer, and the Virtual Microphone playback path already exist and are unchanged by this epic — remote generation must reuse the same playback path. Story 2.5's measurements (CPU ~0.10x real-time; a WebGPU build falsely reporting `provider: webgpu:0` while running on CPU; a GPU losing context mid-generation) are the cautionary evidence behind the no-silent-fallback and device-error rules.
- **Windows caveat:** Story 2.8 (Windows virtual-microphone control surface) is still unstarted, so the Windows driver's dependency row and its provisioning steps have no verified control surface behind them yet.
