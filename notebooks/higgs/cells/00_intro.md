# Higgs TTS 3 ile okuma listesinden ses veri seti

Bir referans sesle (kendi kaydın ya da hazır bir ses) bir okuma listesindeki her cümleyi seslendirir. Her satır bir WAV olur ve iş ilerledikçe **Google Drive'a kaydedilir**. Colab kopsa da, makine sıfırlansa da **kaldığın yerden devam edersin**.

**Nasıl kullanılır**

1. Runtime → Change runtime type → **L4 GPU** (ya da A100). T4'ün belleği yetmez.
2. **1. hücre**: `MODE = "new"`, Drive klasörünü ve iş adını yaz.
3. Hücreleri sırayla çalıştır. **5–7. hücreler** sesi, ayarları ve okuma listesini sorar. **8. hücre** ilk 3 satırı dinletir. **9. hücre** üretimi başlatır, **10. hücre** ilerlemeyi gösterir.
4. Colab koparsa: **1. hücrede** `MODE = "continue"` seç, aynı klasör ve iş adını yaz, sonra **Runtime → Run all**. Ses, ayarlar ve liste Drive'daki işten okunur, form değerleri yok sayılır.

**Drive'daki iş klasörü**: `manifest.json` (her satırın durumu), `events.jsonl` (her denemenin kaydı), `config.json`, `reference.wav` + `reference.txt`, `reading_list.txt`, `wavs/000001.wav…`, `metadata.csv` (Piper/LJSpeech biçimi `id|metin`), `logs/`.

**Lisans**: `bosonai/higgs-tts-3-4b` araştırma ve ticari olmayan kullanım içindir (içerik üreticilerine ayrı bir izin var, "Higgs Audio" adının anılması gerekir). Bu sesle eğitilen bir modelin ya da veri setinin ticari kullanımı lisansa takılabilir. Başkasının sesini izni olmadan kopyalama.

**Kontrol etiketleri**: Bir satırın başına `<|emotion:sadness|>`, `<|prosody:speed_slow|>`, `<|style:whispering|>` gibi etiketler koyabilirsin; satır içine `<|prosody:pause|>` ya da `<|sfx:laughter|>Haha,` gibi etiketler girebilirsin. Etiketler `metadata.csv`'ye yazılmaz. Tam liste 6. hücrede.
