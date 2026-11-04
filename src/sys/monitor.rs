//! Medições do sistema em uma thread própria. A parte cara, uso de GPU por programa,
//! só roda com a janela principal aberta.

use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Performance::*;
use windows::Win32::System::Power::*;
use windows::Win32::System::SystemInformation::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::etw::{self, Trace};
use super::kmt::Kmt;
use super::nvml::{GpuReading, Nvml};

pub const HISTORY: usize = 60;

#[link(name = "ntdll")]
extern "system" {
    fn NtQuerySystemInformation(class: u32, info: *mut c_void, len: u32, ret: *mut u32) -> i32;
}

#[derive(Clone, Default)]
pub struct Group {
    pub exe: String,
    pub pids: Vec<u32>,
    pub cpu: f32,
    pub ram: u64,
    pub gpu: f32,
    pub vram: u64,
    /// Bytes por segundo.
    pub disk: u64,
    pub net: u64,
}

/// Uma linha da aba Detalhes: um processo, não o programa inteiro.
#[derive(Clone, Default)]
pub struct ProcRow {
    pub pid: u32,
    pub exe: String,
    pub cpu: f32,
    pub ram: u64,
    pub gpu: f32,
    pub vram: u64,
    pub disk: u64,
    pub net: u64,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub cpu: f32,
    pub threads: Vec<f32>,
    pub ghz: f32,
    pub ram_used: u64,
    pub ram_total: u64,
    pub gpu: GpuReading,
    pub gpu_name: String,
    pub hist_cpu: VecDeque<f32>,
    pub hist_gpu: VecDeque<f32>,
    pub hist_ram: VecDeque<f32>,
    pub hist_vram: VecDeque<f32>,
    pub groups: Vec<Group>,
    pub procs: Vec<ProcRow>,
    pub detail: bool,
    /// False quando falta administrador e a coluna de rede não tem dados.
    pub net_ok: bool,
}

impl Snapshot {
    pub fn ram_pct(&self) -> f32 {
        if self.ram_total == 0 {
            0.0
        } else {
            self.ram_used as f32 / self.ram_total as f32 * 100.0
        }
    }
    pub fn vram_pct(&self) -> f32 {
        if self.gpu.total == 0 {
            0.0
        } else {
            self.gpu.used as f32 / self.gpu.total as f32 * 100.0
        }
    }
}

pub static DETAIL: AtomicBool = AtomicBool::new(false);
/// Atualiza a cada segundo, sem as leituras caras do modo detalhado.
pub static FAST: AtomicBool = AtomicBool::new(false);

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CpuTimes {
    idle: i64,
    kernel: i64,
    user: i64,
    dpc: i64,
    interrupt: i64,
    irq_count: u32,
    _pad: u32,
}

struct Proc {
    pid: u32,
    time: i64,
    ram: u64,
    io: u64,
    exe: String,
}

fn read_processes(buf: &mut Vec<u8>, threads: Option<&mut HashMap<u32, u32>>) -> Vec<Proc> {
    unsafe {
        loop {
            let mut ret = 0u32;
            let st = NtQuerySystemInformation(5, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut ret);
            if st as u32 == 0xC0000004 {
                buf.resize(ret as usize + 65536, 0);
                continue;
            }
            if st != 0 {
                return Vec::new();
            }
            break;
        }
        let mut out = Vec::with_capacity(512);
        let mut off = 0usize;
        let mut threads = threads;
        loop {
            let p = buf.as_ptr().add(off);
            let next = (p as *const u32).read_unaligned() as usize;
            let private = (p.add(8) as *const i64).read_unaligned() as u64;
            let user = (p.add(40) as *const i64).read_unaligned();
            let kernel = (p.add(48) as *const i64).read_unaligned();
            let name_len = (p.add(56) as *const u16).read_unaligned() as usize / 2;
            let name_ptr = (p.add(64) as *const *const u16).read_unaligned();
            let pid = (p.add(80) as *const usize).read_unaligned() as u32;
            let reads = (p.add(232) as *const u64).read_unaligned();
            let writes = (p.add(240) as *const u64).read_unaligned();
            if let Some(map) = &mut threads {
                let n = (p.add(4) as *const u32).read_unaligned() as usize;
                for i in 0..n {
                    let tid = (p.add(256 + i * 80 + 48) as *const usize).read_unaligned() as u32;
                    map.insert(tid, pid);
                }
            }
            if pid != 0 {
                let exe = if name_ptr.is_null() {
                    "System".to_string()
                } else {
                    String::from_utf16_lossy(std::slice::from_raw_parts(name_ptr, name_len))
                };
                out.push(Proc { pid, time: user + kernel, ram: private, io: reads + writes, exe });
            }
            if next == 0 {
                break;
            }
            off += next;
        }
        out
    }
}

fn read_cpu(prev: &mut Vec<CpuTimes>, threads: &mut Vec<f32>) -> f32 {
    let n = unsafe {
        let mut si = SYSTEM_INFO::default();
        GetSystemInfo(&mut si);
        si.dwNumberOfProcessors as usize
    };
    let mut cur = vec![CpuTimes::default(); n];
    let size = (n * std::mem::size_of::<CpuTimes>()) as u32;
    let mut ret = 0;
    unsafe {
        if NtQuerySystemInformation(8, cur.as_mut_ptr() as *mut _, size, &mut ret) != 0 {
            return 0.0;
        }
    }
    threads.resize(n, 0.0);
    let (mut idle_all, mut total_all) = (0i64, 0i64);
    if prev.len() == n {
        for i in 0..n {
            let idle = cur[i].idle - prev[i].idle;
            let total = (cur[i].kernel - prev[i].kernel) + (cur[i].user - prev[i].user);
            threads[i] = if total > 0 { 100.0 * (total - idle) as f32 / total as f32 } else { 0.0 };
            idle_all += idle;
            total_all += total;
        }
    }
    *prev = cur;
    if total_all > 0 {
        100.0 * (total_all - idle_all) as f32 / total_all as f32
    } else {
        0.0
    }
}

struct GpuCounters {
    query: PDH_HQUERY,
    engine: PDH_HCOUNTER,
    memory: PDH_HCOUNTER,
    perf: PDH_HCOUNTER,
}

impl GpuCounters {
    fn new() -> Option<Self> {
        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryW(None, 0, &mut query) != 0 {
                return None;
            }
            let mut engine = PDH_HCOUNTER::default();
            let mut memory = PDH_HCOUNTER::default();
            let mut perf = PDH_HCOUNTER::default();
            PdhAddEnglishCounterW(query, w!("\\GPU Engine(*engtype_3D)\\Utilization Percentage"), 0, &mut engine);
            PdhAddEnglishCounterW(query, w!("\\GPU Process Memory(*)\\Dedicated Usage"), 0, &mut memory);
            PdhAddEnglishCounterW(query, w!("\\Processor Information(_Total)\\% Processor Performance"), 0, &mut perf);
            PdhCollectQueryData(query);
            Some(Self { query, engine, memory, perf })
        }
    }

    fn array(counter: PDH_HCOUNTER, buf: &mut Vec<u8>) -> Vec<(String, f64)> {
        let mut out = Vec::new();
        unsafe {
            let mut size = 0u32;
            let mut count = 0u32;
            let st = PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None);
            if st != PDH_MORE_DATA {
                return out;
            }
            buf.resize(size as usize, 0);
            let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            let st = PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items));
            if st != 0 {
                return out;
            }
            for i in 0..count as usize {
                let it = &*items.add(i);
                if it.FmtValue.CStatus != 0 {
                    continue;
                }
                let name = it.szName.to_string().unwrap_or_default();
                out.push((name, it.FmtValue.Anonymous.doubleValue));
            }
        }
        out
    }

    fn read(&self, buf: &mut Vec<u8>) -> (HashMap<u32, f32>, HashMap<u32, u64>, f32) {
        let mut gpu: HashMap<u32, f32> = HashMap::new();
        let mut vram: HashMap<u32, u64> = HashMap::new();
        unsafe { PdhCollectQueryData(self.query) };
        for (name, v) in Self::array(self.engine, buf) {
            if let Some(pid) = instance_pid(&name) {
                *gpu.entry(pid).or_default() += v as f32;
            }
        }
        for (name, v) in Self::array(self.memory, buf) {
            if let Some(pid) = instance_pid(&name) {
                *vram.entry(pid).or_default() += v as u64;
            }
        }
        let mut perf = 0.0;
        unsafe {
            let mut v = PDH_FMT_COUNTERVALUE::default();
            if PdhGetFormattedCounterValue(self.perf, PDH_FMT_DOUBLE, None, &mut v) == 0 {
                perf = v.Anonymous.doubleValue as f32;
            }
        }
        (gpu, vram, perf)
    }
}

impl Drop for GpuCounters {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.query);
        }
    }
}

/// Instâncias têm a forma `pid_1234_luid_0x..._phys_0...`.
fn instance_pid(name: &str) -> Option<u32> {
    name.strip_prefix("pid_")?.split('_').next()?.parse().ok()
}

fn base_mhz() -> f32 {
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Info {
        number: u32,
        max: u32,
        current: u32,
        limit: u32,
        idle_max: u32,
        idle_cur: u32,
    }
    unsafe {
        let mut si = SYSTEM_INFO::default();
        GetSystemInfo(&mut si);
        let mut v = vec![Info::default(); si.dwNumberOfProcessors as usize];
        let size = (v.len() * std::mem::size_of::<Info>()) as u32;
        let st = CallNtPowerInformation(ProcessorInformation, None, 0, Some(v.as_mut_ptr() as *mut _), size);
        if st.is_ok() {
            v[0].max as f32
        } else {
            0.0
        }
    }
}

fn push(h: &mut VecDeque<f32>, v: f32) {
    if h.len() == HISTORY {
        h.pop_front();
    }
    h.push_back(v);
}

pub fn spawn(hub: isize, msg: u32) -> Arc<Mutex<Snapshot>> {
    let shared = Arc::new(Mutex::new(Snapshot::default()));
    let out = shared.clone();
    std::thread::Builder::new()
        .name("monitor".into())
        .spawn(move || run(out, hub, msg))
        .expect("thread do monitor");
    shared
}

fn run(out: Arc<Mutex<Snapshot>>, hub: isize, msg: u32) {
    let mut kmt = Kmt::new();
    let mut nvml: Option<Nvml> = None;
    let mut gpu_name = String::new();
    let mhz = base_mhz();
    let mut prev_cpu = Vec::new();
    let mut threads = Vec::new();
    let mut prev_time: HashMap<u32, i64> = HashMap::new();
    let mut prev_io: HashMap<u32, u64> = HashMap::new();
    let mut trace: Option<Trace> = None;
    let mut trace_tried = false;
    let mut pbuf = vec![0u8; 1 << 20];
    let mut cbuf = Vec::new();
    let mut counters: Option<GpuCounters> = None;
    let mut last = Instant::now();
    let logical = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f32;
    let (mut h_cpu, mut h_gpu, mut h_ram, mut h_vram) =
        (VecDeque::new(), VecDeque::new(), VecDeque::new(), VecDeque::new());
    loop {
        let detail = DETAIL.load(Ordering::Relaxed);
        let cpu = read_cpu(&mut prev_cpu, &mut threads);
        let elapsed = last.elapsed().as_secs_f32().max(0.05);
        last = Instant::now();

        let mut mem = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        unsafe {
            let _ = GlobalMemoryStatusEx(&mut mem);
        }
        // A biblioteca da NVIDIA pesa cerca de 20 MB: só fica carregada com a janela aberta.
        if detail && nvml.is_none() {
            nvml = Nvml::load();
            if let Some(n) = &nvml {
                gpu_name = n.name.clone();
            }
        } else if !detail {
            nvml = None;
        }
        let mut gpu = kmt.as_mut().map(|k| k.read()).unwrap_or_default();
        if let Some(n) = &nvml {
            let extra = n.read();
            gpu.temp = extra.temp;
            gpu.mhz = extra.mhz;
            gpu.total = extra.total;
        }

        if detail && !trace_tried {
            trace_tried = true;
            trace = Trace::start();
        } else if !detail {
            trace = None;
            trace_tried = false;
        }
        let mut tids = HashMap::new();
        let procs = read_processes(&mut pbuf, if trace.is_some() { Some(&mut tids) } else { None });
        let io_now = if trace.is_some() {
            etw::set_threads(tids);
            etw::take()
        } else {
            HashMap::new()
        };
        let mut io_map = HashMap::with_capacity(procs.len());
        let mut groups: HashMap<String, Group> = HashMap::with_capacity(procs.len());
        let mut times = HashMap::with_capacity(procs.len());
        let (mut pgpu, mut pvram, mut perf) = (HashMap::new(), HashMap::new(), 0.0);
        if detail {
            if counters.is_none() {
                counters = GpuCounters::new();
            }
            if let Some(c) = &counters {
                (pgpu, pvram, perf) = c.read(&mut cbuf);
            }
        } else {
            counters = None;
        }
        let mut rows = Vec::with_capacity(if detail { procs.len() } else { 0 });
        for p in procs {
            let before = prev_time.get(&p.pid).copied().unwrap_or(p.time);
            let dt = (p.time - before).max(0) as f32 / 1e7;
            times.insert(p.pid, p.time);
            let g = groups.entry(p.exe.to_ascii_lowercase()).or_default();
            let mut row = ProcRow { pid: p.pid, cpu: dt / elapsed / logical * 100.0, ram: p.ram, ..Default::default() };
            if detail {
                row.exe = p.exe.clone();
            }
            if g.exe.is_empty() {
                g.exe = p.exe;
            }
            g.pids.push(p.pid);
            if detail {
                let done = prev_io.get(&p.pid).copied().unwrap_or(p.io);
                io_map.insert(p.pid, p.io);
                match io_now.get(&p.pid) {
                    Some(v) => {
                        row.net = (v.net as f32 / elapsed) as u64;
                        row.disk = (v.disk as f32 / elapsed) as u64;
                    }
                    None if trace.is_none() => {
                        row.disk = (p.io.saturating_sub(done) as f32 / elapsed) as u64;
                    }
                    None => {}
                }
            }
            row.gpu = pgpu.get(&p.pid).copied().unwrap_or(0.0);
            // O Windows às vezes reporta mais do que a placa tem; isso não é VRAM real.
            let v = pvram.get(&p.pid).copied().unwrap_or(0);
            if gpu.total == 0 || v <= gpu.total {
                row.vram = v;
            }
            g.cpu += row.cpu;
            g.ram += row.ram;
            g.gpu += row.gpu;
            g.vram += row.vram;
            g.net += row.net;
            g.disk += row.disk;
            if detail {
                rows.push(row);
            }
        }
        prev_time = times;
        prev_io = io_map;
        let mut groups: Vec<Group> = groups.into_values().collect();
        groups.sort_by(|a, b| {
            let (x, y) = (a.cpu + a.gpu, b.cpu + b.gpu);
            y.partial_cmp(&x).unwrap().then(b.ram.cmp(&a.ram))
        });

        let ram_used = mem.ullTotalPhys - mem.ullAvailPhys;
        push(&mut h_cpu, cpu);
        push(&mut h_gpu, gpu.util);
        push(&mut h_ram, mem.dwMemoryLoad as f32);
        let vp = if gpu.total > 0 { gpu.used as f32 / gpu.total as f32 * 100.0 } else { 0.0 };
        push(&mut h_vram, vp);
        {
            let mut s = out.lock().unwrap();
            s.cpu = cpu;
            s.threads = threads.clone();
            if detail && perf > 0.0 {
                s.ghz = perf / 100.0 * mhz / 1000.0;
            }
            s.ram_used = ram_used;
            s.ram_total = mem.ullTotalPhys;
            s.gpu = gpu;
            s.gpu_name = gpu_name.clone();
            s.hist_cpu = h_cpu.clone();
            s.hist_gpu = h_gpu.clone();
            s.hist_ram = h_ram.clone();
            s.hist_vram = h_vram.clone();
            s.groups = groups;
            s.procs = rows;
            s.detail = detail;
            s.net_ok = trace.is_some();
        }
        unsafe {
            let _ = PostMessageW(Some(HWND(hub as *mut _)), msg, WPARAM(0), LPARAM(0));
        }
        let fast = detail || FAST.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(if fast { 1000 } else { 2000 }));
    }
}
