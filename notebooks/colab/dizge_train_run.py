"""Run notebooks/Piper_Train_Dizge.ipynb unattended on a Colab runtime.

    python3 dizge_train_run.py <notebook.ipynb> <zip path(s)> [batch_size] [checkpoint_every_epochs]

Fills the form fields (no Drive, the given dataset zip(s), output under
/content/piper-out), drops the export cell, and executes the rest with
`jupyter nbconvert` in this process — run it with nohup/setsid so it
outlives the caller. Training resumes by itself when a `last.ckpt` exists
under /content/piper-out/<model>/lightning_logs/*/checkpoints/ (the
training cell looks there first), so after a runtime reset: put the last
checkpoint back in that place and run this again.

Progress: /content/piper-run/train.log (the training command's output is
tee'd there) and the checkpoint files themselves.
"""
import json
import os
import re
import subprocess
import sys

notebook, zips = sys.argv[1], sys.argv[2]
batch = sys.argv[3] if len(sys.argv) > 3 else "32"
every = sys.argv[4] if len(sys.argv) > 4 else "50"
RUN = "/content/piper-run"
os.makedirs(RUN, exist_ok=True)

nb = json.load(open(notebook, encoding="utf-8"))


def set_param(source, name, value):
    new, n = re.subn(rf"^{name} = .*$", f"{name} = {value}", source, flags=re.M)
    assert n == 1, f"{name} not found"
    return new


cells = []
for cell in nb["cells"]:
    src = "".join(cell["source"])
    if cell["cell_type"] == "code":
        if src.startswith("#@title 1."):
            src = set_param(src, "USE_DRIVE", "False")
        elif src.startswith("#@title 5."):
            src = set_param(src, "ZIP_PATH", json.dumps(zips))
        elif src.startswith("#@title 6."):
            src = set_param(src, "BATCH_SIZE", batch)
            src = set_param(src, "CHECKPOINT_EVERY_EPOCHS", every)
            # Stream the training output to a log file a supervisor can read.
            src = src.replace("get_ipython().system(cmd)",
                              f'get_ipython().system(cmd + " 2>&1 | tee -a {RUN}/train.log")')
        elif src.startswith("#@title 7."):
            continue  # export needs files.download; done separately on demand
    cell["source"] = src
    cells.append(cell)
nb["cells"] = cells
run_nb = os.path.join(RUN, "run.ipynb")
json.dump(nb, open(run_nb, "w", encoding="utf-8"), ensure_ascii=False)

with open(os.path.join(RUN, "status"), "w") as f:
    f.write("running\n")
result = subprocess.run(
    ["jupyter", "nbconvert", "--to", "notebook", "--execute", run_nb, "--output", "run.out.ipynb",
     "--ExecutePreprocessor.timeout=-1", "--ExecutePreprocessor.kernel_name=python3"],
    cwd=RUN)
with open(os.path.join(RUN, "status"), "w") as f:
    f.write(f"exited {result.returncode}\n")
