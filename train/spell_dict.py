"""Частотные словари для орфографии: слова из корпусов, по языку."""

import re
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import paths

WORD = {"ru": re.compile(r"[а-яё]+"), "en": re.compile(r"[a-z]+")}
TOP = {"ru": 300_000, "en": 150_000}
OUT = paths.ROOT / "spell"


def build(lang: str, limit: int = 3_000_000) -> None:
    cnt, wiki = Counter(), Counter()
    for name in (f"{lang}.train.txt", f"chat_{lang}.train.txt"):
        with open(paths.CORPUS / name, encoding="utf-8") as f:
            for i, line in enumerate(f):
                if i >= limit:
                    break
                ws = WORD[lang].findall(line.lower())
                cnt.update(ws)
                if not name.startswith("chat_"):
                    wiki.update(ws)
    known = (lambda w: True)
    if lang == "ru":
        import pymorphy3

        morph = pymorphy3.MorphAnalyzer()
        known = morph.word_is_known
    # в чатах частые опечатки («хотябы», «коментарий») — в словарь только проверенное:
    # знает OpenCorpora, есть в Википедии или очень частое
    ok = lambda w, c: known(w) if lang == "ru" else (wiki[w] >= 3 or c >= 200)
    words = [(w, c) for w, c in cnt.most_common() if c >= 3 and len(w) <= 24 and (ok(w, c) or wiki[w] >= 3)]
    words = words[: TOP[lang]]
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / f"{lang}.tsv").write_text("".join(f"{w}\t{c}\n" for w, c in words), encoding="utf-8")
    # встречалось ≥5 раз, но не в словаре (жаргон, неологизмы): не исправлять
    in_dict = {w for w, _ in words}
    seen = [w for w, c in cnt.items() if c >= 5 and w not in in_dict]
    (OUT / f"{lang}_seen.txt").write_text("\n".join(seen), encoding="utf-8")
    # редкие правильные слова вне словаря: учим их не трогать.
    # В чатах среди редких много опечаток — берём только проверенные.
    rare = [w for w, c in cnt.items() if c in (1, 2) and 4 <= len(w) <= 16 and wiki[w]]
    if lang == "ru":
        rare = [w for w in rare if known(w)]
    (OUT / f"{lang}_rare.txt").write_text("\n".join(rare), encoding="utf-8")
    print(f"{lang}: словарь {len(words)}, редких проверенных {len(rare)}", flush=True)


if __name__ == "__main__":
    for lang in ("ru", "en"):
        build(lang)
