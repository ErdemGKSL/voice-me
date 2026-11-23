#@title 5. Veri: voice-me-recorder ya da Higgs veri seti zip'i
#@markdown Boş bırakırsan yükleme penceresi açılır. Drive'daki zip için tam yolu yaz; birden çok zip için virgülle ayır.
ZIP_PATH = "" #@param {type:"string"}
#@markdown Birden çok zip (ör. iki PC'den gelen yedekler) yüklenirse hepsi birleştirilir.
#@markdown Higgs veri setinin zip'i (`metadata.csv` + `wavs/*.flac`, 24 kHz) de olur: sesler eğitim hızına (22050 Hz) çevrilir.
#@markdown Her klibin tepe seviyesi buna getirilir (voice-me-recorder da 0.89 kullanır); Higgs sesleri ~10 dB kısık geldiği için ana model ve ince ayar verisi aynı seviyede olur. 0 = dokunma.
NORMALIZE_PEAK = 0.89 #@param {type:"number"}
import csv, json, shutil, subprocess, wave, zipfile
import numpy as np
from collections import Counter
from concurrent.futures import ThreadPoolExecutor

zips = [p.strip() for p in ZIP_PATH.split(",") if p.strip()]
if not zips:
    from google.colab import files
    zips = [os.path.join("/content", z) for z in files.upload().keys()]

shutil.rmtree("/content/dataset", ignore_errors=True)
os.makedirs("/content/dataset/wavs")
os.makedirs("/content/dataset/raw")
rows, manifest, sources = {}, {}, {}
for z in zips:
    with zipfile.ZipFile(z) as archive:
        names = set(archive.namelist())
        assert "metadata.csv" in names and any(n.startswith("wavs/") for n in names), f"{z}: veri seti zip'i değil"
        if "manifest.json" in names:
            manifest = json.loads(archive.read("manifest.json"))
        for line in archive.read("metadata.csv").decode("utf-8-sig").splitlines():
            if "|" in line:
                name, text = line.split("|", 1)
                # Recorder rows name the file ("x.wav"); Higgs rows give only the id.
                key = os.path.splitext(name.strip())[0]
                rows.setdefault(key, text.strip())
        for n in names:
            stem, ext = os.path.splitext(os.path.basename(n))
            if n.startswith("wavs/") and ext in (".wav", ".flac") and ".." not in n and stem not in sources:
                target = os.path.join("/content/dataset/raw", os.path.basename(n))
                with archive.open(n) as src, open(target, "wb") as dst:
                    shutil.copyfileobj(src, dst)
                sources[stem] = target

model_name = "erdem-tr-dizge"
if manifest.get("speaker"):
    model_name = "".join(c if c.isalnum() else "-" for c in manifest["speaker"].lower()).strip("-") + "-tr-dizge"
# Piper's medium/high models train at 22050 Hz (x-low at 16000); anything else is converted.
sample_rate = str(manifest.get("sample_rate", 22050))
if sample_rate not in ("16000", "22050"):
    sample_rate = "22050"

def normalize_peak(path):
    """Scale the clip so its loudest sample sits at NORMALIZE_PEAK."""
    if NORMALIZE_PEAK <= 0:
        return
    with wave.open(path) as w:
        params, audio = w.getparams(), np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16)
    peak = int(np.abs(audio.astype(np.int32)).max()) if audio.size else 0
    if peak == 0:
        return
    scaled = np.clip(np.round(audio.astype(np.float64) * (NORMALIZE_PEAK * 32767 / peak)), -32768, 32767)
    with wave.open(path, "wb") as w:
        w.setparams(params)
        w.writeframes(scaled.astype(np.int16).tobytes())

def to_training_wav(stem):
    """Copy as is if already mono/16-bit at the training rate, else convert with ffmpeg;
    then bring the peak level to NORMALIZE_PEAK."""
    src, dst = sources[stem], f"/content/dataset/wavs/{stem}.wav"
    same = False
    if src.endswith(".wav"):
        with wave.open(src) as w:
            same = w.getnchannels() == 1 and w.getsampwidth() == 2 and w.getframerate() == int(sample_rate)
    if same:
        os.replace(src, dst)
    else:
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", src, "-ac", "1", "-ar", sample_rate,
                        "-sample_fmt", "s16", dst], check=True)
        os.remove(src)
    normalize_peak(dst)

keep = [n for n in rows if n in sources]
with ThreadPoolExecutor(os.cpu_count() or 4) as pool:
    list(pool.map(to_training_wav, keep))
shutil.rmtree("/content/dataset/raw", ignore_errors=True)
rows = {f"{n}.wav": rows[n] for n in keep}
with open("/content/dataset/metadata.csv", "w", encoding="utf-8") as f:
    f.writelines(f"{n}|{t}\n" for n, t in sorted(rows.items()))
total = 0.0
for n in rows:
    with wave.open(f"/content/dataset/wavs/{n}") as w:
        assert w.getnchannels() == 1 and w.getsampwidth() == 2 and w.getframerate() == int(sample_rate), f"{n}: mono/16-bit/{sample_rate} Hz değil"
        total += w.getnframes() / w.getframerate()
print(f"{len(rows)} cümle, {total/60:.1f} dk ses, model adı: {model_name}")
if total < 30 * 60:
    print("UYARI: 30 dakikadan az ses var; sıfırdan eğitim için 8 saat, finetune için en az birkaç yüz cümle önerilir.")

# Fonem denetimi: eğitime geçmeden önce haritada olmayan sembol var mı?
from piper.phoneme_ids import DEFAULT_PHONEME_ID_MAP
seen, unknown = Counter(), Counter()
for i, text in enumerate(rows.values()):
    for sentence in dz.phonemize("tr", text):
        for ch in sentence:
            seen[ch] += 1
            if ch not in DEFAULT_PHONEME_ID_MAP:
                unknown[ch] += 1
    if i < 3:
        print(text, "->", ["".join(s) for s in dz.phonemize("tr", text)])
print("farklı sembol:", len(seen), "| haritada olmayan:", dict(unknown))
assert not unknown, "phoneme_id_map'te olmayan semboller var, eğitime geçme"

output_dir = f"{OUTPUT_ROOT}/{model_name}"
os.makedirs(output_dir, exist_ok=True)
audio_cache_dir = "/content/audio_cache_dizge"
shutil.rmtree(audio_cache_dir, ignore_errors=True)  # eski eSpeak fonemleri karışmasın
os.makedirs(audio_cache_dir)
