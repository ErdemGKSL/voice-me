---
title: 'Spike — In-Process Chatterbox Inference on ONNX Runtime'
type: 'feature'
created: '2026-09-21'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
context: ['_bmad-output/implementation-artifacts/spec-2-5-model-contract.md']
baseline_commit: 'a9b548a256ca3858a800d2220ade2c76e5bb2b4c'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** `voice-me-tts` is an 11-line `todo!()` stub still described as "the Chatterbox sidecar", and the whole of AD-12 — that Chatterbox-Multilingual V3's ONNX export can be driven in-process from Rust, fast enough to use in a live voice chat — is an unverified architectural bet. Story 2.6 wires generation into the Speak Action on top of it, and Epic 3 provisions files nobody has yet confirmed are the right files.

**Approach:** Port the AD-12 generation loop to Rust in `voice-me-tts`, drive it from an example binary running inside a real GPUI app, and measure it. Land the two pieces of permanent contract the loop forces into existence — the AD-11 audio buffer in `voice-me-core` and the AD-5 Tokio/GPUI bridge — then record latency, quantization quality, and the exact runtime-file list as the spike's actual output.

## Boundaries & Constraints

**Always:** The generation loop is real module code in `voice-me-tts`, not throwaway example code, so Story 2.6 wires an existing engine rather than rewriting one (spec-2-1's precedent: the tray adapter became the real adapter). The AD-11 buffer is core-defined, 24 kHz mono f32, and is what `TtsPort::generate` returns and `VirtualMicPort::play` accepts. The AD-5 bridge is one shared utility owned by `voice-me-core`, and generation runs on `tokio::task::spawn_blocking` — the spike must exercise it from inside `gpui_kit::application().run(..)`, since a plain `fn main` would prove nothing about the bridge. `voice-me-tts` reads model files from a cache directory and never downloads them. The default build keeps `ort/load-dynamic` and no HTTP-client crate, so AD-8 holds for everything CI compiles.

**Never:** No `TtsPort::generate` wiring into the Speak Action, no queueing/serialization policy, no `AppState` backend plumbing, no notifications, no playback — Stories 2.6 and 2.9 own those. Do not implement any part of `voice-me-deps`. Do not attempt CUDA (see Decision 1). Do not add Chinese/Japanese/Korean/Hebrew preprocessing — AD-12 excludes them from v1.

**Decisions taken (human, 2026-09-21):**

1. **CUDA is off the table on this machine, on evidence, not preference.** The Quadro M1200 is GM107/sm_50; ONNX Runtime 1.28.2's prebuilt CUDA floor is sm_60 with no PTX target to JIT from, Arch ships only the Turing+ `nvidia-open` driver, and the 580 branch was the last to support Maxwell. A correct install would still fail at run.
2. **The GPU path is the WebGPU EP over Vulkan instead.** Both Vulkan devices are usable through Mesa with no proprietary driver — Intel HD 630 (ANV, `shaderFloat16 = true`) and the Quadro M1200 (NVK, `shaderFloat16 = false`). The Intel device is therefore where the FP16-vs-Q4 comparison happens; the Quadro is measured on Q4 only.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Turkish generation | `"Merhaba, bugün nasılsın?"`, lang `tr`, a reference clip | A 24 kHz mono f32 buffer of audible, recognizably-cloned Turkish | N/A |
| English generation | `"Hey, I'll be right back."`, lang `en`, same clip | Same, in English, from the same cloned voice | N/A |
| Non-wav reference | The wav re-encoded to mp3/flac/ogg | Decoded and resampled to 24 kHz mono f32; generation is unaffected | Unsupported container → domain error naming the format |
| Reference not 24 kHz | The existing 48 kHz stereo 31 s sample | Downmixed to mono and resampled to exactly 24 kHz before `speech_encoder` | N/A |
| Missing model file | Cache dir lacks `language_model_q4.onnx_data` | Session build fails with a domain error naming the missing path | Never a panic; the example prints the file list to fetch |
| Missing ONNX Runtime | No `libonnxruntime.so` at the configured path | Clear domain error naming the dylib and how to provision it | Never a panic |
| Stop token never emitted | Degenerate input, 256 tokens produced | Loop stops at `max_new_tokens` and still vocodes what it has | N/A |
| Empty text | `""` | No generation attempted; domain error | Rejected before any session runs |

</frozen-after-approval>

## Code Map

Read `_bmad-output/implementation-artifacts/spec-2-5-model-contract.md` first — it holds the verified graph I/O, the generation loop, the `ort` rc.13 API shapes, and this machine's execution-provider facts. Do not re-derive any of it.

- `crates/voice-me-core/src/audio.rs` (new) -- the AD-11 buffer. 24 kHz mono f32; `SAMPLE_RATE: u32 = 24_000`; a `samples: Vec<f32>` payload and a `duration()`. Export from `lib.rs`.
- `crates/voice-me-core/src/tokio_bridge.rs` (new) -- the AD-5 bridge: a GPUI `Global` holding a `tokio::runtime::Handle`, plus a `spawn_blocking` wrapper returning something awaitable from `cx.background_spawn`. `voice-me-core` already depends on `gpui-kit` (default-features off) for `TrayPort`'s context type — same precedent, see `ports.rs`.
- `crates/voice-me-core/src/ports.rs` -- `TtsPort::generate` returns `AudioBuffer` and takes text + reference-clip path + language; `VirtualMicPort::play` takes `&AudioBuffer`. Its doc comment still says "Chatterbox sidecar" — fix.
- `crates/voice-me-core/src/error.rs` -- add variants for model/session failure, a missing runtime asset (naming the path), and unsupported audio input. Follow the existing style: each variant documents *why* it is its own variant.
- `crates/voice-me-audio-linux/src/lib.rs`, `crates/voice-me-audio-windows/src/lib.rs` -- mechanical `todo!()` signature updates for the new `VirtualMicPort::play`, exactly as spec-2-1 did for `voice-me-tray-windows`. No behavior.
- `crates/voice-me-tts/src/` -- the real work: `reference.rs` (symphonia decode + rubato resample to 24 kHz mono f32), `tokenizer.rs` (language tag + `tokenizer.json`), `sessions.rs` (the four `ort` sessions, EP selection, built once), `generate.rs` (the KV-cached loop), `lib.rs` (`TtsAdapter`; `TtsPort::generate` stays `todo!()` — Story 2.6 owns lifecycle and sequencing).
- `crates/voice-me-tts/examples/tts-spike.rs` (new) -- the driver, inside `gpui_kit::application().run(..)`, dispatching generation through the AD-5 bridge and writing a wav for listening. Named `tts-spike`, not `spike`: cargo writes every workspace example into one shared directory (see `hotkey-spike.rs`'s own note).
- `crates/voice-me-hotkey-linux/examples/hotkey-spike.rs` -- the established example-spike shape to copy.
- `crates/voice-me-tray-linux/src/lib.rs` -- how an adapter holds long-lived state in a GPUI `Global`; the sessions need the same treatment.
- `_bmad-output/planning-artifacts/architecture/architecture-voice-me-2026-09-20/ARCHITECTURE-SPINE.md` -- Deferred section: three bullets to resolve (latency, ONNX Runtime provisioning, quantization quality) plus AD-9's CUDA assumption, which Decision 1 contradicts.
- `README.md` -- how to fetch the runtime assets and run the spike.
- `~/.local/share/voice-me/reference_voice_sample.wav` -- the existing 48 kHz stereo 31 s clip; `ffmpeg` is available to make the non-wav variants.

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-core/src/audio.rs`, `src/ports.rs`, `src/error.rs`, `src/lib.rs` -- the AD-11 buffer, the two port signatures that carry it, and the new error variants -- fixes the format guess AD-11 exists to prevent
- [x] `crates/voice-me-audio-linux/src/lib.rs`, `crates/voice-me-audio-windows/src/lib.rs` -- mechanical stub signature updates -- keeps `cargo check --workspace` green
- [x] `crates/voice-me-core/src/tokio_bridge.rs` -- the AD-5 bridge, one shared utility, with a test that a `spawn_blocking` result comes back
- [x] `crates/voice-me-tts/src/reference.rs` -- decode any supported container to 24 kHz mono f32, with tests for the downmix and the resample ratio
- [x] `crates/voice-me-tts/src/tokenizer.rs` -- language tag + `tokenizer.json`, with a test asserting the exact template `[6563, 255, …, 0, 6561, 6561]` and that `[tr]`/`[en]` are single tokens
- [x] `crates/voice-me-tts/src/sessions.rs` -- build the four sessions from a cache dir, EP selectable (CPU / WebGPU device 0 / WebGPU device 1), missing files reported as domain errors naming the path
- [x] `crates/voice-me-tts/src/generate.rs` -- the KV-cached loop per the contract: prefill positions, repetition penalty over all prior tokens, greedy argmax, stop token, 60 KV tensors threaded `present.{L}` → `past_key_values.{L}`, then vocode
- [x] `crates/voice-me-tts/Cargo.toml` -- `ort` default `load-dynamic`; an off-by-default `webgpu-probe` feature selecting `download-binaries` + `webgpu` instead -- keeps AD-8 true for everything CI builds
- [x] `crates/voice-me-tts/examples/tts-spike.rs` -- the GPUI-hosted driver: generate, time it, write a wav
- [x] `_bmad-output/planning-artifacts/.../ARCHITECTURE-SPINE.md`, `README.md` -- record the measurements, the required runtime-file list, and Decision 1's contradiction of AD-9's CUDA assumption

**Acceptance Criteria:**
- Given the provisioned cache directory and the existing reference clip, when the spike runs for Turkish and for English, then each writes a wav that is audible and recognizably the cloned voice, with no Python installed or invoked anywhere in the process
- Given generation runs, when session construction and per-utterance inference are timed separately, then both numbers are recorded per execution provider measured — CPU/Q4, WebGPU/Intel, WebGPU/NVK — confirming or refuting AD-10's build-once/hold decision, and the WebGPU numbers are only reported as GPU numbers after node placement is confirmed from ORT's logs
- Given Q4 and FP32 outputs for the same text and clip, when both are listened to, then the spec records whether quantization degrades the cloned voice audibly; FP16 is additionally compared on the Intel device, which is the only one here with `shaderFloat16`
- Given the spike ran, when the architecture doc is updated, then it names every ONNX Runtime library, execution-provider library, and model file that had to be present for each path — the input Story 3.2's provisioning work depends on — and states the viability outcome
- Given `cargo check --workspace` and `cargo test --workspace` with default features, then both pass with no `ort` binary download and no network access

## Implementation Notes

**Outcome: AD-12 holds.** Chatterbox-Multilingual V3 generates audible, cloned
Turkish and English speech in-process on ONNX Runtime from Rust, with no
Python installed or invoked anywhere in the process, driven from inside
`gpui_kit::application().run(..)` through the AD-5 bridge. Every wav below
clones the user's own Reference Voice Sample
(`~/.local/share/voice-me/reference_voice_sample.wav`, 48 kHz stereo 31 s),
which also exercises the downmix-and-resample path.

### Measurements (i7-7700HQ 4C/8T, 15 GB, 2026-09-21)

`"Merhaba, bugün nasılsın?"` unless noted; one process per row.

| Variant / provider | Session build | `speech_encoder` | `language_model` | `conditional_decoder` | Utterance total | Audio |
| --- | --- | --- | --- | --- | --- | --- |
| **Q4 / CPU** | 89.7 s | 1.37 s | 1.43 s (51 tok, 28.0 ms/tok) | 17.65 s | **20.49 s** | 2.00 s |
| Q4 / CPU, English | 86.5 s | 1.19 s | 1.20 s (47 tok, 25.5 ms/tok) | 16.71 s | 19.12 s | 1.84 s |
| Q4 / CPU, mp3 reference | 91.1 s | 1.59 s | 1.67 s (51 tok, 32.7 ms/tok) | 18.64 s | 21.93 s | 2.00 s |
| FP32 / CPU | 93.6 s | 1.68 s | 6.58 s (61 tok, 107.9 ms/tok) | 19.83 s | 28.14 s | 2.40 s |
| FP16 / CPU | 88.1 s | 1.55 s | 22.02 s (61 tok, 361.0 ms/tok) | 18.51 s | 42.11 s | 2.40 s |
| FP16 / WebGPU Intel HD 630 | 110.0 s | 2.56 s | 22.04 s (64 tok, 344.4 ms/tok) | 49.12 s | 73.76 s | 2.52 s |

Short-line control, `"Merhaba"`, same binary (the `webgpu-probe` build) so the
three providers are directly comparable:

| Provider (Q4) | Session build | `language_model` | `conditional_decoder` | Total |
| --- | --- | --- | --- | --- |
| CPU | 76.5 s | 30 tok, **24.9 ms/tok** | 12.71 s | 14.38 s |
| WebGPU, Intel HD 630 (ANV) | 94.4 s | 30 tok, 311.3 ms/tok | 40.21 s | 53.88 s |
| WebGPU, Quadro M1200 (NVK) | 108.8 s | 30 tok, 1173.9 ms/tok | 177.06 s | 219.58 s |

**AD-10's build-once/hold decision is confirmed, emphatically.** Session
construction is 76–110 s against a ~20 s utterance. It is also large enough
that Story 2.6 cannot treat it as a lazy first-call cost quietly — it needs an
explicit warm-up and a visible "still loading" state.

**The token loop is not the bottleneck — `conditional_decoder` is,** at 86 % of
a Q4/CPU utterance. Its cost tracks the *concatenated* sequence (the ~150
cropped reference tokens plus the generated ones), so it is near-constant per
utterance: `"Merhaba"` costs 12.7 s against 17.6 s for a full sentence. Future
latency work belongs there, not in the decode loop — which also means
Chatterbox-Nano (a smaller *language model*) would not move the number much.

**Real-time factor is 0.10×.** Not usable in a live voice chat as it stands;
usable for the PRD's type-a-line interaction only because AD-10 makes the
Speak Action asynchronous. Whether ~20 s per utterance is acceptable at all is
a product decision this spike deliberately does not make.

### Quantization quality

Wavs for the listening check, all from the user's own clip, at
`~/.local/share/voice-me/spike-2-5/`:

| File | What it is |
| --- | --- |
| `tr-q4-cpu.wav` | the shipping CPU path, Turkish |
| `en-q4-cpu.wav` | the shipping CPU path, English |
| `tr-fp32-cpu.wav` | the unquantized quality baseline |
| `tr-fp16-cpu.wav` | FP16, same line |
| `tr-fp16-webgpu-intel.wav` | FP16 on the only device here with `shaderFloat16` |
| `tr-q4-cpu-from-mp3-reference.wav` | Q4 from the mp3 re-encode of the same clip |
| `short-tr-q4-{cpu,webgpu-intel,webgpu-nvk}.wav` | the `"Merhaba"` provider control |

**Revised after the review pass — the original divergence was mostly a bug,
not quantization.** The first measurements had Q4 at 51 tokens (2.00 s) against
FP32/FP16's 61 (2.40 s), and concluded Q4 read the sentence differently. With
the repetition penalty fixed to span the full 8194-wide logits head (see the
Review Triage Log), the same prompt gives **Q4 59 tokens / 2.32 s and FP32 57
tokens / 2.24 s** — they now agree closely, and Q4 is no longer the odd one
out. Both figures are stable across repeat runs. Treat every pre-review number
in the tables above as measured under that bug: the *timings* stand, since the
penalty costs nothing measurable, but the token counts and audio lengths do
not. The fresh wavs are `tr-q4-cpu-postreview.wav` and
`tr-fp32-cpu-postreview.wav`; FP16 was not re-measured. **The audible verdict is the
user's and is not recorded here yet.** It cannot be automated either: the
decoder's unseeded `randn_like` means no two runs are byte-identical, which is
why the tests assert shape, rate, duration and non-silence instead.

### Runtime-file list (the Story 3.2 input)

For the Linux CPU path, 1.56 GB plus an 8.7 MB runtime:

- `libonnxruntime.so` from `onnxruntime-linux-x64-1.28.2.tgz` — the core
  library only; the tarball ships **no** execution-provider libraries, and
  none are needed *for the CPU path*. This is the only set Story 2.5 could
  confirm, not a decision to ship CPU-only — the CUDA set is deferred to
  hardware that can run it (below).
- `<cache>/tokenizer.json`
- `<cache>/onnx/speech_encoder.onnx` + `.onnx_data` (592 MB)
- `<cache>/onnx/embed_tokens.onnx` + `.onnx_data` (68 MB)
- `<cache>/onnx/language_model_q4.onnx` + `.onnx_data` (354 MB)
- `<cache>/onnx/conditional_decoder.onnx` + `.onnx_data` (534 MB)

`ModelCache::required_files` is that list in code, and the example prints it
with each entry marked present/missing rather than failing on the first.
Verified by pointing `VOICE_ME_MODEL_CACHE` at an empty directory.

### Decision 1 against AD-9 — GPU deferred here, not dropped

CUDA was skipped because this dev machine cannot run it (Decision 1), not
because it was eliminated as an approach — AD-9's CUDA/FP16 backend is still
the plan and stays unverified until there is an sm_60-or-newer GPU to measure
on. WebGPU was attempted, and **runs**: 24.9 ms/tok
on CPU against 311 ms/tok on the Intel HD 630 and 1173.9 ms/tok on the Quadro
via NVK. This is a real GPU run, not a silent fallback — ORT's node-placement
log puts 183 of `language_model_q4`'s 189 nodes on `WebGpuExecutionProvider`,
including all 30 `GroupQueryAttention`, all 151 `MatMulNBits` and all 60
`SkipSimplifiedLayerNormalization`, leaving six attention-mask shape ops on
CPU. The `com.microsoft` contrib ops are therefore *not* the obstacle; the
hardware is. The Quadro additionally loses its Vulkan device
(`VK_ERROR_DEVICE_LOST` from NVK) partway through `conditional_decoder` on
anything longer than a single word.

AD-9's rule stands, but its "FP16 on GPU" half has no reachable backend on
Linux here and must not be treated as verified.

### Three traps found while measuring

1. **`webgpu-probe` as originally written could not reach WebGPU at all.**
   Cargo features are additive: `--no-default-features --features webgpu-probe`
   could not unset the `load-dynamic` flag glued onto the `ort` dependency
   line, and with `load-dynamic` on, ort-sys skips its download entirely and
   builds a CPU-only binary that still prints `provider: webgpu:0`. Fixed by
   moving `load-dynamic` behind a default `dynamic-runtime` feature, which is
   what makes the spec's own verification command mean what it says.
   `init_runtime` is `cfg`-split accordingly — there is no dylib to resolve in
   a statically linked build.
2. **`ort::ep::WebGPU::with_device_id` had no observable effect.** Both `0` and
   `1` landed on the discrete adapter, and so did `MESA_VK_DEVICE_SELECT`. The
   only thing that chose a device was restricting the Vulkan loader:
   `VK_DRIVER_FILES=/usr/share/vulkan/icd.d/intel_icd.json`. The probe build
   also needs `LD_LIBRARY_PATH` pointing at pyke's `libwebgpu_dawn.so` in
   `~/.cache/ort.pyke.io/dfbin/...`, which is installed nowhere.
3. **`.error_on_failure()` guards EP registration only.** Node placement has to
   be read out of ORT's verbose log, which is why `--verbose-placement` exists.
   Verbose logging cost nothing measurable here — the verbose and non-verbose
   WebGPU runs timed identically (1173.3 vs 1173.9 ms/tok) — so the placement
   evidence and the timings come from the same configuration.

### Edge cases from the I/O matrix

All verified, none by inspection alone:

- **Non-wav reference** — mp3 generated an identical 51-token, 2.00 s
  utterance to the wav. flac and ogg decode to the same 24 kHz mono audio,
  covered by `tests/reference_containers.rs` (opt-in via `VOICE_ME_TEST_CLIPS`,
  so `cargo test --workspace` stays fixture-free). A non-audio file reports
  `UnsupportedAudioInput` naming the container rather than panicking.
- **Reference not 24 kHz** — the 48 kHz stereo clip decodes to 30.91 s of
  24 kHz mono every run. A hand-written RIFF fixture in `reference.rs` covers
  the whole probe→decode→downmix→resample path with no encoder and no
  network.
- **Missing model file** — `MissingRuntimeAsset` naming the absolute path, plus
  the full nine-file list, before ONNX Runtime opens anything.
- **Missing ONNX Runtime** — `MissingRuntimeAsset` naming the dylib when the
  path is wrong; a `SpeechEngine` error naming `ORT_DYLIB_PATH` and pointing at
  the README when it is unset. Never a panic.
- **Empty text** — `VoiceMeError::EmptyText` from both `TextTokenizer::encode`
  and `generate`, the latter before any session runs.
- **Stop token never emitted** — the loop's `max_new_tokens` bound and
  `stopped_on_token` flag are exercised by the token accounting (every run
  here stopped on the token); the vocoder path is shared either way, since
  only the trailing 6562 is conditionally stripped.

### What is left open

- **Listening verdict (user, 2026-09-21): every path is stable except WebGPU
  on the Quadro/NVK, whose audio is audibly corrupted.** This closes the
  acceptance criterion for audible, recognizably-cloned speech on the CPU
  paths, and it upgrades the NVK finding from "unusably slow" to "produces
  wrong output": the `VK_ERROR_DEVICE_LOST` the run reported partway through
  `conditional_decoder` is not a cosmetic teardown warning, it corrupts the
  waveform. NVK on Maxwell is therefore disqualified on correctness, not just
  on speed — a `local-webgpu` variant must treat a device that loses context
  mid-inference as a failure to surface, never as audio to play.

- **Quality verdict (user, 2026-09-21, on the post-review wavs): FP32 sounds
  better than Q4.** This reverses the working assumption that Q4 is the CPU
  default, and the cost of honouring it is far smaller than the headline
  "Q4 is 3.9× faster" suggests — that ratio is over the *language model*,
  which is only ~8 % of an utterance. End to end it is **19.13 s (Q4) against
  22.59 s (FP32), i.e. ~18 % slower**, because `conditional_decoder` dominates
  both and is indifferent to the weight variant.

  Nor is distribution the obstacle it first appeared to be. `language_model.onnx_data`
  is **2.08 GB**, over GitHub Releases' 2 GB per-asset limit — but mirroring
  there was only ever a convenience. AD-7 has been revised accordingly: an
  oversized asset is fetched by direct static URL from the MIT-licensed
  Hugging Face origin at the pinned revision, using the same resumable ranged
  download Story 3.2 owes anyway. FP32 is therefore the intended CPU default.
  **`language_model_q4f16` (304 MB) was never tested** and is the obvious
  candidate before accepting either horn of that trade — it is smaller than Q4
  and may carry most of FP32's quality. Resolve before Story 2.6 fixes a
  default and before Story 3.2 decides what to provision.
- **Windows** is untouched: no DirectML measurement, and both audio stubs are
  signature-only updates.
- `TtsPort::generate` stays `todo!()` by design — Story 2.6 owns session
  lifecycle and sequencing, and now has real numbers to design against.

### Verification pass after implementation (build workflow, step-03)

The matrix audit found one row whose covering evidence was not a test that
runs: **stop token never emitted**. The loop's `max_new_tokens` bound was
correct and the trailing-6562 strip was conditional, but every measured run
stopped on the token, so the capped path was reasoned about rather than
exercised — deleting the condition would have clipped a token off every capped
utterance with the whole suite green.

Closed by extracting the trim decision into `generate::speech_body`, mirroring
how `overlay_window_kind_for` and `session_kind_for` were extracted for the
same reason, and asserting both branches plus the produced-nothing case.
`voice-me-tts` now has 29 tests.

Also: `tests/reference_containers.rs` was run for real with
`VOICE_ME_TEST_CLIPS` pointed at the mp3/flac/ogg re-encodes — all three decode
to the same 24 kHz mono audio. It stays opt-in, so `cargo test --workspace`
does not cover that row on its own.

One clippy warning this story introduced (`explicit_counter_loop` on the decode
loop) is now an `#[allow]` with a reason: `total_len` is the attention-mask
length the model is given, not a loop counter, and clippy's `(step_len..).zip(..)`
rewrite hides that. `cargo clippy --workspace` is clean.

## Spec Change Log

## Review Triage Log

Targeted correctness review of `generate.rs` against the model contract
(2026-09-21), scoped to the decode loop because its only validation was "the
audio sounds right" — the decoder's unseeded `randn_like` rules out a golden-wav
test, so a subtle arithmetic error would degrade quality with every test green.

**Routed to patch**

- **The repetition penalty could not reach most of the logits head** — `medium`,
  and it changed real output. `penalised` was sized `STOP_SPEECH_TOKEN + 2`
  (6564) while `language_model`'s `logits` are **8194** wide, so any generated
  token at index ≥ 6564 was neither penalised on lookup nor recorded on
  generation — `.get()`/`.get_mut()` made both a silent no-op rather than a
  panic. Verified by measurement, not by reading: with the penalty spanning the
  full head, the same prompt went from a stable 51 tokens / 2.00 s to a stable
  59 tokens / 2.32 s (two runs each), and FP32 from 61 / 2.40 s to 57 / 2.24 s.
  The model was emitting out-of-nominal-range tokens and repeating them
  unsuppressed, truncating utterances early. Route: patch — `SPEECH_HEAD_VOCAB`.
- **A drifted `tokenizer.json` would corrupt positions silently** — `medium`.
  `prefill_positions` gives index 0 the position `index - 1` = **-1** unless the
  first token is ≥ 6561, which holds only because the export's
  `TemplateProcessing` puts `EXAGGERATION` there. A negative position is an
  out-of-range gather into `text_pos_emb`. Nothing validated this at run time:
  the template test pins the *vendored fixture*, while the file actually loaded
  comes from the provisioned cache — and the model card's own discussions (#4,
  #10) record the tokenizer and the graphs drifting apart before. Route: patch —
  `template_is_intact`, checked in `encode`, with four tests.

**Rejected**

- **`speech_body` strips the trailing token only when a stop token was emitted,
  where the reference implementation always slices `[1:-1]`** — verified, and
  deliberately kept. On a run that hit `max_new_tokens` the final token is real
  speech, so the reference's unconditional trim would clip every capped
  utterance by one token. Every observed run stops on the token, so the two
  never differ in practice; the divergence is documented at the function.

**Verified correct against the contract, no change**

Prefill positions (the `-1`, and both `6561`/`6563` at position 0) · penalty
sign handling (`s<0 ? s*p : s/p`) · the opening `6561` seeded into the penalty
set · 60 KV tensors positionally aligned `present.{L}.{kv}` → `past_key_values.{L}.{kv}`
· f16 KV allocation for the fp16 variant, f32 otherwise · `attention_mask` as
i64 ones of `total_len`, growing by one per step · decode-step position
`step + 1` · conditioning prefix concatenated ahead of the prompt embeddings ·
`audio_tokens` prepended to the vocoder input · constants 6561/6562/6563/30
layers/`[1,16,0,64]` init.


## Design Notes

**Why the loop is real code and the example is thin.** The AC already forces permanent contract into `voice-me-core` (AD-11, AD-5). Given that, putting the loop in `examples/` only guarantees Story 2.6 rewrites it. `TtsPort::generate` staying `todo!()` is the honest seam: this story proves the engine runs, 2.6 owns when it runs.

**Getting the assets onto the machine.** No HTTP-client crate enters the workspace. The README documents plain `curl` commands into `~/.cache/voice-me/` — the ONNX Runtime 1.28.2 linux-x64 tarball, and the model files from the pinned revision `452d3f434aa592098f1eedac9099f33642ab2da5`. The spike reads `ORT_DYLIB_PATH` and a cache path, and when a file is absent it says which one. That missing-file list *is* the Story 3.2 deliverable, so make the error precise.

**Download budget:** ~1.56 GB for the shared graphs + Q4, ~2.24 GB adding FP16, ~3.28 GB adding the FP32 baseline. 82 GB free. The FP32 copy exists only for the quality comparison and can be deleted afterwards.

**Measure in this order, cheapest first:** CPU/Q4 end to end → Q4-vs-FP32 quality on CPU → WebGPU feasibility probe (does Dawn initialize, and do the contrib ops actually land on the GPU) → WebGPU timings per device → FP16 on Intel. Stop early and record the reason if a stage is a dead end; a documented dead end is a valid spike result.

**Do not write a golden-wav equality test.** `conditional_decoder` bakes in an unseeded `randn_like` for its flow-matching prior, so the same inputs give slightly different audio every run. Assert shape, sample rate, duration bounds, and non-silence.

**Free the KV tensors per utterance.** transformers.js leaked a full KV cache per `generate` call and reached ~24 GB. With 30 layers × 2 tensors growing every step, this matters here.

## Verification

**Commands:**
- `cargo check --workspace` -- expected: type-checks with the changed `TtsPort`/`VirtualMicPort` signatures and both mechanically-updated stubs
- `cargo test --workspace` -- expected: the tokenizer-template, reference-decode, and bridge tests pass with no model files and no network
- `cargo run -p voice-me-tts --example tts-spike -- tr "Merhaba, bugün nasılsın?"` -- expected: writes a wav, prints session-build and per-utterance timings
- `cargo run --no-default-features --features webgpu-probe -p voice-me-tts --example tts-spike -- ...` -- expected: the WebGPU probe, with ORT node-placement logging enabled

**Manual checks (if no CLI):**
- Listen to the Turkish and English wavs: audible, intelligible, recognizably the reference voice
- Listen to Q4 against FP32 (and FP16 on the Intel device) for the same line; record whether the difference is audible
- Confirm from ORT's logs that the `language_model` nodes actually ran on WebGPU rather than falling back to CPU before quoting any GPU timing
