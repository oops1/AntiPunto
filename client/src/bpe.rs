//! Токенизатор языковой модели: BPE по байтам UTF-8, как у GPT-2.

use std::collections::HashMap;
use std::path::Path;

pub struct Bpe {
    vocab: HashMap<String, u32>,
    /// «левый правый» → очерёдность слияния.
    ranks: HashMap<String, u32>,
    unk: u32,
    bytes: [char; 256],
    cache: HashMap<String, Vec<u32>>,
}

/// Байт → символ словаря: печатные как есть, остальные со сдвигом за U+0100.
fn byte_chars() -> [char; 256] {
    let mut out = ['\0'; 256];
    let mut n = 0u32;
    for b in 0..256u32 {
        let direct = (0x21..=0x7e).contains(&b) || (0xa1..=0xac).contains(&b) || (0xae..=0xff).contains(&b);
        let c = if direct {
            b
        } else {
            n += 1;
            255 + n
        };
        out[b as usize] = char::from_u32(c).unwrap_or('\0');
    }
    out
}

const CONTRACTIONS: [&str; 7] = ["'s", "'t", "'re", "'ve", "'m", "'ll", "'d"];

/// Куски текста, как у GPT-2: слово, число или знаки с пробелом перед ними; пробелы отдельно.
fn split(text: &str) -> Vec<&str> {
    let cs: Vec<(usize, char)> = text.char_indices().collect();
    let at = |k: usize| cs.get(k).map_or(text.len(), |c| c.0);
    let run = |from: usize, pred: &dyn Fn(char) -> bool| {
        let mut j = from;
        while j < cs.len() && pred(cs[j].1) {
            j += 1;
        }
        j
    };
    let other = |c: char| !c.is_whitespace() && !c.is_alphabetic() && !c.is_numeric();
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i].1;
        if c == '\'' {
            let rest = &text[cs[i].0..];
            if let Some(m) = CONTRACTIONS.iter().find(|m| rest.starts_with(**m)) {
                out.push(&rest[..m.len()]);
                i += m.len();
                continue;
            }
        }
        // один пробел приклеивается к слову после него
        let k = if c == ' ' { i + 1 } else { i };
        let mut j = run(k, &|c| c.is_alphabetic());
        if j == k {
            j = run(k, &|c| c.is_numeric());
        }
        if j == k {
            j = run(k, &other);
        }
        if j == k {
            // пробелы: последний из них оставляем слову, если оно есть
            j = run(i, &|c| c.is_whitespace());
            if j < cs.len() && j - i > 1 {
                j -= 1;
            }
        }
        out.push(&text[cs[i].0..at(j)]);
        i = j;
    }
    out
}

impl Bpe {
    /// vocab — токены по номерам, по одному в строке; merges — пары «левый правый» по очерёдности.
    pub fn load(vocab: &Path, merges: &Path, unk: u32) -> Result<Bpe, String> {
        let read = |p: &Path| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
        let vocab = read(vocab)?.lines().enumerate().map(|(i, t)| (t.to_string(), i as u32)).collect();
        let ranks = read(merges)?.lines().enumerate().map(|(i, l)| (l.to_string(), i as u32)).collect();
        Ok(Bpe { vocab, ranks, unk, bytes: byte_chars(), cache: HashMap::new() })
    }

    fn piece(&self, piece: &str) -> Vec<u32> {
        let mut parts: Vec<String> = piece.bytes().map(|b| self.bytes[b as usize].to_string()).collect();
        loop {
            let best = (0..parts.len().saturating_sub(1))
                .filter_map(|i| self.ranks.get(&format!("{} {}", parts[i], parts[i + 1])).map(|r| (*r, i)))
                .min();
            let Some((_, at)) = best else { break };
            let (a, b) = (parts[at].clone(), parts[at + 1].clone());
            let mut merged = Vec::with_capacity(parts.len());
            let mut i = 0;
            while i < parts.len() {
                if i + 1 < parts.len() && parts[i] == a && parts[i + 1] == b {
                    merged.push(format!("{a}{b}"));
                    i += 2;
                } else {
                    merged.push(std::mem::take(&mut parts[i]));
                    i += 1;
                }
            }
            parts = merged;
        }
        parts.iter().map(|p| self.vocab.get(p).copied().unwrap_or(self.unk)).collect()
    }

    pub fn encode(&mut self, text: &str) -> Vec<u32> {
        let mut out = Vec::new();
        for p in split(text) {
            if let Some(ids) = self.cache.get(p) {
                out.extend(ids);
                continue;
            }
            let ids = self.piece(p);
            out.extend(&ids);
            if self.cache.len() < 20_000 {
                self.cache.insert(p.to_string(), ids);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces() {
        assert_eq!(split("я хочу  домой "), ["я", " хочу", " ", " домой", " "]);
        assert_eq!(split("it's 15%, да"), ["it", "'s", " 15", "%,", " да"]);
        assert_eq!(split("a\tb"), ["a", "\t", "b"]);
        assert!(split("").is_empty());
    }

    /// Токены как у исходного токенизатора: tests/lm_tokens.tsv пишет train/lm_fixture.py.
    #[test]
    fn tokens_match_python() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let dir = crate::lmstore::unpack_for_tests();
        let m = crate::lmstore::manifest();
        let mut bpe = Bpe::load(&dir.join("lm_vocab.txt"), &dir.join("lm_merges.txt"), m.meta("unk") as u32).unwrap();
        let fixture = std::fs::read_to_string(root.join("tests/lm_tokens.tsv")).expect("lm_tokens.tsv");
        let mut n = 0;
        for line in fixture.lines() {
            let (text, ids) = line.split_once('\t').unwrap_or((line, ""));
            let text = text.replace("\\t", "\t").replace("\\n", "\n").replace("\\\\", "\\");
            let want: Vec<u32> = ids.split(' ').filter_map(|v| v.parse().ok()).collect();
            assert_eq!(bpe.encode(&text), want, "{text:?}");
            n += 1;
        }
        assert!(n >= 100, "мало примеров: {n}");
    }
}
