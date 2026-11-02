"""Полные парадигмы: недостающие формы слов словаря из OpenCorpora («подключаюсь», «отредактируй»)."""

import sys
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import pymorphy3

import paths

SKIP = {"PRTF"}  # полные причастия: по 24 формы, в чатах редки


def add(path: Path = paths.ROOT / "spell" / "ru.tsv") -> int:
    morph = pymorphy3.MorphAnalyzer()
    rows = [l.split("\t") for l in path.read_text(encoding="utf-8").splitlines()]
    have = {w: int(c) for w, c in rows}
    total, seen_forms, lexemes = defaultdict(int), defaultdict(int), {}
    for w, c in have.items():
        p = morph.parse(w)[0]
        if not p.is_known:
            continue
        key = (p.normal_form, p.tag.POS)
        total[key] += c
        seen_forms[key] += 1
        lexemes.setdefault(key, p)
    new = {}
    for key, p in lexemes.items():
        # частота новой формы — средняя по известным формам слова
        freq = max(1, total[key] // max(1, seen_forms[key]) // 4)
        for f in p.lexeme:
            w = f.word
            if f.tag.POS in SKIP or w.replace("ё", "е") in have or w in have or not 3 <= len(w) <= 24:
                continue
            new[w] = max(new.get(w, 0), freq)
    path.write_text("".join(f"{w}\t{c}\n" for w, c in [*have.items(), *sorted(new.items())]), encoding="utf-8")
    return len(new)


if __name__ == "__main__":
    print(f"добавлено форм: {add()}", file=sys.stderr)
