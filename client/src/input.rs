//! Синтетический ввод и смена раскладки.

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayoutList, HKL, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, SendInput, VIRTUAL_KEY, VK_BACK,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, PostMessageW, WM_INPUTLANGCHANGEREQUEST};

use crate::hook::MARKER;
use crate::keymap::RU;

fn key(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: MARKER },
        },
    }
}

fn unicode(out: &mut Vec<INPUT>, text: &str) {
    for u in text.encode_utf16() {
        out.push(key(0, u, KEYEVENTF_UNICODE));
        out.push(key(0, u, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
    }
}

fn send(inputs: &[INPUT]) -> bool {
    inputs.is_empty() || unsafe { SendInput(inputs, size_of::<INPUT>() as i32) } as usize == inputs.len()
}

/// Стереть erase символов и напечатать text.
pub fn replace(erase: usize, text: &str) -> bool {
    let mut v = Vec::with_capacity(erase * 2);
    for _ in 0..erase {
        v.push(key(VK_BACK.0, 0, KEYBD_EVENT_FLAGS(0)));
        v.push(key(VK_BACK.0, 0, KEYEVENTF_KEYUP));
    }
    let ok = send(&v);
    // приложения с TSF (Блокнот Win11) иначе путают порядок
    std::thread::sleep(std::time::Duration::from_millis(8));
    ok && type_text(text)
}

pub fn type_text(text: &str) -> bool {
    let mut v = Vec::new();
    unicode(&mut v, text);
    send(&v)
}

pub fn tap(vk: u16, scan: u16, ext: bool) -> bool {
    let f = if ext { KEYEVENTF_EXTENDEDKEY } else { KEYBD_EVENT_FLAGS(0) };
    send(&[key(vk, scan, f), key(vk, scan, f | KEYEVENTF_KEYUP)])
}

/// Попросить окно переднего плана сменить раскладку.
pub fn switch_layout(lay: u8) {
    // смена языка в полёте теряет нажатие: дать приложению дочитать очередь
    std::thread::sleep(std::time::Duration::from_millis(40));
    unsafe {
        let n = GetKeyboardLayoutList(None);
        let mut list = vec![HKL::default(); n.max(0) as usize];
        GetKeyboardLayoutList(Some(&mut list));
        let want = if lay == RU { 0x19 } else { 0x09 };
        if let Some(h) = list.iter().find(|h| (h.0 as usize & 0x3FF) == want) {
            let _ =
                PostMessageW(Some(GetForegroundWindow()), WM_INPUTLANGCHANGEREQUEST, WPARAM(0), LPARAM(h.0 as isize));
        }
    }
}
