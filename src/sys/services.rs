//! Serviços do Windows: lista, iniciar e parar. Editar fica com a janela nativa.

use windows::core::*;
use windows::Win32::System::Services::*;

#[derive(Clone, Debug)]
pub struct Service {
    pub name: String,
    pub display: String,
    pub running: bool,
    pub pid: u32,
    pub start: u32,
}

impl Service {
    pub fn start_name(&self) -> &'static str {
        match self.start {
            0 | 1 | 2 => "Automático",
            3 => "Manual",
            4 => "Desativado",
            _ => "Outro",
        }
    }
}

fn wstr(p: PWSTR) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { p.to_string().unwrap_or_default() }
    }
}

fn manager(access: u32) -> Option<SC_HANDLE> {
    unsafe { OpenSCManagerW(None, None, access).ok() }
}

fn start_type(scm: SC_HANDLE, name: &HSTRING) -> u32 {
    unsafe {
        let Ok(h) = OpenServiceW(scm, name, SERVICE_QUERY_CONFIG) else { return 99 };
        let mut need = 0;
        let _ = QueryServiceConfigW(h, None, 0, &mut need);
        let mut buf = vec![0u8; need as usize + 8];
        let cfg = buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW;
        let mut out = 99;
        if QueryServiceConfigW(h, Some(cfg), buf.len() as u32, &mut need).is_ok() {
            out = (*cfg).dwStartType.0;
        }
        let _ = CloseServiceHandle(h);
        out
    }
}

pub fn list() -> Vec<Service> {
    let mut out = Vec::new();
    let Some(scm) = manager(SC_MANAGER_ENUMERATE_SERVICE) else { return out };
    unsafe {
        let (mut need, mut count, mut resume) = (0u32, 0u32, 0u32);
        let _ = EnumServicesStatusExW(
            scm,
            SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            None,
            &mut need,
            &mut count,
            Some(&mut resume),
            None,
        );
        let mut buf = vec![0u8; need as usize + 4096];
        resume = 0;
        let r = EnumServicesStatusExW(
            scm,
            SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            Some(&mut buf),
            &mut need,
            &mut count,
            Some(&mut resume),
            None,
        );
        if r.is_ok() {
            let items = std::slice::from_raw_parts(buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW, count as usize);
            for s in items {
                let name = wstr(s.lpServiceName);
                let st = start_type(scm, &HSTRING::from(name.as_str()));
                out.push(Service {
                    display: wstr(s.lpDisplayName),
                    running: s.ServiceStatusProcess.dwCurrentState == SERVICE_RUNNING,
                    pid: s.ServiceStatusProcess.dwProcessId,
                    start: st,
                    name,
                });
            }
        }
        let _ = CloseServiceHandle(scm);
    }
    out.sort_by_key(|s| s.display.to_lowercase());
    out
}

/// Descrição e caminho do executável, já no idioma do Windows.
pub fn details(name: &str) -> (String, String) {
    let mut desc = String::new();
    let mut bin = String::new();
    let Some(scm) = manager(SC_MANAGER_CONNECT) else { return (desc, bin) };
    unsafe {
        if let Ok(h) = OpenServiceW(scm, &HSTRING::from(name), SERVICE_QUERY_CONFIG) {
            let mut need = 0;
            let _ = QueryServiceConfig2W(h, SERVICE_CONFIG_DESCRIPTION, None, &mut need);
            let mut b = vec![0u8; need as usize + 8];
            if QueryServiceConfig2W(h, SERVICE_CONFIG_DESCRIPTION, Some(&mut b), &mut need).is_ok() {
                desc = wstr((*(b.as_ptr() as *const SERVICE_DESCRIPTIONW)).lpDescription);
            }
            let _ = QueryServiceConfigW(h, None, 0, &mut need);
            let mut c = vec![0u8; need as usize + 8];
            let cfg = c.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW;
            if QueryServiceConfigW(h, Some(cfg), c.len() as u32, &mut need).is_ok() {
                bin = wstr((*cfg).lpBinaryPathName);
            }
            let _ = CloseServiceHandle(h);
        }
        let _ = CloseServiceHandle(scm);
    }
    (desc, bin)
}

#[derive(Clone, Copy)]
pub enum Action {
    Start,
    Stop,
    Restart,
}

fn state(h: SC_HANDLE) -> Option<SERVICE_STATUS_CURRENT_STATE> {
    unsafe {
        let mut s = SERVICE_STATUS::default();
        QueryServiceStatus(h, &mut s).ok()?;
        Some(s.dwCurrentState)
    }
}

/// Roda em segundo plano, porque um serviço pode levar segundos para parar.
pub fn control(name: String, act: Action) {
    let _ = std::thread::Builder::new().name("servico".into()).spawn(move || unsafe {
        let Some(scm) = manager(SC_MANAGER_CONNECT) else { return };
        let access = SERVICE_START | SERVICE_STOP | SERVICE_QUERY_STATUS;
        if let Ok(h) = OpenServiceW(scm, &HSTRING::from(name.as_str()), access) {
            if matches!(act, Action::Stop | Action::Restart) {
                let mut s = SERVICE_STATUS::default();
                let _ = ControlService(h, SERVICE_CONTROL_STOP, &mut s);
                for _ in 0..100 {
                    if state(h) == Some(SERVICE_STOPPED) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
            if matches!(act, Action::Start | Action::Restart) {
                let _ = StartServiceW(h, None);
            }
            let _ = CloseServiceHandle(h);
        }
        let _ = CloseServiceHandle(scm);
    });
}

pub fn open_native() {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    unsafe {
        let msc = windows::core::HSTRING::from(crate::sys::system_tool("services.msc").as_os_str());
        let _ = ShellExecuteW(None, w!("open"), &msc, None, None, SW_SHOWNORMAL);
    }
}

// O Windows não tem comando para abrir as propriedades de um serviço. Então abre a janela
// Serviços, acha o item pela automação de interface e pede a ação padrão da lista.
// O Enter vai por mensagem à própria lista, nunca ao teclado: não digita em outra janela.
mod props {
    use std::collections::HashSet;
    use std::time::{Duration, Instant};

    use windows::core::{BOOL, BSTR};
    use windows::Win32::Foundation::*;
    use windows::Win32::System::Com::*;
    use windows::Win32::System::Threading::*;
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Accessibility::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    const LVM_GETITEMCOUNT: u32 = 0x1004;
    const VK_RETURN: usize = 0x0D;

    fn class_of(h: HWND) -> String {
        let mut b = [0u16; 64];
        let n = unsafe { GetClassNameW(h, &mut b) };
        String::from_utf16_lossy(&b[..n.max(0) as usize])
    }

    fn tops() -> Vec<HWND> {
        crate::sys::procs::all_windows()
    }

    pub fn frames() -> HashSet<isize> {
        tops().into_iter().filter(|h| class_of(*h) == "MMCMainFrame").map(|h| h.0 as isize).collect()
    }

    fn list_in(frame: HWND) -> Option<HWND> {
        unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> BOOL {
            let out = &mut *(l.0 as *mut Vec<HWND>);
            if class_of(h) == "SysListView32" {
                out.push(h);
            }
            true.into()
        }
        let mut v: Vec<HWND> = Vec::new();
        unsafe {
            let _ = EnumChildWindows(Some(frame), Some(cb), LPARAM(&mut v as *mut _ as isize));
        }
        // A lista de serviços é a maior da janela.
        v.into_iter().max_by_key(|h| unsafe { SendMessageW(*h, LVM_GETITEMCOUNT, None, None).0 })
    }

    fn select_item(list: HWND, display: &str) -> windows::core::Result<()> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let ua: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let root = ua.ElementFromHandle(list)?;
            let name = VARIANT::from(BSTR::from(display));
            let cond = ua.CreatePropertyCondition(UIA_NamePropertyId, &name)?;
            let item = root.FindFirst(TreeScope_Descendants, &cond)?;
            let pick: IUIAutomationSelectionItemPattern = item.GetCurrentPatternAs(UIA_SelectionItemPatternId)?;
            pick.Select()?;
            item.SetFocus()
        }
    }

    #[cfg(debug_assertions)]
    pub fn dump(frame: isize) -> Vec<String> {
        let pid_of = |h: HWND| {
            let mut pid = 0;
            unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
            pid
        };
        let owner = pid_of(HWND(frame as *mut _));
        tops()
            .into_iter()
            .filter(|h| pid_of(*h) == owner && unsafe { IsWindowVisible(*h).as_bool() })
            .map(|h| format!("[{}] {}", class_of(h), crate::sys::procs::window_title(h)))
            .collect()
    }

    /// Janelas de propriedades abertas agora pelo console.
    pub fn dialogs() -> Vec<(HWND, String)> {
        tops()
            .into_iter()
            .filter(|h| class_of(*h) == "#32770" && unsafe { IsWindowVisible(*h).as_bool() })
            .map(|h| (h, crate::sys::procs::window_title(h)))
            .collect()
    }

    pub fn open(display: &str) -> Result<isize, &'static str> {
        close_open();
        let t0 = Instant::now();
        while frames().iter().any(|f| crate::sys::procs::window_title(HWND(*f as *mut _)) == "Serviços") && t0.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let before = frames();
        std::process::Command::new(crate::sys::system_tool("mmc")).arg(crate::sys::system_tool("services.msc")).spawn().map_err(|_| "não abriu a janela Serviços")?;
        let t0 = Instant::now();
        let frame = loop {
            if t0.elapsed() > Duration::from_secs(15) {
                return Err("a janela Serviços não apareceu");
            }
            if let Some(f) = frames().difference(&before).next() {
                break HWND(*f as *mut _);
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let list = loop {
            if t0.elapsed() > Duration::from_secs(15) {
                return Err("a lista de serviços não carregou");
            }
            if let Some(l) = list_in(frame) {
                let n = unsafe { SendMessageW(l, LVM_GETITEMCOUNT, None, None).0 };
                if n > 20 {
                    break l;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        unsafe {
            let _ = SetForegroundWindow(frame);
        }
        let t2 = Instant::now();
        while select_item(list, display).is_err() {
            if t2.elapsed() > Duration::from_secs(4) {
                return Err("não achei o serviço na lista");
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        unsafe {
            let _ = PostMessageW(Some(list), WM_KEYDOWN, WPARAM(VK_RETURN), LPARAM(1));
            let _ = PostMessageW(Some(list), WM_KEYUP, WPARAM(VK_RETURN), LPARAM(0xC000_0001u32 as isize));
        }
        let t1 = Instant::now();
        while t1.elapsed() < Duration::from_secs(5) {
            if dialogs().iter().any(|d| d.1.contains(display)) {
                return Ok(frame.0 as isize);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err("a janela Serviços abriu, mas as propriedades não")
    }

    /// Fecha as janelas Serviços abertas: com várias ao mesmo tempo, só a última responde.
    /// Encerra o processo porque o console recusa fechar com uma janela de propriedades aberta.
    pub fn close_open() {
        for h in tops() {
            let title = crate::sys::procs::window_title(h);
            if class_of(h) != "MMCMainFrame" || !(title == "Serviços" || title == "Services") {
                continue;
            }
            let mut pid = 0;
            unsafe {
                GetWindowThreadProcessId(h, Some(&mut pid));
                if let Ok(p) = OpenProcess(PROCESS_TERMINATE, false, pid) {
                    let _ = TerminateProcess(p, 0);
                    let _ = CloseHandle(p);
                }
            }
        }
    }
}

#[cfg(debug_assertions)]
pub fn open_properties(display: &str) -> std::result::Result<isize, &'static str> {
    props::open(display)
}

static OPENING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Abre sem travar a interface; se falhar, a janela Serviços já fica aberta para seguir à mão.
/// Cliques seguidos entram em fila: cada um fecha a janela anterior antes de abrir a sua.
pub fn show_properties(display: String) {
    let _ = std::thread::Builder::new().name("propriedades".into()).spawn(move || {
        let _busy = OPENING.lock();
        let _ = props::open(&display);
    });
}

#[cfg(debug_assertions)]
pub fn console_dump(frame: isize) -> Vec<String> {
    props::dump(frame)
}

#[cfg(debug_assertions)]
pub fn console_windows() -> Vec<isize> {
    props::frames().into_iter().collect()
}

#[cfg(debug_assertions)]
pub fn close_consoles() {
    props::close_open();
}
