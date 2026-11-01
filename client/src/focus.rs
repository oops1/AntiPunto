//! Элемент с фокусом ввода через UI Automation: поле пароля?

use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::UI::Accessibility::{CUIAutomation8, IUIAutomation, IUIAutomation2};
use windows::core::Interface;

pub struct Focus {
    uia: Option<IUIAutomation>,
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
}
