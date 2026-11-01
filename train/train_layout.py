"""Обучение модели раскладки на сгенерированных окнах."""

import argparse
import math
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import numpy as np
import torch
import torch.nn.functional as F

from model_layout import LayoutNet
from paths import LAYOUT

DATA = LAYOUT


def load(name: str, dev):
    d = np.load(DATA / f"{name}.npz")
    return [torch.from_numpy(d[k]).to(dev) for k in ("tok", "lay", "lab")]


def step_loss(model, tok, lay, lab):
    z = model(tok.long(), lay.long()).float()
    m = lab >= 0
    y = lab[m].float()
    z = z[m]
    return F.binary_cross_entropy_with_logits(z, y), ((z > 0).float() == y).float().mean()


@torch.no_grad()
def evaluate(model, data, bs: int = 4096):
    model.eval()
    tot, acc, n = 0.0, 0.0, 0
    for i in range(0, len(data[0]), bs):
        with torch.autocast("cuda", torch.bfloat16):
            l, a = step_loss(model, *(x[i : i + bs] for x in data))
        k = len(data[0][i : i + bs])
        tot, acc, n = tot + l.item() * k, acc + a.item() * k, n + k
    model.train()
    return tot / n, acc / n


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--steps", type=int, default=20000)
    ap.add_argument("--bs", type=int, default=1024)
    ap.add_argument("--lr", type=float, default=1e-3)
    ap.add_argument("--out", type=Path, default=DATA / "layout.pt")
    a = ap.parse_args()

    dev = torch.device("cuda")
    torch.manual_seed(0)
    tr, va = load("train", dev), load("valid", dev)
    model = LayoutNet().to(dev)
    print(f"параметров {sum(p.numel() for p in model.parameters()):,}, окон {len(tr[0]):,}", flush=True)

    opt = torch.optim.AdamW(model.parameters(), lr=a.lr, betas=(0.9, 0.98), weight_decay=0.01)
    sched = torch.optim.lr_scheduler.LambdaLR(
        opt, lambda s: min(1.0, s / 500) * 0.5 * (1 + math.cos(math.pi * min(s, a.steps) / a.steps))
    )
    t0, best = time.time(), 9.0
    for step in range(1, a.steps + 1):
        idx = torch.randint(0, len(tr[0]), (a.bs,), device=dev)
        with torch.autocast("cuda", torch.bfloat16):
            loss, _ = step_loss(model, *(x[idx] for x in tr))
        opt.zero_grad(set_to_none=True)
        loss.backward()
        torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
        opt.step()
        sched.step()
        if step % 1000 == 0 or step == a.steps:
            vl, vacc = evaluate(model, va)
            print(f"шаг {step}: train {loss.item():.4f}  valid {vl:.4f}  точность символов {vacc:.4f}  {time.time() - t0:.0f} с", flush=True)
            if vl < best:
                best = vl
                torch.save(model.state_dict(), a.out)
