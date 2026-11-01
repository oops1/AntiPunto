//! Физические клавиши: те же токены, что в train/keys.py.

use std::sync::LazyLock;

pub const PAD: u8 = 0;
pub const SPACE: u8 = 1;
pub const OTHER: u8 = 96;
pub const VOCAB: usize = 97;
pub const EN: u8 = 0;
pub const RU: u8 = 1;

const ROWS: [(&str, &str, &str, &str); 4] = [
    ("`1234567890-=", "~!@#$%^&*()_+", "ё1234567890-=", "Ё!\"№;%:?*()_+"),
    ("qwertyuiop[]\\", "QWERTYUIOP{}|", "йцукенгшщзхъ\\", "ЙЦУКЕНГШЩЗХЪ/"),
    ("asdfghjkl;'", "ASDFGHJKL:\"", "фывапролджэ", "ФЫВАПРОЛДЖЭ"),
    ("zxcvbnm,./", "ZXCVBNM<>?", "ячсмитьбю.", "ЯЧСМИТЬБЮ,"),
];

const SCANS: [u16; 47] = [
    0x29, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, //
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x2B, //
    0x1E, 0x1F, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, //
    0x2C, 0x2D, 0x2E, 0x2F, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35,
];

pub struct Keymap {
    chars: [[char; VOCAB]; 2],
    neutral: [bool; VOCAB],
}

pub static KEYMAP: LazyLock<Keymap> = LazyLock::new(|| {
    let mut chars = [['\0'; VOCAB]; 2];
    let mut k = 0;
    for (el, eh, rl, rh) in ROWS {
        for (((a, b), c), d) in el.chars().zip(eh.chars()).zip(rl.chars()).zip(rh.chars()) {
            chars[EN as usize][2 + 2 * k] = a;
            chars[EN as usize][3 + 2 * k] = b;
            chars[RU as usize][2 + 2 * k] = c;
            chars[RU as usize][3 + 2 * k] = d;
            k += 1;
        }
    }
    assert_eq!(k, SCANS.len());
    chars[0][SPACE as usize] = ' ';
    chars[1][SPACE as usize] = ' ';
    let mut neutral = [false; VOCAB];
    for t in 0..VOCAB {
        neutral[t] = t == SPACE as usize || t == OTHER as usize || (t >= 2 && chars[0][t] == chars[1][t]);
    }
    Keymap { chars, neutral }
});

impl Keymap {
    pub fn key_index(&self, scan: u16) -> Option<usize> {
        SCANS.iter().position(|&s| s == scan)
    }

    /// Токен клавиши k с учётом Shift и Caps Lock для букв раскладки.
    pub fn token(&self, k: usize, shift: bool, caps: bool, lay: u8) -> u8 {
        let letter = self.chars[lay as usize][2 + 2 * k].is_alphabetic();
        (2 + 2 * k + (shift ^ (caps && letter)) as usize) as u8
    }

    pub fn char_of(&self, tok: u8, lay: u8) -> char {
        self.chars[lay as usize][tok as usize]
    }

    pub fn neutral(&self, tok: u8) -> bool {
        self.neutral[tok as usize]
    }

    /// Обратная таблица для самотеста: символ → токен в раскладке.
    pub fn token_of(&self, c: char, lay: u8) -> Option<u8> {
        (1..OTHER).find(|&t| self.chars[lay as usize][t as usize] == c)
    }
}
