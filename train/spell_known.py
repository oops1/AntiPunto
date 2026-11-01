"""Фильтр Блума известных слов: словоформы OpenCorpora и частые слова корпусов — не исправлять."""

import math
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import pymorphy3

import paths

MODELS = Path(__file__).resolve().parent.parent / "models"
P = 0.005
MASK = (1 << 64) - 1


def fnv(b: bytes) -> int:
    h = 0xCBF29CE484222325
    for x in b:
        h = ((h ^ x) * 0x100000001B3) & MASK
    return h


def mix(x: int) -> int:
    """splitmix64: второй независимый хеш."""
    x = (x + 0x9E3779B97F4A7C15) & MASK
    x = ((x ^ (x >> 30)) * 0xBF58476D1CE4E5B9) & MASK
    x = ((x ^ (x >> 27)) * 0x94D049BB133111EB) & MASK
    return x ^ (x >> 31)


def norm(w: str) -> bytes:
    return w.lower().replace("ё", "е").encode("utf-8")


if __name__ == "__main__":
    words = {norm(w) for w, *_ in pymorphy3.MorphAnalyzer().dictionary.iter_known_words()}
    for lang in ("ru", "en"):
        seen = (paths.ROOT / "spell" / f"{lang}_seen.txt").read_text(encoding="utf-8").split("\n")
        words.update(norm(w) for w in seen if w)
    n = len(words)
    m = int(-n * math.log(P) / math.log(2) ** 2)
    k = max(1, round(m / n * math.log(2)))
    bits = bytearray((m + 7) // 8)
    for w in words:
        h1 = fnv(w)
        h2 = mix(h1) | 1
        for i in range(k):
            j = ((h1 + i * h2) & MASK) % m
            bits[j >> 3] |= 1 << (j & 7)
    out = MODELS / "spell_known.bloom"
    out.write_bytes(b"APBL" + struct.pack("<IQ", k, m) + bytes(bits))
    print(f"слов {n}, бит {m}, k={k}, {out.stat().st_size / 1e6:.1f} МБ", file=sys.stderr)
