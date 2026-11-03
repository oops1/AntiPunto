//! Орфография: словарь, кандидаты (как train/spell_common.py), ранжировщик.

use std::path::Path;

use crate::ov::Model;

pub const K: usize = 16;
const CTX: usize = 40;
const WLEN: usize = 20;
pub const L: usize = CTX + 2 * WLEN + 2 + 3;
const PAD: i32 = 0;
const SEP: i32 = 1;
const UNK: i32 = 2;
const CHARS: &str = " абвгдеёжзийклмнопрстуфхцчшщъыьэюяabcdefghijklmnopqrstuvwxyz0123456789.,!?;:-'\"()";
const N_CHARS: i32 = 81;
/// Признаки кандидата: место в списке, расстояние правки, корзина частоты.
const F_RANK: i32 = N_CHARS + 3;
const F_DIST: i32 = F_RANK + K as i32;
const F_FREQ: i32 = F_DIST + 3;

fn char_id(c: char) -> i32 {
    CHARS.chars().position(|x| x == c).map_or(UNK, |i| i as i32 + 3)
}

/// 0 — нет в словаре; дальше по логарифму частоты (как freq_bucket в spell_common.py).
fn freq_bucket(f: u32) -> i32 {
    if f == 0 { 0 } else { (1 + ((f as f64).log2() / 1.5) as i32).min(15) }
}

const ALPHA_RU: &str = "абвгдеёжзийклмнопрстуфхцчшщъыьэюя";
const ALPHA_EN: &str = "abcdefghijklmnopqrstuvwxyz";
/// Однобуквенные слова, допустимые как часть разбивки слитного набора.
const SINGLE_RU: &str = "авикосуя";
const SINGLE_EN: &str = "ai";
/// Клавиши над пробелом: задели вместо него («приветькак»).
const SPACE_RU: &str = "чсмитьб";
const SPACE_EN: &str = "xcvbnm";
/// Разбивок среди кандидатов не больше.
const SPLITS: usize = 3;
/// Часть разбивки до 3 букв — только частое слово.
const SHORT_FREQ: u32 = 8000;

/// FNV-1a по символам, без символа skip (для удалений без копирования).
fn fnv_skip(s: &[char], skip: usize) -> u32 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (i, &c) in s.iter().enumerate() {
        if i != skip {
            h ^= c as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    (h ^ (h >> 32)) as u32
}

fn fnv(s: &[char]) -> u32 {
    fnv_skip(s, usize::MAX)
}

/// Все строки в одной правке: удаление, перестановка, замена, вставка (как edits1 в Python).
fn edits1(w: &[char], alpha: &[char]) -> Vec<Vec<char>> {
    let mut out = Vec::with_capacity(alpha.len() * (2 * w.len() + 1) + 2 * w.len());
    for i in 0..w.len() {
        let mut v = w.to_vec();
        v.remove(i);
        out.push(v);
    }
    for i in 0..w.len().saturating_sub(1) {
        let mut v = w.to_vec();
        v.swap(i, i + 1);
        out.push(v);
    }
    for i in 0..=w.len() {
        for &c in alpha {
            let mut v = w.to_vec();
            v.insert(i, c);
            out.push(v);
            if i < w.len() && w[i] != c {
                let mut v = w.to_vec();
                v[i] = c;
                out.push(v);
            }
        }
    }
    out
}

/// Расстояние Дамерау–Левенштейна (оптимальное выравнивание строк).
pub fn osa(a: &[char], b: &[char]) -> usize {
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, v) in d[0].iter_mut().enumerate() {
        *v = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let c = (a[i - 1] != b[j - 1]) as usize;
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + c);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

/// (хеш, номер слова) по хешу + корзины по старшим битам: поиск за два обращения к памяти.
struct Index {
    items: Vec<(u32, u32)>,
    heads: Vec<u32>,
    shift: u32,
}

impl Index {
    fn new(mut items: Vec<(u32, u32)>) -> Index {
        items.sort_unstable();
        items.shrink_to_fit();
        let bits = (usize::BITS - items.len().max(256).leading_zeros()).min(24);
        let shift = 32 - bits;
        let mut heads = vec![0u32; (1 << bits) + 1];
        for &(h, _) in &items {
            heads[(h >> shift) as usize + 1] += 1;
        }
        for b in 1..heads.len() {
            heads[b] += heads[b - 1];
        }
        Index { items, heads, shift }
    }

    fn get(&self, h: u32) -> impl Iterator<Item = u32> + '_ {
        let b = (h >> self.shift) as usize;
        let slot = &self.items[self.heads[b] as usize..self.heads[b + 1] as usize];
        slot.iter().filter(move |&&(x, _)| x == h).map(|&(_, i)| i)
    }
}

/// Словарь: слова одной строкой, индексы по 32-битным хешам (ложные совпадения отсекает osa).
pub struct Dict {
    text: String,
    /// Начало слова i в text; последний элемент — конец.
    offs: Vec<u32>,
    freq: Vec<u32>,
    exact: Index,
    /// Хеши слов без одной буквы.
    deletes: Index,
    alpha: Vec<char>,
    single: Vec<char>,
    space_keys: Vec<char>,
}

impl Dict {
    pub fn load(path: &Path, ru: bool) -> Result<Dict, String> {
        let src = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let (mut text, mut offs, mut freq) = (String::new(), vec![0u32], Vec::new());
        let (mut exact, mut deletes) = (Vec::new(), Vec::new());
        for line in src.lines() {
            let Some((w, c)) = line.split_once('\t') else { continue };
            let id = freq.len() as u32;
            let chars: Vec<char> = w.chars().collect();
            exact.push((fnv(&chars), id));
            let mut ds: Vec<u32> = (0..chars.len()).map(|i| fnv_skip(&chars, i)).collect();
            ds.sort_unstable();
            ds.dedup();
            deletes.extend(ds.into_iter().map(|h| (h, id)));
            freq.push(c.trim().parse().unwrap_or(1));
            text.push_str(w);
            offs.push(text.len() as u32);
        }
        text.shrink_to_fit();
        let pick = |a: &str, b: &str| if ru { a } else { b }.chars().collect();
        Ok(Dict {
            text,
            offs,
            freq,
            exact: Index::new(exact),
            deletes: Index::new(deletes),
            alpha: pick(ALPHA_RU, ALPHA_EN),
            single: pick(SINGLE_RU, SINGLE_EN),
            space_keys: pick(SPACE_RU, SPACE_EN),
        })
    }

    fn word(&self, i: u32) -> &str {
        &self.text[self.offs[i as usize] as usize..self.offs[i as usize + 1] as usize]
    }

    fn id(&self, w: &str) -> Option<u32> {
        let chars: Vec<char> = w.chars().collect();
        self.exact.get(fnv(&chars)).find(|&i| self.word(i) == w)
    }

    pub fn contains(&self, w: &str) -> bool {
        self.id(w).is_some()
    }

    /// Частота слова; у разбивки «в чем» — меньшая из частей.
    pub fn freq_of(&self, w: &str) -> u32 {
        w.split(' ').map(|p| self.id(p).map_or(0, |i| self.freq[i as usize])).min().unwrap_or(0)
    }

    fn part_ok(&self, w: &[char], f: u32) -> bool {
        match w.len() {
            1 => self.single.contains(&w[0]),
            2 | 3 => f >= SHORT_FREQ,
            _ => true,
        }
    }

    /// Часть разбивки: само слово (0 правок) или два самых частых в одной правке (как part в Python).
    fn part(&self, p: &[char]) -> Vec<(u32, usize)> {
        let s: String = p.iter().collect();
        if let Some(i) = self.id(&s) {
            return if self.part_ok(p, self.freq[i as usize]) { vec![(i, 0)] } else { Vec::new() };
        }
        if p.len() < 3 {
            return Vec::new();
        }
        let mut found = Vec::new();
        self.near(p, &mut found);
        found.sort_unstable();
        found.dedup();
        let mut best: Vec<(i64, &str, u32)> = found
            .into_iter()
            .filter_map(|i| {
                let w = self.word(i);
                let wc: Vec<char> = w.chars().collect();
                let f = self.freq[i as usize];
                (wc.len() > 1 && self.part_ok(&wc, f) && osa(p, &wc) == 1).then_some((-(f as i64), w, i))
            })
            .collect();
        best.sort();
        best.into_iter().take(2).map(|(_, _, i)| (i, 1)).collect()
    }

    /// Пропущенный пробел: «вчем» → «в чем»; пробел мог стать соседней буквой («приветькак»).
    fn splits(&self, t: &[char]) -> Vec<String> {
        let mut out = Vec::new();
        for i in 1..t.len() {
            for skip in 0..2 {
                if skip == 1 && (i + 1 >= t.len() || !self.space_keys.contains(&t[i])) {
                    continue;
                }
                let rights = self.part(&t[i + skip..]);
                if rights.is_empty() {
                    continue;
                }
                for (l, le) in self.part(&t[..i]) {
                    for &(r, re) in &rights {
                        if le + re + skip <= 2 {
                            out.push(format!("{} {}", self.word(l), self.word(r)));
                        }
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Номера слов в одной правке от v (удаление с каждой стороны); с лишними — их отсечёт osa.
    fn near(&self, v: &[char], found: &mut Vec<u32>) {
        for skip in std::iter::once(usize::MAX).chain(0..v.len()) {
            let h = fnv_skip(v, skip);
            found.extend(self.exact.get(h));
            found.extend(self.deletes.get(h));
        }
    }

    /// Словарные слова в пределах 2 правок и до SPLITS разбивок: (расстояние, −частота, слово), лучшие первыми.
    pub fn candidates(&self, typed: &str, splits: bool) -> Vec<String> {
        let t: Vec<char> = typed.chars().collect();
        let mut found = Vec::new();
        self.near(&t, &mut found);
        for v in edits1(&t, &self.alpha) {
            self.near(&v, &mut found);
        }
        found.sort_unstable();
        found.dedup();
        let mut words: Vec<(usize, i64, String)> = found
            .into_iter()
            .map(|i| (i, self.word(i)))
            .filter(|&(_, w)| w != typed)
            .map(|(i, w)| (osa(&t, &w.chars().collect::<Vec<_>>()), -(self.freq[i as usize] as i64), w.to_string()))
            .filter(|&(d, _, _)| d <= 2)
            .collect();
        words.sort();
        // разбивка — это ещё и вставленный пробел: ей допускаем на правку больше
        let all = if splits { self.splits(&t) } else { Vec::new() };
        let mut parts: Vec<(usize, i64, String)> = all
            .into_iter()
            .map(|s| (osa(&t, &s.chars().collect::<Vec<_>>()), -(self.freq_of(&s) as i64), s))
            .filter(|&(d, _, _)| d <= 3)
            .collect();
        parts.sort();
        parts.truncate(SPLITS);
        words.truncate(K - 1 - parts.len());
        words.extend(parts);
        words.sort();
        words.into_iter().map(|(_, _, w)| w).collect()
    }
}

/// Фильтр Блума словоформ (train/spell_known.py): известное слово — не опечатка.
pub struct Known {
    k: u32,
    m: u64,
    bits: Vec<u8>,
}

fn fnv_bytes(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| (h ^ x as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

impl Known {
    pub fn load(path: &Path) -> Result<Known, String> {
        let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if b.len() < 16 || &b[..4] != b"APBL" {
            return Err(format!("{}: не фильтр Блума", path.display()));
        }
        let k = u32::from_le_bytes(b[4..8].try_into().unwrap());
        let m = u64::from_le_bytes(b[8..16].try_into().unwrap());
        Ok(Known { k, m, bits: b[16..].to_vec() })
    }

    pub fn contains(&self, w: &str) -> bool {
        let w = w.to_lowercase().replace('ё', "е");
        let h1 = fnv_bytes(w.as_bytes());
        let h2 = mix(h1) | 1;
        (0..self.k as u64).all(|i| {
            let j = h1.wrapping_add(i.wrapping_mul(h2)) % self.m;
            self.bits[(j >> 3) as usize] & (1 << (j & 7)) != 0
        })
    }
}

fn encode_into(out: &mut [i32], s: &str, n: usize) -> usize {
    let mut k = 0;
    for c in s.chars().take(n) {
        out[k] = char_id(c);
        k += 1;
    }
    k
}

/// Строка «контекст | набранное | кандидат | признаки», как row() в spell_common.py.
pub fn row(ctx: &str, typed: &str, cand: &str, rank: usize, dist: usize, freq: u32) -> [i32; L] {
    let mut out = [PAD; L];
    out[L - 3] = F_RANK + rank as i32;
    out[L - 2] = F_DIST + dist.min(2) as i32;
    out[L - 1] = F_FREQ + freq_bucket(freq);
    let lower: Vec<char> = ctx.to_lowercase().chars().collect();
    let tail: String = lower[lower.len().saturating_sub(CTX)..].iter().collect();
    let mut buf = [PAD; CTX];
    let n = encode_into(&mut buf, &tail, CTX);
    out[CTX - n..CTX].copy_from_slice(&buf[..n]);
    out[CTX] = SEP;
    encode_into(&mut out[CTX + 1..CTX + 1 + WLEN], typed, WLEN);
    out[CTX + 1 + WLEN] = SEP;
    encode_into(&mut out[CTX + 2 + WLEN..], cand, WLEN);
    out
}

pub struct Speller {
    pub ru: Dict,
    pub en: Dict,
    /// Словоформы OpenCorpora и частые слова корпусов: такие не исправляем.
    pub known: Option<Known>,
    model: Model,
    /// Второй проход: разбивки там, где первый не уверен.
    split: Option<Model>,
    pub threshold: f32,
}

impl Speller {
    pub fn new(
        ru: Dict,
        en: Dict,
        known: Option<Known>,
        model: Model,
        split: Option<Model>,
        threshold: f32,
    ) -> Speller {
        Speller { ru, en, known, model, split, threshold }
    }

    pub fn dict(&self, ru: bool) -> &Dict {
        if ru { &self.ru } else { &self.en }
    }

    /// Выбор модели и его вероятность (выбор == typed — оставить); None — не спрашивали.
    pub fn suggest(&mut self, ctx: &str, typed: &str, ru: bool) -> Result<Option<(String, f32)>, String> {
        Ok(self.rank(ctx, typed, ru)?.map(|mut v| v.swap_remove(0)))
    }

    /// Все варианты с вероятностями, лучший первым; typed — «оставить».
    pub fn rank(&mut self, ctx: &str, typed: &str, ru: bool) -> Result<Option<Vec<(String, f32)>>, String> {
        // поля по отдельности: модель занимаем мутабельно
        let d = if ru { &self.ru } else { &self.en };
        if d.contains(typed) || self.known.as_ref().is_some_and(|k| k.contains(typed)) {
            return Ok(None);
        }
        let cands = d.candidates(typed, false);
        let first = if cands.is_empty() { None } else { Some(Self::score(d, &mut self.model, ctx, typed, &cands)?) };
        if first.as_ref().is_some_and(|v| v[0].0 != typed && v[0].1 >= self.threshold) {
            return Ok(first);
        }
        let Some(m) = self.split.as_mut() else { return Ok(first) };
        let all = d.candidates(typed, true);
        if !all.iter().any(|c| c.contains(' ')) {
            return Ok(first);
        }
        let second = Self::score(d, m, ctx, typed, &all)?;
        // второй проход отвечает только разбивкой
        Ok(if second[0].0.contains(' ') { Some(second) } else { first })
    }

    fn score(
        d: &Dict,
        model: &mut Model,
        ctx: &str,
        typed: &str,
        cands: &[String],
    ) -> Result<Vec<(String, f32)>, String> {
        let mut x = vec![PAD; K * L];
        let opts: Vec<&str> = std::iter::once(typed).chain(cands.iter().map(String::as_str)).collect();
        let t: Vec<char> = typed.chars().collect();
        x[..L].copy_from_slice(&row(ctx, typed, typed, 0, 0, 0));
        for (j, c) in cands.iter().enumerate() {
            let dist = osa(&t, &c.chars().collect::<Vec<_>>());
            let r = row(ctx, typed, c, j + 1, dist, d.freq_of(c));
            x[(j + 1) * L..(j + 2) * L].copy_from_slice(&r);
        }
        let mut z = [0f32; K];
        model.infer(&[&x], &mut z)?;
        let n = opts.len();
        let max = z[..n].iter().cloned().fold(f32::MIN, f32::max);
        let e: Vec<f32> = z[..n].iter().map(|v| (v - max).exp()).collect();
        let sum: f32 = e.iter().sum();
        let mut v: Vec<(String, f32)> = opts.iter().zip(&e).map(|(o, x)| (o.to_string(), x / sum)).collect();
        // устойчивая: при равенстве выигрывает «оставить», как раньше
        v.sort_by(|a, b| b.1.total_cmp(&a.1));
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osa_distance() {
        let c = |s: &str| s.chars().collect::<Vec<_>>();
        assert_eq!(osa(&c("преокт"), &c("проект")), 1);
        assert_eq!(osa(&c("черерез"), &c("через")), 2);
        assert_eq!(osa(&c("abc"), &c("abc")), 0);
    }

    #[test]
    fn known_words_bloom_matches_python() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../models/spell_known.bloom");
        let k = Known::load(&path).expect("models/spell_known.bloom");
        for w in ["вентральным", "окапываются", "мастями", "сохранись", "ёжик", "ежик", "гитхаб", "щас"]
        {
            assert!(k.contains(w), "{w}");
        }
        for w in ["удететь", "преокт", "черерез", "которобго"] {
            assert!(!k.contains(w), "{w}");
        }
    }

    /// Кандидаты как в Python: tests/spell_candidates.tsv пишет train/spell_export.py.
    #[test]
    fn candidates_match_python() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let fixture = std::fs::read_to_string(root.join("tests/spell_candidates.tsv")).expect("spell_candidates.tsv");
        let ru = Dict::load(&root.join("../models/spell_ru.tsv"), true).unwrap();
        let en = Dict::load(&root.join("../models/spell_en.tsv"), false).unwrap();
        let mut n = 0;
        for line in fixture.lines() {
            let mut parts = line.split('\t');
            let (lang, typed) = (parts.next().unwrap(), parts.next().unwrap());
            let want: Vec<&str> = parts.next().unwrap_or("").split('|').filter(|s| !s.is_empty()).collect();
            let d = if lang == "ru" { &ru } else { &en };
            assert_eq!(d.candidates(typed, true), want, "{typed}");
            n += 1;
        }
        assert!(n >= 50, "мало примеров: {n}");
    }

    #[test]
    #[ignore]
    fn candidates_timing() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let ru = Dict::load(&root.join("../models/spell_ru.tsv"), true).unwrap();
        for w in ["подключаюсб", "коментари", "сесию", "видеш", "артифакты", "рабоате"]
        {
            let t: Vec<char> = w.chars().collect();
            let t0 = std::time::Instant::now();
            let mut found = Vec::new();
            ru.near(&t, &mut found);
            for v in edits1(&t, &ru.alpha) {
                ru.near(&v, &mut found);
            }
            let t1 = t0.elapsed();
            let n = found.len();
            let c = ru.candidates(w, true);
            println!("{w}: найдено {n}, поиск {t1:?}, всего {:?}, {}", t0.elapsed(), c.len());
        }
    }

    #[test]
    fn alphabet_size_matches_python() {
        assert_eq!(CHARS.chars().count() as i32, N_CHARS);
    }

    #[test]
    fn row_layout() {
        let r = row("я хочу ", "удететь", "улететь", 1, 1, 1000);
        assert_eq!((r[L - 3], r[L - 2], r[L - 1]), (F_RANK + 1, F_DIST + 1, F_FREQ + freq_bucket(1000)));
        assert_eq!(r[CTX], SEP);
        assert_eq!(r[CTX + 1], char_id('у'));
        assert_eq!(r[CTX + 1 + WLEN], SEP);
        assert_eq!(r[CTX + 2 + WLEN], char_id('у'));
        assert_eq!(r[CTX - 1], char_id(' '));
    }
}
