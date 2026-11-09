#@title 4. Piper'a dizge yaması (tekrar çalıştırmak güvenli)
import pathlib
cfg = pathlib.Path(REPO_DIR, "src/piper/config.py")
ds = pathlib.Path(REPO_DIR, "src/piper/train/vits/dataset.py")

def patch(path, old, new):
    s = path.read_text(encoding="utf-8")
    if new in s:
        return
    assert s.count(old) == 1, f"{path.name}: yama noktası bulunamadı: {old!r}"
    path.write_text(s.replace(old, new), encoding="utf-8")

patch(cfg, '    TEXT = "text"\n', '    TEXT = "text"\n    DIZGE = "dizge"\n')
patch(ds, "import math\n", "import math\nimport os\nimport sys\n")
patch(ds, "phoneme_type=PhonemeType.ESPEAK,",
      'phoneme_type=PhonemeType(os.environ.get("PIPER_PHONEME_TYPE", "espeak")),')
patch(ds, "        phonemizer = EspeakPhonemizer()\n",
'''        if os.environ.get("PIPER_PHONEME_TYPE") == "dizge":
            sys.path.insert(0, "/content")
            from dizge_phonemizer import DizgePhonemizer
            phonemizer = DizgePhonemizer("/content/dizge/model.onnx", "/content/dizge/vocab.json")
        else:
            phonemizer = EspeakPhonemizer()
''')
os.environ["PIPER_PHONEME_TYPE"] = "dizge"
!cd {REPO_DIR} && git diff --stat && python -c "import piper.config as c; print(list(c.PhonemeType))"
