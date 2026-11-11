//! Idioma da interface. Todo texto passa pelo desenho, então a tradução mora só ali.
//! Em português nada disso roda; as tabelas só são montadas quando outro idioma é escolhido.
//! Cada idioma é um arquivo `res/xx.tsv` com "português<TAB>tradução". O que faltar num
//! idioma sai em inglês.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

pub const PT: u8 = 0;
pub const EN: u8 = 1;
pub const ES: u8 = 2;
pub const RU: u8 = 3;
pub const ZH: u8 = 4;

/// Nomes na lista de idiomas, cada um na própria língua, na ordem dos números acima.
pub const NAMES: [&str; 5] = ["Português (Brasil)", "English", "Español", "Русский", "简体中文"];

static LANG: AtomicU8 = AtomicU8::new(PT);

struct Pattern {
    parts: Vec<&'static str>,
    to: &'static str,
}

struct Table {
    exact: HashMap<&'static str, &'static str>,
    patterns: Vec<Pattern>,
}

fn build(src: &'static str) -> Table {
    let mut t = Table { exact: HashMap::new(), patterns: Vec::new() };
    for line in src.lines() {
        let Some((pt, to)) = line.split_once('\t') else { continue };
        if pt.contains("{}") {
            t.patterns.push(Pattern { parts: pt.split("{}").collect(), to });
        } else {
            t.exact.insert(pt, to);
        }
    }
    // O padrão com mais texto fixo é o mais específico.
    t.patterns.sort_by_key(|p| std::cmp::Reverse(p.parts.iter().map(|s| s.len()).sum::<usize>()));
    t
}

fn table(lang: u8) -> &'static Table {
    static T: [OnceLock<Table>; 4] = [OnceLock::new(), OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let (i, src) = match lang {
        ES => (1, include_str!("../res/es.tsv")),
        RU => (2, include_str!("../res/ru.tsv")),
        ZH => (3, include_str!("../res/zh.tsv")),
        _ => (0, include_str!("../res/en.tsv")),
    };
    T[i].get_or_init(|| build(src))
}

/// Idioma do Windows, para escolher na primeira abertura.
pub fn system_lang() -> u8 {
    let id = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    match id & 0x3ff {
        0x16 => PT,
        0x0a => ES,
        0x19 => RU,
        // Só o chinês simplificado (China e Singapura); o tradicional fica no inglês.
        0x04 if id == 0x0804 || id == 0x1004 => ZH,
        _ => EN,
    }
}

pub fn set_lang(lang: u8) {
    LANG.store(if (lang as usize) < NAMES.len() { lang } else { EN }, Ordering::Relaxed);
    CACHE.with(|c| c.borrow_mut().clear());
}

pub fn lang() -> u8 {
    LANG.load(Ordering::Relaxed)
}

/// Fora do português: o guia, as novidades e os formatos seguem o inglês.
pub fn is_english() -> bool {
    lang() != PT
}

/// Testes e fotos: liga o inglês.
pub fn set_english(on: bool) {
    set_lang(if on { EN } else { PT });
}

/// Separador decimal do idioma atual.
pub fn comma() -> &'static str {
    match lang() {
        EN | ZH => ".",
        _ => ",",
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

fn fill(to: &str, caps: &[String]) -> String {
    let mut out = String::with_capacity(to.len() + 16);
    let mut next = 0;
    let mut rest = to;
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

/// Tabelas a consultar, em ordem: a do idioma e, se não for o inglês, a do inglês.
fn tables() -> impl Iterator<Item = &'static Table> {
    let l = lang();
    std::iter::once(table(l)).chain((l != EN).then(|| table(EN)))
}

fn dynamic(s: &str, depth: u8) -> Option<String> {
    for t in tables() {
        for p in &t.patterns {
            if let Some(caps) = captures(p, s) {
                let caps: Vec<String> = caps.iter().map(|c| translate(c, depth + 1).unwrap_or_else(|| c.to_string())).collect();
                return Some(fill(p.to, &caps));
            }
        }
    }
    None
}

fn exact(s: &str) -> Option<&'static str> {
    tables().find_map(|t| t.exact.get(s).copied())
}

fn translate(s: &str, depth: u8) -> Option<String> {
    if let Some(to) = exact(s) {
        return Some(to.to_string());
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
    if lang() == PT {
        return Cow::Borrowed(s);
    }
    if let Some(to) = exact(s) {
        return Cow::Borrowed(to);
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

/// Teste: grava o que foi desenhado fora do português sem tradução.
#[cfg(debug_assertions)]
fn note_missing(s: &str) {
    use std::io::Write;
    if let Ok(path) = std::env::var("PRISMA_I18N_MISS") {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{s}");
        }
    }
}

#[cfg(test)]
mod tests {


    /// Os idiomas novos trazem os mesmos `{}` que o português, para nada se perder no meio.
    #[test]
    fn placeholders_match() {
        for (name, src) in [("es", include_str!("../res/es.tsv")), ("ru", include_str!("../res/ru.tsv")), ("zh", include_str!("../res/zh.tsv")), ("en", include_str!("../res/en.tsv"))] {
            for line in src.lines() {
                let Some((pt, to)) = line.split_once('\t') else { continue };
                let holes = |s: &str| s.matches('{').count();
                assert_eq!(holes(pt), holes(to), "{name}: {line}");
            }
        }
    }
}
