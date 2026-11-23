#@title 8. Ön dinleme: ilk 3 satır (işe yazılmaz)
#@markdown Beğenmezsen 5–6. hücrelere dönüp ayarı değiştir; iş henüz başlamadı. (Yeni işte değişiklikten sonra 7. hücreyi de tekrar çalıştır.)
PREVIEW_LINES = 3 #@param {type:"integer"}
from IPython.display import Audio, display
manifest = hc.load_manifest(JOB_DIR)
CONFIG = manifest["config"]
ref_local = "/content/higgs/reference.wav"
hc.copy_atomic(os.path.join(JOB_DIR, "reference.wav"), ref_local)
for index, item in enumerate(manifest["items"][:PREVIEW_LINES]):
    wav = hc.synth(item["text"], CONFIG, CONFIG["base_seed"] + index, ref_local, manifest["reference"]["text"],
                   timeout=hc.read_timeout(item["text"]))
    path = f"/content/higgs/preview_{item['id']}.wav"
    open(path, "wb").write(wav)
    problem = hc.validate(path, item["text"], CONFIG)
    print(f"{item['id']}: {item['text'][:100]}" + (f"  ⚠ {problem}" if problem else ""))
    display(Audio(path))
