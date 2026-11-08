//! Início com o Windows por tarefa agendada, e a instalação escolhida nas boas-vindas.

use std::os::windows::process::CommandExt;
use std::process::Command;

const NO_WINDOW: u32 = 0x0800_0000;
const TASK: &str = "Prisma";
/// Tarefa sem horário: é assim que abrir pelo atalho vira administrador sem perguntar.
const TASK_OPEN: &str = "PrismaOpen";

pub fn is_enabled() -> bool {
    Command::new(crate::sys::system_tool("schtasks"))
        .args(["/Query", "/TN", TASK])
        .creation_flags(NO_WINDOW)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Precisa rodar como administrador. Devolve false quando o Windows recusa.
/// Só a versão instalada ganha a tarefa: veja `create_task`.
pub fn set_enabled(on: bool, admin: bool) -> bool {
    if on {
        return create_task(TASK, &std::env::current_exe().unwrap_or_default(), "--background", true, admin);
    }
    delete_task(TASK)
}

fn delete_task(name: &str) -> bool {
    let cmd = Command::new(crate::sys::system_tool("schtasks")).args(["/Delete", "/F", "/TN", name]).creation_flags(NO_WINDOW).output();
    cmd.map(|o| o.status.success()).unwrap_or(false)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// O padrão do Windows para tarefas não serve aqui: não abre na bateria, fecha o programa
/// depois de 72 horas e o deixa com prioridade baixa. Só o XML troca essas três coisas.
fn task_xml(exe: &std::path::Path, args: &str, logon: bool, admin: bool) -> String {
    let user = xml_escape(&user_name());
    let trigger = if logon { format!("<LogonTrigger><UserId>{user}</UserId></LogonTrigger>") } else { String::new() };
    let level = if admin { "HighestAvailable" } else { "LeastPrivilege" };
    let command = xml_escape(&exe.display().to_string());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\r\n\
<Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\r\n\
<Triggers>{trigger}</Triggers>\r\n\
<Principals><Principal id=\"Author\"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>{level}</RunLevel></Principal></Principals>\r\n\
<Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><Priority>5</Priority></Settings>\r\n\
<Actions Context=\"Author\"><Exec><Command>{command}</Command><Arguments>{args}</Arguments></Exec></Actions>\r\n\
</Task>\r\n"
    )
}

/// Conta pelo Windows, como `PC\nome`. `%USERNAME%` é texto que qualquer programa troca.
fn user_name() -> String {
    use windows::Win32::Security::Authentication::Identity::*;
    let mut buf = [0u16; 512];
    let mut len = buf.len() as u32;
    if unsafe { GetUserNameExW(NameSamCompatible, Some(windows::core::PWSTR(buf.as_mut_ptr())), &mut len) } {
        String::from_utf16_lossy(&buf[..len as usize])
    } else {
        String::new()
    }
}

/// Só para a cópia em Arquivos de Programas. Em qualquer outra pasta, um programa comum troca o
/// arquivo, ou o XML entre gravar e o Windows ler, e a tarefa passaria a abrir o que ele quiser.
/// O XML fica ao lado do programa, onde só administrador escreve.
pub fn create_task(name: &str, exe: &std::path::Path, args: &str, logon: bool, admin: bool) -> bool {
    // A versão de desenvolvimento testa tarefas comuns a partir da própria pasta.
    if !in_install_dir(exe) && !cfg!(debug_assertions) {
        return false;
    }
    let admin = admin && in_install_dir(exe);
    let file = exe.with_file_name(format!("{name}.xml"));
    let mut bytes = vec![0xff, 0xfe];
    bytes.extend(task_xml(exe, args, logon, admin).encode_utf16().flat_map(u16::to_le_bytes));
    if std::fs::write(&file, bytes).is_err() {
        return false;
    }
    let cmd = Command::new(crate::sys::system_tool("schtasks")).args(["/Create", "/F", "/TN", name, "/XML"]).arg(&file).creation_flags(NO_WINDOW).output();
    let _ = std::fs::remove_file(&file);
    cmd.map(|o| o.status.success()).unwrap_or(false)
}

/// Ajusta o nível das tarefas da versão instalada. Precisa rodar como administrador.
pub fn sync_admin(on: bool) -> bool {
    let exe = install_dir().join("prisma.exe");
    let ok = if on {
        create_task(TASK_OPEN, &exe, "", false, true)
    } else {
        delete_task(TASK_OPEN);
        true
    };
    if is_enabled() {
        return create_task(TASK, &exe, "--background", true, on) && ok;
    }
    ok
}

/// Abre a cópia de administrador. Instalado, a tarefa abre sem perguntar; portátil, a confirmação
/// do Windows é o único caminho seguro, porque qualquer programa pode trocar um arquivo fora de
/// Arquivos de Programas.
pub fn open_as_admin() -> bool {
    if is_installed() {
        let run = Command::new(crate::sys::system_tool("schtasks")).args(["/Run", "/TN", TASK_OPEN]).creation_flags(NO_WINDOW).output();
        if run.map(|o| o.status.success()).unwrap_or(false) {
            return true;
        }
    }
    run_elevated("")
}

pub fn is_elevated() -> bool {
    use windows::Win32::Foundation::*;
    use windows::Win32::Security::*;
    use windows::Win32::System::Threading::*;
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut e as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        let _ = CloseHandle(token);
        ok.is_ok() && e.TokenIsElevated != 0
    }
}

/// Roda uma cópia com a confirmação do Windows. Devolve false se a pessoa recusar.
pub fn run_elevated(args: &str) -> bool {
    use windows::core::*;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let exe = std::env::current_exe().unwrap_or_default();
    let r = unsafe { ShellExecuteW(None, w!("runas"), &HSTRING::from(exe.as_os_str()), &HSTRING::from(args), None, SW_SHOWNORMAL) };
    r.0 as isize > 32
}

/// Mesma confirmação, esperando a cópia terminar. Devolve o código de saída, ou None se a pessoa recusar.
pub fn run_elevated_wait(args: &str) -> Option<u32> {
    use windows::core::*;
    use windows::Win32::Foundation::*;
    use windows::Win32::System::Threading::*;
    use windows::Win32::UI::Shell::*;
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    let exe = HSTRING::from(std::env::current_exe().unwrap_or_default().as_os_str());
    let params = HSTRING::from(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(exe.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        ShellExecuteExW(&mut info).ok()?;
        let mut code = 1;
        let _ = WaitForSingleObject(info.hProcess, 120_000);
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        Some(code)
    }
}

/// Abre uma cópia com a confirmação do Windows e fecha esta. Devolve false se a pessoa recusar.
/// A cópia nova espera esta sair: com as duas abertas, ela só avisaria a outra e sairia.
pub fn relaunch_elevated(extra: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::PostQuitMessage;
    let ok = run_elevated(format!("--wait={} {extra}", std::process::id()).trim_end());
    if ok {
        unsafe { PostQuitMessage(0) };
    }
    ok
}
const IFEO: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Image File Execution Options\\taskmgr.exe";

/// True quando Ctrl+Shift+Esc já abre o Prisma.
pub fn replaces_taskmgr() -> bool {
    use crate::sys::reg;
    reg::text(reg::Hive::Machine, IFEO, "Debugger").is_some_and(|v| v.to_ascii_lowercase().contains("prisma"))
}

/// Mesmo mecanismo do Process Explorer. Desligar devolve o Gerenciador de Tarefas.
pub fn set_replace_taskmgr(on: bool) -> bool {
    replace_taskmgr_with(&std::env::current_exe().unwrap_or_default(), on)
}

/// Ligar só aponta para a cópia instalada. A chave vale para todos os usuários do PC, e um arquivo
/// fora de Arquivos de Programas qualquer programa troca antes de alguém apertar o atalho.
fn replace_taskmgr_with(exe: &std::path::Path, on: bool) -> bool {
    if on && !in_install_dir(exe) {
        return false;
    }
    use crate::sys::reg;
    // Direto pela API do Windows: um reg.exe gravando Debugger nessa chave é o que antivírus
    // procuram, e a recusa vinha sem aviso nenhum.
    let done = if on {
        let run = format!("\"{}\" --taskmgr", exe.display());
        let data: Vec<u8> = run.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect();
        reg::write(reg::Hive::Machine, IFEO, "Debugger", windows::Win32::System::Registry::REG_SZ.0, &data)
    } else {
        reg::delete(reg::Hive::Machine, IFEO, "Debugger")
    };
    // Confere lendo de volta: um antivírus pode aceitar a gravação e desfazer logo depois.
    done && replaces_taskmgr() == on
}

const UNINSTALL_KEY: &str = "HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Prisma";

/// Pela pasta conhecida do Windows: com `%ProgramFiles%` trocado, uma pasta comum passaria por
/// instalada e ganharia a tarefa que abre como administrador sem perguntar.
fn install_dir() -> std::path::PathBuf {
    let base = crate::sys::known_folder(&windows::Win32::UI::Shell::FOLDERID_ProgramFiles);
    let base = if base.as_os_str().is_empty() { std::path::PathBuf::from("C:\\Program Files") } else { base };
    base.join("Prisma")
}

fn shortcut() -> std::path::PathBuf {
    crate::sys::known_folder(&windows::Win32::UI::Shell::FOLDERID_CommonPrograms).join("Prisma.lnk")
}

fn in_install_dir(exe: &std::path::Path) -> bool {
    let exe = exe.display().to_string().to_ascii_lowercase();
    exe.starts_with(&format!("{}\\", install_dir().display().to_string().to_ascii_lowercase()))
}

/// Rodando de Arquivos de Programas é a versão instalada; de qualquer outro lugar, portátil.
pub fn is_installed() -> bool {
    in_install_dir(&std::env::current_exe().unwrap_or_default())
}

fn make_shortcut(target: &str, lnk: &str) -> bool {
    use windows::core::*;
    use windows::Win32::System::Com::*;
    use windows::Win32::UI::Shell::*;
    unsafe {
        // A instalação roda antes do resto do programa, sem o COM iniciado.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Ok(l) = CoCreateInstance::<_, IShellLinkW>(&ShellLink, None, CLSCTX_INPROC_SERVER) else { return false };
        let _ = l.SetPath(&HSTRING::from(target));
        let _ = l.SetDescription(w!("Prisma"));
        let Ok(f) = l.cast::<IPersistFile>() else { return false };
        f.Save(&HSTRING::from(lnk), true).is_ok()
    }
}

fn reg_add(name: &str, ty: &str, value: &str) {
    let _ = Command::new(crate::sys::system_tool("reg"))
        .args(["add", UNINSTALL_KEY, "/v", name, "/t", ty, "/d", value, "/f"])
        .creation_flags(NO_WINDOW)
        .output();
}

/// Copia para Arquivos de Programas, registra no Windows e leva as configurações junto.
/// Lá dentro só administrador escreve, então abrir como administrador sem pedir é seguro.
pub fn install(admin: bool) -> bool {
    let dir = install_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let dest = dir.join("prisma.exe");
    if std::fs::copy(&exe, &dest).is_err() {
        return false;
    }
    let d = dest.display().to_string();
    // Sem o atalho, a pesquisa do Windows não encontra o programa.
    if !make_shortcut(&d, &shortcut().display().to_string()) {
        return false;
    }
    // Instalar já liga o início com o Windows; quem não quiser desliga em Configurações.
    create_task(TASK, &dest, "--background", true, admin);
    if admin {
        create_task(TASK_OPEN, &dest, "", false, true);
    }
    reg_add("DisplayName", "REG_SZ", "Prisma");
    reg_add("DisplayVersion", "REG_SZ", env!("CARGO_PKG_VERSION"));
    reg_add("DisplayIcon", "REG_SZ", &d);
    reg_add("InstallLocation", "REG_SZ", &dir.display().to_string());
    reg_add("UninstallString", "REG_SZ", &format!("\"{d}\" --uninstall"));
    reg_add("NoModify", "REG_DWORD", "1");
    reg_add("NoRepair", "REG_DWORD", "1");
    let size = std::fs::metadata(&dest).map(|m| m.len() / 1024).unwrap_or(0);
    reg_add("EstimatedSize", "REG_DWORD", &size.to_string());

    let portable = exe.parent().map(|p| p.join("PrismaData"));
    let target = crate::sys::known_folder(&windows::Win32::UI::Shell::FOLDERID_RoamingAppData).join("Prisma");
    if let Some(p) = portable {
        if p.is_dir() && !target.join("config.ini").exists() {
            let _ = std::fs::create_dir_all(&target);
            if let Ok(rd) = std::fs::read_dir(&p) {
                for e in rd.flatten() {
                    let _ = std::fs::copy(e.path(), target.join(e.file_name()));
                }
            }
        }
    }
    // Numa reinstalação a pasta já existe: a escolha de agora vale mais que a guardada.
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(target.join("config.ini")) {
        use std::io::Write;
        let _ = writeln!(f, "always_admin={}", admin as u8);
    }
    true
}

/// As escolhas das boas-vindas, feitas de uma vez para o Windows perguntar uma vez só.
#[derive(Clone, Copy, Default)]
pub struct Plan {
    pub install: bool,
    pub admin: bool,
    pub taskmgr: bool,
    /// Esta cópia precisa dar lugar a uma de administrador.
    pub restart: bool,
}

impl Plan {
    pub fn args(&self) -> String {
        let bit = |b: bool| if b { '1' } else { '0' };
        format!("--setup={}{}{}{}:{}", bit(self.install), bit(self.admin), bit(self.taskmgr), bit(self.restart), std::process::id())
    }
}

pub const SETUP_NO_INSTALL: u32 = 1;
pub const SETUP_NO_TASKMGR: u32 = 2;

/// Roda na cópia de administrador. Quem recebe a permissão é a cópia que ela abre no fim.
pub fn setup(arg: &str) -> u32 {
    let (flags, pid) = arg.split_once(':').unwrap_or((arg, ""));
    let bit = |i: usize| flags.as_bytes().get(i) == Some(&b'1');
    let plan = Plan { install: bit(0), admin: bit(1), taskmgr: bit(2), restart: bit(3) };
    let mut exe = std::env::current_exe().unwrap_or_default();
    if plan.install {
        if !install(plan.admin) {
            return SETUP_NO_INSTALL;
        }
        exe = install_dir().join("prisma.exe");
    } else if is_installed() {
        sync_admin(plan.admin);
    }
    // Recusada a troca, o resto continua: o Prisma pega o atalho ele mesmo.
    let tm_ok = !plan.taskmgr || replace_taskmgr_with(&exe, true);
    if plan.restart {
        let _ = Command::new(&exe).arg(format!("--wait={pid}")).spawn();
    }
    if tm_ok { 0 } else { SETUP_NO_TASKMGR }
}

/// A cópia comum abre a instalada, sem a permissão de administrador. Ela espera esta fechar.
pub fn open_installed() -> bool {
    Command::new(install_dir().join("prisma.exe")).arg(format!("--wait={}", std::process::id())).spawn().is_ok()
}

/// Desfaz tudo. As configurações ficam, para uma reinstalação.
pub fn uninstall() {
    delete_task(TASK);
    delete_task(TASK_OPEN);
    if replaces_taskmgr() {
        set_replace_taskmgr(false);
    }
    crate::sys::autohdr::clear_all();
    let _ = std::fs::remove_file(shortcut());
    let _ = Command::new(crate::sys::system_tool("reg")).args(["delete", UNINSTALL_KEY, "/f"]).creation_flags(NO_WINDOW).output();
    // O arquivo em uso só pode sair depois que este processo fechar.
    let line = format!("/c ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"", install_dir().display());
    let _ = Command::new(crate::sys::system_tool("cmd")).raw_arg(line).creation_flags(NO_WINDOW).spawn();
}

/// Depois de atualizar: a lista de programas mostra a versão nova e o atalho volta se faltar.
pub fn refresh_version(admin: bool) {
    if is_installed() {
        reg_add("DisplayVersion", "REG_SZ", env!("CARGO_PKG_VERSION"));
        // Quem veio de uma versão antiga ainda tem as tarefas no padrão do Windows.
        sync_admin(admin);
        if !shortcut().exists() {
            let exe = install_dir().join("prisma.exe").display().to_string();
            make_shortcut(&exe, &shortcut().display().to_string());
        }
    }
}
