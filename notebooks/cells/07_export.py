#@title 7. ONNX export (voice-me için) ve indirme
import glob, json
ckpts = glob.glob(f"{output_dir}/lightning_logs/**/checkpoints/*.ckpt", recursive=True)
assert ckpts, "kontrol noktası yok; önce eğitim hücresini çalıştır"
latest = max(ckpts, key=os.path.getmtime)
print("kontrol noktası:", latest)
out_onnx = f"{output_dir}/{model_name}.onnx"
!cd {REPO_DIR} && python -m piper.train.export_onnx --checkpoint "{latest}" --output-file "{out_onnx}"
cfg_json = json.load(open(config_path, encoding="utf-8"))
assert cfg_json["phoneme_type"] == "dizge", cfg_json["phoneme_type"]
shutil.copy(config_path, out_onnx + ".json")
print("Hazır:", out_onnx, "ve", out_onnx + ".json", f"(phoneme_type=dizge, num_symbols={cfg_json['num_symbols']})")
bundle = f"/content/{model_name}-voice.zip"
with zipfile.ZipFile(bundle, "w", zipfile.ZIP_DEFLATED) as z:
    z.write(out_onnx, f"{model_name}.onnx")
    z.write(out_onnx + ".json", f"{model_name}.onnx.json")
from google.colab import files
files.download(bundle)
