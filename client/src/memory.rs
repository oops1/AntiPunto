//! Свои слова: словарные слова, которые пользователь набирал и оставил как есть.
//! От SAFE раз слово считается своим — правка «на другое слово» ему не предлагается.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::lists;

pub const FILE: &str = "words.tsv";
/// Столько раз набранное и оставленное слово — своё.
pub const SAFE: u32 = 3;
const HEADER: &str = "# Слова, которые вы набирали и оставили как есть. От 3 раз слово считается своим,\n\
                      # и программа не предлагает заменить его на другое слово. Строка: слово, табуляция, сколько раз.\n";
const SAVE_EVERY: Duration = Duration::from_secs(60);

pub struct Memory {
    path: PathBuf,
    counts: HashMap<String, u32>,
    dirty: bool,
    saved: Instant,
}

impl Memory {
    pub fn load() -> Memory {
        let path = lists::path(FILE);
        let counts = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| {
                let (w, n) = l.split_once('\t')?;
                Some((w.trim().to_string(), n.trim().parse().ok()?))
            })
            .filter(|(w, _)| !w.is_empty())
            .collect();
        Memory { path, counts, dirty: false, saved: Instant::now() }
    }

    pub fn safe(&self, w: &str) -> bool {
        self.counts.get(w).is_some_and(|&n| n >= SAFE)
    }

    pub fn hit(&mut self, w: &str, n: u32) {
        *self.counts.entry(w.to_string()).or_default() += n;
        self.dirty = true;
        self.flush();
    }

    /// Не чаще раза в минуту: tmp + переименование, чтобы не потерять файл на полпути.
    fn flush(&mut self) {
        if !self.dirty || self.saved.elapsed() < SAVE_EVERY {
            return;
        }
        let mut rows: Vec<(&String, &u32)> = self.counts.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let mut text = String::from(HEADER);
        for (w, n) in rows {
            text.push_str(&format!("{w}\t{n}\n"));
        }
        let tmp = self.path.with_extension("tmp");
        if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, &self.path).is_ok() {
            self.dirty = false;
        }
        self.saved = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts() {
        let mut m = Memory { path: PathBuf::new(), counts: HashMap::new(), dirty: false, saved: Instant::now() };
        m.hit("кода", 1);
        m.hit("кода", 1);
        assert!(!m.safe("кода"));
        m.hit("кода", 1);
        assert!(m.safe("кода"));
        m.hit("фид", SAFE);
        assert!(m.safe("фид"));
    }
}
