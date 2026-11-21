//! Relatório de bug: junta o formulário, os dados do PC e o registro recente.
//! A página no GitHub é pública, então nada pessoal entra aqui sem a pessoa escrever.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::{w, Interface, HSTRING};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::capture;
use crate::i18n::tr;
use crate::sys::monitor::Snapshot;
use crate::sys::reg::{self, Hive};
use crate::sys::update;
use crate::ui::edit::TextBox;

const REPO: &str = "maliquem/Prisma";
/// Deslogado, o GitHub reembala o endereço em `/login?return_to=` e dá erro 500 quando esse passa de ~7000
/// (medido em 2026-11-21: 6866 passa, 7541 não). Por isso a conta usa o endereço reembalado.
const URL_MAX: usize = 6500;
pub const TEXT_MAX: usize = 4000;
const LOG_LINES: usize = 30;
/// GitHub App do Prisma com device flow. Vazio esconde o envio direto.
pub const CLIENT_ID: &str = "";

static NOTES: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// Guarda em memória o que aconteceu há pouco, para explicar o bug depois.
pub fn note(text: &str) {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    if let Ok(mut n) = NOTES.lock() {
        if n.len() == LOG_LINES {
            n.pop_front();
        }
        n.push_back(format!("{:02}:{:02}:{:02} {text}", t.wHour, t.wMinute, t.wSecond));
    }
}

pub struct Photo {
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
    /// Miniatura pronta: o desenho não reduz a foto inteira a cada quadro.
    pub thumb: (u32, u32, Vec<u8>),
}

impl Photo {
    pub fn new(w: u32, h: u32, bgra: Vec<u8>) -> Self {
        let step = (w.max(h) / 512 + 1) as usize;
        let (tw, th) = (w as usize / step, h as usize / step);
        let mut px = Vec::with_capacity(tw * th * 4);
        for y in 0..th {
            for x in 0..tw {
                let o = (y * step * w as usize + x * step) * 4;
                px.extend_from_slice(&bgra[o..o + 4]);
            }
        }
        Self { w, h, bgra, thumb: (tw as u32, th as u32, px) }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum View {
    Form,
    Preview,
    Sent,
    Direct,
}

#[derive(Clone, Default)]
pub enum Direct {
    #[default]
    Idle,
    Asking,
    Code(String),
    Sending,
    Done,
    Failed(String),
}

pub struct Ready {
    pub title: String,
    pub full: String,
    pub url: String,
    /// O texto não coube no endereço e precisa ser colado à mão.
    pub short: bool,
}

pub struct Form {
    pub area: usize,
    pub level: usize,
    pub extras: bool,
    pub draft: String,
    pub photo: Option<Photo>,
    pub view: View,
    pub ready: Option<Ready>,
    pub direct: Arc<Mutex<Direct>>,
    pub stop: Arc<AtomicBool>,
    pub text: Option<TextBox>,
}

impl Default for Form {
    fn default() -> Self {
        Self { area: 0, level: 1, extras: true, draft: String::new(), photo: None, view: View::Form, ready: None, direct: Arc::default(), stop: Arc::default(), text: None }
    }
}

impl Form {
    /// A janela fechou para a bandeja: guarda o texto antes de a caixa sumir com ela.
    pub fn window_closed(&mut self) {
        if let Some(t) = self.text.take() {
            self.draft = t.text();
        }
    }

    pub fn reset(&mut self) {
        *self = Self { area: self.area, ..Self::default() };
    }

    fn title(&self, area: &str) -> String {
        let first = self.draft.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
        let mut cut: String = first.chars().take(70).collect();
        if first.chars().count() > 70 {
            cut.push_str("...");
        }
        format!("[{}] {area}: {cut}", tr(LEVELS[self.level]))
    }

    fn body(&self, area: &str, info: Option<(&str, &[String])>, cut: bool) -> String {
        let mut b = format!("| {} | {} | {} |\n| --- | --- | --- |\n| {area} | {} | {} |\n\n", tr("Função"), tr("Gravidade"), tr("Versão"), tr(LEVELS[self.level]), update::current());
        b.push_str(&format!("### {}\n\n", tr("O que aconteceu")));
        if cut {
            b.push_str(tr("O texto não coube no endereço. Apague esta linha e cole o relatório completo (Ctrl+V).").as_ref());
        } else {
            b.push_str(self.draft.replace('\r', "").trim());
        }
        b.push_str("\n\n");
        if self.photo.is_some() {
            b.push_str(&format!("### {}\n\n{}\n\n", tr("Foto"), tr("Apague esta linha e cole a foto aqui (Ctrl+V).")));
        }
        if let Some((pc, log)) = info {
            b.push_str(&format!("<details>\n<summary>{}</summary>\n\n```text\n{pc}", tr("Dados do PC e registro")));
            if !log.is_empty() {
                b.push_str(&format!("\n{}\n", log.join("\n")));
            }
            b.push_str("```\n\n</details>\n");
        }
        b
    }

    /// Monta o relatório. O registro antigo sai primeiro quando o endereço fica grande.
    pub fn prepare(&mut self, area: &str, pc: &str, log: &[String]) {
        let title = self.title(area);
        let info = |n: usize| self.extras.then_some((pc, &log[n.min(log.len())..]));
        let full = self.body(area, info(0), false);
        let address = |body: &str| format!("https://github.com/{REPO}/issues/new?title={}&body={}", encode(&title), encode(body));
        let mut url = address(&full);
        let mut short = false;
        let mut skip = 0;
        while too_long(&url) && self.extras && skip < log.len() {
            skip += 1;
            url = address(&self.body(area, info(skip), false));
        }
        if too_long(&url) {
            url = address(&self.body(area, None, false));
        }
        if too_long(&url) {
            url = address(&self.body(area, None, true));
            short = true;
        }
        self.ready = Some(Ready { title, full, url, short });
    }
}

/// Tamanho do endereço dentro do `return_to` do login do GitHub, que é o caso mais longo.
fn too_long(url: &str) -> bool {
    "https://github.com/login?return_to=".len() + encode(url).len() > URL_MAX
}

pub const LEVELS: [&str; 3] = ["Leve", "Moderado", "Grave"];

pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Troca `word` em qualquer caixa, letra a letra (letras raras mudam de bytes em minúscula).
/// `whole`: só onde não há letra nem número dos dois lados.
fn replace_any_case(text: &str, word: &str, by: &str, whole: bool) -> String {
    let w: Vec<char> = word.chars().collect();
    if w.is_empty() {
        return text.to_string();
    }
    let t: Vec<char> = text.chars().collect();
    let free = |k: usize| !whole || !t.get(k).is_some_and(|c| c.is_alphanumeric());
    let (mut out, mut k) = (String::new(), 0);
    while k < t.len() {
        let hit = k + w.len() <= t.len() && t[k..k + w.len()].iter().zip(&w).all(|(a, b)| a.to_lowercase().eq(b.to_lowercase())) && (k == 0 || free(k - 1)) && free(k + w.len());
        if hit {
            out.push_str(by);
            k += w.len();
        } else {
            out.push(t[k]);
            k += 1;
        }
    }
    out
}

/// Tira nome de usuário, nome do PC e caminhos de arquivo. Do caminho em diante, a linha sai.
pub fn scrub(line: &str) -> String {
    scrub_names(line, &[])
}

/// Como `scrub`, e troca também `names` (perfis e aparelhos que a pessoa nomeou) e o nome entre «»,
/// que vem de outro aparelho.
pub fn scrub_names(line: &str, names: &[String]) -> String {
    let env = [("USERNAME", "<user>"), ("COMPUTERNAME", "<pc>")].map(|(v, tag)| (std::env::var(v).unwrap_or_default(), tag));
    let mut s = line.to_string();
    for (name, tag) in env.iter().map(|(n, t)| (n.as_str(), *t)).chain(names.iter().map(|n| (n.as_str(), "<name>"))) {
        let name = name.trim();
        // Nome curto só como palavra inteira: "Al" não come pedaço de "Alt".
        s = replace_any_case(&s, name, tag, name.chars().count() < 3);
    }
    let mut parts = s.split('«');
    let mut q = parts.next().unwrap_or_default().to_string();
    for p in parts {
        match p.split_once('»') {
            Some((_, rest)) => q += &format!("«<name>»{rest}"),
            None => q += &format!("«{p}"),
        }
    }
    let mut s = q;
    let b = s.as_bytes();
    let path = (0..b.len()).find(|&i| {
        let drive = i + 2 < b.len() && b[i].is_ascii_alphabetic() && b[i + 1] == b':' && matches!(b[i + 2], b'\\' | b'/') && (i == 0 || !b[i - 1].is_ascii_alphanumeric());
        drive || (b[i] == b'\\' && b.get(i + 1) == Some(&b'\\'))
    });
    if let Some(i) = path {
        s.truncate(i);
        s.push_str("<path>");
    }
    s.chars().take(200).collect()
}

/// Nome e driver da primeira placa, que é a que desenha a tela principal.
fn gpu() -> Option<String> {
    unsafe {
        let f: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let a = f.EnumAdapters1(0).ok()?;
        let name = a.GetDesc1().ok()?.Description;
        let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
        let v = a.CheckInterfaceSupport(&IDXGIDevice::IID).ok()?;
        Some(format!("{} driver {}.{}.{}.{}", String::from_utf16_lossy(&name[..end]), v >> 48 & 0xffff, v >> 32 & 0xffff, v >> 16 & 0xffff, v & 0xffff))
    }
}

/// O que ajuda a repetir o bug: versões e aparelhos, sem nada que identifique a pessoa.
pub fn machine(snap: &Snapshot, scale: f32) -> String {
    let k = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
    let build: u32 = reg::text(Hive::Machine, k, "CurrentBuildNumber").and_then(|s| s.parse().ok()).unwrap_or(0);
    let release = reg::text(Hive::Machine, k, "DisplayVersion").unwrap_or_default();
    let cpu = reg::text(Hive::Machine, "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0", "ProcessorNameString").unwrap_or_default();
    let (w, h, screens) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN), GetSystemMetrics(SM_CMONITORS)) };
    let hz = crate::app::native_fps(capture::cursor_monitor());
    let hdr = match crate::sys::hdr::hdr_output() {
        Some((_, _, true)) => "on",
        Some(_) => "off",
        None => "no",
    };
    let mut out = format!(
        "Prisma {} ({})\nWindows {} {release} build {build}\nCPU: {}\nGPU: {}\nRAM: {} GB\nScreen: {w}x{h} {hz:.0} Hz, HDR {hdr}, scale {:.0}%, {screens} monitor(s)\nAdmin: {}\nLanguage: {}\n",
        update::current(),
        if cfg!(debug_assertions) { "debug" } else { "release" },
        if build >= 22000 { 11 } else { 10 },
        cpu.trim(),
        gpu().unwrap_or_else(|| snap.gpu_name.clone()),
        (snap.ram_total as f64 / (1u64 << 30) as f64).round(),
        scale * 100.0,
        if crate::startup::is_elevated() { "yes" } else { "no" },
        if crate::i18n::is_english() { "en" } else { "pt" },
    );
    out = out.lines().map(scrub).collect::<Vec<_>>().join("\n");
    out.push('\n');
    out
}

/// Só linhas do registro de ações que dizem algo do Prisma, nunca nomes de arquivos ou programas.
const SAFE_LINES: [&str; 10] = ["erro interno", "o Prisma tinha caído", "plano ", "aplicou ajustes", "desfez ajustes", "desligou a hibernação", "ligou a hibernação", "resetou", "refez", "medidor:"];

/// `names`: perfis e aparelhos que a pessoa nomeou, trocados por <name>.
pub fn recent_log(names: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(c) = crate::crash::last() {
        out.push(format!("Last crash: {c}"));
    }
    let file = std::fs::read_to_string(crate::config::data_dir().join("acoes.log")).unwrap_or_default();
    let mut kept: Vec<String> = file
        .lines()
        .filter(|l| l.split_once(' ').and_then(|r| r.1.split_once(' ')).is_some_and(|r| SAFE_LINES.iter().any(|s| r.1.starts_with(s))))
        .map(|l| scrub_names(l, names))
        .collect();
    let from = kept.len().saturating_sub(10);
    out.extend(kept.drain(from..));
    if let Ok(n) = NOTES.lock() {
        out.extend(n.iter().map(|l| scrub_names(l, names)));
    }
    out
}

pub fn open_url(url: &str) -> bool {
    #[cfg(debug_assertions)]
    if let Ok(path) = std::env::var("PRISMA_URL_OUT") {
        return std::fs::write(path, url).is_ok();
    }
    unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(url), None, None, SW_SHOWNORMAL).0 as isize > 32 }
}

fn json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' | '\t' => out.push(' '),
            c if (c as u32) < 32 => {}
            c => out.push(c),
        }
    }
    out
}

fn number(text: &str, key: &str) -> Option<u64> {
    let at = text.find(&format!("\"{key}\":"))? + key.len() + 3;
    text[at..].chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()
}

const FORM: &str = "Accept: application/json\r\nContent-Type: application/x-www-form-urlencoded\r\n";

/// Pede a licença da pessoa, cria a página e joga a licença fora. Nada fica guardado.
fn send_direct(state: &Mutex<Direct>, stop: &AtomicBool, title: &str, body: &str) -> Result<String, String> {
    let set = |d: Direct| *state.lock().unwrap() = d;
    let failed = || tr("Não consegui falar com o GitHub.").into_owned();
    let (_, reply) = update::post("https://github.com/login/device/code", FORM, &format!("client_id={CLIENT_ID}")).map_err(|_| failed())?;
    let (Some(device), Some(code), Some(page)) = (update::json_text(&reply, "device_code"), update::json_text(&reply, "user_code"), update::json_text(&reply, "verification_uri")) else {
        return Err(failed());
    };
    let mut wait = number(&reply, "interval").unwrap_or(5);
    let end = Instant::now() + Duration::from_secs(number(&reply, "expires_in").unwrap_or(900));
    set(Direct::Code(code));
    open_url(&page);
    let ask = format!("client_id={CLIENT_ID}&device_code={device}&grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code");
    let token = loop {
        for _ in 0..wait * 4 {
            if stop.load(Ordering::Relaxed) {
                return Err(String::new());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        if Instant::now() > end {
            return Err(tr("O código expirou. Tente de novo.").into_owned());
        }
        let Ok((_, reply)) = update::post("https://github.com/login/oauth/access_token", FORM, &ask) else { continue };
        if let Some(t) = update::json_text(&reply, "access_token") {
            break t;
        }
        match update::json_text(&reply, "error").as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => wait += 5,
            _ => return Err(tr("O GitHub recusou a autorização.").into_owned()),
        }
    };
    set(Direct::Sending);
    let head = format!("Accept: application/vnd.github+json\r\nContent-Type: application/json\r\nAuthorization: Bearer {token}\r\nX-GitHub-Api-Version: 2022-11-28\r\n");
    let data = format!("{{\"title\":\"{}\",\"body\":\"{}\"}}", json(title), json(body));
    let (status, reply) = update::post(&format!("https://api.github.com/repos/{REPO}/issues"), &head, &data).map_err(|_| failed())?;
    match update::json_text(&reply, "html_url") {
        Some(url) if status == 201 => Ok(url),
        _ => Err(format!("{} (HTTP {status})", failed())),
    }
}

pub fn start_direct(form: &Form, hub: isize, done: u32) {
    let Some(r) = &form.ready else { return };
    let (title, body) = (r.title.clone(), r.full.clone());
    let (state, stop) = (form.direct.clone(), form.stop.clone());
    stop.store(false, Ordering::Relaxed);
    *state.lock().unwrap() = Direct::Asking;
    std::thread::spawn(move || {
        let end = match send_direct(&state, &stop, &title, &body) {
            Ok(url) => {
                open_url(&url);
                Direct::Done
            }
            Err(e) if e.is_empty() => Direct::Idle,
            Err(e) => Direct::Failed(e),
        };
        *state.lock().unwrap() = end;
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(Some(HWND(hub as *mut _)), done, WPARAM(0), LPARAM(0));
        }
    });
}

#[cfg(debug_assertions)]
pub fn self_test(path: &str, snap: &Snapshot) {
    let user = std::env::var("USERNAME").unwrap_or_default();
    let mut out = String::new();
    for l in [format!("C:\\Users\\{user}\\Desktop\\x.txt falhou"), format!("oi {} tudo", user.to_uppercase()), "https://github.com/a/b ok".into(), "\\\\servidor\\pasta".into(), "12:34:56 normal".into()] {
        out.push_str(&format!("{}\n", scrub(&l)));
    }
    out.push_str(&format!("api sem licença: {:?}\n", update::post("https://api.github.com/repos/maliquem/Prisma/issues", "Accept: application/json\r\nContent-Type: application/json\r\n", "{\"title\":\"x\"}").map(|r| (r.0, r.1.chars().take(60).collect::<String>()))));
    out.push_str(&format!("código com id falso: {:?}\n", update::post("https://github.com/login/device/code", FORM, "client_id=Iv1.0000000000000000").map(|r| (r.0, r.1))));
    out.push_str(&format!("json: {}\n", json("a\"b\\c\nd")));
    out.push_str(&format!("{}\n---\n{:?}\n---\n", machine(snap, 2.0), recent_log(&[])));
    note("toast de teste");
    for (name, text) in [("curto", "O medidor sumiu"), ("longo", &"ação ".repeat(600)[..])] {
        for extras in [true, false] {
            let mut f = Form { draft: text.to_string(), extras, ..Form::default() };
            f.prepare("Medidor", &machine(snap, 2.0), &vec!["12:00:00 linha de registro".to_string(); 200]);
            let r = f.ready.unwrap();
            out.push_str(&format!("{name} extras={extras}: titulo {} url {} short {} full {}\n", r.title.len(), r.url.len(), r.short, r.full.len()));
        }
    }
    let _ = std::fs::write(path, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_short_words() {
        let names = vec!["Al".to_string(), "Perfil da Ana".to_string()];
        assert_eq!(scrub_names("Ctrl + Alt: al conectou", &names), "Ctrl + Alt: <name> conectou");
        assert_eq!(scrub_names("perfil da ana importado", &names), "<name> importado");
        assert_eq!(scrub_names("«PC-SALA» está pedindo", &names), "«<name>» está pedindo");
        assert_eq!(replace_any_case("İx MARIA", "maria", "<user>", false), "İx <user>");
    }

    #[test]
    fn url_limit_counts_login_wrap() {
        // 4000 caracteres de "%C3" viram ~6700 dentro do return_to: já não cabem.
        assert!(too_long(&"%C3".repeat(1334)));
        assert!(!too_long(&"%C3".repeat(1000)));
    }
}
