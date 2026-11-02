"""Правильные слова, которые клиент покажет модели: редкие слова Википедии вне словаря и фильтра."""

import re
import struct
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import paths
from spell_common import Dictionary
from spell_known import MASK, MODELS, fnv, mix, norm

WORDS = {"ru": re.compile(r"[а-яё]+"), "en": re.compile(r"[a-z]+")}
MIN_FREQ = 2  # единичные чаще опечатки


class Bloom:
    def __init__(self, path: Path):
        raw = path.read_bytes()
        self.k, self.m = struct.unpack("<IQ", raw[4:16])
        self.bits = raw[16:]

    def __contains__(self, w: str) -> bool:
        h1 = fnv(norm(w))
        h2 = mix(h1) | 1
        for i in range(self.k):
            j = ((h1 + i * h2) & MASK) % self.m
            if not self.bits[j >> 3] >> (j & 7) & 1:
                return False
        return True


if __name__ == "__main__":
    known = Bloom(MODELS / "spell_known.bloom")
    for lang in ("ru", "en"):
        d = Dictionary(lang)
        cnt = Counter()
        with open(paths.CORPUS / f"{lang}.train.txt", encoding="utf-8") as f:
            for line in f:
                cnt.update(WORDS[lang].findall(line.lower()))
        keep = sorted(
            w
            for w, n in cnt.items()
            if n >= MIN_FREQ and 3 <= len(w) <= 20 and w not in d.freq and w not in known
        )
        (paths.ROOT / "spell" / f"{lang}_keep.txt").write_text("\n".join(keep), encoding="utf-8")
        print(f"{lang}: слов {len(cnt)}, «не трогать» {len(keep)}", file=sys.stderr)
