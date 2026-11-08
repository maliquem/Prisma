//! Consultas de processos usadas por várias funções.

use std::collections::HashMap;

use windows::core::PWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Caminho completo do executável, ou None quando o processo não deixa abrir.
pub fn image_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(
            h,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(h);
        r.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

pub fn exe_name(path: &str) -> String {
    path.rsplit('\\').next().unwrap_or(path).to_string()
}

/// Mapa de pai para filhos, tirado de um instantâneo do sistema.
pub fn parent_map() -> HashMap<u32, u32> {
    let mut map = HashMap::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return map };
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                map.insert(e.th32ProcessID, e.th32ParentProcessID);
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    map
}

/// True quando `pid` é `root` ou descende dele.
pub fn is_descendant(parents: &HashMap<u32, u32>, pid: u32, root: u32) -> bool {
    let mut cur = pid;
    for _ in 0..32 {
        if cur == root {
            return true;
        }
        match parents.get(&cur) {
            Some(&p) if p != 0 && p != cur => cur = p,
            _ => return false,
        }
    }
    false
}

pub fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

pub fn window_title(hwnd: HWND) -> String {
    unsafe {
        let mut buf = [0u16; 256];
        let n = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

/// Janelas de programas visíveis, de cima para baixo.
pub fn top_windows() -> Vec<HWND> {
    unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> windows::core::BOOL {
        let out = &mut *(l.0 as *mut Vec<HWND>);
        if IsWindowVisible(h).as_bool()
            && GetWindowTextLengthW(h) > 0
            && GetWindow(h, GW_OWNER).map_or(true, |o| o.is_invalid())
        {
            let ex = WINDOW_EX_STYLE(GetWindowLongW(h, GWL_EXSTYLE) as u32);
            let mut cloaked = 0u32;
            let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut _, 4);
            if !ex.contains(WS_EX_TOOLWINDOW) && cloaked == 0 {
                out.push(h);
            }
        }
        true.into()
    }
    let mut out: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

/// Todas as janelas de primeiro nível, inclusive as escondidas.
pub fn all_windows() -> Vec<HWND> {
    unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> windows::core::BOOL {
        (&mut *(l.0 as *mut Vec<HWND>)).push(h);
        true.into()
    }
    let mut out: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

/// Mostra o arquivo selecionado no Explorador.
pub fn reveal(path: &str) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new(crate::sys::system_tool("explorer")).raw_arg(format!("/select,\"{path}\"")).spawn();
}

/// Abre a janela de propriedades do próprio Windows para o arquivo.
pub fn file_properties(path: &str) {
    use windows::core::{w, HSTRING};
    use windows::Win32::UI::Shell::*;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;
    let file = HSTRING::from(path);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_INVOKEIDLIST,
        lpVerb: w!("properties"),
        lpFile: windows::core::PCWSTR(file.as_ptr()),
        nShow: SW_SHOW.0,
        ..Default::default()
    };
    unsafe {
        let _ = ShellExecuteExW(&mut info);
    }
}

/// Pesquisa o nome do programa no navegador padrão.
pub fn search_online(exe: &str) {
    use windows::core::{w, HSTRING};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let mut q = String::new();
    for b in exe.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_') {
            q.push(b as char);
        } else {
            q.push_str(&format!("%{b:02X}"));
        }
    }
    let url = format!("https://www.google.com/search?q={q}");
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(url), None, None, SW_SHOWNORMAL);
    }
}

pub enum Forced {
    /// Processo encerrado: nome do programa.
    Killed(String),
    /// Janela do sistema: recebeu o fechamento normal, como o Alt+F4 do Windows.
    Closed,
    /// O Windows recusou encerrar: nome do programa.
    Denied(String),
    Nothing,
}

fn class_of(h: HWND) -> String {
    let mut b = [0u16; 64];
    let n = unsafe { GetClassNameW(h, &mut b) };
    String::from_utf16_lossy(&b[..n.max(0) as usize])
}

/// Janela congelada vira um fantasma do Windows: o que interessa é a janela real por trás.
fn real_window(h: HWND) -> HWND {
    use windows::core::{s, w};
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    if class_of(h) != "Ghost" {
        return h;
    }
    type Ghost = unsafe extern "system" fn(HWND) -> HWND;
    unsafe {
        let Ok(user) = GetModuleHandleW(w!("user32.dll")) else { return h };
        let Some(f) = GetProcAddress(user, s!("HungWindowFromGhostWindow")) else { return h };
        let f: Ghost = std::mem::transmute(f);
        let real = f(h);
        if real.is_invalid() {
            h
        } else {
            real
        }
    }
}

/// Apps da Loja rodam dentro do ApplicationFrameHost: encerrar ele fecharia todos.
fn app_pid(h: HWND, host: u32) -> u32 {
    unsafe extern "system" fn found(c: HWND, l: LPARAM) -> windows::core::BOOL {
        let (host, out) = &mut *(l.0 as *mut (u32, u32));
        let pid = window_pid(c);
        if pid != *host {
            *out = pid;
            return false.into();
        }
        true.into()
    }
    let mut state = (host, host);
    unsafe {
        let _ = EnumChildWindows(Some(h), Some(found), LPARAM(&mut state as *mut _ as isize));
    }
    state.1
}

/// Encerra o programa da janela pelo PID, sem pedir a ele: funciona mesmo travado.
pub fn force_close(h: HWND) -> Forced {
    use windows::Win32::Foundation::*;
    use windows::Win32::System::Threading::*;
    if h.is_invalid() {
        return Forced::Nothing;
    }
    let w = real_window(h);
    if matches!(class_of(w).as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
        // Na área de trabalho, o Alt+F4 normal abre a caixa de desligar.
        unsafe {
            let _ = PostMessageW(Some(GetShellWindow()), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        return Forced::Closed;
    }
    let mut pid = window_pid(w);
    let host = image_path(pid).map(|p| exe_name(&p)).unwrap_or_default();
    if host.eq_ignore_ascii_case("ApplicationFrameHost.exe") {
        pid = app_pid(w, pid);
    }
    let exe = image_path(pid).map(|p| exe_name(&p)).unwrap_or_default();
    if pid == std::process::id() || crate::optimizer::is_system(&exe) {
        unsafe {
            let _ = PostMessageW(Some(w), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        return Forced::Closed;
    }
    let name = if exe.is_empty() { format!("PID {pid}") } else { exe };
    unsafe {
        let Ok(p) = OpenProcess(PROCESS_TERMINATE, false, pid) else { return Forced::Denied(name) };
        let done = TerminateProcess(p, 1).is_ok();
        let _ = CloseHandle(p);
        if done {
            Forced::Killed(name)
        } else {
            Forced::Denied(name)
        }
    }
}

fn tray_pid() -> u32 {
    use windows::core::w;
    unsafe { FindWindowW(w!("Shell_TrayWnd"), None) }.map(window_pid).unwrap_or(0)
}

/// PID do Explorer que desenha a barra de tarefas e a área de trabalho, conferido pelo arquivo:
/// tem de ser o explorer.exe da pasta do Windows, e não outro programa com o mesmo nome.
pub fn shell_pid() -> Option<u32> {
    let pid = match unsafe { GetShellWindow() } {
        h if !h.is_invalid() => window_pid(h),
        _ => tray_pid(),
    };
    let want = crate::sys::system_tool("explorer");
    let path = image_path(pid)?;
    path.eq_ignore_ascii_case(&want.to_string_lossy()).then_some(pid)
}

/// Fecha o Explorer da barra de tarefas, mesmo travado, e espera o Windows abrir outro.
/// Se ele não voltar sozinho, abre. Bloqueia por alguns segundos: chame fora da janela.
pub fn restart_explorer() -> bool {
    let running = tray_pid() != 0 || !unsafe { GetShellWindow() }.is_invalid();
    let old = shell_pid();
    if running && old.is_none() {
        return false;
    }
    if let Some(pid) = old {
        unsafe {
            let Ok(p) = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid) else { return false };
            let done = TerminateProcess(p, 1).is_ok();
            if done {
                let _ = WaitForSingleObject(p, 5_000);
            }
            let _ = CloseHandle(p);
            if !done {
                return false;
            }
        }
    }
    // Abrir um segundo por cima do que o Windows já reabriu viraria uma janela de pasta.
    for _ in 0..80 {
        let now = tray_pid();
        if now != 0 && Some(now) != old {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    std::process::Command::new(crate::sys::system_tool("explorer")).spawn().is_ok()
}
