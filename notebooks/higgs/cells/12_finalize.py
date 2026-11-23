#@title 12. Özet, metadata.csv ve zip
#@markdown Piper eğitimi için 22050 Hz kopyası da çıkar:
EXPORT_22050 = True #@param {type:"boolean"}
import shutil, subprocess, zipfile
if subprocess.run("pgrep -f '[h]iggs_worker.py'", shell=True, capture_output=True).returncode == 0:
    raise SystemExit("Üretim hâlâ çalışıyor; bitmesini bekle ya da önce 11. hücreyle durdur. "
                     "(\"Run all\" ile buraya geldiysen sorun yok, üretim devam ediyor.)")
manifest = hc.load_manifest(JOB_DIR)
hc.reconcile(JOB_DIR, manifest)
hc.save_manifest(JOB_DIR, manifest)
hc.write_metadata(JOB_DIR, manifest)
counts = manifest["counts"]
total_s = sum(i["duration_s"] or 0 for i in manifest["items"] if i["status"] == "done")
print(f"bitti: {counts['done']}, başarısız: {counts['failed']}, bekleyen: {counts['pending']} | toplam ses: {total_s / 60:.1f} dk")
failed = [i for i in manifest["items"] if i["status"] == "failed"]
if failed:
    with open(os.path.join(JOB_DIR, "failed.txt"), "w", encoding="utf-8") as f:
        for item in failed:
            f.write(f"{item['id']}\t{item['error']}\t{item['text']}\n")
    print(f"{len(failed)} başarısız satır failed.txt'ye yazıldı; 9. hücrede RETRY_FAILED ile tekrar denenebilir")
out = f"/content/higgs/{JOB_NAME}.zip"
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
    z.write(os.path.join(JOB_DIR, "metadata.csv"), "metadata.csv")
    for item in manifest["items"]:
        if item["status"] != "done":
            continue
        src = os.path.join(JOB_DIR, item["file"])
        if EXPORT_22050:
            dst = f"/content/higgs/tmp/{item['id']}.wav"
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", src, "-ar", "22050", dst], check=True)
            z.write(dst, f"wavs/{item['id']}.wav")
            os.remove(dst)
        else:
            z.write(src, f"wavs/{item['id']}.wav")
shutil.copyfile(out, os.path.join(JOB_DIR, os.path.basename(out)))
print("zip Drive'a kopyalandı:", os.path.join(JOB_DIR, os.path.basename(out)))
drive.flush_and_unmount()
print("Drive senkronize edildi ve ayrıldı (tekrar kullanmak için 1. hücreyi çalıştır)")
