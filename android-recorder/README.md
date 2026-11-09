# Ses Kaydedici (Android)

Telefondan, boş zamanlarında `voice-me-recorder` ile aynı biçimde ses kaydı almak için küçük bir Kotlin uygulaması. Aldığın kayıtlar masaüstü kaydedicideki ve Colab defterindeki zip biçimiyle birebir uyumludur.

- 20 248 cümle uygulamanın içine gömülü (`app/src/main/assets/sentences.csv`). Sıra masaüstü kaydedicidekiyle aynı: önce dili en çok kapsayan (karakter başına en çok yeni kelime/harf çifti) cümleler, her üç uzun cümleden sonra bir kısa ifade.
- Her kayıt durduğu anda telefona yazılır (`recordings/wavs/<id>.wav`); uygulamayı kapatmak kayıp yaratmaz.
- **Eğitim zip'i**: `metadata.csv`, `wavs/`, `manifest.json` → Colab defterine yükle.
- **Yedekle (devam için)**: aynı dosyalar + `resume.json`. **İçe aktar** ile başka bir telefonda ya da masaüstü kaydedicide kaldığın cümleden devam edersin; masaüstü ve mobil zip'leri birbirine aktarılabilir. Zip'i Drive'a kaydet (dışa aktarırken Drive'ı seçebilirsin).
- Ses kalitesi masaüstü kaydedicideki gibi: işlenmemiş mikrofon kaynağı (otomatik kazanç/gürültü bastırma/yankı engelleme yok), 48 kHz yakalama, DC temizliği, sinc ile 22 050 Hz'e çevirme, kenar sessizliğini kırpma, −1 dBFS'e seviyeleme, fade, tek seferlik 16-bit yuvarlama. Kısa/çok kısık kayıt reddedilir; clipping ve gürültülü ortam (SNR < 30 dB) uyarılır.
- Mikrofon seçilebilir (Bluetooth kulaklık listede yok: 8–16 kHz kayıt verir).

## Kurulum

GitHub Actions `android-recorder` iş akışı her push'ta APK üretir: Actions → son çalışma → Artifacts → `voice-me-recorder-debug-apk`. APK'yı telefona atıp aç ("bilinmeyen kaynaklardan yüklemeye" izin ver).

Yerelde (Android Studio'nun JDK'sı ve SDK'sı ile):

```bash
cd android-recorder
./gradlew testDebugUnitTest assembleDebug
# app/build/outputs/apk/debug/app-debug.apk
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

`sentences.csv` masaüstü kaydedicinin sırasından üretilir:

```bash
VOICE_ME_DUMP=android-recorder/app/src/main/assets/sentences.csv \
  cargo test --release -p voice-me-recorder dump_bundled_order -- --ignored
```

Not: Uygulama silinirse telefondaki kayıtlar da silinir; düzenli **Yedekle** kullan.
