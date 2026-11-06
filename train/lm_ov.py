"""Языковая модель в OpenVINO: тело (CPU или NPU) и выходной слой на хвосте строки.

python lm_ov.py convert                 # из TorchScript (lm_export.py export): fp16, веса int8, полное int8
python lm_ov.py check                   # хвостовые оценки против оценок исходной модели
python lm_ov.py bench <тело> <голова>   # скорость на CPU, NPU и встроенной графике: fp16 | w8 | q8
python lm_ov.py score <тело> <голова> [устройство]   # оценки для lm_fit.py
Хвост — токены после общего для всех вариантов начала: их сумма отличается от полной
на постоянную по группе, а считать приходится втрое меньше.
"""

import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import numpy as np
import openvino as ov

import paths

OUT = paths.ROOT / "lm" / "ov"
DATA = paths.ROOT / "spell"
B = 16
core = ov.Core()


def build(ids, lens, idx, T, S):
    """Строки группы: начало строки, дыра, текст прижат вправо; маска хвоста."""
    toks = [ids[k, : lens[k]] for k in idx[:B]]
    p = 1
    while all(len(t) > p for t in toks) and all(t[p] == toks[0][p] for t in toks):
        p += 1
    x = np.zeros((B, T), np.int64)
    att = np.zeros((B, T), np.int64)
    mask = np.zeros((B, S), np.float32)
    first = np.zeros(B, np.int64)  # ячейка перед текстом: туда кладём состояние начала строки
    cut = 0
    for r in range(B):
        t = toks[r] if r < len(toks) else toks[0]
        keep = min(len(t) - 1, T - 1)
        x[r, 0], att[r, 0] = t[0], 1
        x[r, T - keep :], att[r, T - keep :] = t[len(t) - keep :], 1
        first[r] = T - keep - 1
        if r < len(toks):
            tail = len(t) - p
            cut += tail > min(S, keep)
            mask[r, S - min(tail, S, keep) :] = 1 if tail else 0
    return {"ids": x, "att": att, "first": first}, x[:, T - S :], mask, len(toks), cut


def hidden(cb, f):
    h = np.array(cb({"ids": f["ids"], "att": f["att"]})[0])
    h[np.arange(B), f["first"]] = h[:, 0]
    return h


def run_group(cb, ch, f, tgt, mask, S):
    return ch({"h": hidden(cb, f)[:, -S - 1 : -1], "tgt": tgt, "mask": mask})[0]


def tokens(var):
    tk = np.load(DATA / "lmcal_tokens.npz")
    return tk[var + "_ids"], tk[var + "_len"], tk[var + "_grp"]


def groups_of(name):
    data = json.loads((DATA / "lmcal_text.json").read_text(encoding="utf-8"))
    return [i for i, o in enumerate(data) if o["set"] == name]


def convert():
    import nncf
    import torch

    pr = np.load(OUT / "probe.npz")
    body = ov.convert_model(
        torch.jit.load(str(OUT / "body.ts")).eval(),
        example_input=(torch.from_numpy(pr["ids"]), torch.from_numpy(pr["att"])),
        input=[([-1, -1], ov.Type.i64), ([-1, -1], ov.Type.i64)],
    )
    for i, n in enumerate(("ids", "att")):
        body.inputs[i].get_tensor().set_names({n})
    body.outputs[0].get_tensor().set_names({"h"})
    s = pr["tgt"].shape[1]
    head = ov.convert_model(
        torch.jit.load(str(OUT / "head.ts")).eval(),
        example_input=(torch.from_numpy(pr["h"][:, -s - 1 : -1]), torch.from_numpy(pr["tgt"]), torch.from_numpy(pr["mask"])),
        input=[([-1, -1, 768], ov.Type.f32), ([-1, -1], ov.Type.i64), ([-1, -1], ov.Type.f32)],
    )
    for i, n in enumerate(("h", "tgt", "mask")):
        head.inputs[i].get_tensor().set_names({n})
    head.outputs[0].get_tensor().set_names({"score"})
    f32 = {"INFERENCE_PRECISION_HINT": "f32"}
    h = core.compile_model(body, "CPU", f32)({"ids": pr["ids"], "att": pr["att"]})[0]
    got = core.compile_model(head, "CPU", f32)({"h": h[:, -s - 1 : -1], "tgt": pr["tgt"], "mask": pr["mask"]})[0]
    print("сверка с PyTorch, наибольшее расхождение:", float(np.abs(got - pr["ref"]).max()), flush=True)
    ov.save_model(body, OUT / "body_fp16.xml", compress_to_fp16=True)
    ov.save_model(head, OUT / "head_fp16.xml", compress_to_fp16=True)
    for m, name in ((body, "body"), (head, "head")):
        ov.save_model(nncf.compress_weights(m, mode=nncf.CompressWeightsMode.INT8_ASYM), OUT / f"{name}_w8.xml", compress_to_fp16=True)
    # калибровка — на группах набора «открытые словарные»: в отчёт по своим опечаткам он не входит
    ids, lens, grp = tokens("full")
    rng = np.random.default_rng(3)
    ref = core.compile_model(core.read_model(OUT / "body_fp16.xml"), "CPU")
    cal_b, cal_h = [], []
    for g in rng.choice(groups_of("открытые словарные"), 300, replace=False):
        f, tgt, mask, _, _ = build(ids, lens, np.where(grp == g)[0], 40, 24)
        cal_b.append({k: f[k] for k in ("ids", "att")})
        cal_h.append({"h": hidden(ref, f)[:, -25:-1], "tgt": tgt, "mask": mask})
    q = nncf.quantize(core.read_model(OUT / "body_fp16.xml"), nncf.Dataset(cal_b), model_type=nncf.ModelType.TRANSFORMER, subset_size=300)
    ov.save_model(q, OUT / "body_q8.xml", compress_to_fp16=True)
    q = nncf.quantize(core.read_model(OUT / "head_fp16.xml"), nncf.Dataset(cal_h), subset_size=300)
    ov.save_model(q, OUT / "head_q8.xml", compress_to_fp16=True)
    print("готово", flush=True)


def load(body, head, dev, T):
    m = core.read_model(OUT / f"body_{body}.xml")
    m.reshape({"ids": [B, T], "att": [B, T]})
    return core.compile_model(m, dev, {"PERFORMANCE_HINT": "LATENCY"}), core.compile_model(OUT / f"head_{head}.xml", "CPU", {"PERFORMANCE_HINT": "LATENCY"})


def check():
    """Разности хвостовых оценок внутри группы должны совпасть с разностями полных."""
    ids, lens, grp = tokens("full")
    full = np.load(DATA / "lmcal_scores_rugpt3small.npz")["full"]
    cb, ch = load("fp16", "fp16", "CPU", 40)
    diffs, cuts = [], 0
    for g in groups_of("свои held")[::16]:
        f, tgt, mask, n, cut = build(ids, lens, np.where(grp == g)[0], 40, 24)
        cuts += cut
        if cut or lens[grp == g].max() > 40:
            continue
        sc = run_group(cb, ch, f, tgt, mask, 24)[:n]
        diffs.append(float(np.abs((sc - sc[0]) - (full[g, :n] - full[g, 0])).max()))
    diffs.sort()
    # исходные оценки сняты в bfloat16: расхождение в десятые — норма
    print(f"групп {len(diffs)}: расхождение разностей — медиана {diffs[len(diffs) // 2]:.3f}, наибольшее {diffs[-1]:.3f}; строк с обрезанным хвостом: {cuts}")


def bench(body, head):
    held = groups_of("свои held")[::11][:120]
    for var, T, S, title in (("full", 40, 24, "с текстом справа, 16x40, хвост 24"), ("noright", 32, 8, "на границе слова, 16x32, хвост 8")):
        ids, lens, grp = tokens(var)
        feeds = [build(ids, lens, np.where(grp == g)[0], T, S) for g in held]
        for dev in ("CPU", "NPU", "GPU.0"):
            try:
                t0 = time.time()
                cb, ch = load(body, head, dev, T)
                made = time.time() - t0
                rb, rh = cb.create_infer_request(), ch.create_infer_request()
                tb, th = [], []
                for i, (f, tgt, mask, _, _) in enumerate(feeds[:5] + feeds):
                    t0 = time.perf_counter()
                    rb.infer({"ids": f["ids"], "att": f["att"]})
                    t1 = time.perf_counter()
                    h = rb.get_output_tensor(0).data.copy()
                    h[np.arange(B), f["first"]] = h[:, 0]
                    rh.infer({"h": np.ascontiguousarray(h[:, -S - 1 : -1]), "tgt": tgt, "mask": mask})
                    t2 = time.perf_counter()
                    if i >= 5:
                        tb.append((t1 - t0) * 1000)
                        th.append((t2 - t1) * 1000)
                tot = sorted(a + b for a, b in zip(tb, th))
                print(
                    f"{title}; тело {body} на {dev}, голова {head} на CPU: медиана {tot[len(tot) // 2]:.0f} мс, 90 % до {tot[len(tot) * 9 // 10]:.0f} мс "
                    f"(тело {sorted(tb)[len(tb) // 2]:.0f}, голова {sorted(th)[len(th) // 2]:.0f}); сборка {made:.0f} с",
                    flush=True,
                )
            except Exception as e:
                msg = [l for l in str(e).splitlines() if l.strip()]
                print(f"{title}; тело {body} на {dev}: не собралась — {(msg[-1] if msg else '')[:200]}", flush=True)


def score(body, head, dev="CPU"):
    res = {}
    for var, T, S in (("full", 40, 24), ("noright", 32, 8)):
        ids, lens, grp = tokens(var)
        cb, ch = load(body, head, dev, T)
        arr = np.full((int(grp.max()) + 1, 16), -1e9, np.float32)
        start = np.r_[0, np.where(np.diff(grp))[0] + 1, len(grp)]
        t0 = time.time()
        for g in range(len(start) - 1):
            f, tgt, mask, n, _ = build(ids, lens, np.arange(start[g], start[g + 1]), T, S)
            arr[g, :n] = run_group(cb, ch, f, tgt, mask, S)[:n]
        res[var] = arr
        print(f"{body}+{head} {dev} {var}: {time.time() - t0:.0f} с", flush=True)
    np.savez(DATA / f"lmcal_scores_ov-{body}-{head}-{dev.lower()}.npz", **res)


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    cmd = sys.argv[1]
    if cmd == "convert":
        convert()
    elif cmd == "check":
        check()
    elif cmd == "bench":
        bench(sys.argv[2], sys.argv[3])
    else:
        score(*sys.argv[2:])
