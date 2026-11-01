//! Буфер текущего набора и решения модели.

use crate::keymap::{KEYMAP, OTHER, PAD, SPACE};
use crate::ov::{Model, W};

const MAX_LEN: usize = 256;
/// Позже можно перевернуть слово, если на своей границе было выше.
const RETRO_MIN: f32 = 0.3;
/// Короткие слова решаются только по контексту.
const SHORT: usize = 2;

/// Настоящие короткие слова: такие не переворачиваем.
const SHORT_EN: &[&str] = &[
    "a", "i", "am", "an", "as", "at", "be", "by", "do", "go", "he", "hi", "if", "in", "is", "it", "me", "my", "no",
    "of", "oh", "ok", "on", "or", "so", "to", "up", "us", "we", "vs", "tv", "pc", "id", "ui", "os", "js", "db",
];
const SHORT_RU: &[&str] = &[
    "а", "б", "в", "ж", "и", "к", "о", "с", "у", "я", "ад", "ай", "ах", "бы", "во", "вы", "да", "до", "ее", "её", "ей",
    "же", "за", "из", "им", "их", "ко", "ли", "мы", "на", "не", "ни", "но", "ну", "об", "ой", "он", "от", "ох", "по",
    "со", "то", "ты", "уж", "эх", "юг", "яд",
];

fn real_short_word(word: &[Ch]) -> bool {
    let w: String = word.iter().map(Ch::render).filter(|c| c.is_alphabetic()).flat_map(char::to_lowercase).collect();
    let lay = word.iter().find(|c| !KEYMAP.neutral(c.tok)).map_or(0, |c| c.lay);
    let list = if lay == crate::keymap::RU { SHORT_RU } else { SHORT_EN };
    list.contains(&w.as_str())
}

#[derive(Clone, Copy)]
pub struct Ch {
    pub tok: u8,
    /// Раскладка на экране (после правок).
    pub lay: u8,
    /// Раскладка в момент нажатия: её видит модель.
    pub typed: u8,
    /// Символ для OTHER (цифровой блок).
    pub raw: char,
}

impl Ch {
    pub fn new(tok: u8, lay: u8, raw: char) -> Ch {
        Ch { tok, lay, typed: lay, raw }
    }

    pub fn render(&self) -> char {
        if self.tok == OTHER { self.raw } else { KEYMAP.char_of(self.tok, self.lay) }
    }
}

/// Что поменять: слово и нужная раскладка; буфер ещё не тронут.
pub struct Plan {
    flips: Vec<(usize, usize, u8)>,
    first: usize,
    end: usize,
}

/// Правка экрана: стереть erase символов, напечатать text.
pub struct Fix {
    pub erase: usize,
    pub text: String,
    pub old: String,
    pub new_lay: u8,
    pub last_flipped: bool,
}

/// Слово по началу: вероятность на своей границе и была ли правка.
struct Seen {
    start: usize,
    first_p: f32,
    done: bool,
}

#[derive(Default)]
pub struct Buffer {
    pub chars: Vec<Ch>,
    seen: Vec<Seen>,
}

impl Buffer {
    pub fn clear(&mut self) {
        self.chars.clear();
        self.seen.clear();
    }

    pub fn push(&mut self, ch: Ch) {
        if self.chars.len() >= MAX_LEN {
            let cut = MAX_LEN / 2;
            self.chars.drain(..cut);
            self.seen.retain_mut(|w| {
                w.start = w.start.wrapping_sub(cut);
                w.start < MAX_LEN
            });
        }
        self.chars.push(ch);
    }

    pub fn pop(&mut self) {
        self.chars.pop();
        let n = self.chars.len();
        // слово стёрто или правится: прежняя оценка недействительна
        let chars = &self.chars;
        self.seen.retain(|w| w.start < n && chars[w.start..].iter().any(|c| c.tok == SPACE));
    }

    pub fn has_word(&self) -> bool {
        self.chars.last().is_some_and(|c| c.tok != SPACE)
    }

    /// Только что закончено слово: последний пробел после буквы.
    pub fn word_just_ended(&self) -> bool {
        let n = self.chars.len();
        n >= 2 && self.chars[n - 1].tok == SPACE && self.chars[n - 2].tok != SPACE
    }

    fn spans(&self, end: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut s = None;
        for i in 0..=end {
            let sp = i == end || self.chars[i].tok == SPACE;
            match (sp, s) {
                (false, None) => s = Some(i),
                (true, Some(a)) => {
                    out.push((a, i));
                    s = None;
                }
                _ => {}
            }
        }
        out
    }

    fn window_start(&self, end: usize) -> usize {
        let lo = end.saturating_sub(W);
        let mut s = lo;
        while s > 0 && s < end && self.chars[s - 1].tok != SPACE {
            s += 1;
        }
        while s < end && self.chars[s].tok == SPACE {
            s += 1;
        }
        if s < end {
            return s;
        }
        s = end;
        while s > lo && self.chars[s - 1].tok != SPACE {
            s -= 1;
        }
        s
    }

    /// Вероятности по окну [s, end) исходного набора.
    fn run(&self, model: &mut Model, s: usize, end: usize) -> Result<[f32; W], String> {
        let off = W - (end - s);
        let mut tok = [PAD as i32; W];
        let mut lay = [0i32; W];
        for (i, c) in self.chars[s..end].iter().enumerate() {
            tok[off + i] = c.tok as i32;
            lay[off + i] = c.typed as i32;
        }
        model.run(&tok, &lay)
    }

    /// Прогон модели по окну, кончающемуся последним словом.
    pub fn plan(&mut self, model: &mut Model, threshold: f32) -> Result<Option<Plan>, String> {
        let mut end = self.chars.len();
        while end > 0 && self.chars[end - 1].tok == SPACE {
            end -= 1;
        }
        if end == 0 {
            return Ok(None);
        }
        let s = self.window_start(end);
        let off = W - (end - s);
        let p = self.run(model, s, end)?;

        let mut flips = Vec::new();
        for (a, b) in self.spans(end) {
            if a < s {
                continue;
            }
            let (sum, n) = (a..b)
                .filter(|&i| !KEYMAP.neutral(self.chars[i].tok))
                .fold((0f32, 0), |(acc, n), i| (acc + p[off + i - s], n + 1));
            if n == 0 {
                continue;
            }
            let pm = sum / n as f32;
            let allowed = match self.seen.iter().find(|w| w.start == a) {
                Some(w) => !w.done && (n <= SHORT || w.first_p > RETRO_MIN),
                None => {
                    self.seen.push(Seen { start: a, first_p: pm, done: false });
                    true
                }
            };
            let core = (a..b).find(|&i| !KEYMAP.neutral(self.chars[i].tok)).unwrap_or(a);
            let want = self.chars[core].typed ^ (pm > threshold) as u8;
            let keep = n <= SHORT && real_short_word(&self.chars[a..b]);
            if allowed && !keep && want != self.chars[core].lay {
                flips.push((a, b, want));
            }
        }
        Ok(flips.iter().map(|f| f.0).min().map(|first| Plan { flips, first, end }))
    }

    /// Диагностика: средняя вероятность «перевернуть» по словам окна.
    pub fn probe(&self, model: &mut Model) -> Result<Vec<(String, f32)>, String> {
        let end = self.chars.len();
        let s = self.window_start(end);
        let off = W - (end - s);
        let p = self.run(model, s, end)?;
        Ok(self
            .spans(end)
            .into_iter()
            .filter(|&(a, _)| a >= s)
            .map(|(a, b)| {
                let word: String = self.chars[a..b].iter().map(Ch::render).collect();
                let v: Vec<f32> =
                    (a..b).filter(|&i| !KEYMAP.neutral(self.chars[i].tok)).map(|i| p[off + i - s]).collect();
                (word, v.iter().sum::<f32>() / v.len().max(1) as f32)
            })
            .collect())
    }

    /// Применить план; набранное после слова идёт в новой раскладке.
    pub fn apply(&mut self, plan: Plan) -> Fix {
        let old: String = self.chars[plan.first..].iter().map(Ch::render).collect();
        let last_flipped = plan.flips.iter().any(|f| f.1 == plan.end);
        for &(a, b, want) in &plan.flips {
            for c in &mut self.chars[a..b] {
                c.lay = want;
            }
            if let Some(w) = self.seen.iter_mut().find(|w| w.start == a) {
                w.done = true;
            }
        }
        let new_lay = self.chars[..plan.end].iter().rev().find(|c| !KEYMAP.neutral(c.tok)).map_or(0, |c| c.lay);
        if last_flipped {
            for c in &mut self.chars[plan.end..] {
                c.lay = new_lay;
            }
        }
        let text: String = self.chars[plan.first..].iter().map(Ch::render).collect();
        Fix { erase: self.chars.len() - plan.first, text, old, new_lay, last_flipped }
    }
}
