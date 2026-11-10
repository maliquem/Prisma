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
mod stream;
mod sys;
mod ui;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

fn main() {
    // Antes de tudo: as DLLs carregadas depois da abertura também vêm só da System32,
    // e não da pasta do programa. O link cuida das que carregam na abertura.
    unsafe {
        use windows::Win32::System::LibraryLoader::*;
        let _ = SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32);
    }
    let cfg = config::Config::load();
    i18n::set_english(cfg.lang_en);
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
    if let Some(arg) = std::env::args().find_map(|a| a.strip_prefix("--setup=").map(str::to_string)) {
        std::process::exit(startup::setup(&arg) as i32);
    }
    let pid_arg = |name: &str| std::env::args().find_map(|a| a.strip_prefix(name).and_then(|p| p.parse::<u32>().ok()));
    let updated = pid_arg("--updated=");
    let before = updated.and_then(sys::update::after_update);
    if let Some(pid) = pid_arg("--wait=") {
        sys::update::wait_exit(pid);
    }
    if let Some(v) = std::env::args().find_map(|a| a.strip_prefix("--set-taskmgr=").map(|v| v == "1")) {
        std::process::exit(if startup::set_replace_taskmgr(v) { 0 } else { 3 });
    }
    // Compartilhar tela (host, quem assiste, serviço): roda ao lado de outras cópias, sem o nome único.
    if stream::run_args() {
        return;
    }
    // Um link de convite (prisma://parear/...): a página Compartilhar tela pergunta se pareia.
    #[cfg(feature = "stream")]
    let link = stream::weblink::take_arg();
    #[cfg(not(feature = "stream"))]
    let link = false;
    // Uma sessão que caiu com o HDR desligado (opção do compartilhamento): liga de volta.
    std::thread::spawn(stream::host::restore_hdr);
    unsafe {
        // Antes do nome único: a cópia de administrador precisa encontrar o nome livre.
        let want_admin = cfg.always_admin && !background && updated.is_none() && !cfg!(debug_assertions);
        if want_admin && !startup::is_elevated() && FindWindowW(w!("PrismaHub"), None).is_err() && startup::open_as_admin() {
            return;
        }
        // A versão de desenvolvimento roda ao lado da que está em uso.
        let name = if cfg!(debug_assertions) { w!("Local\\PrismaAppDev") } else { w!("Local\\PrismaApp") };
        let _mutex = CreateMutexW(None, true, name);
        // Acesso negado também quer dizer que existe: a outra cópia roda como administrador.
        if matches!(GetLastError(), ERROR_ALREADY_EXISTS | ERROR_ACCESS_DENIED) {
            if let Ok(h) = FindWindowW(w!("PrismaHub"), None) {
                let monitor = std::env::args().any(|a| a == "--taskmgr") as usize;
                let show = if link { app::SHOW_SHARE } else { monitor };
                let _ = PostMessageW(Some(h), app::WM_SHOW, WPARAM(show), LPARAM(0));
            }
            return;
        }
        crash::install();
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        app::start(background);
        app::with(|a| a.check_news(before));
        if updated.is_some() {
            app::with(|a| {
                let text = format!("Atualizado para a versão {}", sys::update::current());
                a.toast.info(&text);
                startup::refresh_version(a.cfg.always_admin);
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
