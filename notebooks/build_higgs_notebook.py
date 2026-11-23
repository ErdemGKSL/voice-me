"""Build notebooks/Higgs_TTS_Dataset.ipynb from notebooks/higgs/*.

    uv run --python 3.13 python notebooks/build_higgs_notebook.py

`higgs_common.py` and `higgs_worker.py` are written into the notebook as
`%%writefile` cells right after the install cell, so the notebook, its
detached worker and these sources never drift.
"""

import json
import pathlib

HERE = pathlib.Path(__file__).resolve().parent
SRC = HERE / "higgs"


def source(text):
    lines = text.rstrip("\n").split("\n")
    return [line + "\n" for line in lines[:-1]] + [lines[-1]]


def code(text):
    return {"cell_type": "code", "metadata": {}, "execution_count": None, "outputs": [], "source": source(text)}


cells = []
for path in sorted((SRC / "cells").iterdir()):
    text = path.read_text(encoding="utf-8")
    if path.suffix == ".md":
        cells.append({"cell_type": "markdown", "metadata": {}, "source": source(text)})
    else:
        cells.append(code(text))
    if path.name.startswith("03_"):
        for module in ("higgs_common.py", "higgs_worker.py"):
            body = (SRC / module).read_text(encoding="utf-8")
            cells.append(code(f"%%writefile /content/higgs/{module}\n" + body))

notebook = {
    "cells": cells,
    "metadata": {
        "accelerator": "GPU",
        "colab": {"gpuType": "L4", "provenance": []},
        "kernelspec": {"display_name": "Python 3", "name": "python3"},
        "language_info": {"name": "python"},
    },
    "nbformat": 4,
    "nbformat_minor": 0,
}
out = HERE / "Higgs_TTS_Dataset.ipynb"
out.write_text(json.dumps(notebook, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
print("wrote", out.name, f"({len(cells)} cells)")
