"""Обучение ранжировщика с жёстким дедлайном по часам."""

import argparse
import datetime as dt
import math
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import numpy as np
import torch
import torch.nn.functional as F

import paths
from spell_common import K
from spell_model import SpellNet, save_net

DATA = paths.ROOT / "spell"


def load(name: str, dev):
    d = np.load(DATA / f"{name}.npz")
    return torch.from_numpy(d["x"]).to(dev), torch.from_numpy(d["m"]).to(dev), torch.from_numpy(d["y"]).long().to(dev)


def logits(model, x, m):
    b = x.shape[0]
    s = model(x.reshape(b * K, -1).long()).float().reshape(b, K)
    return s.masked_fill(m == 0, -1e4)


@torch.no_grad()
def evaluate(model, data, bs=256):
    model.eval()
    x, m, y = data
    loss = acc = 0.0
    for i in range(0, len(x), bs):
        with torch.autocast("cuda", torch.bfloat16):
            z = logits(model, x[i : i + bs], m[i : i + bs])
        loss += F.cross_entropy(z, y[i : i + bs], reduction="sum").item()
        acc += (z.argmax(1) == y[i : i + bs]).sum().item()
    model.train()
    # иначе пик проверки держится, и Windows выносит видеопамять в ОЗУ
    torch.cuda.empty_cache()
    return loss / len(x), acc / len(x)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--steps", type=int, default=30000)
    ap.add_argument("--bs", type=int, default=64)
    ap.add_argument("--lr", type=float, default=1e-3)
    ap.add_argument("--deadline", required=True, help="ЧЧ:ММ — остановиться не позже")
    ap.add_argument("--out", type=Path, default=DATA / "spell.pt")
    ap.add_argument("--d", type=int, default=128)
    ap.add_argument("--layers", type=int, default=3)
    ap.add_argument("--heads", type=int, default=4)
    a = ap.parse_args()
    cfg = {"d": a.d, "layers": a.layers, "heads": a.heads, "ff": 3 * a.d}
    h, mnt = map(int, a.deadline.split(":"))
    stop = dt.datetime.now().replace(hour=h, minute=mnt, second=0, microsecond=0)
    if stop < dt.datetime.now():
        stop += dt.timedelta(days=1)

    dev = torch.device("cuda")
    torch.manual_seed(0)
    tr, va = load("train", dev), load("valid", dev)
    model = SpellNet(**cfg).to(dev)
    print(f"параметров {sum(p.numel() for p in model.parameters()):,}, групп {len(tr[0]):,}, стоп {stop:%H:%M}", flush=True)
    opt = torch.optim.AdamW(model.parameters(), lr=a.lr, betas=(0.9, 0.98), weight_decay=0.01)
    sched = torch.optim.lr_scheduler.LambdaLR(
        opt, lambda s: min(1.0, s / 500) * 0.5 * (1 + math.cos(math.pi * min(s, a.steps) / a.steps))
    )
    t0, best = time.time(), 9.0
    for step in range(1, a.steps + 1):
        idx = torch.randint(0, len(tr[0]), (a.bs,), device=dev)
        with torch.autocast("cuda", torch.bfloat16):
            z = logits(model, tr[0][idx], tr[1][idx])
        loss = F.cross_entropy(z, tr[2][idx])
        opt.zero_grad(set_to_none=True)
        loss.backward()
        torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
        opt.step()
        sched.step()
        last = dt.datetime.now() >= stop
        if step % 1000 == 0 or step == a.steps or last:
            vl, vacc = evaluate(model, va)
            print(f"шаг {step}: train {loss.item():.4f} valid {vl:.4f} точность {vacc:.4f} {time.time() - t0:.0f} с", flush=True)
            if vl < best:
                best = vl
                save_net(model, cfg, a.out)
        if last:
            print("дедлайн: остановка", flush=True)
            break
