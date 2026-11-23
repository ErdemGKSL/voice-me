# Supervising the Dizge Piper training on Colab

Task for an agent session that supervises one long training run of the
Dizge-phoneme Piper voice "higgs-erkek-tr-dizge" (from scratch, on the 20k
synthetic Higgs dataset) on a Colab runtime it reaches through the
`colab-claude` MCP tools. The user writes Turkish; answer in Turkish.

## What is running

- Runtime: given in the hand-off message (`runtimes/...`), an L4 GPU.
- `/content/piper-run/dizge_train_run.py`-style run: `jupyter nbconvert`
  executes `/content/piper-run/run.ipynb` (the repo's
  `notebooks/Piper_Train_Dizge.ipynb` with form fields filled by
  `notebooks/colab/dizge_train_run.py`). Training output is appended to
  `/content/piper-run/train.log`; `/content/piper-run/status` says
  `running` or `exited <code>`.
- Checkpoints: `/content/piper-out/higgs-erkek-tr-dizge/lightning_logs/version_*/checkpoints/`
  (`epoch=*.ckpt` every 2 epochs and `last.ckpt`). Config:
  `/content/piper-out/higgs-erkek-tr-dizge/config.json`.
- Dataset zip on the runtime: `/content/higgs-erkek-20k.zip`.

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
4. Every new `epoch=*.ckpt` (or at least once an hour): copy the newest
   checkpoint to your own disk (keep only the latest two locally):
   `get_runtime` for the URL + token, then
   `curl -f -H "X-Colab-Runtime-Proxy-Token: $T" -o ckpt.part "$URL/files/content/piper-out/higgs-erkek-tr-dizge/lightning_logs/version_N/checkpoints/<file>" && mv ckpt.part <file>`.
   Tokens expire after about an hour; call `get_runtime` again for a new one.
5. Schedule the next check with `send_later` (~10 minutes). Never stop
   re-arming until the user says the training is done.

## Restart on the same runtime

```
cd /content/piper-run && setsid nohup python3 /content/dizge_train_run.py \
  /content/Piper_Train_Dizge.ipynb /content/higgs-erkek-20k.zip 32 2 > runner.log 2>&1 &
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
   - the dataset zip → `content/higgs-erkek-20k.zip` (you keep a copy: see
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
  (`/files/content/higgs-erkek-20k.zip`, about 2.3 GB; use `curl -f` with
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
