ROWS = [
    ("`1234567890-=", "~!@#$%^&*()_+", "ё1234567890-=", "Ё!\"№;%:?*()_+"),
    ("qwertyuiop[]\\", "QWERTYUIOP{}|", "йцукенгшщзхъ\\", "ЙЦУКЕНГШЩЗХЪ/"),
    ("asdfghjkl;'", "ASDFGHJKL:\"", "фывапролджэ", "ФЫВАПРОЛДЖЭ"),
    ("zxcvbnm,./", "ZXCVBNM<>?", "ячсмитьбю.", "ЯЧСМИТЬБЮ,"),
]

EN_TO_RU: dict[str, str] = {}
RU_TO_EN: dict[str, str] = {}

for en_lo, en_hi, ru_lo, ru_hi in ROWS:
    assert len(en_lo) == len(en_hi) == len(ru_lo) == len(ru_hi)
    for e, r in zip(en_lo + en_hi, ru_lo + ru_hi):
        EN_TO_RU[e] = r
        RU_TO_EN.setdefault(r, e)

EN_TO_RU[" "] = " "
RU_TO_EN[" "] = " "


def en_to_ru(text: str) -> str:
    return "".join(EN_TO_RU.get(c, c) for c in text)


def ru_to_en(text: str) -> str:
    return "".join(RU_TO_EN.get(c, c) for c in text)


if __name__ == "__main__":
    assert en_to_ru("ghbdtn vbh") == "привет мир"
    assert ru_to_en("привет, мир!") == "ghbdtn? vbh!"
    assert en_to_ru("Ghbdtn,") == "Приветб"
    assert en_to_ru("ghbdtn.") == "приветю"
    assert en_to_ru("b,") == "иб"
    assert ru_to_en(en_to_ru("qwerty[];',./")) == "qwerty[];',./"
    print("ok")
