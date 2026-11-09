#@title 3. DizgeBERT modelini indir ve dene
import hashlib, sys, urllib.request
BASE = "https://raw.githubusercontent.com/ErdemGKSL/voice-me/main/piper-voices/g2p/tr-dizge"
os.makedirs("/content/dizge", exist_ok=True)
for name in ("model.onnx", "vocab.json", "SHA256SUMS"):
    urllib.request.urlretrieve(f"{BASE}/{name}", f"/content/dizge/{name}")
for line in open("/content/dizge/SHA256SUMS", encoding="utf-8"):
    digest, name = line.split()
    got = hashlib.sha256(open(f"/content/dizge/{name.lstrip('*')}", "rb").read()).hexdigest()
    assert got == digest, f"{name}: sha256 uyusmuyor ({got})"
print("SHA-256 doğru.")

sys.path.insert(0, "/content")
import importlib, dizge_phonemizer
importlib.reload(dizge_phonemizer)
dz = dizge_phonemizer.DizgePhonemizer("/content/dizge/model.onnx", "/content/dizge/vocab.json")
for t in ["Merhaba Erdem, güneş ve çikolata 12 kahvaltı olduğunu", "Bugün hava çok güzel. Yarın görüşürüz!"]:
    print(t, "->", ["".join(s) for s in dz.phonemize("tr", t)])
