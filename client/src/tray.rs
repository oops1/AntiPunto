//! Значок в трее, меню и цикл сообщений главного потока.

use std::cell::Cell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering::SeqCst};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIN_BALLOONUSERCLICK,
    NOTIFYICONDATAW, Shell_NotifyIconW, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

use crate::hook::{ENABLED, Event, SENDER, foreground_layout};
use crate::keymap::{EN, RU};
use crate::worker::LAST_APP;
use crate::{autostart, lists, popup, settings, update};

pub const WM_TRAY: u32 = WM_APP + 1;
pub const WM_FENCE: u32 = WM_APP + 2;
const WM_NOTIFY_USER: u32 = WM_APP + 3;
const ID_TOGGLE: usize = 1;
const ID_CHECK: usize = 2;
const ID_UPDATE: usize = 3;
const ID_AUTOSTART: usize = 4;
const ID_SKIP_APP: usize = 5;
const ID_LIST_PROGRAMS: usize = 6;
const ID_LIST_IGNORE: usize = 7;
const ID_LIST_FORCE: usize = 8;
const ID_EXIT: usize = 9;
const ID_SPELL: usize = 10;
const ID_SUGGEST: usize = 11;

pub static READY: AtomicBool = AtomicBool::new(false);
pub static STATUS: Mutex<String> = Mutex::new(String::new());
static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
static NOTICE: Mutex<(String, String)> = Mutex::new((String::new(), String::new()));

const ICO_RU: &[u8] = include_bytes!("../assets/tray_ru.ico");
const ICO_EN: &[u8] = include_bytes!("../assets/tray_en.ico");
const ICO_OFF: &[u8] = include_bytes!("../assets/tray_off.ico");

thread_local! {
    static ICONS: Cell<[HICON; 3]> = const { Cell::new([HICON(std::ptr::null_mut()); 3]) };
    static SHOWN: Cell<usize> = const { Cell::new(usize::MAX) };
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
}

pub fn set_status(s: String) {
    *STATUS.lock().unwrap() = s;
}

/// Дождаться, пока главный поток доработает начатые колбэки хука.
pub fn fence() {
    let h = MAIN_HWND.load(SeqCst);
    if h != 0 {
        unsafe { SendMessageW(HWND(h as *mut _), WM_FENCE, None, None) };
    }
}

/// Кадр нужного размера из .ico в памяти.
fn load_icon(data: &[u8], size: i32) -> HICON {
    let n = u16::from_le_bytes([data[4], data[5]]) as usize;
    let mut best: Option<(i32, usize, usize)> = None;
    for i in 0..n {
        let e = &data[6 + 16 * i..22 + 16 * i];
        let w = if e[0] == 0 { 256 } else { e[0] as i32 };
        let len = u32::from_le_bytes(e[8..12].try_into().unwrap()) as usize;
        let off = u32::from_le_bytes(e[12..16].try_into().unwrap()) as usize;
        let score = if w >= size { w - size } else { 1000 + size - w };
        if best.is_none_or(|b| score < b.0) {
            best = Some((score, off, len));
        }
    }
    let (_, off, len) = best.unwrap();
    unsafe {
        CreateIconFromResourceEx(&data[off..off + len], true, 0x0003_0000, size, size, LR_DEFAULTCOLOR)
            .unwrap_or_default()
    }
}

fn nid(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uCallbackMessage: WM_TRAY,
        ..Default::default()
    }
}

fn tip_text(icon: usize) -> String {
    let lay = ["RU", "EN", "пауза"][icon];
    let status = STATUS.lock().unwrap().clone();
    let state = if !READY.load(SeqCst) {
        status
    } else if !ENABLED.load(SeqCst) {
        format!("выключено · {status}")
    } else {
        format!("{lay} · {status}")
    };
    format!("AntiPunto — {state}")
}

fn refresh(hwnd: HWND, force: bool) {
    let icon = if !READY.load(SeqCst) || !ENABLED.load(SeqCst) {
        2
    } else {
        match foreground_layout().0 {
            Some(RU) => 0,
            Some(EN) => 1,
            _ => 2,
        }
    };
    if !force && SHOWN.get() == icon {
        return;
    }
    SHOWN.set(icon);
    let mut d = nid(hwnd);
    d.uFlags = NIF_ICON | NIF_TIP | NIF_MESSAGE;
    d.hIcon = ICONS.get()[icon];
    for (dst, src) in d.szTip.iter_mut().zip(tip_text(icon).encode_utf16().take(127)) {
        *dst = src;
    }
    unsafe {
        if !Shell_NotifyIconW(NIM_MODIFY, &d).as_bool() {
            let _ = Shell_NotifyIconW(NIM_ADD, &d);
        }
    }
}

/// Всплывающее уведомление из любого потока.
pub fn notify(title: impl Into<String>, text: impl Into<String>) {
    *NOTICE.lock().unwrap() = (title.into(), text.into());
    let h = MAIN_HWND.load(SeqCst);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), WM_NOTIFY_USER, WPARAM(0), LPARAM(0));
        }
    }
}

fn balloon(hwnd: HWND) {
    let (title, text) = NOTICE.lock().unwrap().clone();
    let mut d = nid(hwnd);
    d.uFlags = NIF_INFO;
    d.dwInfoFlags = NIIF_INFO;
    for (dst, src) in d.szInfoTitle.iter_mut().zip(title.encode_utf16().take(63)) {
        *dst = src;
    }
    for (dst, src) in d.szInfo.iter_mut().zip(text.encode_utf16().take(255)) {
        *dst = src;
    }
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
    }
}

pub fn check_updates(manual: bool) {
    match update::check() {
        Ok(Some(rel)) => {
            let v = rel.version.clone();
            *update::AVAILABLE.lock().unwrap() = Some(rel);
            notify(format!("AntiPunto {v}"), "Доступна новая версия. Нажмите, чтобы установить.");
        }
        Ok(None) if manual => notify("AntiPunto", format!("Установлена последняя версия {}", update::VERSION)),
        Err(e) if manual => notify("AntiPunto", format!("Не удалось проверить обновления: {e}")),
        _ => {}
    }
}

fn start_install(hwnd: HWND) {
    let Some(rel) = update::AVAILABLE.lock().unwrap().clone() else { return };
    let h = hwnd.0 as isize;
    std::thread::spawn(move || {
        notify("AntiPunto", format!("Загружаю версию {}…", rel.version));
        match update::install(&rel) {
            // установщик сам перезапустит программу
            Ok(()) => unsafe {
                let _ = PostMessageW(Some(HWND(h as *mut _)), WM_COMMAND, WPARAM(ID_EXIT), LPARAM(0));
            },
            Err(e) => notify("Обновление не установлено", e),
        }
    });
}

fn menu(hwnd: HWND) {
    unsafe {
        let Ok(m) = CreatePopupMenu() else { return };
        let on = if ENABLED.load(SeqCst) { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(m, MF_STRING | on, ID_TOGGLE, w!("Включено"));
        let spell = if settings::SPELL.load(SeqCst) { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(m, MF_STRING | spell, ID_SPELL, w!("Исправлять опечатки"));
        let sug = if settings::SUGGEST.load(SeqCst) { MF_CHECKED } else { MF_UNCHECKED };
        let grey = if settings::SPELL.load(SeqCst) { MF_ENABLED } else { MF_GRAYED };
        let _ = AppendMenuW(m, MF_STRING | sug | grey, ID_SUGGEST, w!("Предлагать варианты слова"));
        let status: Vec<u16> = STATUS.lock().unwrap().encode_utf16().chain([0]).collect();
        let _ = AppendMenuW(m, MF_STRING | MF_GRAYED, 0, PCWSTR(status.as_ptr()));
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        let app = LAST_APP.lock().unwrap().clone();
        if !app.is_empty() {
            let label = wide(&format!("Не исправлять в {app}"));
            let _ = AppendMenuW(m, MF_STRING, ID_SKIP_APP, PCWSTR(label.as_ptr()));
        }
        if let Ok(sub) = CreatePopupMenu() {
            let _ = AppendMenuW(sub, MF_STRING, ID_LIST_PROGRAMS, w!("Программы-исключения…"));
            let _ = AppendMenuW(sub, MF_STRING, ID_LIST_IGNORE, w!("Слова, которые не трогать…"));
            let _ = AppendMenuW(sub, MF_STRING, ID_LIST_FORCE, w!("Жёсткие слова…"));
            let _ = AppendMenuW(m, MF_STRING | MF_POPUP, sub.0 as usize, w!("Списки"));
        }
        let auto = if autostart::enabled() { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(m, MF_STRING | auto, ID_AUTOSTART, w!("Запускать при входе в Windows"));
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        let avail = update::AVAILABLE.lock().unwrap().as_ref().map(|r| r.version.clone());
        let label: Vec<u16> = match &avail {
            Some(v) => format!("Установить версию {v}"),
            None => format!("Проверить обновления (у вас {})", update::VERSION),
        }
        .encode_utf16()
        .chain([0])
        .collect();
        let id = if avail.is_some() { ID_UPDATE } else { ID_CHECK };
        let _ = AppendMenuW(m, MF_STRING, id, PCWSTR(label.as_ptr()));
        let _ = AppendMenuW(m, MF_STRING, ID_EXIT, w!("Выход"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(m, TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, None, hwnd, None);
        let _ = DestroyMenu(m);
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Открыть файл списка как двойным кликом; программа подхватит правку сама.
fn open_list(name: &str) {
    lists::ensure_files();
    let path = HSTRING::from(lists::path(name).as_os_str());
    let dir = HSTRING::from(lists::dir().as_os_str());
    unsafe {
        let h = ShellExecuteW(None, w!("open"), &path, PCWSTR::null(), &dir, SW_SHOWNORMAL);
        if (h.0 as isize) <= 32 {
            // нет ассоциации для .txt — явный Блокнот
            ShellExecuteW(None, w!("open"), w!("notepad.exe"), &path, &dir, SW_SHOWNORMAL);
        }
    }
}

fn toggle(hwnd: HWND) {
    ENABLED.fetch_xor(true, SeqCst);
    // пока было выключено, буфер устарел
    if let Some(tx) = SENDER.get() {
        let _ = tx.send(Event::Mouse);
    }
    refresh(hwnd, true);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TRAY => {
                match (lp.0 & 0xFFFF) as u32 {
                    WM_LBUTTONUP => toggle(hwnd),
                    WM_RBUTTONUP | WM_CONTEXTMENU => menu(hwnd),
                    NIN_BALLOONUSERCLICK => start_install(hwnd),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_NOTIFY_USER => {
                balloon(hwnd);
                LRESULT(0)
            }
            WM_COMMAND => {
                match wp.0 & 0xFFFF {
                    ID_TOGGLE => toggle(hwnd),
                    ID_CHECK => {
                        std::thread::spawn(|| check_updates(true));
                    }
                    ID_UPDATE => start_install(hwnd),
                    ID_AUTOSTART => autostart::set(!autostart::enabled()),
                    ID_SPELL => settings::set_spell(!settings::SPELL.load(SeqCst)),
                    ID_SUGGEST => settings::set_suggest(!settings::SUGGEST.load(SeqCst)),
                    ID_SKIP_APP => {
                        let app = LAST_APP.lock().unwrap().clone();
                        match lists::add_program(&app) {
                            Ok(()) => notify("AntiPunto", format!("{app} добавлена в исключения")),
                            Err(e) => notify("AntiPunto", format!("Не удалось записать список: {e}")),
                        }
                    }
                    ID_LIST_PROGRAMS => open_list(lists::PROGRAMS),
                    ID_LIST_IGNORE => open_list(lists::IGNORE),
                    ID_LIST_FORCE => open_list(lists::FORCE),
                    ID_EXIT => {
                        let _ = DestroyWindow(hwnd);
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_TIMER => {
                refresh(hwnd, false);
                LRESULT(0)
            }
            WM_FENCE => LRESULT(0),
            WM_DESTROY => {
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid(hwnd));
                PostQuitMessage(0);
                LRESULT(0)
            }
            m if m != 0 && m == TASKBAR_CREATED.get() => {
                refresh(hwnd, true);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// Создать окно трея и крутить цикл сообщений до выхода.
pub fn run(auto_update_check: bool) {
    if auto_update_check {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(60));
            loop {
                check_updates(false);
                std::thread::sleep(std::time::Duration::from_secs(24 * 3600));
            }
        });
    }
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let class = w!("AntiPunto_tray");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("AntiPunto"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinst.into()),
            None,
        ) else {
            return;
        };
        MAIN_HWND.store(hwnd.0 as isize, SeqCst);
        popup::create(hinst.into());
        TASKBAR_CREATED.set(RegisterWindowMessageW(w!("TaskbarCreated")));

        let size = GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem());
        ICONS.set([load_icon(ICO_RU, size), load_icon(ICO_EN, size), load_icon(ICO_OFF, size)]);
        refresh(hwnd, true);
        SetTimer(Some(hwnd), 1, 250, None);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        MAIN_HWND.store(0, SeqCst);
    }
}
