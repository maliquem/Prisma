//! Serviço opcional, rodando como SYSTEM, para compartilhar também a tela segura do Windows: o aviso
//! do UAC, a tela de bloqueio e a do PIN (inclusive depois de o PC reiniciar sozinho), e o
//! Ctrl+Alt+Del. Só se instala com o consentimento dela, como administradora.
//!
//! - `--stream-service-install`: pergunta, copia o Prisma para `Arquivos de Programas\PrismaServico`
//!   (pasta que só administrador altera), leva a identidade e os aparelhos pareados para
//!   `ProgramData\Prisma\compartilhar` (só SYSTEM, administradores e ela) e liga o serviço.
//! - `--stream-service-remove`: desliga, apaga o serviço e as duas pastas, devolve os aparelhos
//!   pareados e a política do Ctrl+Alt+Del como estavam.
//! - `--stream-service-status`: mostra se está instalado e rodando.
//! - `--stream-invite[=permissões]`: cria um convite (código curto) para o host que estiver rodando,
//!   o do serviço ou o da página.
//!
//! Como funciona: o serviço (sessão 0, sem tela) mantém um ajudante na sessão de quem está no PC,
//! com o mesmo poder de SYSTEM. O ajudante é o host de sempre, capturando pela Desktop Duplication,
//! que segue a tela segura. Se o ajudante cair, o serviço abre outro; se trocar a sessão do console,
//! o ajudante muda junto. O Ctrl+Alt+Del sai do próprio serviço (`SendSAS`), a pedido do ajudante.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Security::Authorization::*;
use windows::Win32::Security::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Registry::*;
use windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId;
use windows::Win32::System::Services::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const NAME: PCWSTR = w!("PrismaCompartilhar");
const DISPLAY: PCWSTR = w!("Prisma: compartilhar tela");
const DESCRIPTION: &str = "Deixa os aparelhos pareados com o Prisma verem e usarem a tela segura do Windows (UAC, bloqueio, PIN). Instalado a pedido; para tirar: prisma.exe --stream-service-remove.";
const STOP_EVENT: PCWSTR = w!("Global\\PrismaCompartilharParar");
const SAS_EVENT: PCWSTR = w!("Global\\PrismaCompartilharSAS");
const POLICY_KEY: PCWSTR = w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\System");
const POLICY_VALUE: PCWSTR = w!("SoftwareSASGeneration");

pub fn run_args(a: &str) -> bool {
    match a {
        "--stream-service" => scm_main(),
        "--stream-service-helper" => helper(),
        "--stream-service-install" => install(),
        "--stream-service-remove" => remove(),
        "--stream-service-status" => status(),
        _ => {
            if a == "--stream-invite" || a.starts_with("--stream-invite=") {
                let v = a.strip_prefix("--stream-invite=").unwrap_or("ver");
                match super::parse_perms(v) {
                    Some(p) => super::net::make_invite(p),
                    None => eprintln!("permissão desconhecida: «{v}»"),
                }
                return true;
            }
            return false;
        }
    }
    true
}

/// Onde o serviço guarda a identidade, os aparelhos e o log.
pub fn data_dir() -> PathBuf {
    super::service_dir()
}

/// Onde fica a cópia do Prisma que o serviço roda.
fn program_dir() -> PathBuf {
    let base = crate::sys::known_folder(&windows::Win32::UI::Shell::FOLDERID_ProgramFiles);
    let base = if base.as_os_str().is_empty() { PathBuf::from("C:\\Program Files") } else { base };
    base.join("PrismaServico")
}

fn marker() -> PathBuf {
    data_dir().join("servico.tsv")
}

/// O serviço está instalado: a identidade e os aparelhos ficam na pasta dele.
pub fn installed() -> bool {
    marker().exists()
}

fn log(line: &str) {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(data_dir().join("servico.log")) {
        let _ = writeln!(f, "{:04}-{:02}-{:02} {:02}:{:02}:{:02} serviço: {line}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond);
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// O resultado também numa janela: pela página do Prisma, ninguém lê o terminal.
fn tell(text: &str, ok: bool) {
    eprintln!("{text}");
    let icon = if ok { MB_ICONINFORMATION } else { MB_ICONWARNING };
    unsafe {
        MessageBoxW(None, &HSTRING::from(lines_tr(text)), w!("Prisma"), MB_OK | icon | MB_TOPMOST | MB_SETFOREGROUND);
    }
}

/// Traduz linha por linha (a tabela de tradução tem uma linha por texto).
fn lines_tr(text: &str) -> String {
    text.split('\n').map(|l| crate::i18n::tr(l).into_owned()).collect::<Vec<_>>().join("\n")
}

fn ask(text: &str, flags: MESSAGEBOX_STYLE) -> bool {
    unsafe { MessageBoxW(None, &HSTRING::from(lines_tr(text)), w!("Prisma: serviço de compartilhar tela"), flags | MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2 | MB_TOPMOST | MB_SETFOREGROUND) == IDYES }
}

// ---------- Instalar e remover ----------

/// SID de quem roda este processo, em texto.
unsafe fn my_sid() -> Option<String> {
    let mut tok = HANDLE::default();
    OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok).ok()?;
    let mut need = 0u32;
    let _ = GetTokenInformation(tok, TokenUser, None, 0, &mut need);
    let mut buf = vec![0u8; need as usize];
    let r = GetTokenInformation(tok, TokenUser, Some(buf.as_mut_ptr() as *mut _), need, &mut need);
    let _ = CloseHandle(tok);
    r.ok()?;
    let user = &*(buf.as_ptr() as *const TOKEN_USER);
    let mut s = PWSTR::null();
    ConvertSidToStringSidW(user.User.Sid, &mut s).ok()?;
    let out = s.to_string().ok();
    let _ = LocalFree(Some(HLOCAL(s.0 as *mut _)));
    out
}

/// Pasta só para SYSTEM, administradores e ela (alterar, sem mudar permissões), sem herdar de cima.
unsafe fn protect_dir(dir: &std::path::Path, sid: &str) -> std::result::Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let sddl = format!("D:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1301bf;;;{sid})");
    let mut sd = PSECURITY_DESCRIPTOR::default();
    ConvertStringSecurityDescriptorToSecurityDescriptorW(&HSTRING::from(sddl), SDDL_REVISION_1, &mut sd, None).map_err(|e| e.message())?;
    let path = HSTRING::from(dir.as_os_str());
    let ok = SetFileSecurityW(&path, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, sd).as_bool();
    let _ = LocalFree(Some(HLOCAL(sd.0)));
    if ok {
        Ok(())
    } else {
        Err(Error::from_thread().message())
    }
}

/// A política que deixa um serviço gerar o Ctrl+Alt+Del: valor atual, ou None se não existe.
unsafe fn sas_policy() -> Option<u32> {
    let mut v = 0u32;
    let mut size = 4u32;
    RegGetValueW(HKEY_LOCAL_MACHINE, POLICY_KEY, POLICY_VALUE, RRF_RT_REG_DWORD, None, Some(&mut v as *mut _ as *mut _), Some(&mut size)).ok().ok()?;
    Some(v)
}

unsafe fn set_sas_policy(v: Option<u32>) -> bool {
    let mut key = HKEY::default();
    if RegCreateKeyExW(HKEY_LOCAL_MACHINE, POLICY_KEY, None, None, REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, None, &mut key, None).is_err() {
        return false;
    }
    let ok = match v {
        Some(v) => RegSetValueExW(key, POLICY_VALUE, None, REG_DWORD, Some(&v.to_le_bytes())).is_ok(),
        None => {
            let r = RegDeleteValueW(key, POLICY_VALUE);
            r.is_ok() || r == ERROR_FILE_NOT_FOUND
        }
    };
    let _ = RegCloseKey(key);
    ok
}

/// Para o serviço e espera até 15 s.
unsafe fn stop_service(svc: SC_HANDLE) {
    let mut st = SERVICE_STATUS::default();
    let _ = ControlService(svc, SERVICE_CONTROL_STOP, &mut st);
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(15) {
        if QueryServiceStatus(svc, &mut st).is_err() || st.dwCurrentState == SERVICE_STOPPED {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn install() {
    if !crate::startup::is_elevated() {
        return eprintln!("instalar o serviço pede administrador: abra o terminal como administrador e rode de novo");
    }
    let text = "Instalar o serviço de compartilhar tela do Prisma?\n\n\
O que ele faz: roda como SYSTEM (o nível mais alto do Windows) e começa junto com o Windows. Com ele, os aparelhos pareados conseguem ver e usar o aviso do UAC, a tela de bloqueio e a do PIN, inclusive depois de o PC reiniciar sozinho. Mexer na tela segura e mandar Ctrl+Alt+Del pede a permissão \"Tela segura\" (ou Acesso total) no aparelho.\n\n\
O que muda no PC:\n\
• uma cópia do Prisma em Arquivos de Programas\\PrismaServico;\n\
• a identidade e os aparelhos pareados passam para ProgramData\\Prisma\\compartilhar, que só o SYSTEM, os administradores e você abrem;\n\
• o serviço \"Prisma: compartilhar tela\".\n\n\
Para tirar tudo: Remover, na página Compartilhar tela do Prisma.";
    if !ask(text, MESSAGEBOX_STYLE(0)) {
        return eprintln!("instalação cancelada");
    }
    let before = unsafe { sas_policy() };
    let want_sas = before.is_some_and(|v| v & 1 != 0)
        || ask(
            "Permitir o Ctrl+Alt+Del de longe?\n\n\
Para isso, o Windows pede uma política: \"SoftwareSASGeneration\" igual a 1, que deixa serviços gerarem o Ctrl+Alt+Del. Ela é desfeita quando o serviço for removido.\n\n\
Sem ela, o resto funciona; só o Ctrl+Alt+Del não.",
            MESSAGEBOX_STYLE(0),
        );
    match unsafe { do_install(before, want_sas) } {
        Ok(()) => {
            eprintln!("  convite para parear um aparelho novo: --stream-invite=permissões (ex.: --stream-invite=total)");
            eprintln!("  log: {}", data_dir().join("servico.log").display());
            eprintln!("  para tirar: --stream-service-remove");
            tell("Serviço instalado e ligado. Para parear um aparelho novo, use Criar convite na página Compartilhar tela.", true);
        }
        Err(e) => tell(&format!("O serviço não foi instalado: {e}"), false),
    }
}

unsafe fn do_install(before: Option<u32>, want_sas: bool) -> std::result::Result<(), String> {
    let scm = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS).map_err(|e| format!("gerenciador de serviços: {}", e.message()))?;
    // Reinstalar: para o que está rodando antes de trocar o programa.
    if let Ok(old) = OpenServiceW(scm, NAME, SERVICE_ALL_ACCESS) {
        stop_service(old);
        let _ = CloseServiceHandle(old);
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dst = program_dir().join("prisma.exe");
    if exe != dst {
        std::fs::create_dir_all(program_dir()).map_err(|e| e.to_string())?;
        std::fs::copy(&exe, &dst).map_err(|e| format!("copiar para {}: {e}", dst.display()))?;
    }
    let sid = my_sid().ok_or("não deu para ler o usuário")?;
    protect_dir(&data_dir(), &sid)?;
    super::net::copy_to_service(&data_dir())?;
    let mut m = format!("usuario\t{sid}\n");
    m += &format!("politica_antes\t{}\n", before.map_or("-".to_string(), |v| v.to_string()));
    m += &format!("politica_mudou\t{}\n", (want_sas && before.is_none_or(|v| v & 1 == 0)) as u8);
    std::fs::write(marker(), m).map_err(|e| e.to_string())?;
    if want_sas && before.is_none_or(|v| v & 1 == 0) && !set_sas_policy(Some(before.unwrap_or(0) | 1)) {
        eprintln!("aviso: a política do Ctrl+Alt+Del não mudou; o resto segue");
    }
    // Caminho entre aspas: um caminho com espaço sem aspas é a falha clássica de serviço.
    let cmd = format!("\"{}\" --stream-service", dst.display());
    let svc = match CreateServiceW(scm, NAME, DISPLAY, SERVICE_ALL_ACCESS, SERVICE_WIN32_OWN_PROCESS, SERVICE_AUTO_START, SERVICE_ERROR_NORMAL, &HSTRING::from(&cmd), None, None, None, None, None) {
        Ok(s) => s,
        Err(e) if e.code() == ERROR_SERVICE_EXISTS.to_hresult() => {
            let s = OpenServiceW(scm, NAME, SERVICE_ALL_ACCESS).map_err(|e| e.message())?;
            ChangeServiceConfigW(s, SERVICE_WIN32_OWN_PROCESS, SERVICE_AUTO_START, SERVICE_ERROR_NORMAL, &HSTRING::from(&cmd), None, None, None, None, None, DISPLAY).map_err(|e| e.message())?;
            s
        }
        Err(e) => return Err(format!("criar o serviço: {}", e.message())),
    };
    let mut desc = wide(DESCRIPTION);
    let d = SERVICE_DESCRIPTIONW { lpDescription: PWSTR(desc.as_mut_ptr()) };
    let _ = ChangeServiceConfig2W(svc, SERVICE_CONFIG_DESCRIPTION, Some(&d as *const _ as *const _));
    // Se cair, o Windows abre de novo depois de 5 s (até três vezes por dia).
    let mut acts = [SC_ACTION { Type: SC_ACTION_RESTART, Delay: 5000 }; 3];
    let fa = SERVICE_FAILURE_ACTIONSW { dwResetPeriod: 86_400, cActions: 3, lpsaActions: acts.as_mut_ptr(), ..Default::default() };
    let _ = ChangeServiceConfig2W(svc, SERVICE_CONFIG_FAILURE_ACTIONS, Some(&fa as *const _ as *const _));
    let started = StartServiceW(svc, None);
    let _ = CloseServiceHandle(svc);
    let _ = CloseServiceHandle(scm);
    started.map_err(|e| format!("ligar o serviço: {}", e.message()))
}

fn remove() {
    if !crate::startup::is_elevated() {
        return eprintln!("remover o serviço pede administrador: abra o terminal como administrador e rode de novo");
    }
    let exe = std::env::current_exe().unwrap_or_default();
    if exe.starts_with(program_dir()) {
        return eprintln!("rode a remoção pelo Prisma de sempre, não pela cópia do serviço (ela não se apaga sozinha)");
    }
    if !ask("Remover o serviço de compartilhar tela do Prisma?\n\nOs aparelhos pareados voltam para a pasta de sempre do Prisma, e a política do Ctrl+Alt+Del volta como estava.", MESSAGEBOX_STYLE(0)) {
        return eprintln!("remoção cancelada");
    }
    unsafe {
        if let Ok(scm) = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS) {
            if let Ok(svc) = OpenServiceW(scm, NAME, SERVICE_ALL_ACCESS) {
                stop_service(svc);
                match DeleteService(svc) {
                    Ok(()) => eprintln!("serviço apagado"),
                    Err(e) => eprintln!("não apagou o serviço: {}", e.message()),
                }
                let _ = CloseServiceHandle(svc);
            }
            let _ = CloseServiceHandle(scm);
        }
        let m = std::fs::read_to_string(marker()).unwrap_or_default();
        let field = |k: &str| m.lines().find_map(|l| l.strip_prefix(k).and_then(|r| r.strip_prefix('\t'))).map(str::to_string);
        if field("politica_mudou").as_deref() == Some("1") {
            let back = field("politica_antes").and_then(|v| v.parse().ok());
            if set_sas_policy(back) {
                eprintln!("política do Ctrl+Alt+Del de volta como estava");
            }
        }
    }
    if let Err(e) = super::net::copy_from_service(&data_dir()) {
        eprintln!("aviso: os aparelhos pareados não voltaram ({e}); eles ficam em {}", data_dir().display());
        return;
    }
    for dir in [data_dir(), program_dir()] {
        // O programa do serviço pode levar um instante para soltar o arquivo.
        let mut gone = false;
        for _ in 0..20 {
            if !dir.exists() || std::fs::remove_dir_all(&dir).is_ok() {
                gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        if !gone {
            eprintln!("aviso: não deu para apagar {}", dir.display());
        }
    }
    tell("Serviço removido.", true);
}

fn status() {
    unsafe {
        let state = OpenSCManagerW(None, None, SC_MANAGER_CONNECT).ok().and_then(|scm| {
            let r = OpenServiceW(scm, NAME, SERVICE_QUERY_STATUS).ok().map(|svc| {
                let mut st = SERVICE_STATUS::default();
                let _ = QueryServiceStatus(svc, &mut st);
                let _ = CloseServiceHandle(svc);
                st.dwCurrentState
            });
            let _ = CloseServiceHandle(scm);
            r
        });
        match state {
            None => eprintln!("serviço: não instalado"),
            Some(s) => eprintln!(
                "serviço: {}",
                match s {
                    SERVICE_RUNNING => "rodando",
                    SERVICE_STOPPED => "parado",
                    SERVICE_START_PENDING => "ligando",
                    SERVICE_STOP_PENDING => "parando",
                    _ => "outro estado",
                }
            ),
        }
        eprintln!("pasta dos dados: {}{}", data_dir().display(), if installed() { "" } else { " (não existe)" });
        eprintln!(
            "Ctrl+Alt+Del de longe: {}",
            match sas_policy() {
                Some(v) if v & 1 != 0 => "permitido pela política",
                _ => "a política não deixa",
            }
        );
    }
}

// ---------- O serviço ----------

static mut STATUS: SERVICE_STATUS_HANDLE = SERVICE_STATUS_HANDLE(std::ptr::null_mut());
static STOP: std::sync::OnceLock<isize> = std::sync::OnceLock::new();
static SESSION: std::sync::OnceLock<isize> = std::sync::OnceLock::new();

unsafe fn report(state: SERVICE_STATUS_CURRENT_STATE, exit: u32) {
    let st = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING { SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN | SERVICE_ACCEPT_SESSIONCHANGE } else { 0 },
        dwWin32ExitCode: exit,
        dwWaitHint: 10_000,
        ..Default::default()
    };
    let _ = SetServiceStatus(*std::ptr::addr_of!(STATUS), &st);
}

/// O ajudante do serviço caiu da última vez que abriu (pelo fim do `servico.log`).
pub fn failing() -> bool {
    use std::io::{Read, Seek, SeekFrom};
    thread_local! {
        static LAST: std::cell::Cell<Option<(Instant, bool)>> = const { std::cell::Cell::new(None) };
    }
    if let Some((t, v)) = LAST.get() {
        if t.elapsed() < Duration::from_secs(2) {
            return v;
        }
    }
    let read = || -> Option<bool> {
        let mut f = std::fs::File::open(data_dir().join("servico.log")).ok()?;
        let len = f.metadata().ok()?.len();
        f.seek(SeekFrom::Start(len.saturating_sub(4096))).ok()?;
        let mut b = Vec::new();
        f.read_to_end(&mut b).ok()?;
        let text = String::from_utf8_lossy(&b);
        let last = text.lines().rev().find(|l| l.contains(" serviço: "))?;
        Some(last.contains("o ajudante saiu"))
    };
    let v = read().unwrap_or(false);
    LAST.set(Some((Instant::now(), v)));
    v
}

/// A cópia do serviço é da versão anterior: troca pela deste Prisma.
///
/// Atualizar o Prisma troca só o programa instalado; a cópia em `PrismaServico` ficava na versão
/// velha, e foi assim que a 0.12.0 continuou com o host da 0.11.0, que caía. Só a cópia instalada
/// faz a troca (uma cópia portátil, talvez mais velha, nunca rebaixa o serviço), e só como
/// administrador. Devolve o que aconteceu, para o log.
pub fn refresh_copy() -> Option<String> {
    if !installed() || !crate::startup::is_installed() {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let dst = program_dir().join("prisma.exe");
    if exe == dst {
        return None;
    }
    let mine = std::fs::read(&exe).ok()?;
    if std::fs::read(&dst).is_ok_and(|theirs| theirs == mine) {
        return None;
    }
    if !crate::startup::is_elevated() {
        return Some("a cópia do serviço é de outra versão, e trocar pede administrador".into());
    }
    let r = unsafe { swap_copy(&dst, &mine) };
    let line = match &r {
        Ok(()) => format!("programa trocado pela versão {}", env!("CARGO_PKG_VERSION")),
        Err(e) => format!("não deu para trocar o programa pela versão {}: {e}", env!("CARGO_PKG_VERSION")),
    };
    log(&line);
    Some(line)
}

unsafe fn swap_copy(dst: &std::path::Path, bytes: &[u8]) -> std::result::Result<(), String> {
    let scm = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS).map_err(|e| format!("gerenciador de serviços: {}", e.message()))?;
    let svc = OpenServiceW(scm, NAME, SERVICE_ALL_ACCESS);
    if let Ok(s) = svc {
        stop_service(s);
    }
    // O ajudante sai logo depois do serviço; enquanto isso, o arquivo pode estar preso.
    let t = Instant::now();
    let copied = loop {
        match std::fs::write(dst, bytes) {
            Ok(()) => break Ok(()),
            Err(e) if t.elapsed() > Duration::from_secs(15) => break Err(format!("copiar para {}: {e}", dst.display())),
            Err(_) => std::thread::sleep(Duration::from_millis(300)),
        }
    };
    // Liga de novo mesmo se a cópia falhou: o serviço volta como estava.
    if let Ok(s) = svc {
        let _ = StartServiceW(s, None);
        let _ = CloseServiceHandle(s);
    }
    let _ = CloseServiceHandle(scm);
    copied
}

fn scm_main() {
    unsafe {
        let mut name = wide("PrismaCompartilhar");
        let table = [SERVICE_TABLE_ENTRYW { lpServiceName: PWSTR(name.as_mut_ptr()), lpServiceProc: Some(service_main) }, SERVICE_TABLE_ENTRYW::default()];
        if StartServiceCtrlDispatcherW(table.as_ptr()).is_err() {
            eprintln!("isto é o serviço do Prisma: ele é aberto pelo Windows (instale com --stream-service-install)");
        }
    }
}

unsafe extern "system" fn handler(ctrl: u32, _ev: u32, _data: *mut core::ffi::c_void, _ctx: *mut core::ffi::c_void) -> u32 {
    match ctrl {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            report(SERVICE_STOP_PENDING, 0);
            if let Some(&e) = STOP.get() {
                let _ = SetEvent(HANDLE(e as *mut _));
            }
        }
        SERVICE_CONTROL_SESSIONCHANGE => {
            if let Some(&e) = SESSION.get() {
                let _ = SetEvent(HANDLE(e as *mut _));
            }
        }
        _ => {}
    }
    NO_ERROR.0
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
    let Ok(h) = RegisterServiceCtrlHandlerExW(NAME, Some(handler), None) else { return };
    *std::ptr::addr_of_mut!(STATUS) = h;
    let stop = CreateEventW(None, true, false, None).unwrap_or_default();
    let session = CreateEventW(None, false, false, None).unwrap_or_default();
    let _ = STOP.set(stop.0 as isize);
    let _ = SESSION.set(session.0 as isize);
    // Os eventos com nome que o ajudante abre: o padrão de um processo SYSTEM só deixa SYSTEM e
    // administradores mexerem.
    let helper_stop = CreateEventW(None, true, false, STOP_EVENT).unwrap_or_default();
    let sas = CreateEventW(None, false, false, SAS_EVENT).unwrap_or_default();
    report(SERVICE_RUNNING, 0);
    log("ligado");
    supervise(stop, session, helper_stop, sas);
    log("desligado");
    for h in [stop, session, helper_stop, sas] {
        let _ = CloseHandle(h);
    }
    report(SERVICE_STOPPED, 0);
}

/// O ajudante rodando: processo e sessão.
struct Child {
    process: HANDLE,
    session: u32,
    started: Instant,
}

/// Mantém um ajudante na sessão do console, até mandarem parar.
unsafe fn supervise(stop: HANDLE, session: HANDLE, helper_stop: HANDLE, sas: HANDLE) {
    let mut child: Option<Child> = None;
    let mut wait_until = Instant::now();
    let mut backoff = Duration::from_secs(2);
    loop {
        let active = WTSGetActiveConsoleSessionId();
        if let Some(c) = &child {
            if c.session != active {
                log(&format!("a sessão do console mudou ({} para {active}): ajudante novo", c.session));
                end_child(child.take().unwrap(), helper_stop);
            }
        }
        if child.is_none() && active != u32::MAX && Instant::now() >= wait_until {
            match launch(active) {
                Ok(p) => {
                    log(&format!("ajudante aberto na sessão {active}"));
                    child = Some(Child { process: p, session: active, started: Instant::now() });
                }
                Err(e) => {
                    log(&format!("o ajudante não abriu: {e}"));
                    wait_until = Instant::now() + backoff;
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
            }
        }
        let mut handles = vec![stop, session, sas];
        if let Some(c) = &child {
            handles.push(c.process);
        }
        let r = WaitForMultipleObjects(&handles, false, 1000);
        match r.0 {
            0 => break,
            1 => {}
            2 => send_sas(),
            3 => {
                let c = child.take().unwrap();
                let mut code = 0u32;
                let _ = GetExitCodeProcess(c.process, &mut code);
                let _ = CloseHandle(c.process);
                // Caiu logo: espera mais antes de tentar de novo.
                backoff = if c.started.elapsed() < Duration::from_secs(30) { (backoff * 2).min(Duration::from_secs(60)) } else { Duration::from_secs(2) };
                log(&format!("o ajudante saiu (código {code:#x}); outro em {} s", backoff.as_secs()));
                wait_until = Instant::now() + backoff;
            }
            _ => {}
        }
    }
    if let Some(c) = child {
        end_child(c, helper_stop);
    }
}

/// Pede para o ajudante sair e espera; se não sair em 5 s, encerra (é o processo que o serviço abriu).
unsafe fn end_child(c: Child, helper_stop: HANDLE) {
    let _ = SetEvent(helper_stop);
    if WaitForSingleObject(c.process, 5000) != WAIT_OBJECT_0 {
        let _ = TerminateProcess(c.process, 1);
        let _ = WaitForSingleObject(c.process, 2000);
    }
    let _ = ResetEvent(helper_stop);
    let _ = CloseHandle(c.process);
}

/// Abre o ajudante na sessão, com o poder de SYSTEM deste serviço, e o log como saída.
unsafe fn launch(session: u32) -> std::result::Result<HANDLE, String> {
    let mut tok = HANDLE::default();
    OpenProcessToken(GetCurrentProcess(), TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT | TOKEN_ADJUST_SESSIONID, &mut tok).map_err(|e| e.message())?;
    let mut dup = HANDLE::default();
    let r = DuplicateTokenEx(tok, TOKEN_ALL_ACCESS, None, SecurityImpersonation, TokenPrimary, &mut dup);
    let _ = CloseHandle(tok);
    r.map_err(|e| e.message())?;
    let r = SetTokenInformation(dup, TokenSessionId, &session as *const u32 as *const _, 4);
    if let Err(e) = r {
        let _ = CloseHandle(dup);
        return Err(e.message());
    }
    // Log novo quando passa de 5 MB.
    let log_path = data_dir().join("servico.log");
    if std::fs::metadata(&log_path).is_ok_and(|m| m.len() > 5 << 20) {
        let _ = std::fs::rename(&log_path, log_path.with_extension("log.old"));
    }
    let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, bInheritHandle: true.into(), ..Default::default() };
    let out = CreateFileW(&HSTRING::from(log_path.as_os_str()), FILE_APPEND_DATA.0, FILE_SHARE_READ | FILE_SHARE_WRITE, Some(&sa), OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, None).ok();
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = wide(&format!("\"{}\" --stream-service-helper", exe.display()));
    let mut desk = wide("winsta0\\default");
    let si = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        lpDesktop: PWSTR(desk.as_mut_ptr()),
        dwFlags: if out.is_some() { STARTF_USESTDHANDLES } else { STARTUPINFOW_FLAGS(0) },
        hStdOutput: out.unwrap_or_default(),
        hStdError: out.unwrap_or_default(),
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    let r = CreateProcessAsUserW(Some(dup), &HSTRING::from(exe.as_os_str()), Some(PWSTR(cmd.as_mut_ptr())), None, None, out.is_some(), CREATE_NO_WINDOW, None, None, &si, &mut pi);
    let _ = CloseHandle(dup);
    if let Some(h) = out {
        let _ = CloseHandle(h);
    }
    r.map_err(|e| e.message())?;
    let _ = CloseHandle(pi.hThread);
    Ok(pi.hProcess)
}

/// Ctrl+Alt+Del: só um serviço consegue, e só com a política ligada.
unsafe fn send_sas() {
    let Ok(m) = LoadLibraryW(w!("sas.dll")) else {
        return log("Ctrl+Alt+Del: sem o sas.dll");
    };
    let Some(f) = GetProcAddress(m, s!("SendSAS")) else {
        return log("Ctrl+Alt+Del: sem o SendSAS");
    };
    let f: unsafe extern "system" fn(BOOL) = std::mem::transmute(f);
    f(false.into());
    log(if sas_policy().is_some_and(|v| v & 1 != 0) { "Ctrl+Alt+Del enviado" } else { "Ctrl+Alt+Del pedido, mas a política não deixa (o Windows ignora)" });
}

// ---------- O ajudante ----------

/// O host de sempre, como SYSTEM na sessão de quem usa o PC. Sai quando o serviço pede.
fn helper() {
    super::SERVICE.store(true, Ordering::Relaxed);
    eprintln!("ajudante do serviço: processo {}", std::process::id());
    std::thread::spawn(|| unsafe {
        let Ok(ev) = OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, STOP_EVENT) else { return };
        let _ = WaitForSingleObject(ev, INFINITE);
        eprintln!("ajudante: o serviço pediu para sair");
        std::process::exit(0);
    });
    super::net::host_service();
}

/// O ajudante pede o Ctrl+Alt+Del ao serviço.
pub fn request_sas(who: &str) {
    if !super::SERVICE.load(Ordering::Relaxed) {
        return eprintln!("Ctrl+Alt+Del de «{who}»: só funciona com o serviço instalado");
    }
    // O visualizador manda o pedido repetido: um por segundo basta.
    static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    {
        let mut last = LAST.lock().unwrap();
        if last.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(1)) {
            return;
        }
        *last = Some(std::time::Instant::now());
    }
    unsafe {
        match OpenEventW(EVENT_MODIFY_STATE, false, SAS_EVENT) {
            Ok(ev) => {
                eprintln!("Ctrl+Alt+Del pedido por «{who}»");
                let _ = SetEvent(ev);
                let _ = CloseHandle(ev);
            }
            Err(e) => eprintln!("Ctrl+Alt+Del: o serviço não respondeu ({})", e.message()),
        }
    }
}
