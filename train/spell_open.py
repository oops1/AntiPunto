"""Открытые наборы опечаток (ai-forever/spellcheck_benchmark) → пары «опечатка, верное, контекст».

python spell_open.py <папка data бенчмарка> → spell/open_pairs.tsv (данные — не в репозитории).
Предложения выравниваются по словам; берутся замены одного слова на одно (≤ 2 правки)
и одного на два (пропущенный пробел).
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

WORD = re.compile(r"[А-Яа-яЁё]+")
SKIP_DOMAINS = {"MedSpellchecker"}


def pairs(src: str, cor: str):
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
            yield typed, target, src[max(0, pos - 60) : pos].replace("\t", " ").replace("\n", " ")


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
            for typed, target, ctx in pairs(r["source"], r["correction"]):
                kind = "split" if " " in target else "typo"
                where = "словарное" if typed in d.freq else "вне словаря"
                stats[f"{kind}, {where}"] += 1
                out.append((typed, target, ctx, src, kind, where))
            # слова вне словаря, которые разметчики оставили: учим не трогать
            kept = set(WORD.findall(r["correction"].lower()))
            for m in WORD.finditer(r["source"]):
                w = m.group().lower()
                if m.group() == w and 3 <= len(w) <= 20 and w not in d.freq and w in kept:
                    stats["keep, вне словаря"] += 1
                    out.append((w, "", r["source"][max(0, m.start() - 60) : m.start()].replace("\t", " "), src, "keep", "вне словаря"))
    lines = ["слово\tверное\tконтекст\tисточник\tтип\tнабранное"]
    lines += ["\t".join(r) for r in out]
    (paths.ROOT / "spell" / "open_pairs.tsv").write_text("\n".join(lines) + "\n", encoding="utf-8")
    for k, v in sorted(stats.items()):
        print(f"{k}: {v}")


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    main(Path(sys.argv[1]))
