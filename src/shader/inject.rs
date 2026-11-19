//! Modo Injeção do motor: põe a DLL de shaders (`shader-hook`) no jogo, passa o perfil e dá sinal de vida. O
//! efeito roda dentro do jogo, no Present; aqui ficam o menu, os atalhos (`menu`, `next`, `compare`) e a volta
//! para a janela por cima quando a injeção não serve no jogo (API sem suporte, DLL que não entrou, jogo que caiu).

use std::path::Path;
use std::time::{Duration, Instant};

use prisma_shader::chain::Engine;
use prisma_shader::link::*;
use prisma_shader::Res;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::System::Memory::*;
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::{held, menu, Press};

#[cfg(debug_assertions)]
const DLL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shader-hook/debug/prisma_shader_hook.dll"));
#[cfg(not(debug_assertions))]
const DLL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shader-hook/release/prisma_shader_hook.dll"));
/// Jogos de 32 bits: a mesma DLL em i686 e o ajudante que a prende no jogo.
#[cfg(debug_assertions)]
const DLL32: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shader-hook/i686-pc-windows-msvc/debug/prisma_shader_hook.dll"));
#[cfg(not(debug_assertions))]
const DLL32: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shader-hook/i686-pc-windows-msvc/release/prisma_shader_hook.dll"));
#[cfg(debug_assertions)]
const HELPER32: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shader-hook/i686-pc-windows-msvc/debug/prisma_hook32.exe"));
#[cfg(not(debug_assertions))]
const HELPER32: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shader-hook/i686-pc-windows-msvc/release/prisma_hook32.exe"));
/// Tempo para a DLL carregar e se prender.
const WAIT_SECS: u64 = 8;
/// Tempo para o jogo carregar a API de vídeo depois de abrir a janela.
const API_WAIT: Duration = Duration::from_secs(10);

/// O gancho que pôs a DLL no jogo: o do motor (64 bits) ou o do ajudante de 32 bits.
enum Hook {
    Own(HHOOK),
    Helper(std::process::Child),
}

/// A memória compartilhada com a DLL e o gancho que a pôs no jogo.
struct Link {
    map: HANDLE,
    view: *const Shared,
    hook: std::cell::RefCell<Option<Hook>>,
}

impl Link {
    /// Abre a memória do jogo (ou cria), passa o perfil e a janela, e carrega a DLL se ela ainda não está lá.
    fn attach(pid: u32, game: HWND, path: &Path, wow: bool) -> Res<Self> {
        unsafe {
            let map = CreateFileMappingW(INVALID_HANDLE_VALUE, None, PAGE_READWRITE, 0, std::mem::size_of::<Shared>() as u32, &HSTRING::from(map_name(pid)))?;
            let view = MapViewOfFile(map, FILE_MAP_ALL_ACCESS, 0, 0, 0);
            if view.Value.is_null() {
                let _ = CloseHandle(map);
                return Err("memória compartilhada".into());
            }
            let me = Self { map, view: view.Value as *const Shared, hook: Default::default() };
            let s = me.s();
            s.hwnd.store(game.0 as u64, Relaxed);
            s.beat.store(GetTickCount64(), Relaxed);
            if s.path() != path && !s.set_path(path) {
                return Err(format!("caminho do perfil longo demais: {}", path.display()).into());
            }
            if s.magic.load(SeqCst) == MAGIC {
                eprintln!("shader: a DLL já está no jogo, reconectando");
            } else {
                let hook = if wow {
                    crate::sys::inject::load_wow(pid, DLL32, HELPER32, "shaders", "prisma_shader_hook").map(Hook::Helper)
                } else {
                    crate::sys::inject::load(pid, DLL, "shaders", "prisma_shader_hook").map(Hook::Own)
                };
                *me.hook.borrow_mut() = Some(hook.ok_or("a DLL não entrou no jogo")?);
            }
            Ok(me)
        }
    }

    fn s(&self) -> &Shared {
        unsafe { &*self.view }
    }

    /// A DLL já se prendeu ao jogo: o gancho pode sair.
    fn unhook(&self) {
        match self.hook.take() {
            Some(Hook::Own(h)) => unsafe {
                let _ = UnhookWindowsHookEx(h);
            },
            // Fim da entrada: o ajudante solta o gancho e sai.
            Some(Hook::Helper(mut c)) => {
                drop(c.stdin.take());
                let _ = c.wait();
            }
            None => {}
        }
    }
}

impl Drop for Link {
    /// Sem sinal de vida a DLL para de desenhar na hora, sem esperar os 5 s.
    fn drop(&mut self) {
        self.s().beat.store(0, Relaxed);
        self.s().flags.store(0, Relaxed);
        self.unhook();
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.view as *mut _ });
            let _ = CloseHandle(self.map);
        }
    }
}

use std::sync::atomic::Ordering::*;

/// Grava no perfil que ele volta para a janela por cima, com o motivo no log.
fn to_overlay(eng: &mut Engine, dev: &ID3D11Device, why: &str) -> Res<bool> {
    eprintln!("shader: injeção não serve neste jogo ({why}); o perfil volta para Por cima");
    eng.edit(dev, |p| p.inject = false);
    Ok(false)
}

fn exit_code(process: HANDLE) -> Option<u32> {
    let mut code = 0;
    unsafe { GetExitCodeProcess(process, &mut code) }.ok()?;
    (code != STILL_ACTIVE.0 as u32).then_some(code)
}

/// Roda o modo Injeção até o jogo fechar (`Ok(true)`) ou até ele não servir (`Ok(false)`: o perfil já voltou
/// para Por cima e quem chamou segue com a janela por cima).
pub(super) fn run(path: &Path, game: HWND) -> Res<bool> {
    // A DLL lê o perfil de dentro do jogo, com outra pasta atual.
    let path = &std::path::absolute(path)?;
    let pid = crate::sys::procs::window_pid(game);
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)? };
    let r = run_in(path, game, pid, process);
    unsafe {
        let _ = CloseHandle(process);
    }
    r
}

fn run_in(path: &Path, game: HWND, pid: u32, process: HANDLE) -> Res<bool> {
    let d = crate::capture::d3d()?;
    // Sem cadeia aqui: o motor só acompanha o perfil para o menu; quem desenha é a DLL.
    let mut eng = Engine::open(path, &d.dev, true)?;
    let mut wow = BOOL(0);
    let wow = unsafe { IsWow64Process(process, &mut wow) }.is_ok() && wow.as_bool();
    let since = Instant::now();
    let mut api = crate::meter::video_api(pid);
    while api.is_empty() && since.elapsed() < API_WAIT {
        std::thread::sleep(Duration::from_millis(500));
        api = crate::meter::video_api(pid);
    }
    eprintln!("shader: API pelos módulos do jogo: {}", if api.is_empty() { "nenhuma ainda" } else { &api });
    if !api.is_empty() && api != eng.profile.api {
        eng.edit(&d.dev, |p| p.api = api.clone());
    }
    if !api.is_empty() && !api_supported(&api) {
        return to_overlay(&mut eng, &d.dev, &api);
    }
    let link = match Link::attach(pid, game, &eng.path, wow) {
        Ok(l) => l,
        Err(e) => return to_overlay(&mut eng, &d.dev, &e.to_string()),
    };
    let keys = crate::shader_profile::Keys::load(&crate::shader_profile::keys_path());
    super::hold_keys(&keys);
    let mut menu = menu::Menu::new();
    let (mut fg, mut menu_key, mut next_key) = (false, Press::default(), Press::default());
    let (since, mut polled, mut ready, mut sent) = (Instant::now(), Instant::now(), None, eng.path.clone());
    // Último Present visto: queda na injeção acontece com o jogo desenhando; a do jogo saindo, depois de parar.
    let (mut presents, mut drawing) = (0, Instant::now());
    let result = loop {
        unsafe {
            MsgWaitForMultipleObjects(None, false, if menu.open || fg { 16 } else { 100 }, QS_ALLINPUT);
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if !unsafe { IsWindow(Some(game)) }.as_bool() {
            // A janela some um pouco antes do processo: espera o código de saída para saber se caiu.
            let idle = drawing.elapsed();
            let mut code = None;
            for _ in 0..20 {
                code = exit_code(process);
                if code.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            // Sem Present há 250 ms o jogo já estava saindo: a queda é dele (o Castlevania Advance Collection cai
            // assim sem o Prisma), e o perfil fica na Injeção.
            if code.is_some_and(crate::sys::inject::is_crash) && idle >= Duration::from_millis(250) {
                eprintln!("shader: o jogo caiu ao sair ({:#x}), {} ms depois do último quadro; o perfil continua na Injeção", code.unwrap(), idle.as_millis());
            } else if code.is_some_and(crate::sys::inject::is_crash) {
                eprintln!("shader: o jogo caiu com a injeção ({:#x}); o perfil volta para Por cima", code.unwrap());
                eng.edit(&d.dev, |p| p.inject = false);
            }
            eprintln!("o jogo fechou");
            break Ok(true);
        }
        let s = link.s();
        if s.presents.load(Relaxed) != presents {
            (presents, drawing) = (s.presents.load(Relaxed), Instant::now());
        }
        s.beat.store(unsafe { GetTickCount64() }, Relaxed);
        match ready {
            None => {
                let state = s.state.load(SeqCst);
                if state == STATE_FAILED || (s.magic.load(SeqCst) != MAGIC && since.elapsed().as_secs() >= WAIT_SECS) {
                    break to_overlay(&mut eng, &d.dev, "a DLL não carregou ou não achou o Present");
                }
                if state == STATE_READY {
                    ready = Some(Instant::now());
                    link.unhook();
                    eprintln!("shader: injeção pronta");
                }
            }
            // Nenhum Present passou pela DLL: o jogo apresenta por um caminho que ela não cobre.
            Some(t) if s.presents.load(Relaxed) == 0 && t.elapsed() >= API_WAIT => {
                break to_overlay(&mut eng, &d.dev, "nenhum quadro passou pela DLL");
            }
            _ => {}
        }
        let seen = api_name(s.api.load(Relaxed));
        if !seen.is_empty() && seen != eng.profile.api {
            eprintln!("shader: o jogo apresenta em {seen}");
            eng.edit(&d.dev, |p| p.api = seen.to_string());
            if !api_supported(seen) {
                break to_overlay(&mut eng, &d.dev, seen);
            }
        }
        if polled.elapsed().as_millis() >= 250 {
            polled = Instant::now();
            eng.poll(&d.dev);
        }
        fg = unsafe { GetForegroundWindow() == game && !IsIconic(game).as_bool() };
        if menu_key.hit(fg && held(keys.menu), false) {
            menu.toggle();
        }
        if next_key.hit(fg && held(keys.next), false) {
            eng.next(&d.dev, 1);
        }
        menu.keys(fg, &mut eng, &d.dev);
        menu.pad(&mut eng, &d.dev);
        menu.update(fg, super::client_area(game).map(|c| c.0), &mut eng);
        // Outro perfil (atalho ou menu): a DLL troca no próximo quadro.
        if eng.path != sent {
            sent = eng.path.clone();
            s.set_path(&sent);
        }
        s.flags.store(if fg && held(keys.compare) { FLAG_COMPARE } else { 0 }, Relaxed);
    };
    super::free_keys();
    result
}
