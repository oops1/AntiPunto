"""Окна для модели раскладки: набор текста с забытым переключением."""

import argparse
import math
import multiprocessing as mp
import os
import random
import re
import string
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import numpy as np

from keys import CHAR_TOK, NEUTRAL, NORMALIZE, OTHER, PAD, SPACE, script
import paths

W = 64
P_OK_START, P_SWITCH, P_NOTICE = 0.5, 0.6, 0.15
MIX = [("ru", 0.33), ("en", 0.28), ("mix", 0.10), ("ins_ru", 0.13), ("tech_ru", 0.08), ("ins_en", 0.03), ("cmd", 0.05)]
# названия, которые пишут латиницей посреди русского текста
TECH = """
GitHub GitLab Bitbucket Git YouTube Google Gmail Chrome Firefox Edge Safari Opera Yandex Telegram WhatsApp Viber
Discord Slack Zoom Skype Teams Outlook Excel Word PowerPoint OneDrive OneNote Notepad Windows Linux Ubuntu Debian
Fedora Android iPhone iPad iOS macOS MacBook Apple Samsung Xiaomi Huawei Honor Realme Sony Nvidia AMD Intel Ryzen
GeForce Radeon Steam Epic Origin Twitch Netflix Spotify Amazon AliExpress Ozon Wildberries Avito eBay PayPal Visa
Mastercard WiFi Bluetooth USB HDMI SSD NVMe RAM CPU GPU NPU BIOS UEFI API JSON YAML XML HTML CSS SQL NoSQL PostgreSQL
MySQL SQLite MongoDB Redis Kafka RabbitMQ Docker Kubernetes Helm Terraform Ansible Jenkins Nginx Apache Python Java
JavaScript TypeScript Rust Golang Kotlin Swift Ruby PHP Perl Scala Haskell Node npm yarn pip conda React Vue Angular
Svelte Django Flask FastAPI Spring Laravel Rails PyTorch TensorFlow CUDA OpenVINO Jupyter VSCode Vim Emacs IntelliJ
PyCharm Visual Studio Xcode Figma Photoshop Illustrator Blender Unity Unreal Minecraft Roblox Fortnite Dota CS
Valorant ChatGPT OpenAI Claude Gemini Copilot LLaMA Ollama Midjourney Stable Diffusion Wikipedia Reddit Twitter
Facebook Instagram TikTok LinkedIn Habr Medium Notion Trello Jira Confluence Miro Dropbox iCloud Proton Cloudflare
AWS Azure Heroku Vercel Netlify DigitalOcean Hetzner Selectel Proxmox VMware VirtualBox Hyper-V WSL PowerShell bash
cmd SSH VPN DNS DHCP TCP UDP HTTP HTTPS IP localhost MikroTik Keenetic TP-Link Asus Lenovo Dell HP Acer MSI Gigabyte
commit push pull merge rebase branch release deploy build debug backend frontend fullstack devops QA README
""".split()
WORD = re.compile(r"[A-Za-zА-Яа-яЁё][\w'-]*")
HEX = "0123456789abcdef"
CORPUS = paths.CORPUS
OUT = paths.LAYOUT


def junk(rng: random.Random, words: list[str]) -> str:
    w = [x.lower() for x in rng.sample(words, min(len(words), 3))]
    r = rng.random()
    if r < 0.2:
        return "".join(rng.choices(HEX, k=rng.randint(6, 12)))
    if r < 0.4:
        return "_".join(w[: rng.randint(1, 3)])
    if r < 0.55:
        return w[0] + "".join(x.capitalize() for x in w[1:])
    if r < 0.7:
        return f"https://{w[0]}.com/" + "/".join(w[1:])
    if r < 0.8:
        return "C:\\" + "\\".join(w) + ".txt"
    if r < 0.9:
        return f"v{rng.randint(0, 9)}.{rng.randint(0, 20)}.{rng.randint(0, 99)}"
    return "".join(rng.choices(string.ascii_letters + string.digits, k=rng.randint(6, 14)))


_GLOSSARY: dict[str, tuple[list[str], list[float]]] = {}


def glossary(src: str) -> tuple[list[str], list[float]]:
    """Термины из build_glossary.py с накопленными весами по частоте."""
    if not _GLOSSARY:
        terms: dict[str, list[tuple[str, float]]] = {"mined": [], "tags": []}
        path = CORPUS / "glossary.tsv"
        if path.exists():
            for line in path.read_text(encoding="utf-8").splitlines():
                s, t, n = line.split("\t")
                terms[s].append((t, math.sqrt(int(n)) if s == "mined" else math.log(int(n) + 1)))
        for s, v in terms.items():
            cum, acc = [], 0.0
            for _, w in v:
                acc += w
                cum.append(acc)
            _GLOSSARY[s] = ([t for t, _ in v], cum)
    return _GLOSSARY[src]


def tech(rng: random.Random) -> str:
    # словарь (v4) пока хуже по английскому в русской раскладке
    if not os.environ.get("ANTIPUNTO_GLOSSARY"):
        t = rng.choice(TECH)
        r = rng.random()
        return t.lower() if r < 0.7 else t if r < 0.9 else t.capitalize()
    r = rng.random()
    src = "curated" if r < 0.25 else "mined" if r < 0.65 else "tags"
    terms, cum = glossary(src) if src != "curated" else ([], [])
    if not terms:
        t = rng.choice(TECH)
    else:
        t = rng.choices(terms, cum_weights=cum)[0]
    if src == "mined":
        return t if rng.random() < 0.8 else t.lower()
    r = rng.random()
    return t.lower() if r < 0.7 else t if r < 0.9 else t.capitalize()


def compose(kind: str, a: str, b: str, rng: random.Random) -> str:
    if kind in ("ru", "en"):
        return a
    if kind == "mix":
        return f"{a} {b}" if rng.random() < 0.5 else f"{b} {a}"
    if kind == "tech_ru":
        # короткая русская фраза, название чаще в конце
        parts = a.split(" ")[: rng.randint(2, 9)]
        pos = len(parts) if rng.random() < 0.6 else rng.randint(0, len(parts))
        parts.insert(pos, tech(rng))
        return " ".join(parts)
    bw = WORD.findall(b) or ["data"]
    if kind == "cmd":
        return " ".join(junk(rng, bw) if rng.random() < 0.5 else rng.choice(bw) for _ in range(rng.randint(1, 4)))
    parts = a.split(" ")
    for _ in range(rng.randint(1, 3)):
        r = rng.random()
        if kind == "ins_ru" and r < 0.3:
            ins = tech(rng)
        elif kind == "ins_ru" and r < 0.6:
            ins = junk(rng, bw)
        else:
            ins = rng.choice(bw)
        parts.insert(rng.randint(0, len(parts)), ins)
    return " ".join(parts)


def simulate(text: str, rng: random.Random):
    """Набор text: токены, раскладка на экране, метка «перевернуть»."""
    text = text.translate(NORMALIZE)
    lays = [script(c) for c in text]
    cur = next((l for l in lays if l is not None), None)
    if cur is None:
        return None
    want = []
    for l in lays:
        if l is not None:
            cur = l
        want.append(cur)

    state = want[0] if rng.random() < P_OK_START else 1 - want[0]
    prev = want[0]
    tok, act, lab = [], [], []
    for c, w in zip(text, want):
        if w != prev:
            if rng.random() < P_SWITCH:
                state = w
            prev = w
        elif c == " " and state != w and rng.random() < P_NOTICE:
            state = w
        t = CHAR_TOK[w].get(c)
        if t is None:
            t = CHAR_TOK[1 - w].get(c)
            if t is None:
                t = OTHER
            else:
                w = 1 - w
        tok.append(t)
        act.append(state)
        lab.append(-1 if t in NEUTRAL else int(state != w))
    return tok, act, lab


def word_starts(tok, e: int, w: int = W) -> int:
    """Начало окна: первое слово, целиком влезающее в w."""
    s = max(0, e - w)
    while 0 < s < e and tok[s - 1] != SPACE:
        s += 1
    while s < e and tok[s] == SPACE:
        s += 1
    if s < e:
        return s
    s = e
    while s > max(0, e - W) and tok[s - 1] != SPACE:
        s -= 1
    return s


def window(tok, act, lab, rng: random.Random):
    n = len(tok)
    ends = [e for e in range(1, n + 1) if (e == n or tok[e] == SPACE) and tok[e - 1] != SPACE]
    if not ends:
        return None
    e = rng.choice(ends)
    span = W if rng.random() < 0.5 else rng.randint(1, min(W, e))
    s = word_starts(tok, e, span)
    if all(x < 0 for x in lab[s:e]):
        return None
    return tok[s:e], act[s:e], lab[s:e]


def work(job):
    seed, items = job
    rng = random.Random(seed)
    K = np.full((len(items), W), PAD, np.uint8)
    A = np.zeros((len(items), W), np.uint8)
    L = np.full((len(items), W), -1, np.int8)
    m = 0
    for kind, a, b in items:
        sim = simulate(compose(kind, a, b, rng), rng)
        win = sim and window(*sim, rng)
        if not win:
            continue
        k, ac, lb = win
        K[m, W - len(k):] = k
        A[m, W - len(k):] = ac
        L[m, W - len(k):] = lb
        m += 1
    return K[:m], A[:m], L[:m]


class Pool:
    """Строки из нескольких источников с весами."""

    def __init__(self, parts: list[tuple[list[str], float]]):
        self.lists = [p for p, _ in parts]
        self.weights = [w for _, w in parts]

    def choice(self, rng: random.Random) -> str:
        return rng.choice(rng.choices(self.lists, self.weights)[0])


def pick(kind: str, ru: Pool, en: Pool, rng: random.Random):
    if kind == "ru":
        return ru.choice(rng), ""
    if kind == "en":
        return en.choice(rng), ""
    if kind == "ins_en":
        return en.choice(rng), ru.choice(rng)
    return ru.choice(rng), en.choice(rng)


def plan(ru, en, n: int, rng: random.Random, chunk: int = 20000):
    kinds, probs = zip(*MIX)
    for c in range(0, n, chunk):
        items = [(k, *pick(k, ru, en, rng)) for k in rng.choices(kinds, probs, k=min(chunk, n - c))]
        yield rng.getrandbits(32), items


SOURCES = {"wiki": ("{lang}.{split}.txt", 0.4), "chat": ("chat_{lang}.{split}.txt", 0.6)}


def load(split: str, sources=("wiki", "chat")) -> list[Pool]:
    pools = []
    for lang in ("ru", "en"):
        parts = []
        for src in sources:
            name, w = SOURCES[src]
            p = CORPUS / name.format(lang=lang, split=split)
            parts.append((p.read_text(encoding="utf-8").splitlines(), w))
        pools.append(Pool(parts))
    return pools


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--split", default="train")
    ap.add_argument("--n", type=int, default=6_000_000)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--procs", type=int, default=14)
    ap.add_argument("--sources", default="wiki,chat")
    ap.add_argument("--glossary", action="store_true", help="термины из build_glossary.py (эксперимент v4)")
    a = ap.parse_args()
    if a.glossary:
        os.environ["ANTIPUNTO_GLOSSARY"] = "1"  # наследуют процессы пула

    ru, en = load(a.split, a.sources.split(","))
    rng = random.Random(a.seed)
    with mp.Pool(a.procs) as pool:
        parts = list(pool.imap(work, plan(ru, en, a.n, rng)))
    K, A, L = (np.concatenate([p[i] for p in parts]) for i in range(3))
    OUT.mkdir(parents=True, exist_ok=True)
    np.savez(OUT / f"{a.split}.npz", tok=K, lay=A, lab=L)
    lab = L[L >= 0]
    print(f"{a.split}: окон {len(K)}, размеченных символов {lab.size}, доля «перевернуть» {lab.mean():.3f}")
