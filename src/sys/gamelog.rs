//! Registro detalhado de uma sessão de jogo, em Documentos. É opcional: sem ligar, nada disto roda.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::thread::JoinHandle;
use std::time::Instant;

use windows::Win32::Foundation::*;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::System::Performance::QueryPerformanceFrequency;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Shell::*;

use super::frames::Stats;
use super::monitor::Snapshot;
use crate::i18n::tr;

/// Abaixo disto uma thread não entra no arquivo, em % de um núcleo.
const THREAD_MIN: f32 = 0.5;
const THREADS_SHOWN: usize = 12;
const WORST_SHOWN: usize = 10;
/// Daqui para cima, a leitura final aponta o gargalo.
const BUSY: f32 = 90.0;

/// Um segundo de jogo, montado pelo medidor e gravado fora da linha principal.
pub struct Second {
    pub stats: Stats,
    pub snap: Snapshot,
    pub frames: Vec<i64>,
    pub pid: u32,
    /// Quantas vezes a geração de quadros multiplica o FPS neste segundo (0: desligada ou sem como saber).
    pub gen: u32,
}

enum Msg {
    Second(Box<Second>),
    End(String),
}

pub struct Log {
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
    pub dir: PathBuf,
    /// Segundos com quadros: é tempo de jogo, não de relógio.
    pub seen: u32,
    pub limit: Option<u32>,
    /// Segundos com a geração de quadros ligada, e a maior multiplicação vista.
    pub gen_secs: u32,
    pub gen_max: u32,
    /// O Lossless Scaling esteve aberto durante a gravação.
    pub lossless: bool,
}

/// Documentos do Windows, que pode estar em outra pasta ou no OneDrive.
pub fn root() -> PathBuf {
    let docs = unsafe {
        SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None).ok().and_then(|p| {
            let text = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const _));
            text
        })
    };
    let base = docs.unwrap_or_else(|| format!("{}\\Documents", std::env::var("USERPROFILE").unwrap_or_default()));
    PathBuf::from(base).join("Prisma").join("Logs")
}

#[derive(Default)]
struct ThreadTotal {
    name: String,
    sum: f32,
    max: f32,
    seen: u32,
}

struct Writer {
    summary: PathBuf,
    seconds: BufWriter<File>,
    frames: BufWriter<File>,
    threads: BufWriter<File>,
    read_threads: bool,
    begin: Instant,
    ms_per_tick: f64,
    first: Option<i64>,
    last: i64,
    frame_ms: Vec<f32>,
    frame_at: Vec<f32>,
    rows: u32,
    cpu: f32,
    gpu: f32,
    cores: Vec<f32>,
    vram: u64,
    ram: u64,
    times: HashMap<u32, (u64, u64)>,
    sampled: Option<Instant>,
    totals: HashMap<u32, ThreadTotal>,
    /// Somas da GPU para o resumo: controlador de memória, energia em % do limite e
    /// segundos com o clock segurado por cada motivo.
    mem_util: f32,
    power_pct: f32,
    reasons: [u32; super::nvml::REASONS.len()],
    vram_total: u64,
}

fn opt(v: Option<f32>) -> String {
    v.map_or(String::new(), |v| format!("{v:.2}"))
}

fn gb(b: u64) -> f32 {
    b as f32 / (1u64 << 30) as f32
}

fn mb(b: u64) -> f32 {
    b as f32 / (1u64 << 20) as f32
}

fn filetime(t: FILETIME) -> u64 {
    (t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64
}

/// Nome que o próprio jogo deu à thread, quando deu.
unsafe fn thread_name(h: HANDLE) -> String {
    let Ok(p) = GetThreadDescription(h) else { return String::new() };
    let text = p.to_string().unwrap_or_default();
    let _ = LocalFree(Some(HLOCAL(p.0 as *mut _)));
    text.replace([',', '\n', '\r'], " ")
}

impl Writer {
    fn second(&mut self, s: &Second) {
        let t = self.begin.elapsed().as_secs_f32();
        if self.rows == 0 {
            let cores: String = (0..s.snap.threads.len()).map(|i| format!(",core{i}_pct")).collect();
            let _ = writeln!(self.seconds, "t_s,fps,frametime_ms,worst_ms,dev_ms,avg_fps,low1_fps,low01_fps,real_fps,latency_ms,frame_gen_x,direct_flip,counted_by_flip,spike,frames_kept,cpu_pct,cpu_ghz,gpu_pct,gpu_mhz,gpu_mem_mhz,gpu_c,gpu_w,fan_pct,vram_gb,ram_gb,game_cpu_pct,game_ram_gb,game_gpu_pct,game_vram_gb,game_disk_mb_s,game_net_mb_s,gpu_mem_ctrl_pct,gpu_power_limit_w,gpu_power_pct,gpu_clock_limited_by,gpu_max_mhz,pcie_tx_mb_s,pcie_rx_mb_s,vram_total_gb{cores}");
            let _ = writeln!(self.frames, "t_ms,frametime_ms");
            let _ = writeln!(self.threads, "t_s,tid,name,cpu_pct_of_core,kernel_pct_of_core");
        }
        let (st, sn) = (&s.stats, &s.snap);
        let group = sn.groups.iter().find(|g| g.pids.contains(&s.pid));
        let g = group.cloned().unwrap_or_default();
        let cores: String = sn.threads.iter().map(|v| format!(",{v:.1}")).collect();
        let gp = &sn.gpu;
        let power_pct = if gp.power_limit > 0.0 { gp.watts / gp.power_limit * 100.0 } else { 0.0 };
        let limited: Vec<&str> = super::nvml::REASONS.iter().filter(|r| gp.reasons & r.0 != 0).map(|r| r.1).collect();
        let extra = format!(
            ",{},{:.1},{:.1},{},{},{:.1},{:.1},{:.2}",
            gp.mem_util,
            gp.power_limit,
            power_pct,
            limited.join("+"),
            gp.max_mhz,
            gp.pcie_tx,
            gp.pcie_rx,
            gb(gp.total)
        );
        for (i, r) in super::nvml::REASONS.iter().enumerate() {
            self.reasons[i] += (gp.reasons & r.0 != 0) as u32;
        }
        self.mem_util += gp.mem_util as f32;
        self.power_pct += power_pct;
        self.vram_total = gp.total;
        let _ = writeln!(
            self.seconds,
            "{t:.2},{:.2},{:.3},{:.3},{:.3},{:.2},{:.2},{:.2},{},{},{},{},{},{},{},{:.2},{:.2},{:.2},{},{},{},{:.1},{},{:.3},{:.3},{:.2},{:.3},{:.2},{:.3},{:.2},{:.2}{extra}{cores}",
            st.fps, st.ms, st.worst, st.dev, st.avg, st.low1, st.low01, opt(st.real_fps), opt(st.latency), s.gen, st.direct as u8, st.by_flip as u8, st.spike as u8, st.frames,
            sn.cpu, sn.ghz, sn.gpu.util, sn.gpu.mhz, sn.gpu.mem_mhz, sn.gpu.temp, sn.gpu.watts, sn.gpu.fan, gb(sn.gpu.used), gb(sn.ram_used),
            g.cpu, gb(g.ram), g.gpu, gb(g.vram), mb(g.disk), mb(g.net),
        );
        for f in &s.frames {
            let first = *self.first.get_or_insert(*f);
            if self.last != 0 {
                let ms = ((*f - self.last) as f64 * self.ms_per_tick) as f32;
                let at = ((*f - first) as f64 * self.ms_per_tick) as f32;
                let _ = writeln!(self.frames, "{at:.3},{ms:.3}");
                self.frame_ms.push(ms);
                self.frame_at.push(at);
            }
            self.last = *f;
        }
        self.rows += 1;
        self.cpu += sn.cpu;
        self.gpu += sn.gpu.util;
        self.vram = self.vram.max(sn.gpu.used);
        self.ram = self.ram.max(sn.ram_used);
        self.cores.resize(sn.threads.len(), 0.0);
        for (sum, v) in self.cores.iter_mut().zip(&sn.threads) {
            *sum += v;
        }
        if self.read_threads {
            self.sample_threads(s.pid, t);
        }
        let _ = self.seconds.flush();
        let _ = self.frames.flush();
        let _ = self.threads.flush();
    }

    /// Uso de cada thread do jogo desde a última leitura, em % de um núcleo.
    fn sample_threads(&mut self, pid: u32, t: f32) {
        let span = self.sampled.map(|s| s.elapsed().as_secs_f32() * 1e7);
        self.sampled = Some(Instant::now());
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) else { return };
            let mut e = THREADENTRY32 { dwSize: std::mem::size_of::<THREADENTRY32>() as u32, ..Default::default() };
            let mut more = Thread32First(snap, &mut e).is_ok();
            while more {
                if e.th32OwnerProcessID == pid {
                    self.read_thread(e.th32ThreadID, span, t);
                }
                more = Thread32Next(snap, &mut e).is_ok();
            }
            let _ = CloseHandle(snap);
        }
    }

    unsafe fn read_thread(&mut self, tid: u32, span: Option<f32>, t: f32) {
        let Ok(h) = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, tid) else { return };
        let (mut a, mut b, mut kernel, mut user) = Default::default();
        if GetThreadTimes(h, &mut a, &mut b, &mut kernel, &mut user).is_ok() {
            let (k, u) = (filetime(kernel), filetime(user));
            if !self.totals.contains_key(&tid) {
                self.totals.insert(tid, ThreadTotal { name: thread_name(h), ..Default::default() });
            }
            if let (Some(span), Some((ok, ou))) = (span, self.times.get(&tid).copied()) {
                let pct = (k + u - ok - ou) as f32 / span * 100.0;
                let total = self.totals.entry(tid).or_default();
                total.sum += pct;
                total.max = total.max.max(pct);
                total.seen += 1;
                if pct >= THREAD_MIN {
                    let _ = writeln!(self.threads, "{t:.2},{tid},{},{pct:.1},{:.1}", total.name, (k - ok) as f32 / span * 100.0);
                }
            }
            self.times.insert(tid, (k, u));
        }
        let _ = CloseHandle(h);
    }

    fn finish(&mut self, tail: &str) {
        let Ok(mut out) = std::fs::OpenOptions::new().append(true).open(&self.summary) else { return };
        let _ = writeln!(out, "\n{tail}");
        let line = |out: &mut File, name: &str, value: String| {
            let _ = writeln!(out, "{}: {value}", tr(name));
        };
        let rows = self.rows.max(1) as f32;
        line(&mut out, "Duração", format!("{:.0} s", self.begin.elapsed().as_secs_f32()));
        line(&mut out, "Quadros", self.frame_ms.len().to_string());
        let mut sorted = self.frame_ms.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let (mut median, mut top_gpu, mut top_thread) = (0.0, self.gpu / rows, 0.0f32);
        if !sorted.is_empty() {
            let pick = |q: f32| sorted[((sorted.len() - 1) as f32 * q) as usize];
            median = pick(0.5);
            let span: f32 = sorted.iter().sum();
            line(&mut out, "FPS médio", format!("{:.1}", sorted.len() as f32 * 1000.0 / span));
            line(&mut out, "1% low", format!("{:.1}", 1000.0 / pick(0.99)));
            line(&mut out, "0,1% low", format!("{:.1}", 1000.0 / pick(0.999)));
            line(&mut out, "Frame time mediano", format!("{median:.2} ms"));
            line(&mut out, "Frame time p95", format!("{:.2} ms", pick(0.95)));
            line(&mut out, "Frame time p99", format!("{:.2} ms", pick(0.99)));
            line(&mut out, "Frame time p99,9", format!("{:.2} ms", pick(0.999)));
            line(&mut out, "Pior quadro", format!("{:.2} ms", sorted[sorted.len() - 1]));
        }
        let limit = (median * 2.5).max(median + 8.0);
        let mut spikes: Vec<(f32, f32)> = self.frame_ms.iter().zip(&self.frame_at).filter(|p| *p.0 > limit).map(|(m, a)| (*m, *a)).collect();
        line(&mut out, "Engasgos", spikes.len().to_string());
        spikes.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (ms, at) in spikes.iter().take(WORST_SHOWN) {
            let _ = writeln!(out, "  {:.1} s: {ms:.1} ms", at / 1000.0);
        }
        let _ = writeln!(out);
        line(&mut out, "CPU média", format!("{:.1}%", self.cpu / rows));
        let core = self.cores.iter().copied().fold(0.0, f32::max) / rows;
        line(&mut out, "Núcleo lógico mais usado", format!("{core:.1}%"));
        line(&mut out, "GPU média", format!("{top_gpu:.1}%"));
        line(&mut out, "Controlador de memória da GPU, média", format!("{:.1}%", self.mem_util / rows));
        line(&mut out, "Energia da GPU, média do limite", format!("{:.1}%", self.power_pct / rows));
        let held: Vec<String> = super::nvml::REASONS
            .iter()
            .zip(self.reasons)
            .filter(|(_, n)| *n > 0)
            .map(|(r, n)| format!("{} {:.0}%", r.1, n as f32 / rows * 100.0))
            .collect();
        line(&mut out, "Clock da GPU segurado por", if held.is_empty() { "—".to_string() } else { held.join(", ") });
        if self.vram_total > 0 {
            line(&mut out, "Pico de VRAM", format!("{:.2} / {:.2} GB", gb(self.vram), gb(self.vram_total)));
        } else {
            line(&mut out, "Pico de VRAM", format!("{:.2} GB", gb(self.vram)));
        }
        line(&mut out, "Pico de RAM", format!("{:.2} GB", gb(self.ram)));
        let mut list: Vec<(&u32, &ThreadTotal)> = self.totals.iter().filter(|t| t.1.seen > 0 && t.1.sum / t.1.seen as f32 >= 0.1).collect();
        list.sort_by(|a, b| (b.1.sum / b.1.seen as f32).total_cmp(&(a.1.sum / a.1.seen as f32)));
        if !list.is_empty() {
            let _ = writeln!(out, "\n{}:", tr("Threads mais ocupadas, em % de um núcleo"));
            for (tid, t) in list.iter().take(THREADS_SHOWN) {
                let avg = t.sum / t.seen as f32;
                top_thread = top_thread.max(avg);
                let name = if t.name.is_empty() { "—" } else { t.name.as_str() };
                let _ = writeln!(out, "  {tid} {name}: {} {avg:.1}%, {} {:.1}%", tr("média"), tr("pico"), t.max);
            }
        }
        if top_gpu.is_nan() {
            top_gpu = 0.0;
        }
        let power_share = self.reasons[0] as f32 / rows;
        let thermal_share = (self.reasons[2] + self.reasons[3]) as f32 / rows;
        let vram_near = self.vram_total > 0 && self.vram as f32 >= self.vram_total as f32 * 0.92;
        if vram_near {
            let _ = writeln!(out, "\n{}", tr("A VRAM chegou perto do total: texturas e resolução são os primeiros ajustes."));
        }
        if thermal_share >= 0.1 {
            let _ = writeln!(out, "\n{}", tr("A temperatura segurou o clock da GPU em parte do tempo."));
        }
        let reading = if top_gpu >= BUSY && power_share >= 0.5 {
            "Leitura: GPU cheia e no limite de energia, então o limite é a GPU. Menos carga por quadro (resolução, sombras, efeitos) é o que rende."
        } else if top_gpu >= BUSY {
            "Leitura: GPU cheia na maior parte do tempo, então o limite parece ser a GPU."
        } else if top_thread >= BUSY {
            "Leitura: uma thread perto de um núcleo inteiro com a GPU sobrando, então o limite parece ser a CPU."
        } else {
            "Leitura: nem GPU nem uma thread sozinha ficaram cheias. Veja os trechos em segundos.csv."
        };
        let _ = writeln!(out, "\n{}", tr(reading));
    }
}

impl Log {
    /// `threads` fica desligado em jogos com anti-cheat, que não devem ver outro programa abrindo suas threads.
    pub fn start(exe: &str, head: String, threads: bool, limit: Option<u32>) -> Option<Self> {
        let stamp = crate::sys::tools::now_stamp().replace(':', "-");
        let dir = root().join(format!("{} {stamp}", exe.trim_end_matches(".exe")));
        std::fs::create_dir_all(&dir).ok()?;
        let summary = dir.join("resumo.txt");
        std::fs::write(&summary, head).ok()?;
        let open = |name: &str| File::create(dir.join(name)).ok().map(BufWriter::new);
        let mut f = 0i64;
        unsafe {
            let _ = QueryPerformanceFrequency(&mut f);
        }
        let mut w = Writer {
            summary,
            seconds: open("segundos.csv")?,
            frames: open("quadros.csv")?,
            threads: open("threads.csv")?,
            read_threads: threads,
            begin: Instant::now(),
            ms_per_tick: 1000.0 / f.max(1) as f64,
            first: None,
            last: 0,
            frame_ms: Vec::new(),
            frame_at: Vec::new(),
            rows: 0,
            cpu: 0.0,
            gpu: 0.0,
            cores: Vec::new(),
            vram: 0,
            ram: 0,
            times: HashMap::new(),
            sampled: None,
            totals: HashMap::new(),
            mem_util: 0.0,
            power_pct: 0.0,
            reasons: [0; super::nvml::REASONS.len()],
            vram_total: 0,
        };
        let (tx, rx) = channel();
        let thread = std::thread::Builder::new()
            .name("registro".into())
            .spawn(move || {
                for m in rx {
                    match m {
                        Msg::Second(s) => w.second(&s),
                        Msg::End(tail) => w.finish(&tail),
                    }
                }
            })
            .ok()?;
        Some(Self { tx, thread: Some(thread), dir, seen: 0, limit, gen_secs: 0, gen_max: 0, lossless: false })
    }

    pub fn second(&self, s: Second) {
        let _ = self.tx.send(Msg::Second(Box::new(s)));
    }

    /// Fecha os arquivos com o resumo final. `tail` traz o que só se soube durante o jogo.
    pub fn finish(mut self, tail: String) -> PathBuf {
        let _ = self.tx.send(Msg::End(tail));
        drop(self.tx);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.dir
    }
}
