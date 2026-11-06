"""Языковая модель rugpt3small: выгрузка из PyTorch и оценки кандидатов исходной моделью.

python lm_export.py export   # тело и выходной слой в TorchScript, токены групп — для lm_ov.py
python lm_export.py score    # оценки исходной моделью на видеокарте — для lm_fit.py
Нужен пакет transformers (в учебном окружении проекта его нет — запускать в окружении, где он есть).
Группы (текст вокруг слова и варианты) готовит lm_fit.py dump.
"""

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import numpy as np
import torch
from transformers import AutoModelForCausalLM, AutoTokenizer

import paths

SRC = paths.ROOT / "lm" / "rugpt3small_based_on_gpt2"
OUT = paths.ROOT / "lm" / "ov"
DATA = paths.ROOT / "spell"
K = 16
VARIANTS = (("full", True), ("noright", False))  # с текстом справа и без него


class Body(torch.nn.Module):
    """Скрытые состояния строки «начало, дыра, текст прижат вправо»."""

    def __init__(self, net):
        super().__init__()
        self.net = net

    def forward(self, ids, att):
        # дыра между началом строки и текстом не сдвигает позиции
        pos = (att.cumsum(1) - 1).clamp(min=0)
        return self.net.transformer(input_ids=ids, attention_mask=att, position_ids=pos, use_cache=False).last_hidden_state


class Head(torch.nn.Module):
    """Сумма логарифмов вероятностей токенов хвоста."""

    def __init__(self, net):
        super().__init__()
        self.net = net

    def forward(self, h, tgt, mask):
        lp = torch.log_softmax(self.net.lm_head(h), -1).gather(2, tgt.unsqueeze(2)).squeeze(2)
        return (lp * mask).sum(1)


def rows(data, right: bool):
    texts, grp = [], []
    for i, o in enumerate(data):
        for c in o["opts"]:
            texts.append(o["left"] + c + (o["right"] if right else ""))
            grp.append(i)
    return texts, np.array(grp, np.int32)


def export(tok, data):
    OUT.mkdir(parents=True, exist_ok=True)
    net = AutoModelForCausalLM.from_pretrained(SRC, dtype=torch.float32).eval()
    res = {}
    for name, right in VARIANTS:
        texts, grp = rows(data, right)
        enc = tok(texts, add_special_tokens=False).input_ids
        lens = np.array([len(e) + 1 for e in enc], np.int32)
        ids = np.zeros((len(enc), int(lens.max())), np.int32)
        for k, e in enumerate(enc):
            ids[k, 0] = tok.bos_token_id
            ids[k, 1 : len(e) + 1] = e
        res[name + "_ids"], res[name + "_len"], res[name + "_grp"] = ids, lens, grp
        q = np.sort(lens)
        print(f"{name}: строк {len(lens)}, токенов медиана {q[len(q) // 2]}, 99 % до {q[len(q) * 99 // 100]}, максимум {q[-1]}")
    np.savez(DATA / "lmcal_tokens.npz", **res)

    b, t, s = 16, 40, 24
    g = torch.Generator().manual_seed(1)
    ids = torch.randint(5, 50000, (b, t), generator=g)
    att = torch.ones(b, t, dtype=torch.long)
    att[:, 1:9] = 0
    body, head = Body(net), Head(net)
    with torch.no_grad():
        h = body(ids, att)
        tgt, mask = ids[:, t - s :], torch.ones(b, s)
        ref = head(h[:, t - s - 1 : t - 1], tgt, mask)
    torch.jit.trace(body, (ids, att), check_trace=False).save(str(OUT / "body.ts"))
    torch.jit.trace(head, (h[:, t - s - 1 : t - 1], tgt, mask), check_trace=False).save(str(OUT / "head.ts"))
    np.savez(OUT / "probe.npz", ids=ids.numpy(), att=att.numpy(), h=h.numpy(), tgt=tgt.numpy(), mask=mask.numpy(), ref=ref.numpy())
    print("выгружено в", OUT)


def score(tok, data, budget=1600):
    """Сумма логарифмов вероятностей всей строки: (группа, вариант) → оценка."""
    tok.padding_side = "right"
    if tok.pad_token_id is None:
        tok.pad_token = tok.eos_token
    net = AutoModelForCausalLM.from_pretrained(SRC, dtype=torch.bfloat16).cuda().eval()
    res = {}
    for name, right in VARIANTS:
        texts, grp = rows(data, right)
        lens = [len(t) + 1 for t in tok(texts, add_special_tokens=False).input_ids]
        order = sorted(range(len(texts)), key=lambda i: lens[i])
        out = np.zeros(len(texts), np.float32)
        s = 0
        while s < len(order):
            # пачка по числу токенов: логиты на весь словарь должны помещаться в память карты
            e = s + 1
            while e < len(order) and (e + 1 - s) * lens[order[e]] <= budget:
                e += 1
            idx = order[s:e]
            s = e
            enc = tok([texts[i] for i in idx], add_special_tokens=False, padding=True, return_tensors="pt")
            ids = torch.cat([torch.full((len(idx), 1), tok.bos_token_id), enc.input_ids], 1).cuda()
            att = torch.cat([torch.ones(len(idx), 1, dtype=torch.long), enc.attention_mask], 1).cuda()
            with torch.no_grad():
                lg = net(input_ids=ids, attention_mask=att, use_cache=False).logits[:, :-1].float()
            tgt = ids[:, 1:]
            ll = ((lg.gather(2, tgt.unsqueeze(2)).squeeze(2) - lg.logsumexp(-1)) * att[:, 1:]).sum(1)
            out[idx] = ll.cpu().numpy()
            del lg, ll, tgt, ids, att
            torch.cuda.empty_cache()  # пачки разного размера раздувают запасник
        arr = np.full((len(data), K), -1e9, np.float32)
        col = np.zeros(len(grp), np.int64)
        for k in range(1, len(grp)):
            col[k] = col[k - 1] + 1 if grp[k] == grp[k - 1] else 0
        arr[grp, col] = out
        res[name] = arr
        print(f"{name}: строк {len(texts)}", flush=True)
    np.savez(DATA / "lmcal_scores_rugpt3small.npz", **res)


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    tok = AutoTokenizer.from_pretrained(SRC)
    data = json.loads((DATA / "lmcal_text.json").read_text(encoding="utf-8"))
    {"export": export, "score": score}[sys.argv[1]](tok, data)
