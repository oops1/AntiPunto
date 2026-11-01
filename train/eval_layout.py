"""Оценка как в рантайме: слово за словом, с ретроактивной правкой."""

import argparse
import pickle
import random
import re
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import numpy as np
import torch

from gen_layout import CORPUS, MIX, W, compose, load, pick, simulate, word_starts
from keys import NEUTRAL, SPACE, TOK_CHAR
from model_layout import LayoutNet
from paths import LAYOUT

DATA = LAYOUT
LETTERS = re.compile(r"[a-zа-яё]+")


def spans_of(tok):
    out, s = [], None
    for i, t in enumerate(tok + [SPACE]):
        if t != SPACE and s is None:
            s = i
        elif t == SPACE and s is not None:
            out.append((s, i))
            s = None
    return out


def make_samples(n: int, seed: int, sources):
    ru, en = load("valid", sources)
    rng = random.Random(seed)
    kinds, probs = zip(*MIX)
    out = []
    while len(out) < n:
        k = rng.choices(kinds, probs)[0]
        sim = simulate(compose(k, *pick(k, ru, en, rng), rng), rng)
        if sim:
            out.append((k, *sim, spans_of(sim[0])))
    return out


@torch.no_grad()
def run_model(model, samples, t: float, dev, bs: int = 8192):
    toks = [np.array(s[1], np.int64) for s in samples]
    cur = [np.array(s[2], np.int64) for s in samples]
    core = [np.array([x not in NEUTRAL for x in s[1]]) for s in samples]
    done = [dict() for _ in samples]
    for j in range(max(len(s[4]) for s in samples)):
        ids = [i for i, s in enumerate(samples) if len(s[4]) > j]
        T = np.zeros((len(ids), W), np.int64)
        A = np.zeros((len(ids), W), np.int64)
        se = []
        for r, i in enumerate(ids):
            e = samples[i][4][j][1]
            s = max(word_starts(samples[i][1], e), e - W)
            T[r, W - (e - s):] = toks[i][s:e]
            A[r, W - (e - s):] = cur[i][s:e]
            se.append((s, e))
        P = np.concatenate([
            torch.sigmoid(model(torch.from_numpy(T[k:k + bs]).to(dev), torch.from_numpy(A[k:k + bs]).to(dev))).float().cpu().numpy()
            for k in range(0, len(ids), bs)
        ])
        for r, i in enumerate(ids):
            s, e = se[r]
            off = W - (e - s)
            for wi, (a, b) in enumerate(samples[i][4][: j + 1]):
                if a < s or wi in done[i]:
                    continue
                m = core[i][a:b]
                if m.any() and P[r, off + a - s : off + b - s][m].mean() > t:
                    cur[i][a:b] ^= 1
                    done[i][wi] = j
    return cur, done


def build_dict():
    p = DATA / "dict.pkl"
    if p.exists():
        return pickle.loads(p.read_bytes())
    D = []
    for lang in ("en", "ru"):
        cnt = Counter()
        with open(CORPUS / f"{lang}.train.txt", encoding="utf-8") as f:
            for i, line in enumerate(f):
                if i >= 1_000_000:
                    break
                cnt.update(LETTERS.findall(line.lower()))
        D.append({w for w, c in cnt.items() if c >= 3})
    p.write_bytes(pickle.dumps(D))
    return D


def baseline(samples, D):
    out = []
    for _, tok, act, _, spans in samples:
        a = np.array(act, np.int64)
        for s, e in spans:
            d = int(round(np.mean(act[s:e])))
            seg = tok[s:e]
            while seg and not TOK_CHAR[d].get(seg[0], "").isalpha():
                seg = seg[1:]
            while seg and not TOK_CHAR[d].get(seg[-1], "").isalpha():
                seg = seg[:-1]
            disp = "".join(c for c in (TOK_CHAR[d].get(t, "") for t in seg) if c.isalpha()).lower()
            conv = "".join(c for c in (TOK_CHAR[1 - d].get(t, "") for t in seg) if c.isalpha()).lower()
            if disp and disp not in D[d] and conv in D[1 - d]:
                a[s:e] ^= 1
        out.append(a)
    return out


def score(samples, cur, done=None):
    c = Counter()
    for idx, ((kind, tok, act, lab, spans), a) in enumerate(zip(samples, cur)):
        lab, act0 = np.array(lab), np.array(act)
        all_ok = True
        for wi, (s, e) in enumerate(spans):
            m = lab[s:e] >= 0
            if not m.any():
                continue
            want = (act0[s:e] ^ np.maximum(lab[s:e], 0))[m]
            w0 = (act0[s:e][m] != want).mean() > 0.5
            w1 = (a[s:e][m] != want).mean() > 0.5
            all_ok &= not w1
            for g in ("short" if m.sum() <= 2 else "long", "all"):
                c[g, "wrong" if w0 else "ok"] += 1
                c[g, "miss"] += w0 and w1
                c[g, "false"] += (not w0) and w1
                if done is not None and w0 and not w1 and done[idx].get(wi, wi) > wi:
                    c[g, "late"] += 1
        c["sent"] += 1
        c["sent_ok"] += all_ok
    return c


def report(name, c, f):
    line = f"{name:<14}"
    for g in ("short", "long", "all"):
        line += f"| {g}: ложных {c[g, 'false'] / max(1, c[g, 'ok']):6.2%}  пропусков {c[g, 'miss'] / max(1, c[g, 'wrong']):6.2%} "
    line += f"| поздних {c['all', 'late'] / max(1, c['all', 'wrong']):5.1%} | фраз без ошибок {c['sent_ok'] / c['sent']:6.2%}"
    print(line, file=f, flush=True)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=10000)
    ap.add_argument("--ckpt", type=Path, default=DATA / "layout.pt")
    ap.add_argument("--out", type=Path, default=DATA / "eval.txt")
    ap.add_argument("--sources", default="wiki")
    a = ap.parse_args()

    samples = make_samples(a.n, seed=7, sources=a.sources.split(","))
    dev = torch.device("cuda")
    model = LayoutNet().to(dev).eval()
    model.load_state_dict(torch.load(a.ckpt, map_location=dev))

    with open(a.out, "w", encoding="utf-8") as f:
        report("без правки", score(samples, [np.array(s[2]) for s in samples]), f)
        report("словарь", score(samples, baseline(samples, build_dict())), f)
        for t in (0.5, 0.7, 0.9, 0.97):
            cur, done = run_model(model, samples, t, dev)
            report(f"сеть t={t}", score(samples, cur, done), f)
        cur, done = run_model(model, samples, 0.9, dev)
        print("\nпо типам, сеть t=0.9:", file=f)
        for kind, _ in MIX:
            sub = [i for i, s in enumerate(samples) if s[0] == kind]
            report(kind, score([samples[i] for i in sub], [cur[i] for i in sub], [done[i] for i in sub]), f)
    print(a.out.read_text(encoding="utf-8"))
