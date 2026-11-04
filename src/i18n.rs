//! Idioma da interface. Todo texto passa pelo desenho, então a tradução mora só ali.
//! Em português nada disso roda; a tabela só é montada se o inglês for escolhido.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

static EN: AtomicBool = AtomicBool::new(false);

struct Pattern {
    parts: Vec<&'static str>,
    en: &'static str,
}

struct Table {
    exact: HashMap<&'static str, &'static str>,
    patterns: Vec<Pattern>,
}

fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = Table { exact: HashMap::new(), patterns: Vec::new() };
        for line in include_str!("../res/en.tsv").lines() {
            let Some((pt, en)) = line.split_once('\t') else { continue };
            if pt.contains("{}") {
                t.patterns.push(Pattern { parts: pt.split("{}").collect(), en });
            } else {
                t.exact.insert(pt, en);
            }
        }
        // O padrão com mais texto fixo é o mais específico.
        t.patterns.sort_by_key(|p| std::cmp::Reverse(p.parts.iter().map(|s| s.len()).sum::<usize>()));
        t
    })
}

/// Linguagem do Windows, para escolher o idioma na primeira abertura.
pub fn system_is_portuguese() -> bool {
    const LANG_PORTUGUESE: u16 = 0x16;
    unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() & 0x3ff == LANG_PORTUGUESE }
}

pub fn set_english(on: bool) {
    EN.store(on, Ordering::Relaxed);
}

pub fn is_english() -> bool {
    EN.load(Ordering::Relaxed)
}

/// Separador decimal do idioma atual.
pub fn comma() -> &'static str {
    if is_english() {
        "."
    } else {
        ","
    }
}

fn captures<'a>(p: &Pattern, s: &'a str) -> Option<Vec<&'a str>> {
    let first = p.parts[0];
    let last = *p.parts.last()?;
    let body = s.strip_prefix(first)?.strip_suffix(last)?;
    let mut out = Vec::with_capacity(p.parts.len() - 1);
    let mut rest = body;
    for part in &p.parts[1..p.parts.len() - 1] {
        let i = rest.find(part)?;
        if i == 0 {
            return None;
        }
        out.push(&rest[..i]);
        rest = &rest[i + part.len()..];
    }
    if rest.is_empty() {
        return None;
    }
    out.push(rest);
    Some(out)
}

fn fill(en: &str, caps: &[String]) -> String {
    let mut out = String::with_capacity(en.len() + 16);
    let mut next = 0;
    let mut rest = en;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        let Some(end) = rest.find('}') else { break };
        let index = rest[..end].parse::<usize>().ok().map_or(next, |n| n.saturating_sub(1));
        if let Some(c) = caps.get(index) {
            out.push_str(c);
        }
        next = index + 1;
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

fn dynamic(s: &str, depth: u8) -> Option<String> {
    let t = table();
    for p in &t.patterns {
        if let Some(caps) = captures(p, s) {
            let caps: Vec<String> = caps.iter().map(|c| translate(c, depth + 1).unwrap_or_else(|| c.to_string())).collect();
            return Some(fill(p.en, &caps));
        }
    }
    None
}

fn translate(s: &str, depth: u8) -> Option<String> {
    if let Some(en) = table().exact.get(s) {
        return Some(en.to_string());
    }
    if depth < 3 {
        dynamic(s, depth)
    } else {
        None
    }
}

thread_local! {
    static CACHE: RefCell<HashMap<String, Option<String>>> = RefCell::new(HashMap::new());
}

/// Texto no idioma atual. Sem tradução, devolve o original: nomes de programas, por exemplo.
pub fn tr(s: &str) -> Cow<'_, str> {
    if !is_english() {
        return Cow::Borrowed(s);
    }
    if let Some(en) = table().exact.get(s) {
        return Cow::Borrowed(en);
    }
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(v) = c.get(s) {
            return v.as_ref().map_or(Cow::Borrowed(s), |e| Cow::Owned(e.clone()));
        }
        // Números mudam a cada segundo: o cache não pode crescer para sempre.
        if c.len() > 4096 {
            c.clear();
        }
        let v = dynamic(s, 0);
        #[cfg(debug_assertions)]
        if v.is_none() {
            note_missing(s);
        }
        c.insert(s.to_string(), v.clone());
        v.map_or(Cow::Borrowed(s), Cow::Owned)
    })
}

/// Teste: grava o que foi desenhado em inglês sem tradução.
#[cfg(debug_assertions)]
fn note_missing(s: &str) {
    use std::io::Write;
    if let Ok(path) = std::env::var("PRISMA_I18N_MISS") {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{s}");
        }
    }
}
