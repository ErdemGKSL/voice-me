"""Build /content/higgs-erkek-25k.zip: the 20k zip plus job2, minus clips whose Whisper CER is over the limit.

    python3 assemble.py [cer_limit]
"""
import io, json, os, subprocess, sys, zipfile
LIMIT = float(sys.argv[1]) if len(sys.argv) > 1 else 0.15
OUT = "/content/higgs-erkek-25k.zip"
cer = {}
for f in ("/content/cer_20k.jsonl", "/content/cer_5k.jsonl"):
    for line in open(f, encoding="utf-8"):
        r = json.loads(line)
        cer[r["id"]] = r["cer"]
dropped = []
with zipfile.ZipFile("/content/higgs-erkek-20k.zip") as old, zipfile.ZipFile(OUT + ".tmp", "w", zipfile.ZIP_STORED) as z:
    rows = []
    for line in old.read("metadata.csv").decode("utf-8-sig").splitlines():
        if "|" not in line: continue
        i, t = line.split("|", 1)
        if i not in cer or cer[i] > LIMIT:
            dropped.append((i, cer.get(i))); continue
        z.writestr(f"wavs/{i}.flac", old.read(f"wavs/{i}.flac")); rows.append(f"{i}|{t}")
    for line in open("/content/job2/metadata.csv", encoding="utf-8-sig"):
        if "|" not in line: continue
        name, t = line.rstrip("\n").split("|", 1)
        i = os.path.splitext(name)[0]
        if i not in cer or cer[i] > LIMIT:
            dropped.append((i, cer.get(i))); continue
        flac = subprocess.run(["ffmpeg", "-loglevel", "error", "-i", f"/content/job2/wavs/{i}.wav", "-c:a", "flac",
                               "-f", "flac", "-"], check=True, capture_output=True).stdout
        z.writestr(f"wavs/{i}.flac", flac); rows.append(f"{i}|{t}")
    z.writestr("metadata.csv", "\n".join(rows) + "\n")
    z.writestr("manifest.json", json.dumps({"speaker": "higgs-erkek", "sample_rate": 24000, "lines": len(rows),
                                            "cer_limit": LIMIT, "dropped": dropped}, ensure_ascii=False, indent=1))
os.replace(OUT + ".tmp", OUT)
print(f"ZIP_READY {len(rows)} satır, {len(dropped)} çıkarıldı", flush=True)
