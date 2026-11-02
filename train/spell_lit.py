"""Литературный корпус для орфографии: предложения из книг и примеров словаря (данные — не в репозитории).

python spell_lit.py <папка .txt> [<папка .txt> ...] → corpus/lit_ru.{train,valid,test}.txt
Кодировка файлов определяется сама (UTF-8 или CP1251).
"""

import re
import sys
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import paths

SENT = re.compile(r"(?<=[.!?…])(?<![\s(][А-ЯЁ]\.)\s+(?=[«\"—–-]?\s*[А-ЯЁ])")
CITE = re.compile(r"\((?:[А-ЯЁ]\.\s*){1,2}[А-ЯЁ][а-яё-]+\)|\(«[^»]*»,?\s*\d{4}\)|\([a-z.]+\.[a-z]+,\s*\d{4}\)")
MARKUP = re.compile(r"[А-ЯЁ]{3,}|\b[А-Я]\d\b|[\[\]‘’•<>]")


def read(p: Path) -> str:
    b = p.read_bytes()
    t = b.decode("utf-8-sig", errors="replace")
    # битые байты в выдаче pdftotext терпим; много — значит, CP1251
    if t.count("�") > len(t) / 1000:
        t = b.decode("cp1251", errors="replace")
    # шрифт PDF со сдвинутой кириллицей: «ɫɥɨɜɚɪɶ» = «словарь»
    shifted = sum(0x22B <= ord(c) <= 0x27B for c in t)
    if shifted > sum("а" <= c <= "я" for c in t):
        t = "".join(chr(ord(c) + 0x1D6) if 0x22B <= ord(c) <= 0x27B else c for c in t)
    return t


def sentences(text: str):
    text = text.replace("­", "").replace("-\n", "")
    text = re.sub(r"\s+", " ", text)
    text = re.sub(r"([а-яё])- ([а-яё])", r"\1\2", text)  # перенос «пол- зут»
    for s in SENT.split(text):
        s = re.sub(r"\s+([.,!?;:])", r"\1", CITE.sub("", s)).strip(" –—-")
        letters = [c for c in s if c.isalpha()]
        if not 25 <= len(s) <= 300 or len(letters) < 15 or MARKUP.search(s):
            continue
        if sum("а" <= c.lower() <= "я" or c in "ёЁ" for c in letters) < 0.9 * len(letters):
            continue
        yield s


if __name__ == "__main__":
    out = {"train": [], "valid": [], "test": []}
    seen = set()
    for d in sys.argv[1:]:
        for f in sorted(Path(d).glob("*.txt")):
            n = 0
            for s in sentences(read(f)):
                if s in seen:
                    continue
                seen.add(s)
                h = zlib.crc32(s.encode()) % 100
                out["valid" if h == 0 else "test" if h == 1 else "train"].append(s)
                n += 1
            print(f"{f.name}: {n}", file=sys.stderr)
    for split, lines in out.items():
        (paths.CORPUS / f"lit_ru.{split}.txt").write_text("\n".join(lines) + "\n", encoding="utf-8")
    print({k: len(v) for k, v in out.items()}, file=sys.stderr)
