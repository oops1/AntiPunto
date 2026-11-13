"""Орфография: словарь, кандидаты (до 2 правок: варианты в 1 правку + SymSpell), опечатки, кодирование."""

import math
import random
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import numpy as np

import paths
from layouts import ROWS

K = 16  # кандидатов в группе; 0-й — «оставить как есть»
SPLITS = 3  # разбивок слитного набора среди кандидатов, не больше
SHORT_FREQ = 8000  # часть разбивки до 3 букв — только частое слово («ко» да, «ка» нет)
SHORT_FREQ_LOOSE = 1000  # ...а если целое не слово вовсе — порог мягче («до сих»)
CTX, WLEN, RCTX = 40, 20, 20
L = CTX + 2 * WLEN + RCTX + 3 + 3  # контекст, SEP, набранное, SEP, кандидат, SEP, справа, 3 признака
R0 = CTX + 2 * WLEN + 3  # начало правого контекста
ALPHABET = {"ru": "абвгдеёжзийклмнопрстуфхцчшщъыьэюя", "en": "abcdefghijklmnopqrstuvwxyz"}
# однобуквенные слова, которые могут стоять отдельно при разбивке слитного набора
SINGLE = {"ru": "авикосуя", "en": "ai"}
# клавиши над пробелом: задели вместо него («приветькак»)
SPACE_KEYS = {"ru": "чсмитьб", "en": "xcvbnm"}
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


def row(ctx: str, typed: str, cand: str, rank: int = 0, dist: int = 0, freq: int = 0, right: str = "") -> np.ndarray:
    """Контекст слева прижат к SEP; слова и контекст справа — с начала своих полей; в конце признаки."""
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
    out[R0 - 1] = SEP
    r = encode(right.lower(), RCTX)
    out[R0 : R0 + len(r)] = r
    return out


def widen(x: np.ndarray) -> np.ndarray:
    """Группы старого формата (до правого контекста, L = 85) → текущая разметка."""
    if x.shape[-1] == L:
        return x
    old = x.shape[-1]
    out = np.zeros(x.shape[:-1] + (L,), x.dtype)
    out[..., : old - 3] = x[..., : old - 3]
    out[..., old - 3] = SEP
    out[..., L - 3 :] = x[..., old - 3 :]
    return out


class Dictionary:
    def __init__(self, lang: str, top: int | None = None):
        rows = [l.split("\t") for l in (paths.ROOT / "spell" / f"{lang}.tsv").read_text(encoding="utf-8").splitlines()]
        rows = rows[:top] if top else rows
        self.lang = lang
        self.alpha = ALPHABET[lang]
        self.single = SINGLE[lang]
        self.space_keys = SPACE_KEYS[lang]
        self.words = [w for w, _ in rows]
        self.freq = {w: int(c) for w, c in rows}
        self.deletes: dict[str, list[int]] = {}
        for i, w in enumerate(self.words):
            for d in deletes1(w):
                self.deletes.setdefault(d, []).append(i)

    def near(self, v: str, found: set[str]) -> None:
        """Словарные слова в одной правке от v (удаление с каждой стороны)."""
        for k in (v, *deletes1(v)):
            if k in self.freq:
                found.add(k)
            for i in self.deletes.get(k, ()):
                found.add(self.words[i])

    def freq_of(self, w: str) -> int:
        """Частота слова; у разбивки «в чем» — меньшая из частей."""
        return min(self.freq.get(p, 0) for p in w.split(" "))

    def part_ok(self, w: str, loose: bool = False) -> bool:
        if len(w) == 1:
            return w in self.single
        return len(w) > 3 or self.freq.get(w, 0) >= (SHORT_FREQ_LOOSE if loose else SHORT_FREQ)

    def part(self, p: str, loose: bool = False) -> list[tuple[str, int]]:
        """Часть разбивки: само слово (0 правок) или два самых частых в одной правке."""
        if p in self.freq:
            return [(p, 0)] if self.part_ok(p, loose) else []
        if len(p) < 3:
            return []
        found: set[str] = set()
        self.near(p, found)
        best = sorted((-self.freq[w], w) for w in found if len(w) > 1 and self.part_ok(w, loose) and osa(p, w) == 1)
        return [(w, 1) for _, w in best[:2]]

    def splits(self, typed: str) -> set[str]:
        """Пропущенный пробел: «вчем» → «в чем»; пробел мог стать соседней буквой («приветькак»).
        Целое не слово — короткие части пропускаем с мягким порогом частоты."""
        loose = typed not in self.freq
        out = set()
        for i in range(1, len(typed)):
            for skip in (0, 1):
                if skip and (i + 1 >= len(typed) or typed[i] not in self.space_keys):
                    continue
                rights = self.part(typed[i + skip :], loose)
                for lw, le in self.part(typed[:i], loose) if rights else ():
                    for rw, re_ in rights:
                        if le + re_ + skip <= 2:
                            out.add(f"{lw} {rw}")
        return out

    def candidates(self, typed: str) -> list[str]:
        """Словарные слова в пределах 2 правок и до SPLITS разбивок на два слова, лучшие первыми."""
        found: set[str] = set()
        self.near(typed, found)
        # вторая правка: от каждого варианта набранного в одну правку
        for v in edits1(typed, self.alpha):
            self.near(v, found)
        found.discard(typed)
        if typed in self.freq:
            # словарное слово: вариант, отличный только буквой ё, — не правка
            plain = typed.replace("ё", "е")
            found = {w for w in found if w.replace("ё", "е") != plain}
        words = sorted((osa(typed, w), -self.freq[w], w) for w in found)
        words = [s for s in words if s[0] <= 2]
        # разбивка — это ещё и вставленный пробел: ей допускаем на правку больше
        parts = sorted((osa(typed, w), -self.freq_of(w), w) for w in self.splits(typed))
        parts = [s for s in parts if s[0] <= 3][:SPLITS]
        keep = words[: K - 1 - len(parts)] + parts
        return [w for _, _, w in sorted(keep)]


def deletes1(w: str) -> set[str]:
    return {w[:i] + w[i + 1 :] for i in range(len(w))}


def edits1(w: str, alpha: str) -> set[str]:
    """Все строки в одной правке: удаление, перестановка, замена, вставка."""
    out = deletes1(w)
    out.update(w[:i] + w[i + 1] + w[i] + w[i + 2 :] for i in range(len(w) - 1))
    for i in range(len(w) + 1):
        for c in alpha:
            out.add(w[:i] + c + w[i:])
            if i < len(w):
                out.add(w[:i] + c + w[i + 1 :])
    out.discard(w)
    return out


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
    """Правдоподобная опечатка; в длинных словах бывает вторая и третья."""
    out = corrupt1(w, lang, rng)
    if len(w) >= 6 and rng.random() < 0.35:
        out = corrupt1(out, lang, rng)
        if len(w) >= 9 and rng.random() < 0.15:
            out = corrupt1(out, lang, rng)
    return out


def _pos(w: str, rng: random.Random) -> int:
    """Место правки; первая буква вдвое реже (реально 7 % против 13 %)."""
    i = rng.randrange(len(w))
    return rng.randrange(1, len(w)) if i == 0 and rng.random() < 0.5 else i


def corrupt1(w: str, lang: str, rng: random.Random) -> str:
    """Одна опечатка; доли — по 977 реальным (error_stats.txt)."""
    if len(w) < 3:
        return w
    ru = lang == "ru"
    i = _pos(w, rng)
    r = rng.random()
    if ru and r < 0.3 and w.endswith(("ться", "тся")):
        return w[:-4] + "тся" if w.endswith("ться") else w[:-3] + "ться"
    if r < 0.20 and NEAR.get(w[i]):
        # задета соседняя клавиша вдобавок к нужной («продолджай»)
        ins = rng.choice(NEAR[w[i]])
        j = i + rng.randrange(2)
        if j == 0 and ins in SINGLE[lang]:
            j = 1  # «в»+«чем» — это пропущенный пробел
        return w[:j] + ins + w[j:]
    if r < 0.37:
        j = min(i, len(w) - 2)
        return w[:j] + w[j + 1] + w[j] + w[j + 2 :]
    if r < 0.47 and NEAR.get(w[i]):
        return w[:i] + rng.choice(NEAR[w[i]]) + w[i + 1 :]
    if r < 0.52:
        # лишняя буква из соседних («скачжи», «подоумать») или ь в конце («задачь»)
        if ru and rng.random() < 0.2:
            return w + "ь"
        near = [c for c in w[max(0, i - 2) : i + 3] if c != w[i] and (i == 0 or c != w[i - 1])]
        return w[:i] + rng.choice(near or ALPHABET[lang]) + w[i:]
    if r < 0.55:
        return w[:i] + w[i] + w[i:]
    if r < 0.59 and ru:
        vs = [j for j, c in enumerate(w) if c in VOWELS]
        if vs:
            j = rng.choice(vs)
            return w[:j] + VOWELS[w[j]] + w[j + 1 :]
    if r < 0.62:
        return w[:i] + rng.choice(ALPHABET[lang]) + w[i + 1 :]
    if r < 0.64 and len(w) >= 6:
        return w[:-2]  # не дожатое окончание («заголов», «рабоат»)
    # пропуск; удвоенная буква, ь/ъ и й — чаще («сесии», «ждеш», «интерфес»)
    doubles = [j for j in range(len(w) - 1) if w[j] == w[j + 1]]
    soft = [j for j, c in enumerate(w) if c in "ьъй"] if ru else []
    r2 = rng.random()
    if doubles and r2 < 0.8:
        i = rng.choice(doubles)
    elif soft and r2 < 0.9:
        i = rng.choice(soft)
    return w[:i] + w[i + 1 :]
