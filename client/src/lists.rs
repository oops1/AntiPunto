//! Списки пользователя: программы-исключения, слова-исключения, жёсткие слова.

use std::collections::HashSet;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};

use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::Storage::FileSystem::{
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FindFirstChangeNotificationW,
    FindNextChangeNotification,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Threading::{INFINITE, WaitForSingleObject};
use windows::Win32::UI::Shell::{FOLDERID_Documents, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath};
use windows::core::HSTRING;

pub const PROGRAMS: &str = "programs.txt";
pub const IGNORE: &str = "ignore.txt";
pub const FORCE: &str = "force.txt";

const HEADERS: [(&str, &str); 3] = [
    (PROGRAMS, "# Программы, в которых AntiPunto ничего не исправляет. Одна на строку, имя exe:\n# game.exe\n"),
    (IGNORE, "# Слова, которые никогда не исправлять (ники, сокращения). Одно на строку:\n# xnj\n"),
    (
        FORCE,
        "# Желанные слова — пишите так, как хотите видеть на экране, например: шифт\n\
         # набрано не в той раскладке (iban) — исправим всегда; набрано правильно — не тронем.\n",
    ),
];

/// «Документы\AntiPunto»: не AppData — её Блокнот Windows 11 (MSIX) видит виртуализированной.
pub fn dir() -> PathBuf {
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(&FOLDERID_Documents, KNOWN_FOLDER_FLAG(0), None) {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const c_void));
            if !s.is_empty() {
                return PathBuf::from(s).join("AntiPunto");
            }
        }
    }
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default().join("Documents").join("AntiPunto")
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

/// Папка изменилась: перечитать при следующем нажатии.
static DIRTY: AtomicBool = AtomicBool::new(true);

/// Сторож папки: спит в ядре, Windows будит его только при записи в неё.
pub fn watch() {
    std::thread::spawn(|| unsafe {
        ensure_files();
        let d = HSTRING::from(dir().as_os_str());
        let filter = FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_FILE_NAME;
        let Ok(h) = FindFirstChangeNotificationW(&d, false, filter) else { return };
        while WaitForSingleObject(h, INFINITE) == WAIT_OBJECT_0 {
            DIRTY.store(true, SeqCst);
            if FindNextChangeNotification(h).is_err() {
                break;
            }
        }
    });
}

#[derive(Default)]
pub struct Lists {
    pub programs: HashSet<String>,
    pub ignore: HashSet<String>,
    pub force: HashSet<String>,
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
    /// Перечитать, только если сторож видел изменения; иначе — одно чтение флага.
    pub fn refresh(&mut self) {
        if !DIRTY.swap(false, SeqCst) {
            return;
        }
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
