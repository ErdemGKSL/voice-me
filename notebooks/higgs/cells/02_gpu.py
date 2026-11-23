#@title 2. GPU ve disk kontrolü
#@markdown T4 gibi 20 GB'tan az bellekli bir GPU'da yine de denemek için işaretle:
FORCE_SMALL_GPU = False #@param {type:"boolean"}
import shutil, subprocess
gpu = subprocess.run("nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits",
                     shell=True, capture_output=True, text=True)
if gpu.returncode != 0:
    raise SystemExit("GPU yok: Runtime → Change runtime type → L4 GPU seç, sonra yeniden çalıştır.")
name, mem_mib = [part.strip() for part in gpu.stdout.strip().split("\n")[0].split(",")]
print(f"GPU: {name}, {int(mem_mib) / 1024:.0f} GB")
if int(mem_mib) < 20000 and not FORCE_SMALL_GPU:
    raise SystemExit("Bu GPU'nun belleği az (en az ~20 GB gerekir). L4 ya da A100 seç, "
                     "ya da FORCE_SMALL_GPU'yu işaretle (büyük ihtimalle bellek yetmez).")
free_gb = shutil.disk_usage("/content/drive/MyDrive").free / 1e9
print(f"Drive'da boş alan: {free_gb:.1f} GB (1000 satır ≈ 0.5 GB)")
