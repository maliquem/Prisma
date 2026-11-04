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
        let _ = ShellExecuteW(None, w!("open"), w!("services.msc"), None, None, SW_SHOWNORMAL);
    }
}
