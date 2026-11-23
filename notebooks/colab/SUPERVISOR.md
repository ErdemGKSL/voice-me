# Supervising the Dizge Piper training on Colab

Task for an agent session that supervises one long training run of the
Dizge-phoneme Piper voice "higgs-erkek-tr-dizge" (from scratch, on the 25k
synthetic Higgs dataset: 20k lines + 5k diverse lines, Whisper-filtered) on a Colab runtime it reaches through the
`colab-claude` MCP tools. The user writes Turkish; answer in Turkish.

## Pipeline before training (`/content/pipeline.sh`)

Training now starts at the end of a chain on the same runtime. Start it once:
```
mkdir -p /content/pipe && setsid nohup bash /content/pipeline.sh > /content/pipe/pipeline.log 2>&1 &
```
`cat /content/pipe/stage` and `tail /content/pipe/pipeline.log` say where it is:
1. Higgs generates 5000 new diverse lines into `/content/job2` (~25–40 min);
   progress: `/content/higgs/status.json` (counts), `/content/higgs/worker.log`.
2. Whisper (`large-v3-turbo`) scores the old 20k clips → `/content/cer_20k.jsonl`;
   progress: `tail /content/pipe/cer20k.log` (prints sn/satır every 200 lines).
3. Whisper scores the new 5k → `/content/cer_5k.jsonl` (`/content/pipe/cer5k.log`).
4. `assemble.py` builds `/content/higgs-erkek-25k.zip` without clips over CER 0.15
   (`/content/pipe/zip.log` prints `ZIP_READY <kept> satır, <dropped> çıkarıldı`).
5. Training starts (`dizge_train_run.py ... higgs-erkek-25k.zip 32 50`).

If the chain process is gone and `stage` is not `EĞİTİM`, read the end of the
logs, fix a clear cause, then start it again with the same command: finished
stages are skipped and steps 1–3 resume where they stopped. Report the
`ZIP_READY` numbers to the user. Before training starts, the GPU in use is
Higgs (stage 1) or Whisper (stages 2–3); that is expected. The chain's
scripts are in the repo under `notebooks/colab/diverse/`. If the runtime is
lost before `ZIP_READY`, tell the user and stop: do not rebuild the chain
on a new runtime by yourself.

## What is running

- Runtime: given in the hand-off message (`runtimes/...`), an L4 GPU.
- `/content/piper-run/dizge_train_run.py`-style run: `jupyter nbconvert`
  executes `/content/piper-run/run.ipynb` (the repo's
  `notebooks/Piper_Train_Dizge.ipynb` with form fields filled by
  `notebooks/colab/dizge_train_run.py`). Training output is appended to
  `/content/piper-run/train.log`; `/content/piper-run/status` says
  `running` or `exited <code>`.
- Checkpoints: `/content/piper-out/higgs-erkek-tr-dizge/lightning_logs/version_*/checkpoints/`
  (`epoch=*.ckpt` every 50 epochs and `last.ckpt` every 2). Config:
  `/content/piper-out/higgs-erkek-tr-dizge/config.json`.
- Dataset zip on the runtime: `/content/higgs-erkek-25k.zip` (built by the pipeline).

## Every check (about every 10 minutes)

1. `list_runtimes`. If the runtime is gone, go to **Recovery**.
2. With `execute_code` (always pass a new UUID `executionId`; the client
   cuts calls off at 60 s, so keep each call short and never `sleep` long;
   re-send the identical call to reattach after a timeout):
   - `cat /content/piper-run/status`, `tail -c 1500 /content/piper-run/train.log`,
     `ls -la` the checkpoints directory, `nvidia-smi --query-gpu=memory.used --format=csv`.
   - This activity also keeps the runtime from being reclaimed as idle.
3. If `status` is `exited` with training not finished: read the end of
   `train.log` and `/content/piper-run/run.out.ipynb` errors, fix the cause
   if it is clear (out of memory → rerun with a smaller batch, e.g. 24 or
   16), then restart (see **Restart on the same runtime**).
4. At least once an hour (and every new `epoch=*.ckpt`): copy `last.ckpt` to your own disk (keep only the latest two locally):
   `get_runtime` for the URL + token, then
   `curl -f -H "X-Colab-Runtime-Proxy-Token: $T" -o ckpt.part "$URL/files/content/piper-out/higgs-erkek-tr-dizge/lightning_logs/version_N/checkpoints/<file>" && mv ckpt.part <file>`.
   Tokens expire after about an hour; call `get_runtime` again for a new one.
5. Schedule the next check with `send_later` (~10 minutes). Never stop
   re-arming until the user says the training is done.

## Restart on the same runtime

```
cd /content/piper-run && setsid nohup python3 /content/dizge_train_run.py \
  /content/Piper_Train_Dizge.ipynb /content/higgs-erkek-25k.zip 32 50 > runner.log 2>&1 &
```
The notebook's training cell finds `last.ckpt` under the output directory
and resumes from it. Re-running reinstalls nothing that is already there
and rebuilds the audio cache (a few minutes for 20k lines).

## Recovery (runtime gone)

1. `create_runtime` L4 (`VARIANT_GPU`, `SHAPE_STANDARD`, new UUID
   `requestId`). If refused (FAILED_PRECONDITION), try A100, then retry L4
   every few minutes; tell the user once.
2. Upload with `notebooks/colab/upload.py <url> <token> <local> <remote>`
   from the repo checkout:
   - the dataset zip → `content/higgs-erkek-25k.zip` (you keep a copy: see
     **Setup**),
   - `notebooks/Piper_Train_Dizge.ipynb` → `content/Piper_Train_Dizge.ipynb`,
   - `notebooks/colab/dizge_train_run.py` → `content/dizge_train_run.py`,
   - `notebooks/colab/dizge_export.sh` → `content/dizge_export.sh`,
   - your latest checkpoint → create the directories
     `content/piper-out`, `.../higgs-erkek-tr-dizge`, `.../lightning_logs`,
     `.../version_0`, `.../checkpoints` (one `PUT {"type":"directory"}` per
     level) and upload it as `.../version_0/checkpoints/last.ckpt`,
   - the `config.json` you pulled → `content/piper-out/higgs-erkek-tr-dizge/config.json`.
3. Start it as in **Restart on the same runtime**.

## Setup (once, at the start)

- Check the `colab-claude` tools respond (`list_runtimes`). If they are
  missing, tell the user the Colab connector is not available in this
  session and stop.
- Pull the dataset zip from the runtime to your disk once, for recovery
  (`/files/content/higgs-erkek-25k.zip` once `ZIP_READY`, about 2.8 GB; use `curl -f` with
  `Range` resumes if it breaks). Also pull `config.json` once it exists.

## Reporting

- Tell the user (in Turkish, short) when: training is confirmed running
  with its seconds per epoch; every 50 epochs; any restart or recovery and
  why; anything you cannot fix.
- Every 100 epochs: run `bash /content/dizge_export.sh` on the runtime,
  pull `/content/piper-run/higgs-erkek-tr-dizge-voice.zip` and send it to
  the user with `SendUserFile` (caption: epoch number), so they can listen
  to progress in voice-me.

## Never

- Never delete the runtime or a checkpoint unless the user asks.
- Never put the Discord webhook, tokens or checkpoints in the repository.
- Never commit to the repository; this task only supervises.
