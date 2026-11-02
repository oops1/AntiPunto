//! Орфография: словарь, кандидаты (как train/spell_common.py), ранжировщик.

use std::collections::HashMap;
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

fn fnv(s: &[char]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &c in s {
        h ^= c as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn deletes1(w: &[char]) -> Vec<Vec<char>> {
    (0..w.len()).map(|i| w.iter().enumerate().filter(|&(j, _)| j != i).map(|(_, &c)| c).collect()).collect()
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

pub struct Dict {
    words: Vec<String>,
    freq: HashMap<String, u32>,
    /// (хеш удаления, номер слова), отсортировано по хешу.
    deletes: Vec<(u64, u32)>,
}

impl Dict {
    pub fn load(path: &Path) -> Result<Dict, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut words = Vec::new();
        let mut freq = HashMap::new();
        let mut deletes = Vec::new();
        for line in text.lines() {
            let Some((w, c)) = line.split_once('\t') else { continue };
            let id = words.len() as u32;
            let chars: Vec<char> = w.chars().collect();
            for d in deletes1(&chars) {
                deletes.push((fnv(&d), id));
            }
            freq.insert(w.to_string(), c.trim().parse().unwrap_or(1));
            words.push(w.to_string());
        }
        deletes.sort_unstable();
        Ok(Dict { words, freq, deletes })
    }

    pub fn contains(&self, w: &str) -> bool {
        self.freq.contains_key(w)
    }

    fn by_delete(&self, key: &[char]) -> impl Iterator<Item = &str> {
        let h = fnv(key);
        let start = self.deletes.partition_point(|&(x, _)| x < h);
        self.deletes[start..].iter().take_while(move |&&(x, _)| x == h).map(|&(_, i)| self.words[i as usize].as_str())
    }

    /// Словарные слова в пределах ~2 правок, лучшие первыми.
    pub fn candidates(&self, typed: &str) -> Vec<String> {
        let t: Vec<char> = typed.chars().collect();
        let d1 = deletes1(&t);
        let mut found: Vec<&str> = Vec::new();
        for k in std::iter::once(t.clone()).chain(d1.iter().cloned()) {
            let s: String = k.iter().collect();
            if let Some((w, _)) = self.freq.get_key_value(&s) {
                found.push(w);
            }
            found.extend(self.by_delete(&k));
        }
        // две лишние буквы в набранном («черерез» → «через»)
        for k in &d1 {
            for k2 in deletes1(k) {
                let s: String = k2.iter().collect();
                if let Some((w, _)) = self.freq.get_key_value(&s) {
                    found.push(w);
                }
            }
        }
        found.sort_unstable();
        found.dedup();
        let mut scored: Vec<(usize, i64, &str)> = found
            .into_iter()
            .filter(|&w| w != typed)
            .map(|w| (osa(&t, &w.chars().collect::<Vec<_>>()), -(self.freq[w] as i64), w))
            .filter(|&(d, _, _)| d <= 2)
            .collect();
        scored.sort();
        scored.into_iter().take(K - 1).map(|(_, _, w)| w.to_string()).collect()
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
    pub threshold: f32,
}

impl Speller {
    pub fn new(ru: Dict, en: Dict, known: Option<Known>, model: Model, threshold: f32) -> Speller {
        Speller { ru, en, known, model, threshold }
    }

    pub fn dict(&self, ru: bool) -> &Dict {
        if ru { &self.ru } else { &self.en }
    }

    /// Выбор модели и его вероятность (выбор == typed — оставить); None — не спрашивали.
    pub fn suggest(&mut self, ctx: &str, typed: &str, ru: bool) -> Result<Option<(String, f32)>, String> {
        let d = self.dict(ru);
        if d.contains(typed) || self.known.as_ref().is_some_and(|k| k.contains(typed)) {
            return Ok(None);
        }
        let cands = d.candidates(typed);
        if cands.is_empty() {
            return Ok(None);
        }
        let mut x = vec![PAD; K * L];
        let opts: Vec<&str> = std::iter::once(typed).chain(cands.iter().map(String::as_str)).collect();
        let t: Vec<char> = typed.chars().collect();
        x[..L].copy_from_slice(&row(ctx, typed, typed, 0, 0, 0));
        for (j, c) in cands.iter().enumerate() {
            let dist = osa(&t, &c.chars().collect::<Vec<_>>());
            let r = row(ctx, typed, c, j + 1, dist, d.freq[c.as_str()]);
            x[(j + 1) * L..(j + 2) * L].copy_from_slice(&r);
        }
        let mut z = [0f32; K];
        self.model.infer(&[&x], &mut z)?;
        let n = opts.len();
        let max = z[..n].iter().cloned().fold(f32::MIN, f32::max);
        let e: Vec<f32> = z[..n].iter().map(|v| (v - max).exp()).collect();
        let sum: f32 = e.iter().sum();
        let (best, p) =
            e.iter().enumerate().map(|(i, v)| (i, v / sum)).fold((0, 0.0), |a, b| if b.1 > a.1 { b } else { a });
        Ok(Some((opts[best].to_string(), p)))
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
