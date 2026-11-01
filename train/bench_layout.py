"""Экспорт в OpenVINO IR и замер на CPU/NPU."""

import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import numpy as np
import openvino as ov
import torch

from model_layout import W, LayoutNet
from paths import LAYOUT

DATA = LAYOUT


class Probs(torch.nn.Module):
    def __init__(self, m):
        super().__init__()
        self.m = m

    def forward(self, tok, lay):
        return torch.sigmoid(self.m(tok.long(), lay.long()))


if __name__ == "__main__":
    ckpt = Path(sys.argv[1]) if len(sys.argv) > 1 else DATA / "layout.pt"
    name = sys.argv[2] if len(sys.argv) > 2 else "layout"
    torch.backends.mha.set_fastpath_enabled(False)
    net = LayoutNet().eval()
    net.load_state_dict(torch.load(ckpt, map_location="cpu"))
    wrap = Probs(net).eval()

    d = np.load(DATA / "valid.npz")
    tok = d["tok"][:200].astype(np.int32)
    lay = d["lay"][:200].astype(np.int32)
    with torch.no_grad():
        ref = wrap(torch.from_numpy(tok), torch.from_numpy(lay)).numpy()

    ex = (torch.zeros(1, W, dtype=torch.int32), torch.zeros(1, W, dtype=torch.int32))
    ovm = ov.convert_model(wrap, example_input=ex, input=[([1, W], ov.Type.i32), ([1, W], ov.Type.i32)])
    ov.save_model(ovm, DATA / f"{name}.xml", compress_to_fp16=True)
    size = (DATA / f"{name}.bin").stat().st_size / 1e6
    print(f"IR: {DATA / f'{name}.xml'}, веса {size:.2f} МБ")

    core = ov.Core()
    core.set_property({"CACHE_DIR": str(DATA / "ovcache")})
    for dev in ("CPU", "NPU"):
        t = time.perf_counter()
        cm = core.compile_model(DATA / f"{name}.xml", dev, {"PERFORMANCE_HINT": "LATENCY"})
        comp = time.perf_counter() - t
        req = cm.create_infer_request()
        out = np.stack([req.infer([tok[i : i + 1], lay[i : i + 1]])[0][0] for i in range(len(tok))])
        diff = np.abs(out - ref).max()
        agree = ((out > 0.5) == (ref > 0.5)).mean()
        for _ in range(50):
            req.infer([tok[:1], lay[:1]])
        n = 2000
        ts = []
        for i in range(n):
            t = time.perf_counter()
            req.infer([tok[i % 200 : i % 200 + 1], lay[i % 200 : i % 200 + 1]])
            ts.append(time.perf_counter() - t)
        ts = np.array(ts) * 1e3
        print(f"{dev}: компиляция {comp:.2f} с, задержка медиана {np.median(ts):.3f} мс, p99 {np.percentile(ts, 99):.3f} мс, "
              f"расхождение с torch {diff:.4f}, совпадение решений {agree:.4%}")
