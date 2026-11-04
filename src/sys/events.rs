//! Consulta ao Log de Eventos pela API nativa. Não exige administrador no log System.

use windows::core::*;
use windows::Win32::System::EventLog::*;

#[derive(Clone, Debug, Default)]
pub struct Event {
    pub id: u32,
    pub provider: String,
    /// Data e hora em UTC, como vem do log: 2026-11-04T10:22:00.
    pub time: String,
    pub data: String,
}

fn between<'a>(xml: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let a = xml.find(start)? + start.len();
    let b = xml[a..].find(end)?;
    Some(&xml[a..a + b])
}

fn parse(xml: &str) -> Event {
    let id = between(xml, "<EventID", "</EventID>").and_then(|s| s.rsplit('>').next()).and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    let provider = between(xml, "Provider Name='", "'").or(between(xml, "Provider Name=\"", "\"")).unwrap_or("").to_string();
    let time = between(xml, "SystemTime='", "'").or(between(xml, "SystemTime=\"", "\"")).unwrap_or("").chars().take(19).collect();
    let mut data = String::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<Data") {
        rest = &rest[i + 5..];
        let Some(open) = rest.find('>') else { break };
        if rest[..open].ends_with('/') {
            continue;
        }
        let Some(close) = rest[open..].find("</Data>") else { break };
        let v = &rest[open + 1..open + close];
        if !data.is_empty() {
            data.push(' ');
        }
        data.push_str(v);
        rest = &rest[open + close..];
    }
    Event { id, provider, time, data }
}

/// Eventos do log System que casam com o filtro XPath, do mais novo ao mais antigo.
pub fn query(xpath: &str, max: usize) -> Vec<Event> {
    let mut out = Vec::new();
    unsafe {
        let Ok(q) = EvtQuery(
            None,
            w!("System"),
            &HSTRING::from(xpath),
            EvtQueryChannelPath.0 | EvtQueryReverseDirection.0,
        ) else {
            return out;
        };
        'read: while out.len() < max {
            let mut handles = [0isize; 32];
            let mut got = 0u32;
            if EvtNext(q, &mut handles, 1000, 0, &mut got).is_err() {
                break;
            }
            for h in &handles[..got as usize] {
                let ev = EVT_HANDLE(*h);
                let mut used = 0u32;
                let mut props = 0u32;
                let _ = EvtRender(None, ev, EvtRenderEventXml.0, 0, None, &mut used, &mut props);
                let mut buf = vec![0u16; used as usize / 2 + 1];
                let ok = EvtRender(None, ev, EvtRenderEventXml.0, used, Some(buf.as_mut_ptr() as *mut _), &mut used, &mut props);
                if ok.is_ok() {
                    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
                    out.push(parse(&String::from_utf16_lossy(&buf[..end])));
                }
                let _ = EvtClose(ev);
                if out.len() >= max {
                    break 'read;
                }
            }
        }
        let _ = EvtClose(q);
    }
    out
}

const DAY_MS: u64 = 86_400_000;

/// Blocos defeituosos por número de disco nos últimos dias (Evento 7 do driver disk).
pub fn bad_blocks(days: u64) -> Vec<(u32, u32, String)> {
    let x = format!(
        "*[System[Provider[@Name='disk'] and (EventID=7) and TimeCreated[timediff(@SystemTime) <= {}]]]",
        days * DAY_MS
    );
    let mut out: Vec<(u32, u32, String)> = Vec::new();
    for e in query(&x, 500) {
        let Some(n) = e.data.to_ascii_lowercase().split("harddisk").nth(1).and_then(|s| {
            let d: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
            d.parse().ok()
        }) else {
            continue;
        };
        match out.iter_mut().find(|v| v.0 == n) {
            Some(v) => v.1 += 1,
            None => out.push((n, 1, e.time.clone())),
        }
    }
    out
}

/// Quedas e erros graves: reinício inesperado, tela azul, desligamento incorreto e hardware.
pub fn crashes(days: u64) -> Vec<Event> {
    let t = days * DAY_MS;
    let x = format!(
        "*[System[((Provider[@Name='Microsoft-Windows-Kernel-Power'] and EventID=41) or \
         (EventID=1001 and Provider[@Name='Microsoft-Windows-WER-SystemErrorReporting']) or \
         (Provider[@Name='EventLog'] and EventID=6008) or \
         Provider[@Name='Microsoft-Windows-WHEA-Logger']) and TimeCreated[timediff(@SystemTime) <= {t}]]]"
    );
    query(&x, 100)
}

/// Resultado mais recente do Diagnóstico de Memória do Windows.
pub fn memory_result() -> Option<Event> {
    let x = "*[System[Provider[@Name='Microsoft-Windows-MemoryDiagnostics-Results']]]";
    query(x, 1).into_iter().next()
}
