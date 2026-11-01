//! Списки пользователя: программы-исключения, слова-исключения, жёсткие слова.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

pub const PROGRAMS: &str = "programs.txt";
pub const IGNORE: &str = "ignore.txt";
pub const FORCE: &str = "force.txt";

const HEADERS: [(&str, &str); 3] = [
    (PROGRAMS, "# Программы, в которых AntiPunto ничего не исправляет. Одна на строку, имя exe:\n# game.exe\n"),
    (IGNORE, "# Слова, которые никогда не исправлять (ники, сокращения). Одно на строку:\n# xnj\n"),
    (
        FORCE,
        "# Жёсткие слова: набранное не в той раскладке исправляется всегда.\n# Пишите правильное слово:\n# привет\n",
    ),
];

pub fn dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default().join("AntiPunto")
}

pub fn path(name: &str) -> PathBuf {
    dir().join(name)
}

/// Создать файлы с подсказкой, если их нет.
pub fn ensure_files() {
    let _ = std::fs::create_dir_all(dir());
    for (name, header) in HEADERS {
        let p = path(name);
        if !p.exists() {
            let _ = std::fs::write(p, header);
        }
    }
}

/// Нормальная форма слова: буквы и цифры в нижнем регистре.
pub fn norm(word: impl IntoIterator<Item = char>) -> String {
    word.into_iter().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

#[derive(Default)]
pub struct Lists {
    pub programs: HashSet<String>,
    pub ignore: HashSet<String>,
    pub force: HashSet<String>,
    stamps: [Option<SystemTime>; 3],
    checked: Option<Instant>,
}

fn read(name: &str, f: impl Fn(&str) -> String) -> HashSet<String> {
    std::fs::read_to_string(path(name))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(f)
        .filter(|s| !s.is_empty())
        .collect()
}

impl Lists {
    /// Перечитать файлы, если изменились; не чаще раза в 2 с.
    pub fn refresh(&mut self) {
        if self.checked.is_some_and(|t| t.elapsed().as_secs() < 2) {
            return;
        }
        self.checked = Some(Instant::now());
        let stamps = [PROGRAMS, IGNORE, FORCE].map(|n| std::fs::metadata(path(n)).and_then(|m| m.modified()).ok());
        if stamps == self.stamps {
            return;
        }
        self.stamps = stamps;
        self.programs = read(PROGRAMS, str::to_lowercase);
        self.ignore = read(IGNORE, |l| norm(l.chars()));
        self.force = read(FORCE, |l| norm(l.chars()));
    }
}

/// Добавить программу в programs.txt.
pub fn add_program(name: &str) -> std::io::Result<()> {
    use std::io::Write;
    ensure_files();
    let mut f = std::fs::OpenOptions::new().append(true).open(path(PROGRAMS))?;
    writeln!(f, "{name}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn norm() {
        assert_eq!(super::norm("GitHub,".chars()), "github");
        assert_eq!(super::norm("Ёжик!".chars()), "ёжик");
    }
}
