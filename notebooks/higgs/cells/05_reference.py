#@title 5. Ses: hazır ses ya da kendi referans kaydın (yalnızca yeni işte)
#@markdown **Hazır sesler** seed'den üretilir ve her satırda o ses kopyalanır. Etiketteki Hz, ölçülen ortalama ses perdesidir.
VOICE = "Erkek – normal, seed 38 (~115–126 Hz)" #@param ["Kendi kaydım", "Erkek – normal, seed 38 (~115–126 Hz)", "Erkek – tok, seed 39 (~112 Hz)", "Erkek – kalın (~80 Hz)", "Kadın – orta (~190 Hz)", "Kadın – ince (~210 Hz)"]
#@markdown ---
#@markdown **Kendi kaydım** seçiliyse: Drive'daki dosyanın yolu (boşsa yükleme penceresi açılır). WAV, MP3, FLAC, M4A olur; 5–15 sn ideal.
REF_DRIVE_PATH = "" #@param {type:"string"}
#@markdown Kayıtta tam olarak ne söylendiği (zorunlu; klonlama kalitesini çok etkiler):
REF_TRANSCRIPT = "" #@param {type:"string"}
import shutil, subprocess
import soundfile as sf
from IPython.display import Audio, display

if MODE == "continue":
    print("devam modu: referans Drive'daki işten okunuyor")
    display(Audio(os.path.join(JOB_DIR, "reference.wav")))
else:
    raw = "/content/higgs/ref_raw"
    if VOICE == "Kendi kaydım":
        assert REF_TRANSCRIPT.strip(), "REF_TRANSCRIPT boş: kayıtta ne söylendiğini yaz"
        if REF_DRIVE_PATH.strip():
            src = REF_DRIVE_PATH if REF_DRIVE_PATH.startswith("/") else os.path.join("/content/drive/MyDrive", REF_DRIVE_PATH)
            shutil.copyfile(src, raw)
        else:
            from google.colab import files
            uploaded = files.upload()
            assert uploaded, "dosya yüklenmedi"
            open(raw, "wb").write(next(iter(uploaded.values())))
        ref_text = REF_TRANSCRIPT.strip()
        voice_info = {"kind": "upload", "label": VOICE}
    else:
        ref_text = hc.make_preset_reference(VOICE, raw)
        voice_info = {"kind": "preset", "label": VOICE, **hc.VOICE_PRESETS[VOICE]}
    # 24 kHz mono 16-bit, leading/trailing silence trimmed.
    ref_local = "/content/higgs/reference.wav"
    trim = "silenceremove=start_periods=1:start_threshold=-45dB,areverse,silenceremove=start_periods=1:start_threshold=-45dB,areverse"
    subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", raw, "-af", trim,
                    "-ac", "1", "-ar", "24000", "-sample_fmt", "s16", ref_local], check=True)
    duration = sf.info(ref_local).duration
    print(f"referans: {duration:.1f} sn")
    if duration > 30:
        raise SystemExit("Referans 30 sn'den uzun; 5–15 sn'lik bir parça kullan.")
    if not 5 <= duration <= 15:
        print("uyarı: 5–15 sn arası en iyi sonucu verir")
    hc.copy_atomic(ref_local, os.path.join(JOB_DIR, "reference.wav"))
    with open(os.path.join(JOB_DIR, "reference.txt"), "w", encoding="utf-8") as f:
        f.write(ref_text + "\n")
    REFERENCE = {"sha256": hc.sha256_file(ref_local), "duration_s": round(duration, 2), "text": ref_text, "voice": voice_info}
    display(Audio(ref_local))
