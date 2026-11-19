//! Limitador e HDR dentro do jogo. São opcionais e por jogo: nunca entram em jogo com anti-cheat detectado.

use std::os::windows::ffi::OsStrExt;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::*};
use std::time::Instant;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::Storage::FileSystem::GetBinaryTypeW;
use windows::Win32::System::Diagnostics::Debug::WriteProcessMemory;
use windows::Win32::System::Memory::*;
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const DLL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/hook/release/prisma_hook.dll"));
const MAGIC: u32 = 0x5052_534d;
const STATE_READY: u32 = 2;
const STATE_FAILED: u32 = 3;
/// Tempo para o módulo carregar e o jogo apresentar um quadro por ele.
const WAIT_SECS: u64 = 8;
const STILL_ACTIVE: u32 = 259;
const SCS_64BIT_BINARY: u32 = 6;
/// Códigos de saída de queda, tirando o Ctrl+C.
const CRASH_MASK: u32 = 0xF000_0000;
const CRASH_CODES: u32 = 0xC000_0000;
const CTRL_C: u32 = 0xC000_013A;

/// Mesmo formato de `hook/src/lib.rs`.
#[repr(C)]
struct Shared {
    magic: AtomicU32,
    state: AtomicU32,
    fps_milli: AtomicU32,
    pad: u32,
    beat: AtomicU64,
    presents: AtomicU64,
    waits: AtomicU64,
    hdr_proxy: AtomicU32,
    hdr_on: AtomicU32,
    hdr_peak: AtomicU32,
    hdr_paper: AtomicU32,
    hdr_gamma: AtomicU32,
    hdr_sat: AtomicU32,
    hdr_boost: AtomicU32,
    hdr_black: AtomicU32,
    hdr_chains: AtomicU32,
    hdr_error: AtomicU32,
    hdr_frames: AtomicU64,
}

const _: () = assert!(std::mem::size_of::<Shared>() == 88);

/// Ajustes do HDR próprio. Pico, paper white e preto em nits; o resto são números puros.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hdr {
    pub on: bool,
    pub peak: f32,
    pub paper: f32,
    pub gamma: f32,
    pub sat: f32,
    pub boost: f32,
    pub black: f32,
}

impl Hdr {
    /// Os limites vêm da calibração da tela; sem perfil, os de uma TV HDR comum.
    pub fn from_profile(p: Option<&crate::config::HdrProfile>) -> Self {
        let (peak, full, black) = p.map_or((1000.0, 400.0, 0.0), |p| (p.peak, p.full, p.black));
        let peak = if peak > 80.0 { peak } else { 1000.0 };
        // Paper white até o brilho de tela inteira branca: o brilho médio não aciona o limitador da TV.
        let paper = 200f32.min(if full > 80.0 { full } else { peak });
        Self { on: true, peak, paper, gamma: 2.2, sat: 1.0, boost: 1.0, black: black.clamp(0.0, 1.0) }
    }
}

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum Status {
    Waiting,
    Ready,
    Failed,
}

pub struct Limiter {
    pub pid: u32,
    process: HANDLE,
    hook: Option<HHOOK>,
    map: HANDLE,
    view: *const Shared,
    since: Instant,
    pub status: Status,
}

/// O arquivo tem nome pelo conteúdo: um jogo aberto com a versão anterior não impede a nova.
fn dll_path() -> Option<std::path::PathBuf> {
    let sum = DLL.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100_0000_01b3));
    let dir = crate::config::data_dir().join("limiter");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("prisma_hook_{sum:016x}.dll"));
    if std::fs::read(&path).map_or(true, |d| d != DLL) {
        std::fs::write(&path, DLL).ok()?;
    }
    Some(path)
}

fn window_thread(pid: u32) -> Option<u32> {
    let list = crate::sys::procs::top_windows();
    let hwnd = list.into_iter().find(|h| crate::sys::procs::window_pid(*h) == pid)?;
    let tid = unsafe { GetWindowThreadProcessId(hwnd, None) };
    (tid != 0).then_some(tid)
}

fn map_name(pid: u32) -> HSTRING {
    HSTRING::from(format!("Local\\PrismaLimiter-{pid}"))
}

impl Limiter {
    /// Começa a carregar no jogo. Se já foi carregado antes, só reconecta.
    pub fn attach(pid: u32) -> Option<Self> {
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut me = Self { pid, process, hook: None, map: HANDLE::default(), view: std::ptr::null(), since: Instant::now(), status: Status::Waiting };
            if !me.open() {
                let path = dll_path()?;
                let tid = window_thread(pid)?;
                let module = LoadLibraryExW(&HSTRING::from(path.as_path()), None, DONT_RESOLVE_DLL_REFERENCES).ok()?;
                let proc = GetProcAddress(module, s!("prisma_hook"))?;
                let hook: HOOKPROC = Some(std::mem::transmute(proc));
                me.hook = SetWindowsHookExW(WH_GETMESSAGE, hook, Some(module.into()), tid).ok();
                let _ = FreeLibrary(module);
                me.hook?;
                let _ = PostThreadMessageW(tid, WM_NULL, WPARAM(0), LPARAM(0));
            }
            Some(me)
        }
    }

    fn open(&mut self) -> bool {
        unsafe {
            let Ok(map) = OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, &map_name(self.pid)) else { return false };
            let view = MapViewOfFile(map, FILE_MAP_ALL_ACCESS, 0, 0, 0);
            if view.Value.is_null() {
                let _ = CloseHandle(map);
                return false;
            }
            self.map = map;
            self.view = view.Value as *const Shared;
            true
        }
    }

    fn shared(&self) -> Option<&Shared> {
        unsafe { self.view.as_ref() }.filter(|s| s.magic.load(SeqCst) == MAGIC)
    }

    /// Abre o jogo parado, carrega a DLL com o HDR pedido e só então solta: a cadeia de troca nasce
    /// já com o proxy. Se a DLL não carregar, o jogo abre normal, sem HDR.
    pub fn launch(exe: &str, hdr: &Hdr) -> std::result::Result<Self, String> {
        if crate::meter::anti_cheat(exe) {
            return Err("anti-cheat detectado".into());
        }
        unsafe {
            let mut kind = 0;
            if GetBinaryTypeW(&HSTRING::from(exe), &mut kind).is_err() || kind != SCS_64BIT_BINARY {
                return Err("o jogo não é 64 bits".into());
            }
            let path = dll_path().ok_or("sem a DLL")?;
            let dir = std::path::Path::new(exe).parent().ok_or("caminho sem pasta")?;
            let si = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
            let mut pi = PROCESS_INFORMATION::default();
            let mut line: Vec<u16> = format!("\"{exe}\"").encode_utf16().chain(Some(0)).collect();
            // Aberto fora da Steam, o jogo pode se reabrir por ela, e a cópia nova viria sem a DLL. Com o
            // número do jogo no ambiente, como a Steam faz, ele segue nesta cópia.
            let mut env: Vec<u16> = Vec::new();
            if let Some(id) = super::stores::steam_app_id(std::path::Path::new(exe)) {
                for (k, v) in std::env::vars().filter(|(k, _)| !k.eq_ignore_ascii_case("SteamAppId") && !k.eq_ignore_ascii_case("SteamGameId")).chain([("SteamAppId".into(), id.clone()), ("SteamGameId".into(), id)]) {
                    env.extend(format!("{k}={v}").encode_utf16().chain(Some(0)));
                }
                env.push(0);
            }
            let block = (!env.is_empty()).then_some(env.as_ptr() as *const std::ffi::c_void);
            CreateProcessW(&HSTRING::from(exe), Some(PWSTR(line.as_mut_ptr())), None, None, false, CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT, block, &HSTRING::from(dir), &si, &mut pi).map_err(|e| e.message())?;
            let mut me = Self { pid: pi.dwProcessId, process: pi.hProcess, hook: None, map: HANDLE::default(), view: std::ptr::null(), since: Instant::now(), status: Status::Waiting };
            // A memória nasce aqui, com o HDR já pedido: a DLL só abre a mesma.
            if let Ok(map) = CreateFileMappingW(INVALID_HANDLE_VALUE, None, PAGE_READWRITE, 0, std::mem::size_of::<Shared>() as u32, &map_name(me.pid)) {
                me.map = map;
                me.view = MapViewOfFile(map, FILE_MAP_ALL_ACCESS, 0, 0, 0).Value as *const Shared;
            }
            if let Some(s) = me.view.as_ref() {
                s.hdr_proxy.store(1, SeqCst);
                me.set_hdr(hdr);
                if load_remote(pi.hProcess, &path) {
                    let until = Instant::now() + std::time::Duration::from_secs(WAIT_SECS);
                    while !matches!(s.state.load(SeqCst), STATE_READY | STATE_FAILED) && Instant::now() < until {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                }
            }
            ResumeThread(pi.hThread);
            let _ = CloseHandle(pi.hThread);
            Ok(me)
        }
    }

    /// Muda os ajustes ao vivo: o jogo lê a cada quadro.
    pub fn set_hdr(&self, h: &Hdr) {
        let Some(s) = (unsafe { self.view.as_ref() }) else { return };
        s.hdr_on.store(h.on as u32, Relaxed);
        for (at, v) in [(&s.hdr_peak, h.peak), (&s.hdr_paper, h.paper), (&s.hdr_gamma, h.gamma), (&s.hdr_sat, h.sat), (&s.hdr_boost, h.boost), (&s.hdr_black, h.black)] {
            at.store(v.to_bits(), Relaxed);
        }
    }

    /// Cadeias com HDR, quadros desenhados por ele e o último erro (HRESULT) visto no jogo.
    pub fn hdr_stats(&self) -> Option<(u32, u64, u32)> {
        let s = self.shared()?;
        Some((s.hdr_chains.load(Relaxed), s.hdr_frames.load(Relaxed), s.hdr_error.load(Relaxed)))
    }

    /// Chamado uma vez por segundo: dá sinal de vida, passa o limite e confere se pegou.
    pub fn update(&mut self, fps: u32) {
        if self.view.is_null() {
            self.open();
        }
        let timed_out = self.since.elapsed().as_secs() >= WAIT_SECS;
        let Some(s) = self.shared() else {
            if timed_out {
                self.status = Status::Failed;
            }
            return;
        };
        s.beat.store(unsafe { GetTickCount64() }, Relaxed);
        s.fps_milli.store(fps * 1000, Relaxed);
        let (state, presents) = (s.state.load(SeqCst), s.presents.load(Relaxed));
        self.status = match self.status {
            Status::Waiting if state == STATE_FAILED => Status::Failed,
            Status::Waiting if state == STATE_READY && presents > 0 => Status::Ready,
            Status::Waiting if timed_out => Status::Failed,
            other => other,
        };
        if self.status != Status::Waiting {
            if let Some(h) = self.hook.take() {
                unsafe {
                    let _ = UnhookWindowsHookEx(h);
                }
            }
        }
    }

    /// O limite em vigor no jogo, quando o limitador engatou.
    pub fn limit(&self) -> Option<u32> {
        let s = self.shared().filter(|_| self.status == Status::Ready)?;
        Some(s.fps_milli.load(Relaxed) / 1000).filter(|v| *v > 0)
    }

    /// Código de saída, se o jogo já fechou.
    pub fn exit_code(&self) -> Option<u32> {
        let mut code = 0;
        unsafe { GetExitCodeProcess(self.process, &mut code) }.ok()?;
        (code != STILL_ACTIVE).then_some(code)
    }
}

/// `LoadLibraryW` numa linha nova do jogo, que ainda está parado. O kernel32 fica no mesmo endereço
/// em todo processo da sessão, por isso o endereço daqui vale lá.
unsafe fn load_remote(process: HANDLE, path: &std::path::Path) -> bool {
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let size = name.len() * 2;
    let at = VirtualAllocEx(process, None, size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    if at.is_null() {
        return false;
    }
    let mut ok = false;
    if WriteProcessMemory(process, at, name.as_ptr() as *const _, size, None).is_ok() {
        let load = GetModuleHandleW(w!("kernel32.dll")).ok().and_then(|k| GetProcAddress(k, s!("LoadLibraryW")));
        if let Some(load) = load {
            if let Ok(t) = CreateRemoteThread(process, None, 0, Some(std::mem::transmute(load)), Some(at), 0, None) {
                let mut code = 0;
                ok = WaitForSingleObject(t, WAIT_SECS as u32 * 1000) == WAIT_OBJECT_0 && GetExitCodeThread(t, &mut code).is_ok() && code != 0;
                let _ = CloseHandle(t);
            }
        }
    }
    // Com a linha ainda presa (tempo esgotado), a memória fica: soltar agora derrubaria o jogo.
    if ok {
        let _ = VirtualFreeEx(process, at, 0, MEM_RELEASE);
    }
    ok
}

pub fn is_crash(code: u32) -> bool {
    code & CRASH_MASK == CRASH_CODES && code != CTRL_C
}

impl Drop for Limiter {
    fn drop(&mut self) {
        unsafe {
            if let Some(s) = self.shared() {
                s.fps_milli.store(0, Relaxed);
            }
            if let Some(h) = self.hook.take() {
                let _ = UnhookWindowsHookEx(h);
            }
            if !self.view.is_null() {
                let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.view as *mut _ });
                let _ = CloseHandle(self.map);
            }
            let _ = CloseHandle(self.process);
        }
    }
}

/// Teste do HDR: abre o jogo com HDR e, a cada segundo até ele fechar, relê os ajustes de um
/// arquivo (`on=1`, `peak=`, `paper=`, `gamma=`, `sat=`, `boost=`, `black=`, um por linha) e grava o estado.
#[cfg(debug_assertions)]
pub fn test_hdr(out: &str, exe: &str, params: &str) {
    let cfg = crate::config::Config::load();
    let mut hdr = Hdr::from_profile(cfg.hdr_profiles.last());
    let mut l = match Limiter::launch(exe, &hdr) {
        Ok(l) => l,
        Err(e) => {
            let _ = std::fs::write(out, format!("não abriu: {e}\n"));
            return;
        }
    };
    let mut log = format!("pid {} perfil {:?}\n", l.pid, cfg.hdr_profiles.last().map(|p| (&p.screen, p.peak, p.full, p.black)));
    let mut i = 0;
    while l.exit_code().is_none() {
        std::thread::sleep(std::time::Duration::from_secs(1));
        for line in std::fs::read_to_string(params).unwrap_or_default().lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let Ok(v) = v.trim().parse::<f32>() else { continue };
            match k.trim() {
                "on" => hdr.on = v != 0.0,
                "peak" => hdr.peak = v,
                "paper" => hdr.paper = v,
                "gamma" => hdr.gamma = v,
                "sat" => hdr.sat = v,
                "boost" => hdr.boost = v,
                "black" => hdr.black = v,
                _ => {}
            }
        }
        l.set_hdr(&hdr);
        l.update(0);
        log += &format!("{i} s: {:?} hdr {:?} {hdr:?}\n", l.status, l.hdr_stats().map(|(c, f, e)| (c, f, format!("{e:#x}"))));
        // A cada 5 s, a tela de verdade: a luminância que saiu e uma foto em SDR.
        if i % 5 == 4 {
            if let Ok(f) = crate::capture::grab(crate::capture::cursor_monitor()) {
                if let Some(h) = &f.half {
                    log += &format!("   nits maior, média, p99: {:?}\n", crate::capture::half_nits(h));
                }
                let _ = crate::capture::save_png(&format!("{out}.png"), f.w, f.h, &f.bgra);
            }
        }
        let _ = std::fs::write(out, &log);
        i += 1;
    }
    log += &format!("saiu: {:?}\n", l.exit_code().map(|c| format!("{c:#x}")));
    let _ = std::fs::write(out, &log);
}

/// Teste: carrega num processo, limita e devolve o que aconteceu.
#[cfg(debug_assertions)]
pub fn test(pid: u32, fps: u32, secs: u64) -> String {
    let Some(mut l) = Limiter::attach(pid) else { return "nao carregou".to_string() };
    let mut out = String::new();
    for i in 0..secs {
        std::thread::sleep(std::time::Duration::from_secs(1));
        l.update(if i < secs / 2 { fps } else { fps / 2 });
        let s = l.shared().map(|s| (s.state.load(SeqCst), s.presents.load(Relaxed), s.waits.load(Relaxed)));
        out += &format!("{i} s: {:?} estado {s:?}\n", l.status);
    }
    out
}
