# voice-me

Project scaffold in progress — being defined step by step with BMAD-METHOD.

## Running

```bash
cargo run -p voice-me-app
```

The app is tray-resident: closing the Settings window leaves it running in
the tray, and "Settings…" in the tray menu brings the window back.

Pressing the global hotkey summons the prompt overlay: a borderless window
with a single focused field. Type a line and press `Enter` to send it;
`Escape`, or clicking away, closes it and discards what you typed. Dismissing
the overlay never quits the app — it stays in the tray either way.

A confirmed line is now actually generated (Story 2.6), in the speech
language from `settings.toml` (`speech_language`, default `tr`), in the voice
of the saved Reference Voice Sample. That needs the runtime assets below to
be provisioned; without them the app starts normally and a desktop
notification names the exact missing file the first time you press the
hotkey.

Generation runs on Tokio's blocking pool, never on the UI thread, and exactly
one runs at a time — press the hotkey again mid-generation and the second
line is queued. The sessions take ~90 s to build and are built once, eagerly
at startup when a Reference Voice Sample already exists; a Speak Action that
lands while that is still happening gets one "still getting ready"
notification and its audio afterwards.

## The Virtual Microphone

The generated line is played into a virtual microphone called `voice-me`
(Story 2.9), not through your speakers. Select it as the input device in
Discord, a browser mic test, or Sound settings, and what you type is what
that application hears.

On **Linux** the device is two PipeWire/PulseAudio nodes — no kernel driver,
no `sudo`: a plain sink called `voice-me-sink` that voice-me plays into, and
`voice-me`, an ordinary-looking microphone remapped from that sink's monitor,
which is the one you select in other applications. (Two nodes because a
single node published as a virtual source cannot be addressed by name from
another process — playback silently lands on your default output instead.)
The app installs both for you the first time it starts without them, in the
background; they persist across reboots, because installing also writes one
drop-in at `~/.config/pipewire/pipewire-pulse.conf.d/voice-me.conf`. Check it
with:

```bash
pactl list short sources | grep voice-me   # expect exactly one entry
```

Removing it is manual for now (a dependency row in Settings comes in Epic 3):

```bash
cargo run -p voice-me-audio-linux --bin mic-spike -- --uninstall
```

If the device is missing, duplicated, or there is no audio server, nothing is
played — deliberately, since PulseAudio answers an unresolvable device by
falling back to your speakers — and a desktop notification says which of
those it was.

On **Windows** the Virtual Microphone is VB-Audio's **VB-CABLE**
(donationware, www.vb-cable.com). voice-me plays each line to its
"CABLE Input" playback device; in your voice-chat app, select
**CABLE Output** as the microphone. If it is missing, open
**Settings → Dependencies** and press **Install** on the Virtual Microphone
row: voice-me downloads VB-Audio's official driver pack (pinned and
checksum-verified), then runs VB's own setup, which Windows asks you to
allow. If Windows wants a restart afterwards, restart and press
**Check again**. voice-me never installs the driver silently or at startup,
and never plays to your speakers instead: with no cable, or more than one,
a Speak Action notifies rather than playing.

## Speech generation (spec-2-5 spike)

Speech is generated **in this process**, by ONNX Runtime, from the
Chatterbox-Multilingual V3 ONNX export. There is no sidecar process and no
Python anywhere in the pipeline.

The engine itself never downloads anything (AD-8) — it reads its files from
a cache directory owned by `voice-me-deps`. You never fetch them by hand:
open **Settings → Dependencies** and press **Install** on each missing row.

- **Speech model files (Q4)** — the nine files the CPU backend needs, about
  1.56 GB, with progress shown on the row. An interrupted download resumes
  from the bytes already on disk the next time Install is pressed, and every
  file is checked against a pinned SHA-256 before it is used.
- **ONNX Runtime** (Linux x64, Windows x64) — the core library from
  Microsoft's `onnxruntime-linux-x64-1.28.2.tgz` release, extracted to
  `<cache>/runtime/libonnxruntime.so`, or on Windows from
  `onnxruntime-win-x64-1.28.2.zip`, extracted to
  `<cache>/runtime/onnxruntime.dll`. No execution-provider libraries are
  fetched for the CPU backend. On other systems, or when `ORT_DYLIB_PATH`
  points somewhere that does not exist, the row shows short manual steps
  instead of an Install button.

When a row finishes, the check runs again by itself; the row turns "ready"
and the Prompt Overlay accepts input without a restart.

### Piper and eSpeak NG

Piper (natural, instant stock voices) is the first-run backend on Linux and
Windows, with the Turkish voice `tr_TR-fahrettin-medium`. It reads text
through the `espeak-ng` program, run as a separate process; nothing of
eSpeak NG is linked.

- **Linux** — install `espeak-ng` with your package manager; the eSpeak NG
  row shows the command.
- **Windows** — voice-me looks for `espeak-ng.exe` in its cache, then under
  `C:\Program Files\eSpeak NG\`, then on `PATH`. When none is found, the
  eSpeak NG row's **Install** downloads the official eSpeak NG 1.52.0
  `espeak-ng.msi` (12.8 MB, pinned SHA-256), unpacks it into
  `<cache>\espeak-ng\` by reading the package itself (no `msiexec`, no
  admin prompt, no registry or `PATH` change), and deletes the `.msi`. Delete that directory to remove it.

For reference, the weights come from
`onnx-community/chatterbox-multilingual-ONNX` (MIT), pinned to revision
`452d3f434aa592098f1eedac9099f33642ab2da5` — the tokenizer and the graphs
have drifted against each other before, so the revision is not optional.
The sources, sizes and checksums live in
`crates/voice-me-deps/src/sources.rs`. Each `.onnx_data` holds its graph's
weights and records its own location as a **bare relative filename**, so it
sits next to its `.onnx`. The cache directory defaults to
`$XDG_CACHE_HOME/voice-me`; `VOICE_ME_MODEL_CACHE` overrides it.

### Run the spike

```bash
cargo run --release -p voice-me-tts --example tts-spike -- tr "Merhaba, bugün nasılsın?"
cargo run --release -p voice-me-tts --example tts-spike -- en "Hey, I'll be right back."
```

It decodes the Reference Voice Sample at
`~/.local/share/voice-me/reference_voice_sample.wav` (any container; `--reference`
points elsewhere), generates, prints per-stage timings, and writes a wav to
`$TMPDIR`. `--variant fp32` loads the unquantized baseline for an A/B listen;
that one is an extra 2.08 GB.

If a file is missing, the run prints the **complete** list of files it needs
with each one marked present or missing, rather than failing on the first.

### What it measures (i7-7700HQ, 4C/8T, 15 GB, 2026-09-21)

`"Merhaba, bugün nasılsın?"`, the 31 s Reference Voice Sample, one process
per row:

| Variant / provider | Session build | `language_model` | `conditional_decoder` | Utterance total |
| --- | --- | --- | --- | --- |
| **Q4 / CPU** | 86–91 s | 28.0 ms/token | 17.6 s | **20.5 s** for 2.0 s of audio |
| FP32 / CPU | 94 s | 107.9 ms/token | 19.8 s | 28.1 s |
| FP16 / CPU | 88 s | 361.0 ms/token | 18.5 s | 42.1 s |
| FP16 / WebGPU (Intel HD 630) | 110 s | 344.4 ms/token | 49.1 s | 73.8 s |

Q4 on CPU is the fastest path by a wide margin, and session construction
dominates everything — which is exactly what AD-10's "build once and hold"
rule is for. `conditional_decoder` is the real per-utterance cost (86 % of
it), not the token loop.

### GPU

GPU acceleration is still planned — it just could not be exercised on the
machine this was developed on, so it is deferred rather than dropped. What
follows describes *this hardware*, not voice-me's direction:

- **CUDA is unreachable here**, so it was skipped rather than ruled out. The Quadro M1200 is GM107 = sm_50. ONNX Runtime
  1.28.2's prebuilt CUDA floor is sm_60 with no PTX target below
  `120-virtual` to JIT from, so a correct install would still fail at run
  with "no kernel image is available". Separately, Arch ships only
  `nvidia-open` (Turing+). On an sm_60-or-newer NVIDIA GPU the CUDA
  execution provider is expected to work as AD-9 describes; nothing here
  tests that either way.
- **WebGPU over Vulkan runs, and is far slower than the CPU.** Measured on
  the same line, same binary: 24.9 ms/token on CPU against 311 ms/token on
  the Intel HD 630 and 1174 ms/token on the Quadro via NVK. ORT's own
  node-placement log confirms this is a real GPU run — 183 of the 189
  `language_model_q4` nodes land on `WebGpuExecutionProvider`, including all
  30 `GroupQueryAttention` and all 151 `MatMulNBits` — so the contrib ops are
  not the problem; the hardware is. The Quadro additionally loses its Vulkan
  device mid-run (`VK_ERROR_DEVICE_LOST` out of NVK) on anything longer than
  a word.
- Reaching WebGPU at all needs `--no-default-features --features
  webgpu-probe`, which swaps `load-dynamic` for ort's `download-binaries`
  (pyke's Dawn-bundling distribution): a build-time download and a static
  link, i.e. the opposite of what AD-8 mandates, never built in CI. Its
  `libwebgpu_dawn.so` is not installed anywhere, so the built example needs
  `LD_LIBRARY_PATH` pointing into `~/.cache/ort.pyke.io/dfbin/...`.
- `ort::ep::WebGPU::with_device_id` had **no effect** here — Dawn picked the
  discrete adapter regardless. The only thing that selected a device was
  restricting the Vulkan loader: `VK_DRIVER_FILES=/usr/share/vulkan/icd.d/intel_icd.json`.

## Linux: prompt overlay and always-on-top

Whether the overlay is drawn **above a fullscreen window from another
application** depends on the session, the same way the global hotkey's
backend does:

| Session | Overlay window | Above a fullscreen window? |
|---------|----------------|----------------------------|
| X11 | always-on-top, taskbar-less popup | expected — **not yet verified on real hardware** |
| Wayland | ordinary focused window | no — and the compositor, not voice-me, decides where it appears |

This is a platform limitation, not a bug. The GPUI version this app is built
on has no always-on-top window type that works under Wayland, and the one
mechanism that could provide it — `zwlr_layer_shell_v1` — is not implemented
by GNOME/Mutter, so it is deliberately not used. Under Wayland the overlay
still opens, focuses, and accepts typing normally; it may simply sit behind a
fullscreen game instead of over it.

## Linux: global hotkey setup

The global hotkey uses one of two backends, chosen at runtime from the
session:

| Session | Backend | Setup needed |
|---------|---------|--------------|
| X11 | `global-hotkey` (`XGrabKey`) | none |
| Wayland | raw `evdev` read of `/dev/input/event*` | `input`-group membership |

`global-hotkey` is X11-only, and GNOME does not implement the desktop
portal's GlobalShortcuts interface, so on Wayland the hotkey is matched by
reading the keyboard devices directly. That needs read access to the input
devices, which is granted by joining the `input` group — a **one-time**
setup step:

```bash
sudo usermod -aG input $USER
```

**Understand what this grants before running it.** `input`-group membership
lets *any* process running as your user read every keystroke you type,
system-wide — including passwords typed into other applications, on any
desktop or login screen. It is not scoped to voice-me. The Wayland backend
does exactly that: it reads the raw keyboard stream and matches your
combination against it. On X11 nothing of the sort is needed, and voice-me
does not ask for it.

Log out and back in (a new shell is not enough — group membership is
established at login). Until then the Hotkey tab reports the missing access
instead of binding anything; the rest of the app runs normally.

Two properties of the Wayland backend are consequences of the platform, not
bugs:

- The combination **also reaches the focused application** — it is a passive
  read, not an exclusive grab. (The alternative, `EVIOCGRAB`, would steal all
  keyboard input from every other application.)
- **Conflicts cannot be detected.** Nothing is registered with any server, so
  there is no way to learn that another application already uses the
  combination. Inline conflict reporting exists only on X11.

To verify the backend on its own, without the app:

```bash
cargo run -p voice-me-hotkey-linux --example hotkey-spike            # Ctrl+Alt+KeyV
cargo run -p voice-me-hotkey-linux --example hotkey-spike -- Ctrl+Alt+KeyB
```
