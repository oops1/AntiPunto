//! Рабочий поток: буфер, модель, исправления.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
use windows::core::PWSTR;

use crate::engine::{Buffer, Ch, Plan};
use crate::focus::Focus;
use crate::hook::{CORRECTING, Event, HAS_WORD, Key, UNDO_READY, is_modifier};
use crate::keymap::{KEYMAP, OTHER, SPACE};
use crate::lists::{self, Lists, Mode};
use crate::lm::Lm;
use crate::memory::Memory;
use crate::ov::Model;
use crate::popup;
use crate::spell::Speller;
use crate::stats::{Saver, Stats};
use crate::tray::{READY, fence, set_status};
use crate::{Config, input, lmstore, load_lm, load_model, load_speller, settings};

#[derive(Default)]
struct Mods {
    shift_l: bool,
    shift_r: bool,
    ctrl_l: bool,
    ctrl_r: bool,
    alt_l: bool,
    alt_r: bool,
    win_l: bool,
    win_r: bool,
}

impl Mods {
    fn shift(&self) -> bool {
        self.shift_l || self.shift_r
    }
    fn command(&self) -> bool {
        self.ctrl_l || self.ctrl_r || self.alt_l || self.alt_r || self.win_l || self.win_r
    }
}

/// Варианты ниже этой уверенности не показываем.
const OFFER_MIN_P: f32 = 0.05;
/// Для словарного слова варианты показываем только заметные.
const OFFER_DICT_MIN_P: f32 = 0.8;

/// Результат орфографии для слова: границы в буфере и варианты.
struct Checked {
    a: usize,
    b: usize,
    word: String,
    ranked: Vec<(String, f32)>,
    in_dict: bool,
    /// Оценки с языковой моделью: порог другой.
    lm: bool,
    ru: bool,
}

/// Слово, оставленное на своей границе: языковая модель проверит его ещё раз по следующему слову.
struct Recheck {
    a: usize,
    b: usize,
    word: String,
    ru: bool,
}

/// Варианты для слова a..b в буфере; text — что там сейчас на экране.
struct Offer {
    a: usize,
    b: usize,
    text: String,
    items: Vec<String>,
}

/// Последняя автоправка: Pause возвращает как было.
struct Undo {
    before: Buffer,
    text: String,
    old: String,
    old_lay: u8,
    /// Словарное слово, которое заменила орфография: отмена делает его своим.
    word: Option<String>,
}

struct Worker {
    cfg: Config,
    model: Model,
    focus: Focus,
    lists: Lists,
    /// Свои слова: словарные, набранные и оставленные пользователем.
    memory: Memory,
    buf: Buffer,
    mods: Mods,
    caps: bool,
    hwnd: isize,
    proc_name: String,
    excluded: bool,
    /// В этой программе правим только раскладку.
    layout_only: bool,
    target_lay: Option<u8>,
    undo: Option<Undo>,
    stats: Stats,
    /// Двойной правый Ctrl: нажатие без других клавиш и время прошлого.
    rctrl_clean: bool,
    rctrl_tap: Option<Instant>,
    speller: Option<Speller>,
    speller_failed: bool,
    /// Сомнительные слова фразы: окно у курсора держится до конца фразы.
    offers: Vec<Offer>,
    /// Что сейчас показывает окно: строки по тексту слова.
    shown: Vec<String>,
    recheck: Option<Recheck>,
    /// Языковая модель грузится в фоне: сборка для NPU занимает десятки секунд.
    lm_loading: Option<Receiver<Result<Lm, String>>>,
    lm_failed: bool,
    /// Файлы модели удаляют или заменяют: не грузить до следующей команды.
    lm_hold: bool,
    /// Строка состояния без приписки о языковой модели.
    status: String,
}

/// Программа, где печатали последней: для пункта меню «не исправлять здесь».
pub static LAST_APP: Mutex<String> = Mutex::new(String::new());

fn process_name(hwnd: isize) -> String {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&mut pid));
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return String::new() };
        let mut buf = [0u16; 512];
        let mut n = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return String::new();
        }
        let path = String::from_utf16_lossy(&buf[..n as usize]);
        path.rsplit('\\').next().unwrap_or_default().to_lowercase()
    }
}

fn numpad_char(vk: u16) -> Option<char> {
    match VIRTUAL_KEY(vk) {
        v if (VK_NUMPAD0.0..=VK_NUMPAD9.0).contains(&v.0) => char::from_digit((v.0 - VK_NUMPAD0.0) as u32, 10),
        VK_MULTIPLY => Some('*'),
        VK_ADD => Some('+'),
        VK_SUBTRACT => Some('-'),
        VK_DIVIDE => Some('/'),
        _ => None,
    }
}

impl Worker {
    /// Учесть модификатор; true — сработал двойной правый Ctrl.
    fn modifier(&mut self, k: &Key) -> bool {
        let down = !k.up;
        let mut double = false;
        if k.vk == VK_RCONTROL.0 {
            if down && !self.mods.ctrl_r {
                self.rctrl_clean = true;
            } else if !down && self.rctrl_clean {
                let now = Instant::now();
                double = self.rctrl_tap.is_some_and(|t| now - t < Duration::from_millis(400));
                self.rctrl_tap = if double { None } else { Some(now) };
            }
        } else if down {
            self.rctrl_clean = false;
            self.rctrl_tap = None;
        }
        match VIRTUAL_KEY(k.vk) {
            VK_LSHIFT => self.mods.shift_l = down,
            VK_RSHIFT => self.mods.shift_r = down,
            VK_LCONTROL => self.mods.ctrl_l = down,
            VK_RCONTROL => self.mods.ctrl_r = down,
            VK_LMENU => self.mods.alt_l = down,
            VK_RMENU => self.mods.alt_r = down,
            VK_LWIN => self.mods.win_l = down,
            VK_RWIN => self.mods.win_r = down,
            VK_CAPITAL if down => self.caps = !self.caps,
            _ => {}
        }
        double
    }

    /// Символ печатной клавиши в раскладке lay.
    fn char_of(&self, k: &Key, lay: u8) -> Option<Ch> {
        if let Some(raw) = numpad_char(k.vk) {
            return Some(Ch::new(OTHER, lay, raw));
        }
        if k.ext {
            return None;
        }
        let idx = KEYMAP.key_index(k.scan)?;
        Some(Ch::new(KEYMAP.token(idx, self.mods.shift(), self.caps, lay), lay, '\0'))
    }

    fn handle(&mut self, ev: Event, rx: &Receiver<Event>, pending: &mut VecDeque<Event>) {
        let k = match ev {
            Event::Mouse => {
                self.buf.clear();
                self.undo = None;
                self.rctrl_tap = None;
                self.dismiss();
                return;
            }
            Event::Pick(i) => {
                self.pick(i);
                return;
            }
            Event::Key(k) => k,
            Event::Flush | Event::ResetStats | Event::Models | Event::LmUnload | Event::LmReady => return,
        };
        if is_modifier(k.vk) {
            if self.modifier(&k) && !self.excluded {
                self.pause();
            }
            return;
        }
        if k.up {
            return;
        }
        self.rctrl_clean = false;
        self.rctrl_tap = None;
        if let Some(i) = k.pick {
            self.pick(i);
            return;
        }
        self.lists.refresh();
        if k.hwnd != self.hwnd {
            self.hwnd = k.hwnd;
            self.buf.clear();
            self.undo = None;
            self.proc_name = process_name(k.hwnd);
            *LAST_APP.lock().unwrap() = self.proc_name.clone();
        }
        let mode = self.lists.programs.get(&self.proc_name).copied();
        self.excluded = self.cfg.exclude.contains(&self.proc_name) || mode == Some(Mode::Off);
        self.layout_only = mode == Some(Mode::Layout);
        if k.deferred {
            self.replay(k, rx, pending);
        } else {
            self.typed(k, rx, pending);
        }
    }

    /// Клавиша уже дошла до приложения (или Enter/Tab на проверке).
    fn typed(&mut self, k: Key, rx: &Receiver<Event>, pending: &mut VecDeque<Event>) {
        self.target_lay = None;
        if k.undo {
            // хук проглотил Backspace; отменять нечего — стереть как обычно
            if self.undo.is_some() {
                self.pause();
            } else {
                input::tap(k.vk, k.scan, k.ext);
                self.buf.pop();
            }
            return;
        }
        let Some(lay) = k.lay.filter(|_| !self.excluded && !self.mods.command()) else {
            self.buf.clear();
            self.undo = None;
            if k.boundary {
                input::tap(k.vk, k.scan, k.ext);
            }
            return;
        };
        if k.vk == VK_PAUSE.0 {
            self.pause();
            return;
        }
        self.undo = None;
        match VIRTUAL_KEY(k.vk) {
            VK_BACK => self.buf.pop(),
            VK_SPACE => {
                self.buf.push(Ch::new(SPACE, lay, ' '));
                if self.buf.word_just_ended() {
                    self.correct(rx, pending, true);
                }
            }
            VK_RETURN | VK_TAB if k.boundary => {
                // после Enter сообщение уже ушло: варианты не к месту
                self.correct(rx, pending, false);
                input::tap(k.vk, k.scan, k.ext);
                self.buf.clear();
            }
            _ => match self.char_of(&k, lay) {
                Some(ch) => self.buf.push(ch),
                None => self.buf.clear(),
            },
        }
    }

    /// Загрузить орфографию, если она включена и ещё не загружена.
    fn ensure_speller(&mut self) {
        if self.speller.is_some() || self.speller_failed || !settings::SPELL.load(SeqCst) {
            return;
        }
        let t = Instant::now();
        match load_speller(&self.cfg) {
            Ok(s) => {
                self.speller = Some(s);
                if self.cfg.debug {
                    println!("орфография загружена за {:?}", t.elapsed());
                }
            }
            Err(e) => {
                self.speller_failed = true;
                if self.cfg.debug {
                    println!("орфография не загружена: {e}");
                }
            }
        }
    }

    /// Языковая модель: подключить, когда готова; отпустить, когда выключена или удаляется.
    fn ensure_lm(&mut self) {
        let Some(sp) = self.speller.as_mut() else { return };
        if !settings::LM.load(SeqCst) || self.lm_hold {
            if sp.lm.take().is_some() {
                set_status(self.status.clone());
                if self.cfg.debug {
                    println!("языковая модель выгружена");
                }
            }
            // загрузку не бросаем: вторая параллельная не нужна; готовую, но лишнюю — выгрузить
            if let Some(rx) = &self.lm_loading
                && !matches!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty))
            {
                self.lm_loading = None;
            }
            return;
        }
        if sp.lm.is_some() {
            return;
        }
        if let Some(rx) = &self.lm_loading {
            let got = match rx.try_recv() {
                Ok(r) => r,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(_) => Err("загрузка прервана".to_string()),
            };
            self.lm_loading = None;
            match got {
                Ok(lm) => {
                    set_status(format!("{}; языковая модель на {}", self.status, lm.device));
                    if self.cfg.debug {
                        println!("языковая модель на {}", lm.device);
                    }
                    sp.lm = Some(lm);
                }
                Err(e) => {
                    self.lm_failed = true;
                    set_status(format!("{}; языковая модель не загружена: {e}", self.status));
                    if self.cfg.debug {
                        println!("языковая модель не загружена: {e}");
                    }
                }
            }
            return;
        }
        if self.lm_failed || !lmstore::installed(&self.cfg.lm_dir) {
            return;
        }
        if !lmstore::load_begin(&self.cfg.lm_dir) {
            // прошлая загрузка оборвалась — возможно, сбой на этом процессоре: сами не повторяем
            settings::set_lm(false);
            set_status(format!("{}; языковая модель выключена: прошлая загрузка оборвалась", self.status));
            if self.cfg.debug {
                println!("языковая модель выключена: прошлая загрузка оборвалась");
            }
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let cfg = self.cfg.clone();
        std::thread::spawn(move || {
            let loaded = load_lm(&cfg);
            lmstore::load_done(&cfg.lm_dir);
            let _ = tx.send(loaded);
            // подключить сразу, не дожидаясь следующего слова
            if let Some(ev) = crate::hook::SENDER.get() {
                let _ = ev.send(Event::LmReady);
            }
        });
        self.lm_loading = Some(rx);
    }

    /// Орфография последнего слова с учётом правки раскладки: все варианты модели.
    fn spell_check(&mut self, plan: Option<&Plan>) -> Option<Checked> {
        if !settings::SPELL.load(SeqCst) {
            self.speller = None; // выключили — освободить память
            return None;
        }
        if self.layout_only {
            return None;
        }
        self.ensure_speller();
        self.ensure_lm();
        let sp = self.speller.as_mut()?;
        let mut sim = self.buf.clone();
        if let Some(p) = plan {
            sim.apply(p.clone());
        }
        let (a, b, word, ctx, ru) = sim.spell_target()?;
        if self.lists.ignore.contains(&word) || self.lists.force.contains(&word) {
            return None;
        }
        if self.memory.safe(&word) && sp.dict(ru).contains(&word) {
            return None;
        }
        let r = match sp.rank(&ctx, &word, "", ru) {
            Ok(r) => r?,
            Err(e) => {
                if self.cfg.debug {
                    println!("  орфография: ошибка — {e}");
                }
                return None;
            }
        };
        if self.cfg.debug {
            let top: Vec<String> = r.items.iter().take(4).map(|(w, p)| format!("{w}={p:.2}")).collect();
            let tag = match (r.in_dict, r.lm) {
                (true, true) => " (словарное, с языковой моделью)",
                (true, false) => " (словарное)",
                (false, true) => " (с языковой моделью)",
                (false, false) => "",
            };
            println!("  орфография: «{word}»{tag} {}", top.join(" "));
        }
        Some(Checked { a, b, word, ranked: r.items, in_dict: r.in_dict, lm: r.lm, ru })
    }

    /// Предыдущее слово ещё раз, теперь с текстом справа: его видит только языковая модель.
    /// sim — буфер после правок этого шага.
    fn spell_recheck(&mut self, sim: &Buffer) -> Option<Checked> {
        let r = self.recheck.take()?;
        if self.layout_only {
            return None;
        }
        let sp = self.speller.as_mut()?;
        if sp.lm.is_none() || !r.ru {
            return None;
        }
        if r.b >= sim.chars.len() || sim.chars[r.a..r.b].iter().map(Ch::render).collect::<String>() != r.word {
            return None;
        }
        let ctx: String = sim.chars[..r.a].iter().map(Ch::render).collect();
        let right: String = sim.chars[r.b..].iter().map(Ch::render).collect();
        if right.trim().is_empty() {
            return None;
        }
        let ranked = sp.rank(&ctx, &r.word, &right, r.ru).ok()??;
        if self.cfg.debug {
            let top: Vec<String> = ranked.items.iter().take(4).map(|(w, p)| format!("{w}={p:.2}")).collect();
            println!("  перепроверка «{}» + «{}»: {}", r.word, right.trim_end(), top.join(" "));
        }
        if !ranked.lm {
            return None;
        }
        Some(Checked {
            a: r.a,
            b: r.b,
            word: r.word,
            ranked: ranked.items,
            in_dict: ranked.in_dict,
            lm: true,
            ru: r.ru,
        })
    }

    /// Правка словом другой длины: сдвинуть строки вариантов правее неё.
    fn shift_offers(&mut self, from: usize, delta: isize) {
        for x in self.offers.iter_mut().filter(|x| x.a >= from) {
            x.a = (x.a as isize + delta) as usize;
            x.b = (x.b as isize + delta) as usize;
        }
    }

    /// Варианты для окна у курсора; fixed — лучший уже подставлен.
    fn offer_items(c: &Checked, fixed: bool) -> Vec<String> {
        let min_p = if c.in_dict { OFFER_DICT_MIN_P } else { OFFER_MIN_P };
        let mut items: Vec<String> = c
            .ranked
            .iter()
            .filter(|(w, p)| *w != c.word && *p >= min_p)
            .skip(fixed as usize)
            .map(|(w, _)| w.clone())
            .take(popup::PER_ROW - fixed as usize)
            .collect();
        if fixed && !items.is_empty() {
            items.push(c.word.clone());
        }
        items
    }

    fn render(&self, a: usize, b: usize) -> String {
        self.buf.chars[a..b].iter().map(Ch::render).collect()
    }

    /// Добавить строку вариантов для слова a..b и обновить окно.
    fn add_offer(&mut self, a: usize, b: usize, items: Vec<String>) {
        if items.is_empty() || !settings::SUGGEST.load(SeqCst) || b > self.buf.chars.len() {
            return;
        }
        let text = self.render(a, b);
        if self.cfg.debug {
            println!("  варианты для «{text}»: {}", items.join(" · "));
        }
        self.offers.push(Offer { a, b, text, items });
        if self.offers.len() > popup::ROWS {
            self.offers.remove(0);
        }
        self.show_offers();
    }

    /// Окно по текущим строкам у курсора; без курсора — не показываем.
    fn show_offers(&mut self) {
        self.shown = self.offers.iter().map(|o| o.text.clone()).collect();
        if self.offers.is_empty() {
            popup::hide();
            return;
        }
        let Some(caret) = self.focus.caret() else {
            if self.cfg.debug {
                println!("  варианты: курсор не найден");
            }
            popup::hide();
            return;
        };
        if self.cfg.debug {
            println!("  окно вариантов: курсор ({})", caret.how);
        }
        let rows = self.offers.iter().map(|o| popup::Row { word: o.text.clone(), items: o.items.clone() }).collect();
        popup::show(rows, caret.rect);
    }

    /// Убрать строки слов, которых в буфере больше нет (стёрли, Enter, клик);
    /// true — окно отстало от списка и его надо перерисовать.
    fn sync_offers(&mut self) -> bool {
        let chars = &self.buf.chars;
        self.offers
            .retain(|o| o.b <= chars.len() && chars[o.a..o.b].iter().map(Ch::render).collect::<String>() == o.text);
        !self.offers.iter().map(|o| o.text.as_str()).eq(self.shown.iter().map(String::as_str))
    }

    /// Выбран вариант i (сквозной номер): заменить слово, хвост напечатать заново.
    fn pick(&mut self, i: usize) {
        let mut k = i;
        let Some(row) = self.offers.iter().position(|o| {
            let hit = k < o.items.len();
            if !hit {
                k -= o.items.len();
            }
            hit
        }) else {
            return;
        };
        let o = self.offers.remove(row);
        let word = o.items[k].clone();
        if o.b > self.buf.chars.len() || self.render(o.a, o.b) != o.text || self.focus.is_password() {
            self.show_offers();
            return;
        }
        CORRECTING.store(true, SeqCst);
        fence();
        // с зажатым Ctrl Backspace стёр бы слово целиком
        let t = Instant::now();
        while unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) } as u16 & 0x8000 != 0 {
            if t.elapsed() > Duration::from_secs(3) {
                CORRECTING.store(false, SeqCst);
                self.offers.insert(row, o);
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let before = self.buf.clone();
        if self.buf.replace_word(o.a, o.b, &word) {
            let old: String = before.chars[o.a..].iter().map(Ch::render).collect();
            let text: String = self.buf.chars[o.a..].iter().map(Ch::render).collect();
            if input::replace(before.chars.len() - o.a, &text) {
                self.stats.picks += 1;
                if self.cfg.debug {
                    println!("вариант {}: «{}» → «{word}»", i + 1, o.text);
                }
                let delta = self.buf.chars.len() as isize - before.chars.len() as isize;
                self.shift_offers(o.a + 1, delta);
                self.undo =
                    Some(Undo { before: before.clone(), text, old, old_lay: before.chars[o.a].lay, word: None });
                UNDO_READY.store(true, SeqCst);
            } else {
                self.buf.clear();
            }
        }
        CORRECTING.store(false, SeqCst);
        self.sync_offers();
        self.show_offers();
    }

    fn dismiss(&mut self) {
        if !self.offers.is_empty() {
            self.offers.clear();
            self.show_offers();
        }
    }

    /// Pause/Break: отменить последнюю правку или перевернуть последнее слово.
    fn pause(&mut self) {
        if let Some(u) = self.undo.take() {
            if input::replace(u.text.chars().count(), &u.old) {
                self.buf = u.before;
                self.buf.freeze();
                input::switch_layout(u.old_lay);
                self.stats.undos += 1;
                if let Some(w) = &u.word {
                    self.memory.hit(w, crate::memory::SAFE);
                }
                if self.cfg.debug {
                    println!("отмена: «{}» → «{}»", u.text, u.old);
                }
            } else {
                self.buf.clear();
            }
            return;
        }
        if self.focus.is_password() {
            return;
        }
        if let Some(fix) = self.buf.flip_last() {
            if input::replace(fix.erase, &fix.text) {
                input::switch_layout(fix.new_lay);
                self.target_lay = Some(fix.new_lay);
                self.stats.forced += 1;
                if self.cfg.debug {
                    println!("вручную: «{}» → «{}»", fix.old, fix.text);
                }
            } else {
                self.buf.clear();
            }
        }
    }

    /// Нажатие проглочено во время правки: напечатать заново.
    fn replay(&mut self, k: Key, rx: &Receiver<Event>, pending: &mut VecDeque<Event>) {
        self.undo = None;
        let lay = self.target_lay.or(k.lay);
        let printable =
            k.vk == VK_SPACE.0 || numpad_char(k.vk).is_some() || (!k.ext && KEYMAP.key_index(k.scan).is_some());
        let Some(lay) = lay.filter(|_| printable && !self.excluded && !self.mods.command()) else {
            input::tap(k.vk, k.scan, k.ext);
            if k.vk == VK_BACK.0 {
                self.buf.pop();
            } else {
                self.buf.clear();
            }
            return;
        };
        let ch = if k.vk == VK_SPACE.0 { Ch::new(SPACE, lay, ' ') } else { self.char_of(&k, lay).unwrap() };
        input::type_text(&ch.render().to_string());
        self.buf.push(ch);
        if ch.tok == SPACE && self.buf.word_just_ended() {
            self.correct(rx, pending, true);
        }
    }

    fn correct(&mut self, rx: &Receiver<Event>, pending: &mut VecDeque<Event>, offer_ok: bool) {
        // пароль: не смотреть и не трогать
        if self.focus.is_password() {
            self.buf.clear();
            self.stats.password_skips += 1;
            if self.cfg.debug {
                println!("  [{}] поле пароля — пропуск", self.proc_name);
            }
            return;
        }
        self.stats.words += 1;
        if self.cfg.debug {
            let lay = self
                .buf
                .chars
                .iter()
                .rev()
                .find(|c| !KEYMAP.neutral(c.tok))
                .map_or("?", |c| ["EN", "RU"][c.lay as usize]);
            if let Ok(words) = self.buf.probe(&mut self.model) {
                let w: Vec<String> = words.iter().map(|(w, p)| format!("{w}={p:.2}")).collect();
                println!("  [{} {lay}] {}", self.proc_name, w.join(" "));
            }
        }
        let t = std::time::Instant::now();
        let plan = self.buf.plan(&mut self.model, self.cfg.threshold, &self.lists);
        self.stats.infer_us += t.elapsed().as_micros() as u64;
        let plan = match plan {
            Ok(p) => p,
            Err(e) => {
                set_status(format!("ошибка прогона: {e}"));
                return;
            }
        };
        let checked = self.spell_check(plan.as_ref());
        let fix_of = |c: &Checked, thr: f32| {
            let (w, p) = &c.ranked[0];
            (*w != c.word && *p >= thr).then(|| (c.a, c.b, w.clone(), *p))
        };
        let thr_for =
            |sp: &Option<Speller>, c: &Checked| sp.as_ref().map_or(f32::INFINITY, |s| s.threshold_for(c.in_dict, c.lm));
        let spell = checked.as_ref().and_then(|c| fix_of(c, thr_for(&self.speller, c)));
        let dict_word = checked.as_ref().filter(|c| c.in_dict).map(|c| c.word.clone());
        if let Some(w) = &dict_word
            && spell.is_none()
        {
            self.memory.hit(w, 1);
        }
        // предыдущее слово с текстом справа — на буфере, каким он станет после правок этого шага
        let late = {
            let mut sim = self.buf.clone();
            if let Some(p) = &plan {
                sim.apply(p.clone());
            }
            if let Some((a, b, new, _)) = &spell {
                sim.replace_word(*a, *b, new);
            }
            self.spell_recheck(&sim)
        };
        let late_fix = late.as_ref().and_then(|c| fix_of(c, thr_for(&self.speller, c)));
        let late_word = late.as_ref().filter(|c| c.in_dict).map(|c| c.word.clone());
        // оставленное слово проверим ещё раз после следующего
        self.recheck = match &checked {
            Some(c) if spell.is_none() => Some(Recheck { a: c.a, b: c.b, word: c.word.clone(), ru: c.ru }),
            _ => None,
        };
        // двухбуквенное («оп» → «по») на границе не трогаем: решит языковая модель по следующему слову
        if checked.is_none() && self.speller.as_ref().is_some_and(|s| s.lm.is_some()) {
            let mut sim = self.buf.clone();
            if let Some(p) = &plan {
                sim.apply(p.clone());
            }
            if let Some((a, b, word, _, true)) = sim.spell_target_from(2)
                && word.chars().count() == 2
                && !self.lists.ignore.contains(&word)
                && !self.lists.force.contains(&word)
            {
                self.recheck = Some(Recheck { a, b, word, ru: true });
            }
        }
        // слово после автоправки могло стать длиннее
        let mut offer = checked.filter(|_| offer_ok).map(|c| {
            let b = spell.as_ref().map_or(c.b, |s| c.a + s.2.chars().count());
            (c.a, b, Self::offer_items(&c, spell.is_some()))
        });
        // варианты предыдущего слова: прежняя строка устарела
        if let Some(c) = &late {
            self.offers.retain(|o| o.a != c.a);
        }
        let late_offer =
            late.filter(|_| offer_ok && late_fix.is_none()).map(|c| (c.a, c.b, Self::offer_items(&c, false)));
        if plan.is_none() && spell.is_none() && late_fix.is_none() {
            for (a, b, items) in late_offer.into_iter().chain(offer) {
                self.add_offer(a, b, items);
            }
            return;
        }
        let mut done = false;
        CORRECTING.store(true, SeqCst);
        fence();
        // всё, что прошло до флага, уже на экране
        let mut after = Vec::new();
        let mut typed_more = false;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                Event::Key(k) if k.deferred => {
                    typed_more = true;
                    pending.push_back(Event::Key(k));
                }
                Event::Key(k) if is_modifier(k.vk) => {
                    let _ = self.modifier(&k);
                }
                Event::Key(k) if k.up => {}
                Event::Key(k) if after.is_empty() && !k.boundary && k.hwnd == self.hwnd && !self.mods.command() => {
                    let ch = if k.vk == VK_SPACE.0 {
                        k.lay.map(|lay| Ch::new(SPACE, lay, ' '))
                    } else {
                        k.lay.and_then(|lay| self.char_of(&k, lay))
                    };
                    match ch {
                        Some(ch) => {
                            typed_more = true;
                            self.buf.push(ch);
                        }
                        None => after.push(Event::Key(k)),
                    }
                }
                other => after.push(other),
            }
        }
        if after.is_empty() {
            let before = self.buf.clone();
            let old_lay = before.chars.iter().rev().find(|c| !KEYMAP.neutral(c.tok)).map_or(0, |c| c.lay);
            let mut first = usize::MAX;
            let mut switch = None;
            if let Some(plan) = plan {
                let fix = self.buf.apply(plan);
                first = before.chars.len() - fix.erase;
                if fix.last_flipped {
                    switch = Some(fix.new_lay);
                }
            }
            if let Some((a, b, new, _)) = &spell
                && self.buf.replace_word(*a, *b, new)
            {
                first = first.min(*a);
                self.stats.spell_fixes += 1;
            }
            if let Some((a, b, new, _)) = &late_fix
                && self.buf.replace_word(*a, *b, new)
            {
                first = first.min(*a);
                self.stats.spell_fixes += 1;
                if let Some(w) = &late_word {
                    self.memory.unhit(w);
                }
                let delta = new.chars().count() as isize - (*b - *a) as isize;
                self.shift_offers(*b, delta);
                if let Some(o) = offer.as_mut()
                    && o.0 >= *b
                {
                    o.0 = (o.0 as isize + delta) as usize;
                    o.1 = (o.1 as isize + delta) as usize;
                }
            }
            if first < before.chars.len() {
                let old: String = before.chars[first..].iter().map(Ch::render).collect();
                let text: String = self.buf.chars[first..].iter().map(Ch::render).collect();
                if !input::replace(before.chars.len() - first, &text) {
                    self.buf.clear();
                } else {
                    self.stats.fixes += 1;
                    *self.stats.by_app.entry(self.proc_name.clone()).or_default() += 1;
                    if self.cfg.debug {
                        let p = spell.as_ref().map_or(String::new(), |s| format!(" (орфография {:.2})", s.3));
                        let l =
                            late_fix.as_ref().map_or(String::new(), |s| format!(" (по следующему слову {:.2})", s.3));
                        println!("«{old}» → «{text}»{p}{l}");
                    }
                    let word = match (&spell, &late_fix) {
                        (Some(_), _) => dict_word.clone(),
                        (None, Some(_)) => late_word.clone(),
                        _ => None,
                    };
                    self.undo = Some(Undo { before, text, old, old_lay, word });
                    // Backspace отменяет, только если после правки ничего не набрано
                    if !typed_more {
                        UNDO_READY.store(true, SeqCst);
                        done = true;
                    }
                    if let Some(lay) = switch {
                        input::switch_layout(lay);
                        self.target_lay = Some(lay);
                    }
                }
            }
        }
        CORRECTING.store(false, SeqCst);
        if done && pending.is_empty() {
            for (a, b, items) in late_offer.into_iter().chain(offer) {
                self.add_offer(a, b, items);
            }
        }
        for ev in after.into_iter().rev() {
            pending.push_front(ev);
        }
    }
}

pub fn run(cfg: Config, rx: Receiver<Event>, caps: bool) {
    let t0 = std::time::Instant::now();
    let model = match load_model(&cfg) {
        Ok(m) => m,
        Err(e) => {
            set_status(format!("модель не загружена: {e}"));
            while rx.recv().is_ok() {}
            return;
        }
    };
    let status = format!("нейросеть на {}", model.device);
    set_status(status.clone());
    if cfg.debug {
        println!("модель на {} за {:?}", model.device, t0.elapsed());
    }
    READY.store(true, SeqCst);
    lists::ensure_files();
    lists::watch();
    let mut lists = Lists::default();
    let memory = Memory::load();
    lists.refresh();
    let stats = Stats::load();
    let mut saver = Saver::new(&stats);
    let mut w = Worker {
        cfg,
        model,
        focus: Focus::new(),
        lists,
        memory,
        buf: Buffer::default(),
        mods: Mods::default(),
        caps,
        hwnd: 0,
        proc_name: String::new(),
        excluded: false,
        layout_only: false,
        target_lay: None,
        undo: None,
        stats,
        rctrl_clean: false,
        rctrl_tap: None,
        speller: None,
        speller_failed: false,
        offers: Vec::new(),
        shown: Vec::new(),
        recheck: None,
        lm_loading: None,
        lm_failed: false,
        lm_hold: false,
        status,
    };
    w.ensure_speller();
    w.ensure_lm();
    let mut pending = VecDeque::new();
    loop {
        let ev = match pending.pop_front() {
            Some(e) => e,
            None => match rx.recv() {
                Ok(e) => e,
                Err(_) => return,
            },
        };
        match ev {
            Event::Flush => saver.force(&w.stats),
            Event::ResetStats => {
                w.stats = Stats::default();
                saver.force(&w.stats);
            }
            Event::Models => {
                w.lm_failed = false;
                w.lm_hold = false;
                w.ensure_lm();
            }
            Event::LmUnload => {
                w.lm_hold = true;
                w.ensure_lm();
            }
            Event::LmReady => w.ensure_lm(),
            ev => w.handle(ev, &rx, &mut pending),
        }
        if w.sync_offers() {
            w.show_offers();
        }
        HAS_WORD.store(w.buf.has_word() && !w.excluded, SeqCst);
        saver.tick(&w.stats);
    }
}
