//! Автозапуск при входе в Windows: значение в HKCU\...\Run (то же, что ставит установщик).

use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::core::{HSTRING, w};

const RUN: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const NAME: windows::core::PCWSTR = w!("AntiPunto");

pub fn enabled() -> bool {
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN, NAME, RRF_RT_REG_SZ, None, None, None).is_ok() }
}

pub fn set(on: bool) {
    unsafe {
        if on {
            let Ok(exe) = std::env::current_exe() else { return };
            let value = HSTRING::from(format!("\"{}\"", exe.display()));
            let bytes = (value.len() + 1) * 2;
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, RUN, NAME, REG_SZ.0, Some(value.as_ptr().cast()), bytes as u32);
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN, NAME);
        }
    }
}
