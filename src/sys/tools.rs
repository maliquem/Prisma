//! Ferramentas do Windows com saída ao vivo e veredito em português.
//! Código de saída engana (sfc, defrag), então cada fluxo decide pelo que realmente importa.

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Globalization::*;

use super::disks::Level;

#[derive(Clone, Copy, PartialEq)]
pub enum Enc {
    Oem,
    Ansi,
    Utf8,
    Utf16,
}

fn decode(cp: Enc, b: &[u8]) -> String {
    match cp {
        Enc::Utf8 => String::from_utf8_lossy(b).to_string(),
        Enc::Utf16 => {
            let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            String::from_utf16_lossy(&w)
        }
        _ => unsafe {
            let page = if cp == Enc::Oem { GetOEMCP() } else { GetACP() };
            let n = MultiByteToWideChar(page, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), b, None);
            let mut w = vec![0u16; n.max(0) as usize];
            MultiByteToWideChar(page, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), b, Some(&mut w));
            String::from_utf16_lossy(&w)
        },
    }
}

pub type Call = Box<dyn FnOnce(&mut Vec<String>) -> bool + Send>;

pub enum Work {
    Cmd { exe: String, args: Vec<String>, enc: Enc },
    Native(Call),
}

pub struct Step {
    pub label: String,
    pub work: Work,
}

pub fn cmd(label: &str, exe: &str, args: &[&str], enc: Enc) -> Step {
    Step {
        label: label.to_string(),
        work: Work::Cmd { exe: exe.to_string(), args: args.iter().map(|s| s.to_string()).collect(), enc },
    }
}

pub fn native(label: &str, f: impl FnOnce(&mut Vec<String>) -> bool + Send + 'static) -> Step {
    Step { label: label.to_string(), work: Work::Native(Box::new(f)) }
}

#[derive(Clone, Default)]
pub struct Run {
    pub running: bool,
    pub step: String,
    pub lines: Vec<String>,
    pub verdict: Option<(Level, String)>,
}

pub type Shared = Arc<Mutex<Run>>;

fn push(s: &Shared, line: &str) {
    let line = line.trim_end();
    if line.trim().is_empty() {
        return;
    }
    let mut r = s.lock().unwrap();
    // Linhas de progresso se sobrescrevem em vez de encher a tela.
    if line.contains('%') {
        if let Some(l) = r.lines.last_mut() {
            if l.contains('%') && l.chars().take(8).eq(line.chars().take(8)) {
                *l = line.to_string();
                return;
            }
        }
    }
    r.lines.push(line.to_string());
    let over = r.lines.len().saturating_sub(800);
    if over > 0 {
        r.lines.drain(..over);
    }
}

fn pump(mut o: impl Read, enc: Enc, out: &Shared) {
    let mut buf = [0u8; 4096];
    let mut pending: Vec<u8> = Vec::new();
    let mut text = String::new();
    while let Ok(n) = o.read(&mut buf) {
        if n == 0 {
            break;
        }
        pending.extend_from_slice(&buf[..n]);
        let take = if enc == Enc::Utf16 { pending.len() & !1 } else { pending.len() };
        text.push_str(&decode(enc, &pending[..take]));
        pending.drain(..take);
        while let Some(i) = text.find(['\n', '\r']) {
            let line: String = text.drain(..=i).collect();
            push(out, &line);
        }
    }
    push(out, &text);
}

fn run_cmd(exe: &str, args: &[String], enc: Enc, out: &Shared) -> Option<i32> {
    let mut c = Command::new(exe)
        .args(args)
        .creation_flags(0x0800_0000)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .ok()?;
    let (o, e) = (c.stdout.take()?, c.stderr.take()?);
    let errors = out.clone();
    let t = std::thread::spawn(move || pump(e, enc, &errors));
    pump(o, enc, out);
    let _ = t.join();
    c.wait().ok()?.code()
}

/// Comando como seria digitado, para quem acompanha saber o que rodou.
fn shown(exe: &str, args: &[String]) -> String {
    let mut s = format!("  > {exe}");
    for a in args {
        if a.contains(' ') {
            s += &format!(" \"{a}\"");
        } else {
            s += &format!(" {a}");
        }
    }
    s
}

pub fn elapsed(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 60 {
        format!("{} min {} s", s / 60, s % 60)
    } else {
        format!("{:.1} s", d.as_secs_f32()).replace('.', crate::i18n::comma())
    }
}

/// Resultado de cada etapa, entregue ao veredito do fluxo.
pub struct Done {
    pub label: String,
    pub code: Option<i32>,
    pub ok: bool,
}

pub type Verdict = Box<dyn FnOnce(&[Done]) -> (Level, String) + Send>;

pub fn start(steps: Vec<Step>, verdict: Verdict, shared: Shared) {
    *shared.lock().unwrap() = Run { running: true, ..Default::default() };
    let _ = std::thread::Builder::new().name("ferramenta".into()).spawn(move || {
        let mut done = Vec::new();
        let begin = Instant::now();
        for s in steps {
            shared.lock().unwrap().step = s.label.clone();
            push(&shared, &format!("▸ {}  {}", clock(), s.label));
            let t0 = Instant::now();
            let (code, ok) = match s.work {
                Work::Cmd { exe, args, enc } => {
                    push(&shared, &shown(&exe, &args));
                    let c = run_cmd(&exe, &args, enc, &shared);
                    (c, c == Some(0))
                }
                Work::Native(f) => {
                    let mut lines = Vec::new();
                    let ok = f(&mut lines);
                    for l in lines {
                        push(&shared, &l);
                    }
                    (None, ok)
                }
            };
            let time = elapsed(t0.elapsed());
            // Vários programas devolvem código diferente de zero sem ter falhado: o veredito decide.
            push(&shared, &match (ok, code) {
                (true, _) => format!("  ✓ Etapa concluída em {time}"),
                (false, Some(c)) => format!("  Terminou com código {c} em {time}"),
                (false, None) => format!("  ✗ Não concluiu ({time})"),
            });
            done.push(Done { label: s.label, code, ok });
        }
        push(&shared, &format!("▸ {}  Fim, em {}", clock(), elapsed(begin.elapsed())));
        let v = verdict(&done);
        let mut r = shared.lock().unwrap();
        r.step.clear();
        r.verdict = Some(v);
        r.running = false;
    });
}

pub fn simple(ok_text: &'static str, fail_text: &'static str) -> Verdict {
    Box::new(move |d: &[Done]| {
        if d.iter().all(|s| s.ok) {
            (Level::Good, ok_text.to_string())
        } else {
            let names: Vec<&str> = d.iter().filter(|s| !s.ok).map(|s| s.label.as_str()).collect();
            (Level::Warn, format!("{fail_text}: {}", names.join(", ")))
        }
    })
}

pub fn neutral(text: &'static str) -> Verdict {
    Box::new(move |_| (Level::Unknown, text.to_string()))
}

/// Hora atual, para as linhas do passo a passo.
pub fn clock() -> String {
    now_stamp().get(11..).unwrap_or_default().to_string()
}

pub fn now_stamp() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// O SFC devolve 0 mesmo com problema: o que vale é o que ele gravou no CBS.log.
pub fn sfc_verdict(since: &str, log: &str) -> (Level, String) {
    let (mut repaired, mut failed, mut seen) = (0, 0, false);
    for l in log.lines() {
        if !l.contains("[SR]") || l.get(..19).is_none_or(|t| t < since) {
            continue;
        }
        seen = true;
        if l.contains("Cannot repair") {
            failed += 1;
        } else if l.contains("Repairing") || l.contains("Repaired file") {
            repaired += 1;
        }
    }
    if !seen {
        (Level::Unknown, "Não achei o registro do SFC. Confira a saída acima.".to_string())
    } else if failed > 0 {
        (Level::Risk, format!("O SFC não conseguiu reparar {failed} arquivos. Veja CBS.log e rode o reparo de novo após reiniciar."))
    } else if repaired > 0 {
        (Level::Warn, format!("Reparou {repaired} arquivos. Reinicie e rode de novo para confirmar."))
    } else {
        (Level::Good, "Windows íntegro: nenhum arquivo corrompido.".to_string())
    }
}

pub fn read_cbs() -> String {
    let path = std::env::var("windir").unwrap_or_else(|_| "C:\\Windows".into()) + "\\Logs\\CBS\\CBS.log";
    std::fs::read(path).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default()
}

/// Categorias da Limpeza de Disco que não apagam nada do usuário.
pub const CLEAN: &[&str] = &[
    "Temporary Files",
    "Thumbnail Cache",
    "Update Cleanup",
    "Windows Error Reporting Files",
    "Delivery Optimization Files",
    "Temporary Setup Files",
    "Downloaded Program Files",
    "Old ChkDsk Files",
    "Setup Log Files",
    "System error memory dump files",
];
