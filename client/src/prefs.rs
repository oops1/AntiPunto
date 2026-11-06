//! Окно настроек: отдельный запуск `--settings`; с резидентом общается реестром, файлами и сообщением.

use std::cell::RefCell;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    DRAWITEMSTRUCT, EM_GETMODIFY, EM_SETMODIFY, ODS_FOCUS, ODS_NOFOCUSRECT, ODS_SELECTED, ODT_LISTBOX, SetWindowTheme,
};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, HSTRING, PCWSTR, w};

use crate::lists::{self, Mode};
use crate::stats::Stats;
use crate::{autostart, lmstore, settings, theme, tray, update};

/// Клиентская область и боковая панель при 96 dpi.
const W: f32 = 680.0;
const H: f32 = 470.0;
const NAV_W: f32 = 170.0;
const X0: f32 = 198.0;
const CW: f32 = W - X0 - 28.0;

const PAGES: [&str; 6] = ["Общее", "Программы", "Слова", "Модели", "Статистика", "О программе"];
const P_GENERAL: usize = 0;
const P_PROGRAMS: usize = 1;
const P_WORDS: usize = 2;
const P_MODELS: usize = 3;
const P_STATS: usize = 4;
const P_ABOUT: usize = 5;

const ID_NAV: usize = 100; // + страница
const ID_SPELL: usize = 200;
const ID_SUGGEST: usize = 201;
const ID_AUTOSTART: usize = 202;
const ID_THEME: usize = 210; // + значение темы
const ID_PROGRAMS: usize = 300;
const ID_MODE: usize = 310; // + 0 всё, 1 раскладка, 2 не трогать
const ID_WORDS: usize = 400;
const ID_WORDLIST: usize = 410; // + номер списка
const ID_FOLDER: usize = 420;
const ID_RESET: usize = 500;
const ID_CHECK: usize = 600;
const ID_SITE: usize = 601;
const ID_LM_GET: usize = 700;
const ID_LM_DEL: usize = 701;
const ID_LM_USE: usize = 702;
const ID_LM_SRC: usize = 703;

const WM_UPDATE_DONE: u32 = WM_APP + 1;
const WM_LM: u32 = WM_APP + 2;
const TIMER_STATS: usize = 1;

/// Списки слов на странице «Слова»: файл, подпись, пояснение.
const WORDLISTS: [(&str, &str, &str); 2] = [
    (
        lists::IGNORE,
        "Не трогать",
        "Слова, которые программа никогда не исправляет: ники, сокращения, свои словечки. По одному на строку.",
    ),
    (
        lists::FORCE,
        "Жёсткие слова",
        "Слова, которые нужны всегда. Пишите так, как хотите видеть: набрано в другой раскладке — исправит, \
         набрано верно — не тронет. По одному на строку.",
    ),
];

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Nav,
    Toggle,
    Radio,
    Push,
}

fn kind(id: usize) -> Kind {
    match id {
        ID_NAV..ID_SPELL => Kind::Nav,
        ID_SPELL | ID_SUGGEST | ID_AUTOSTART | ID_LM_USE => Kind::Toggle,
        ID_THEME..ID_PROGRAMS | ID_MODE..ID_WORDS | ID_WORDLIST..ID_FOLDER => Kind::Radio,
        _ => Kind::Push,
    }
}

struct Prog {
    name: String,
    mode: Option<Mode>,
    fixes: u64,
}

#[derive(Default)]
struct State {
    page: usize,
    scale: f32,
    font: HFONT,
    bold: HFONT,
    title: HFONT,
    small: HFONT,
    /// Кисть фона полей под текущую тему.
    field: HBRUSH,
    /// Дочерние окна и их страницы.
    ctrls: Vec<(HWND, usize)>,
    progs: Vec<Prog>,
    stats: Stats,
    wordlist: usize,
}

thread_local! {
    static ST: RefCell<State> = RefCell::new(State::default());
}

/// Итог проверки обновлений из фонового потока.
static UPDATE: Mutex<String> = Mutex::new(String::new());

/// Состояние языковой модели на странице «Модели»; меняют фоновые потоки.
#[derive(Clone, PartialEq)]
enum LmState {
    Checking,
    Absent,
    /// Принято и всего, байт.
    Loading(u64, u64),
    Removing,
    Ready,
    /// Файлы есть, но не прошли сверку.
    Bad(String),
    Failed(String),
}

static LM: Mutex<LmState> = Mutex::new(LmState::Checking);
static LM_STOP: AtomicBool = AtomicBool::new(false);

// Изменяемый заём — только на время замыкания: внутри нельзя звать Win32, оно шлёт сообщения.
fn st<R>(f: impl FnOnce(&State) -> R) -> R {
    ST.with_borrow(f)
}

fn st_mut<R>(f: impl FnOnce(&mut State) -> R) -> R {
    ST.with_borrow_mut(f)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn px(v: f32) -> i32 {
    (v * st(|s| s.scale)).round() as i32
}

/// 12345 → «12 345».
fn num(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('\u{a0}');
        }
        out.push(c);
    }
    out
}

fn resident() -> Option<HWND> {
    unsafe { FindWindowW(tray::CLASS, None).ok().filter(|h| !h.is_invalid()) }
}

/// Команда резиденту; без него настройки и так прочтутся при запуске.
fn ctl(what: usize) {
    if let Some(h) = resident() {
        unsafe {
            let _ = PostMessageW(Some(h), tray::ctl_msg(), WPARAM(what), LPARAM(0));
        }
    }
}

fn load_progs() -> Vec<Prog> {
    let stats = Stats::load();
    let listed = lists::programs();
    let fixes = |n: &str| stats.by_app.get(n).copied().unwrap_or(0);
    let mut out: Vec<Prog> =
        listed.iter().map(|(n, m)| Prog { name: n.clone(), mode: Some(*m), fixes: fixes(n) }).collect();
    let mut rest: Vec<Prog> = stats
        .by_app
        .iter()
        .filter(|(n, _)| !n.is_empty() && !listed.iter().any(|(l, _)| l == *n))
        .map(|(n, f)| Prog { name: n.clone(), mode: None, fixes: *f })
        .collect();
    rest.sort_by_key(|p| std::cmp::Reverse(p.fixes));
    out.extend(rest);
    out
}

unsafe fn item(hwnd: HWND, id: usize) -> HWND {
    unsafe { GetDlgItem(Some(hwnd), id as i32).unwrap_or_default() }
}

unsafe fn redraw(hwnd: HWND, id: usize) {
    unsafe {
        let _ = InvalidateRect(Some(item(hwnd, id)), None, false);
    }
}

unsafe fn redraw_all(hwnd: HWND) {
    unsafe {
        let _ = RedrawWindow(Some(hwnd), None, None, RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN);
    }
}

unsafe fn window_text(h: HWND) -> String {
    unsafe {
        let mut buf = vec![0u16; GetWindowTextLengthW(h) as usize + 1];
        let n = GetWindowTextW(h, &mut buf) as usize;
        String::from_utf16_lossy(&buf[..n])
    }
}

/// Выбранная строка списка программ.
unsafe fn selected(hwnd: HWND) -> Option<usize> {
    unsafe {
        let i = SendMessageW(item(hwnd, ID_PROGRAMS), LB_GETCURSEL, None, None).0;
        (i >= 0 && (i as usize) < st(|s| s.progs.len())).then_some(i as usize)
    }
}

/// Отмечен ли переключатель или выбран ли вариант.
unsafe fn checked(hwnd: HWND, id: usize) -> bool {
    match id {
        ID_SPELL => settings::SPELL.load(SeqCst),
        ID_SUGGEST => settings::SUGGEST.load(SeqCst),
        ID_AUTOSTART => autostart::enabled(),
        ID_LM_USE => settings::LM.load(SeqCst),
        ID_NAV..ID_SPELL => st(|s| s.page) == id - ID_NAV,
        ID_THEME..ID_PROGRAMS => settings::THEME.load(SeqCst) as usize == id - ID_THEME,
        ID_MODE..ID_WORDS => {
            let mode = unsafe { selected(hwnd) }.map(|i| st(|s| s.progs[i].mode));
            matches!((id - ID_MODE, mode), (0, Some(None)) | (1, Some(Some(Mode::Layout))) | (2, Some(Some(Mode::Off))))
        }
        ID_WORDLIST..ID_FOLDER => st(|s| s.wordlist) == id - ID_WORDLIST,
        _ => false,
    }
}

unsafe fn fill(dc: HDC, r: &RECT, c: COLORREF) {
    unsafe {
        let b = CreateSolidBrush(c);
        FillRect(dc, r, b);
        let _ = DeleteObject(b.into());
    }
}

unsafe fn frame(dc: HDC, r: &RECT, c: COLORREF) {
    unsafe {
        let b = CreateSolidBrush(c);
        FrameRect(dc, r, b);
        let _ = DeleteObject(b.into());
    }
}

/// Текст с переносом по ширине; возвращает нижний край.
unsafe fn text(dc: HDC, font: HFONT, color: COLORREF, x: f32, y: f32, w: f32, s: &str) -> i32 {
    // пустую строку DrawTextW читает по указателю как нуль-терминированную
    if s.is_empty() {
        return px(y);
    }
    unsafe {
        SelectObject(dc, font.into());
        SetTextColor(dc, color);
        let mut r = RECT { left: px(x), top: px(y), right: px(x + w), bottom: px(y) + 10_000 };
        let h = DrawTextW(dc, &mut wide(s), &mut r, DT_WORDBREAK | DT_NOPREFIX | DT_LEFT);
        px(y) + h
    }
}

/// Нарисовать в память и вывести разом: без мерцания и недорисованных кадров.
unsafe fn buffered(dc: HDC, r: &RECT, draw: impl FnOnce(HDC, RECT)) {
    unsafe {
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let mem = CreateCompatibleDC(Some(dc));
        let bmp = CreateCompatibleBitmap(dc, w, h);
        let old = SelectObject(mem, bmp.into());
        draw(mem, RECT { left: 0, top: 0, right: w, bottom: h });
        let _ = BitBlt(dc, r.left, r.top, w, h, Some(mem), 0, 0, SRCCOPY);
        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
    }
}

unsafe fn draw_button(hwnd: HWND, d: &DRAWITEMSTRUCT, dc: HDC, r: RECT) {
    unsafe {
        let pal = theme::palette();
        let id = d.CtlID as usize;
        let pressed = d.itemState.0 & ODS_SELECTED.0 != 0;
        let focus = d.itemState.0 & ODS_FOCUS.0 != 0 && d.itemState.0 & ODS_NOFOCUSRECT.0 == 0;
        let mut label = wide(&window_text(d.hwndItem));
        let on = checked(hwnd, id);
        SetBkMode(dc, TRANSPARENT);
        SelectObject(dc, st(|s| s.font).into());
        SetTextColor(dc, pal.ink);
        let line = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX;
        match kind(id) {
            Kind::Nav => {
                fill(dc, &r, if on || pressed { pal.active } else { pal.panel });
                if on {
                    fill(dc, &RECT { right: r.left + px(3.0), ..r }, pal.blue);
                }
                let mut tr = RECT { left: r.left + px(18.0), ..r };
                DrawTextW(dc, &mut label, &mut tr, line | DT_LEFT);
            }
            Kind::Toggle => {
                fill(dc, &r, pal.bg);
                let b = px(18.0);
                let (left, top) = (r.left + px(3.0), (r.top + r.bottom - b) / 2);
                let bx = RECT { left, top, right: left + b, bottom: top + b };
                if on {
                    fill(dc, &bx, pal.blue);
                    let pen = CreatePen(PS_SOLID, px(2.0), pal.bg);
                    let old = SelectObject(dc, pen.into());
                    let _ = MoveToEx(dc, left + px(4.0), top + px(9.0), None);
                    let _ = LineTo(dc, left + px(8.0), top + px(13.0));
                    let _ = LineTo(dc, left + px(14.0), top + px(5.0));
                    SelectObject(dc, old);
                    let _ = DeleteObject(pen.into());
                } else {
                    fill(dc, &bx, if pressed { pal.active } else { pal.bg });
                    frame(dc, &bx, pal.frame);
                }
                let mut tr = RECT { left: bx.right + px(10.0), ..r };
                DrawTextW(dc, &mut label, &mut tr, line | DT_LEFT);
            }
            Kind::Radio => {
                if on {
                    fill(dc, &r, pal.blue);
                    SetTextColor(dc, pal.bg);
                } else {
                    fill(dc, &r, if pressed { pal.active } else { pal.panel });
                    frame(dc, &r, pal.line);
                }
                let mut tr = r;
                DrawTextW(dc, &mut label, &mut tr, line | DT_CENTER);
            }
            Kind::Push => {
                fill(dc, &r, if pressed { pal.active } else { pal.panel });
                frame(dc, &r, pal.frame);
                let mut tr = r;
                DrawTextW(dc, &mut label, &mut tr, line | DT_CENTER);
            }
        }
        if focus {
            let k = px(3.0);
            SetTextColor(dc, pal.ink);
            let _ =
                DrawFocusRect(dc, &RECT { left: r.left + k, top: r.top + k, right: r.right - k, bottom: r.bottom - k });
        }
    }
}

/// Строка списка программ: имя, число правок, режим.
unsafe fn draw_prog(d: &DRAWITEMSTRUCT, dc: HDC, r: RECT) {
    unsafe {
        let pal = theme::palette();
        let sel = d.itemState.0 & ODS_SELECTED.0 != 0;
        fill(dc, &r, if sel { pal.active } else { pal.bg });
        let Some((name, mode, fixes)) =
            st(|s| s.progs.get(d.itemID as usize).map(|p| (p.name.clone(), p.mode, p.fixes)))
        else {
            return;
        };
        SetBkMode(dc, TRANSPARENT);
        let line = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS;
        let (font, small) = st(|s| (s.font, s.small));
        SelectObject(dc, font.into());
        SetTextColor(dc, pal.ink);
        let mut nr = RECT { left: r.left + px(10.0), right: r.right - px(250.0), ..r };
        DrawTextW(dc, &mut wide(&name), &mut nr, line | DT_LEFT);
        let (label, color) = match mode {
            None => ("исправлять всё", pal.grey),
            Some(Mode::Layout) => ("только раскладка", pal.blue),
            Some(Mode::Off) => ("не трогать", pal.red),
        };
        SetTextColor(dc, color);
        let mut mr = RECT { left: r.right - px(140.0), right: r.right - px(10.0), ..r };
        DrawTextW(dc, &mut wide(label), &mut mr, line | DT_RIGHT);
        if fixes > 0 {
            SelectObject(dc, small.into());
            SetTextColor(dc, pal.grey);
            let mut fr = RECT { left: r.right - px(245.0), right: r.right - px(150.0), ..r };
            DrawTextW(dc, &mut wide(&format!("правок: {}", num(fixes))), &mut fr, line | DT_RIGHT);
        }
    }
}

unsafe fn paint(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let wdc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        buffered(wdc, &rc, |dc, rc| paint_page(dc, rc));
        let _ = EndPaint(hwnd, &ps);
    }
}

unsafe fn paint_page(dc: HDC, rc: RECT) {
    unsafe {
        let pal = theme::palette();
        fill(dc, &rc, pal.bg);
        fill(dc, &RECT { right: px(NAV_W), ..rc }, pal.panel);
        fill(dc, &RECT { left: px(NAV_W), right: px(NAV_W) + 1, ..rc }, pal.line);
        SetBkMode(dc, TRANSPARENT);
        let (page, font, bold, title, small) = st(|s| (s.page, s.font, s.bold, s.title, s.small));
        text(dc, bold, pal.ink, 18.0, 20.0, NAV_W, "AntiPunto");
        text(dc, small, pal.grey, 18.0, 42.0, NAV_W, &format!("версия {}", update::VERSION));
        text(dc, title, pal.ink, X0, 16.0, CW, PAGES[page]);
        match page {
            P_GENERAL => {
                text(
                    dc,
                    small,
                    pal.grey,
                    X0 + 31.0,
                    94.0,
                    CW - 31.0,
                    "Раскладка исправляется всегда, пока программа включена.",
                );
                text(
                    dc,
                    small,
                    pal.grey,
                    X0 + 31.0,
                    152.0,
                    CW - 31.0,
                    "Окно у курсора, когда программа сомневается. Выбор — Ctrl+цифра или щелчок.",
                );
                text(dc, bold, pal.ink, X0, 244.0, CW, "Оформление");
                text(dc, small, pal.grey, X0, 267.0, CW, "Тема окна вариантов и этого окна.");
            }
            P_PROGRAMS => {
                text(
                    dc,
                    small,
                    pal.grey,
                    X0,
                    62.0,
                    CW,
                    "Для каждой программы можно выбрать, что исправлять. В списке — программы с особым режимом \
                     и те, где AntiPunto уже что-то правил.",
                );
                text(dc, font, pal.ink, X0, 340.0, CW, "В выбранной программе:");
                text(
                    dc,
                    small,
                    pal.grey,
                    X0,
                    410.0,
                    CW,
                    "Программу не из списка добавляет пункт «Не исправлять в…» в меню значка у часов.",
                );
            }
            P_WORDS => {
                text(dc, small, pal.grey, X0, 102.0, CW, WORDLISTS[st(|s| s.wordlist)].2);
                text(dc, small, pal.grey, X0 + 214.0, 399.0, CW - 214.0, "Изменения сохраняются сами.");
            }
            P_MODELS => paint_models(dc),
            P_STATS => paint_stats(dc),
            _ => {
                text(dc, bold, pal.ink, X0, 66.0, CW, "Исправляет раскладку и опечатки по смыслу фразы.");
                text(
                    dc,
                    font,
                    pal.ink,
                    X0,
                    96.0,
                    CW,
                    "Нейросеть работает на этом компьютере. Набранный текст никуда не отправляется и не сохраняется.",
                );
                let status = UPDATE.lock().unwrap().clone();
                text(dc, font, pal.grey, X0, 160.0, CW, &status);
                text(
                    dc,
                    small,
                    pal.grey,
                    X0,
                    250.0,
                    CW,
                    &format!("Списки слов и программ: {}", lists::dir().display()),
                );
                text(dc, small, pal.grey, X0, 290.0, CW, "Открытый код, лицензия MIT.");
            }
        }
    }
}

unsafe fn paint_stats(dc: HDC) {
    unsafe {
        let pal = theme::palette();
        let (font, bold, small) = st(|s| (s.font, s.bold, s.small));
        let s = st(|s| s.stats.clone());
        let ms = if s.words > 0 { s.infer_us as f64 / s.words as f64 / 1000.0 } else { 0.0 };
        let rows = [
            ("Проверено слов", num(s.words)),
            ("Исправлено раскладок", num(s.fixes)),
            ("Исправлено опечаток", num(s.spell_fixes)),
            ("Выбрано вариантов из окна у курсора", num(s.picks)),
            ("Отменено клавишей Backspace", num(s.undos)),
            ("Переключено вручную (двойной правый Ctrl)", num(s.forced)),
            ("Пропущено в полях паролей", num(s.password_skips)),
            ("Нейросеть думает над словом, в среднем", format!("{ms:.1} мс").replace('.', ",")),
        ];
        let mut y = 66.0;
        for (label, value) in rows {
            text(dc, font, pal.ink, X0, y, 330.0, label);
            SelectObject(dc, bold.into());
            SetTextColor(dc, pal.ink);
            let mut r = RECT { left: px(X0 + 330.0), top: px(y), right: px(X0 + CW), bottom: px(y + 24.0) };
            DrawTextW(dc, &mut wide(&value), &mut r, DT_SINGLELINE | DT_RIGHT | DT_NOPREFIX);
            y += 27.0;
        }
        let mut top: Vec<(&String, &u64)> = s.by_app.iter().filter(|(n, _)| !n.is_empty()).collect();
        top.sort_by(|a, b| b.1.cmp(a.1));
        if !top.is_empty() {
            let list: Vec<String> = top.iter().take(4).map(|(n, f)| format!("{n} — {}", num(**f))).collect();
            text(dc, small, pal.grey, X0, y + 8.0, CW, &format!("Больше всего правок раскладки: {}.", list.join(", ")));
        }
        text(
            dc,
            small,
            pal.grey,
            X0 + 204.0,
            392.0,
            CW - 204.0,
            "Считаются только числа — набранный текст не сохраняется.",
        );
    }
}

fn mb(bytes: u64) -> u64 {
    (bytes + (1 << 19)) >> 20
}

unsafe fn paint_models(dc: HDC) {
    unsafe {
        let pal = theme::palette();
        let (font, bold, small) = st(|s| (s.font, s.bold, s.small));
        text(dc, bold, pal.ink, X0, 62.0, CW, "Языковая модель rugpt3small");
        text(
            dc,
            small,
            pal.grey,
            X0,
            86.0,
            CW,
            "Смотрит на текст вокруг слова и помогает выбрать исправление: исправляется больше опечаток, \
             в том числе тех, где получилось другое слово. Работает на этом компьютере, только для русского языка.",
        );
        let total = mb(lmstore::manifest().total());
        let (line, color) = match LM.lock().unwrap().clone() {
            LmState::Checking => ("Проверяю файлы модели…".to_string(), pal.grey),
            LmState::Absent => (format!("Не установлена. Размер закачки — {total} МБ."), pal.ink),
            LmState::Loading(done, all) => {
                (format!("Загрузка: {} % ({} из {} МБ)", done * 100 / all.max(1), mb(done), mb(all)), pal.blue)
            }
            LmState::Removing => ("Удаляю…".to_string(), pal.grey),
            LmState::Ready => ("Установлена, файлы проверены.".to_string(), pal.green),
            LmState::Bad(e) => (format!("Не используется: {e}. Скачайте модель заново."), pal.red),
            LmState::Failed(e) => (format!("Не получилось: {e}"), pal.red),
        };
        text(dc, font, color, X0, 142.0, CW, &line);
        text(
            dc,
            small,
            pal.grey,
            X0 + 31.0,
            274.0,
            CW - 31.0,
            "Если печатать стало заметно медленнее — выключите: модель останется на диске.",
        );
        text(
            dc,
            small,
            pal.grey,
            X0,
            318.0,
            CW,
            "Модель скачивается из репозитория AntiPunto на GitHub. Перед каждым использованием её файлы \
             сверяются с контрольными суммами, вшитыми в программу: изменённая модель не загрузится.",
        );
    }
}

fn lm_post(hwnd: isize) {
    unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_LM, WPARAM(0), LPARAM(0));
    }
}

fn lm_set(hwnd: isize, s: LmState) {
    *LM.lock().unwrap() = s;
    lm_post(hwnd);
}

/// Сверить установленные файлы с перечнем — в фоне: это чтение 170 МБ.
fn lm_check(hwnd: HWND) {
    if matches!(*LM.lock().unwrap(), LmState::Loading(..) | LmState::Removing) {
        return;
    }
    let dir = lmstore::dir();
    if !lmstore::installed(&dir) {
        // неполный набор файлов — то же, что нет модели
        *LM.lock().unwrap() = LmState::Absent;
        return;
    }
    *LM.lock().unwrap() = LmState::Checking;
    let h = hwnd.0 as isize;
    std::thread::spawn(move || {
        lm_set(
            h,
            match lmstore::verify(&dir) {
                Ok(()) => LmState::Ready,
                Err(e) => LmState::Bad(e),
            },
        );
    });
}

fn lm_download(hwnd: HWND) {
    LM_STOP.store(false, SeqCst);
    let total = lmstore::manifest().total();
    *LM.lock().unwrap() = LmState::Loading(0, total);
    // резидент отпускает прежние файлы: их заменят
    ctl(tray::CTL_LM_UNLOAD);
    let h = hwnd.0 as isize;
    std::thread::spawn(move || {
        let progress = |done: u64, all: u64| {
            let mut s = LM.lock().unwrap();
            let step = all.max(1) / 200;
            let moved = !matches!(*s, LmState::Loading(d, _) if done < d + step && done < all);
            if moved {
                *s = LmState::Loading(done, all);
                drop(s);
                lm_post(h);
            }
        };
        let done = lmstore::install(&lmstore::dir(), &progress, &LM_STOP);
        lm_set(
            h,
            match done {
                Ok(()) => LmState::Ready,
                Err(_) if LM_STOP.load(SeqCst) => LmState::Absent,
                Err(e) => LmState::Failed(e),
            },
        );
        ctl(tray::CTL_RELOAD);
    });
}

fn lm_remove(hwnd: HWND) {
    *LM.lock().unwrap() = LmState::Removing;
    ctl(tray::CTL_LM_UNLOAD);
    let h = hwnd.0 as isize;
    std::thread::spawn(move || {
        // резидент отпускает файлы не мгновенно
        let mut last = Ok(());
        for _ in 0..40 {
            last = lmstore::remove(&lmstore::dir());
            if last.is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        lm_set(
            h,
            match last {
                Ok(()) => LmState::Absent,
                Err(e) => LmState::Failed(format!("файлы заняты ({e})")),
            },
        );
        ctl(tray::CTL_RELOAD);
    });
}

/// Кнопки страницы «Модели» под текущее состояние.
unsafe fn lm_buttons(hwnd: HWND) {
    unsafe {
        if st(|s| s.page) != P_MODELS {
            return;
        }
        let s = LM.lock().unwrap().clone();
        let (get, del) = match s {
            LmState::Loading(..) => (Some("Отменить"), false),
            LmState::Checking | LmState::Removing => (None, false),
            LmState::Ready => (None, true),
            LmState::Bad(_) => (Some("Скачать заново"), true),
            LmState::Absent | LmState::Failed(_) => (Some("Скачать и установить"), lmstore::installed(&lmstore::dir())),
        };
        let (bg, bd) = (item(hwnd, ID_LM_GET), item(hwnd, ID_LM_DEL));
        if let Some(label) = get {
            let _ = SetWindowTextW(bg, &HSTRING::from(label));
        }
        // фокус не должен остаться на спрятанной кнопке
        let focus = GetFocus();
        if (focus == bg && get.is_none()) || (focus == bd && !del) {
            let _ = SetFocus(Some(item(hwnd, ID_NAV + P_MODELS)));
        }
        let _ = ShowWindow(bg, if get.is_some() { SW_SHOW } else { SW_HIDE });
        let _ = ShowWindow(bd, if del { SW_SHOW } else { SW_HIDE });
        let _ = SetWindowPos(
            bd,
            None,
            px(if get.is_some() { X0 + 228.0 } else { X0 }),
            px(192.0),
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        redraw(hwnd, ID_LM_GET);
    }
}

/// Заполнить список программ заново, сохранив выбор.
unsafe fn fill_progs(hwnd: HWND) {
    unsafe {
        let lb = item(hwnd, ID_PROGRAMS);
        let keep = selected(hwnd).map(|i| st(|s| s.progs[i].name.clone()));
        let progs = load_progs();
        let sel = keep.and_then(|k| progs.iter().position(|p| p.name == k));
        let n = progs.len();
        st_mut(|s| s.progs = progs);
        SendMessageW(lb, LB_RESETCONTENT, None, None);
        for i in 0..n {
            SendMessageW(lb, LB_ADDSTRING, None, Some(LPARAM(i as isize)));
        }
        if n > 0 {
            SendMessageW(lb, LB_SETCURSEL, Some(WPARAM(sel.unwrap_or(0))), None);
        }
        for k in 0..3 {
            redraw(hwnd, ID_MODE + k);
        }
    }
}

/// Режим выбранной программы: None — исправлять всё.
unsafe fn set_mode(hwnd: HWND, mode: Option<Mode>) {
    unsafe {
        let Some(i) = selected(hwnd) else { return };
        let listed: Vec<(String, Mode)> = st_mut(|s| {
            s.progs[i].mode = mode;
            s.progs.iter().filter_map(|p| p.mode.map(|m| (p.name.clone(), m))).collect()
        });
        if let Err(e) = lists::save_programs(&listed) {
            let msg = HSTRING::from(format!("Не удалось записать список программ: {e}"));
            MessageBoxW(Some(hwnd), &msg, w!("AntiPunto"), MB_OK | MB_ICONWARNING);
        }
        let _ = InvalidateRect(Some(item(hwnd, ID_PROGRAMS)), None, false);
        for k in 0..3 {
            redraw(hwnd, ID_MODE + k);
        }
    }
}

unsafe fn load_words(hwnd: HWND) {
    unsafe {
        let edit = item(hwnd, ID_WORDS);
        let body = lists::entries(WORDLISTS[st(|s| s.wordlist)].0).join("\r\n");
        let _ = SetWindowTextW(edit, &HSTRING::from(body));
        SendMessageW(edit, EM_SETMODIFY, Some(WPARAM(0)), None);
    }
}

/// Записать поле слов, если его правили.
unsafe fn save_words(hwnd: HWND) {
    unsafe {
        let edit = item(hwnd, ID_WORDS);
        if SendMessageW(edit, EM_GETMODIFY, None, None).0 == 0 {
            return;
        }
        let lines: Vec<String> = window_text(edit).lines().map(String::from).collect();
        match lists::save_entries(WORDLISTS[st(|s| s.wordlist)].0, &lines) {
            Ok(()) => {
                SendMessageW(edit, EM_SETMODIFY, Some(WPARAM(0)), None);
            }
            Err(e) => {
                let msg = HSTRING::from(format!("Не удалось записать список: {e}"));
                MessageBoxW(Some(hwnd), &msg, w!("AntiPunto"), MB_OK | MB_ICONWARNING);
            }
        }
    }
}

unsafe fn set_page(hwnd: HWND, page: usize) {
    unsafe {
        save_words(hwnd);
        let old = st_mut(|s| std::mem::replace(&mut s.page, page));
        for (h, p) in st(|s| s.ctrls.clone()) {
            let _ = ShowWindow(h, if p == page || p == usize::MAX { SW_SHOW } else { SW_HIDE });
        }
        match page {
            P_PROGRAMS => fill_progs(hwnd),
            P_WORDS => load_words(hwnd),
            P_MODELS => {
                lm_check(hwnd);
                lm_buttons(hwnd);
            }
            P_STATS => {
                st_mut(|s| s.stats = Stats::load());
                // резидент пишет счётчики раз в 30 с — попросить свежие
                ctl(tray::CTL_FLUSH);
                SetTimer(Some(hwnd), TIMER_STATS, 350, None);
            }
            _ => {}
        }
        redraw(hwnd, ID_NAV + old);
        redraw(hwnd, ID_NAV + page);
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
}

/// Тёмный заголовок и полосы прокрутки; кисть полей.
unsafe fn apply_theme(hwnd: HWND) {
    unsafe {
        let dark = theme::dark();
        let flag = BOOL::from(dark);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, (&raw const flag).cast(), 4);
        let look = if dark { w!("DarkMode_Explorer") } else { w!("Explorer") };
        for id in [ID_PROGRAMS, ID_WORDS] {
            let _ = SetWindowTheme(item(hwnd, id), look, PCWSTR::null());
        }
        let brush = CreateSolidBrush(theme::palette().bg);
        let old = st_mut(|s| std::mem::replace(&mut s.field, brush));
        if !old.is_invalid() {
            let _ = DeleteObject(old.into());
        }
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED;
        let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, flags);
        redraw_all(hwnd);
    }
}

unsafe fn command(hwnd: HWND, id: usize, code: u32) {
    unsafe {
        match id {
            // Enter в окне: нажать то, что в фокусе
            1 => {
                let f = GetFocus();
                if GetParent(f).ok() == Some(hwnd) && f != item(hwnd, ID_PROGRAMS) {
                    SendMessageW(f, BM_CLICK, None, None);
                }
            }
            2 => {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            ID_NAV..ID_SPELL => set_page(hwnd, id - ID_NAV),
            ID_SPELL => {
                settings::set_spell(!settings::SPELL.load(SeqCst));
                ctl(tray::CTL_RELOAD);
                redraw(hwnd, id);
            }
            ID_SUGGEST => {
                settings::set_suggest(!settings::SUGGEST.load(SeqCst));
                ctl(tray::CTL_RELOAD);
                redraw(hwnd, id);
            }
            ID_AUTOSTART => {
                autostart::set(!autostart::enabled());
                redraw(hwnd, id);
            }
            ID_THEME..ID_PROGRAMS => {
                settings::set_theme((id - ID_THEME) as u32);
                ctl(tray::CTL_RELOAD);
                apply_theme(hwnd);
            }
            ID_PROGRAMS if code == LBN_SELCHANGE => {
                for k in 0..3 {
                    redraw(hwnd, ID_MODE + k);
                }
            }
            ID_MODE..ID_WORDS => set_mode(hwnd, [None, Some(Mode::Layout), Some(Mode::Off)][id - ID_MODE]),
            ID_WORDS if code == EN_KILLFOCUS => save_words(hwnd),
            ID_WORDLIST..ID_FOLDER => {
                save_words(hwnd);
                st_mut(|s| s.wordlist = id - ID_WORDLIST);
                load_words(hwnd);
                for k in 0..WORDLISTS.len() {
                    redraw(hwnd, ID_WORDLIST + k);
                }
                let _ = InvalidateRect(Some(hwnd), None, true);
            }
            ID_FOLDER => {
                lists::ensure_files();
                let dir = HSTRING::from(lists::dir().as_os_str());
                ShellExecuteW(None, w!("open"), &dir, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
            }
            ID_RESET => {
                let q = w!("Обнулить все счётчики?");
                if MessageBoxW(Some(hwnd), q, w!("AntiPunto"), MB_YESNO | MB_ICONQUESTION) == IDYES {
                    if resident().is_some() {
                        ctl(tray::CTL_RESET);
                    } else {
                        let _ = std::fs::remove_file(crate::stats::dir().join(crate::stats::FILE));
                    }
                    st_mut(|s| s.stats = Stats::default());
                    SetTimer(Some(hwnd), TIMER_STATS, 350, None);
                    let _ = InvalidateRect(Some(hwnd), None, true);
                }
            }
            ID_CHECK => {
                *UPDATE.lock().unwrap() = "Проверяю…".into();
                let _ = InvalidateRect(Some(hwnd), None, true);
                let h = hwnd.0 as isize;
                std::thread::spawn(move || {
                    let text = match update::check() {
                        Ok(Some(rel)) => {
                            format!("Вышла версия {}. Установить её можно из меню значка у часов.", rel.version)
                        }
                        Ok(None) => format!("Установлена последняя версия {}.", update::VERSION),
                        Err(e) => format!("Не удалось проверить обновления: {e}"),
                    };
                    *UPDATE.lock().unwrap() = text;
                    let _ = PostMessageW(Some(HWND(h as *mut _)), WM_UPDATE_DONE, WPARAM(0), LPARAM(0));
                });
            }
            ID_LM_GET => {
                if matches!(*LM.lock().unwrap(), LmState::Loading(..)) {
                    LM_STOP.store(true, SeqCst);
                } else {
                    lm_download(hwnd);
                }
                lm_buttons(hwnd);
                let _ = InvalidateRect(Some(hwnd), None, true);
            }
            ID_LM_DEL => {
                let q = w!("Удалить языковую модель с диска? Её можно будет скачать снова.");
                if MessageBoxW(Some(hwnd), q, w!("AntiPunto"), MB_YESNO | MB_ICONQUESTION) == IDYES {
                    lm_remove(hwnd);
                    lm_buttons(hwnd);
                    let _ = InvalidateRect(Some(hwnd), None, true);
                }
            }
            ID_LM_USE => {
                settings::set_lm(!settings::LM.load(SeqCst));
                ctl(tray::CTL_RELOAD);
                redraw(hwnd, id);
            }
            ID_LM_SRC => {
                let url = HSTRING::from(format!(
                    "https://github.com/{}/blob/main/models/Ext/rugpt3small/SOURCE.md",
                    update::REPO
                ));
                ShellExecuteW(None, w!("open"), &url, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
            }
            ID_SITE => {
                let url = HSTRING::from(format!("https://github.com/{}", update::REPO));
                ShellExecuteW(None, w!("open"), &url, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
            }
            _ => {}
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_COMMAND => {
                command(hwnd, wp.0 & 0xFFFF, ((wp.0 >> 16) & 0xFFFF) as u32);
                LRESULT(0)
            }
            WM_DRAWITEM => {
                let d = &*(lp.0 as *const DRAWITEMSTRUCT);
                if d.CtlType == ODT_LISTBOX {
                    buffered(d.hDC, &d.rcItem, |dc, r| draw_prog(d, dc, r));
                } else {
                    buffered(d.hDC, &d.rcItem, |dc, r| draw_button(hwnd, d, dc, r));
                }
                LRESULT(1)
            }
            WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORSTATIC => {
                let pal = theme::palette();
                let dc = HDC(wp.0 as *mut _);
                SetTextColor(dc, pal.ink);
                SetBkColor(dc, pal.bg);
                LRESULT(st(|s| s.field).0 as isize)
            }
            WM_TIMER => {
                let _ = KillTimer(Some(hwnd), TIMER_STATS);
                st_mut(|s| s.stats = Stats::load());
                let _ = InvalidateRect(Some(hwnd), None, true);
                LRESULT(0)
            }
            WM_UPDATE_DONE => {
                let _ = InvalidateRect(Some(hwnd), None, true);
                LRESULT(0)
            }
            WM_LM => {
                lm_buttons(hwnd);
                let _ = InvalidateRect(Some(hwnd), None, true);
                LRESULT(0)
            }
            WM_SETTINGCHANGE => {
                // сменили тему Windows
                apply_theme(hwnd);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                paint(hwnd);
                LRESULT(0)
            }
            WM_CLOSE => {
                save_words(hwnd);
                LM_STOP.store(true, SeqCst);
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// Создать дочернее окно страницы; MAX — на всех страницах.
#[allow(clippy::too_many_arguments)]
unsafe fn child(hwnd: HWND, class: PCWSTR, label: &str, style: u32, id: usize, page: usize, r: (f32, f32, f32, f32)) {
    unsafe {
        let style = WINDOW_STYLE(style) | WS_CHILD | WS_TABSTOP;
        let Ok(h) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            &HSTRING::from(label),
            style,
            px(r.0),
            px(r.1),
            px(r.2),
            px(r.3),
            Some(hwnd),
            Some(HMENU(id as *mut _)),
            None,
            None,
        ) else {
            return;
        };
        SendMessageW(h, WM_SETFONT, Some(WPARAM(st(|s| s.font).0 as usize)), Some(LPARAM(1)));
        st_mut(|s| s.ctrls.push((h, page)));
    }
}

unsafe fn build(hwnd: HWND) {
    unsafe {
        let button = |label: &str, id: usize, page: usize, r: (f32, f32, f32, f32)| {
            child(hwnd, w!("BUTTON"), label, BS_OWNERDRAW as u32, id, page, r)
        };
        for (i, name) in PAGES.iter().enumerate() {
            button(name, ID_NAV + i, usize::MAX, (0.0, 76.0 + 36.0 * i as f32, NAV_W, 36.0));
        }
        button("Исправлять опечатки", ID_SPELL, P_GENERAL, (X0, 66.0, CW, 26.0));
        button("Предлагать варианты слова", ID_SUGGEST, P_GENERAL, (X0, 124.0, CW, 26.0));
        button("Запускать при входе в Windows", ID_AUTOSTART, P_GENERAL, (X0, 196.0, CW, 26.0));
        let themes = [("Как в Windows", 150.0), ("Светлая", 110.0), ("Тёмная", 110.0)];
        let mut x = X0;
        for (i, (name, w)) in themes.iter().enumerate() {
            button(name, ID_THEME + i, P_GENERAL, (x, 292.0, *w, 30.0));
            x += w + 8.0;
        }

        let list = (LBS_OWNERDRAWFIXED | LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32 | WS_VSCROLL.0 | WS_BORDER.0;
        child(hwnd, w!("LISTBOX"), "", list, ID_PROGRAMS, P_PROGRAMS, (X0, 104.0, CW, 216.0));
        // высота — ровно восемь строк и рамка
        let lb = item(hwnd, ID_PROGRAMS);
        SendMessageW(lb, LB_SETITEMHEIGHT, Some(WPARAM(0)), Some(LPARAM(px(28.0) as isize)));
        let _ = SetWindowPos(lb, None, 0, 0, px(CW), px(28.0) * 8 + 2, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        let modes = [("Исправлять всё", 150.0), ("Только раскладку", 170.0), ("Не трогать", 118.0)];
        let mut x = X0;
        for (i, (name, w)) in modes.iter().enumerate() {
            button(name, ID_MODE + i, P_PROGRAMS, (x, 366.0, *w, 30.0));
            x += w + 8.0;
        }

        let mut x = X0;
        for (i, (_, name, _)) in WORDLISTS.iter().enumerate() {
            let w = [130.0, 150.0][i];
            button(name, ID_WORDLIST + i, P_WORDS, (x, 62.0, w, 30.0));
            x += w + 8.0;
        }
        let edit = (ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32 | WS_VSCROLL.0 | WS_BORDER.0;
        child(hwnd, w!("EDIT"), "", edit, ID_WORDS, P_WORDS, (X0, 158.0, CW, 222.0));
        button("Открыть папку списков", ID_FOLDER, P_WORDS, (X0, 392.0, 200.0, 30.0));

        button("Скачать и установить", ID_LM_GET, P_MODELS, (X0, 192.0, 220.0, 30.0));
        button("Удалить", ID_LM_DEL, P_MODELS, (X0 + 228.0, 192.0, 110.0, 30.0));
        button("Учитывать текст вокруг слова", ID_LM_USE, P_MODELS, (X0, 246.0, CW, 26.0));
        button("Откуда модель и её лицензия", ID_LM_SRC, P_MODELS, (X0, 386.0, 250.0, 30.0));

        button("Сбросить счётчики", ID_RESET, P_STATS, (X0, 386.0, 190.0, 30.0));

        button("Проверить обновления", ID_CHECK, P_ABOUT, (X0, 196.0, 200.0, 30.0));
        button("Страница на GitHub", ID_SITE, P_ABOUT, (X0 + 208.0, 196.0, 180.0, 30.0));
    }
}

/// `--settings [страница] [quiet] [dark|light]`: окно настроек; quiet и тема — для снимков.
pub fn run(page: usize, quiet: bool, look: Option<u32>) {
    unsafe {
        let class = w!("AntiPunto_settings");
        if let Ok(h) = FindWindowW(class, None)
            && !h.is_invalid()
        {
            let _ = ShowWindow(h, SW_RESTORE);
            let _ = SetForegroundWindow(h);
            return;
        }
        settings::load();
        if let Some(v) = look {
            settings::THEME.store(v, SeqCst);
        }
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: LoadIconW(Some(hinst.into()), PCWSTR(std::ptr::without_provenance(1))).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);

        let dpi = GetDpiForSystem();
        let k = dpi as f32 / 96.0;
        let make = |weight: FONT_WEIGHT, size: f32| {
            CreateFontW(
                -(size * k).round() as i32,
                0,
                0,
                0,
                weight.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                w!("Segoe UI"),
            )
        };
        st_mut(|s| {
            s.scale = k;
            s.page = page.min(PAGES.len() - 1);
            s.font = make(FW_NORMAL, 15.0);
            s.bold = make(FW_SEMIBOLD, 15.0);
            s.title = make(FW_SEMIBOLD, 22.0);
            s.small = make(FW_NORMAL, 13.0);
        });

        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
        let mut rc = RECT { left: 0, top: 0, right: px(W), bottom: px(H) };
        let _ = AdjustWindowRectExForDpi(&mut rc, style, false, WS_EX_CONTROLPARENT, dpi);
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST), &mut mi);
        let work = mi.rcWork;
        let (x, y) = ((work.left + work.right - w) / 2, (work.top + work.bottom - h) / 2);
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            class,
            w!("AntiPunto — настройки"),
            style,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(hinst.into()),
            None,
        ) else {
            return;
        };
        build(hwnd);
        apply_theme(hwnd);
        set_page(hwnd, st(|s| s.page));
        let _ = ShowWindow(hwnd, if quiet { SW_SHOWNOACTIVATE } else { SW_SHOW });
        if !quiet {
            let _ = SetFocus(Some(item(hwnd, ID_NAV + st(|s| s.page))));
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn num() {
        assert_eq!(super::num(0), "0");
        assert_eq!(super::num(999), "999");
        assert_eq!(super::num(12345), "12\u{a0}345");
        assert_eq!(super::num(1234567), "1\u{a0}234\u{a0}567");
    }
}
