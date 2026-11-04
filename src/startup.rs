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