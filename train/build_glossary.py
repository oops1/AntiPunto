"""Словарь латинских терминов: из русских корпусов и тегов StackExchange."""

import gzip
import json
import re
import sys
import time
import urllib.request
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import paths

CORPUS = paths.CORPUS
OUT = CORPUS / "glossary.tsv"
LATIN = re.compile(r"(?<![\w@/.])[A-Za-z][A-Za-z0-9+#]*(?:[.\-][A-Za-z0-9+#]+)*(?![\w@/])")
CYR = re.compile(r"[а-яёА-ЯЁ]")
SITES = {
    "stackoverflow": 10, "softwareengineering": 5, "electronics": 10, "arduino": 5, "raspberrypi": 5,
    "superuser": 10, "serverfault": 10, "unix": 5, "networkengineering": 5, "dba": 5,
}


def mine(limit_lines: int = 2_000_000) -> Counter:
    """Латинские слова внутри русских строк; регистр — самый частый."""
    cnt, forms = Counter(), {}
    for name in ("ru.train.txt", "chat_ru.train.txt"):
        with open(CORPUS / name, encoding="utf-8") as f:
            for i, line in enumerate(f):
                if i >= limit_lines:
                    break
                if not CYR.search(line):
                    continue
                for w in LATIN.findall(line):
                    if len(w) < 2:
                        continue
                    k = w.lower()
                    cnt[k] += 1
                    forms.setdefault(k, Counter())[w] += 1
    return Counter({forms[k].most_common(1)[0][0]: c for k, c in cnt.items() if c >= 5})


def fetch(site: str, page: int) -> dict:
    """Страница тегов: из кэша se_tags/ (качается curl) или по сети."""
    cached = CORPUS / "se_tags" / f"{site}_{page}.json"
    if cached.exists():
        raw = cached.read_bytes()
    else:
        url = f"https://api.stackexchange.com/2.3/tags?order=desc&sort=popular&site={site}&pagesize=100&page={page}"
        req = urllib.request.Request(url, headers={"User-Agent": "AntiPunto-glossary/1.0"})
        with urllib.request.urlopen(req, timeout=30) as r:
            raw = r.read()
        time.sleep(0.3)
    return json.loads(gzip.decompress(raw) if raw[:2] == b"\x1f\x8b" else raw)


def se_tags() -> Counter:
    out = Counter()
    for site, pages in SITES.items():
        for page in range(1, pages + 1):
            data = fetch(site, page)
            for t in data.get("items", []):
                name = t["name"].replace("-", " ")
                if re.search(r"[a-z]", name):
                    out[name] = max(out[name], t.get("count", 1))
            if data.get("backoff"):
                time.sleep(data["backoff"])
            if not data.get("has_more"):
                break
        print(f"{site}: всего тегов {len(out)}", flush=True)
    return out


def common_english(top: int = 20000, limit_lines: int = 1_000_000) -> set[str]:
    """Частые английские слова: их в словарь терминов не берём."""
    cnt = Counter()
    for name in ("en.train.txt", "chat_en.train.txt"):
        with open(CORPUS / name, encoding="utf-8") as f:
            for i, line in enumerate(f):
                if i >= limit_lines:
                    break
                cnt.update(re.findall(r"[a-z]+", line.lower()))
    return {w for w, _ in cnt.most_common(top)}


def is_term(t: str, common: set[str]) -> bool:
    """Термин, а не обычное слово: цифры, iPhone/GitHub, аббревиатура или редкое слово."""
    if any(c.isdigit() for c in t) or any(c in t for c in "+#._"):
        return True
    words = t.split()
    if any(any(c.isupper() for c in w[1:]) for w in words):
        return True
    if t.isupper() and 2 <= len(t) <= 6:
        return True
    return not all(w.lower() in common for w in words)


if __name__ == "__main__":
    if "--clean" in sys.argv:
        common = common_english()
        rows = [l.split("\t") for l in OUT.read_text(encoding="utf-8").splitlines()]
        kept = [r for r in rows if is_term(r[1], common)]
        (CORPUS / "glossary_clean.tsv").write_text("".join("\t".join(r) + "\n" for r in kept), encoding="utf-8")
        print(f"очищено: {len(rows)} -> {len(kept)}")
        sys.exit()
    mined = mine()
    print(f"из корпусов: {len(mined)}", flush=True)
    tags = se_tags() if "--no-tags" not in sys.argv else Counter()
    with open(OUT, "w", encoding="utf-8") as f:
        for src, c in (("mined", mined), ("tags", tags)):
            for term, n in c.most_common():
                f.write(f"{src}\t{term}\t{n}\n")
    print(f"записано: {OUT}, {len(mined) + len(tags)} терминов")
