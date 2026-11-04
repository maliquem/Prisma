#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod capture;
mod config;
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
    let background = std::env::args().any(|a| a == "--background");
    if std::env::args().any(|a| a == "--uninstall") {
        unsafe {
            let msg = w!("Remover o Prisma deste computador? Suas configurações ficam guardadas.");
            if MessageBoxW(None, msg, w!("Prisma"), MB_YESNO | MB_ICONQUESTION) == IDYES {
                startup::uninstall();
            }
        }
        return;
    }
    if std::env::args().any(|a| a == "--install") {
        if !startup::install(true) {
            unsafe {
                MessageBoxW(None, w!("Não consegui instalar o Prisma."), w!("Prisma"), MB_OK | MB_ICONERROR);
            }
        }
        return;
    }
    unsafe {
        let _mutex = CreateMutexW(None, true, w!("Local\\PrismaApp"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(h) = FindWindowW(w!("PrismaHub"), None) {
                let monitor = std::env::args().any(|a| a == "--taskmgr") as usize;
                let _ = PostMessageW(Some(h), app::WM_SHOW, WPARAM(monitor), LPARAM(0));
            }
            return;
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        app::start(background);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        app::shutdown();
    }
}
