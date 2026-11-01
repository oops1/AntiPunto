"""Экспорт ранжировщика в OpenVINO IR [K, L] → [K] и копия словарей в models/."""

import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import numpy as np
import openvino as ov
import torch

import paths
from spell_common import K, L
from spell_model import SpellNet

DATA = paths.ROOT / "spell"
MODELS = Path(__file__).resolve().parent.parent / "models"


class Scores(torch.nn.Module):
    def __init__(self, m):
        super().__init__()
        self.m = m

    def forward(self, x):
        return self.m(x.long())


if __name__ == "__main__":
    torch.backends.mha.set_fastpath_enabled(False)
    net = SpellNet().eval()
    net.load_state_dict(torch.load(DATA / "spell.pt", map_location="cpu"))
    wrap = Scores(net).eval()
    ovm = ov.convert_model(wrap, example_input=torch.zeros(K, L, dtype=torch.int32), input=[([K, L], ov.Type.i32)])
    ov.save_model(ovm, DATA / "spell.xml", compress_to_fp16=True)

    d = np.load(DATA / "valid.npz")
    x = d["x"][:50].astype(np.int32)
    core = ov.Core()
    for dev in ("CPU", "NPU"):
        cm = core.compile_model(DATA / "spell.xml", dev, {"INFERENCE_PRECISION_HINT": "f32"} if dev == "CPU" else {})
        same = 0
        for g in x:
            with torch.no_grad():
                ref = wrap(torch.from_numpy(g)).numpy()
            got = cm([g])[0]
            same += int(ref.argmax() == got.argmax())
        print(f"{dev}: совпадение выбора с torch {same}/{len(x)}")

    MODELS.mkdir(exist_ok=True)
    for name in ("spell.xml", "spell.bin"):
        shutil.copy(DATA / name, MODELS / name)
    for lang in ("ru", "en"):
        shutil.copy(DATA / f"{lang}.tsv", MODELS / f"spell_{lang}.tsv")
    print("скопировано в", MODELS)
