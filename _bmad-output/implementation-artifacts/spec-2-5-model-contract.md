# Chatterbox-Multilingual ONNX — Model Contract (reference for spec-2-5)

Compiled 2026-09-21 from the HF model card, the
[export script](https://github.com/VladOS95-cyber/onnx_conversion_scripts/tree/main/chatterbox),
and direct protobuf inspection of the downloaded graph headers. Everything
here is verified, not inferred. Pin the repo revision
`452d3f434aa592098f1eedac9099f33642ab2da5` of
`onnx-community/chatterbox-multilingual-ONNX` (MIT) — the tokenizer and the
graphs have drifted against each other before (HF discussions #4, #10).

## Files

All graphs live in a flat `onnx/` directory; `tokenizer.json` is at the repo
root. Every graph stores its weights in an external `<name>.onnx_data` whose
`location` is a **bare relative filename**, so the `_data` file must sit next
to its `.onnx` and the session must be built from a *path*
(`commit_from_file`), never from an in-memory byte slice.

| File | Size |
| --- | --- |
| `onnx/speech_encoder.onnx` + `.onnx_data` | 1.2 MB + 591 MB |
| `onnx/embed_tokens.onnx` + `.onnx_data` | 13 KB + 68 MB |
| `onnx/language_model_q4.onnx` + `.onnx_data` | 228 KB + 354 MB |
| `onnx/language_model_fp16.onnx` + `.onnx_data` | 173 KB + 1040 MB |
| `onnx/language_model.onnx` + `.onnx_data` (FP32 baseline) | 171 KB + 2081 MB |
| `onnx/conditional_decoder.onnx` + `.onnx_data` | 6.4 MB + 534 MB |
| `tokenizer.json` | 72 KB |
| `default_voice.wav` (24 kHz known-good reference clip) | 714 KB |

Shared set (encoder + embed + decoder + tokenizer) = 1.20 GB. Plus Q4 =
1.56 GB; plus FP16 = 2.24 GB; plus FP32 = 3.28 GB.

## Graph I/O

**`speech_encoder.onnx`** (opset 20)

| dir | name | dtype | shape |
| --- | --- | --- | --- |
| in | `audio_values` | f32 | `[batch, num_samples]` — **24 kHz mono**, uncropped |
| out | `audio_features` | f32 | `[batch, seq, 1024]` (the `cond_emb` prefix) |
| out | `audio_tokens` | i64 | `[batch, audio_seq]` (the `prompt_token`) |
| out | `speaker_embeddings` | f32 | `[batch, 192]` |
| out | `speaker_features` | f32 | `[batch, feature_dim, 80]` |

The graph holds a fixed internal 24k→16k resample, so the input must literally
be 24 kHz. It crops internally (10 s @24 k for mel, 6 s @16 k for cond tokens).

**`embed_tokens.onnx`** (opset 20)

| dir | name | dtype | shape |
| --- | --- | --- | --- |
| in | `input_ids` | i64 | `[batch, seq]` |
| in | `position_ids` | i64 | `[batch, seq]` |
| in | `exaggeration` | f32 | `[batch]` |
| out | `inputs_embeds` | f32 | `[batch, seq, 1024]` |

Embedding tables: `text_emb [2352,1024]`, `speech_emb [8194,1024]`,
`text_pos_emb [2050,1024]`, `speech_pos_emb [4100,1024]` — hard caps of ~2049
text positions and ~4099 speech positions.

**`language_model{,_fp16,_q4}.onnx`** (opset 21 + `com.microsoft` 1, IR 10) —
62 inputs, 61 outputs, 30 layers × 16 KV heads × head_dim 64.

| dir | name | dtype (fp32/q4) | dtype (fp16) | shape |
| --- | --- | --- | --- | --- |
| in | `inputs_embeds` | f32 | **f32** | `[batch, seq, 1024]` |
| in | `attention_mask` | i64 | i64 | `[batch, total_seq]` |
| in | `past_key_values.{0..29}.{key,value}` | f32 | **f16** | `[batch, 16, past_seq, 64]` |
| out | `logits` | f32 | **f32** | `[batch, seq, 8194]` |
| out | `present.{0..29}.{key,value}` | f32 | **f16** | `[batch, 16, total_seq, 64]` |

`present.{L}.{kv}` maps positionally onto `past_key_values.{L}.{kv}`.
`seqlens_k` for `GroupQueryAttention` is derived inside the graph from
`attention_mask`; nothing extra is passed.

**`conditional_decoder.onnx`** (opset 17)

| dir | name | dtype | shape |
| --- | --- | --- | --- |
| in | `speech_tokens` | i64 | `[batch, num_speech_tokens]` |
| in | `speaker_embeddings` | f32 | `[batch, 192]` |
| in | `speaker_features` | f32 | `[batch, feature_dim, 80]` |
| out | `waveform` | f32 | `[batch, num_samples]` @ 24 kHz |

CFM settings (`n_timesteps` 10, `inference_cfg_rate` 0.7, cosine schedule) and
a 480-sample cosine fade-in are baked in. It discards the prompt-mel region
itself, so the output is new speech only.

## Constants

`S3GEN_SR` 24000 · `S3_SR` 16000 · `S3_TOKEN_RATE` 25 tok/s ·
`SPEECH_VOCAB_SIZE` 6561 · `START_SPEECH_TOKEN` 6561 · `STOP_SPEECH_TOKEN` 6562 ·
`EXAGGERATION_TOKEN` 6563 · `ENC_COND_LEN` 6×16000 · `DEC_COND_LEN` 10×24000.

Defaults: `exaggeration` 0.5, `max_new_tokens` 256, `repetition_penalty` 1.2.

## Text side

Prepend a literal bracketed tag to the string before tokenizing:
`format!("[{}]{}", lang.to_lowercase(), text)`. These are real single tokens —
`[en]` = 708, `[tr]` = 712.

`tokenizer.json` (BPE, 2454 entries) normalizes `" "` → `"[SPACE]"` and its
`TemplateProcessing` wraps every sequence as
`[6563, 255, …text…, 0, 6561, 6561]` — EXAGGERATION, BOS, text, EOS, then
**two** START_SPEECH. Loading it with the `tokenizers` crate and calling
`encode(s, true)` reproduces this for free; hand-rolling it means
reimplementing the template exactly.

Turkish and English need no further preprocessing. zh/ja/ko/he do (AD-12
excludes them from v1).

## Generation loop

1. `inputs_embeds = embed_tokens(input_ids, position_ids, [exaggeration])`.
   Prefill positions are `where(input_ids >= 6561, 0, arange(seq) - 1)` — note
   the `-1`, and that both 6561 and 6563 take position 0.
2. Step 0 only: run `speech_encoder` once, then
   `inputs_embeds = concat([audio_features, inputs_embeds], axis=1)`;
   initialise all 60 KV tensors as zeros `[1,16,0,64]`;
   `attention_mask = ones([1, seq])`.
3. `logits, *present = language_model(inputs_embeds, attention_mask, **past)`;
   take `logits[:, -1, :]`.
4. Repetition penalty 1.2 over **all** previously generated speech tokens,
   the initial 6561 included: `s < 0 ? s * 1.2 : s / 1.2` at those ids.
5. `next = argmax(...)` — pure greedy. The ONNX export has **no** temperature,
   top-p, or cfg-weight knob.
6. Stop on 6562 or `max_new_tokens`.
7. Feed back only `next` as `[1,1]` with `position_ids = [[i + 1]]`, append a
   `1` to `attention_mask`, and move `present.{L}.{kv}` into
   `past_key_values.{L}.{kv}`.

Vocoding: `speech_tokens = concat([audio_tokens, generated[1..-1]])` — strip
the leading 6561 and the trailing 6562 — then `conditional_decoder(...)` and
`squeeze(0)`.

## Gotchas

- Every `language_model` variant is ORT-optimised and uses `com.microsoft`
  contrib ops (`GroupQueryAttention` ×30, `SkipSimplifiedLayerNormalization`
  ×60, and for q4 `MatMulNBits` ×151). A minimal ORT build without contrib ops
  cannot load them.
- **FP16 is not a plain dtype swap and is effectively GPU-only.** Explicit
  `Cast` nodes keep `inputs_embeds` and `logits` at f32; only the 60 KV
  tensors are f16, and fp16 `GroupQueryAttention` is a CUDA/DirectML path.
- **Q4 needs no dtype change at all** — its graph I/O is identical to FP32,
  only the weights are 4-bit. This is the CPU path.
- IR 10 needs ORT ≥ 1.18; the model card pins `onnxruntime==1.22.1`.
- The decoder bakes in `randn_like` for the flow-matching prior with no seed
  input, so **identical inputs give slightly different audio each run** — do
  not write a byte-equality test against a golden wav.
- transformers.js leaked a full KV cache per `generate` call
  ([#1734](https://github.com/huggingface/transformers.js/issues/1734)); reuse
  or free the 60 KV tensors per utterance rather than allocating fresh.
- TensorRT conversion fails for the T3/S3 graphs (dynamic shapes + contrib
  ops) — only the voice encoder converts.

## `ort` 2.0.0-rc.13 shapes that bite

Verified by compiling against the crate, not from memory.

- Feature is `load-dynamic`. The dylib path comes from either the
  `ORT_DYLIB_PATH` env var or `ort::init_from(path)` — there is **no**
  `.with_dylib_path()` builder. `EnvironmentBuilder::commit()` returns
  `bool`, not `Result`, and the environment is immutable once committed.
- `Session::builder()` returns a `Result`, and **every** `with_*` on
  `SessionBuilder` also returns a `Result` (`BuilderResult`), so each needs
  `?`. `commit_from_file` takes `&mut self`. `run` takes `&mut self`.
- `ort::inputs!` does **not** return a `Result` — the `?` belongs to the
  `Tensor::from_array(..)?` calls inside it. The named arm
  (`inputs!["a" => v]`) yields a plain `Vec<(Cow<str>, SessionInputValue)>`
  you can `.push()` onto, which is what makes threading 60 KV entries
  tractable.
- `SessionOutputs<'s>` borrows the session mutably, so **it must be dropped
  before the next `run`** or it is a hard `E0499`. `outputs.remove(name)`
  hands back an owned `DynValue` via an `Arc` bump — no tensor copy. Extract
  everything needed, then `drop(outputs)`.
- Build tensors with `Tensor::from_array((shape, vec))?` (owned, no copy) or
  `TensorRef::from_array_view((shape, &slice))?` (borrowed). Read them with
  `try_extract_tensor::<T>() -> (&Shape, &[T])` or
  `try_extract_array::<T>() -> ArrayViewD<T>`.
- EP builders are in `ort::ep` (`ort::execution_providers` is deprecated):
  `ep::CPU`, `ep::CUDA`, `ep::WebGPU`, … `.build()`, and
  `.error_on_failure()` to turn a silent CPU fallback into a hard error —
  use it, or a "GPU" measurement may quietly be a CPU one.
- ORT 1.28.2 linux-x64 CPU tarball is 8.7 MB and ships only the core
  `libonnxruntime.so.1.28.2` (no provider libs):
  `https://github.com/microsoft/onnxruntime/releases/download/v1.28.2/onnxruntime-linux-x64-1.28.2.tgz`.
  The CUDA 12 build is 404 MB, CUDA 13 is 230 MB.
- Pin `ndarray = "0.17"` — that is what ort rc.13 itself depends on.

## Supporting crates

- `tokenizers` 0.23.2, `default-features = false, features = ["onig"]`.
  `Tokenizer::from_file(p)?` then `encode(text, true)?` / `get_ids()`. Its
  error is `Box<dyn Error + Send + Sync>`.
- `symphonia` 0.6.1 — only `mp3` is a non-default flag; flac/ogg/vorbis/wav
  are already on. 0.6 changed the API: `next_packet()` returns
  `Result<Option<Packet>>`, `packet.track_id` is a field,
  `default_track(TrackType::Audio)`, `track.codec_params` is an
  `Option<CodecParameters>` enum to match on, and decoding yields a
  `GenericAudioBufferRef` with `copy_to_vec_interleaved(&mut Vec<f32>)`.
  Pre-0.6 tutorials will not compile.
- `rubato` 5.0.0 — `SincFixedIn`/`FftFixedIn` are **gone**; the types are
  `Async` (`new_sinc`/`new_poly`) and `Fft`, and buffers go through
  `rubato::audioadapter_buffers`. For a fixed `in_rate → 24000` offline
  conversion use `Fft::new(in_rate, 24000, chunk, 1, FixedSync::Input)?` and
  `process_all(&adapter, len, None)?`, which trims the startup delay.

## This machine's execution-provider reality (measured 2026-09-21)

CPU: i7-7700HQ, 4C/8T, AVX2 + FMA, no AVX-512. 15 GB RAM, 82 GB free disk.

**CUDA is not reachable and should not be attempted.** The Quadro M1200 is
GM107 = **sm_50**. ONNX Runtime 1.28.2's CI builds its CUDA 12 binaries for
`60-real;70-real;75-real;80-real;86-real;90-real;120-real;120-virtual` and its
CUDA 13 binaries for sm_75+, so the prebuilt floor is **sm_60** with no PTX
target below `120-virtual` to JIT from — a correct CUDA install would still
fail at run with "no kernel image is available". Separately, Arch ships only
`nvidia-open` (Turing+), and the 580 branch was the last to support Maxwell.

**Vulkan is reachable, on two devices**, via the WebGPU EP (Dawn → Vulkan):

| Vulkan device | Driver | API | `shaderFloat16` | Usable LM variants |
| --- | --- | --- | --- | --- |
| Intel HD 630 (KBL GT2) | Mesa ANV | 1.4 | **true** | q4, fp16, fp32 |
| Quadro M1200 (GM107) | Mesa **NVK** | 1.3 | false | q4, fp32 |

Select between them with `ep::WebGPU::default().with_device_id(i32)`. Neither
has dedicated fast VRAM worth counting on — the Intel part shares system RAM
and Gen9 GT2 is roughly 0.4 TFLOPS fp32, so it may well measure *slower* than
the CPU for a 0.5 B transformer. That is the thing being measured.

**Two traps when measuring a WebGPU run:**

1. `.error_on_failure()` catches EP *registration* failure only. It does not
   catch per-node fallback: any op the WebGPU EP lacks a kernel for silently
   runs on CPU, and the `language_model` graphs are full of `com.microsoft`
   contrib ops (`GroupQueryAttention` ×30, `SkipSimplifiedLayerNormalization`
   ×60, `MatMulNBits` ×151 on q4). Confirm actual node placement from ORT's
   own logging before calling any number a GPU number.
2. Microsoft's `onnxruntime-linux-x64-*.tgz` contains **no** WebGPU provider,
   so `load-dynamic` against it cannot reach WebGPU at all. pyke's prebuilt
   `x86_64-unknown-linux-gnu+webgpu` distribution (13.7 MB, bundles Dawn) is
   reachable through ort's `download-binaries`, but it is static-link +
   download-at-build — the opposite of what AD-8/AD-12 mandate. An
   architecture-clean WebGPU path needs ONNX Runtime built from source with
   `--use_webgpu --build_shared_lib` and mirrored per AD-7.
