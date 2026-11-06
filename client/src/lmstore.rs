//! Необязательная языковая модель: где лежит, закачка из репозитория, проверка по вшитому перечню.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};

use crate::update::{REPO, get_with, sha256_hex};

/// Перечень частей и файлов с SHA-256: вшит при сборке, подменить его можно только вместе с программой.
const MANIFEST: &str = include_str!("../../models/Ext/rugpt3small/manifest.tsv");
const PATH: &str = "models/Ext/rugpt3small";

pub struct Entry {
    pub name: &'static str,
    pub size: u64,
    pub sha: &'static str,
}

pub struct Manifest {
    pub name: &'static str,
    pub parts: Vec<Entry>,
    pub files: Vec<Entry>,
    meta: Vec<(&'static str, &'static str)>,
}

impl Manifest {
    pub fn meta(&self, key: &str) -> usize {
        self.meta.iter().find(|(k, _)| *k == key).and_then(|(_, v)| v.parse().ok()).unwrap_or(0)
    }

    pub fn total(&self) -> u64 {
        self.parts.iter().map(|p| p.size).sum()
    }
}

pub fn manifest() -> Manifest {
    let mut m = Manifest { name: "", parts: Vec::new(), files: Vec::new(), meta: Vec::new() };
    for line in MANIFEST.lines() {
        let f: Vec<&'static str> = line.split('\t').collect();
        match f.as_slice() {
            ["meta", "name", v] => m.name = v,
            ["meta", k, v] => m.meta.push((k, v)),
            [kind @ ("part" | "file"), name, size, sha] => {
                let e = Entry { name, size: size.parse().unwrap_or(0), sha };
                if *kind == "part" { m.parts.push(e) } else { m.files.push(e) }
            }
            _ => {}
        }
    }
    m
}

/// Папка установленной модели: %LOCALAPPDATA%\AntiPunto\models\<имя пакета>.
pub fn dir() -> PathBuf {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default();
    local.join("AntiPunto").join("models").join(manifest().name)
}

/// Все файлы на месте и нужного размера (содержимое проверяет verify).
pub fn installed(dir: &Path) -> bool {
    manifest().files.iter().all(|f| std::fs::metadata(dir.join(f.name)).is_ok_and(|m| m.len() == f.size))
}

/// Сверка содержимого с перечнем: изменённый или чужой файл не используем.
pub fn verify(dir: &Path) -> Result<(), String> {
    for f in manifest().files {
        let data = std::fs::read(dir.join(f.name)).map_err(|e| format!("{}: {e}", f.name))?;
        if data.len() as u64 != f.size || sha256_hex(&data)? != f.sha {
            return Err(format!("файл модели {} изменён: контрольная сумма не совпала", f.name));
        }
    }
    Ok(())
}

/// Часть пакета: из репозитория; в отладочной сборке — из папки или адреса в ANTIPUNTO_MODELS_SRC.
fn fetch(name: &str, on: &mut dyn FnMut(usize) -> bool) -> Result<Vec<u8>, String> {
    let mut base = format!("https://raw.githubusercontent.com/{REPO}/main/{PATH}");
    if cfg!(debug_assertions)
        && let Ok(src) = std::env::var("ANTIPUNTO_MODELS_SRC")
    {
        if !src.starts_with("https://") {
            let data = std::fs::read(Path::new(&src).join(name)).map_err(|e| format!("{name}: {e}"))?;
            on(data.len());
            return Ok(data);
        }
        base = src;
    }
    get_with(&format!("{base}/{name}"), on)
}

/// Скачать части, сверить, разрезать на файлы и поставить в dir; progress — (принято, всего) байт.
pub fn install(dir: &Path, progress: &dyn Fn(u64, u64), stop: &AtomicBool) -> Result<(), String> {
    let m = manifest();
    let total = m.total();
    let mut blob: Vec<u8> = Vec::with_capacity(total as usize);
    for p in &m.parts {
        let done = blob.len() as u64;
        let data = fetch(p.name, &mut |n| {
            progress(done + n as u64, total);
            !stop.load(SeqCst)
        })?;
        if data.len() as u64 != p.size || sha256_hex(&data)? != p.sha {
            return Err(format!("{}: контрольная сумма не совпала — закачка повреждена или подменена", p.name));
        }
        blob.extend_from_slice(&data);
    }
    let tmp = dir.with_extension("tmp");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let mut at = 0usize;
    for f in &m.files {
        let end = at + f.size as usize;
        let data = blob.get(at..end).ok_or("пакет короче перечня")?;
        if sha256_hex(data)? != f.sha {
            return Err(format!("{}: контрольная сумма не совпала", f.name));
        }
        std::fs::write(tmp.join(f.name), data).map_err(|e| format!("{}: {e}", f.name))?;
        at = end;
    }
    let _ = std::fs::remove_dir_all(dir);
    std::fs::rename(&tmp, dir).map_err(|e| format!("не удалось поставить модель: {e}"))
}

pub fn remove(dir: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
        _ => Ok(()),
    }
}

/// Модель из частей репозитория во временной папке: для тестов без сети.
#[cfg(test)]
pub fn unpack_for_tests() -> PathBuf {
    use std::sync::OnceLock;
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let m = manifest();
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(PATH);
        let dir = std::env::temp_dir().join(format!("antipunto-test-{}", m.name));
        if !installed(&dir) {
            let blob: Vec<u8> = m.parts.iter().flat_map(|p| std::fs::read(src.join(p.name)).expect(p.name)).collect();
            std::fs::create_dir_all(&dir).unwrap();
            let mut at = 0;
            for f in &m.files {
                std::fs::write(dir.join(f.name), &blob[at..at + f.size as usize]).unwrap();
                at += f.size as usize;
            }
        }
        dir
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_consistent() {
        let m = manifest();
        assert!(m.name.starts_with("rugpt3small-"));
        assert_eq!(m.parts.len(), 4);
        assert_eq!(m.total(), m.files.iter().map(|f| f.size).sum::<u64>());
        assert!(m.parts.iter().chain(&m.files).all(|e| e.sha.len() == 64 && e.size > 0));
        assert_eq!((m.meta("rows"), m.meta("tokens"), m.meta("tail"), m.meta("hidden")), (16, 40, 24, 768));
    }

    /// Части в репозитории соответствуют перечню; изменённый файл не проходит проверку.
    #[test]
    fn parts_match_manifest_and_tamper_is_caught() {
        let m = manifest();
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(PATH);
        for p in &m.parts {
            let data = std::fs::read(src.join(p.name)).expect(p.name);
            assert_eq!(sha256_hex(&data).unwrap(), p.sha, "{}", p.name);
        }
        let dir = unpack_for_tests();
        verify(&dir).unwrap();
        let bad = std::env::temp_dir().join(format!("antipunto-test-{}-bad", m.name));
        let _ = std::fs::remove_dir_all(&bad);
        std::fs::create_dir_all(&bad).unwrap();
        for f in &m.files {
            let mut data = std::fs::read(dir.join(f.name)).unwrap();
            if f.name == "lm_vocab.txt" {
                data[100] ^= 1;
            }
            std::fs::write(bad.join(f.name), data).unwrap();
        }
        assert!(installed(&bad));
        assert!(verify(&bad).unwrap_err().contains("lm_vocab.txt"));
        let _ = std::fs::remove_dir_all(&bad);
    }

    /// Установка из папки с частями (как из сети) и удаление.
    #[test]
    fn install_from_parts() {
        let m = manifest();
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(PATH);
        // SAFETY: тесты этой переменной больше нигде не читают
        unsafe { std::env::set_var("ANTIPUNTO_MODELS_SRC", &src) };
        let dir = std::env::temp_dir().join(format!("antipunto-test-{}-install", m.name));
        let seen = std::sync::atomic::AtomicU64::new(0);
        install(&dir, &|done, _| seen.store(done, SeqCst), &AtomicBool::new(false)).unwrap();
        assert_eq!(seen.load(SeqCst), m.total());
        verify(&dir).unwrap();
        remove(&dir).unwrap();
        assert!(!installed(&dir));
    }
}
