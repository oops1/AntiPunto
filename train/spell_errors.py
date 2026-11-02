"""Примеры решений ранжировщика: ложные правки редких слов и ошибки в опечатках."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import numpy as np
import torch

import paths
from spell_common import CHAR_ID, CTX, K, WLEN
from spell_model import SpellNet

DATA = paths.ROOT / "spell"
ID_CHAR = {v: k for k, v in CHAR_ID.items()}


def text(row, a, n):
    return "".join(ID_CHAR.get(int(c), "") for c in row[a : a + n] if c >= 3)


if __name__ == "__main__":
    t = float(sys.argv[1]) if len(sys.argv) > 1 else 0.90
    d = np.load(DATA / "test.npz")
    x, m, y = d["x"][:6000], d["m"][:6000], d["y"][:6000]
    net = SpellNet().eval()
    net.load_state_dict(torch.load(DATA / "spell.pt", map_location="cpu"))
    with torch.no_grad():
        s = net(torch.from_numpy(x).long().reshape(-1, x.shape[-1])).reshape(len(x), K)
        s = s.masked_fill(torch.from_numpy(m) == 0, -1e4)
        p = torch.softmax(s, 1).numpy()
    false, wrong = [], []
    for g in range(len(x)):
        b, c = p[g].argmax(), p[g].max()
        if b == 0 or c < t:
            continue
        typed = text(x[g][0], CTX + 1, WLEN)
        chosen = text(x[g][b], CTX + 2 + WLEN, WLEN)
        ctx = text(x[g][0], 0, CTX)
        line = f"{ctx[-25:]:>25} | {typed} → {chosen} ({c:.2f})"
        if y[g] == 0:
            false.append(line)
        elif b != y[g]:
            wrong.append(line + f" [верно: {text(x[g][y[g]], CTX + 2 + WLEN, WLEN)}]")
    out = DATA / "errors.txt"
    out.write_text("ЛОЖНЫЕ ПРАВКИ:\n" + "\n".join(false[:60]) + "\n\nНЕВЕРНЫЕ:\n" + "\n".join(wrong[:30]), encoding="utf-8")
    print(out)
