# Erdem'in sesi: Piper eğitimi (DizgeBERT g2p)

`voice-me-recorder` ile aldığın **eğitim zip'ini** (`voice-me-dataset-*.zip`) ya da **yedek zip'ini** (`voice-me-backup-*.zip`) yükle; defter gerisini kendi yapar.

1. Runtime → Change runtime type → **T4 GPU**.
2. Runtime → **Run all**. Zip'i yükleme penceresi çıkınca seç (ya da 5. hücredeki `ZIP_PATH` ile Drive yolu ver).
3. Eğitim bitince (ya da durdurunca) son hücre `.onnx` + `.onnx.json` üretir ve indirir. Bunları voice-me'nin Piper ses klasörüne koy.

**İki aşamalı eğitim (önerilen):**

1. **Ana model, sıfırdan:** Higgs ile üretilen 20.000 cümlelik veri setinin zip'ini (`metadata.csv` + `wavs/*.flac`) Drive'a koy, 5. hücrede `ZIP_PATH`'e yolunu yaz, 6. hücrede `PRETRAINED_CHECKPOINT` boş kalsın. Sesler otomatik 22050 Hz'e çevrilir. Bu uzun sürer (L4'te günler); oturum kapanınca defteri tekrar çalıştır, Drive'daki `last.ckpt`'tan devam eder.
2. **Kendi sesinle ince ayar:** Ana model yeterince iyi olunca `voice-me-recorder` zip'ini ver, 6. hücrede `PRETRAINED_CHECKPOINT`'e ana modelin Drive'daki `.ckpt` yolunu yaz. Model adı zip'teki konuşmacıdan gelir, ana modelin klasörüne yazmaz.

Fonemizasyon uygulamadakiyle **birebir aynı**: voice-me'nin `turkish.rs`/`dizge.rs`/`phonemes.rs` kodunun Python portu (`dizge_phonemizer.py`) eğitimde de kullanılır, config'e `phoneme_type: "dizge"` yazılır.

Oturum kapanırsa: aynı defteri yeniden çalıştır. Çıktı Drive'a yazıldığı için `last.ckpt` bulunur ve eğitim kaldığı yerden sürer.
