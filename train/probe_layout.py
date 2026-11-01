"""Диагностика: вероятности по словам для текста как на экране."""

import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import torch

from keys import CHAR_TOK, EN, NEUTRAL, OTHER, PAD, RU, SPACE
from model_layout import W, LayoutNet


def encode(text: str):
    tok, lay = [], []
    for c in text:
        l = EN if c.isascii() else RU
        tok.append(SPACE if c == " " else CHAR_TOK[l].get(c, OTHER))
        lay.append(l)
    return tok, lay


def probs(model, tok, lay):
    t = torch.tensor([[PAD] * (W - len(tok)) + tok])
    l = torch.tensor([[0] * (W - len(lay)) + lay])
    with torch.no_grad():
        return torch.sigmoid(model(t, l))[0, W - len(tok):].tolist()


def words(text, p, tok):
    out, s = [], 0
    for i in range(len(text) + 1):
        if i == len(text) or text[i] == " ":
            if i > s:
                v = [p[j] for j in range(s, i) if tok[j] not in NEUTRAL]
                out.append(f"{text[s:i]}={sum(v) / max(1, len(v)):.3f}")
            s = i + 1
    return "  ".join(out)


if __name__ == "__main__":
    ckpt = sys.argv[1]
    model = LayoutNet().eval()
    model.load_state_dict(torch.load(ckpt, map_location="cpu"))
    lines = []
    for text in sys.argv[2:]:
        tok, lay = encode(text)
        lines.append("целиком:  " + words(text, probs(model, tok, lay), tok))
        # как в программе: окно до каждого конца слова
        ends = [i for i in range(1, len(text) + 1) if (i == len(text) or text[i] == " ") and text[i - 1] != " "]
        for e in ends:
            lines.append(f"   до {e:>2}: " + words(text[:e], probs(model, tok[:e], lay[:e]), tok[:e]))
    out = Path(tempfile.gettempdir()) / "antipunto_probe.txt"
    out.write_text("\n".join(lines), encoding="utf-8")
    print(out)
