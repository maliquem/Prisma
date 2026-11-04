//! Rede e disco por programa com o rastreamento de eventos do Windows.
//! Precisa de administrador e só roda com a janela aberta; sem isso, ninguém paga o custo.

use std::collections::HashMap;
use std::sync::Mutex;

use windows::core::*;
use windows::Win32::System::Diagnostics::Etw::*;

const TCPIP: GUID = GUID::from_u128(0x9a280ac0_c8e0_11d1_84e2_00c04fb998a2);
const UDPIP: GUID = GUID::from_u128(0xbf3a50c5_a9c9_4988_a005_2df0b7c80f80);
const DISKIO: GUID = GUID::from_u128(0x3d6fa8d4_fe05_11d0_9dda_00c04fd7ba7c);
const SESSION_GUID: GUID = GUID::from_u128(0x7c1e4d52_9b3a_4f61_8e2d_5a6f0c3b9d10);

const NAME: PCWSTR = w!("PrismaTrace");

#[derive(Clone, Copy, Default)]
pub struct Io {
    pub net: u64,
    pub disk: u64,
}

static TOTALS: Mutex<Option<HashMap<u32, Io>>> = Mutex::new(None);
static THREADS: Mutex<Option<HashMap<u32, u32>>> = Mutex::new(None);

/// Tabela de thread para processo: eventos de disco só trazem a thread.
pub fn set_threads(map: HashMap<u32, u32>) {
    *THREADS.lock().unwrap() = Some(map);
}

/// Devolve o acumulado desde a última chamada.
pub fn take() -> HashMap<u32, Io> {
    TOTALS.lock().unwrap().get_or_insert_with(HashMap::new).drain().collect()
}

unsafe extern "system" fn on_event(r: *mut EVENT_RECORD) {
    let r = &*r;
    let h = &r.EventHeader;
    let g = h.ProviderId;
    let op = h.EventDescriptor.Opcode;
    if r.UserData.is_null() {
        return;
    }
    let d = std::slice::from_raw_parts(r.UserData as *const u8, r.UserDataLength as usize);
    let u32_at = |o: usize| d.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    if g == TCPIP || g == UDPIP {
        if !matches!(op, 10 | 11 | 26 | 27) {
            return;
        }
        let (Some(pid), Some(size)) = (u32_at(0), u32_at(4)) else { return };
        if let Ok(mut t) = TOTALS.lock() {
            t.get_or_insert_with(HashMap::new).entry(pid).or_default().net += size as u64;
        }
    } else if g == DISKIO {
        if !matches!(op, 10 | 11) {
            return;
        }
        let Some(size) = u32_at(8) else { return };
        let tid = u32_at(48).unwrap_or(0);
        let mut pid = 0;
        if tid != 0 {
            if let Ok(m) = THREADS.lock() {
                pid = m.as_ref().and_then(|m| m.get(&tid).copied()).unwrap_or(0);
            }
        }
        if pid == 0 && h.ProcessId != u32::MAX {
            pid = h.ProcessId;
        }
        if pid != 0 {
            if let Ok(mut t) = TOTALS.lock() {
                t.get_or_insert_with(HashMap::new).entry(pid).or_default().disk += size as u64;
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
    b.p.Wnode.Guid = SESSION_GUID;
    b.p.LoggerNameOffset = std::mem::size_of::<EVENT_TRACE_PROPERTIES>() as u32;
    b
}

fn stop_session() {
    let mut p = props();
    unsafe {
        let _ = ControlTraceW(CONTROLTRACE_HANDLE::default(), NAME, &mut p.p, EVENT_TRACE_CONTROL_STOP);
    }
}

pub struct Trace {
    handle: CONTROLTRACE_HANDLE,
    consumer: PROCESSTRACE_HANDLE,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Trace {
    /// Sem administrador devolve None, e o Monitor segue sem a coluna de rede.
    pub fn start() -> Option<Self> {
        unsafe {
            // Uma queda anterior pode ter deixado a sessão aberta.
            stop_session();
            let mut p = props();
            p.p.LogFileMode = EVENT_TRACE_REAL_TIME_MODE | EVENT_TRACE_SYSTEM_LOGGER_MODE;
            p.p.BufferSize = 64;
            p.p.MinimumBuffers = 4;
            p.p.MaximumBuffers = 16;
            p.p.FlushTimer = 1;
            p.p.EnableFlags = EVENT_TRACE_FLAG_NETWORK_TCPIP | EVENT_TRACE_FLAG_DISK_IO;
            let mut handle = CONTROLTRACE_HANDLE::default();
            if StartTraceW(&mut handle, NAME, &mut p.p).is_err() {
                return None;
            }
            let mut log = EVENT_TRACE_LOGFILEW::default();
            log.LoggerName = PWSTR(NAME.as_ptr() as *mut u16);
            log.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
            log.Anonymous2.EventRecordCallback = Some(on_event);
            let consumer = OpenTraceW(&mut log);
            if consumer.Value == u64::MAX {
                stop_session();
                return None;
            }
            let c = consumer.Value;
            let thread = std::thread::Builder::new()
                .name("etw".into())
                .spawn(move || {
                    let _ = ProcessTrace(&[PROCESSTRACE_HANDLE { Value: c }], None, None);
                })
                .ok();
            Some(Self { handle, consumer, thread })
        }
    }
}

impl Drop for Trace {
    fn drop(&mut self) {
        unsafe {
            let mut p = props();
            let _ = ControlTraceW(self.handle, PCWSTR::null(), &mut p.p, EVENT_TRACE_CONTROL_STOP);
            let _ = CloseTrace(self.consumer);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        *THREADS.lock().unwrap() = None;
    }
}
