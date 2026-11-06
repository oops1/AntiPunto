"""Вес языковой модели и пороги для клиента (константы LM_* в client/src/spell.rs).

python lm_fit.py dump [spell_v12]     # группы настоящих опечаток текстом и оценки ранжировщика
python lm_fit.py [имя файла оценок]   # подбор на одних наборах, отчёт на других
Между ними — lm_export.py score (или lm_ov.py score): оценки вариантов языковой моделью.

Правило клиента: P = softmax(ln p_ранжировщика + λ·lm); правим, если лучший не «оставить» и P ≥ порог.
Две стадии с общим порогом: на границе слова (без текста справа) и после следующего слова (с ним);
ранжировщик текста справа не видит. Порог подбирается так, чтобы плохих правок было столько же,
сколько у ранжировщика без языковой модели.
"""

import json
import sys
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import numpy as np

import paths

DATA = paths.ROOT / "spell"
NEG = -1e9
# пороги ранжировщика без языковой модели: вне словаря и для словарных слов (client/src/main.rs)
BASE, BASE_DICT = 0.70, 0.90
# словарное слово проверяем языковой моделью, только если ранжировщик в нём не уверен
GATE = 0.9


def dump(ck: str):
    import torch

    import spell_finetune as ft
    import spell_real_eval as real
    from spell_common import CHARS, CTX, R0, RCTX, WLEN, Dictionary
    from spell_keep import Bloom
    from spell_known import MODELS
    from spell_model import load_net
    from spell_train import load, logits

    dev = torch.device("cuda")
    known = Bloom(MODELS / "spell_known.bloom")
    d = Dictionary("ru")
    texts, xs, ms, ys = [], [], [], []

    def add(name, cases, in_dict):
        n = 0
        for w, t, ctx, r in cases:
            g = ft.group(d, known, w, t, ctx, in_dict, r)
            if g:
                xs.append(g[0])
                ms.append(g[1])
                ys.append(g[2])
                texts.append({"set": name, "left": ctx, "right": r, "opts": [w] + d.candidates(w), "y": int(g[2])})
                n += 1
        print(name, n, flush=True)

    mine = real.load_cases()
    half = lambda w: zlib.crc32(w.encode()) % 2
    add("свои dev", [(w, t, ctx, r) for w, t, ctx, _, r in mine if half(w) == 0 and r], False)
    add("свои held", [(w, t, ctx, r) for w, t, ctx, _, r in mine if half(w) == 1 and r], False)
    opened = [c for c in ft.open_cases() if c[3]]
    add("открытые train", [c for c in opened if zlib.crc32(c[0].encode()) % 10 > 5], False)
    add("открытые val", [c for c in opened if zlib.crc32(c[0].encode()) % 10 == 0], False)
    rw = [c for c in ft.open_cases("ru", "словарное") if c[1] and c[3] and c[0].replace("ё", "е") != c[1].replace("ё", "е")]
    add("открытые словарные", rw, True)

    def text(ids):
        return "".join(CHARS[i - 3] if 3 <= i < len(CHARS) + 3 else "?" for i in ids if i != 0)

    # синтетические словарные группы: здесь есть верные слова, которые нельзя испортить
    x, m, y = (a.numpy() for a in load("dict_valid", torch.device("cpu")))
    n = 0
    for g in range(len(y)):
        r0 = x[g, 0]
        typed = text(r0[CTX + 1 : CTX + 1 + WLEN])
        if r0[R0] == 0 or not typed or not ("а" <= typed[0] <= "я" or typed[0] == "ё"):
            continue
        opts = [typed] + [text(x[g, j, CTX + 2 + WLEN : CTX + 2 + 2 * WLEN]) for j in range(1, int(m[g].sum()))]
        xs.append(x[g])
        ms.append(m[g])
        ys.append(int(y[g]))
        texts.append({"set": "словарные синтетика", "left": text(r0[:CTX]), "right": text(r0[R0 : R0 + RCTX]), "opts": opts, "y": int(y[g])})
        n += 1
    print("словарные синтетика", n, flush=True)

    X = torch.from_numpy(np.stack(xs)).to(dev)
    M = torch.from_numpy(np.stack(ms)).to(dev)
    X[:, :, R0 : R0 + RCTX] = 0  # ранжировщик текста справа не видит
    net = load_net(DATA / f"{ck}.pt").to(dev)
    out = []
    with torch.no_grad():
        for i in range(0, len(X), 512):
            with torch.autocast("cuda", torch.bfloat16):
                out.append(logits(net, X[i : i + 512], M[i : i + 512]).float().cpu())
    np.savez(DATA / "lmcal_ours.npz", ranker=torch.cat(out).numpy(), mask=np.stack(ms), y=np.array(ys))
    (DATA / "lmcal_text.json").write_text(json.dumps(texts, ensure_ascii=False), encoding="utf-8")
    print("групп", len(texts), "строк", sum(len(o["opts"]) for o in texts))


def fit(scores: str):
    data = json.loads((DATA / "lmcal_text.json").read_text(encoding="utf-8"))
    ours = np.load(DATA / "lmcal_ours.npz")
    lm = np.load(DATA / f"{scores}.npz")
    ranker = ours["ranker"] if "ranker" in ours.files else ours["v12"]
    mask = ours["mask"].astype(bool)
    y = ours["y"]
    names = np.array([o["set"] for o in data])
    idx = np.arange(len(y))

    def lsm(a):
        a = np.where(mask, a, NEG)
        a = a - a.max(1, keepdims=True)
        return a - np.log(np.exp(a).sum(1, keepdims=True))

    def decide(sc):
        p = np.exp(lsm(sc))
        return p.argmax(1), p.max(1), p[:, 0]

    def rule(lam, gate=1.1):
        """Решения двух стадий; без λ — один ранжировщик на границе слова."""
        b0, c0, keep0 = decide(ranker)
        if not lam:
            return [(b0, c0)], None
        call = keep0 < gate
        stages = []
        for var in ("noright", "full"):
            b, c, _ = decide(lsm(ranker) + lam * lm[var])
            stages.append((np.where(call, b, b0), np.where(call, c, c0)))
        return stages, call

    def outcome(stages, thr, sel):
        done = np.zeros(len(y), bool)
        good = np.zeros(len(y), bool)
        bad = np.zeros(len(y), bool)
        for b, c in stages:
            act = ~done & (b != 0) & (c >= thr)
            ok = act & (b == y) & (y != 0)
            good |= ok
            bad |= act & ~ok
            done |= act
        return int(good[sel].sum()), int(bad[sel].sum())

    def match(stages, budget, sel):
        lo, hi = 0.30, 0.9999
        for _ in range(30):
            mid = (lo + hi) / 2
            if outcome(stages, mid, sel)[1] > budget:
                lo = mid
            else:
                hi = mid
        return hi

    def study(title, fit_sel, reports, base_thr, lams, gate):
        print(f"=== {title}: (верных, плохих)")
        old, _ = rule(0)
        budget = outcome(old, base_thr, fit_sel)[1]
        line = f"  без языковой модели, порог {base_thr:.2f}".ljust(46) + f"| подбор {outcome(old, base_thr, fit_sel)}"
        print(line + "".join(f" | {n} {outcome(old, base_thr, s)}" for n, s in reports))
        for lam in lams:
            stages, call = rule(lam, gate)
            t = match(stages, budget, fit_sel)
            line = f"  λ={lam}, порог {t:.3f}".ljust(46) + f"| подбор {outcome(stages, t, fit_sel)}"
            line += "".join(f" | {n} {outcome(stages, t, s)}" for n, s in reports)
            if gate < 1:
                line += f" | модель зовётся для {call[fit_sel & (y == 0)].mean():.0%} верных слов"
            print(line)

    out_fit = (names == "свои dev") | (names == "открытые train")
    study("слова вне словаря", out_fit, [("свои held", names == "свои held"), ("открытые val", names == "открытые val")], BASE, (0.2, 0.3, 0.4, 0.5), 1.1)
    syn = names == "словарные синтетика"
    reports = [("синтетика, отчёт", syn & (idx % 2 == 1)), ("открытые словарные", names == "открытые словарные")]
    study("словарные слова", syn & (idx % 2 == 0), reports, BASE_DICT, (0.5, 0.75, 1.0), GATE)


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    if len(sys.argv) > 1 and sys.argv[1] == "dump":
        dump(sys.argv[2] if len(sys.argv) > 2 else "spell_v12")
    else:
        fit(sys.argv[1] if len(sys.argv) > 1 else "lmcal_scores_rugpt3small")
