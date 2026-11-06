//! Языковая модель: насколько вариант слова подходит к тексту вокруг (как train/lm_ov.py).

use std::path::Path;

use crate::bpe::Bpe;
use crate::lmstore;
use crate::ov::Model;

/// Столько знаков слева и справа от слова отдаём модели.
const LEFT_CHARS: usize = 60;
const RIGHT_CHARS: usize = 30;

pub struct Lm {
    body: Model,
    head: Model,
    bpe: Bpe,
    bos: i32,
    /// Строк, токенов в строке, токенов хвоста, ширина состояния.
    b: usize,
    t: usize,
    s: usize,
    h: usize,
    pub device: String,
}

fn bits(v: &[f32]) -> &[i32] {
    // SAFETY: f32 и i32 одного размера и выравнивания; тензор получает те же байты
    unsafe { std::slice::from_raw_parts(v.as_ptr().cast(), v.len()) }
}

impl Lm {
    /// Тело — на первом доступном устройстве, выходной слой — на процессоре. Файлы сверяются с перечнем.
    pub fn load(ov_dir: &Path, dir: &Path, devices: &[&str], cache_dir: &Path) -> Result<Lm, String> {
        lmstore::verify(dir)?;
        let m = lmstore::manifest();
        let (b, t, s, h) = (m.meta("rows"), m.meta("tokens"), m.meta("tail"), m.meta("hidden"));
        let bpe = Bpe::load(&dir.join("lm_vocab.txt"), &dir.join("lm_merges.txt"), m.meta("unk") as u32)?;
        let body =
            Model::load_shaped(ov_dir, &dir.join("lm_body.xml"), devices, cache_dir, &[b * t, b * t], b * t * h)?;
        let head =
            Model::load_shaped(ov_dir, &dir.join("lm_head.xml"), &["CPU"], cache_dir, &[b * s * h, b * s, b * s], b)?;
        let device = body.device.clone();
        Ok(Lm { body, head, bpe, bos: m.meta("bos") as i32, b, t, s, h, device })
    }

    /// Логарифм вероятности хвоста строки «left + вариант + right» для каждого варианта,
    /// с точностью до общей для всех постоянной. None — строки не помещаются в окно модели.
    pub fn score(&mut self, left: &str, opts: &[&str], right: &str) -> Result<Option<Vec<f32>>, String> {
        let (b, t, s, h) = (self.b, self.t, self.s, self.h);
        if opts.is_empty() || opts.len() > b {
            return Ok(None);
        }
        let lc: Vec<char> = left.chars().collect();
        let left: String = lc[lc.len().saturating_sub(LEFT_CHARS)..].iter().collect();
        let right: String = right.chars().take(RIGHT_CHARS).collect();
        let rows: Vec<Vec<u32>> = opts.iter().map(|o| self.bpe.encode(&format!("{left}{o}{right}"))).collect();
        // общее начало строк на оценки не влияет: считаем только хвосты
        let mut p = 0;
        while rows.iter().all(|r| r.len() > p && r[p] == rows[0][p]) {
            p += 1;
        }
        let tail = rows.iter().map(|r| r.len() - p).max().unwrap_or(0);
        if tail == 0 || tail > s || tail > t - 1 {
            return Ok(None);
        }
        let keep = p.min(t - 1 - tail);
        let mut ids = vec![0i32; b * t];
        let mut att = vec![0i32; b * t];
        let mut first = vec![0usize; b];
        for r in 0..b {
            let row = rows.get(r).unwrap_or(&rows[0]);
            let text = &row[p - keep..];
            let start = t - text.len();
            // начало строки, дыра, текст прижат вправо
            ids[r * t] = self.bos;
            att[r * t] = 1;
            for (k, &id) in text.iter().enumerate() {
                ids[r * t + start + k] = id as i32;
                att[r * t + start + k] = 1;
            }
            first[r] = start - 1;
        }
        let mut hidden = vec![0f32; b * t * h];
        self.body.infer(&[&ids, &att], &mut hidden)?;
        let mut hh = vec![0f32; b * s * h];
        let mut tgt = vec![0i32; b * s];
        let mut mask = vec![0f32; b * s];
        for r in 0..b {
            let n = rows.get(r).map_or(0, |row| row.len() - p);
            for k in 0..s {
                let pos = t - s + k;
                // первый токен текста предсказывает начало строки, а не дыра перед ним
                let pred = if pos - 1 == first[r] { 0 } else { pos - 1 };
                hh[(r * s + k) * h..][..h].copy_from_slice(&hidden[(r * t + pred) * h..][..h]);
                tgt[r * s + k] = ids[r * t + pos];
                mask[r * s + k] = (k >= s - n) as u8 as f32;
            }
        }
        let mut out = vec![0f32; b];
        self.head.infer(&[bits(&hh), &tgt, bits(&mask)], &mut out)?;
        out.truncate(opts.len());
        Ok(out.iter().all(|v| v.is_finite()).then_some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ov_dir() -> Option<std::path::PathBuf> {
        let d = std::env::var_os("ANTIPUNTO_OV").map(std::path::PathBuf::from)?;
        d.join("openvino_c.dll").exists().then_some(d)
    }

    /// Подходящее по смыслу слово получает оценку выше. Нужен OpenVINO: ANTIPUNTO_OV=<папка с openvino_c.dll>.
    #[test]
    fn context_prefers_fitting_word() {
        let Some(ov) = ov_dir() else {
            eprintln!("пропуск: нет ANTIPUNTO_OV");
            return;
        };
        let dir = lmstore::unpack_for_tests();
        let cache = std::env::temp_dir().join("antipunto-test-ovcache");
        let _ = std::fs::create_dir_all(&cache);
        let mut lm = Lm::load(&ov, &dir, &["CPU"], &cache).unwrap();
        let sc = lm.score("я хочу ", &["удететь", "улететь", "удалить"], " домой на самолёте").unwrap().unwrap();
        assert!(sc[1] > sc[0] && sc[1] > sc[2], "{sc:?}");
        let sc = lm.score("", &["бела", "была"], " у меня собака").unwrap().unwrap();
        assert!(sc[1] > sc[0], "{sc:?}");
        // одинаковые варианты и слишком длинный хвост — без оценки
        assert!(lm.score("а ", &["б", "б"], "").unwrap().is_none());
    }
}
