//! Настройки из меню трея и окна настроек: HKCU\Software\AntiPunto.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::SeqCst};

use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_DWORD, RRF_RT_REG_DWORD, RegGetValueW, RegSetKeyValueW};
use windows::core::{PCWSTR, w};

const KEY: PCWSTR = w!("Software\\AntiPunto");

/// Исправлять опечатки (раскладка — отдельно, «Включено»).
pub static SPELL: AtomicBool = AtomicBool::new(true);
/// Показывать варианты у курсора, когда модель сомневается.
pub static SUGGEST: AtomicBool = AtomicBool::new(true);
/// Тема окон: 0 — как в Windows, 1 — светлая, 2 — тёмная.
pub static THEME: AtomicU32 = AtomicU32::new(0);

fn get(name: PCWSTR) -> Option<u32> {
    let mut v = 0u32;
    let mut n = size_of::<u32>() as u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, KEY, name, RRF_RT_REG_DWORD, None, Some((&raw mut v).cast()), Some(&mut n))
            .is_ok()
            .then_some(v)
    }
}

fn put(name: PCWSTR, v: u32) {
    unsafe {
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, KEY, name, REG_DWORD.0, Some((&raw const v).cast()), 4);
    }
}

pub fn load() {
    SPELL.store(get(w!("Spell")) != Some(0), SeqCst);
    SUGGEST.store(get(w!("Suggest")) != Some(0), SeqCst);
    THEME.store(get(w!("Theme")).unwrap_or(0).min(2), SeqCst);
}

pub fn set_spell(on: bool) {
    SPELL.store(on, SeqCst);
    put(w!("Spell"), on as u32);
}

pub fn set_suggest(on: bool) {
    SUGGEST.store(on, SeqCst);
    put(w!("Suggest"), on as u32);
}

pub fn set_theme(v: u32) {
    THEME.store(v, SeqCst);
    put(w!("Theme"), v);
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Registry::RegDeleteKeyValueW;

    #[test]
    fn registry_roundtrip() {
        let name = w!("TestValue");
        assert_eq!(get(name), None);
        put(name, 0);
        assert_eq!(get(name), Some(0));
        put(name, 1);
        assert_eq!(get(name), Some(1));
        unsafe {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, KEY, name);
        }
        assert_eq!(get(name), None);
    }
}
