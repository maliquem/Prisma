#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod capture;
mod config;
mod crash;
mod hdr_menu;
mod i18n;
mod link;
mod optimizer;
mod meter;
mod overlays;
mod pages;
mod paste;
mod report;
mod shader;
mod shader_profile;
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
    // Um link prisma:// com outros argumentos é um link com aspas para acrescentar opções: nada abre.
    #[cfg(feature = "stream")]
    if !stream::weblink::link_alone(&std::env::args().collect::<Vec<_>>()) {
        return;
    }
    // Teste: abre o caminho como o radial e os perfis abrem (com administrador, pelo Explorer).
    #[cfg(debug_assertions)]
    if let Some(p) = std::env::args().find_map(|a| a.strip_prefix("--test-open=").map(str::to_string)) {
        std::process::exit(if sys::quick::open_as_user(&p) { 0 } else { 1 });
    }
    let cfg = config::Config::load();
    i18n::set_lang(cfg.lang);
    overlays::set_notice_secs(cfg.toast_secs);
    let background = std::env::args().any(|a| a == "--background");
    if std::env::args().any(|a| a == "--uninstall") {
        if startup::is_installed() && !startup::is_elevated() {
            if !startup::uninstall_as_admin() {
                let msg = HSTRING::from(i18n::tr("Nada foi removido: o Windows não deu a permissão de administrador.").as_ref());
                unsafe { MessageBoxW(None, &msg, w!("Prisma"), MB_OK | MB_ICONWARNING) };
            }
            return;
        }
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
    // Cópia de administrador que mexe nas listas do HidHide e sai.
    if let Some(code) = sys::hidhide::run_arg() {
        std::process::exit(code);
    }
    // Cópia de administrador que esconde, mostra ou instala atualizações do Windows e sai.
    if let Some(code) = sys::winupd::run_arg() {
        std::process::exit(code);
    }
    // Cópia de administrador que instala ou remove o motor do equalizador e sai.
    if let Some(code) = sys::eqapo::run_arg() {
        std::process::exit(code);
    }
    // Cópia vigia da tela virtual: espera o host sair, de qualquer jeito, e devolve as telas daqui.
    if sys::vdd::run_arg() {
        return;
    }
    // Compartilhar tela (host, quem assiste, serviço): roda ao lado de outras cópias, sem o nome único.
    if stream::run_args() {
        return;
    }
    // "Colar com o Prisma" do botão direito do Explorer: a janela de cópia, também sem o nome único.
    if paste::run_args() {
        return;
    }
    // Motor de shaders (--shader-profile) e prévia (--shader-preview), também sem o nome único.
    if shader::run_args() {
        return;
    }
    // Aberto pela Steam (`--hdr %command%`): abre o jogo com o HDR do Prisma, sem o nome único.
    if sys::autohdr::run_args() {
        return;
    }
    // Um link de convite (prisma://parear/...): a página Compartilhar tela pergunta se pareia.
    #[cfg(feature = "stream")]
    let link = stream::weblink::take_arg();
    #[cfg(not(feature = "stream"))]
    let link = false;
    // Pasta escolhida no menu do botão direito do Explorer.
    let folder = sys::shellmenu::from_args(&std::env::args().collect::<Vec<_>>());
    // Uma sessão que caiu com o HDR desligado (opção do compartilhamento): liga de volta.
    std::thread::spawn(stream::host::restore_hdr);
    unsafe {
        // Antes do nome único: a cópia de administrador precisa encontrar o nome livre.
        let want_admin = cfg.always_admin && !background && updated.is_none() && !cfg!(debug_assertions);
        if want_admin && !startup::is_elevated() && FindWindowW(w!("PrismaHub"), None).is_err() && startup::open_as_admin() {
            // A cópia de administrador abre sem argumentos: a pasta vai quando a janela dela existir.
            if let Some((kind, dir)) = &folder {
                for _ in 0..100 {
                    if let Ok(h) = FindWindowW(w!("PrismaHub"), None) {
                        send_folder(h, *kind, dir);
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
            return;
        }
        // A versão de desenvolvimento roda ao lado da que está em uso.
        let name = if cfg!(debug_assertions) { w!("Local\\PrismaAppDev") } else { w!("Local\\PrismaApp") };
        let _mutex = CreateMutexW(None, true, name);
        // Acesso negado também quer dizer que existe: a outra cópia roda como administrador.
        if matches!(GetLastError(), ERROR_ALREADY_EXISTS | ERROR_ACCESS_DENIED) {
            if let Ok(h) = FindWindowW(w!("PrismaHub"), None) {
                if let Some((kind, dir)) = &folder {
                    send_folder(h, *kind, dir);
                    return;
                }
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
        // Só a instância principal chega aqui: a ponte com o Prisma Launcher.
        if let Some(hub) = app::with(|a| a.hub) {
            crate::link::start(hub);
            app::with(|a| a.apply_pads());
            sys::pads::start(hub);
        }
        app::with(|a| {
            a.sync_explorer_menu();
            if let Some((kind, dir)) = &folder {
                a.open_folder(*kind, dir);
            }
        });
        app::with(|a| a.check_news(before));
        if updated.is_some() {
            app::with(|a| {
                let text = format!("Atualizado para a versão {}", sys::update::current());
                a.toast.info(&text);
                startup::refresh_version(a.cfg.always_admin);
            });
        }
        // O Prisma caiu com um controle escondido pelo HidHide: devolve as listas como estavam.
        std::thread::spawn(|| sys::hidhide::limpar());
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
        // O controle escondido volta a aparecer: sem o Prisma, a ponte não existe.
        sys::hidhide::limpar();
        // O VIIPER só serve à ponte: fecha junto.
        sys::xbridge::shutdown();
        // Sair sem destruir o que ainda está carregado: o Direct3D não sobrevive à ordem
        // de saída do Windows e derrubava o processo aqui, depois de tudo salvo.
        let _ = TerminateProcess(GetCurrentProcess(), 0);
    }
}

/// Manda à cópia aberta a pasta escolhida no menu do Explorer (WM_COPYDATA, texto em UTF-16).
unsafe fn send_folder(h: HWND, kind: sys::shellmenu::Open, dir: &str) {
    let w: Vec<u16> = dir.encode_utf16().collect();
    let cd = windows::Win32::System::DataExchange::COPYDATASTRUCT { dwData: kind as usize, cbData: (w.len() * 2) as u32, lpData: w.as_ptr() as *mut _ };
    // A cópia aberta precisa poder vir para a frente: o Explorer deu esse direito a esta.
    let _ = AllowSetForegroundWindow(ASFW_ANY);
    let _ = SendMessageTimeoutW(h, WM_COPYDATA, WPARAM(0), LPARAM(&cd as *const _ as isize), SMTO_ABORTIFHUNG, 15000, None);
}
