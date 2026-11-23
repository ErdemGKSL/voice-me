#!/bin/bash
# Diversified data + Whisper filter + training, one stage after another.
# Re-running is safe: finished stages are skipped (markers in /content/pipe/), the
# Higgs worker and Whisper scoring resume where they stopped.
#   setsid nohup bash /content/pipeline.sh > /content/pipe/pipeline.log 2>&1 &
set -u
P=/content/pipe; mkdir -p $P
say() { echo "$(date -u +%H:%M:%S) $*"; echo "$*" > $P/stage; }
. /content/cer_env.sh
cd /content

if [ ! -f $P/gen.done ]; then
  say "1/4 Higgs: 5000 yeni cümle üretiliyor"
  [ -f /content/job2/manifest.json ] || python3 /content/build_job2.py || exit 1
  rm -f /content/higgs/STOP
  python3 /content/higgs/higgs_worker.py /content/job2 32 >> /content/higgs/worker2.out 2>&1
  python3 - <<'PY' || exit 1
import json, sys; sys.path.insert(0, "/content/higgs"); import higgs_common as hc
m = hc.load_manifest("/content/job2"); c = hc.count(m); print("job2:", c)
hc.stop_server()
sys.exit(0 if c["pending"] == 0 else 1)
PY
  touch $P/gen.done
fi

if [ ! -f $P/cer20k.done ]; then
  say "2/4 Whisper: 20k eski klip taranıyor"
  python3 whisper_cer.py /content/dataset/metadata.csv /content/dataset/wavs cer_20k.jsonl large-v3-turbo 8 >> $P/cer20k.log 2>&1 || exit 1
  touch $P/cer20k.done
fi

if [ ! -f $P/cer5k.done ]; then
  say "3/4 Whisper: 5k yeni klip taranıyor"
  python3 whisper_cer.py /content/job2/metadata.csv /content/job2/wavs cer_5k.jsonl large-v3-turbo 8 >> $P/cer5k.log 2>&1 || exit 1
  touch $P/cer5k.done
fi

if [ ! -f $P/zip.done ]; then
  say "4/4 25k zip hazırlanıyor (CER > 0.15 çıkarılıyor)"
  python3 assemble.py 0.15 >> $P/zip.log 2>&1 || exit 1
  touch $P/zip.done
fi

say "EĞİTİM: başlatılıyor"
mkdir -p /content/piper-run
cd /content/piper-run && exec python3 /content/dizge_train_run.py /content/Piper_Train_Dizge.ipynb /content/higgs-erkek-25k.zip 32 50 > /content/piper-run/runner.log 2>&1
