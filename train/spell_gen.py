"""Группы для ранжировщика: контекст, опечатка, K кандидатов, номер верного."""

import argparse
import multiprocessing as mp
import random
import re
import sys
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import numpy as np

import paths
from spell_common import K, L, Dictionary, corrupt, corrupt1, osa, row
from spell_keep import Bloom
from spell_known import MODELS

WORDS = {"ru": re.compile(r"[а-яё]+"), "en": re.compile(r"[a-z]+")}
P_KEEP = 0.35  # доля групп «правильное редкое слово — не трогать»
P_SPLIT = 0.12  # доля опечаток «пропущен пробел»: «вчем» → «в чем»

_dicts: dict[str, Dictionary] = {}
_keep: dict[str, list[str]] = {}
_known: Bloom | None = None


def init(split: str):
    global _known
    _known = Bloom(MODELS / "spell_known.bloom")
    for lang in ("ru", "en"):
        _dicts[lang] = Dictionary(lang)
        # только слова, которые клиент покажет модели (spell_keep.py); тест — на невиданных
        words = (paths.ROOT / "spell" / f"{lang}_keep.txt").read_text(encoding="utf-8").split("\n")
        held = split != "train"
        _keep[lang] = [w for w in words if (zlib.crc32(w.encode()) % 10 == 0) == held]


def merged(d: Dictionary, sent: str, all_spans, rng: random.Random):
    """Два соседних слова без пробела; пробел мог стать буквой, в части — опечатка."""
    pairs = [(x, y) for x, y in zip(all_spans, all_spans[1:]) if sent[x[1] : y[0]] == " "]
    if not pairs:
        return None
    (a, b), (c, e) = rng.choice(pairs)
    w1, w2 = sent[a:b].lower(), sent[c:e].lower()
    if not (d.part_ok(w1) and d.part_ok(w2) and w1 in d.freq and w2 in d.freq):
        return None
    r = rng.random()
    if r < 0.15:
        gap = rng.choice(d.space_keys)
    else:
        gap = ""
        if r < 0.45:
            # опечатка в одной из частей
            if len(w2) >= 3 and (len(w1) < 3 or rng.random() < 0.5):
                return a, w1 + corrupt1(w2, d.lang, rng), f"{w1} {w2}"
            if len(w1) >= 3:
                return a, corrupt1(w1, d.lang, rng) + w2, f"{w1} {w2}"
    return a, w1 + gap + w2, f"{w1} {w2}"


def group(lang: str, sent: str, rng: random.Random):
    d = _dicts[lang]
    all_spans = [(m.start(), m.end()) for m in WORDS[lang].finditer(sent.lower())]
    spans = [s for s in all_spans if s[1] - s[0] >= 3]
    if not spans:
        return None
    a, b = rng.choice(spans)
    ctx = sent[:a]
    word = sent[a:b].lower()
    if rng.random() < P_SPLIT * (1 - P_KEEP):
        m = merged(d, sent, all_spans, rng)
        if m is None:
            return None
        a, typed, target = m
        ctx = sent[:a]
        if len(typed) > 20 or typed in d.freq or typed in _known:
            return None
    elif rng.random() < P_KEEP:
        typed, target = rng.choice(_keep[lang]), None
        if typed in d.freq:
            return None
    else:
        if word not in d.freq:
            return None
        typed, target = corrupt(word, lang, rng), word
        if typed == word or typed in d.freq or typed in _known:
            return None  # клиент известные слова модели не показывает
    cands = d.candidates(typed)
    if target is not None and target not in cands:
        return None
    if not cands:
        return None
    opts = [typed] + cands
    rows = np.zeros((K, L), np.uint8)
    rows[0] = row(ctx, typed, typed)
    for j, c in enumerate(cands, 1):
        rows[j] = row(ctx, typed, c, j, osa(typed, c), d.freq_of(c))
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
    # книги и примеры словаря (spell_lit.py): мало, но чистое согласование — повторяем
    lit = paths.CORPUS / f"lit_{lang}.{split}.txt"
    if lit.exists():
        out += lit.read_text(encoding="utf-8").splitlines() * 3
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
    with mp.Pool(a.procs, initializer=init, initargs=(a.split,)) as pool:
        parts = pool.map(work, jobs)
    X, M, Y = (np.concatenate([p[i] for p in parts]) for i in range(3))
    np.savez(paths.ROOT / "spell" / f"{a.split}.npz", x=X, m=M, y=Y)
    print(f"{a.split}: групп {len(X)}, «оставить» {(Y == 0).mean():.2f}, кандидатов в среднем {M.sum(1).mean():.1f}")
