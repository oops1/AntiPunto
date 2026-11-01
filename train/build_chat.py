"""Разговорный корпус: комментарии Pikabu и субтитры OpenSubtitles."""

import gzip
import hashlib
import io
import json
import random
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import zstandard

from build_corpus import split_of
import paths

RAW = paths.CORPUS / "raw"
OUT = paths.CORPUS
LIMIT = 4_000_000

MD_LINK = re.compile(r"!?\[([^\]]*)\]\([^)]*\)")
URL = re.compile(r"https?://\S+|www\.\S+")
MENTION = re.compile(r"@\w+")
TAGS = re.compile(r"<[^>]+>|[*_~`#]+")
SENT = re.compile(r"(?<=[.!?…])\s+")
SPACES = re.compile(r"\s+")
CYR = re.compile(r"[а-яёА-ЯЁ]")
LAT = re.compile(r"[a-zA-Z]")
REPEAT = re.compile(r"(.)\1{4,}")


def ok(s: str, lang: str) -> bool:
    if not 5 <= len(s) <= 300 or REPEAT.search(s):
        return False
    cyr, lat = len(CYR.findall(s)), len(LAT.findall(s))
    letters = cyr + lat
    if letters < 3 or letters < 0.5 * len(s.replace(" ", "")):
        return False
    return cyr >= 0.7 * letters if lang == "ru" else lat >= 0.97 * letters


def partial_lines(path: Path, opener):
    """Строки из обрезанного архива; хвост отбрасывается."""
    with open(path, "rb") as raw:
        stream = io.TextIOWrapper(opener(raw), encoding="utf-8", errors="replace")
        try:
            prev = None
            for line in stream:
                if prev is not None:
                    yield prev
                prev = line
        except (EOFError, zstandard.ZstdError, OSError):
            pass


def pikabu_comments():
    for line in partial_lines(RAW / "pikabu_00.part.zst", lambda f: zstandard.ZstdDecompressor().stream_reader(f)):
        try:
            post = json.loads(line)
        except json.JSONDecodeError:
            continue
        cm = post.get("comments") or []
        texts = cm.get("text_markdown", []) if isinstance(cm, dict) else [c.get("text_markdown") for c in cm]
        for t in texts:
            if not t:
                continue
            for para in t.split("\n"):
                para = para.strip()
                if not para or para.startswith(">"):
                    continue
                para = TAGS.sub("", MENTION.sub("", URL.sub("", MD_LINK.sub(r"\1", para))))
                for s in SENT.split(para):
                    yield SPACES.sub(" ", s).strip(" ,")


def subtitles(lang: str, rng: random.Random):
    buf = []
    for line in partial_lines(RAW / f"opensub_{lang}.part.gz", lambda f: gzip.GzipFile(fileobj=f)):
        s = SPACES.sub(" ", line.strip().lstrip("-– ").strip())
        if not s:
            continue
        buf.append(s)
        if len(buf) >= rng.choice((1, 1, 2, 3)):
            yield " ".join(buf)
            buf = []


def write(lang: str, sources) -> None:
    files = {k: (OUT / f"chat_{lang}.{k}.txt").open("w", encoding="utf-8") for k in ("train", "valid", "test")}
    seen: set[bytes] = set()
    for name, gen, limit in sources:
        n = 0
        for s in gen:
            if not ok(s, lang):
                continue
            key = hashlib.blake2b(s.lower().encode(), digest_size=8).digest()
            if key in seen:
                continue
            seen.add(key)
            files[split_of(s)].write(s + "\n")
            n += 1
            if n >= limit:
                break
        print(f"{lang} {name}: {n}", flush=True)
    for f in files.values():
        f.close()


if __name__ == "__main__":
    rng = random.Random(3)
    write("ru", [("pikabu", pikabu_comments(), LIMIT // 2), ("opensub", subtitles("ru", rng), LIMIT // 2)])
    write("en", [("opensub", subtitles("en", rng), LIMIT)])
