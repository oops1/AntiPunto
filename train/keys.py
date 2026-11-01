"""Физические клавиши: токен = клавиша + Shift, не зависит от раскладки."""

from layouts import ROWS

EN, RU = 0, 1
PAD, SPACE = 0, 1

KEYS: list[tuple[str, str, str, str]] = []
for row in ROWS:
    KEYS.extend(zip(*row))

OTHER = 2 + 2 * len(KEYS)
VOCAB = OTHER + 1

CHAR_TOK: list[dict[str, int]] = [{" ": SPACE}, {" ": SPACE}]
TOK_CHAR: list[dict[int, str]] = [{SPACE: " "}, {SPACE: " "}]
for k, (el, eh, rl, rh) in enumerate(KEYS):
    for lay, lo, hi in ((EN, el, eh), (RU, rl, rh)):
        for tok, ch in ((2 + 2 * k, lo), (3 + 2 * k, hi)):
            CHAR_TOK[lay].setdefault(ch, tok)
            TOK_CHAR[lay][tok] = ch

# одинаковы в обеих раскладках: цифры, пробел, часть знаков
NEUTRAL = frozenset({SPACE, OTHER} | {t for t, c in TOK_CHAR[EN].items() if TOK_CHAR[RU].get(t) == c})

NORMALIZE = str.maketrans({
    "«": '"', "»": '"', "„": '"', "“": '"', "”": '"', "’": "'", "‘": "'",
    "—": "-", "–": "-", " ": " ", "…": "...",
})


def script(c: str) -> int | None:
    if c.isascii():
        return EN if c.isalpha() else None
    if "а" <= c.lower() <= "я" or c in "ёЁ":
        return RU
    return None


def render(toks, lays) -> str:
    return "".join(TOK_CHAR[l].get(t, "¤") for t, l in zip(toks, lays))
