"""Export iatagun/dizge-g2p to the ONNX graph voice-me downloads.

    pip install --index-url https://download.pytorch.org/whl/cpu torch
    pip install "transformers>=4.44,<5" onnx onnxruntime safetensors huggingface_hub
    python export_onnx.py            # writes model.onnx and vocab.json here

The model reads one letter per token, so the 32 000-row word-embedding table
is cut down to [PAD] [UNK] [CLS] [SEP] and the letters voice-me feeds it;
`token_type_ids` are always zero and are folded into the graph. The graph's
MatMul weights are then quantized to int8 (per channel), which keeps it
under GitHub's 100 MB file limit; on 20 000 frequent Turkish words it
agrees with the float32 graph on 98.9 % of them.
"""

import json
import os

import torch
from huggingface_hub import snapshot_download
from onnxruntime.quantization import QuantType, quantize_dynamic
from onnxruntime.quantization.shape_inference import quant_pre_process
from transformers import AutoModelForTokenClassification, AutoTokenizer

REPO = "iatagun/dizge-g2p"
REVISION = "2afa9b7941a6feac766e040107561fe6d27ac8f1"
LETTERS = "abcçdefgğhıijklmnoöpqrsştuüvwxyzâîû"
SPECIAL = [0, 1, 2, 3]  # [PAD] [UNK] [CLS] [SEP]
HERE = os.path.dirname(os.path.abspath(__file__))

local = snapshot_download(REPO, revision=REVISION)
tokenizer = AutoTokenizer.from_pretrained(local)
model = AutoModelForTokenClassification.from_pretrained(local).eval()
with open(os.path.join(local, "task_config.json"), encoding="utf-8") as f:
    task = json.load(f)

letter_ids = {}
for letter in LETTERS:
    ids = tokenizer(letter, add_special_tokens=False)["input_ids"]
    assert len(ids) == 1, (letter, ids)
    letter_ids[letter] = ids[0]
kept = SPECIAL + sorted(set(letter_ids.values()) - set(SPECIAL))
new_id = {old: new for new, old in enumerate(kept)}

embeddings = model.bert.embeddings.word_embeddings.weight.data[kept].clone()
model.bert.embeddings.word_embeddings = torch.nn.Embedding.from_pretrained(
    embeddings, freeze=True, padding_idx=0
)
model.config.vocab_size = len(kept)


class Graph(torch.nn.Module):
    def __init__(self, model):
        super().__init__()
        self.model = model

    def forward(self, input_ids, attention_mask):
        return self.model(
            input_ids=input_ids,
            attention_mask=attention_mask,
            token_type_ids=torch.zeros_like(input_ids),
        ).logits


fp32 = os.path.join(HERE, "model.fp32.onnx")
pre = os.path.join(HERE, "model.pre.onnx")
torch.onnx.export(
    Graph(model).eval(),
    (torch.tensor([[2, 4, 5, 3]]), torch.tensor([[1, 1, 1, 1]])),
    fp32,
    input_names=["input_ids", "attention_mask"],
    output_names=["logits"],
    dynamic_axes={
        "input_ids": {0: "batch", 1: "sequence"},
        "attention_mask": {0: "batch", 1: "sequence"},
        "logits": {0: "batch", 1: "sequence"},
    },
    opset_version=17,
    dynamo=False,
)
quant_pre_process(fp32, pre, skip_symbolic_shape=True)
quantize_dynamic(
    pre,
    os.path.join(HERE, "model.onnx"),
    weight_type=QuantType.QInt8,
    per_channel=True,
    op_types_to_quantize=["MatMul"],
)
os.remove(fp32)
os.remove(pre)

vocab = {
    "source": f"https://huggingface.co/{REPO}/tree/{REVISION}",
    "licence": "MIT",
    "pad": 0,
    "unk": 1,
    "cls": 2,
    "sep": 3,
    "max_length": task["max_length"],
    "chars": {letter: new_id[old] for letter, old in letter_ids.items()},
    "labels": task["label_list"],
}
with open(os.path.join(HERE, "vocab.json"), "w", encoding="utf-8") as f:
    json.dump(vocab, f, ensure_ascii=False, indent=1)
