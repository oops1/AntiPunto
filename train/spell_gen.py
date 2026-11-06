"""Группы для ранжировщика: контекст, опечатка, K кандидатов, номер верного.

--kind typo (по умолчанию): набранное — вне словаря (опечатка, слитное или редкое правильное слово).
--kind dict: набранное — словарное слово; ошибка, совпавшая с другим словом («бела» → «была»),
или правильное слово, которое надо оставить. У строки 0 — частота набранного.
Контекст справа: пусто (первая проверка в клиенте), следующее слово (перепроверка) или RCTX символов.
"""

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
from spell_common import K, L, RCTX, Dictionary, corrupt, corrupt1, osa, row
from spell_keep import Bloom
from spell_known import MODELS

WORDS = {"ru": re.compile(r"[а-яё]+"), "en": re.compile(r"[a-z]+")}
P_KEEP = 0.35  # доля групп «правильное редкое слово — не трогать»
P_SPLIT = 0.2  # доля опечаток «пропущен пробел»: «вчем» → «в чем»
P_RW = 0.4  # словарный режим: доля попыток сделать ошибку, совпавшую со словом
P_NO_RIGHT, P_NEXT = 0.35, 0.45  # доли групп без правого контекста и только со следующим словом
NEXT = re.compile(r"\W*\w+\W?")

_dicts: dict[str, Dictionary] = {}
_keep: dict[str, list[str]] = {}
_known: Bloom | None = None
_kind = "typo"


def init(split: str, kind: str = "typo"):
    global _known, _kind
    _kind = kind
    _known = Bloom(MODELS / "spell_known.bloom")
    for lang in ("ru", "en"):
        _dicts[lang] = Dictionary(lang)
        # только слова, которые клиент покажет модели (spell_keep.py); тест — на невиданных
        words = (paths.ROOT / "spell" / f"{lang}_keep.txt").read_text(encoding="utf-8").split("\n")
        held = split != "train"
        _keep[lang] = [w for w in words if (zlib.crc32(w.encode()) % 10 == 0) == held]


def right_ctx(sent: str, b: int, rng: random.Random) -> str:
    """Текст справа от слова, как увидит клиент: ничего, следующее слово с разделителем или всё."""
    r = rng.random()
    if r < P_NO_RIGHT:
        return ""
    tail = sent[b : b + RCTX]
    if r < P_NO_RIGHT + P_NEXT:
        m = NEXT.match(tail)
        return m.group() if m else tail
    return tail


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
                return a, e, w1 + corrupt1(w2, d.lang, rng), f"{w1} {w2}"
            if len(w1) >= 3:
                return a, e, corrupt1(w1, d.lang, rng) + w2, f"{w1} {w2}"
    return a, e, w1 + gap + w2, f"{w1} {w2}"


def group(lang: str, sent: str, rng: random.Random):
    d = _dicts[lang]
    all_spans = [(m.start(), m.end()) for m in WORDS[lang].finditer(sent.lower())]
    spans = [s for s in all_spans if s[1] - s[0] >= 3]
    if not spans:
        return None
    # длинные слова опечатываются чаще (реально половина опечаток в словах 9–12 букв)
    a, b = rng.choices(spans, [e - s - 2 for s, e in spans])[0]
    ctx = sent[:a]
    word = sent[a:b].lower()
    if rng.random() < P_SPLIT * (1 - P_KEEP):
        m = merged(d, sent, all_spans, rng)
        if m is None:
            return None
        a, b, typed, target = m
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
    right = right_ctx(sent, b, rng)
    rows = np.zeros((K, L), np.uint8)
    rows[0] = row(ctx, typed, typed, right=right)
    for j, c in enumerate(cands, 1):
        rows[j] = row(ctx, typed, c, j, osa(typed, c), d.freq_of(c), right)
    mask = np.zeros(K, np.uint8)
    mask[: len(opts)] = 1
    return rows, mask, 0 if target is None else opts.index(target)


def group_dict(lang: str, sent: str, rng: random.Random):
    """Словарное слово из текста: ошибка, ставшая другим словом, или «оставить»."""
    d = _dicts[lang]
    spans = [(m.start(), m.end()) for m in WORDS[lang].finditer(sent.lower())]
    spans = [(s, e) for s, e in spans if e - s >= 3 and sent[s:e].lower() in d.freq]
    if not spans:
        return None
    a, b = rng.choice(spans)
    ctx, word = sent[:a], sent[a:b].lower()
    typed, target = word, None
    if rng.random() < P_RW:
        for _ in range(4):
            typed = corrupt(word, lang, rng)
            if typed != word and typed in d.freq:
                break
        else:
            return None
        target = word
    if not 3 <= len(typed) <= 20:
        return None  # клиент проверяет слова от 3 букв
    cands = d.candidates(typed)
    if not cands or (target is not None and target not in cands):
        return None
    right = right_ctx(sent, b, rng)
    rows = np.zeros((K, L), np.uint8)
    rows[0] = row(ctx, typed, typed, 0, 0, d.freq[typed], right)
    for j, c in enumerate(cands, 1):
        rows[j] = row(ctx, typed, c, j, osa(typed, c), d.freq_of(c), right)
    mask = np.zeros(K, np.uint8)
    mask[: len(cands) + 1] = 1
    return rows, mask, 0 if target is None else cands.index(target) + 1


def work(job):
    seed, items = job
    rng = random.Random(seed)
    X, M, Y = [], [], []
    make = group_dict if _kind == "dict" else group
    for lang, sent in items:
        g = make(lang, sent, rng)
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
    ap.add_argument("--kind", default="typo", choices=("typo", "dict"))
    a = ap.parse_args()
    rng = random.Random((1 if a.split == "train" else 2) + (10 if a.kind == "dict" else 0))
    sents = {lang: load(a.split, lang) for lang in ("ru", "en")}
    jobs = []
    for c in range(0, a.n, 20000):
        items = [("ru" if rng.random() < 0.7 else "en",) for _ in range(min(20000, a.n - c))]
        items = [(l[0], rng.choice(sents[l[0]])) for l in items]
        jobs.append((rng.getrandbits(32), items))
    with mp.Pool(a.procs, initializer=init, initargs=(a.split, a.kind)) as pool:
        parts = pool.map(work, jobs)
    X, M, Y = (np.concatenate([p[i] for p in parts]) for i in range(3))
    name = f"{a.split}.npz" if a.kind == "typo" else f"dict_{a.split}.npz"
    np.savez(paths.ROOT / "spell" / name, x=X, m=M, y=Y)
    print(f"{name}: групп {len(X)}, «оставить» {(Y == 0).mean():.2f}, кандидатов в среднем {M.sum(1).mean():.1f}")
