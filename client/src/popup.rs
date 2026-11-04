//! Варианты сомнительных слов фразы у курсора: окно без фокуса, выбор мышью или Ctrl+цифрой.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering::SeqCst};

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use crate::hook::{Event, SENDER};

/// Вариантов на слово и слов в окне: всего не больше 9 цифр.
pub const PER_ROW: usize = 3;
pub const ROWS: usize = 3;
pub static VISIBLE: AtomicBool = AtomicBool::new(false);
/// Сколько вариантов на экране: лишние цифры не глотаем.
pub static COUNT: AtomicUsize = AtomicUsize::new(0);

const WM_SHOW: u32 = WM_APP + 10;
const WM_HIDE: u32 = WM_APP + 11;
const TIMEOUT_MS: u32 = 120_000;

static HWND_: AtomicIsize = AtomicIsize::new(0);

const ZERO: RECT = RECT { left: 0, top: 0, right: 0, bottom: 0 };
// COLORREF — 0x00BBGGRR
const INK: COLORREF = COLORREF(0x0020_2020);
const GREY: COLORREF = COLORREF(0x0070_7070);
const RED: COLORREF = COLORREF(0x0000_00DC);
const GREEN: COLORREF = COLORREF(0x0028_9600);

/// Строка окна: набранное слово и варианты замены.
pub struct Row {
    pub word: String,
    pub items: Vec<String>,
}

struct State {
    rows: Vec<Row>,
    caret: RECT,
    /// Окно на экране: для мышиного хука.
    rect: RECT,
    /// Ячейки вариантов внутри окна, по сквозному номеру.
    cells: Vec<RECT>,
    /// Подписи слов слева.
    labels: Vec<RECT>,
    font: isize,
    bold: isize,
    /// Мелкий шрифт подписи «Ctrl» у цифры.
    small: isize,
    scale: f32,
}

static STATE: Mutex<State> = Mutex::new(State {
    rows: Vec::new(),
    caret: ZERO,
    rect: ZERO,
    cells: Vec::new(),
    labels: Vec::new(),
    font: 0,
    bold: 0,
    small: 0,
    scale: 1.0,
});
const BLUE: COLORREF = COLORREF(0x00C0_7820);

fn post(msg: u32) {
    let h = HWND_.load(SeqCst);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), msg, WPARAM(0), LPARAM(0));
        }
    }
}

/// Показать строки вариантов под курсором (из любого потока).
pub fn show(rows: Vec<Row>, caret: RECT) {
    let mut s = STATE.lock().unwrap();
    COUNT.store(rows.iter().map(|r| r.items.len()).sum(), SeqCst);
    s.rows = rows;
    s.caret = caret;
    VISIBLE.store(true, SeqCst);
    post(WM_SHOW);
}

pub fn hide() {
    if VISIBLE.swap(false, SeqCst) {
        COUNT.store(0, SeqCst);
        post(WM_HIDE);
    }
}

/// Клик пришёлся в окно вариантов.
pub fn hit(pt: POINT) -> bool {
    if !VISIBLE.load(SeqCst) {
        return false;
    }
    let r = STATE.lock().unwrap().rect;
    pt.x >= r.left && pt.x < r.right && pt.y >= r.top && pt.y < r.bottom
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Какие буквы меняются: (в набранном — убрать/заменить, в варианте — новые).
fn diff(from: &str, to: &str) -> (Vec<bool>, Vec<bool>) {
    let (a, b): (Vec<char>, Vec<char>) = (from.chars().collect(), to.chars().collect());
    let (n, m) = (a.len(), b.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, v) in d[0].iter_mut().enumerate() {
        *v = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let sub = d[i - 1][j - 1] + (a[i - 1] != b[j - 1]) as usize;
            d[i][j] = sub.min(d[i - 1][j] + 1).min(d[i][j - 1] + 1);
        }
    }
    let (mut fa, mut fb) = (vec![false; n], vec![false; m]);
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && d[i][j] == d[i - 1][j - 1] + (a[i - 1] != b[j - 1]) as usize {
            if a[i - 1] != b[j - 1] {
                fa[i - 1] = true;
                fb[j - 1] = true;
            }
            i -= 1;
            j -= 1;
        } else if i > 0 && d[i][j] == d[i - 1][j] + 1 {
            fa[i - 1] = true;
            i -= 1;
        } else {
            fb[j - 1] = true;
            j -= 1;
        }
    }
    (fa, fb)
}

/// Как красить буквы: обычные и отмеченные.
struct Mark {
    plain: COLORREF,
    ink: COLORREF,
}

const MARK_NEW: Mark = Mark { plain: INK, ink: GREEN };
const MARK_BAD: Mark = Mark { plain: GREY, ink: RED };

/// Текст отрезками: отмеченные буквы жирным цветом. r = None — только ширина.
unsafe fn marked(dc: HDC, r: Option<&RECT>, text: &str, marks: &[bool], m: &Mark, fonts: (isize, isize)) -> i32 {
    let chars: Vec<char> = text.chars().collect();
    let mut x = r.map_or(0, |r| r.left);
    let start = x;
    let mut i = 0;
    while i < chars.len() {
        let on = marks.get(i).copied().unwrap_or(false);
        let mut j = i;
        while j < chars.len() && marks.get(j).copied().unwrap_or(false) == on {
            j += 1;
        }
        let run: String = chars[i..j].iter().collect();
        unsafe {
            SelectObject(dc, HGDIOBJ((if on { fonts.1 } else { fonts.0 }) as *mut _));
            let sz = text_size(dc, &run);
            if let Some(r) = r {
                let mut rr = RECT { left: x, ..*r };
                SetTextColor(dc, if on { m.ink } else { m.plain });
                DrawTextW(dc, &mut wide(&run), &mut rr, DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX);
            }
            x += sz.cx;
        }
        i = j;
    }
    unsafe {
        SelectObject(dc, HGDIOBJ(fonts.0 as *mut _));
    }
    x - start
}

/// Буквы набранного, которые меняет хотя бы один вариант.
fn suspicious(r: &Row) -> Vec<bool> {
    let mut bad = vec![false; r.word.chars().count()];
    for item in &r.items {
        for (b, f) in bad.iter_mut().zip(diff(&r.word, item).0) {
            *b |= f;
        }
    }
    bad
}

fn text_size(dc: HDC, s: &str) -> SIZE {
    let mut sz = SIZE::default();
    unsafe {
        let _ = GetTextExtentPoint32W(dc, &wide(s), &mut sz);
    }
    sz
}

/// Клавиша «Ctrl» перед цифрой: размер рамки; r = Some — нарисовать слева в ячейке.
unsafe fn ctrl_key(dc: HDC, small: isize, scale: f32, r: Option<(i32, &RECT)>) -> SIZE {
    unsafe {
        let px = |v: f32| (v * scale).round() as i32;
        let prev = SelectObject(dc, HGDIOBJ(small as *mut _));
        let t = text_size(dc, "Ctrl");
        let key = SIZE { cx: t.cx + px(8.0), cy: t.cy + px(2.0) };
        if let Some((left, c)) = r {
            let top = (c.top + c.bottom - key.cy) / 2;
            let pen = CreatePen(PS_SOLID, 1, BLUE);
            let old_pen = SelectObject(dc, pen.into());
            let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
            let _ = RoundRect(dc, left, top, left + key.cx, top + key.cy, px(3.0), px(3.0));
            SelectObject(dc, old_brush);
            SelectObject(dc, old_pen);
            let _ = DeleteObject(pen.into());
            SetTextColor(dc, BLUE);
            let mut kr = RECT { left, top, right: left + key.cx, bottom: top + key.cy };
            DrawTextW(dc, &mut wide("Ctrl"), &mut kr, DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX);
        }
        SelectObject(dc, prev);
        key
    }
}

/// Разметить ячейки и поставить окно под курсор (или над ним у низа экрана).
unsafe fn layout(hwnd: HWND) {
    unsafe {
        let mut s = STATE.lock().unwrap();
        let c = s.caret;
        let mon = MonitorFromPoint(POINT { x: c.left, y: c.bottom }, MONITOR_DEFAULTTONEAREST);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let k = dx as f32 / 96.0;
        let px = |v: f32| (v * k).round() as i32;
        for f in [s.font, s.bold, s.small] {
            if f != 0 {
                let _ = DeleteObject(HGDIOBJ(f as *mut _));
            }
        }
        let make = |weight: FONT_WEIGHT, size: f32| {
            CreateFontW(
                -px(size),
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
        let font = make(FW_NORMAL, 15.0);
        s.font = font.0 as isize;
        s.bold = make(FW_BOLD, 15.0).0 as isize;
        s.small = make(FW_NORMAL, 10.0).0 as isize;
        s.scale = k;
        let fonts = (s.font, s.bold);
        let dc = GetDC(Some(hwnd));
        let old = SelectObject(dc, font.into());
        let line_h = text_size(dc, "Ag").cy + px(10.0);
        let key_w = ctrl_key(dc, s.small, k, None).cx + px(4.0);
        let label_w = s
            .rows
            .iter()
            .map(|r| marked(dc, None, &format!("{} →", r.word), &suspicious(r), &MARK_BAD, fonts))
            .max()
            .unwrap_or(0)
            + px(20.0);
        let (mut cells, mut labels) = (Vec::new(), Vec::new());
        let mut w = 0;
        let mut n = 0;
        for (row, r) in s.rows.iter().enumerate() {
            let (top, bottom) = (row as i32 * line_h, (row as i32 + 1) * line_h);
            labels.push(RECT { left: 0, top, right: label_w, bottom });
            let mut x = label_w;
            for item in &r.items {
                n += 1;
                let iw = marked(dc, None, item, &diff(&r.word, item).1, &MARK_NEW, fonts);
                let cw = key_w + text_size(dc, &n.to_string()).cx + px(7.0) + iw + px(20.0);
                cells.push(RECT { left: x, top, right: x + cw, bottom });
                x += cw;
            }
            w = w.max(x + px(4.0));
        }
        SelectObject(dc, old);
        ReleaseDC(Some(hwnd), dc);
        let h = line_h * s.rows.len() as i32;
        s.cells = cells;
        s.labels = labels;

        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let work = mi.rcWork;
        let left = (c.left - px(10.0)).clamp(work.left, (work.right - w).max(work.left));
        let mut top = c.bottom + px(6.0);
        if top + h > work.bottom {
            top = c.top - h - px(6.0);
        }
        top = top.max(work.top);
        s.rect = RECT { left, top, right: left + w, bottom: top + h };
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), left, top, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
}

unsafe fn paint(hwnd: HWND) {
    unsafe {
        let s = STATE.lock().unwrap();
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let bg = CreateSolidBrush(COLORREF(0x00FA_FAFA));
        let line = CreateSolidBrush(COLORREF(0x00C8_C8C8));
        let frame = CreateSolidBrush(COLORREF(0x00A0_A0A0));
        FillRect(dc, &rc, bg);
        let old = SelectObject(dc, HGDIOBJ(s.font as *mut _));
        SetBkMode(dc, TRANSPARENT);
        let gap = (7.0 * s.scale).round() as i32;
        let pad = (10.0 * s.scale).round() as i32;
        let mut n = 0;
        let mut cells = s.cells.iter();
        for (row, (r, lab)) in s.rows.iter().zip(&s.labels).enumerate() {
            if row > 0 {
                FillRect(dc, &RECT { left: 0, top: lab.top, right: rc.right, bottom: lab.top + 1 }, line);
            }
            let fonts = (s.font, s.bold);
            let lr = RECT { left: lab.left + pad, ..*lab };
            marked(dc, Some(&lr), &format!("{} →", r.word), &suspicious(r), &MARK_BAD, fonts);
            for item in &r.items {
                n += 1;
                let Some(c) = cells.next() else { break };
                let num = n.to_string();
                let key = ctrl_key(dc, s.small, s.scale, Some((c.left + pad, c)));
                let mut cr = RECT { left: c.left + pad + key.cx + (4.0 * s.scale).round() as i32, ..*c };
                SetTextColor(dc, BLUE);
                DrawTextW(dc, &mut wide(&num), &mut cr, DT_SINGLELINE | DT_VCENTER | DT_LEFT);
                cr.left += text_size(dc, &num).cx + gap;
                marked(dc, Some(&cr), item, &diff(&r.word, item).1, &MARK_NEW, fonts);
            }
        }
        FrameRect(dc, &rc, frame);
        SelectObject(dc, old);
        for b in [bg, line, frame] {
            let _ = DeleteObject(b.into());
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_SHOW => {
                layout(hwnd);
                SetTimer(Some(hwnd), 1, TIMEOUT_MS, None);
                LRESULT(0)
            }
            WM_HIDE | WM_TIMER => {
                VISIBLE.store(false, SeqCst);
                COUNT.store(0, SeqCst);
                let _ = KillTimer(Some(hwnd), 1);
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_LBUTTONDOWN => {
                let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
                let i = STATE
                    .lock()
                    .unwrap()
                    .cells
                    .iter()
                    .position(|c| x >= c.left && x < c.right && y >= c.top && y < c.bottom);
                // окно обновит рабочий поток после замены
                if let (Some(i), Some(tx)) = (i, SENDER.get()) {
                    let _ = tx.send(Event::Pick(i));
                }
                LRESULT(0)
            }
            WM_PAINT => {
                paint(hwnd);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// `--popup-demo`: окно с образцом строк у курсора мыши на 6 с — проверка отрисовки без хука.
pub fn demo() {
    unsafe {
        create(GetModuleHandleW(None).unwrap_or_default().into());
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let row =
            |w: &str, items: &[&str]| Row { word: w.into(), items: items.iter().map(|s| s.to_string()).collect() };
        show(
            vec![row("пливет", &["привет", "плывет"]), row("сесии", &["сессии", "серии", "сесии"])],
            RECT { left: pt.x, top: pt.y, right: pt.x, bottom: pt.y + 20 },
        );
        let end = std::time::Instant::now() + std::time::Duration::from_secs(6);
        let mut msg = MSG::default();
        while std::time::Instant::now() < end {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

/// Создать окно в потоке с циклом сообщений (главном).
pub fn create(hinst: HINSTANCE) {
    unsafe {
        let class = w!("AntiPunto_popup");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_HAND).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        if let Ok(h) = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            class,
            w!(""),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinst),
            None,
        ) {
            HWND_.store(h.0 as isize, SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::diff;

    #[test]
    fn diff_marks_changed_letters() {
        // пливет → привет: «л» заменена на «р»
        let (a, b) = diff("пливет", "привет");
        assert_eq!(a, [false, true, false, false, false, false]);
        assert_eq!(b, [false, true, false, false, false, false]);
        // сесию → сессию: вставлена одна «с»
        let (a, b) = diff("сесию", "сессию");
        assert_eq!(a.iter().filter(|&&x| x).count(), 0);
        assert_eq!(b.iter().filter(|&&x| x).count(), 1);
    }
}
