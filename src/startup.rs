//! Início com o Windows por tarefa agendada, que abre como administrador sem perguntar.

use std::os::windows::process::CommandExt;
use std::process::Command;

const NO_WINDOW: u32 = 0x0800_0000;
const TASK: &str = "Prisma";

pub fn is_enabled() -> bool {
    Command::new("schtasks")
        .args(["/Query", "/TN", TASK])
        .creation_flags(NO_WINDOW)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Precisa rodar como administrador. Devolve false quando o Windows recusa.
pub fn set_enabled(on: bool) -> bool {
    let mut cmd = Command::new("schtasks");
    if on {
        let exe = std::env::current_exe().unwrap_or_default();
        let run = format!("\"{}\" --background", exe.display());
        cmd.args(["/Create", "/F", "/TN", TASK, "/SC", "ONLOGON", "/RL", "HIGHEST", "/TR", &run]);
    } else {
        cmd.args(["/Delete", "/F", "/TN", TASK]);
    }
    cmd.creation_flags(NO_WINDOW).output().map(|o| o.status.success()).unwrap_or(false)
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

/// Roda uma cópia com a confirmação do Windows.
pub fn run_elevated(args: &str) {
    use windows::core::*;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let exe = std::env::current_exe().unwrap_or_default();
    unsafe {
        ShellExecuteW(None, w!("runas"), &HSTRING::from(exe.as_os_str()), &HSTRING::from(args), None, SW_SHOWNORMAL);
    }
}

/// Abre uma cópia com a confirmação do Windows e fecha esta.
pub fn relaunch_elevated() {
    use windows::core::*;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::{PostQuitMessage, SW_SHOWNORMAL};
    let exe = std::env::current_exe().unwrap_or_default();
    let path = HSTRING::from(exe.as_os_str());
    unsafe {
        let r = ShellExecuteW(None, w!("runas"), &path, None, None, SW_SHOWNORMAL);
        if r.0 as isize > 32 {
            PostQuitMessage(0);
        }
    }
}
const IFEO: &str = "HKLM\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Image File Execution Options\\taskmgr.exe";

/// True quando Ctrl+Shift+Esc já abre o Prisma.
pub fn replaces_taskmgr() -> bool {
    Command::new("reg")
        .args(["query", IFEO, "/v", "Debugger"])
        .creation_flags(NO_WINDOW)
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).to_ascii_lowercase().contains("prisma"))
        .unwrap_or(false)
}

/// Mesmo mecanismo do Process Explorer. Desligar devolve o Gerenciador de Tarefas.
pub fn set_replace_taskmgr(on: bool) -> bool {
    let mut cmd = Command::new("reg");
    if on {
        let exe = std::env::current_exe().unwrap_or_default();
        let run = format!("\"{}\" --taskmgr", exe.display());
        cmd.args(["add", IFEO, "/v", "Debugger", "/t", "REG_SZ", "/d", &run, "/f"]);
    } else {
        cmd.args(["delete", IFEO, "/v", "Debugger", "/f"]);
    }
    cmd.creation_flags(NO_WINDOW).output().map(|o| o.status.success()).unwrap_or(false)
}

const UNINSTALL_KEY: &str = "HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Prisma";

fn install_dir() -> std::path::PathBuf {
    let base = std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".into());
    std::path::PathBuf::from(base).join("Prisma")
}

fn shortcut() -> std::path::PathBuf {
    let base = std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into());
    std::path::PathBuf::from(base).join("Microsoft\\Windows\\Start Menu\\Programs\\Prisma.lnk")
}

/// Rodando de Arquivos de Programas é a versão instalada; de qualquer outro lugar, portátil.
pub fn is_installed() -> bool {
    let exe = std::env::current_exe().unwrap_or_default().display().to_string().to_ascii_lowercase();
    exe.starts_with(&format!("{}\\", install_dir().display().to_string().to_ascii_lowercase()))
}

fn make_shortcut(target: &str, lnk: &str) -> bool {
    use windows::core::*;
    use windows::Win32::System::Com::*;
    use windows::Win32::UI::Shell::*;
    unsafe {
        let Ok(l) = CoCreateInstance::<_, IShellLinkW>(&ShellLink, None, CLSCTX_INPROC_SERVER) else { return false };
        let _ = l.SetPath(&HSTRING::from(target));
        let _ = l.SetDescription(w!("Prisma"));
        let Ok(f) = l.cast::<IPersistFile>() else { return false };
        f.Save(&HSTRING::from(lnk), true).is_ok()
    }
}

fn reg_add(name: &str, ty: &str, value: &str) {
    let _ = Command::new("reg")
        .args(["add", UNINSTALL_KEY, "/v", name, "/t", ty, "/d", value, "/f"])
        .creation_flags(NO_WINDOW)
        .output();
}

/// Copia para Arquivos de Programas, registra no Windows e leva as configurações junto.
/// Lá dentro só administrador escreve, então abrir como administrador sem pedir é seguro.
pub fn install(open_after: bool) -> bool {
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
    make_shortcut(&d, &shortcut().display().to_string());
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
    let target = std::path::PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join("Prisma");
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
    !open_after || Command::new(&dest).spawn().is_ok()
}

/// Desfaz tudo. As configurações ficam, para uma reinstalação.
pub fn uninstall() {
    set_enabled(false);
    if replaces_taskmgr() {
        set_replace_taskmgr(false);
    }
    let _ = std::fs::remove_file(shortcut());
    let _ = Command::new("reg").args(["delete", UNINSTALL_KEY, "/f"]).creation_flags(NO_WINDOW).output();
    // O arquivo em uso só pode sair depois que este processo fechar.
    let line = format!("/c ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"", install_dir().display());
    let _ = Command::new("cmd").raw_arg(line).creation_flags(NO_WINDOW).spawn();
}
