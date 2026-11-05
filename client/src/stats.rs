//! Счётчики работы (без текста) для окна статистики.

use std::collections::BTreeMap;
use std::time::Instant;

pub fn dir() -> std::path::PathBuf {
    std::env::var_os("APPDATA").map(std::path::PathBuf::from).unwrap_or_default().join("AntiPunto")
}

#[derive(Default, Clone, PartialEq)]
pub struct Stats {
    pub words: u64,
    pub fixes: u64,
    pub undos: u64,
    pub forced: u64,
    pub password_skips: u64,
    pub infer_us: u64,
    pub spell_fixes: u64,
    /// Выбрано вариантов из окна у курсора.
    pub picks: u64,
    pub by_app: BTreeMap<String, u64>,
}

pub struct Saver {
    saved: Stats,
    at: Instant,
}

pub const FILE: &str = "stats.txt";

impl Stats {
    pub fn load() -> Stats {
        let mut s = Stats::default();
        for line in std::fs::read_to_string(dir().join(FILE)).unwrap_or_default().lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v: u64 = v.trim().parse().unwrap_or(0);
            match k.trim() {
                "words" => s.words = v,
                "fixes" => s.fixes = v,
                "undos" => s.undos = v,
                "forced" => s.forced = v,
                "password_skips" => s.password_skips = v,
                "infer_us" => s.infer_us = v,
                "spell_fixes" => s.spell_fixes = v,
                "picks" => s.picks = v,
                k => {
                    if let Some(app) = k.strip_prefix("app.") {
                        s.by_app.insert(app.to_string(), v);
                    }
                }
            }
        }
        s
    }

    fn save(&self) {
        let mut out = format!(
            "words={}\nfixes={}\nundos={}\nforced={}\npassword_skips={}\ninfer_us={}\nspell_fixes={}\npicks={}\n",
            self.words,
            self.fixes,
            self.undos,
            self.forced,
            self.password_skips,
            self.infer_us,
            self.spell_fixes,
            self.picks
        );
        for (app, n) in &self.by_app {
            out += &format!("app.{app}={n}\n");
        }
        let _ = std::fs::create_dir_all(dir());
        let _ = std::fs::write(dir().join(FILE), out);
    }
}

impl Saver {
    pub fn new(s: &Stats) -> Saver {
        Saver { saved: s.clone(), at: Instant::now() }
    }

    /// Записать сейчас: окно настроек ждёт свежие числа.
    pub fn force(&mut self, s: &Stats) {
        s.save();
        self.saved = s.clone();
        self.at = Instant::now();
    }

    /// Записать на диск не чаще раза в 30 с и только при изменениях.
    pub fn tick(&mut self, s: &Stats) {
        if self.at.elapsed().as_secs() >= 30 && *s != self.saved {
            s.save();
            self.saved = s.clone();
            self.at = Instant::now();
        }
    }
}
