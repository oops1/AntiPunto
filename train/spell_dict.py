"""Частотные словари для орфографии: слова из корпусов, по языку."""

import re
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import paths

WORD = {"ru": re.compile(r"[а-яё]+"), "en": re.compile(r"[a-z]+")}
TOP = {"ru": 300_000, "en": 150_000}
JUNK_RATIO, JUNK_WIKI = 30, 10  # см. junk_rule
OUT = paths.ROOT / "spell"


def junk_rule(known_words: list[str], cnt: Counter, wiki: Counter, known):
    """Опечатка из корпусов: слово не знает OpenCorpora, в Википедии оно редко,
    а в одной правке есть настоящее слово в JUNK_RATIO раз чаще («следущее», «гна», «можеш»)."""
    import spell_jargon
    from spell_common import deletes1, osa

    index: dict[str, list[str]] = {}
    for w in known_words:
        for d in deletes1(w):
            index.setdefault(d, []).append(w)
    freq = {w: cnt[w] for w in known_words}
    jargon = spell_jargon.words()

    def junk(w: str) -> bool:
        if len(w) < 3 or known(w) or w in jargon or wiki[w] >= JUNK_WIKI:
            return False
        near: set[str] = set()
        for k in (w, *deletes1(w)):
            if k in freq:
                near.add(k)
            near.update(index.get(k, ()))
        near.discard(w)
        return any(osa(w, v) == 1 and freq[v] >= JUNK_RATIO * cnt[w] for v in near)

    return junk


def build(lang: str, limit: int = 3_000_000) -> None:
    cnt, wiki = Counter(), Counter()
    for name in (f"{lang}.train.txt", f"chat_{lang}.train.txt"):
        with open(paths.CORPUS / name, encoding="utf-8") as f:
            for i, line in enumerate(f):
                if i >= limit:
                    break
                ws = WORD[lang].findall(line.lower())
                cnt.update(ws)
                if not name.startswith("chat_"):
                    wiki.update(ws)
    known = (lambda w: True)
    if lang == "ru":
        import pymorphy3

        morph = pymorphy3.MorphAnalyzer()
        known = morph.word_is_known
    # в чатах частые опечатки («хотябы», «коментарий») — в словарь только проверенное:
    # знает OpenCorpora, есть в Википедии или очень частое
    ok = lambda w, c: known(w) if lang == "ru" else (wiki[w] >= 3 or c >= 200)
    words = [(w, c) for w, c in cnt.most_common() if c >= 3 and len(w) <= 24 and (ok(w, c) or wiki[w] >= 3)]
    words = words[: TOP[lang]]
    if lang == "ru":
        junk = junk_rule([w for w, _ in words if known(w)], cnt, wiki, known)
    else:
        # без морфологии: «настоящее» — есть в списке английских слов (en_wordlist.txt) или часто
        # в Википедии; чёрный список — частые ошибки Википедии; частое в корпусах (≥ 200) не трогаем:
        # «mustn», «havin» из субтитров клиент видит как слова
        bad, wordlist = set(), set()
        if (OUT / "en_misspellings.txt").exists():
            bad = {l.split("->")[0].strip().lower() for l in (OUT / "en_misspellings.txt").read_text(encoding="utf-8").splitlines() if "->" in l}
        if (OUT / "en_wordlist.txt").exists():
            wordlist = set((OUT / "en_wordlist.txt").read_text(encoding="utf-8").split())
        sure = lambda w: (w in wordlist or wiki[w] >= JUNK_WIKI) and w not in bad
        rule = junk_rule([w for w, _ in words if sure(w)], cnt, wiki, sure)
        junk = lambda w: w in bad or (cnt[w] < 200 and rule(w))
    words = [(w, c) for w, c in words if not junk(w)]
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / f"{lang}.tsv").write_text("".join(f"{w}\t{c}\n" for w, c in words), encoding="utf-8")
    if lang == "ru":
        import spell_forms
        import spell_jargon

        spell_forms.add(OUT / "ru.tsv")
        spell_jargon.add(OUT / "ru.tsv")
    # встречалось ≥5 раз, но не в словаре (жаргон, неологизмы): не исправлять
    in_dict = {w for w, _ in words}
    seen = [w for w, c in cnt.items() if c >= 5 and w not in in_dict and not junk(w)]
    (OUT / f"{lang}_seen.txt").write_text("\n".join(seen), encoding="utf-8")
    # редкие правильные слова вне словаря: учим их не трогать.
    # В чатах среди редких много опечаток — берём только проверенные.
    rare = [w for w, c in cnt.items() if c in (1, 2) and 4 <= len(w) <= 16 and wiki[w]]
    if lang == "ru":
        rare = [w for w in rare if known(w)]
    (OUT / f"{lang}_rare.txt").write_text("\n".join(rare), encoding="utf-8")
    print(f"{lang}: словарь {len(words)}, редких проверенных {len(rare)}", flush=True)


if __name__ == "__main__":
    for lang in ("ru", "en"):
        build(lang)
