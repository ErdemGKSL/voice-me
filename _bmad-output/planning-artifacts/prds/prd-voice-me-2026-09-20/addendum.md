# Addendum: voice-me PRD

Technical-how detail that doesn't belong in the PRD's capability-level narrative. Pair with `_bmad-output/planning-artifacts/briefs/brief-voice-me-2026-09-20/addendum.md`, which holds the earlier architecture-options research (Chatterbox packaging, GPUI's tray/hotkey gaps, competitive landscape) — not repeated here. This file only adds what's new since the brief: the specific driver candidate and the crate structure direction.

## Windows virtual microphone driver: candidate identified

- **Candidate**: [VirtualDrivers/Virtual-Audio-Driver](https://github.com/VirtualDrivers/Virtual-Audio-Driver), release `25.7.14`.
- License: MIT (original code) + MS-PL (incorporates Microsoft's WDK Sysvad driver samples).
- Signing: this specific release is signed via **SignPath.io** using a SignPath Foundation certificate (free code-signing for qualifying open-source projects) — a real, working signing path, distinct from the project's general README, which describes a beta/test-signing-mode requirement (`bcdedit /set testsigning on`). This substantially changes the Windows driver-signing risk flagged in the brief: rather than the team obtaining its own EV certificate and WHQL signing, the plan is to depend on an already-signed third-party driver.
- Control mechanism: **not confirmed**. The general README says the driver is configured through standard Windows UI (Sound Settings, Device Manager, Volume Mixer), and that "named pipes, shared memory buffers" for programmatic control are available only as a **custom build on request from the developer** — this is not confirmed as a standard/default feature of the public release. This is Open Question 2 in the PRD — needs direct verification (read the actual release assets/source, or contact the maintainer) before the architecture stage locks this in as the control mechanism.
- Fallback if named-pipe control isn't available out of the box: investigate whether the driver exposes any other scriptable interface (registry-based config, a CLI tool, a COM/WinRT API), or whether forking the driver to add a control channel is more realistic than commissioning a custom build.

## Proposed Rust workspace / crate structure

Direction from the user: monorepo, split aggressively into crates rather than one monolithic crate — separate crate for the binary entrypoint, separate crate(s) for tests, everything else as proper lib crates. Proposed starting shape (to be refined at `bmad-architecture`, not locked here):

```
voice-me/                      # cargo workspace root
├── Cargo.toml                 # [workspace] members
├── crates/
│   ├── voice-me-app/          # bin crate: entrypoint, wires everything together, GPUI window/app setup
│   ├── voice-me-ui/           # lib crate: gpui-kit-based views/components (Prompt Overlay, settings, voice setup), styled per gpui-kit-design-guides
│   ├── voice-me-hotkey/       # lib crate: global hotkey registration/capture, per-OS backends behind a trait
│   ├── voice-me-audio/        # lib crate: virtual microphone integration, per-OS backends behind a trait
│   ├── voice-me-tts/          # lib crate: Sidecar Process lifecycle + IPC client, Chatterbox request/response types
│   ├── voice-me-deps/         # lib crate: Dependency Check + provisioning (Python runtime, GPU detection, driver install)
│   ├── voice-me-i18n/         # lib crate: TR/EN string catalogs, language switching
│   ├── voice-me-core/         # lib crate: shared domain types (Reference Voice Sample, settings/config, error types)
│   └── voice-me-tests/        # test-only crate: integration tests exercising the above as a black box
```

Rationale sketch (for architecture stage to confirm or revise):
- Per-OS logic (hotkey capture, virtual audio device control) isolated behind small trait-based lib crates so Linux/Windows implementations don't leak into the rest of the app, and a future macOS backend is an additive crate, not a rewrite.
- `voice-me-tts` isolates all Sidecar Process/IPC concerns so the "how we talk to the Python sidecar" decision can change without touching UI or hotkey code.
- `voice-me-app` stays thin — composition root only, no business logic — so `voice-me-tests` (and any future headless testing) can exercise the lib crates without spinning up the GPUI app.
- Exact crate boundaries, naming, and whether some of these merge (e.g. `voice-me-deps` folding into `voice-me-tts`) are architecture-stage decisions, not fixed here.

---

## Superseded: the bundled-Python sidecar (2026-09-21)

Everything above that assumes a **Sidecar Process** — a bundled CPython/PyTorch runtime reached from `voice-me-tts` over local IPC — is superseded. Chatterbox-Multilingual V3 is published as a complete ONNX export (`onnx-community/chatterbox-multilingual-ONNX`, MIT), reference-voice cloning included, so `voice-me-tts` runs the model **in-process** via the `ort` crate (ONNX Runtime): no Python, no child process, no IPC.

This resolves PRD Open Question 5 by removing its premise. Binding detail lives in **Architecture Spine AD-12** (with consequences recorded in AD-5, AD-7, AD-8, AD-9, AD-10 and AD-11); `epics.md` and `SPEC.md` are updated to match. Two scope consequences worth carrying forward: v1 speech languages exclude Chinese, Japanese, Hebrew and Korean (their text normalization is Python-only), and generated audio is not Perth-watermarked in v1.
