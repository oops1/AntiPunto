"""Экспорт ранжировщика в OpenVINO IR [K, L] → [K] и копия словарей в models/."""

import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import numpy as np
import openvino as ov
import torch

import paths
from spell_common import K, L, widen
from spell_model import load_net

DATA = paths.ROOT / "spell"
MODELS = Path(__file__).resolve().parent.parent / "models"


class Scores(torch.nn.Module):
    def __init__(self, m):
        super().__init__()
        self.m = m

    def forward(self, x):
        return self.m(x.long())


def main():
    torch.backends.mha.set_fastpath_enabled(False)
    net = load_net(DATA / "spell.pt")
    wrap = Scores(net).eval()
    ovm = ov.convert_model(wrap, example_input=torch.zeros(K, L, dtype=torch.int32), input=[([K, L], ov.Type.i32)])
    ov.save_model(ovm, DATA / "spell.xml", compress_to_fp16=True)

    d = np.load(DATA / "valid.npz")
    x = widen(d["x"][:50]).astype(np.int32)
    core = ov.Core()
    for dev in ("CPU", "NPU"):
        cm = core.compile_model(DATA / "spell.xml", dev, {"INFERENCE_PRECISION_HINT": "f32"} if dev == "CPU" else {})
        same = 0
        for g in x:
            with torch.no_grad():
                ref = wrap(torch.from_numpy(g)).numpy()
            got = cm([g])[0]
            same += int(ref.argmax() == got.argmax())
        print(f"{dev}: совпадение выбора с torch {same}/{len(x)}")

    MODELS.mkdir(exist_ok=True)
    for name in ("spell.xml", "spell.bin"):
        shutil.copy(DATA / name, MODELS / name)
    for lang in ("ru", "en"):
        shutil.copy(DATA / f"{lang}.tsv", MODELS / f"spell_{lang}.tsv")
    print("скопировано в", MODELS)
    write_fixture()


def write_fixture():
    """Кандидаты Python для теста candidates_match_python в клиенте."""
    import random

    from spell_common import Dictionary, corrupt

    rng = random.Random(7)
    fixed = {
        "ru": ["преокт", "магазн", "пониаю", "черерез", "видеш", "коментари", "каежстя", "сесию", "подключаюсб",
               "удететь", "пливет", "езжик", "рабоате", "ёжк", "вчем", "тутже", "выпустирелиз", "неблоы",
               "воригинае", "приветькак", "проверьналичие", "модлельвклчюить"],
        "en": ["problme", "teh", "recieve", "godo", "fiel", "ithink", "thisis", "goodidea"],
    }
    # словарные слова: ошибка, совпавшая со словом; вариант с одной лишь ё — не кандидат
    in_dict = {"ru": ["бела", "пне", "еще", "все", "была", "чт", "кога", "тоже", "ветке"], "en": ["form", "than", "wether", "cant"]}
    lines = []
    for lang, n in (("ru", 60), ("en", 30)):
        d = Dictionary(lang)
        words = fixed[lang] + [corrupt(w, lang, rng) for w in rng.sample(d.words[:20000], n)]
        words = [w for w in words if w not in d.freq] + in_dict[lang] + rng.sample(d.words[:50000], 20)
        # «|» между кандидатами: в разбивках есть пробел
        lines += [f"{lang}\t{w}\t{'|'.join(d.candidates(w))}" for w in words]
    out = Path(__file__).resolve().parent.parent / "client" / "tests" / "spell_candidates.tsv"
    out.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print("фикстура кандидатов:", len(lines))


if __name__ == "__main__":
    main()
