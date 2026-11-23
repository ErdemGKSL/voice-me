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
# Eğitim düzeltmeleri (fork'ta eksikti):
# 1) eğitim verisi her epoch karıştırılsın, yarım kalan son batch atılsın;
lit = pathlib.Path(REPO_DIR, "src/piper/train/vits/lightning.py")
patch(ds, "            self.train_dataset,\n",
      "            self.train_dataset,\n            shuffle=True,\n            drop_last=True,\n")
# 2) öğrenme hızı zamanlayıcıları her epoch sonunda adımlansın (manual optimization
#    kullanıldığı için Lightning bunu kendisi yapmıyor, LR sabit kalıyordu);
patch(lit, "    def validation_step(self, batch: Batch, batch_idx: int):\n",
"""    def on_train_epoch_end(self):
        for scheduler in self.lr_schedulers():
            scheduler.step()

    def validation_step(self, batch: Batch, batch_idx: int):
""")
# 3) init_from_checkpoint (ince ayar) üreticiyle birlikte ayırt ediciyi de yüklesin.
patch(lit, "            self._load_generator_weights(init_from_checkpoint)\n",
      "            self._load_generator_weights(init_from_checkpoint)\n"
      "            self._load_discriminator_weights(init_from_checkpoint)\n")
patch(lit, "    def _load_generator_weights(self, ckpt_path: str):\n",
"""    def _load_discriminator_weights(self, ckpt_path: str):
        if not os.path.isfile(ckpt_path):
            return
        ckpt = torch.load(ckpt_path, map_location="cpu", weights_only=False)
        state = ckpt.get("state_dict", ckpt)
        d_state = {k[len("model_d."):]: v for k, v in state.items() if k.startswith("model_d.")}
        if not d_state:
            _LOGGER.warning("init_from_checkpoint: no discriminator weights in %s", ckpt_path)
            return
        missing, _unexpected = self.model_d.load_state_dict(d_state, strict=False)
        _LOGGER.info("init_from_checkpoint: discriminator loaded (%d tensors, %d missing)",
                     len(d_state), len(missing))

    def _load_generator_weights(self, ckpt_path: str):
""")
os.environ["PIPER_PHONEME_TYPE"] = "dizge"
!cd {REPO_DIR} && git diff --stat && python -c "import piper.config as c; print(list(c.PhonemeType))"
