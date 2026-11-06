"""Образцы для теста токенизатора клиента: client/tests/lm_tokens.tsv (текст, номера токенов).

Запускать в окружении с transformers; тексты — из общедоступных образцов кандидатов и ручных строк.
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from transformers import AutoTokenizer

import paths

ROOT = Path(__file__).resolve().parent.parent
HAND = [
    "я хочу улететь домой",
    "Привет, как дела? Всё хорошо!",
    "ёжик в тумане — это мультфильм 1975 года",
    "it's what we've done, isn't it",
    "git commit -m \"fix: опечатка\"",
    "два  пробела   и ещё    четыре",
    "пробел в конце ",
    " пробел в начале",
    "хвост из пробелов   ",
    "табуляция\tвнутри\tстроки",
    "перенос\nстроки и\n\nдва",
    "цена 1 250,50 руб. (скидка 15%)",
    "e-mail: user@example.com, сайт https://example.com/a?b=1",
    "СЛОВА ПРОПИСНЫМИ и СмЕшАнНыЕ",
    "в чем дело",
    "смайлик 🙂 и стрелка → тут",
    "'кавычки' и д'Артаньян",
    "x",
    "",
]


def esc(s: str) -> str:
    return s.replace("\\", "\\\\").replace("\t", "\\t").replace("\n", "\\n")


if __name__ == "__main__":
    tok = AutoTokenizer.from_pretrained(paths.ROOT / "lm" / "rugpt3small_based_on_gpt2")
    texts = list(HAND)
    for line in (ROOT / "client" / "tests" / "spell_candidates.tsv").read_text(encoding="utf-8").splitlines()[:60]:
        _, typed, cands = (line.split("\t") + [""])[:3]
        for c in [typed] + cands.split("|")[:3]:
            if c:
                texts.append(f"вот тут {c} и дальше")
    out = [f"{esc(t)}\t{' '.join(map(str, tok(t, add_special_tokens=False).input_ids))}" for t in texts]
    (ROOT / "client" / "tests" / "lm_tokens.tsv").write_text("\n".join(out) + "\n", encoding="utf-8", newline="\n")
    print(len(out), "строк")
