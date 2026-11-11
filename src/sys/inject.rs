//! Limitador dentro do jogo. É opcional e por jogo: nunca entra em jogo com anti-cheat detectado.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::*};
use std::time::Instant;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::*;
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
    if std::fs::metadata(&path).map_or(true, |m| m.len() != DLL.len() as u64) {
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
