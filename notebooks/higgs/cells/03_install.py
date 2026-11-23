#@title 3. Kurulum (her yeni makinede bir kez, ~3–4 dk)
import os, shutil, subprocess
os.chdir("/content")
if shutil.which("sgl-omni") is None:
    # outlines-core has no wheel here and builds from source, so it needs Rust.
    !curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal > /dev/null
    os.environ["PATH"] = os.path.expanduser("~/.cargo/bin") + ":" + os.environ["PATH"]
    !uv pip install --system -q --prerelease=allow "sglang-omni==0.2.0" requests librosa soundfile
else:
    print("sglang-omni zaten kurulu")
!hf download bosonai/higgs-tts-3-4b > /dev/null && echo "model indirildi"
import torch
assert torch.cuda.is_available(), "Kurulumdan sonra torch CUDA'yı görmüyor; makineyi sıfırlayıp tekrar dene."
os.makedirs(os.path.join(JOB_DIR, "logs"), exist_ok=True)
!pip freeze > "{JOB_DIR}/logs/pip-freeze.txt"
print("torch", torch.__version__, "| CUDA hazır")
