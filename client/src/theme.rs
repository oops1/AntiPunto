//! Цвета окон: светлая и тёмная тема.

use std::sync::atomic::Ordering::SeqCst;

use windows::Win32::Foundation::COLORREF;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::core::w;

use crate::settings;

/// Значения настройки Theme; 0 — как в Windows.
pub const LIGHT: u32 = 1;
pub const DARK: u32 = 2;

// COLORREF — 0x00BBGGRR
pub struct Palette {
    pub bg: COLORREF,
    /// Боковая панель и кнопки.
    pub panel: COLORREF,
    /// Нажатая кнопка, выбранный раздел.
    pub active: COLORREF,
    pub ink: COLORREF,
    pub grey: COLORREF,
    pub red: COLORREF,
    pub green: COLORREF,
    pub blue: COLORREF,
    pub line: COLORREF,
    pub frame: COLORREF,
}

static PAL_LIGHT: Palette = Palette {
    bg: COLORREF(0x00FA_FAFA),
    panel: COLORREF(0x00EE_EEEE),
    active: COLORREF(0x00DD_DDDD),
    ink: COLORREF(0x0020_2020),
    grey: COLORREF(0x0070_7070),
    red: COLORREF(0x0000_00DC),
    green: COLORREF(0x0028_9600),
    blue: COLORREF(0x00C0_7820),
    line: COLORREF(0x00C8_C8C8),
    frame: COLORREF(0x00A0_A0A0),
};

static PAL_DARK: Palette = Palette {
    bg: COLORREF(0x002B_2B2B),
    panel: COLORREF(0x0020_2020),
    active: COLORREF(0x0040_4040),
    ink: COLORREF(0x00EA_EAEA),
    grey: COLORREF(0x00A0_A0A0),
    red: COLORREF(0x0070_70FF),
    green: COLORREF(0x0060_D970),
    blue: COLORREF(0x00FF_B870),
    line: COLORREF(0x0048_4848),
    frame: COLORREF(0x0068_6868),
};

/// Приложения Windows в тёмном режиме.
fn system_dark() -> bool {
    let mut v = 1u32;
    let mut n = size_of::<u32>() as u32;
    unsafe {
        let _ = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut v).cast()),
            Some(&mut n),
        );
    }
    v == 0
}

pub fn dark() -> bool {
    match settings::THEME.load(SeqCst) {
        LIGHT => false,
        DARK => true,
        _ => system_dark(),
    }
}

pub fn palette() -> &'static Palette {
    if dark() { &PAL_DARK } else { &PAL_LIGHT }
}
