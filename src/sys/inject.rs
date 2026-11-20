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
    hdr_ui: AtomicU32,
    hdr_split: AtomicU32,
    hdr_native: AtomicU32,
    hdr_gpu_us: AtomicU32,
    hdr_split_frames: AtomicU64,
    hdr_src_peak: AtomicU32,
    hdr_src_black: AtomicU32,
    hdr_src_paper: AtomicU32,
    hdr_scan: AtomicU32,
    scan_gen: AtomicU32,
    scan_frames: AtomicU32,
    scan_peak: AtomicU32,
    scan_p999: AtomicU32,
    scan_p99: AtomicU32,
    scan_med: AtomicU32,
    scan_black: AtomicU32,
    scan_ui: AtomicU32,
    scan_white: AtomicU32,
    scan_mode: AtomicU32,
    hdr_pq: AtomicU32,
    hdr_untold: AtomicU32,
}

const _: () = assert!(std::mem::size_of::<Shared>() == 176);

/// `Shared::hdr_split` do gancho: um passe de tela cheia depois da interface juntou tudo de novo.
pub const SPLIT_LATE: u32 = 2;

/// Ajustes do HDR próprio. Pico, paper white e preto em nits; o resto são números puros.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hdr {
    pub on: bool,
    pub peak: f32,
    pub paper: f32,
    pub gamma: f32,
    pub sat: f32,
    pub boost: f32,
    pub black: f32,
    /// Brilho da interface em nits; zero deixa a interface junto com a cena.
    pub ui: f32,
    /// O que o jogo em HDR entrega, pelo Analisar: pico, preto e branco da interface em nits. Zero é não
    /// medido: o gancho usa o que a tela informa.
    pub src_peak: f32,
    pub src_black: f32,
    pub src_paper: f32,
}

/// O que o Analisar mediu, em nits: pico e preto (o mais claro e o mais escuro dos quadros), p99,9 e p99 do
/// maior canal, mediana da cena, brilho da interface (0 sem ela separada), a fração da cena em branco de
/// papel ou acima e a saída do jogo (0 SDR, 1 HDR10, 2 scRGB).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scan {
    pub frames: u32,
    pub peak: f32,
    pub p999: f32,
    pub p99: f32,
    pub med: f32,
    pub black: f32,
    pub ui: f32,
    pub white: f32,
    pub mode: u32,
}

impl Hdr {
    /// Os limites vêm da calibração da tela; sem perfil, os de uma TV HDR comum.
    pub fn from_profile(p: Option<&crate::config::HdrProfile>) -> Self {
        let (peak, full, black) = p.map_or((1000.0, 400.0, 0.0), |p| (p.peak, p.full, p.black));
        let peak = if peak > 80.0 { peak } else { 1000.0 };
        // Paper white até o brilho de tela inteira branca: o brilho médio não aciona o limitador da TV.
        let paper = 200f32.min(if full > 80.0 { full } else { peak });
        Self { on: true, peak, paper, gamma: 2.2, sat: 1.0, boost: 1.0, black: black.clamp(0.0, 1.0), ..Self::default() }
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
fn dll_path(dll: &[u8], dir: &str, name: &str, ext: &str) -> Option<std::path::PathBuf> {
    let sum = dll.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100_0000_01b3));
    let dir = crate::config::data_dir().join(dir);
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{name}_{sum:016x}.{ext}"));
    if std::fs::read(&path).map_or(true, |d| d != dll) {
        std::fs::write(&path, dll).ok()?;
    }
    Some(path)
}

/// Põe a DLL `dll` (gravada em `data_dir\<dir>`) na linha da janela do jogo pelo gancho exportado `name`. O
/// gancho volta para quem chamou: solte depois que a DLL se prender (ela se prende sozinha ao carregar).
pub fn load(pid: u32, dll: &[u8], dir: &str, name: &str) -> Option<HHOOK> {
    let path = dll_path(dll, dir, name, "dll")?;
    let tid = window_thread(pid)?;
    let export = std::ffi::CString::new(name).ok()?;
    unsafe {
        let module = LoadLibraryExW(&HSTRING::from(path.as_path()), None, DONT_RESOLVE_DLL_REFERENCES).ok()?;
        let hook = GetProcAddress(module, PCSTR(export.as_ptr() as *const u8)).and_then(|proc| {
            let hook: HOOKPROC = Some(std::mem::transmute(proc));
            SetWindowsHookExW(WH_GETMESSAGE, hook, Some(module.into()), tid).ok()
        });
        let _ = FreeLibrary(module);
        let _ = PostThreadMessageW(tid, WM_NULL, WPARAM(0), LPARAM(0));
        hook
    }
}

/// Jogo de 32 bits: quem põe o gancho é o ajudante de 32 bits `helper` (também gravado em `data_dir\<dir>`).
/// Ele fica de pé até a entrada dele fechar: solte (`drop(child.stdin.take())`) depois que a DLL se prender.
pub fn load_wow(pid: u32, dll: &[u8], helper: &[u8], dir: &str, name: &str) -> Option<std::process::Child> {
    use std::io::BufRead;
    use std::process::{Command, Stdio};
    let path = dll_path(dll, dir, &format!("{name}32"), "dll")?;
    let exe = dll_path(helper, dir, "prisma_hook32", "exe")?;
    let tid = window_thread(pid)?;
    let mut child = Command::new(exe).arg(&path).arg(name).arg(tid.to_string()).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().ok()?;
    // A resposta em outra linha: um ajudante preso (antivírus olhando o exe recém-gravado, jogo parado) não
    // segura o motor; no estouro ele é fechado e quem chamou volta para Por cima.
    let (tx, rx) = std::sync::mpsc::channel();
    if let Some(o) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut line = String::new();
            let ok = std::io::BufReader::new(o).read_line(&mut line).is_ok() && line.trim() == "ok";
            let _ = tx.send(ok);
        });
    }
    if rx.recv_timeout(std::time::Duration::from_secs(WAIT_SECS)) == Ok(true) {
        return Some(child);
    }
    let _ = child.kill();
    let _ = child.wait();
    None
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
                me.hook = Some(load(pid, DLL, "limiter", "prisma_hook")?);
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
    /// já com o proxy. Se a DLL não carregar, o jogo abre normal, sem HDR. Quem chama confere o anti-cheat
    /// (`autohdr::allowed`).
    pub fn launch(exe: &str, args: &str, hdr: &Hdr) -> std::result::Result<Self, String> {
        unsafe {
            let mut kind = 0;
            if GetBinaryTypeW(&HSTRING::from(exe), &mut kind).is_err() || kind != SCS_64BIT_BINARY {
                return Err("o jogo não é 64 bits".into());
            }
            let path = dll_path(DLL, "limiter", "prisma_hook", "dll").ok_or("sem a DLL")?;
            let dir = std::path::Path::new(exe).parent().ok_or("caminho sem pasta")?;
            let si = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
            let mut pi = PROCESS_INFORMATION::default();
            let mut line: Vec<u16> = format!("\"{exe}\" {args}").trim_end().encode_utf16().chain(Some(0)).collect();
            // Aberto fora da Steam, o jogo pode se reabrir por ela, e a cópia nova viria sem a DLL. Com o
            // número do jogo no ambiente, como a Steam faz, ele segue nesta cópia.
            let mut env: Vec<u16> = Vec::new();
            if let Some(id) = super::stores::steam_app_id(std::path::Path::new(exe)) {
                // `vars_os`: `vars` derruba o Prisma com uma variável que não seja Unicode válido.
                let steam = |k: &std::ffi::OsStr| k.eq_ignore_ascii_case("SteamAppId") || k.eq_ignore_ascii_case("SteamGameId");
                for (k, v) in std::env::vars_os().filter(|(k, _)| !steam(k)).chain([("SteamAppId".into(), id.clone().into()), ("SteamGameId".into(), id.into())]) {
                    env.extend(k.encode_wide().chain(Some(b'=' as u16)).chain(v.encode_wide()).chain(Some(0)));
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
        if let Some(s) = unsafe { self.view.as_ref() } {
            write_hdr(s, h);
        }
    }

    /// Cadeias com HDR, quadros desenhados por ele e o último erro (HRESULT) visto no jogo.
    pub fn hdr_stats(&self) -> Option<(u32, u64, u32)> {
        let s = self.shared()?;
        Some((s.hdr_chains.load(Relaxed), s.hdr_frames.load(Relaxed), s.hdr_error.load(Relaxed)))
    }

    /// Saída do jogo (0 SDR, 1 HDR10, 2 scRGB), como saiu a interface no último quadro (0 nada desenhado
    /// depois da cena, 1 separada, 2 tela cheia depois da interface, 3 desligada) e quadros com ela separada.
    pub fn hdr_split(&self) -> Option<(u32, u32, u64)> {
        let s = self.shared()?;
        Some((s.hdr_native.load(Relaxed), s.hdr_split.load(Relaxed), s.hdr_split_frames.load(Relaxed)))
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
                // O código de saída é o HMODULE cortado em 32 bits e pode dar zero: quem diz se carregou é o estado na memória.
                ok = WaitForSingleObject(t, WAIT_SECS as u32 * 1000) == WAIT_OBJECT_0;
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

fn write_hdr(s: &Shared, h: &Hdr) {
    s.hdr_on.store(h.on as u32, Relaxed);
    for (at, v) in [(&s.hdr_peak, h.peak), (&s.hdr_paper, h.paper), (&s.hdr_gamma, h.gamma), (&s.hdr_sat, h.sat), (&s.hdr_boost, h.boost), (&s.hdr_black, h.black), (&s.hdr_ui, h.ui), (&s.hdr_src_peak, h.src_peak), (&s.hdr_src_black, h.src_black), (&s.hdr_src_paper, h.src_paper)] {
        at.store(v.to_bits(), Relaxed);
    }
}

/// Os ajustes do HDR de um jogo que o Prisma abriu com ele (menu dentro do jogo): só a memória
/// compartilhada, sem gancho e sem mexer no limite de FPS de quem mais estiver ligado ao jogo.
pub struct HdrLive {
    map: HANDLE,
    view: *const Shared,
}

impl HdrLive {
    pub fn connect(pid: u32) -> Option<Self> {
        unsafe {
            let map = OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, &map_name(pid)).ok()?;
            let view = MapViewOfFile(map, FILE_MAP_ALL_ACCESS, 0, 0, 0);
            let me = Self { map, view: view.Value as *const Shared };
            let s = me.view.as_ref()?;
            (s.magic.load(SeqCst) == MAGIC && s.hdr_proxy.load(SeqCst) != 0).then_some(me)
        }
    }

    fn shared(&self) -> &Shared {
        // Só existe com a vista aberta (`connect`).
        unsafe { &*self.view }
    }

    pub fn get(&self) -> Hdr {
        let s = self.shared();
        let f = |a: &AtomicU32| f32::from_bits(a.load(Relaxed));
        Hdr { on: s.hdr_on.load(Relaxed) != 0, peak: f(&s.hdr_peak), paper: f(&s.hdr_paper), gamma: f(&s.hdr_gamma), sat: f(&s.hdr_sat), boost: f(&s.hdr_boost), black: f(&s.hdr_black), ui: f(&s.hdr_ui), src_peak: f(&s.hdr_src_peak), src_black: f(&s.hdr_src_black), src_paper: f(&s.hdr_src_paper) }
    }

    /// Saída do jogo no último quadro (0 SDR, 1 HDR10, 2 scRGB) e quadros desenhados pelo passe do HDR.
    pub fn native(&self) -> (u32, u64) {
        let s = self.shared();
        (s.hdr_native.load(Relaxed), s.hdr_frames.load(Relaxed))
    }

    /// O último quadro veio de um buffer de 10 bits sem espaço de cor: SDR ou HDR10, o Prisma decide.
    pub fn untold(&self) -> bool {
        self.shared().hdr_untold.load(Relaxed) != 0
    }

    /// Lê esse buffer como HDR10.
    pub fn set_pq(&self, on: bool) {
        self.shared().hdr_pq.store(on as u32, Relaxed);
    }

    /// A medida do Analisar ligada agora; 0 nenhuma.
    pub fn scanning(&self) -> u32 {
        self.shared().hdr_scan.load(Relaxed)
    }

    /// Liga o Analisar com a medida `gen` (diferente da anterior); 0 desliga.
    pub fn scan_set(&self, gen: u32) {
        self.shared().hdr_scan.store(gen, Relaxed);
    }

    /// O que a medida `gen` somou até agora; nada antes do primeiro quadro que conta.
    pub fn scan(&self, gen: u32) -> Option<Scan> {
        let s = self.shared();
        if s.scan_gen.load(Acquire) != gen {
            return None;
        }
        let f = |a: &AtomicU32| f32::from_bits(a.load(Relaxed));
        Some(Scan { frames: s.scan_frames.load(Relaxed), peak: f(&s.scan_peak), p999: f(&s.scan_p999), p99: f(&s.scan_p99), med: f(&s.scan_med), black: f(&s.scan_black), ui: f(&s.scan_ui), white: f(&s.scan_white), mode: s.scan_mode.load(Relaxed) })
    }

    pub fn set(&self, h: &Hdr) {
        write_hdr(self.shared(), h);
    }

    /// Tempo de GPU do passe do HDR em ms (média), com ele agindo no jogo: curva ligada e uma cadeia com proxy
    /// medida, inclusive quando o jogo já sai em HDR e o Prisma reajusta. O gancho zera o tempo quando a última
    /// cadeia com proxy sai; os primeiros quadros, ainda sem medida, também ficam de fora.
    pub fn gpu_ms(&self) -> Option<f64> {
        let s = self.shared();
        let us = s.hdr_gpu_us.load(Relaxed);
        (s.hdr_on.load(Relaxed) != 0 && us > 0).then(|| us as f64 / 1000.0)
    }
}

impl Drop for HdrLive {
    fn drop(&mut self) {
        unsafe {
            if !self.view.is_null() {
                let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.view as *mut _ });
            }
            let _ = CloseHandle(self.map);
        }
    }
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
/// arquivo (`on=1`, `peak=`, `paper=`, `gamma=`, `sat=`, `boost=`, `black=`, `ui=`, um por linha) e grava o estado.
#[cfg(debug_assertions)]
pub fn test_hdr(out: &str, exe: &str, params: &str, args: &str) {
    let cfg = crate::config::Config::load();
    let mut hdr = crate::sys::autohdr::calibrated(&cfg, &crate::sys::autohdr::primary_gdi());
    let mut l = match Limiter::launch(exe, args, &hdr) {
        Ok(l) => l,
        Err(e) => {
            let _ = std::fs::write(out, format!("não abriu: {e}\n"));
            return;
        }
    };
    let mut log = format!("pid {} calibração {:?}\n", l.pid, (hdr.peak, hdr.paper, hdr.black));
    let (mut i, mut scan) = (0, 0);
    // native=N: o HDR próprio passa pela detecção do app (autohdr::watch) em vez do Analisar à mão.
    let mut native = None::<u8>;
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
                "ui" => hdr.ui = v,
                // Analisar: um número novo começa outra medida, 0 para.
                "scan" => scan = v as u32,
                "native" if native.is_none() => native = Some(v as u8),
                _ => {}
            }
        }
        l.set_hdr(&hdr);
        l.update(0);
        log += &format!("{i} s: {:?} hdr {:?} saída, interface, quadros separados {:?} {hdr:?}\n", l.status, l.hdr_stats().map(|(c, f, e)| (c, f, format!("{e:#x}"))), l.hdr_split());
        if let Some(live) = HdrLive::connect(l.pid) {
            match native {
                Some(n) => {
                    if let Some(new) = crate::sys::autohdr::watch(&live, n) {
                        native = Some(new);
                    }
                    log += &format!("   HDR próprio {native:?} sem espaço de cor {} análise {:#x}\n", live.untold(), live.scanning());
                }
                None => live.scan_set(scan),
            }
            log += &format!("   saída do jogo, quadros {:?} gpu {:?} ms análise {:?}\n", live.native(), live.gpu_ms(), live.scan(if native.is_some() { live.scanning() } else { scan }));
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HdrProfile;

    #[test]
    fn hdr_from_profile() {
        let p = |peak, full, black| HdrProfile { peak, full, black, ..Default::default() };
        // Sem perfil: TV comum, paper white 200.
        let h = Hdr::from_profile(None);
        assert_eq!((h.peak, h.paper, h.black), (1000.0, 200.0, 0.0));
        // Paper white nunca passa do brilho de tela cheia; preto fica entre 0 e 1.
        let h = Hdr::from_profile(Some(&p(600.0, 150.0, 3.0)));
        assert_eq!((h.peak, h.paper, h.black), (600.0, 150.0, 1.0));
        // Medida sem tela cheia usa o pico; pico ruim volta ao padrão.
        let h = Hdr::from_profile(Some(&p(50.0, 0.0, 0.05)));
        assert_eq!((h.peak, h.paper, h.black), (1000.0, 200.0, 0.05));
    }
}
