#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod capture;
mod config;
mod crash;
mod i18n;
mod optimizer;
mod overlays;
mod pages;
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
    if std::env::args().any(|a| a == "--install") {
        if !startup::install(true) {
            unsafe {
                let msg = HSTRING::from(i18n::tr("Não consegui instalar o Prisma.").as_ref());
                MessageBoxW(None, &msg, w!("Prisma"), MB_OK | MB_ICONERROR);
            }
        }
        return;
    }
    let updated = std::env::args().find_map(|a| a.strip_prefix("--updated=").and_then(|p| p.parse::<u32>().ok()));
    if let Some(pid) = updated {
        sys::update::after_update(pid);
    }
    unsafe {
        // A versão de desenvolvimento roda ao lado da que está em uso.
        let name = if cfg!(debug_assertions) { w!("Local\\PrismaAppDev") } else { w!("Local\\PrismaApp") };
        let _mutex = CreateMutexW(None, true, name);
        if GetLastError() == ERROR_ALREADY_EXISTS {
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
                a.toast.info("O Prisma fechou sozinho da última vez. O relatório ficou guardado.");
            });
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        app::shutdown();
    }
}
