"""Группы «слово спрятано»: выбрать верное слово среди похожих только по контексту.

Набранное пусто, кандидаты — слово из текста и словарные соседи в двух правках (формы, похожие слова).
Строка 0 («оставить») выключена маской. Формат строк тот же, что у spell_gen.py.
"""

import argparse
import multiprocessing as mp
import random
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import numpy as np

import paths
from spell_common import K, L, RCTX, Dictionary, row
from spell_gen import NEXT, WORDS, load

P_NO_RIGHT, P_NEXT = 0.2, 0.4  # без правого контекста и только следующее слово
MIN_OPTS = 3

_dicts: dict[str, Dictionary] = {}
_near: dict[tuple[str, str], list[str]] = {}


def init():
    for lang in ("ru", "en"):
        _dicts[lang] = Dictionary(lang)


def options(lang: str, word: str) -> list[str]:
    """Слово и его соседи, частые первыми."""
    key = (lang, word)
    if key not in _near:
        d = _dicts[lang]
        near = [c for c in d.candidates(word) if " " not in c][: K - 2]
        _near[key] = sorted([word] + near, key=lambda w: (-d.freq[w], w))
    return _near[key]


def group(lang: str, sent: str, rng: random.Random):
    d = _dicts[lang]
    spans = [(m.start(), m.end()) for m in WORDS[lang].finditer(sent.lower())]
    spans = [(s, e) for s, e in spans if 3 <= e - s <= 20 and sent[s:e].lower() in d.freq]
    if not spans:
        return None
    a, b = rng.choices(spans, [e - s - 2 for s, e in spans])[0]
    word = sent[a:b].lower()
    opts = options(lang, word)
    if len(opts) < MIN_OPTS:
        return None
    r = rng.random()
    right = "" if r < P_NO_RIGHT else sent[b : b + RCTX]
    if P_NO_RIGHT <= r < P_NO_RIGHT + P_NEXT:
        m = NEXT.match(right)
        right = m.group() if m else right
    rows = np.zeros((K, L), np.uint8)
    for j, c in enumerate(opts, 1):
        rows[j] = row(sent[:a], "", c, j, 0, d.freq[c], right)
    mask = np.zeros(K, np.uint8)
    mask[1 : len(opts) + 1] = 1
    return rows, mask, opts.index(word) + 1


def work(job):
    seed, items = job
    rng = random.Random(seed)
    X, M, Y = [], [], []
    for lang, sent in items:
        g = group(lang, sent, rng)
        if g:
            X.append(g[0])
            M.append(g[1])
            Y.append(g[2])
    return np.stack(X), np.stack(M), np.array(Y, np.int16)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--split", default="train")
    ap.add_argument("--n", type=int, default=2_600_000, help="попыток; групп выйдет меньше")
    ap.add_argument("--procs", type=int, default=14)
    a = ap.parse_args()
    rng = random.Random(21 if a.split == "train" else 22)
    sents = {lang: load(a.split, lang) for lang in ("ru", "en")}
    jobs = []
    for c in range(0, a.n, 20000):
        langs = ["ru" if rng.random() < 0.7 else "en" for _ in range(min(20000, a.n - c))]
        jobs.append((rng.getrandbits(32), [(l, rng.choice(sents[l])) for l in langs]))
    with mp.Pool(a.procs, initializer=init) as pool:
        parts = pool.map(work, jobs)
    X, M, Y = (np.concatenate([p[i] for p in parts]) for i in range(3))
    name = f"ctx_{a.split}.npz"
    np.savez(paths.ROOT / "spell" / name, x=X, m=M, y=Y)
    first = (Y == 1).mean()
    print(f"{name}: групп {len(X)}, кандидатов в среднем {M.sum(1).mean():.1f}, самый частый верен в {first:.3f}")
