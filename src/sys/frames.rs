//! Tempo de quadro de qualquer jogo pelo rastreamento de eventos do Windows, sem tocar no jogo.
//! Só roda com o medidor na tela: sem medidor, ninguém paga esse custo.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Diagnostics::Etw::*;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

const DXGKRNL: GUID = GUID::from_u128(0x802ec45a_1e99_4b83_9920_87c98277ba9d);
/// Marcadores de latência que jogos com Reflex publicam.
const PCL: GUID = GUID::from_u128(0x0d216f06_82a6_4d49_bc4f_8f38ae56efab);
// A versão de teste roda junto com a de verdade: sessão com o mesmo nome derrubaria a outra.
#[cfg(not(debug_assertions))]
const SESSION: GUID = GUID::from_u128(0x7c1e4d52_9b3a_4f61_8e2d_5a6f0c3b9d11);
#[cfg(not(debug_assertions))]
const NAME: PCWSTR = w!("PrismaFrames");
#[cfg(debug_assertions)]
const SESSION: GUID = GUID::from_u128(0x7c1e4d52_9b3a_4f61_8e2d_5a6f0c3b9d12);
#[cfg(debug_assertions)]
const NAME: PCWSTR = w!("PrismaFramesDev");

const KEYWORD_BASE: u64 = 0x1;
const KEYWORD_PRESENT: u64 = 0x800_0000;
const VERBOSE: u8 = 5;
const PRESENT_INFO: u16 = 184;
const FLIP_INFO: u16 = 168;
const FLIP_MPO_INFO: u16 = 252;
const MARKER_SIM_START: u32 = 0;
const MARKER_PRESENT_END: u32 = 5;

/// Cerca de um minuto a 120 FPS.
const KEEP: usize = 7200;
const STRIP: usize = 120;
/// Sem quadro novo por este tempo, o número deixa de valer. O Windows entrega os eventos a cada segundo.
const STALE_MS: f32 = 3000.0;

#[derive(Default)]
struct State {
    target: u32,
    /// Um quarto de segundo em unidades do relógio.
    quiet: i64,
    last_present: i64,
    /// Quadros contados pela troca de imagem desde a última leitura.
    by_flip: u32,
    /// Todos os quadros, só enquanto o registro em arquivo está ligado.
    rec: Option<Vec<i64>>,
    times: VecDeque<i64>,
    /// Apresentações por processo desde a última leitura.
    presents: HashMap<u32, u32>,
    flips: u32,
    sims: VecDeque<i64>,
    /// Início de cada quadro, para somar a latência quando ele termina.
    open: HashMap<u64, i64>,
    latency: VecDeque<f32>,
}

impl State {
    /// História limpa, sem perder o que não é história.
    fn fresh(&mut self, target: u32) -> Self {
        Self { target, quiet: self.quiet, rec: self.rec.take(), presents: std::mem::take(&mut self.presents), ..Default::default() }
    }
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

#[derive(Clone, Default, Debug)]
pub struct Stats {
    pub frames: usize,
    pub fps: f32,
    pub avg: f32,
    pub low1: f32,
    pub low01: f32,
    pub ms: f32,
    /// Desvio em relação à média local, em ms.
    pub dev: f32,
    pub strip: Vec<f32>,
    pub spike: bool,
    pub real_fps: Option<f32>,
    pub latency: Option<f32>,
    /// A imagem do jogo vai direto à tela, sem composição do Windows.
    pub direct: bool,
    /// Pior intervalo do último segundo, para descartar carregamentos.
    pub worst: f32,
    /// O jogo parou de avisar a apresentação, e os quadros saem da troca de imagem.
    pub by_flip: bool,
}

unsafe extern "system" fn on_event(r: *mut EVENT_RECORD) {
    let r = &*r;
    let h = &r.EventHeader;
    let Ok(mut guard) = STATE.lock() else { return };
    let Some(s) = guard.as_mut() else { return };
    if h.ProviderId == DXGKRNL {
        let id = h.EventDescriptor.Id;
        if !matches!(id, PRESENT_INFO | FLIP_INFO | FLIP_MPO_INFO) {
            return;
        }
        *s.presents.entry(h.ProcessId).or_default() += 1;
        if h.ProcessId == s.target {
            let flip = id != PRESENT_INFO;
            let counted = if flip {
                s.flips += 1;
                h.TimeStamp - s.last_present > s.quiet
            } else {
                s.last_present = h.TimeStamp;
                true
            };
            if counted {
                s.by_flip += flip as u32;
                if s.times.len() >= KEEP {
                    s.times.pop_front();
                }
                s.times.push_back(h.TimeStamp);
                if let Some(rec) = &mut s.rec {
                    rec.push(h.TimeStamp);
                }
            }
        }
    } else if h.ProviderId == PCL && h.ProcessId == s.target && r.UserDataLength >= 12 && !r.UserData.is_null() {
        let d = std::slice::from_raw_parts(r.UserData as *const u8, 12);
        let marker = u32::from_le_bytes([d[0], d[1], d[2], d[3]]);
        let frame = u64::from_le_bytes([d[4], d[5], d[6], d[7], d[8], d[9], d[10], d[11]]);
        if marker == MARKER_SIM_START {
            if s.sims.len() >= 2048 {
                s.sims.pop_front();
            }
            s.sims.push_back(h.TimeStamp);
            if s.open.len() > 64 {
                s.open.clear();
            }
            s.open.insert(frame, h.TimeStamp);
        } else if marker == MARKER_PRESENT_END {
            if let Some(start) = s.open.remove(&frame) {
                if s.latency.len() >= 120 {
                    s.latency.pop_front();
                }
                s.latency.push_back((h.TimeStamp - start) as f32);
            }
        }
    }
}

#[repr(C)]
struct Props {
    p: EVENT_TRACE_PROPERTIES,
    name: [u16; 64],
}

fn props() -> Box<Props> {
    let mut b: Box<Props> = Box::new(unsafe { std::mem::zeroed() });
    b.p.Wnode.BufferSize = std::mem::size_of::<Props>() as u32;
    b.p.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
    b.p.Wnode.ClientContext = 1;
    b.p.Wnode.Guid = SESSION;
    b.p.LoggerNameOffset = std::mem::size_of::<EVENT_TRACE_PROPERTIES>() as u32;
    b
}

pub struct Tracker {
    handle: CONTROLTRACE_HANDLE,
    consumer: PROCESSTRACE_HANDLE,
    thread: Option<std::thread::JoinHandle<()>>,
    frequency: f32,
    pcl: bool,
}

/// Só os eventos que interessam chegam ao Prisma: o resto nem sai do kernel.
unsafe fn enable_filtered(handle: CONTROLTRACE_HANDLE) -> WIN32_ERROR {
    #[repr(C)]
    struct Ids {
        filter_in: u8,
        reserved: u8,
        count: u16,
        ids: [u16; 3],
    }
    let ids = Ids { filter_in: 1, reserved: 0, count: 3, ids: [PRESENT_INFO, FLIP_INFO, FLIP_MPO_INFO] };
    let mut desc = EVENT_FILTER_DESCRIPTOR { Ptr: &ids as *const Ids as u64, Size: std::mem::size_of::<Ids>() as u32, Type: EVENT_FILTER_TYPE_EVENT_ID };
    let params = ENABLE_TRACE_PARAMETERS {
        Version: ENABLE_TRACE_PARAMETERS_VERSION_2,
        EnableFilterDesc: &mut desc,
        FilterDescCount: 1,
        ..Default::default()
    };
    EnableTraceEx2(handle, &DXGKRNL, EVENT_CONTROL_CODE_ENABLE_PROVIDER.0, VERBOSE, KEYWORD_BASE | KEYWORD_PRESENT, 0, 0, Some(&params))
}

impl Tracker {
    /// Sem permissão de rastreamento devolve o motivo, e o medidor mostra o resto.
    pub fn start() -> std::result::Result<Self, String> {
        unsafe {
            let mut stale = props();
            let _ = ControlTraceW(CONTROLTRACE_HANDLE::default(), NAME, &mut stale.p, EVENT_TRACE_CONTROL_STOP);
            let mut p = props();
            p.p.LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
            p.p.BufferSize = 64;
            p.p.MinimumBuffers = 4;
            p.p.MaximumBuffers = 16;
            p.p.FlushTimer = 1;
            let mut handle = CONTROLTRACE_HANDLE::default();
            let started = StartTraceW(&mut handle, NAME, &mut p.p);
            if started != ERROR_SUCCESS {
                return Err(format!("a sessão não abriu (erro {})", started.0));
            }
            let enabled = enable_filtered(handle);
            if enabled != ERROR_SUCCESS {
                let mut stop = props();
                let _ = ControlTraceW(handle, PCWSTR::null(), &mut stop.p, EVENT_TRACE_CONTROL_STOP);
                // 1450: o Windows deixa só 8 sessões ouvindo a placa de vídeo, e outros programas usam as outras.
                return Err(format!("a placa de vídeo não aceitou a sessão (erro {})", enabled.0));
            }
            let mut f = 0i64;
            let _ = QueryPerformanceFrequency(&mut f);
            *STATE.lock().unwrap() = Some(State { quiet: f / 4, ..Default::default() });
            let mut log = EVENT_TRACE_LOGFILEW::default();
            log.LoggerName = PWSTR(NAME.as_ptr() as *mut u16);
            log.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD | PROCESS_TRACE_MODE_RAW_TIMESTAMP;
            log.Anonymous2.EventRecordCallback = Some(on_event);
            let consumer = OpenTraceW(&mut log);
            if consumer.Value == u64::MAX {
                let e = GetLastError();
                let mut stop = props();
                let _ = ControlTraceW(handle, PCWSTR::null(), &mut stop.p, EVENT_TRACE_CONTROL_STOP);
                return Err(format!("não deu para ler a sessão (erro {})", e.0));
            }
            let c = consumer.Value;
            let thread = std::thread::Builder::new()
                .name("quadros".into())
                .spawn(move || {
                    let _ = ProcessTrace(&[PROCESSTRACE_HANDLE { Value: c }], None, None);
                })
                .ok();
            Ok(Self { handle, consumer, thread, frequency: f as f32, pcl: false })
        }
    }

    /// Troca o processo medido e zera a história: outro jogo, outro minuto.
    pub fn set_target(&mut self, pid: u32) {
        if let Some(s) = STATE.lock().unwrap().as_mut() {
            if s.target != pid {
                *s = s.fresh(pid);
            }
        }
    }

    /// Apaga a história sem trocar de processo.
    pub fn reset(&mut self) {
        if let Some(s) = STATE.lock().unwrap().as_mut() {
            *s = s.fresh(s.target);
        }
    }

    /// Liga a cópia de cada quadro para o registro em arquivo.
    pub fn set_recording(&mut self, on: bool) {
        if let Some(s) = STATE.lock().unwrap().as_mut() {
            if !on {
                s.rec = None;
            } else if s.rec.is_none() {
                s.rec = Some(Vec::new());
            }
        }
    }

    /// Quadros desde a última chamada, com o relógio do Windows.
    pub fn take_frames(&mut self) -> Vec<i64> {
        STATE.lock().unwrap().as_mut().and_then(|s| s.rec.as_mut().map(std::mem::take)).unwrap_or_default()
    }

    /// A sessão do Windows ainda entrega eventos? Parar de fora encerra a linha que lê.
    pub fn alive(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Ligar os marcadores faz o jogo enviar pings e teclas sintéticas: só com o medidor aberto.
    pub fn set_markers(&mut self, on: bool) {
        if self.pcl != on {
            self.pcl = on;
            let code = if on { EVENT_CONTROL_CODE_ENABLE_PROVIDER } else { EVENT_CONTROL_CODE_DISABLE_PROVIDER };
            unsafe {
                let _ = EnableTraceEx2(self.handle, &PCL, code.0, VERBOSE, u64::MAX, 0, 0, None);
            }
        }
    }

    /// Quem mais apresentou quadros desde a última chamada, e quantos foram os de `pid`.
    pub fn presenters(&self, pid: u32) -> (Option<(u32, u32)>, u32) {
        let mut g = STATE.lock().unwrap();
        let Some(s) = g.as_mut() else { return (None, 0) };
        let top = s.presents.iter().max_by_key(|p| *p.1).map(|(p, n)| (*p, *n));
        let own = s.presents.get(&pid).copied().unwrap_or(0);
        s.presents.clear();
        (top, own)
    }

    pub fn stats(&self) -> Stats {
        let mut out = Stats::default();
        let mut g = STATE.lock().unwrap();
        let Some(s) = g.as_mut() else { return out };
        let tick = 1000.0 / self.frequency;
        let mut now = 0i64;
        unsafe {
            let _ = QueryPerformanceCounter(&mut now);
        }
        if s.times.back().is_some_and(|last| (now - last) as f32 * tick > STALE_MS) {
            // Número antigo parece medição: sem quadro novo, a história sai inteira.
            *s = s.fresh(s.target);
            return out;
        }
        let ms: Vec<f32> = s.times.iter().zip(s.times.iter().skip(1)).map(|(a, b)| (b - a) as f32 * tick).filter(|v| *v > 0.0).collect();
        out.frames = ms.len();
        out.direct = s.flips > 0;
        out.by_flip = s.by_flip > 0;
        s.flips = 0;
        s.by_flip = 0;
        let last = *s.times.back().unwrap_or(&0);
        let since = |list: &VecDeque<i64>, t: f32| list.iter().rev().take_while(|v| (last - **v) as f32 * tick < t).count();
        if !s.latency.is_empty() {
            out.latency = Some(s.latency.iter().sum::<f32>() / s.latency.len() as f32 * tick);
        }
        if since(&s.sims, 1000.0) > 2 {
            out.real_fps = Some((since(&s.sims, 1000.0) - 1) as f32);
        }
        if ms.is_empty() {
            return out;
        }
        let recent = since(&s.times, 1000.0).clamp(2, ms.len() + 1) - 1;
        let tail = &ms[ms.len() - recent..];
        let span: f32 = tail.iter().sum();
        out.fps = tail.len() as f32 * 1000.0 / span;
        out.ms = span / tail.len() as f32;
        out.worst = tail.iter().copied().fold(0.0, f32::max);
        out.avg = ms.len() as f32 * 1000.0 / ms.iter().sum::<f32>();
        let mut sorted = ms.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let pick = |q: f32| sorted[((sorted.len() - 1) as f32 * q) as usize];
        out.low1 = 1000.0 / pick(0.99);
        out.low01 = 1000.0 / pick(0.999);
        out.strip = ms[ms.len().saturating_sub(STRIP)..].to_vec();
        out.spike = out.strip.iter().rev().take(30).any(|v| *v > (out.ms * 2.5).max(out.ms + 8.0));
        // Desvio contra a média móvel, que ignora a troca lenta de cena para cena.
        let w = ms[ms.len().saturating_sub(240)..].to_vec();
        if w.len() > 9 {
            let sum: f32 = w.windows(9).map(|c| (c[4] - c.iter().sum::<f32>() / 9.0).powi(2)).sum();
            out.dev = (sum / (w.len() - 8) as f32).sqrt();
        }
        out
    }
}

impl Drop for Tracker {
    fn drop(&mut self) {
        unsafe {
            let mut p = props();
            let _ = ControlTraceW(self.handle, PCWSTR::null(), &mut p.p, EVENT_TRACE_CONTROL_STOP);
            let _ = CloseTrace(self.consumer);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        *STATE.lock().unwrap() = None;
    }
}

/// Jogos com Reflex registram este provedor ao abrir: basta perguntar quem se registrou.
pub fn reflex_pids() -> Vec<u32> {
    let mut out = Vec::new();
    unsafe {
        let mut buf = vec![0u8; 64 << 10];
        let mut size = 0u32;
        let done = EnumerateTraceGuidsEx(TraceGuidQueryInfo, Some(&PCL as *const GUID as *const _), std::mem::size_of::<GUID>() as u32, Some(buf.as_mut_ptr() as *mut _), buf.len() as u32, &mut size);
        if done != ERROR_SUCCESS || (size as usize) < std::mem::size_of::<TRACE_GUID_INFO>() {
            return out;
        }
        let info = &*(buf.as_ptr() as *const TRACE_GUID_INFO);
        let mut at = std::mem::size_of::<TRACE_GUID_INFO>();
        for _ in 0..info.InstanceCount {
            if at + std::mem::size_of::<TRACE_PROVIDER_INSTANCE_INFO>() > size as usize {
                break;
            }
            let item = &*(buf.as_ptr().add(at) as *const TRACE_PROVIDER_INSTANCE_INFO);
            out.push(item.Pid);
            if item.NextOffset == 0 {
                break;
            }
            at += item.NextOffset as usize;
        }
    }
    out
}

/// Teste: mede um processo por alguns segundos.
#[cfg(debug_assertions)]
pub fn measure(pid: u32, secs: u64) -> String {
    let mut t = match Tracker::start() {
        Ok(t) => t,
        Err(e) => return e,
    };
    t.set_target(pid);
    let mut out = String::new();
    for _ in 0..secs {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let s = t.stats();
        out += &format!("fps {:.1} avg {:.1} low1 {:.1} low01 {:.1} ms {:.2} dev {:.2} direto {} troca {} top {:?}\n", s.fps, s.avg, s.low1, s.low01, s.ms, s.dev, s.direct, s.by_flip, t.presenters(pid).0);
    }
    out
}
