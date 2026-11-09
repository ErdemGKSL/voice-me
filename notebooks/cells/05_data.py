#@title 5. Veri: voice-me-recorder zip'i
#@markdown Boş bırakırsan yükleme penceresi açılır. Drive'daki zip için tam yolu yaz.
ZIP_PATH = "" #@param {type:"string"}
#@markdown Birden çok zip (ör. iki PC'den gelen yedekler) yüklenirse hepsi birleştirilir.
import csv, json, shutil, wave, zipfile
from collections import Counter

zips = [ZIP_PATH] if ZIP_PATH.strip() else []
if not zips:
    from google.colab import files
    zips = [os.path.join("/content", z) for z in files.upload().keys()]

shutil.rmtree("/content/dataset", ignore_errors=True)
os.makedirs("/content/dataset/wavs")
rows, manifest = {}, {}
for z in zips:
    with zipfile.ZipFile(z) as archive:
        names = set(archive.namelist())
        assert "metadata.csv" in names and any(n.startswith("wavs/") for n in names), f"{z}: voice-me-recorder zip'i değil"
        if "manifest.json" in names:
            manifest = json.loads(archive.read("manifest.json"))
        for line in archive.read("metadata.csv").decode("utf-8-sig").splitlines():
            if "|" in line:
                name, text = line.split("|", 1)
                rows.setdefault(name, text.strip())
        for n in names:
            if n.startswith("wavs/") and n.endswith(".wav") and ".." not in n:
                target = os.path.join("/content/dataset/wavs", os.path.basename(n))
                if not os.path.exists(target):
                    with archive.open(n) as src, open(target, "wb") as dst:
                        shutil.copyfileobj(src, dst)

model_name = "erdem-tr-dizge"
if manifest.get("speaker"):
    model_name = "".join(c if c.isalnum() else "-" for c in manifest["speaker"].lower()).strip("-") + "-tr-dizge"
sample_rate = str(manifest.get("sample_rate", 22050))
assert sample_rate in ("16000", "22050"), sample_rate

rows = {n: t for n, t in rows.items() if os.path.exists(f"/content/dataset/wavs/{n}")}
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
