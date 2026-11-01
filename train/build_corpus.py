"""Нарезка parquet Википедии на предложения: ru/en, train/valid/test."""

import argparse
import hashlib
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import pyarrow.parquet as pq

import paths

SENT_SPLIT = re.compile(r"(?<=[.!?…])\s+(?=[\"«(]?[A-ZА-ЯЁ0-9])")
SPACES = re.compile(r"\s+")
CYR = re.compile(r"[а-яёА-ЯЁ]")
LAT = re.compile(r"[a-zA-Z]")
JUNK = re.compile(r"[|{}<>=\\_]|https?://|\.\w{2,4}/")


def sentences(text: str):
    for para in text.split("\n"):
        para = para.strip()
        if len(para) < 20 or para.startswith(("*", "#", "!")):
            continue
        yield from SENT_SPLIT.split(para)


def good(s: str, lang: str) -> bool:
    if not 20 <= len(s) <= 300 or JUNK.search(s):
        return False
    cyr, lat = len(CYR.findall(s)), len(LAT.findall(s))
    letters = cyr + lat
    if letters < 0.6 * len(s.replace(" ", "")):
        return False
    main = cyr if lang == "ru" else lat
    return main >= 0.85 * letters


def split_of(s: str) -> str:
    h = int.from_bytes(hashlib.blake2b(s.encode(), digest_size=4).digest(), "little") % 1000
    return "test" if h < 5 else "valid" if h < 10 else "train"


def build(raw: Path, out: Path, lang: str, limit: int) -> None:
    out.mkdir(parents=True, exist_ok=True)
    files = {k: (out / f"{lang}.{k}.txt").open("w", encoding="utf-8") for k in ("train", "valid", "test")}
    seen: set[bytes] = set()
    n = 0
    for pf in sorted(raw.glob(f"*.{lang}_*.parquet")):
        for batch in pq.ParquetFile(pf).iter_batches(columns=["text"], batch_size=2000):
            for text in batch.column(0).to_pylist():
                for s in sentences(text):
                    s = SPACES.sub(" ", s).strip()
                    if not good(s, lang):
                        continue
                    key = hashlib.blake2b(s.lower().encode(), digest_size=8).digest()
                    if key in seen:
                        continue
                    seen.add(key)
                    files[split_of(s)].write(s + "\n")
                    n += 1
                    if n >= limit:
                        break
                if n >= limit:
                    break
            if n >= limit:
                break
        print(f"{lang}: {pf.name} -> {n}", flush=True)
        if n >= limit:
            break
    for f in files.values():
        f.close()
    print(f"{lang}: всего {n}")


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--raw", type=Path, default=paths.CORPUS / "raw")
    ap.add_argument("--out", type=Path, default=paths.CORPUS)
    ap.add_argument("--limit", type=int, default=4_000_000)
    a = ap.parse_args()
    for lang in ("ru", "en"):
        build(a.raw, a.out, lang, a.limit)
