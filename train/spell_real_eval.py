"""Оценка на настоящих опечатках: разметка real_ru_labels.tsv, путь как в клиенте.

python spell_real_eval.py spell_v1 spell_v2 spell — чекпойнты из папки данных.
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import torch

import paths
from spell_common import K, L, Dictionary, osa, row
from spell_keep import Bloom
from spell_known import MODELS
from spell_model import load_net

DATA = paths.ROOT / "spell"
THRESHOLDS = (0.6, 0.7, 0.8, 0.85, 0.9)


def load_cases():
    ctx = {}
    for line in (DATA / "real_words.tsv").read_text(encoding="utf-8").splitlines()[1:]:
        lang, w, _, _, _, _, c = (line.split("\t") + [""] * 7)[:7]
        if lang == "ru":
            ctx[w] = c
    cases = []
    lines = (DATA / "real_ru_labels.tsv").read_text(encoding="utf-8").splitlines()[1:]
    counts = dict(l.split("\t")[:2] for l in (DATA / "real_ru_unlabeled.tsv").read_text(encoding="utf-8").splitlines()[1:])
    for line in lines:
        w, label, cls = line.split("\t")[:3]
        if cls in ("typo", "jargon_typo") and " " not in label:
            cases.append((w, label.replace("ё", "е"), ctx.get(w, ""), int(counts.get(w, 1))))
        elif cls == "keep":
            cases.append((w, None, ctx.get(w, ""), int(counts.get(w, 1))))
    return cases


def choose(net, d: Dictionary, known: Bloom, ctx: str, w: str):
    """(выбор, уверенность); None — клиент модель не спрашивает."""
    if w in d.freq or w in known:
        return None
    cands = d.candidates(w)
    if not cands:
        return None
    x = torch.zeros((K, L), dtype=torch.long)
    x[0] = torch.tensor(row(ctx, w, w))
    for j, c in enumerate(cands, 1):
        x[j] = torch.tensor(row(ctx, w, c, j, osa(w, c), d.freq[c]))
    with torch.no_grad():
        p = torch.softmax(net(x)[: len(cands) + 1], 0)
    b = int(p.argmax())
    return (w if b == 0 else cands[b - 1]), float(p[b])


if __name__ == "__main__":
    d, known = Dictionary("ru"), Bloom(MODELS / "spell_known.bloom")
    cases = load_cases()
    typos = [c for c in cases if c[1]]
    keeps = [c for c in cases if not c[1]]
    lines = [f"опечаток {len(typos)}, правильных {len(keeps)}"]
    # потолок: верный ответ среди кандидатов
    reach = sum(1 for w, t, *_ in typos if w not in d.freq and w not in known and t in d.candidates(w))
    in_dict = sum(1 for w, t, *_ in typos if w in d.freq or w in known)
    lines.append(f"верный ответ среди кандидатов: {reach / len(typos):.1%}; опечатка считается известным словом: {in_dict}")
    for name in sys.argv[1:]:
        net = load_net(DATA / f"{name}.pt")
        res = [(c, choose(net, d, known, c[2], c[0])) for c in cases]
        lines.append(f"== {name}")
        for t in THRESHOLDS:
            right = wrong = false = 0
            for (w, target, _, _), r in res:
                acted = r is not None and r[0] != w and r[1] >= t
                if target:
                    right += acted and r[0].replace("ё", "е") == target
                    wrong += acted and r[0].replace("ё", "е") != target
                else:
                    false += acted
            lines.append(
                f"  порог {t:.2f}: исправлено верно {right / len(typos):.1%}, неверно {wrong / len(typos):.1%}"
                f" | испорчено правильных {false}/{len(keeps)}"
            )
    out = DATA / "real_eval.txt"
    out.write_text("\n".join(lines), encoding="utf-8")
    print(out)
