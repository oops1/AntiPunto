"""Жаргон в словарь орфографии: формы лемм из jargon_ru.txt дописываются в ru.tsv."""

import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import pymorphy3

import paths

FREQ = 300  # как у обиходного слова средней частоты
SRC = Path(__file__).parent / "jargon_ru.txt"


def forms(morph, lemma: str, pos: str, like: str | None) -> set[str]:
    if like:
        p = next((p for p in morph.parse(like) if p.tag.POS == pos), None)
        if p is None:
            return {lemma}  # образец несклоняемый
        words = [f.word for f in p.lexeme]
        stem = os.path.commonprefix(words)
        base = lemma[: len(lemma) - (len(like) - len(stem))]
        return {base + w[len(stem) :] for w in words}
    p = next((p for p in morph.parse(lemma) if p.tag.POS == pos), None)
    return {f.word for f in p.lexeme} if p else {lemma}


def words() -> set[str]:
    """Все формы жаргона."""
    morph = pymorphy3.MorphAnalyzer()
    out: set[str] = set()
    for line in SRC.read_text(encoding="utf-8").splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        lemma, pos, *like = line.split()
        out |= forms(morph, lemma, pos, like[0] if like else None)
    return out


def add(path: Path = paths.ROOT / "spell" / "ru.tsv") -> int:
    rows = path.read_text(encoding="utf-8").splitlines()
    new = words()
    # жаргон, бывший в словаре с малой частотой корпуса: не реже FREQ, иначе модель сочтёт его ошибкой
    plain = {w.replace("ё", "е") for w in new}
    for i, r in enumerate(rows):
        w, c = r.split("\t")
        if w.replace("ё", "е") in plain and int(c) < FREQ:
            rows[i] = f"{w}\t{FREQ}"
    have = {r.split("\t")[0] for r in rows}
    new = sorted(w.replace("ё", "е") if w.replace("ё", "е") in have else w for w in new - have)
    new = [w for w in new if w not in have]
    path.write_text("\n".join(rows + [f"{w}\t{FREQ}" for w in new]) + "\n", encoding="utf-8")
    return len(new)


if __name__ == "__main__":
    print(f"добавлено форм: {add()}", file=sys.stderr)
