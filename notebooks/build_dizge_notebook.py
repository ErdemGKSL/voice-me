"""Build notebooks/Piper_Train_Dizge.ipynb from notebooks/cells/*.

    uv run --python 3.13 python notebooks/build_dizge_notebook.py

The notebook's `dizge_phonemizer.py` cell (02) is generated from
piper-voices/g2p/tr-dizge/train/dizge_phonemizer.py, so the two never drift.
"""

import json
import pathlib

HERE = pathlib.Path(__file__).resolve().parent
MODULE = HERE.parent / "piper-voices/g2p/tr-dizge/train/dizge_phonemizer.py"


def source(text):
    lines = text.rstrip("\n").split("\n")
    return [line + "\n" for line in lines[:-1]] + [lines[-1]]


def code(text):
    return {"cell_type": "code", "metadata": {}, "execution_count": None, "outputs": [], "source": source(text)}


cells = []
for path in sorted((HERE / "cells").iterdir()):
    text = path.read_text(encoding="utf-8")
    if path.suffix == ".md":
        cells.append({"cell_type": "markdown", "metadata": {}, "source": source(text)})
    else:
        cells.append(code(text))
    if path.name.startswith("01_"):
        module = MODULE.read_text(encoding="utf-8")
        # The self-test block is for local runs only.
        module = module.split("\n# ---- self-test")[0].rstrip("\n") + "\n"
        cells.append(code("%%writefile /content/dizge_phonemizer.py\n" + module))

notebook = {
    "cells": cells,
    "metadata": {
        "accelerator": "GPU",
        "colab": {"gpuType": "T4", "provenance": []},
        "kernelspec": {"display_name": "Python 3", "name": "python3"},
        "language_info": {"name": "python"},
    },
    "nbformat": 4,
    "nbformat_minor": 0,
}
out = HERE / "Piper_Train_Dizge.ipynb"
out.write_text(json.dumps(notebook, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
print("wrote", out.name, f"({len(cells)} cells)")
