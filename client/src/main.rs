#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod engine;
mod focus;
mod hook;
mod input;
mod keymap;
mod lists;
mod ov;
mod spell;
mod stats;
mod tray;
mod update;
mod worker;

use std::path::PathBuf;
use std::sync::mpsc::channel;

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CAPITAL};
use windows::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL};
use windows::core::w;

use engine::{Buffer, Ch};
use keymap::{EN, KEYMAP, OTHER, RU, SPACE};

pub struct Config {
    pub ov_dir: PathBuf,
    pub model: PathBuf,
    pub devices: Vec<String>,
    pub threshold: f32,
    pub cache_dir: PathBuf,
    pub exclude: Vec<String>,
    pub debug: bool,
    pub update_check: bool,
    pub expect: Option<String>,
    pub spell: bool,
    pub spell_threshold: f32,
}

fn config() -> (Config, Option<String>) {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default();
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| exe_dir.clone());
    let mut c = Config {
        ov_dir: exe_dir.join("openvino"),
        model: exe_dir.join("models").join("layout.xml"),
        devices: vec!["NPU".into(), "CPU".into()],
        threshold: 0.6,
        cache_dir: local.join("AntiPunto").join("ovcache"),
        exclude: ["mstsc.exe", "vmconnect.exe", "keepass.exe", "keepassxc.exe", "1password.exe", "bitwarden.exe"]
            .map(String::from)
            .to_vec(),
        debug: false,
        update_check: true,
        expect: None,
        spell: true,
        spell_threshold: 0.85,
    };
    let mut selftest = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--ov" => c.ov_dir = args.next().unwrap_or_default().into(),
            "--model" => c.model = args.next().unwrap_or_default().into(),
            "--device" => c.devices = args.next().unwrap_or_default().split(',').map(String::from).collect(),
            "--threshold" => c.threshold = args.next().and_then(|v| v.parse().ok()).unwrap_or(c.threshold),
            "--exclude" => c.exclude.extend(args.next().unwrap_or_default().split(',').map(|s| s.to_lowercase())),
            "--debug" => c.debug = true,
            "--no-update-check" => c.update_check = false,
            "--expect" => c.expect = args.next(),
            "--no-spell" => c.spell = false,
            "--spell-threshold" => {
                c.spell_threshold = args.next().and_then(|v| v.parse().ok()).unwrap_or(c.spell_threshold)
            }
            "--spelltest" => selftest = args.next().map(|t| format!("\u{2}{t}")),
            "--version" => {
                println!("AntiPunto {}", update::VERSION);
                std::process::exit(0);
            }
            "--selftest" => selftest = args.next(),
            "--probe" => selftest = args.next().map(|t| format!("\u{1}{t}")),
            _ => {}
        }
    }
    (c, selftest)
}

pub fn load_model(c: &Config) -> Result<ov::Model, String> {
    let _ = std::fs::create_dir_all(&c.cache_dir);
    let devs: Vec<&str> = c.devices.iter().map(String::as_str).collect();
    ov::Model::load(&c.ov_dir, &c.model, &devs, &c.cache_dir)
}

/// Орфография: словари и ранжировщик рядом с моделью раскладки.
pub fn load_speller(c: &Config) -> Result<spell::Speller, String> {
    let dir = c.model.parent().map(PathBuf::from).unwrap_or_default();
    let devs: Vec<&str> = c.devices.iter().map(String::as_str).collect();
    let model = ov::Model::load_shaped(
        &c.ov_dir,
        &dir.join("spell.xml"),
        &devs,
        &c.cache_dir,
        &[spell::K * spell::L],
        spell::K,
    )?;
    let ru = spell::Dict::load(&dir.join("spell_ru.tsv"))?;
    let en = spell::Dict::load(&dir.join("spell_en.tsv"))?;
    let known = spell::Known::load(&dir.join("spell_known.bloom")).ok();
    Ok(spell::Speller::new(ru, en, known, model, c.spell_threshold))
}

/// Орфография по словам фразы, как будто каждое слово только что набрано.
fn spelltest(c: &Config, text: &str) -> Result<(), String> {
    let t0 = std::time::Instant::now();
    let mut sp = load_speller(c)?;
    println!("загрузка {:?}", t0.elapsed());
    let mut out = String::new();
    let mut ctx = String::new();
    for token in text.split(' ').filter(|w| !w.is_empty()) {
        let chars: Vec<char> = token.chars().collect();
        let Some((lead, trail, ru)) = engine::spell_core(&chars) else {
            out += token;
            out.push(' ');
            ctx = out.clone();
            continue;
        };
        let word: String = chars[lead..chars.len() - trail].iter().collect();
        let t = std::time::Instant::now();
        let fix = sp.suggest(&format!("{ctx}{}", String::from_iter(&chars[..lead])), &word, ru)?;
        let shown = match &fix {
            Some((w, p)) if *w == word => {
                println!("  «{word}» оставить {p:.2}");
                word
            }
            Some((w, p)) if *p >= sp.threshold => {
                println!("  «{word}» → «{w}» {p:.2} ({:?})", t.elapsed());
                w.clone()
            }
            Some((w, p)) => {
                println!("  «{word}» → «{w}»? {p:.2} — ниже порога");
                word
            }
            None => word,
        };
        out.extend(&chars[..lead]);
        out += &shown;
        out.extend(&chars[chars.len() - trail..]);
        out.push(' ');
        ctx = out.clone();
    }
    println!("итог: {out}");
    match &c.expect {
        Some(e) if e.trim_end() != out.trim_end() => Err(format!("ожидалось: {e}")),
        _ => Ok(()),
    }
}

/// Набор текста целиком в английской раскладке, как будто забыли переключить.
fn selftest(c: &Config, text: &str) -> Result<(), String> {
    let t0 = std::time::Instant::now();
    let mut model = load_model(c)?;
    println!("устройство {}, загрузка {:?}", model.device, t0.elapsed());
    // «=текст»: каждый символ в раскладке своего алфавита, как на экране
    let (as_screen, text) = match text.strip_prefix('=') {
        Some(t) => (true, t),
        None => (false, text),
    };
    let lay_of = |ch: char| if as_screen && !ch.is_ascii() { RU } else { EN };
    let tok_of = |ch: char| {
        if ch == ' ' {
            SPACE
        } else if as_screen {
            KEYMAP.token_of(ch, lay_of(ch)).unwrap_or(OTHER)
        } else {
            KEYMAP.token_of(ch, RU).or_else(|| KEYMAP.token_of(ch, EN)).unwrap_or(OTHER)
        }
    };
    let mut buf = Buffer::default();
    let mut screen = String::new();
    let mut runs = Vec::new();
    for ch in text.chars() {
        let c2 = Ch::new(tok_of(ch), lay_of(ch), ch);
        screen.push(c2.render());
        buf.push(c2);
        if buf.word_just_ended() {
            if c.debug {
                let w = buf.probe(&mut model)?;
                println!("    {}", w.iter().map(|(w, p)| format!("{w}={p:.2}")).collect::<Vec<_>>().join(" "));
            }
            let t = std::time::Instant::now();
            let plan = buf.plan(&mut model, c.threshold, &lists::Lists::default())?;
            runs.push(t.elapsed());
            if let Some(p) = plan {
                let f = buf.apply(p);
                let keep = screen.chars().count() - f.erase;
                screen = screen.chars().take(keep).collect::<String>() + &f.text;
                println!("  «{}» → «{}»", f.old.trim_end(), f.text.trim_end());
            }
        }
    }
    let typed: String =
        text.chars().map(|ch| if tok_of(ch) == OTHER { ch } else { KEYMAP.char_of(tok_of(ch), lay_of(ch)) }).collect();
    println!("набрано: {typed}\nэкран:   {screen}");
    let mean = runs.iter().sum::<std::time::Duration>() / runs.len().max(1) as u32;
    println!("прогонов {}, в среднем {:?}", runs.len(), mean);
    match &c.expect {
        Some(e) if e.trim_end() != screen.trim_end() => Err(format!("ожидалось: {e}")),
        _ => Ok(()),
    }
}

/// Текст как на экране: каждый символ в раскладке своего алфавита.
fn probe(c: &Config, text: &str) -> Result<(), String> {
    let mut model = load_model(c)?;
    let mut buf = Buffer::default();
    for ch in text.chars() {
        let lay = if ch.is_ascii() { EN } else { RU };
        let tok = if ch == ' ' { SPACE } else { KEYMAP.token_of(ch, lay).unwrap_or(OTHER) };
        buf.push(Ch::new(tok, lay, ch));
    }
    let words = buf.probe(&mut model)?;
    println!("{}", words.iter().map(|(w, p)| format!("{w}={p:.3}")).collect::<Vec<_>>().join("  "));
    Ok(())
}

fn main() {
    let (cfg, st) = config();
    if let Some(text) = st.as_deref().and_then(|t| t.strip_prefix('\u{1}')) {
        if let Err(e) = probe(&cfg, text) {
            eprintln!("ошибка: {e}");
        }
        return;
    }
    if let Some(text) = st.as_deref().and_then(|t| t.strip_prefix('\u{2}')) {
        if let Err(e) = spelltest(&cfg, text) {
            eprintln!("ошибка: {e}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(text) = st {
        if let Err(e) = selftest(&cfg, &text) {
            eprintln!("ошибка: {e}");
            std::process::exit(1);
        }
        return;
    }

    unsafe {
        // это же имя ждёт установщик (AppMutex)
        let _mutex = CreateMutexW(None, false, w!("AntiPunto_single"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        tray::set_status("загрузка модели…".into());
        let (tx, rx) = channel();
        let _ = hook::SENDER.set(tx);
        let caps = GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0;
        let update_check = cfg.update_check;
        std::thread::spawn(move || worker::run(cfg, rx, caps));

        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let kb = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook::kb_proc), Some(hinst.into()), 0);
        let ms = SetWindowsHookExW(WH_MOUSE_LL, Some(hook::mouse_proc), Some(hinst.into()), 0);
        tray::run(update_check);
        if let Ok(h) = kb {
            let _ = UnhookWindowsHookEx(h);
        }
        if let Ok(h) = ms {
            let _ = UnhookWindowsHookEx(h);
        }
    }
}
