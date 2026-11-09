# Erdem'in sesi: Piper eğitimi (DizgeBERT g2p)

`voice-me-recorder` ile aldığın **eğitim zip'ini** (`voice-me-dataset-*.zip`) ya da **yedek zip'ini** (`voice-me-backup-*.zip`) yükle; defter gerisini kendi yapar.

1. Runtime → Change runtime type → **T4 GPU**.
2. Runtime → **Run all**. Zip'i yükleme penceresi çıkınca seç (ya da 5. hücredeki `ZIP_PATH` ile Drive yolu ver).
3. Eğitim bitince (ya da durdurunca) son hücre `.onnx` + `.onnx.json` üretir ve indirir. Bunları voice-me'nin Piper ses klasörüne koy.

Fonemizasyon uygulamadakiyle **birebir aynı**: voice-me'nin `turkish.rs`/`dizge.rs`/`phonemes.rs` kodunun Python portu (`dizge_phonemizer.py`) eğitimde de kullanılır, config'e `phoneme_type: "dizge"` yazılır.

Oturum kapanırsa: aynı defteri yeniden çalıştır. Çıktı Drive'a yazıldığı için `last.ckpt` bulunur ve eğitim kaldığı yerden sürer.
