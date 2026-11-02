"""Орфография: словарь, кандидаты (SymSpell, 1 удаление с каждой стороны), опечатки, кодирование."""

import math
import random
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import numpy as np

import paths
from layouts import ROWS

K = 16  # кандидатов в группе; 0-й — «оставить как есть»
CTX, WLEN = 40, 20
L = CTX + 2 * WLEN + 2 + 3  # контекст, SEP, набранное, SEP, кандидат, 3 признака
ALPHABET = {"ru": "абвгдеёжзийклмнопрстуфхцчшщъыьэюя", "en": "abcdefghijklmnopqrstuvwxyz"}
CHARS = " " + ALPHABET["ru"] + ALPHABET["en"] + "0123456789.,!?;:-'\"()"
PAD, SEP, UNK = 0, 1, 2
CHAR_ID = {c: i + 3 for i, c in enumerate(CHARS)}
# признаки кандидата: место в списке, расстояние правки, корзина частоты
F_RANK = len(CHARS) + 3
F_DIST = F_RANK + K
F_FREQ = F_DIST + 3
VOCAB = F_FREQ + 16


def freq_bucket(f: int) -> int:
    """0 — нет в словаре; дальше по логарифму частоты."""
    return 0 if f <= 0 else min(15, 1 + int(math.log2(f) / 1.5))


def encode(s: str, n: int) -> list[int]:
    return [CHAR_ID.get(c, UNK) for c in s[:n]]


def row(ctx: str, typed: str, cand: str, rank: int = 0, dist: int = 0, freq: int = 0) -> np.ndarray:
    """Контекст слева прижат к SEP; слова — с начала своих полей; в конце признаки."""
    out = np.zeros(L, np.uint8)
    out[L - 3] = F_RANK + rank
    out[L - 2] = F_DIST + min(dist, 2)
    out[L - 1] = F_FREQ + freq_bucket(freq)
    c = encode(ctx.lower()[-CTX:], CTX)
    out[CTX - len(c) : CTX] = c
    out[CTX] = SEP
    t = encode(typed, WLEN)
    out[CTX + 1 : CTX + 1 + len(t)] = t
    out[CTX + 1 + WLEN] = SEP
    k = encode(cand, WLEN)
    out[CTX + 2 + WLEN : CTX + 2 + WLEN + len(k)] = k
    return out


class Dictionary:
    def __init__(self, lang: str, top: int | None = None):
        rows = [l.split("\t") for l in (paths.ROOT / "spell" / f"{lang}.tsv").read_text(encoding="utf-8").splitlines()]
        rows = rows[:top] if top else rows
        self.words = [w for w, _ in rows]
        self.freq = {w: int(c) for w, c in rows}
        self.deletes: dict[str, list[int]] = {}
        for i, w in enumerate(self.words):
            for d in deletes1(w):
                self.deletes.setdefault(d, []).append(i)

    def candidates(self, typed: str) -> list[str]:
        """Словарные слова в пределах ~2 правок, лучшие первыми."""
        found = set()
        d1 = deletes1(typed)
        for k in {typed} | d1:
            if k in self.freq:
                found.add(k)
            for i in self.deletes.get(k, ()):
                found.add(self.words[i])
        # две лишние буквы в набранном («черерез» → «через»)
        for k in d1:
            found.update(x for x in deletes1(k) if x in self.freq)
        found.discard(typed)
        scored = [(osa(typed, w), -self.freq[w], w) for w in found]
        return [w for d, _, w in sorted(scored) if d <= 2][: K - 1]


def deletes1(w: str) -> set[str]:
    return {w[:i] + w[i + 1 :] for i in range(len(w))}


def osa(a: str, b: str) -> int:
    """Расстояние Дамерау–Левенштейна (оптимальное выравнивание строк)."""
    d = [[0] * (len(b) + 1) for _ in range(len(a) + 1)]
    for i in range(len(a) + 1):
        d[i][0] = i
    for j in range(len(b) + 1):
        d[0][j] = j
    for i in range(1, len(a) + 1):
        for j in range(1, len(b) + 1):
            c = a[i - 1] != b[j - 1]
            d[i][j] = min(d[i - 1][j] + 1, d[i][j - 1] + 1, d[i - 1][j - 1] + c)
            if i > 1 and j > 1 and a[i - 1] == b[j - 2] and a[i - 2] == b[j - 1]:
                d[i][j] = min(d[i][j], d[i - 2][j - 2] + 1)
    return d[-1][-1]


def _neighbors() -> dict[str, str]:
    """Соседние клавиши по рядам обеих раскладок."""
    grid = {"ru": [r[2] for r in ROWS[1:]], "en": [r[0] for r in ROWS[1:]]}
    out: dict[str, str] = {}
    for rows in grid.values():
        for y, r in enumerate(rows):
            for x, ch in enumerate(r):
                near = []
                for dy in (-1, 0, 1):
                    if 0 <= y + dy < len(rows):
                        rr = rows[y + dy]
                        for dx in (-1, 0, 1):
                            if (dy or dx) and 0 <= x + dx < len(rr):
                                near.append(rr[x + dx])
                out[ch] = "".join(c for c in near if c.isalpha())
    return out


NEAR = _neighbors()
VOWELS = {"о": "а", "а": "о", "е": "и", "и": "е", "я": "е", "э": "е", "ы": "и"}


def corrupt(w: str, lang: str, rng: random.Random) -> str:
    """Одна правдоподобная опечатка."""
    if len(w) < 3:
        return w
    i = rng.randrange(len(w))
    r = rng.random()
    if r < 0.35 and NEAR.get(w[i]):
        return w[:i] + rng.choice(NEAR[w[i]]) + w[i + 1 :]
    if r < 0.55:
        return w[:i] + w[i + 1 :]
    if r < 0.70:
        ins = rng.choice(NEAR.get(w[i]) or w[i]) if rng.random() < 0.6 else w[i]
        return w[:i] + ins + w[i:]
    if r < 0.85 and i < len(w) - 1:
        return w[:i] + w[i + 1] + w[i] + w[i + 2 :]
    if lang == "ru" and r < 0.95:
        vs = [j for j, c in enumerate(w) if c in VOWELS]
        if vs:
            j = rng.choice(vs)
            return w[:j] + VOWELS[w[j]] + w[j + 1 :]
    if lang == "ru" and w.endswith("ться"):
        return w[:-4] + "тся"
    if lang == "ru" and w.endswith("тся"):
        return w[:-3] + "ться"
    return w[:i] + w[i + 1 :]
