#@title 1. GPU, Drive ve kurulum
import os, subprocess
if subprocess.run("nvidia-smi", shell=True, capture_output=True).returncode != 0:
    raise SystemExit("GPU yok: Runtime → Change runtime type → T4 GPU seç, sonra yeniden çalıştır.")
!nvidia-smi --query-gpu=name,memory.total --format=csv,noheader

#@markdown Drive bağlanırsa kontrol noktaları orada saklanır ve oturum kapansa da eğitim devam edebilir.
USE_DRIVE = True #@param {type:"boolean"}
DRIVE_OUTPUT = "/content/drive/MyDrive/colab/piper" #@param {type:"string"}
if USE_DRIVE:
    from google.colab import drive
    drive.mount('/content/drive')
    OUTPUT_ROOT = DRIVE_OUTPUT
else:
    OUTPUT_ROOT = "/content/piper-out"
os.makedirs(OUTPUT_ROOT, exist_ok=True)

!apt-get -q update -y
!apt-get -q install -y build-essential cmake ninja-build espeak-ng aria2
REPO_URL = "https://github.com/KiON-GiON/piper1-gpl.git"
REPO_DIR = "/content/piper1-gpl"
if not os.path.exists(REPO_DIR):
    !git clone -q -b fixes {REPO_URL} {REPO_DIR}
%cd {REPO_DIR}
!python -m pip install -q -e .[train]
!bash build_monotonic_align.sh
!pip install -q --upgrade gdown scikit-build protobuf==3.20.3
!python setup.py build_ext --inplace
!pip install -q onnxruntime numpy
print("Kurulum tamam.")
