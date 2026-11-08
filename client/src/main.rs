#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod bpe;
mod engine;
mod focus;
mod hook;
mod input;
mod keymap;
mod lists;
mod lm;
mod lmstore;
mod memory;
mod ov;
mod popup;
mod prefs;
mod settings;
mod spell;
mod stats;
mod theme;
mod tray;
mod update;
mod worker;

use std::path::PathBuf;
use std::sync::mpsc::channel;

use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CAPITAL};
use windows::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL};

use engine::{Buffer, Ch};
use keymap::{EN, KEYMAP, OTHER, RU, SPACE};

#[derive(Clone)]
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
    pub spell_dict_threshold: f32,
    /// Папка необязательной языковой модели.
    pub lm_dir: PathBuf,
    pub lm: bool,
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
        spell_threshold: 0.70,
        spell_dict_threshold: 0.90,
        lm_dir: lmstore::dir(),
        lm: true,
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
            "--no-lm" => c.lm = false,
            "--lm-dir" => c.lm_dir = args.next().unwrap_or_default().into(),
            "--spell-threshold" => {
                c.spell_threshold = args.next().and_then(|v| v.parse().ok()).unwrap_or(c.spell_threshold)
            }
            "--spell-dict-threshold" => {
                c.spell_dict_threshold = args.next().and_then(|v| v.parse().ok()).unwrap_or(c.spell_dict_threshold)
            }
            "--spelltest" => selftest = args.next().map(|t| format!("\u{2}{t}")),
            "--ranktest" => selftest = args.next().map(|t| format!("\u{3}{t}")),
            "--popup-demo" => {
                settings::load();
                match args.next().as_deref() {
                    Some("dark") => settings::THEME.store(theme::DARK, std::sync::atomic::Ordering::SeqCst),
                    Some("light") => settings::THEME.store(theme::LIGHT, std::sync::atomic::Ordering::SeqCst),
                    _ => {}
                }
                popup::demo();
                std::process::exit(0);
            }
            "--settings" => {
                let rest: Vec<String> = args.collect();
                let page = rest.iter().find_map(|a| a.parse().ok()).unwrap_or(0);
                let theme = rest.iter().find_map(|a| match a.as_str() {
                    "dark" => Some(theme::DARK),
                    "light" => Some(theme::LIGHT),
                    _ => None,
                });
                prefs::run(page, rest.iter().any(|a| a == "quiet"), theme);
                std::process::exit(0);
            }
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
    let ru = spell::Dict::load(&dir.join("spell_ru.tsv"), true)?;
    let en = spell::Dict::load(&dir.join("spell_en.tsv"), false)?;
    let known = spell::Known::load(&dir.join("spell_known.bloom")).ok();
    Ok(spell::Speller::new(ru, en, known, model, c.spell_threshold, c.spell_dict_threshold))
}

/// Языковая модель из папки установки; файлы сверяются с перечнем, вшитым в программу.
pub fn load_lm(c: &Config) -> Result<lm::Lm, String> {
    let _ = std::fs::create_dir_all(&c.cache_dir);
    let devs: Vec<&str> = c.devices.iter().map(String::as_str).collect();
    lm::Lm::load(&c.ov_dir, &c.lm_dir, &devs, &c.cache_dir)
}

/// Для проверок из командной строки: языковая модель сразу, без фоновой загрузки.
fn attach_lm(c: &Config, sp: &mut spell::Speller) {
    if !c.lm || !lmstore::installed(&c.lm_dir) {
        return;
    }
    let t = std::time::Instant::now();
    // как в резиденте: загрузка в другом потоке
    let cfg = c.clone();
    let loaded = std::thread::spawn(move || load_lm(&cfg)).join().unwrap_or_else(|_| Err("загрузка прервана".into()));
    match loaded {
        Ok(lm) => {
            eprintln!("языковая модель на {}, загрузка {:?}", lm.device, t.elapsed());
            sp.lm = Some(lm);
        }
        Err(e) => eprintln!("языковая модель не загружена: {e}"),
    }
}

/// Решения клиента для файла «слово⇥слева⇥справа»: правка, стадия (1 — на границе слова, 2 — после следующего)
/// и уверенность. Для сверки с расчётом (train/lm_client_eval.py).
fn ranktest(c: &Config, path: &str) -> Result<(), String> {
    let mut sp = load_speller(c)?;
    attach_lm(c, &mut sp);
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let t0 = std::time::Instant::now();
    let (mut n, mut mixed) = (0u32, 0u32);
    for line in text.lines() {
        let mut f = line.split('\t');
        let (word, ctx, right) = (f.next().unwrap_or(""), f.next().unwrap_or(""), f.next().unwrap_or(""));
        let ru = word.chars().any(|ch| ('а'..='я').contains(&ch) || ch == 'ё');
        let (mut out, mut stage, mut conf) = ("-".to_string(), 0, 0f32);
        if let Some(r) = sp.rank(ctx, word, "", ru)? {
            mixed += r.lm as u32;
            let (w, p) = &r.items[0];
            if w != word && *p >= sp.threshold_for(r.in_dict, r.lm) {
                (out, stage, conf) = (w.clone(), 1, *p);
            } else if sp.lm.is_some()
                && ru
                && !right.trim().is_empty()
                && let Some(r) = sp.rank(ctx, word, right, ru)?.filter(|r| r.lm)
            {
                let (w, p) = &r.items[0];
                if w != word && *p >= sp.threshold_for(r.in_dict, true) {
                    (out, stage, conf) = (w.clone(), 2, *p);
                }
            }
        }
        n += 1;
        println!("{word}\t{out}\t{stage}\t{conf:.4}");
    }
    eprintln!("слов {n}, с языковой моделью {mixed}, в среднем {:?} на слово", t0.elapsed() / n.max(1));
    Ok(())
}

/// Орфография по словам фразы, как в клиенте: слово на своей границе, затем ещё раз после следующего.
fn spelltest(c: &Config, text: &str) -> Result<(), String> {
    let t0 = std::time::Instant::now();
    let mut sp = load_speller(c)?;
    println!("загрузка {:?}", t0.elapsed());
    attach_lm(c, &mut sp);
    let mut out: Vec<char> = Vec::new();
    // слово, оставленное на своей границе: начало в out, само слово, русское ли
    let mut prev: Option<(usize, String, bool)> = None;
    for token in text.split(' ').filter(|w| !w.is_empty()) {
        let chars: Vec<char> = token.chars().collect();
        let mut shown = chars.clone();
        let mut cur = None;
        if let Some((lead, trail, ru)) = engine::spell_core(&chars) {
            let word: String = chars[lead..chars.len() - trail].iter().collect();
            let ctx: String = out.iter().chain(&chars[..lead]).collect();
            let t = std::time::Instant::now();
            let ranked = sp.rank(&ctx, &word, "", ru)?;
            let thr = ranked.as_ref().map_or(f32::INFINITY, |r| sp.threshold_for(r.in_dict, r.lm));
            if let Some(r) = &ranked {
                let alt: Vec<String> = r.items.iter().take(4).map(|(w, p)| format!("{w}={p:.2}")).collect();
                let lm = if r.lm { " с языковой моделью" } else { "" };
                println!("  варианты{}{lm}: {}", if r.in_dict { " (словарное)" } else { "" }, alt.join(" "));
            }
            match ranked.as_ref().map(|r| &r.items[0]) {
                Some((w, p)) if *w == word => {
                    println!("  «{word}» оставить {p:.2}");
                    cur = Some((out.len() + lead, word.clone(), ru));
                }
                Some((w, p)) if *p >= thr => {
                    println!("  «{word}» → «{w}» {p:.2} ({:?})", t.elapsed());
                    shown = chars[..lead]
                        .iter()
                        .copied()
                        .chain(w.chars())
                        .chain(chars[chars.len() - trail..].iter().copied())
                        .collect();
                }
                Some((w, p)) => {
                    println!("  «{word}» → «{w}»? {p:.2} — ниже порога");
                    cur = Some((out.len() + lead, word.clone(), ru));
                }
                None => {}
            }
        }
        out.extend(shown);
        out.push(' ');
        // предыдущее слово ещё раз, с текстом справа: только с языковой моделью
        if let Some((start, word, ru)) = prev.take().filter(|p| p.2 && sp.lm.is_some()) {
            let n = word.chars().count();
            let ctx: String = out[..start].iter().collect();
            let right: String = out[start + n..].iter().collect();
            if let Some(r) = sp.rank(&ctx, &word, &right, ru)?.filter(|r| r.lm) {
                let (w, p) = &r.items[0];
                let after = format!("по следующему слову «{}»", right.trim());
                if *w == word {
                    println!("  «{word}» оставить {p:.2} {after}");
                } else if *p >= sp.threshold_for(r.in_dict, r.lm) {
                    println!("  «{word}» → «{w}» {p:.2} {after}");
                    out.splice(start..start + n, w.chars());
                    if let Some(c) = cur.as_mut()
                        && c.0 > start
                    {
                        c.0 = (c.0 as isize + w.chars().count() as isize - n as isize) as usize;
                    }
                } else {
                    println!("  «{word}» → «{w}»? {p:.2} {after} — ниже порога");
                }
            }
        }
        prev = cur;
    }
    let out: String = out.into_iter().collect();
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
    if let Some(path) = st.as_deref().and_then(|t| t.strip_prefix('\u{3}')) {
        if let Err(e) = ranktest(&cfg, path) {
            eprintln!("ошибка: {e}");
            std::process::exit(1);
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

    if !update::hold_single() {
        return;
    }
    unsafe {
        settings::load();
        if !cfg.spell {
            // --no-spell: только на этот запуск
            settings::SPELL.store(false, std::sync::atomic::Ordering::SeqCst);
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
