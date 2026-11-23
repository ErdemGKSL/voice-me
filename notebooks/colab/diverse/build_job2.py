"""Build /content/job2 from /content/divers.txt (one line per sentence, numbers already spelled out)."""
import json, os, sys, unicodedata, shutil
sys.path.insert(0, "/content/higgs")
import higgs_common as hc
import soundfile as sf

J = "/content/job2"
FIRST = 20001
os.makedirs(J + "/wavs", exist_ok=True); os.makedirs(J + "/logs", exist_ok=True)
old = hc.read_json("/content/job/manifest.json")
ref = "/content/higgs/reference.wav"
assert hc.sha256_file(ref) == old["reference"]["sha256"], "reference.wav ilk işten farklı"
hc.copy_atomic(ref, J + "/reference.wav")
open(J + "/reference.txt", "w").write(old["reference"]["text"] + "\n")
text = unicodedata.normalize("NFC", open("/content/divers.txt", encoding="utf-8").read())
lines = [l.strip() for l in text.splitlines() if l.strip()]
open(J + "/reading_list.txt", "w", encoding="utf-8").write("\n".join(lines) + "\n")
config = old["config"]
hc.write_json_atomic(J + "/config.json", config)
items = [{"id": f"{FIRST + i:06d}", "line_no": i + 1, "text": t, "status": "pending", "attempts": 0, "seed": None,
          "file": f"wavs/{FIRST + i:06d}.wav", "bytes": None, "duration_s": None, "gen_s": None, "error": None,
          "finished_at": None} for i, t in enumerate(lines)]
manifest = {"schema_version": 1, "job_name": "erdem-cesitli-5k", "created_at": hc.now(), "updated_at": hc.now(),
            "state": "pending", "config": config, "config_sha256": old["config_sha256"],
            "reference": dict(old["reference"], duration_s=round(sf.info(ref).duration, 2)),
            "reading_list": {"sha256": hc.sha256_file(J + "/reading_list.txt"), "n_items": len(lines)},
            "counts": {}, "items": items}
hc.save_manifest(J, manifest)
print("JOB_READY", len(items), items[0]["id"], items[-1]["id"], flush=True)
