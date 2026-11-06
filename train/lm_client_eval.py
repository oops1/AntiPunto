"""Сквозная проверка клиента на настоящих опечатках: с языковой моделью и без неё.

python lm_client_eval.py <antipunto.exe> <папка openvino> <папка языковой модели>
Клиент сам строит кандидатов, считает ранжировщик и языковую модель и решает в две стадии
(--ranktest); здесь только сверка решений с разметкой.
"""

import subprocess
import sys
import tempfile
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import spell_finetune as ft
import spell_real_eval as real

ROOT = Path(__file__).resolve().parent.parent


def clean(s: str) -> str:
    return s.replace("\t", " ").replace("\n", " ").replace("\r", " ")


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    exe, ov, lm_dir = sys.argv[1:4]
    mine = real.load_cases()
    opened = [c for c in ft.open_cases() if c[3]]
    sets = {
        "свои отложенные": [(w, t, ctx, r) for w, t, ctx, _, r in mine if zlib.crc32(w.encode()) % 2 == 1 and r],
        "открытые отложенные": [c for c in opened if zlib.crc32(c[0].encode()) % 10 == 0],
        "открытые словарные": [c for c in ft.open_cases("ru", "словарное") if c[1] and c[3] and c[0].replace("ё", "е") != c[1].replace("ё", "е")],
    }
    cases = [(name, *c) for name, cs in sets.items() for c in cs]
    with tempfile.TemporaryDirectory() as tmp:
        src = Path(tmp) / "cases.tsv"
        src.write_text("".join(f"{w}\t{clean(ctx)}\t{clean(r)}\n" for _, w, _, ctx, r in cases), encoding="utf-8", newline="\n")
        base = [exe, "--ov", ov, "--model", str(ROOT / "models" / "layout.xml"), "--lm-dir", lm_dir, "--ranktest", str(src)]
        for title, extra in (("без языковой модели", ["--no-lm"]), ("с языковой моделью", [])):
            run = subprocess.run(base + extra, capture_output=True)
            out = run.stdout.decode("utf-8").splitlines()
            print(f"== {title}: {run.stderr.decode('utf-8', 'replace').strip().splitlines()[-1]}")
            assert len(out) == len(cases), (len(out), len(cases), run.stderr.decode("utf-8", "replace")[-500:])
            for name in sets:
                good = bad = late = total = errors = 0
                for (s, w, t, _, _), line in zip(cases, out):
                    if s != name:
                        continue
                    word, fix, stage, _ = line.split("\t")
                    assert word == w, (word, w)
                    total += 1
                    errors += bool(t)
                    if fix != "-":
                        ok = bool(t) and fix.replace("ё", "е") == t.replace("ё", "е")
                        good += ok
                        bad += not ok
                        late += stage == "2"
                print(f"  {name}: случаев {total}, ошибок {errors} | верных правок {good}, плохих {bad}, из них после следующего слова {late}")
