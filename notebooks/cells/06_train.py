#@title 6. Eğitim (otomatik: kontrol noktası varsa devam eder)
#@markdown Boş: sıfırdan (Higgs 20k ana modeli böyle eğitilir). Doluysa (URL veya yol) bu kontrol noktasından finetune edilir: kendi sesinle ince ayar için ana modelin Drive'daki `.ckpt` yolunu yaz.
PRETRAINED_CHECKPOINT = "" #@param {type:"string"}
QUALITY = "medium" #@param ["x-low", "medium", "high"]
#@markdown T4 için 12, L4 için 24–32 (bellek hatası alırsan düşür).
BATCH_SIZE = 12 #@param {type:"integer"}
MAX_EPOCHS = 10000 #@param {type:"integer"}
CHECKPOINT_EVERY_EPOCHS = 5 #@param {type:"integer"}
VALIDATION_FRACTION = 0.01 #@param {type:"number"}

import glob, re
os.environ["PIPER_PHONEME_TYPE"] = "dizge"
config_path = f"{output_dir}/config.json"
quality_args = {
    "x-low": "--model.hidden_channels 96 --model.inter_channels 96 --model.filter_channels 384 ",
    "medium": "",
    "high": ('--model.resblock 1 --model.resblock_kernel_sizes "(3, 7, 11)" '
             '--model.resblock_dilation_sizes "((1, 3, 5), (1, 3, 5), (1, 3, 5))" '
             '--model.upsample_rates "(8, 8, 2, 2)" --model.upsample_initial_channel 512 '
             '--model.upsample_kernel_sizes "(16, 16, 4, 4)" '),
}[QUALITY]

last = glob.glob(f"{output_dir}/lightning_logs/**/checkpoints/last.ckpt", recursive=True)
extra = ""
if last:
    last.sort(key=lambda p: int((re.search(r"version_(\d+)", p) or [0, -1])[1]))
    extra = f'--ckpt_path "{last[-1]}" '
    print("Kaldığı yerden devam:", last[-1])
elif PRETRAINED_CHECKPOINT.strip():
    ckpt = PRETRAINED_CHECKPOINT.strip()
    if ckpt.startswith("http"):
        !aria2c --quiet=true --file-allocation=none -d /content -o pretrained.ckpt "{ckpt}"
        ckpt = "/content/pretrained.ckpt"
    assert os.path.exists(ckpt), "PRETRAINED_CHECKPOINT bulunamadı"
    extra = f'--model.init_from_checkpoint "{ckpt}" '
    print("Finetune:", ckpt)
else:
    print("Sıfırdan eğitim.")

val = (f"--data.validation_split {VALIDATION_FRACTION} --data.num_test_examples 1 "
       if VALIDATION_FRACTION > 0 else "--data.validation_split 0 --data.num_test_examples 0 ")
cmd = (
    f"cd {REPO_DIR} && PIPER_PHONEME_TYPE=dizge python -m piper.train fit "
    f'--data.voice_name "{model_name}" '
    '--data.csv_path "/content/dataset/metadata.csv" '
    '--data.audio_dir "/content/dataset/wavs" '
    '--data.espeak_voice "tr" '
    f'--data.cache_dir "{audio_cache_dir}" '
    f'--data.config_path "{config_path}" '
    f"--data.batch_size {BATCH_SIZE} --model.sample_rate {sample_rate} "
    + quality_args +
    f'--trainer.default_root_dir "{output_dir}" '
    "--trainer.accelerator gpu --trainer.devices 1 "
    f"--trainer.max_epochs {MAX_EPOCHS} --trainer.log_every_n_steps 1000 "
    "--trainer.precision 16-mixed " + val +
    f"--checkpoint.every_n_epochs {CHECKPOINT_EVERY_EPOCHS} --checkpoint.save_top_k -1 "
    "--checkpoint.monitor null "
    f"--last_checkpoint.every_n_epochs {CHECKPOINT_EVERY_EPOCHS} " + extra
)
print(cmd)
get_ipython().system(cmd)
