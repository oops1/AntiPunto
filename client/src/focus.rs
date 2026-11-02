//! Элемент с фокусом ввода через UI Automation: поле пароля, место курсора.

use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, SAFEARRAY,
};
use windows::Win32::System::Ole::{SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetUBound, SafeArrayUnaccessData};
use windows::Win32::System::Variant::{VARIANT, VT_I4};
use windows::Win32::UI::Accessibility::{
    AccessibleObjectFromWindow, CUIAutomation8, IAccessible, IUIAutomation, IUIAutomation2, IUIAutomationTextPattern2,
    TextPatternRangeEndpoint_Start, TextUnit_Character, UIA_TextPattern2Id,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GUITHREADINFO, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, OBJID_CARET,
};
use windows::core::{BOOL, Interface};

pub struct Focus {
    uia: Option<IUIAutomation>,
}

/// Курсор на экране и способ, которым его нашли.
pub struct Caret {
    pub rect: RECT,
    pub how: &'static str,
}

/// Прямоугольники [left, top, width, height]… из SAFEARRAY double.
unsafe fn rects(sa: *mut SAFEARRAY) -> Vec<RECT> {
    let mut out = Vec::new();
    if sa.is_null() {
        return out;
    }
    unsafe {
        let n = SafeArrayGetUBound(sa, 1).map_or(0, |u| u + 1) as usize;
        let mut p = std::ptr::null_mut();
        if n >= 4 && SafeArrayAccessData(sa, &mut p).is_ok() {
            let v = std::slice::from_raw_parts(p as *const f64, n);
            for r in v.as_chunks::<4>().0 {
                let (l, t) = (r[0] as i32, r[1] as i32);
                out.push(RECT { left: l, top: t, right: l + r[2] as i32, bottom: t + r[3] as i32 });
            }
            let _ = SafeArrayUnaccessData(sa);
        }
        let _ = SafeArrayDestroy(sa);
    }
    out
}

impl Focus {
    /// Создавать в том потоке, где будут вызовы.
    pub fn new() -> Focus {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let uia: Option<IUIAutomation> = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER).ok();
            // зависшая программа не должна держать нас дольше
            if let Some(u2) = uia.as_ref().and_then(|u| u.cast::<IUIAutomation2>().ok()) {
                let _ = u2.SetConnectionTimeout(300);
                let _ = u2.SetTransactionTimeout(300);
            }
            Focus { uia }
        }
    }

    pub fn is_password(&self) -> bool {
        let Some(u) = &self.uia else { return false };
        unsafe { u.GetFocusedElement().and_then(|e| e.CurrentIsPassword()).is_ok_and(|b| b.as_bool()) }
    }

    /// Где курсор ввода: системная каретка, MSAA, UIA, иначе низ поля ввода.
    pub fn caret(&self) -> Option<Caret> {
        unsafe {
            let tid = GetWindowThreadProcessId(GetForegroundWindow(), None);
            let mut gti = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
            if GetGUIThreadInfo(tid, &mut gti).is_ok() && !gti.hwndCaret.is_invalid() {
                let r = gti.rcCaret;
                if r.bottom > r.top {
                    let mut a = POINT { x: r.left, y: r.top };
                    let mut b = POINT { x: r.right, y: r.bottom };
                    if ClientToScreen(gti.hwndCaret, &mut a).as_bool()
                        && ClientToScreen(gti.hwndCaret, &mut b).as_bool()
                    {
                        let rect = RECT { left: a.x, top: a.y, right: b.x, bottom: b.y };
                        return Some(Caret { rect, how: "каретка" });
                    }
                }
            }
            // Chromium и Electron: каретка через MSAA
            let target = if gti.hwndFocus.is_invalid() { GetForegroundWindow() } else { gti.hwndFocus };
            let mut p = std::ptr::null_mut();
            if AccessibleObjectFromWindow(target, OBJID_CARET.0 as u32, &IAccessible::IID, &mut p).is_ok()
                && !p.is_null()
            {
                let acc = IAccessible::from_raw(p);
                let mut me = VARIANT::default();
                (*me.Anonymous.Anonymous).vt = VT_I4;
                let (mut l, mut t, mut w, mut h) = (0, 0, 0, 0);
                if acc.accLocation(&mut l, &mut t, &mut w, &mut h, &me).is_ok() && h > 0 && (l, t) != (0, 0) {
                    let rect = RECT { left: l, top: t, right: l + w.max(1), bottom: t + h };
                    return Some(Caret { rect, how: "MSAA" });
                }
            }
            let el = self.uia.as_ref()?.GetFocusedElement().ok()?;
            if let Ok(tp) = el.GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id) {
                let mut active = BOOL(0);
                if let Ok(range) = tp.GetCaretRange(&mut active) {
                    let mut found = range.GetBoundingRectangles().map(|sa| rects(sa)).unwrap_or_default();
                    if found.is_empty() {
                        // пустой диапазон часто без рамки: берём символ слева
                        let _ = range.MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Character, -1);
                        found = range.GetBoundingRectangles().map(|sa| rects(sa)).unwrap_or_default();
                        if let Some(r) = found.last_mut() {
                            r.left = r.right;
                        }
                    }
                    if let Some(r) = found.last().filter(|r| r.bottom > r.top) {
                        return Some(Caret { rect: RECT { right: r.left + 1, ..*r }, how: "UIA" });
                    }
                }
            }
            let r = el.CurrentBoundingRectangle().ok().filter(|r| r.bottom > r.top && r.right > r.left)?;
            Some(Caret {
                rect: RECT { left: r.left + 8, top: r.bottom - 4, right: r.left + 9, bottom: r.bottom },
                how: "поле",
            })
        }
    }
}
