#@title 1. İş: yeni mi, devam mı? (Drive bağlanır)
MODE = "new" #@param ["new", "continue"]
#@markdown Drive'da işlerin tutulacağı klasör (`MyDrive` altından):
DRIVE_PATH = "voice-me/tts-jobs" #@param {type:"string"}
#@markdown Bu işin adı (klasör adı olur). Devam ederken aynısını yaz:
JOB_NAME = "erdem-okuma-1" #@param {type:"string"}
import json, os, re, sys
from google.colab import drive

drive.mount("/content/drive")
assert re.fullmatch(r"[A-Za-z0-9._-]+", JOB_NAME), "İş adında yalnızca harf, rakam, . _ - kullan"
JOB_DIR = os.path.join("/content/drive/MyDrive", DRIVE_PATH.strip("/"), JOB_NAME)
MANIFEST = os.path.join(JOB_DIR, "manifest.json")
if MODE == "new":
    if os.path.exists(MANIFEST):
        raise SystemExit(f"{JOB_DIR} içinde zaten bir iş var. Devam etmek için MODE = 'continue' seç ya da başka bir iş adı yaz.")
    os.makedirs(JOB_DIR, exist_ok=True)
    print("yeni iş:", JOB_DIR)
else:
    if not os.path.exists(MANIFEST):
        raise SystemExit(f"{JOB_DIR} içinde manifest.json yok. Klasör ve iş adını kontrol et ya da MODE = 'new' seç.")
    summary = json.load(open(MANIFEST, encoding="utf-8"))
    print("devam edilecek iş:", JOB_DIR)
    print("durum:", summary["state"], "| sayılar:", summary["counts"], "| son güncelleme:", summary["updated_at"])
    print("ayarlar:", json.dumps(summary["config"], ensure_ascii=False))
os.makedirs("/content/higgs", exist_ok=True)
