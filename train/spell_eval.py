"""Оценка ранжировщика на группах: исправления, ложные исправления, пропуски по порогам."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import numpy as np
import torch

import paths
from spell_common import K
from spell_model import load_net

DATA = paths.ROOT / "spell"

if __name__ == "__main__":
    split = sys.argv[1] if len(sys.argv) > 1 else "test"
    ckpt = sys.argv[2] if len(sys.argv) > 2 else "spell"
    d = np.load(DATA / f"{split}.npz")
    x, m, y = torch.from_numpy(d["x"]).long(), torch.from_numpy(d["m"]), torch.from_numpy(d["y"]).long()
    net = load_net(DATA / f"{ckpt}.pt")
    probs = []
    with torch.no_grad():
        for i in range(0, len(x), 256):
            b = x[i : i + 256]
            s = net(b.reshape(-1, b.shape[-1])).reshape(len(b), K).masked_fill(m[i : i + 256] == 0, -1e4)
            probs.append(torch.softmax(s, 1))
    p = torch.cat(probs)
    best, conf = p.argmax(1), p.max(1).values
    from spell_common import CHAR_ID, CTX, WLEN

    id_char = {v: k for k, v in CHAR_ID.items()}
    typed = ["".join(id_char.get(int(c), "") for c in r[0][CTX + 1 : CTX + 1 + WLEN] if c >= 3) for r in d["x"]]
    ru = torch.tensor([any("а" <= ch <= "я" or ch == "ё" for ch in t) for t in typed])
    typo, keep = y != 0, y == 0
    lines = [f"групп {len(y)}: с опечаткой {int(typo.sum())}, правильных редких {int(keep.sum())}"]
    for lang, lm in (("ru", ru), ("en", ~ru)):
        tl, kl = typo & lm, keep & lm
        lines.append(f"{lang}: опечаток {int(tl.sum())}, правильных {int(kl.sum())}")
        for t in (0.5, 0.7, 0.8, 0.85, 0.9, 0.95):
            act = (best != 0) & (conf >= t)
            right = (act & (best == y) & tl).sum() / tl.sum()
            wrong = (act & (best != y) & tl).sum() / tl.sum()
            false = (act & kl).sum() / max(1, kl.sum())
            lines.append(
                f"  порог {t:.2f}: исправлено верно {right:.1%}, неверно {wrong:.1%} | ложных правок {false:.2%}"
            )
    out = DATA / f"eval_{split}_{ckpt}.txt"
    out.write_text("\n".join(lines), encoding="utf-8")
    print(out)
