"""Дообучение ранжировщика на настоящих опечатках с подмешиванием синтетики.

python spell_finetune.py --init spell_v8.pt --out spell_v9.pt
Учим на открытых парах (spell_open.py) и половине своей разметки (dev); отбираем шаг
по отложенным 10 % открытых пар и синтетическому valid; отчёт — на второй половине
разметки (held), которой модель не видела.
--dict dict_train.npz: словарный режим — в батч подмешиваются группы со словарным набранным
(ошибка, ставшая другим словом, или «оставить»), в настоящие — открытые пары «словарное».
Каждая настоящая пара даёт две группы: без контекста справа (первая проверка) и с ним (перепроверка
после следующего слова); отчёты — в обоих видах.
"""

import argparse
import math
import sys
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import numpy as np
import torch
import torch.nn.functional as F

import paths
import spell_real_eval as real
from spell_common import K, L, R0, Dictionary, osa, row
from spell_keep import Bloom
from spell_known import MODELS
from spell_model import load_net, save_net
from spell_train import evaluate, load, logits

DATA = paths.ROOT / "spell"
T = 0.75


def group(d: Dictionary, known: Bloom, typed: str, target: str | None, ctx: str, in_dict: bool = False, right: str = ""):
    """Группа как в клиенте; None — клиент модель не спросит или верного ответа нет среди кандидатов."""
    if typed in d.freq:
        if not in_dict:
            return None
    elif typed in known:
        return None
    cands = d.candidates(typed)
    if not cands:
        return None
    y = 0
    if target:
        hits = [j for j, c in enumerate(cands, 1) if c.replace("ё", "е") == target.replace("ё", "е")]
        if not hits:
            return None
        y = hits[0]
    rows = np.zeros((K, L), np.uint8)
    rows[0] = row(ctx, typed, typed, 0, 0, d.freq.get(typed, 0), right)
    for j, c in enumerate(cands, 1):
        rows[j] = row(ctx, typed, c, j, osa(typed, c), d.freq_of(c), right)
    mask = np.zeros(K, np.uint8)
    mask[: len(cands) + 1] = 1
    return rows, mask, y


def open_cases(lang: str = "ru", want: str = "вне словаря"):
    name = "open_pairs.tsv" if lang == "ru" else "open_pairs_en.tsv"
    for line in (DATA / name).read_text(encoding="utf-8").split("\n")[1:]:
        parts = line.split("\t")
        if len(parts) not in (6, 7):
            continue
        w, t, ctx, _src, _kind, where = parts[:6]
        if where == want and len(w) >= 3 and (not t or len(t) >= 3):
            yield w, t or None, ctx, parts[6] if len(parts) == 7 else ""


def cat(*sets):
    return tuple(torch.cat(ts) for ts in zip(*sets))


def tensors(groups, dev):
    x = torch.from_numpy(np.stack([g[0] for g in groups])).to(dev)
    m = torch.from_numpy(np.stack([g[1] for g in groups])).to(dev)
    y = torch.tensor([g[2] for g in groups], dtype=torch.long, device=dev)
    return x, m, y


@torch.no_grad()
def client_score(model, data, totals, t=T):
    """Как клиент при пороге t: верно / неверно / испорчено; totals — все случаи, включая недостижимые."""
    model.eval()
    x, m, y = data
    right = wrong = false = 0
    for i in range(0, len(x), 256):
        with torch.autocast("cuda", torch.bfloat16):
            p = torch.softmax(logits(model, x[i : i + 256], m[i : i + 256]), 1)
        conf, b = p.max(1)
        yy = y[i : i + 256]
        act = (b != 0) & (conf >= t)
        right += int((act & (b == yy) & (yy != 0)).sum())
        wrong += int((act & (b != yy) & (yy != 0)).sum())
        false += int((act & (yy == 0)).sum())
    model.train()
    n_t, n_k = totals
    return f"верно {right}/{n_t} ({right / n_t:.1%}) неверно {wrong} испорчено {false}/{n_k}"


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    ap = argparse.ArgumentParser()
    ap.add_argument("--init", type=Path, default=DATA / "spell_v8.pt")
    ap.add_argument("--out", type=Path, default=DATA / "spell_v9.pt")
    ap.add_argument("--steps", type=int, default=3000)
    ap.add_argument("--bs", type=int, default=64)
    ap.add_argument("--lr", type=float, default=1e-4)
    ap.add_argument("--real", type=float, default=0.5, help="доля настоящих в батче")
    ap.add_argument("--every", type=int, default=250)
    ap.add_argument("--mine", type=int, default=1, help="во сколько раз повторить свои случаи")
    ap.add_argument("--keep-cap", type=int, default=10**9, help="не больше стольких открытых «оставить»")
    ap.add_argument("--en", type=int, default=0, help="сколько английских открытых пар добавить (0 — без английского)")
    ap.add_argument("--dict", type=Path, help="словарные группы генератора (dict_train.npz)")
    ap.add_argument("--dict-frac", type=float, default=0.4, help="доля словарных групп в батче")
    ap.add_argument("--rw-en", type=int, default=6000, help="сколько английских открытых пар «словарное» добавить")
    ap.add_argument("--pos-lr", type=float, help="свой шаг для позиционных эмбеддингов (новые позиции правого контекста)")
    a = ap.parse_args()
    dev = torch.device("cuda")
    torch.manual_seed(0)
    known = Bloom(MODELS / "spell_known.bloom")
    dicts = {"ru": Dictionary("ru"), "en": Dictionary("en")}
    d = dicts["ru"]

    mine = real.load_cases()
    half = lambda c: zlib.crc32(c[0].encode()) % 2
    mine_dev = [(w, t, ctx, r) for w, t, ctx, _, r in mine if half((w,)) == 0]
    mine_held = [(w, t, ctx, r) for w, t, ctx, _, r in mine if half((w,)) == 1]
    opened = list(open_cases())
    open_val = [c for c in opened if zlib.crc32(c[0].encode()) % 10 == 0]
    open_tr = [c for c in opened if zlib.crc32(c[0].encode()) % 10 != 0]

    def build(cases, lang="ru", in_dict=False, right=False):
        """Группы как в клиенте; right — с контекстом справа (перепроверка после следующего слова)."""
        gs = [g for g in (group(dicts[lang], known, w, t, ctx, in_dict, r if right else "") for w, t, ctx, r in cases) if g]
        return tensors(gs, dev), (sum(1 for c in cases if c[1]), sum(1 for c in cases if not c[1]))

    def both(cases, lang="ru", in_dict=False):
        """Обучение: каждая пара без правого контекста и с ним."""
        l, _ = build(cases, lang, in_dict)
        r, _ = build([c for c in cases if c[3]], lang, in_dict, True)
        return cat(l, r)

    keeps = [c for c in open_tr if not c[1]][: a.keep_cap]
    open_tr = [c for c in open_tr if c[1]] + keeps
    train_real = both(open_tr + mine_dev * a.mine)
    val_open, tot_open = build(open_val)
    val_open_r = build(open_val, right=True)[0]
    held, tot_held = build(mine_held)
    held_r = build(mine_held, right=True)[0]
    if a.en:
        import random

        en = list(open_cases("en"))
        random.Random(0).shuffle(en)
        en_val = [c for c in en if zlib.crc32(c[0].encode()) % 10 == 0][:3000]
        en_tr = [c for c in en if zlib.crc32(c[0].encode()) % 10 != 0][: a.en]
        en_train = both(en_tr, "en")
        val_en, tot_en = build(en_val, "en")
        val_en_r = build(en_val, "en", right=True)[0]
        train_real = cat(train_real, en_train)
        print(f"английских групп {len(en_train[0])} («оставить» {(en_train[2] == 0).float().mean():.2f}), отбор по {len(val_en[0])}", flush=True)
    rw = {}
    if a.dict:
        import random

        dsynth, dvalid = load(a.dict.stem, dev), load(a.dict.stem.replace("train", "valid"), dev)
        # отложенные словарные «оставить» синтетики: мера ложных правок на правильных словах,
        # отдельно без правого контекста и с ним
        has_r = dvalid[0][:, 0, R0] != 0
        dkeep = {
            "без правого": tuple(t[(dvalid[2] == 0) & ~has_r] for t in dvalid),
            "с правым": tuple(t[(dvalid[2] == 0) & has_r] for t in dvalid),
        }
        for lang, cap in (("ru", 10**9), ("en", a.rw_en)):
            cases = [c for c in open_cases(lang, "словарное") if c[1] and c[0].replace("ё", "е") != c[1].replace("ё", "е")]
            random.Random(1).shuffle(cases)
            val = [c for c in cases if zlib.crc32(c[0].encode()) % 10 == 0]
            tr = [c for c in cases if zlib.crc32(c[0].encode()) % 10 != 0][:cap]
            rw_tr = both(tr, lang, True)
            rw[lang] = build(val, lang, True)
            rw[lang + "+"] = build(val, lang, True, True)
            train_real = cat(train_real, rw_tr)
            print(f"словарные открытые {lang}: обучение {len(rw_tr[0])} (из {len(tr)}), отбор {len(rw[lang][0][0])}", flush=True)
        print(
            f"словарные группы генератора {len(dsynth[0])} («оставить» {(dsynth[2] == 0).float().mean():.2f}), "
            f"отложенных «оставить» {len(dkeep['без правого'][0])} + {len(dkeep['с правым'][0])} с правым",
            flush=True,
        )
    synth, valid = load("train", dev), load("valid", dev)
    print(
        f"настоящих групп для обучения {len(train_real[0])} (открытых {len(open_tr)}, своих {len(mine_dev)}), "
        f"«оставить» {(train_real[2] == 0).float().mean():.2f}; отбор по {len(val_open[0])} открытым; "
        f"отчёт по {len(held[0])} своим (опечаток {tot_held[0]}, правильных {tot_held[1]})",
        flush=True,
    )

    model = load_net(a.init).train().to(dev)
    groups = [{"params": [p for n, p in model.named_parameters() if n != "pos"]}]
    if a.pos_lr:
        groups.append({"params": [model.pos], "lr": a.pos_lr})
    else:
        groups[0]["params"].append(model.pos)
    opt = torch.optim.AdamW(groups, lr=a.lr, betas=(0.9, 0.98), weight_decay=0.01)
    sched = torch.optim.lr_scheduler.LambdaLR(opt, lambda s: min(1.0, s / 100) * 0.5 * (1 + math.cos(math.pi * min(s, a.steps) / a.steps)))
    nr = max(1, int(a.bs * a.real))
    nd = int(a.bs * a.dict_frac) if a.dict else 0

    def report(step):
        vl, vacc = evaluate(model, valid)
        ol, oacc = evaluate(model, val_open)
        print(
            f"шаг {step}: открытые точность {oacc:.4f} | синтетика {vl:.4f} {vacc:.4f} | "
            f"открытые как клиент: {client_score(model, val_open, tot_open)} | свои held: {client_score(model, held, tot_held)}",
            flush=True,
        )
        print(
            f"    с правым контекстом: открытые {client_score(model, val_open_r, tot_open)} | свои held {client_score(model, held_r, tot_held)}",
            flush=True,
        )
        if a.en:
            print(f"    английские как клиент: {client_score(model, val_en, tot_en)} | с правым {client_score(model, val_en_r, tot_en)}", flush=True)
        if a.dict:
            dl, dacc = evaluate(model, dvalid)
            print(f"    словарные: синтетика {dl:.4f} {dacc:.4f}", flush=True)
            for t in (0.9, 0.95, 0.98):
                fp = " / ".join(client_score(model, k, (1, len(k[0])), t).split("испорчено ")[1] for k in dkeep.values())
                line = f"    порог {t:.2f}: ложных правок без/с правым {fp}"
                for lang in rw:
                    line += f" | {lang} {client_score(model, *rw[lang], t)}"
                print(line, flush=True)
        if step == 0 or step == a.steps:
            for t in (0.6, 0.65, 0.7, 0.8):
                line = f"    порог {t:.2f}: открытые {client_score(model, val_open, tot_open, t)} | свои held {client_score(model, held, tot_held, t)} | с правым {client_score(model, held_r, tot_held, t)}"
                if a.en:
                    line += f" | английские {client_score(model, val_en, tot_en, t)}"
                print(line, flush=True)
        return oacc, vacc

    base_open, base_synth = report(0)
    best = (base_open, 0)
    for step in range(1, a.steps + 1):
        ir = torch.randint(0, len(train_real[0]), (nr,), device=dev)
        is_ = torch.randint(0, len(synth[0]), (a.bs - nr - nd,), device=dev)
        parts = [(train_real, ir), (synth, is_)]
        if nd:
            parts.append((dsynth, torch.randint(0, len(dsynth[0]), (nd,), device=dev)))
        x = torch.cat([p[0][i] for p, i in parts])
        m = torch.cat([p[1][i] for p, i in parts])
        y = torch.cat([p[2][i] for p, i in parts])
        with torch.autocast("cuda", torch.bfloat16):
            z = logits(model, x, m)
        loss = F.cross_entropy(z, y)
        opt.zero_grad(set_to_none=True)
        loss.backward()
        torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
        opt.step()
        sched.step()
        if step % a.every == 0 or step == a.steps:
            oacc, vacc = report(step)
            # лучший по открытым, если синтетика просела не больше чем на пункт
            if oacc > best[0] and vacc >= base_synth - 0.01:
                best = (oacc, step)
                save_net(model, torch.load(a.init, map_location="cpu")["cfg"], a.out.with_name(a.out.stem + "_best.pt"))
    save_net(model, torch.load(a.init, map_location="cpu")["cfg"], a.out)
    print(f"последний шаг → {a.out}; лучший по открытым шаг {best[1]} ({best[0]:.4f})")
