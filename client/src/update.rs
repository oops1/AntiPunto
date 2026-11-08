//! Проверка новых версий на GitHub и установка обновления.

use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering::SeqCst};

use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

pub const REPO: &str = "oops1/AntiPunto";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
pub struct Release {
    pub version: String,
    pub page: String,
    setup: Option<String>,
    sums: Option<String>,
}

/// Найденное обновление, ждёт решения пользователя.
pub static AVAILABLE: Mutex<Option<Release>> = Mutex::new(None);

struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn open(h: *mut c_void, what: &str) -> Result<Handle, String> {
    if h.is_null() { Err(format!("{what}: {}", windows::core::Error::from_thread())) } else { Ok(Handle(h)) }
}

/// GET по https; редиректы WinHTTP проходит сам.
fn get(url: &str) -> Result<Vec<u8>, String> {
    get_with(url, &mut |_| true)
}

/// GET с отчётом о ходе: on получает число принятых байт; false — прервать.
pub fn get_with(url: &str, on: &mut dyn FnMut(usize) -> bool) -> Result<Vec<u8>, String> {
    let rest = url.strip_prefix("https://").ok_or("только https")?;
    let (host, path) = match rest.split_once('/') {
        Some((h, p)) => (h, format!("/{p}")),
        None => (rest, "/".to_string()),
    };
    unsafe {
        let agent = HSTRING::from(format!("AntiPunto/{VERSION}"));
        let s =
            open(WinHttpOpen(&agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0), "open")?;
        // застывшее соединение обрывается само: имя, связь, отправка, приём — мс
        let _ = WinHttpSetTimeouts(s.0, 10_000, 15_000, 15_000, 20_000);
        let c = open(WinHttpConnect(s.0, &HSTRING::from(host), 443, 0), "connect")?;
        let r = open(
            WinHttpOpenRequest(
                c.0,
                w!("GET"),
                &HSTRING::from(path),
                PCWSTR::null(),
                PCWSTR::null(),
                null(),
                WINHTTP_FLAG_SECURE,
            ),
            "request",
        )?;
        let hdr: Vec<u16> = "Accept: application/vnd.github+json\r\n".encode_utf16().collect();
        WinHttpSendRequest(r.0, Some(&hdr), None, 0, 0, 0).map_err(|e| e.to_string())?;
        WinHttpReceiveResponse(r.0, null_mut()).map_err(|e| e.to_string())?;
        let mut code = 0u32;
        let mut len = 4u32;
        WinHttpQueryHeaders(
            r.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut code as *mut u32).cast()),
            &mut len,
            null_mut(),
        )
        .map_err(|e| e.to_string())?;
        if code != 200 {
            return Err(format!("HTTP {code}"));
        }
        let mut out = Vec::new();
        loop {
            let mut avail = 0u32;
            WinHttpQueryDataAvailable(r.0, &mut avail).map_err(|e| e.to_string())?;
            if avail == 0 {
                break;
            }
            let start = out.len();
            out.resize(start + avail as usize, 0);
            let mut read = 0u32;
            WinHttpReadData(r.0, out[start..].as_mut_ptr().cast(), avail, &mut read).map_err(|e| e.to_string())?;
            out.truncate(start + read as usize);
            if !on(out.len()) {
                return Err("отменено".into());
            }
            if out.len() > 300 << 20 {
                return Err("слишком большой ответ".into());
            }
        }
        Ok(out)
    }
}

/// Все строковые значения ключа в JSON (без полноценного разбора).
fn json_strings<'a>(s: &'a str, key: &str) -> Vec<&'a str> {
    let pat = format!("\"{key}\"");
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find(&pat) {
        rest = &rest[i + pat.len()..];
        let t = rest.trim_start();
        let Some(t) = t.strip_prefix(':') else { continue };
        let Some(t) = t.trim_start().strip_prefix('"') else { continue };
        if let Some(end) = t.find('"') {
            out.push(&t[..end]);
        }
    }
    out
}

fn parse_version(v: &str) -> Vec<u32> {
    v.trim_start_matches('v').split(['.', '-']).map_while(|p| p.parse().ok()).collect()
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    parse_version(candidate) > parse_version(current)
}

/// Последний релиз, если он новее текущей версии.
pub fn check() -> Result<Option<Release>, String> {
    let body = get(&format!("https://api.github.com/repos/{REPO}/releases/latest"))?;
    let s = String::from_utf8_lossy(&body);
    let tag = json_strings(&s, "tag_name").first().map(|t| t.to_string()).ok_or("нет tag_name")?;
    let assets = json_strings(&s, "browser_download_url");
    let rel = Release {
        version: tag.trim_start_matches('v').to_string(),
        page: json_strings(&s, "html_url")
            .first()
            .map_or_else(|| format!("https://github.com/{REPO}/releases"), |u| u.to_string()),
        setup: assets.iter().find(|u| u.ends_with("-setup.exe")).map(|u| u.to_string()),
        sums: assets.iter().find(|u| u.ends_with("SHA256SUMS.txt")).map(|u| u.to_string()),
    };
    Ok(is_newer(&rel.version, VERSION).then_some(rel))
}

pub fn sha256_hex(data: &[u8]) -> Result<String, String> {
    let mut out = [0u8; 32];
    let st = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out) };
    if st.0 != 0 {
        return Err(format!("BCryptHash {:#x}", st.0));
    }
    Ok(out.iter().map(|b| format!("{b:02x}")).collect())
}

fn open_page(rel: &Release) {
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(&rel.page), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Мьютекс единственного экземпляра; это же имя проверяет установщик (AppMutex).
static SINGLE: AtomicIsize = AtomicIsize::new(0);

/// Занять место единственного экземпляра; false — программа уже запущена.
pub fn hold_single() -> bool {
    unsafe {
        let Ok(h) = CreateMutexW(None, false, w!("AntiPunto_single")) else { return false };
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(h);
            return false;
        }
        SINGLE.store(h.0 as isize, SeqCst);
        true
    }
}

/// Отпустить мьютекс: установщик, увидев его, в тихом режиме просто выходит.
pub fn release_single() {
    let h = SINGLE.swap(0, SeqCst);
    if h != 0 {
        unsafe {
            let _ = CloseHandle(HANDLE(h as *mut _));
        }
    }
}

/// Скачать установщик, сверить SHA-256 и запустить его.
pub fn install(rel: &Release) -> Result<(), String> {
    let (Some(setup_url), Some(sums_url)) = (rel.setup.as_deref(), rel.sums.as_deref()) else {
        open_page(rel);
        return Err("в релизе нет установщика или SHA256SUMS.txt — открыта страница релиза".into());
    };
    let name = setup_url.rsplit('/').next().unwrap_or("AntiPunto-setup.exe");
    let sums = String::from_utf8_lossy(&get(sums_url)?).to_string();
    let want =
        sums.lines()
            .find_map(|l| {
                l.split_whitespace().collect::<Vec<_>>().split_first().and_then(|(h, rest)| {
                    rest.iter().any(|f| f.trim_start_matches('*') == name).then(|| h.to_lowercase())
                })
            })
            .ok_or("установщика нет в SHA256SUMS.txt")?;
    let data = get(setup_url)?;
    let got = sha256_hex(&data)?;
    if got != want {
        return Err(format!("контрольная сумма не совпала: {got}"));
    }
    let path = std::env::temp_dir().join(name);
    std::fs::write(&path, &data).map_err(|e| e.to_string())?;
    // с загруженной моделью программа завершается не мгновенно — мьютекс отпустить заранее
    release_single();
    let h = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(path.as_os_str()),
            w!("/SILENT /SUPPRESSMSGBOXES /NORESTART"),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if (h.0 as isize) <= 32 {
        hold_single();
        return Err(format!("не удалось запустить установщик ({})", h.0 as isize));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_instance_mutex_is_released_for_the_installer() {
        if !hold_single() {
            eprintln!("AntiPunto запущена — мьютекс занят, проверка пропущена");
            return;
        }
        assert!(!hold_single(), "второй захват в том же процессе должен быть отказом");
        release_single();
        assert!(hold_single());
        release_single();
    }

    #[test]
    fn versions() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.3"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
    }

    #[test]
    fn json() {
        let s = r#"{"html_url": "https://x/rel", "tag_name":"v1.2.3", "assets":[{"browser_download_url":"https://x/AntiPunto-1.2.3-setup.exe"}]}"#;
        assert_eq!(json_strings(s, "tag_name"), ["v1.2.3"]);
        assert_eq!(json_strings(s, "browser_download_url"), ["https://x/AntiPunto-1.2.3-setup.exe"]);
    }

    #[test]
    fn sha() {
        assert_eq!(sha256_hex(b"abc").unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
