"""Transcribe clips with Whisper and score them against their text (character error rate).

    python3 whisper_cer.py <metadata: id|text or name.wav|text> <wav dir> <out.jsonl> [model] [workers]

Resumable: ids already in out.jsonl are skipped. Numbers are spelled out on both sides
with the phonemizer's own spell_numbers, so "3" and "üç" match.
"""
import concurrent.futures, json, os, re, sys, threading, time
sys.path.insert(0, "/content")
from dizge_phonemizer import spell_numbers
from faster_whisper import WhisperModel
from math import gcd
import numpy as np, soundfile as sf
from scipy.signal import resample_poly

def load16k(path):
    audio, rate = sf.read(path, dtype="float32", always_2d=True)
    audio = audio.mean(axis=1)
    if rate != 16000:
        g = gcd(16000, rate)
        audio = resample_poly(audio, 16000 // g, rate // g).astype(np.float32)
    return audio

meta, wav_dir, out = sys.argv[1], sys.argv[2], sys.argv[3]
model_name = sys.argv[4] if len(sys.argv) > 4 else "large-v3"
workers = int(sys.argv[5]) if len(sys.argv) > 5 else 4

def norm(t):
    t = t.replace("I", "ı").replace("İ", "i").lower()
    t = spell_numbers(t)
    t = re.sub(r"[^\wçğıöşüâîû ]", " ", t).replace("_", " ")
    t = t.replace("â", "a").replace("î", "i").replace("û", "u")
    return re.sub(r"\s+", "", t)  # spacing differences ("dört yüz" / "dörtyüz") do not count

def lev(a, b):
    prev = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        cur = [i]
        for j, cb in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (ca != cb)))
        prev = cur
    return prev[-1]

rows = []
for line in open(meta, encoding="utf-8-sig"):
    if "|" in line:
        name, text = line.rstrip("\n").split("|", 1)
        rows.append((os.path.splitext(name.strip())[0], text.strip()))
done = set()
if os.path.exists(out):
    for line in open(out, encoding="utf-8"):
        try: done.add(json.loads(line)["id"])
        except ValueError: pass
todo = [r for r in rows if r[0] not in done]
print(f"{len(rows)} satır, {len(done)} önceden bitmiş, {len(todo)} kaldı", flush=True)
model = WhisperModel(model_name, device="cuda", compute_type="float16", num_workers=workers)
lock, f, t0, n = threading.Lock(), open(out, "a", encoding="utf-8"), time.time(), 0

def path_of(i):
    for ext in (".wav", ".flac"):
        p = os.path.join(wav_dir, i + ext)
        if os.path.exists(p): return p
    return None

def one(row):
    global n
    i, text = row
    p = path_of(i)
    if p is None: return
    segs, _ = model.transcribe(load16k(p), language="tr", beam_size=1, without_timestamps=True,
                               condition_on_previous_text=False, vad_filter=False)
    hyp = " ".join(s.text.strip() for s in segs)
    r, h = norm(text), norm(hyp)
    cer = lev(r, h) / max(1, len(r))
    with lock:
        f.write(json.dumps({"id": i, "cer": round(cer, 4), "text": text, "hyp": hyp}, ensure_ascii=False) + "\n")
        n += 1
        if n % 200 == 0:
            f.flush()
            print(f"{n}/{len(todo)}  {(time.time()-t0)/n:.3f} sn/satır", flush=True)

with concurrent.futures.ThreadPoolExecutor(workers) as pool:
    list(pool.map(one, todo))
f.close()
print("BITTI", flush=True)
