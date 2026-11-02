//! Низкоуровневые хуки: только кладут событие в очередь.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::mpsc::Sender;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyboardLayout, VIRTUAL_KEY, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_LCONTROL, VK_LMENU,
    VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RETURN, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GUITHREADINFO, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, HC_ACTION,
    KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_UP, MSLLHOOKSTRUCT, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_RBUTTONDOWN,
    WM_XBUTTONDOWN,
};

use crate::keymap::{EN, RU};
use crate::popup;

/// Метка наших синтетических нажатий в dwExtraInfo.
pub const MARKER: usize = 0x5345_4B59;

pub static SENDER: OnceLock<Sender<Event>> = OnceLock::new();
pub static ENABLED: AtomicBool = AtomicBool::new(true);
pub static CORRECTING: AtomicBool = AtomicBool::new(false);
pub static HAS_WORD: AtomicBool = AtomicBool::new(false);
/// Только что была автоправка: голый Backspace её отменит.
pub static UNDO_READY: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug)]
pub struct Key {
    pub vk: u16,
    pub scan: u16,
    pub up: bool,
    pub ext: bool,
    pub lay: Option<u8>,
    pub hwnd: isize,
    /// Проглочено хуком: воспроизвести после правки.
    pub deferred: bool,
    /// Enter/Tab проглочен до проверки слова.
    pub boundary: bool,
    /// Backspace проглочен как отмена автоправки.
    pub undo: bool,
    /// Цифра при открытых вариантах: номер варианта.
    pub pick: Option<usize>,
}

fn held(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetAsyncKeyState(vk.0 as i32) as u16 & 0x8000 != 0 }
}

pub enum Event {
    Key(Key),
    Mouse,
    /// Клик по варианту в окне у курсора.
    Pick(usize),
}

pub fn is_modifier(vk: u16) -> bool {
    [
        VK_SHIFT,
        VK_LSHIFT,
        VK_RSHIFT,
        VK_CONTROL,
        VK_LCONTROL,
        VK_RCONTROL,
        VK_MENU,
        VK_LMENU,
        VK_RMENU,
        VK_LWIN,
        VK_RWIN,
        VK_CAPITAL,
    ]
    .iter()
    .any(|v| v.0 == vk)
}

/// Раскладка потока с фокусом ввода: RU, EN или другая.
pub fn foreground_layout() -> (Option<u8>, HWND) {
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut tid = GetWindowThreadProcessId(hwnd, None);
        let mut gti = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
        if GetGUIThreadInfo(tid, &mut gti).is_ok() && !gti.hwndFocus.is_invalid() {
            tid = GetWindowThreadProcessId(gti.hwndFocus, None);
        }
        let lang = (GetKeyboardLayout(tid).0 as usize & 0xFFFF) as u16;
        let lay = match lang & 0x3FF {
            0x19 => Some(RU),
            0x09 => Some(EN),
            _ => None,
        };
        (lay, hwnd)
    }
}

pub unsafe extern "system" fn kb_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if code == HC_ACTION as i32 && ENABLED.load(SeqCst) {
            let k = &*(lp.0 as *const KBDLLHOOKSTRUCT);
            if k.dwExtraInfo != MARKER {
                let vk = k.vkCode as u16;
                let up = k.flags.0 & LLKHF_UP.0 != 0;
                let modifier = is_modifier(vk);
                let deferred = !up && !modifier && CORRECTING.load(SeqCst);
                let boundary = !up && !deferred && (vk == VK_RETURN.0 || vk == VK_TAB.0) && HAS_WORD.load(SeqCst);
                // любая клавиша, кроме голого Backspace, отменять уже нечего
                let undo = !up
                    && !modifier
                    && UNDO_READY.swap(false, SeqCst)
                    && vk == VK_BACK.0
                    && !deferred
                    && ![VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN].into_iter().any(held);
                // Ctrl+цифра при открытых вариантах; голые цифры — это текст
                let digit = match vk {
                    0x31..=0x39 => vk as usize - 0x31,
                    0x61..=0x69 => vk as usize - 0x61,
                    _ => usize::MAX,
                };
                let pick = (!up
                    && !deferred
                    && digit < popup::COUNT.load(SeqCst)
                    && popup::VISIBLE.load(SeqCst)
                    && held(VK_CONTROL)
                    && ![VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN].into_iter().any(held))
                .then_some(digit);
                let (lay, hwnd) = foreground_layout();
                if let Some(tx) = SENDER.get() {
                    let ev = Key {
                        vk,
                        scan: k.scanCode as u16,
                        up,
                        ext: k.flags.0 & LLKHF_EXTENDED.0 != 0,
                        lay,
                        hwnd: hwnd.0 as isize,
                        deferred,
                        boundary,
                        undo,
                        pick,
                    };
                    if tx.send(Event::Key(ev)).is_ok() && (deferred || boundary || undo || pick.is_some()) {
                        return LRESULT(1);
                    }
                }
            }
        }
        CallNextHookEx(None, code, wp, lp)
    }
}

pub unsafe extern "system" fn mouse_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if code == HC_ACTION as i32 {
            let m = wp.0 as u32;
            let pt = (*(lp.0 as *const MSLLHOOKSTRUCT)).pt;
            // клик по варианту — не сброс набранного
            if [WM_LBUTTONDOWN, WM_RBUTTONDOWN, WM_MBUTTONDOWN, WM_XBUTTONDOWN].contains(&m) && !popup::hit(pt) {
                UNDO_READY.store(false, SeqCst);
                if let Some(tx) = SENDER.get() {
                    let _ = tx.send(Event::Mouse);
                }
            }
        }
        CallNextHookEx(None, code, wp, lp)
    }
}
