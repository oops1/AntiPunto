"""Группы для ранжировщика: контекст, опечатка, K кандидатов, номер верного."""

import argparse
import multiprocessing as mp
import random
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import numpy as np

import paths
from spell_common import K, L, Dictionary, corrupt, osa, row

WORDS = {"ru": re.compile(r"[а-яё]+"), "en": re.compile(r"[a-z]+")}
P_KEEP = 0.35  # доля групп «правильное редкое слово — не трогать»

_dicts: dict[str, Dictionary] = {}
_rare: dict[str, list[str]] = {}


def init():
    for lang in ("ru", "en"):
        _dicts[lang] = Dictionary(lang)
        _rare[lang] = (paths.ROOT / "spell" / f"{lang}_rare.txt").read_text(encoding="utf-8").split("\n")


def group(lang: str, sent: str, rng: random.Random):
    d = _dicts[lang]
    spans = [(m.start(), m.end()) for m in WORDS[lang].finditer(sent.lower())]
    spans = [s for s in spans if s[1] - s[0] >= 3]
    if not spans:
        return None
    a, b = rng.choice(spans)
    ctx = sent[:a]
    word = sent[a:b].lower()
    if rng.random() < P_KEEP:
        typed, target = rng.choice(_rare[lang]), None
        if typed in d.freq:
            return None
    else:
        if word not in d.freq:
            return None
        typed, target = corrupt(word, lang, rng), word
        if typed == word or typed in d.freq:
            return None  # пока только «не слова»
    cands = d.candidates(typed)
    if target is not None and target not in cands:
        return None
    if not cands:
        return None
    opts = [typed] + cands
    rows = np.zeros((K, L), np.uint8)
    rows[0] = row(ctx, typed, typed)
    for j, c in enumerate(cands, 1):
        rows[j] = row(ctx, typed, c, j, osa(typed, c), d.freq[c])
    mask = np.zeros(K, np.uint8)
    mask[: len(opts)] = 1
    return rows, mask, 0 if target is None else opts.index(target)


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


def load(split: str, lang: str) -> list[str]:
    out = []
    for name in (f"{lang}.{split}.txt", f"chat_{lang}.{split}.txt"):
        out += (paths.CORPUS / name).read_text(encoding="utf-8").splitlines()[:1_500_000]
    return out


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--split", default="train")
    ap.add_argument("--n", type=int, default=900_000)
    ap.add_argument("--procs", type=int, default=10)
    a = ap.parse_args()
    rng = random.Random(1 if a.split == "train" else 2)
    sents = {lang: load(a.split, lang) for lang in ("ru", "en")}
    jobs = []
    for c in range(0, a.n, 20000):
        items = [("ru" if rng.random() < 0.7 else "en",) for _ in range(min(20000, a.n - c))]
        items = [(l[0], rng.choice(sents[l[0]])) for l in items]
        jobs.append((rng.getrandbits(32), items))
    with mp.Pool(a.procs, initializer=init) as pool:
        parts = pool.map(work, jobs)
    X, M, Y = (np.concatenate([p[i] for p in parts]) for i in range(3))
    np.savez(paths.ROOT / "spell" / f"{a.split}.npz", x=X, m=M, y=Y)
    print(f"{a.split}: групп {len(X)}, «оставить» {(Y == 0).mean():.2f}, кандидатов в среднем {M.sum(1).mean():.1f}")
