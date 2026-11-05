#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod capture;
mod config;
mod crash;
mod i18n;
mod optimizer;
mod meter;
mod overlays;
mod pages;
mod report;
#[cfg(debug_assertions)]
mod selftest;
mod startup;
mod sys;
mod ui;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

fn main() {
    i18n::set_english(config::Config::load().lang_en);
    let background = std::env::args().any(|a| a == "--background");
    if std::env::args().any(|a| a == "--uninstall") {
        unsafe {
            let msg = HSTRING::from(i18n::tr("Remover o Prisma deste computador? Suas configurações ficam guardadas.").as_ref());
            if MessageBoxW(None, &msg, w!("Prisma"), MB_YESNO | MB_ICONQUESTION) == IDYES {
                startup::uninstall();
            }
        }
        return;
    }
    if let Some(arg) = std::env::args().find(|a| a.starts_with("--install")) {
        let parent = arg.strip_prefix("--install=").and_then(|p| p.parse::<u32>().ok());
        if !startup::install(true, parent) {
            unsafe {
                let msg = HSTRING::from(i18n::tr("Não consegui instalar o Prisma.").as_ref());
                MessageBoxW(None, &msg, w!("Prisma"), MB_OK | MB_ICONERROR);
            }
        }
        return;
    }
    let pid_arg = |name: &str| std::env::args().find_map(|a| a.strip_prefix(name).and_then(|p| p.parse::<u32>().ok()));
    let updated = pid_arg("--updated=");
    if let Some(pid) = updated {
        sys::update::after_update(pid);
    }
    if let Some(pid) = pid_arg("--wait=") {
        sys::update::wait_exit(pid);
    }
    if let Some(v) = std::env::args().find_map(|a| a.strip_prefix("--set-taskmgr=").map(|v| v == "1")) {
        startup::set_replace_taskmgr(v);
        return;
    }
    unsafe {
        // A versão de desenvolvimento roda ao lado da que está em uso.
        let name = if cfg!(debug_assertions) { w!("Local\\PrismaAppDev") } else { w!("Local\\PrismaApp") };
        let _mutex = CreateMutexW(None, true, name);
        // Acesso negado também quer dizer que existe: a outra cópia roda como administrador.
        if matches!(GetLastError(), ERROR_ALREADY_EXISTS | ERROR_ACCESS_DENIED) {
            if let Ok(h) = FindWindowW(w!("PrismaHub"), None) {
                let monitor = std::env::args().any(|a| a == "--taskmgr") as usize;
                let _ = PostMessageW(Some(h), app::WM_SHOW, WPARAM(monitor), LPARAM(0));
            }
            return;
        }
        crash::install();
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        app::start(background);
        if updated.is_some() {
            app::with(|a| {
                let text = format!("Atualizado para a versão {}", sys::update::current());
                a.toast.info(&text);
                startup::refresh_version();
            });
        }
        if crash::pending() {
            app::with(|a| {
                a.log("o Prisma tinha caído: relatório em falha_anterior.dmp");
                a.toast.info("O Prisma fechou sozinho da última vez. Se puder, conte em Reportar bug.");
            });
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        app::shutdown();
        // Sair sem destruir o que ainda está carregado: o Direct3D não sobrevive à ordem
        // de saída do Windows e derrubava o processo aqui, depois de tudo salvo.
        let _ = TerminateProcess(GetCurrentProcess(), 0);
    }
}
