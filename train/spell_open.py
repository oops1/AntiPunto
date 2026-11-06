"""Открытые наборы опечаток → пары «опечатка, верное, контекст» (данные — не в репозитории).

python spell_open.py <папка data ai-forever/spellcheck_benchmark> → spell/open_pairs.tsv
python spell_open.py --en <parquet GitHub Typo Corpus> <список ошибок Википедии> → spell/open_pairs_en.tsv
Предложения выравниваются по словам; берутся замены одного слова на одно (≤ 2 правки)
и одного на два (пропущенный пробел). Парам без контекста (список Википедии) контекст
подбирается из предложений корпуса с верным словом.
"""

import difflib
import json
import re
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import paths
from spell_common import Dictionary, osa

WORDS = {"ru": re.compile(r"[А-Яа-яЁё]+"), "en": re.compile(r"[A-Za-z]+")}
WORD = WORDS["ru"]
SKIP_DOMAINS = {"MedSpellchecker"}


def pairs(src: str, cor: str, lang: str = "ru"):
    WORD = WORDS[lang]
    a = [(m.group(), m.start()) for m in WORD.finditer(src)]
    b = [m.group() for m in WORD.finditer(cor)]
    sm = difflib.SequenceMatcher(None, [w.lower() for w, _ in a], [w.lower() for w in b], autojunk=False)
    for tag, i1, i2, j1, j2 in sm.get_opcodes():
        if tag != "replace":
            continue
        typed, pos = a[i1][0].lower(), a[i1][1]
        if i2 - i1 != 1 or typed != a[i1][0] and not a[i1][0][1:].islower():
            continue
        target = " ".join(b[j1:j2]).lower()
        if j2 - j1 == 1 and osa(typed, target) <= 2 or j2 - j1 == 2 and osa(typed, target.replace(" ", "")) <= 1:
            end = pos + len(typed)
            yield typed, target, clean(src[max(0, pos - 60) : pos]), clean(src[end : end + 30])


def clean(s: str) -> str:
    return s.replace("\t", " ").replace("\r", " ").replace("\n", " ")


def main(root: Path):
    d = Dictionary("ru")
    out, stats = [], Counter()
    for p in sorted(root.rglob("*.json")):
        domain = p.relative_to(root).parts[0]
        if domain in SKIP_DOMAINS or (domain == "MultidomainGold" and len(p.relative_to(root).parts) == 2):
            continue  # сводные файлы MultidomainGold дублируют поддомены
        for line in p.read_text(encoding="utf-8-sig").splitlines():
            if not line.strip():
                continue
            r = json.loads(line)
            stats["предложений"] += 1
            src = f"{domain}/{p.stem}"
            for typed, target, ctx, right in pairs(r["source"], r["correction"]):
                kind = "split" if " " in target else "typo"
                where = "словарное" if typed in d.freq else "вне словаря"
                stats[f"{kind}, {where}"] += 1
                out.append((typed, target, ctx, src, kind, where, right))
            # слова вне словаря, которые разметчики оставили: учим не трогать
            kept = set(WORD.findall(r["correction"].lower()))
            for m in WORD.finditer(r["source"]):
                w = m.group().lower()
                if m.group() == w and 3 <= len(w) <= 20 and w not in d.freq and w in kept:
                    stats["keep, вне словаря"] += 1
                    s = r["source"]
                    out.append((w, "", clean(s[max(0, m.start() - 60) : m.start()]), src, "keep", "вне словаря", clean(s[m.end() : m.end() + 30])))
    lines = ["слово\tверное\tконтекст\tисточник\tтип\tнабранное\tсправа"]
    lines += ["\t".join(r) for r in out]
    (paths.ROOT / "spell" / "open_pairs.tsv").write_text("\n".join(lines) + "\n", encoding="utf-8")
    for k, v in sorted(stats.items()):
        print(f"{k}: {v}")


def prose(line: str) -> bool:
    """Строка из коммита похожа на текст, а не на код."""
    letters = sum(c.isalpha() or c == " " for c in line)
    return len(line) >= 20 and letters / len(line) > 0.8 and len(WORDS["en"].findall(line)) >= 4


def main_en(parquet: Path, wiki_list: Path):
    import pyarrow.parquet as pq

    d = Dictionary("en")
    out, stats = [], Counter()
    t = pq.read_table(parquet).to_pydict()
    for src, cor in zip(t["text"], t["correction"]):
        if not prose(src) or not src.isascii():
            continue
        stats["строк текста"] += 1
        for typed, target, ctx, right in pairs(src, cor, "en"):
            if target.replace(" ", "") not in d.freq and target not in d.freq:
                continue  # имена из кода
            kind = "split" if " " in target else "typo"
            where = "словарное" if typed in d.freq else "вне словаря"
            stats[f"{kind}, {where}"] += 1
            out.append((typed, target, ctx, "github_typo_corpus", kind, where, right))
        # слова вне словаря, оставленные правкой, — «не трогать» (не больше одного со строки)
        kept = set(WORDS["en"].findall(cor.lower()))
        for m in WORDS["en"].finditer(src):
            w = m.group()
            if w.islower() and 3 <= len(w) <= 20 and w not in d.freq and w in kept and stats["keep, вне словаря"] < 30000:
                stats["keep, вне словаря"] += 1
                ctx = clean(src[max(0, m.start() - 60) : m.start()])
                out.append((w, "", ctx, "github_typo_corpus", "keep", "вне словаря", clean(src[m.end() : m.end() + 30])))
                break
    # список Википедии: пары без контекста, контекст — из предложений корпуса с верным словом
    wiki = []
    for line in wiki_list.read_text(encoding="utf-8").splitlines():
        if "->" not in line:
            continue
        w, rest = line.split("->", 1)
        w, targets = w.strip().lower(), [x.strip().lower() for x in rest.split(",")]
        if WORDS["en"].fullmatch(w) and 3 <= len(w) <= 20 and w not in d.freq and targets[0] in d.freq:
            wiki.append((w, targets[0]))
    need = {t for _, t in wiki}
    ctxs: dict[str, list[tuple[str, str]]] = {}
    with open(paths.CORPUS / "en.train.txt", encoding="utf-8") as f:
        for i, line in enumerate(f):
            if i >= 1_500_000:
                break
            low = line.lower()
            for m in WORDS["en"].finditer(low):
                w = m.group()
                if w in need and len(ctxs.setdefault(w, [])) < 2:
                    ctxs[w].append((clean(line[max(0, m.start() - 60) : m.start()]).strip(), clean(line[m.end() : m.end() + 30])))
    for w, t in wiki:
        for ctx, right in ctxs.get(t, []):
            stats["wiki typo, вне словаря"] += 1
            out.append((w, t, ctx, "wikipedia_misspellings", "typo", "вне словаря", right))
    lines = ["слово\tверное\tконтекст\tисточник\tтип\tнабранное\tсправа"]
    lines += ["\t".join(r) for r in out]
    (paths.ROOT / "spell" / "open_pairs_en.tsv").write_text("\n".join(lines) + "\n", encoding="utf-8")
    for k, v in sorted(stats.items()):
        print(f"{k}: {v}")


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    if sys.argv[1] == "--en":
        main_en(Path(sys.argv[2]), Path(sys.argv[3]))
    else:
        main(Path(sys.argv[1]))
