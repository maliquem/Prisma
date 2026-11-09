//! Atalhos da pesquisa: programas do menu Iniciar, perguntas na internet ou a uma IA e comandos
//! do PowerShell e do CMD, abertos sem herdar o administrador do Prisma quando não se pede.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::sys::catalog::StartApp;
use crate::sys::index::fold;

// ---------- Programas ----------

#[derive(Default)]
struct Apps {
    list: Vec<StartApp>,
    folded: Vec<Vec<u8>>,
    at: Option<Instant>,
    loading: bool,
}

static APPS: LazyLock<Mutex<Apps>> = LazyLock::new(Default::default);
static APPS_VER: AtomicU64 = AtomicU64::new(0);

/// O menu Iniciar é lido de novo quando a lista passa disso.
const FRESH: Duration = Duration::from_secs(300);

/// Muda sempre que a lista de programas muda.
pub fn apps_version() -> u64 {
    APPS_VER.load(Ordering::Relaxed)
}

fn wake(notify: isize) {
    unsafe {
        let h = Some(HWND(notify as *mut _));
        let _ = PostMessageW(h, crate::app::WM_FIND_DONE, WPARAM(0), LPARAM(0));
        let _ = PostMessageW(h, crate::app::WM_FIND_DONE, WPARAM(1), LPARAM(0));
    }
}

/// Lê o menu Iniciar em segundo plano, se a lista não for recente.
pub fn refresh_apps(notify: isize) {
    {
        let mut a = APPS.lock().unwrap();
        if a.loading || a.at.is_some_and(|t| t.elapsed() < FRESH) {
            return;
        }
        a.loading = true;
    }
    let _ = std::thread::Builder::new().name("menu-iniciar".into()).spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let mut list = crate::sys::catalog::start_apps();
        CoUninitialize();
        let mut seen = std::collections::HashSet::new();
        list.retain(|a| seen.insert(a.name.to_lowercase()));
        let folded = list
            .iter()
            .map(|a| {
                let mut v = Vec::new();
                fold(&a.name, &mut v);
                v
            })
            .collect();
        let mut a = APPS.lock().unwrap();
        a.list = list;
        a.folded = folded;
        a.at = Some(Instant::now());
        a.loading = false;
        drop(a);
        APPS_VER.fetch_add(1, Ordering::Relaxed);
        wake(notify);
    });
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Programas cujo nome tem todas as palavras digitadas. O nome exato vem primeiro, depois o que
/// começa assim, depois palavra que começa assim e por fim o que só contém.
pub fn find_apps(q: &str, max: usize) -> Vec<StartApp> {
    let mut fq = Vec::new();
    fold(q.trim(), &mut fq);
    if fq.len() < 2 {
        return Vec::new();
    }
    let words: Vec<&[u8]> = fq.split(|&b| b == b' ').filter(|w| !w.is_empty()).collect();
    let a = APPS.lock().unwrap();
    let mut hits: Vec<(u8, usize, usize)> = Vec::new();
    for (i, name) in a.folded.iter().enumerate() {
        let mut word_start = true;
        let mut ok = true;
        for w in &words {
            match find_sub(name, w) {
                None => {
                    ok = false;
                    break;
                }
                Some(p) => {
                    let starts = p == 0 || !name[p - 1].is_ascii_alphanumeric();
                    word_start &= starts;
                }
            }
        }
        if !ok {
            continue;
        }
        let rank = if *name == fq {
            0
        } else if name.starts_with(&fq) {
            1
        } else if word_start {
            2
        } else {
            3
        };
        hits.push((rank, name.len(), i));
    }
    hits.sort();
    hits.iter().take(max).map(|&(_, _, i)| a.list[i].clone()).collect()
}

/// Programa do menu Iniciar com este nome, sem diferença de maiúsculas.
fn installed(names: &[&str]) -> Option<StartApp> {
    let a = APPS.lock().unwrap();
    a.list.iter().find(|p| names.iter().any(|n| p.name.eq_ignore_ascii_case(n))).cloned()
}

/// Abre como o menu Iniciar abriria. Como administrador só programas comuns: os da Loja não
/// aceitam e abrem normalmente.
pub fn launch_app(a: &StartApp, admin: bool, elevated: bool) -> bool {
    let low = a.path.to_ascii_lowercase();
    if admin && low.ends_with(".exe") && !low.contains("\\windowsapps\\") {
        let dir = std::path::Path::new(&a.path).parent().map(|p| p.display().to_string()).unwrap_or_default();
        return if elevated { std::process::Command::new(&a.path).current_dir(&dir).spawn().is_ok() } else { run_admin(&a.path, "", &dir) };
    }
    std::process::Command::new(crate::sys::system_tool("explorer")).raw_arg(format!("\"shell:AppsFolder\\{}\"", a.id)).spawn().is_ok()
}

// ---------- Internet e IAs ----------

pub struct Engine {
    pub key: &'static str,
    pub name: &'static str,
    pub ai: bool,
    url: &'static str,
    /// O site aceita a pergunta no endereço.
    prefill: bool,
    /// Nomes do aplicativo no menu Iniciar.
    apps: &'static [&'static str],
}

pub const ENGINES: &[Engine] = &[
    Engine { key: "google", name: "Google", ai: false, url: "https://www.google.com/search?q=", prefill: true, apps: &[] },
    Engine { key: "bing", name: "Bing", ai: false, url: "https://www.bing.com/search?q=", prefill: true, apps: &[] },
    Engine { key: "duckduckgo", name: "DuckDuckGo", ai: false, url: "https://duckduckgo.com/?q=", prefill: true, apps: &[] },
    Engine { key: "brave", name: "Brave Search", ai: false, url: "https://search.brave.com/search?q=", prefill: true, apps: &[] },
    Engine { key: "claude", name: "Claude", ai: true, url: "https://claude.ai/new?q=", prefill: true, apps: &["Claude"] },
    Engine { key: "chatgpt", name: "ChatGPT", ai: true, url: "https://chatgpt.com/?q=", prefill: true, apps: &["ChatGPT"] },
    Engine { key: "gemini", name: "Gemini", ai: true, url: "https://gemini.google.com/app", prefill: false, apps: &["Gemini", "Google Gemini"] },
    Engine { key: "copilot", name: "Copilot", ai: true, url: "https://copilot.microsoft.com/?q=", prefill: true, apps: &["Copilot", "Microsoft Copilot"] },
    Engine { key: "perplexity", name: "Perplexity", ai: true, url: "https://www.perplexity.ai/search?q=", prefill: true, apps: &["Perplexity"] },
    Engine { key: "grok", name: "Grok", ai: true, url: "https://grok.com/?q=", prefill: true, apps: &["Grok"] },
    Engine { key: "deepseek", name: "DeepSeek", ai: true, url: "https://chat.deepseek.com/", prefill: false, apps: &["DeepSeek"] },
];

/// Aplicativo instalado deste buscador ou IA.
pub fn engine_app(i: usize) -> Option<StartApp> {
    let e = ENGINES.get(i)?;
    if e.apps.is_empty() {
        return None;
    }
    installed(e.apps)
}

/// Escolhidos nas opções. Sem escolha: o Google e as IAs instaladas.
pub fn chosen(cfg: &str) -> Vec<usize> {
    match cfg {
        "" => (0..ENGINES.len()).filter(|&i| i == 0 || (ENGINES[i].ai && engine_app(i).is_some())).collect(),
        "-" => Vec::new(),
        s => {
            let keys: Vec<&str> = s.split(',').map(str::trim).collect();
            (0..ENGINES.len()).filter(|&i| keys.contains(&ENGINES[i].key)).collect()
        }
    }
}

/// A pergunta não chega pronta e vai pela área de transferência.
pub fn needs_paste(i: usize) -> bool {
    engine_app(i).is_some() || !ENGINES[i].prefill
}

fn encode(q: &str) -> String {
    let mut out = String::new();
    for b in q.trim().bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Abre o buscador ou a IA. Devolve true quando a pergunta tem de ir copiada para colar.
pub fn ask(i: usize, q: &str) -> bool {
    let Some(e) = ENGINES.get(i) else { return false };
    if let Some(a) = engine_app(i) {
        launch_app(&a, false, false);
        return true;
    }
    let url = if e.prefill { format!("{}{}", e.url, encode(q)) } else { e.url.to_string() };
    crate::pages::search::open(&url);
    !e.prefill
}

/// Parece pergunta: termina com "?" ou começa com palavra de pergunta.
pub fn is_question(q: &str) -> bool {
    let t = q.trim();
    if t.ends_with('?') {
        return t.chars().count() > 3;
    }
    let mut f = Vec::new();
    fold(t, &mut f);
    let f = String::from_utf8_lossy(&f).to_string();
    let first = f.split_whitespace().next().unwrap_or_default();
    let words = f.split_whitespace().count();
    const ASK: &[&str] = &[
        "como", "qual", "quais", "quando", "onde", "quem", "quanto", "quantos", "quantas", "porque", "por", "o", "pra", "para", "what", "how", "why", "when", "where", "who", "which", "whats", "is", "are", "can", "does", "do", "should",
    ];
    words >= 3 && ASK.contains(&first) && (first != "o" || f.starts_with("o que ")) && (first != "por" || f.starts_with("por que ")) && (!matches!(first, "pra" | "para") || f.contains(" que "))
}

// ---------- Comandos ----------

/// PowerShell 7 se instalado, senão o do Windows.
pub fn powershell() -> PathBuf {
    let seven = crate::sys::known_folder(&windows::Win32::UI::Shell::FOLDERID_ProgramFiles).join("PowerShell\\7\\pwsh.exe");
    if seven.is_file() {
        seven
    } else {
        crate::sys::system_tool("powershell")
    }
}

fn utf16_b64(s: &str) -> String {
    let bytes: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    b64(&bytes)
}

pub(crate) fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            out.push(if k <= c.len() { T[(n >> (18 - 6 * k) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Pasta do usuário: o terminal abre nela, como o Terminal do Windows.
pub fn home() -> PathBuf {
    crate::sys::known_folder(&windows::Win32::UI::Shell::FOLDERID_Profile)
}

/// Roda o comando numa janela nova, que fica aberta. Normal roda sem administrador mesmo com o
/// Prisma como administrador.
pub fn run_command(ps: bool, cmd: &str, admin: bool, elevated: bool) -> bool {
    let (exe, args) = if ps {
        let script = format!("Write-Host -ForegroundColor DarkGray {}\n{}", ps_quote(&format!("> {cmd}")), cmd);
        (powershell(), format!("-NoLogo -NoExit -EncodedCommand {}", utf16_b64(&script)))
    } else {
        (crate::sys::system_tool("cmd"), format!("/s /k \"{cmd}\""))
    };
    let dir = home().display().to_string();
    let file = exe.display().to_string();
    if admin && !elevated {
        return run_admin(&file, &args, &dir);
    }
    if !admin && elevated {
        return run_as_user(&file, &args, &dir);
    }
    std::process::Command::new(&exe).raw_arg(&args).current_dir(&dir).creation_flags(CREATE_NEW_CONSOLE.0).spawn().is_ok()
}

/// Pede o administrador ao Windows. Se a pessoa recusar, nada abre.
fn run_admin(file: &str, args: &str, dir: &str) -> bool {
    use windows::core::w;
    use windows::Win32::UI::Shell::*;
    let (f, a, d) = (HSTRING::from(file), HSTRING::from(args), HSTRING::from(dir));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(f.as_ptr()),
        lpParameters: if args.is_empty() { PCWSTR::null() } else { PCWSTR(a.as_ptr()) },
        lpDirectory: PCWSTR(d.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info).is_ok() }
}

/// Com o Prisma como administrador: abre com a permissão do Explorer, a da pessoa.
fn run_as_user(file: &str, args: &str, dir: &str) -> bool {
    let Some(token) = crate::sys::procs::shell_pid().and_then(crate::sys::procs::shell_token) else { return false };
    let mut line: Vec<u16> = format!("\"{file}\" {args}").encode_utf16().chain([0]).collect();
    let app = HSTRING::from(file);
    let cwd = HSTRING::from(dir);
    let si = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    let mut pi = PROCESS_INFORMATION::default();
    unsafe {
        let ok = CreateProcessWithTokenW(token, LOGON_WITH_PROFILE, &app, Some(PWSTR(line.as_mut_ptr())), CREATE_NEW_CONSOLE | CREATE_UNICODE_ENVIRONMENT, None, &cwd, &si, &mut pi).is_ok();
        let _ = CloseHandle(token);
        if ok {
            let _ = CloseHandle(pi.hThread);
            let _ = CloseHandle(pi.hProcess);
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions() {
        assert!(is_question("Como rodar Phantom Blade Zero melhor no PC?"));
        assert!(is_question("como rodar phantom blade zero"));
        assert!(is_question("what is a pagefile"));
        assert!(is_question("o que é DLSS"));
        assert!(!is_question("prisma"));
        assert!(!is_question("relatorio 2024 final"));
        assert!(!is_question("o relatorio final"));
        assert!(!is_question("a?"));
    }

    #[test]
    fn encodes() {
        assert_eq!(encode("a b?"), "a%20b%3F");
        assert_eq!(encode("ção"), "%C3%A7%C3%A3o");
        assert_eq!(b64(b"ab"), "YWI=");
        assert_eq!(b64(b"abc"), "YWJj");
        assert_eq!(b64(b"a"), "YQ==");
    }
}
