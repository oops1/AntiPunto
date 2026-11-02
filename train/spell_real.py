"""Настоящие опечатки из своих сообщений Claude Code: незнакомые слова, кандидаты, выбор модели.

python spell_real.py <папка с .jsonl> — результат в папке данных (spell/real_*.tsv), не в репозитории.
"""

import json
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import torch

import paths
from spell_common import K, L, Dictionary, osa, row
from spell_keep import Bloom
from spell_known import MODELS
from spell_model import load_net

DATA = paths.ROOT / "spell"
WORD = re.compile(r"[А-Яа-яЁё]+|[A-Za-z]+")
SKIP = ("<", "This session is being continued", "[Request interrupted", "Caveat:")


def user_texts(root: Path):
    seen = set()
    for f in root.rglob("*.jsonl"):
        if "subagents" in f.parts:
            continue
        for line in f.open(encoding="utf-8", errors="replace"):
            try:
                r = json.loads(line)
            except json.JSONDecodeError:
                continue
            if r.get("type") != "user" or r.get("isSidechain") or r.get("isMeta"):
                continue
            c = r.get("message", {}).get("content")
            parts = [c] if isinstance(c, str) else [p.get("text", "") for p in c or [] if p.get("type") == "text"]
            for t in parts:
                t = re.sub(r"<pasted_content.*?</pasted_content>", " ", t, flags=re.S).strip()
                if t and not t.startswith(SKIP) and t not in seen:
                    seen.add(t)
                    yield t


if __name__ == "__main__":
    root = Path(sys.argv[1])
    known = Bloom(MODELS / "spell_known.bloom")
    dicts = {"ru": Dictionary("ru"), "en": Dictionary("en")}
    net = load_net(DATA / (sys.argv[2] if len(sys.argv) > 2 else "spell.pt"))
    msgs = list(user_texts(root))
    words = Counter()
    ctxs = defaultdict(list)
    total = 0
    for t in msgs:
        for m in WORD.finditer(t):
            w = m.group()
            total += 1
            if w != w.lower() or not 3 <= len(w) <= 20:
                continue
            lang = "en" if w.isascii() else "ru"
            if w in dicts[lang].freq or w in known:
                continue
            words[(lang, w)] += 1
            if len(ctxs[(lang, w)]) < 3:
                ctxs[(lang, w)].append(t[max(0, m.start() - 60) : m.start()])
    out = [f"сообщений {len(msgs)}, слов {total}, незнакомых {sum(words.values())} ({len(words)} разных)"]
    rows = []
    for (lang, w), n in words.most_common():
        d = dicts[lang]
        cands = d.candidates(w)
        if not cands:
            rows.append((lang, w, n, "", 0.0, "нет кандидатов", ctxs[(lang, w)][0]))
            continue
        ctx = ctxs[(lang, w)][0]
        x = torch.zeros((K, L), dtype=torch.long)
        x[0] = torch.tensor(row(ctx, w, w))
        for j, c in enumerate(cands, 1):
            x[j] = torch.tensor(row(ctx, w, c, j, osa(w, c), d.freq[c]))
        with torch.no_grad():
            s = net(x)[: len(cands) + 1]
        p = torch.softmax(s, 0)
        b = int(p.argmax())
        rows.append((lang, w, n, cands[0], float(p[b]), "оставить" if b == 0 else cands[b - 1], ctx))
    lines = ["язык\tслово\tраз\tпервый кандидат\tуверенность\tвыбор модели\tконтекст"]
    lines += ["\t".join(str(v).replace("\t", " ").replace("\n", " ") for v in r) for r in rows]
    (DATA / "real_words.tsv").write_text("\n".join(lines), encoding="utf-8")
    print("\n".join(out), file=sys.stderr)
