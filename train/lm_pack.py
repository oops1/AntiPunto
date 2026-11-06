"""Пакет языковой модели для клиента: models/Ext/rugpt3small — части и перечень с контрольными суммами.

python lm_pack.py
Берёт сжатые тело и голову (lm_ov.py convert) и файлы токенизатора исходной модели из data/lm.
Клиент качает части, сверяет SHA-256 с перечнем, вшитым в программу, и режет склейку на файлы.
"""

import hashlib
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import json

import numpy as np
import openvino as ov

import paths

SRC = paths.ROOT / "lm"
OUT = Path(__file__).resolve().parent.parent / "models" / "Ext" / "rugpt3small"
NAME = "rugpt3small-1"
PARTS = 4
B, T, S, H = 16, 40, 24, 768  # вариантов, токенов в строке, токенов хвоста, ширина состояния


def fixed(path: Path, shapes: dict, ints: tuple) -> ov.Model:
    """Статические формы и входы i32: так модель годится для NPU и для клиента."""
    m = ov.Core().read_model(path)
    m.reshape(shapes)
    ppp = ov.preprocess.PrePostProcessor(m)
    for name in ints:
        ppp.input(name).tensor().set_element_type(ov.Type.i32)
    return ppp.build()


def sha(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    tok_dir = SRC / "rugpt3small_based_on_gpt2"
    files: list[tuple[str, bytes]] = []
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        body = fixed(SRC / "ov" / "body_q8.xml", {"ids": [B, T], "att": [B, T]}, ("ids", "att"))
        head = fixed(SRC / "ov" / "head_q8.xml", {"h": [B, S, H], "tgt": [B, S], "mask": [B, S]}, ("tgt",))
        ov.save_model(body, tmp / "lm_body.xml", compress_to_fp16=True)
        ov.save_model(head, tmp / "lm_head.xml", compress_to_fp16=True)
        # проверка: упакованные модели собираются и считают
        core = ov.Core()
        ids = np.zeros((B, T), np.int32)
        ids[:, 0] = 1
        att = np.zeros((B, T), np.int32)
        att[:, 0] = 1
        h = core.compile_model(tmp / "lm_body.xml", "CPU")({"ids": ids, "att": att})[0]
        sc = core.compile_model(tmp / "lm_head.xml", "CPU")(
            {"h": h[:, :S], "tgt": np.zeros((B, S), np.int32), "mask": np.ones((B, S), np.float32)}
        )[0]
        assert h.shape == (B, T, H) and sc.shape == (B,) and np.isfinite(sc).all(), (h.shape, sc.shape)
        for name in ("lm_body.xml", "lm_body.bin", "lm_head.xml", "lm_head.bin"):
            files.append((name, (tmp / name).read_bytes()))
    # словарь токенов по номерам, по одному в строке; правила слияния без заголовка
    vocab = json.loads((tok_dir / "vocab.json").read_text(encoding="utf-8"))
    tokens = [""] * len(vocab)
    for t, i in vocab.items():
        assert not any(c.isspace() for c in t), repr(t)
        tokens[i] = t
    assert all(tokens)
    files.append(("lm_vocab.txt", ("\n".join(tokens) + "\n").encode("utf-8")))
    merges = [l for l in (tok_dir / "merges.txt").read_text(encoding="utf-8").split("\n") if l and not l.startswith("#version")]
    files.append(("lm_merges.txt", ("\n".join(merges) + "\n").encode("utf-8")))

    blob = b"".join(b for _, b in files)
    step = -(-len(blob) // PARTS)
    OUT.mkdir(parents=True, exist_ok=True)
    for old in OUT.glob("rugpt3small.part*"):
        old.unlink()
    lines = [
        f"# Языковая модель {NAME}: перечень вшит в программу, по нему проверяются закачка и установленные файлы.",
        f"meta\tname\t{NAME}",
        f"meta\tbos\t{vocab['<s>']}",
        f"meta\tunk\t{vocab['<unk>']}",
        f"meta\trows\t{B}",
        f"meta\ttokens\t{T}",
        f"meta\ttail\t{S}",
        f"meta\thidden\t{H}",
    ]
    for i in range(PARTS):
        part = blob[i * step : (i + 1) * step]
        name = f"rugpt3small.part{i + 1}"
        (OUT / name).write_bytes(part)
        lines.append(f"part\t{name}\t{len(part)}\t{sha(part)}")
    for name, b in files:
        lines.append(f"file\t{name}\t{len(b)}\t{sha(b)}")
    (OUT / "manifest.tsv").write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    print("\n".join(lines))
    print(f"всего {len(blob) / 1e6:.1f} МБ, частей {PARTS} по {step / 1e6:.1f} МБ → {OUT}")
