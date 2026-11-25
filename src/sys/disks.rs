//! Discos físicos: atividade em tempo real e saúde pelo SMART, sem driver.
//! A saúde roda em thread própria porque pode acordar disco parado.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::IO::DeviceIoControl;

const QUERY_PROPERTY: u32 = 0x2D1400;
const DISK_PERFORMANCE: u32 = 0x70020;
const GET_GEOMETRY: u32 = 0x700A0;
const VOLUME_EXTENTS: u32 = 0x560000;
const ATA_PASS: u32 = 0x4D02C;
const SCSI_PASS: u32 = 0x4D004;

const BUS_ATA: u32 = 3;
const BUS_USB: u32 = 7;
const BUS_RAID: u32 = 8;
const BUS_SAS: u32 = 10;
const BUS_SATA: u32 = 11;
const BUS_FILE: u32 = 15;
const BUS_NVME: u32 = 17;

fn open(path: &str, access: u32) -> Option<HANDLE> {
    unsafe {
        let share = FILE_SHARE_READ | FILE_SHARE_WRITE;
        CreateFileW(
            &HSTRING::from(path),
            access,
            share,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
        .ok()
    }
}

fn ioctl(h: HANDLE, code: u32, input: &[u8], output: &mut [u8]) -> Option<usize> {
    unsafe {
        let mut got = 0;
        DeviceIoControl(
            h,
            code,
            Some(input.as_ptr() as *const c_void),
            input.len() as u32,
            Some(output.as_mut_ptr() as *mut c_void),
            output.len() as u32,
            Some(&mut got),
            None,
        )
        .ok()?;
        Some(got as usize)
    }
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn text_at(b: &[u8], o: usize) -> String {
    if o == 0 || o >= b.len() {
        return String::new();
    }
    let end = b[o..].iter().position(|c| *c == 0).map_or(b.len(), |e| o + e);
    String::from_utf8_lossy(&b[o..end]).trim().to_string()
}

fn query(prop: u32) -> [u8; 12] {
    let mut q = [0u8; 12];
    q[..4].copy_from_slice(&prop.to_le_bytes());
    q
}

#[derive(Clone, Default)]
pub struct Volume {
    pub letter: char,
    pub label: String,
    pub total: u64,
    pub free: u64,
}

#[derive(Clone)]
pub struct Disk {
    pub number: u32,
    pub name: String,
    pub serial: String,
    pub bus: u32,
    pub ssd: bool,
    pub size: u64,
    pub volumes: Vec<Volume>,
    pub read: u64,
    pub write: u64,
    pub active: f32,
    pub hist: Vec<f32>,
    prev: Option<Perf>,
}

#[derive(Clone, Copy)]
struct Perf {
    read: u64,
    write: u64,
    idle: u64,
    time: u64,
    at: Instant,
}

impl Disk {
    pub fn bus_name(&self) -> &'static str {
        match self.bus {
            BUS_NVME => "NVMe",
            BUS_SATA | BUS_ATA => "SATA",
            BUS_USB => "USB",
            BUS_SAS => "SAS",
            BUS_RAID => "RAID",
            _ => "Outro",
        }
    }
    pub fn kind(&self) -> &'static str {
        if self.ssd {
            "SSD"
        } else {
            "HD"
        }
    }
}

fn read_perf(h: HANDLE) -> Option<Perf> {
    let mut b = [0u8; 88];
    ioctl(h, DISK_PERFORMANCE, &[], &mut b)?;
    Some(Perf {
        read: u64_at(&b, 0),
        write: u64_at(&b, 8),
        idle: u64_at(&b, 32),
        time: u64_at(&b, 56),
        at: Instant::now(),
    })
}

/// Se a letra fica num SSD. None quando o Windows não diz (disco de rede, sem acesso).
pub fn is_ssd(letter: char) -> Option<bool> {
    let h = open(&format!("\\\\.\\{letter}:"), 0)?;
    let mut b = [0u8; 256];
    let got = ioctl(h, VOLUME_EXTENTS, &[], &mut b);
    unsafe {
        let _ = CloseHandle(h);
    }
    got?;
    describe(u32_at(&b, 8)).map(|d| d.ssd)
}

fn describe(n: u32) -> Option<Disk> {
    let h = open(&format!("\\\\.\\PhysicalDrive{n}"), 0)?;
    let mut b = [0u8; 1024];
    let got = ioctl(h, QUERY_PROPERTY, &query(0), &mut b);
    let mut seek = [0u8; 12];
    let has_seek = ioctl(h, QUERY_PROPERTY, &query(7), &mut seek).is_some();
    // A geometria funciona sem direito de leitura, então não exige administrador.
    let mut geo = [0u8; 256];
    let size = ioctl(h, GET_GEOMETRY, &[], &mut geo).map(|_| u64_at(&geo, 24)).unwrap_or(0);
    let prev = read_perf(h);
    unsafe {
        let _ = CloseHandle(h);
    }
    got?;
    let bus = u32_at(&b, 28);
    if bus == BUS_FILE || size == 0 {
        return None;
    }
    let vendor = text_at(&b, u32_at(&b, 12) as usize);
    let product = text_at(&b, u32_at(&b, 16) as usize);
    let name = if vendor.is_empty() || product.starts_with(&vendor) { product } else { format!("{vendor} {product}") };
    Some(Disk {
        number: n,
        name,
        serial: text_at(&b, u32_at(&b, 24) as usize),
        bus,
        ssd: bus == BUS_NVME || (has_seek && seek[8] == 0),
        size,
        volumes: Vec::new(),
        read: 0,
        write: 0,
        active: 0.0,
        hist: Vec::new(),
        prev,
    })
}

fn volumes() -> HashMap<u32, Vec<Volume>> {
    let mut out: HashMap<u32, Vec<Volume>> = HashMap::new();
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = HSTRING::from(format!("{letter}:\\"));
        let kind = unsafe { GetDriveTypeW(&root) };
        if kind != 3 && kind != 2 {
            continue;
        }
        let Some(h) = open(&format!("\\\\.\\{letter}:"), 0) else { continue };
        let mut b = [0u8; 256];
        let got = ioctl(h, VOLUME_EXTENTS, &[], &mut b);
        unsafe {
            let _ = CloseHandle(h);
        }
        if got.is_none() {
            continue;
        }
        let mut v = Volume { letter, ..Default::default() };
        unsafe {
            let (mut free, mut total) = (0u64, 0u64);
            let _ = GetDiskFreeSpaceExW(&root, None, Some(&mut total), Some(&mut free));
            v.total = total;
            v.free = free;
            let mut label = [0u16; 64];
            let _ = GetVolumeInformationW(&root, Some(&mut label), None, None, None, None);
            let end = label.iter().position(|c| *c == 0).unwrap_or(0);
            v.label = String::from_utf16_lossy(&label[..end]);
        }
        out.entry(u32_at(&b, 8)).or_default().push(v);
    }
    out
}

pub fn list() -> Vec<Disk> {
    let mut vols = volumes();
    let mut out: Vec<Disk> = (0..32).filter_map(describe).collect();
    for d in &mut out {
        d.volumes = vols.remove(&d.number).unwrap_or_default();
    }
    out.sort_by_key(|d| d.volumes.first().map_or('~', |v| v.letter));
    out
}

pub fn sample(list: &mut [Disk]) {
    for d in list {
        let Some(h) = open(&format!("\\\\.\\PhysicalDrive{}", d.number), 0) else { continue };
        let now = read_perf(h);
        unsafe {
            let _ = CloseHandle(h);
        }
        let Some(now) = now else { continue };
        if let Some(p) = d.prev {
            let dt = (now.at - p.at).as_secs_f64().max(0.05);
            d.read = (now.read.saturating_sub(p.read) as f64 / dt) as u64;
            d.write = (now.write.saturating_sub(p.write) as f64 / dt) as u64;
            let span = now.time.saturating_sub(p.time) as f64;
            let idle = now.idle.saturating_sub(p.idle) as f64;
            d.active = if span > 0.0 { (100.0 * (1.0 - idle / span)).clamp(0.0, 100.0) as f32 } else { 0.0 };
        }
        d.prev = Some(now);
        if d.hist.len() == 60 {
            d.hist.remove(0);
        }
        d.hist.push(d.active);
    }
}

#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub enum Level {
    #[default]
    Unknown,
    Good,
    Warn,
    Risk,
}

#[derive(Clone, Default, Debug)]
pub struct Health {
    pub level: Level,
    pub summary: String,
    pub temp: Option<i32>,
    pub hours: Option<u64>,
    pub details: Vec<(String, String)>,
}

fn nvme(h: HANDLE) -> Option<Health> {
    let mut buf = vec![0u8; 8 + 40 + 512];
    buf[..4].copy_from_slice(&50u32.to_le_bytes());
    let spec: [u32; 6] = [3, 2, 2, 0, 40, 512];
    for (i, v) in spec.iter().enumerate() {
        buf[8 + i * 4..12 + i * 4].copy_from_slice(&v.to_le_bytes());
    }
    let input = buf.clone();
    ioctl(h, QUERY_PROPERTY, &input, &mut buf)?;
    let off = 8 + u32_at(&buf, 8 + 16) as usize;
    let d = buf.get(off..off + 512)?;
    let warn = d[0];
    let temp = u16::from_le_bytes([d[1], d[2]]) as i32 - 273;
    let spare = d[3] as u32;
    let spare_min = d[4] as u32;
    let used = d[5] as u32;
    let written = u64_at(d, 48) * 512_000;
    let read = u64_at(d, 32) * 512_000;
    let cycles = u64_at(d, 112);
    let hours = u64_at(d, 128);
    let unsafe_off = u64_at(d, 144);
    let errors = u64_at(d, 160);
    let life = 100u32.saturating_sub(used);

    let mut notes = Vec::new();
    let mut level = Level::Good;
    if warn != 0 {
        level = Level::Risk;
        notes.push("O próprio disco reportou alerta crítico".to_string());
    }
    if errors > 0 {
        level = Level::Risk;
        notes.push(format!("{errors} erros de mídia"));
    }
    if spare < spare_min {
        level = Level::Risk;
        notes.push("Reserva de blocos abaixo do mínimo".to_string());
    }
    if level == Level::Good && (life <= 10 || spare < spare_min + 10 || temp >= 70) {
        level = Level::Warn;
        notes.push(if life <= 10 { "Pouca vida útil restante".to_string() } else { "Temperatura ou reserva pedem atenção".to_string() });
    }
    let gb = |v: u64| format!("{:.1} TB", v as f64 / 1e12).replace('.', crate::i18n::comma());
    Some(Health {
        level,
        summary: if notes.is_empty() { "Sem sinais de problema".to_string() } else { notes.join(" · ") },
        temp: Some(temp),
        hours: Some(hours),
        details: vec![
            ("Vida útil restante".into(), format!("{life}%")),
            ("Reserva disponível".into(), format!("{spare}% (mínimo {spare_min}%)")),
            ("Total escrito".into(), gb(written)),
            ("Total lido".into(), gb(read)),
            ("Vezes ligado".into(), cycles.to_string()),
            ("Desligamentos incorretos".into(), unsafe_off.to_string()),
            ("Erros de mídia".into(), errors.to_string()),
        ],
    })
}

struct Attr {
    id: u8,
    cur: u8,
    raw: u64,
    threshold: u8,
}

fn parse_attrs(data: &[u8], thresholds: Option<&[u8]>) -> Vec<Attr> {
    let mut out = Vec::new();
    for i in 0..30 {
        let o = 2 + i * 12;
        let id = data[o];
        if id == 0 {
            continue;
        }
        let mut raw = 0u64;
        for b in (0..6).rev() {
            raw = raw << 8 | data[o + 5 + b] as u64;
        }
        let threshold = thresholds
            .and_then(|t| (0..30).map(|j| &t[2 + j * 12..14 + j * 12]).find(|e| e[0] == id).map(|e| e[1]))
            .unwrap_or(0);
        out.push(Attr { id, cur: data[o + 3], raw, threshold });
    }
    out
}

fn ata_task(h: HANDLE, feature: u8, command: u8, data: bool) -> Option<([u8; 512], [u8; 8])> {
    let mut b = vec![0u8; 48 + 512];
    b[..2].copy_from_slice(&48u16.to_le_bytes());
    let flags: u16 = if data { 1 | 2 } else { 1 };
    b[2..4].copy_from_slice(&flags.to_le_bytes());
    if data {
        b[8..12].copy_from_slice(&512u32.to_le_bytes());
    }
    b[12..16].copy_from_slice(&10u32.to_le_bytes());
    b[24..32].copy_from_slice(&48u64.to_le_bytes());
    b[40..48].copy_from_slice(&[feature, 1, 1, 0x4F, 0xC2, 0xA0, command, 0]);
    let input = b.clone();
    ioctl(h, ATA_PASS, &input, &mut b)?;
    let mut out = [0u8; 512];
    out.copy_from_slice(&b[48..560]);
    let mut tf = [0u8; 8];
    tf.copy_from_slice(&b[40..48]);
    Some((out, tf))
}

/// Mesmo pedido embrulhado em SCSI, que é como adaptadores USB aceitam.
fn sat_task(h: HANDLE, feature: u8) -> Option<[u8; 512]> {
    const HEAD: usize = 56;
    const DATA: usize = 96;
    let mut b = vec![0u8; DATA + 512];
    b[..2].copy_from_slice(&(HEAD as u16).to_le_bytes());
    b[6] = 16;
    b[7] = 32;
    b[8] = 1;
    b[12..16].copy_from_slice(&512u32.to_le_bytes());
    b[16..20].copy_from_slice(&10u32.to_le_bytes());
    b[24..32].copy_from_slice(&(DATA as u64).to_le_bytes());
    b[32..36].copy_from_slice(&(HEAD as u32).to_le_bytes());
    let cdb = [0x85, 0x08, 0x0E, 0, feature, 0, 1, 0, 1, 0, 0x4F, 0, 0xC2, 0xA0, 0xB0, 0];
    b[36..52].copy_from_slice(&cdb);
    let input = b.clone();
    ioctl(h, SCSI_PASS, &input, &mut b)?;
    if b[2] != 0 {
        return None;
    }
    let mut out = [0u8; 512];
    out.copy_from_slice(&b[DATA..DATA + 512]);
    // Adaptador que ignora o comando devolve tudo zerado.
    out.iter().any(|v| *v != 0).then_some(out)
}

fn judge_smart(attrs: &[Attr], failing: bool, ssd: bool) -> Health {
    let get = |id: u8| attrs.iter().find(|a| a.id == id);
    let raw = |id: u8| get(id).map_or(0, |a| a.raw);
    let realloc = raw(5);
    let pending = raw(197);
    let uncorrect = raw(198);
    let reported = raw(187).max(raw(188));
    let mut notes = Vec::new();
    let mut level = Level::Good;
    if failing {
        level = Level::Risk;
        notes.push("O próprio disco prevê falha".to_string());
    }
    if let Some(a) = attrs.iter().find(|a| a.threshold != 0 && a.cur != 0 && a.cur <= a.threshold) {
        level = Level::Risk;
        notes.push(format!("Atributo {} abaixo do limite do fabricante", a.id));
    }
    if pending > 0 || uncorrect > 0 {
        level = Level::Risk;
        notes.push(format!("{} setores instáveis", pending.max(uncorrect)));
    }
    if realloc > 0 || reported > 0 || raw(10) > 0 {
        if level != Level::Risk {
            level = Level::Warn;
        }
        notes.push(format!("{realloc} setores realocados"));
    }
    let temp = get(194).or(get(190)).map(|a| (a.raw & 0xff) as i32);
    if temp.is_some_and(|t| t >= 55) && level == Level::Good {
        level = Level::Warn;
        notes.push("Temperatura alta".to_string());
    }
    let hours = get(9).map(|a| a.raw & 0xffff_ffff);
    let life = if ssd {
        [231u8, 202, 233, 177, 169].iter().find_map(|id| get(*id)).map(|a| a.cur as u32).filter(|v| (1..=100).contains(v))
    } else {
        None
    };
    if life.is_some_and(|l| l <= 10) && level == Level::Good {
        level = Level::Warn;
        notes.push("Pouca vida útil restante".to_string());
    }
    let mut details = Vec::new();
    if let Some(l) = life {
        details.push(("Vida útil restante".to_string(), format!("{l}%")));
    }
    details.extend([
        ("Setores realocados".to_string(), realloc.to_string()),
        ("Setores pendentes".to_string(), pending.to_string()),
        ("Setores incorrigíveis".to_string(), uncorrect.to_string()),
    ]);
    if let Some(a) = get(12) {
        details.push(("Vezes ligado".to_string(), a.raw.to_string()));
    }
    if let Some(a) = get(199) {
        details.push(("Erros de cabo".to_string(), a.raw.to_string()));
    }
    if let Some(a) = get(4) {
        details.push(("Partidas do motor".to_string(), a.raw.to_string()));
    }
    Health {
        level,
        summary: if notes.is_empty() { "Sem sinais de problema".to_string() } else { notes.join(" · ") },
        temp,
        hours,
        details,
    }
}

fn read_health(number: u32, bus: u32, ssd: bool) -> Health {
    let none = |s: &str| Health { summary: s.to_string(), ..Default::default() };
    let access = (GENERIC_READ.0 | GENERIC_WRITE.0) as u32;
    let Some(h) = open(&format!("\\\\.\\PhysicalDrive{number}"), access) else {
        return none("Precisa do Prisma como administrador");
    };
    let out = if bus == BUS_NVME {
        nvme(h)
    } else {
        let mut res = None;
        if let Some((data, _)) = ata_task(h, 0xD0, 0xB0, true) {
            let th = ata_task(h, 0xD1, 0xB0, true).map(|v| v.0);
            let failing = ata_task(h, 0xDA, 0xB0, false).is_some_and(|(_, t)| t[3] == 0xF4 && t[4] == 0x2C);
            let attrs = parse_attrs(&data, th.as_ref().map(|t| &t[..]));
            if !attrs.is_empty() {
                res = Some(judge_smart(&attrs, failing, ssd));
            }
        }
        if res.is_none() {
            if let Some(data) = sat_task(h, 0xD0) {
                let th = sat_task(h, 0xD1);
                let attrs = parse_attrs(&data, th.as_ref().map(|t| &t[..]));
                if !attrs.is_empty() {
                    res = Some(judge_smart(&attrs, false, ssd));
                }
            }
        }
        res
    };
    unsafe {
        let _ = CloseHandle(h);
    }
    out.unwrap_or_else(|| none(if bus == BUS_USB { "O adaptador USB não libera os dados de saúde" } else { "Este disco não respondeu" }))
}

pub type HealthMap = Arc<Mutex<HashMap<u32, Health>>>;

/// Lê a saúde de todos em segundo plano e preenche `map` conforme termina.
pub fn spawn_health(list: &[Disk], map: HealthMap) {
    let items: Vec<(u32, u32, bool)> = list.iter().map(|d| (d.number, d.bus, d.ssd)).collect();
    let _ = std::thread::Builder::new().name("saude".into()).spawn(move || {
        let bad = crate::sys::events::bad_blocks(30);
        for (n, bus, ssd) in items {
            let mut h = read_health(n, bus, ssd);
            // O Windows pode dizer Íntegro enquanto registra blocos ruins: o evento pesa na nota.
            if let Some((_, count, when)) = bad.iter().find(|b| b.0 == n) {
                let note = format!("{count} blocos com erro em 30 dias");
                h.summary = if h.level == Level::Good || h.level == Level::Unknown {
                    format!("{note}, o último em {}", when.replace('T', " "))
                } else {
                    format!("{} · {note}", h.summary)
                };
                if h.level != Level::Risk {
                    h.level = Level::Warn;
                }
                h.details.push(("Blocos com erro (30 dias)".to_string(), count.to_string()));
            }
            map.lock().unwrap().insert(n, h);
        }
    });
}

// ---------- Ferramentas do Windows: CHKDSK, formatar, partições e letra ----------
// Tudo roda pelo PowerShell do Windows como administrador, escondido, e conta o andamento num
// arquivo que a página lê. O comando sai só de `script`, que valida o alvo e bloqueia o disco do
// Windows: letra e número vêm da lista da página, o rótulo é o único texto livre e vai entre aspas.

/// Sistemas de arquivos de Formatar e Criar partição.
pub const FILE_SYSTEMS: [&str; 3] = ["NTFS", "exFAT", "FAT32"];

#[derive(Clone, Debug, PartialEq)]
pub enum Job {
    /// CHKDSK só leitura.
    Scan(char),
    /// CHKDSK /f /x; com `sectors`, /r (procura setores ruins).
    Repair { letter: char, sectors: bool },
    Format { letter: char, fs: usize, label: String, quick: bool },
    /// Letra nova; None tira a letra.
    Letter { letter: char, to: Option<char> },
    Delete(char),
    /// Partição nova no espaço livre: tamanho em GB, ou None para todo o maior espaço livre.
    Create { disk: u32, gb: Option<u64>, fs: usize, label: String },
    Resize { letter: char, gb: u64 },
}

/// Rótulo aceito pelo sistema de arquivos: sem caracteres de controle nem os proibidos, no tamanho dele.
fn check_label(label: &str, fs: usize) -> std::result::Result<(), &'static str> {
    let max = match fs {
        0 => 32,
        1 => 15,
        2 => 11,
        _ => return Err("Sistema de arquivos desconhecido."),
    };
    if label.chars().count() > max {
        return Err(match max {
            32 => "Rótulo longo demais: até 32 letras no NTFS.",
            15 => "Rótulo longo demais: até 15 letras no exFAT.",
            _ => "Rótulo longo demais: até 11 letras no FAT32.",
        });
    }
    if label.chars().any(|c| c.is_control() || "\"*/:<>?\\|".contains(c)) {
        return Err("O rótulo não aceita \" * / : < > ? \\ |");
    }
    Ok(())
}

/// Bytes de `gb` (GB de 1000), descendo para múltiplo de 1 MiB: partições alinham assim.
fn gb_bytes(gb: u64) -> u64 {
    gb.saturating_mul(1_000_000_000) / (1 << 20) * (1 << 20)
}

/// Por que o Prisma não monta este pedido, ou Ok. `windows` é a letra do Windows e `system`, as
/// letras do disco onde ele está.
pub fn refuse(job: &Job, windows: char, system: &[char]) -> std::result::Result<(), &'static str> {
    let letter = match job {
        Job::Scan(l) | Job::Delete(l) | Job::Repair { letter: l, .. } | Job::Format { letter: l, .. } | Job::Letter { letter: l, .. } | Job::Resize { letter: l, .. } => Some(*l),
        Job::Create { .. } => None,
    };
    if letter.is_some_and(|l| !l.is_ascii_uppercase()) {
        return Err("Letra de volume inválida.");
    }
    let on_system = letter.is_some_and(|l| l == windows || system.contains(&l));
    const LOCKED: &str = "Disco do Windows: formatar e apagar ficam bloqueados.";
    match job {
        Job::Format { .. } | Job::Delete(_) if on_system => return Err(LOCKED),
        Job::Format { fs, label, .. } => check_label(label, *fs)?,
        Job::Create { gb: Some(0), .. } => return Err("Tamanho inválido."),
        Job::Create { fs, label, .. } => check_label(label, *fs)?,
        Job::Letter { letter, .. } if *letter == windows => return Err("A letra do Windows não muda."),
        Job::Letter { letter, to: Some(t) } if !t.is_ascii_uppercase() || t == letter => return Err("Escolha outra letra."),
        Job::Resize { gb, .. } if *gb == 0 || *gb > 1_000_000 => return Err("Tamanho inválido."),
        _ => {}
    }
    Ok(())
}

/// O script do PowerShell que faz o pedido e conta tudo em `log`. A última linha do log é
/// `@@fim <código>`: o código de saída do CHKDSK, 0 para os outros, -1 em erro.
pub fn script(job: &Job, windows: char, system: &[char], log: &str) -> std::result::Result<String, &'static str> {
    use crate::sys::quick::ps_quote;
    refuse(job, windows, system)?;
    let chk = |l: char, args: &str, answer: bool| {
        // O 'Y' responde à pergunta do CHKDSK de marcar o reparo para a próxima reinicialização.
        let pipe = if answer { "'Y' | " } else { "" };
        format!("{pipe}& (Join-Path $env:SystemRoot 'System32\\chkdsk.exe') {l}:{args} 2>&1 | ForEach-Object {{ Say $_ }}\nSay \"@@fim $LASTEXITCODE\"")
    };
    let fmt = |fs: usize, label: &str| format!("-FileSystem {} -NewFileSystemLabel {} -Confirm:$false -ErrorAction Stop", FILE_SYSTEMS[fs], ps_quote(label));
    let body = match job {
        Job::Scan(l) => chk(*l, "", false),
        Job::Repair { letter, sectors } => chk(*letter, if *sectors { " /r /x" } else { " /f /x" }, true),
        Job::Format { letter, fs, label, quick } => {
            format!("Format-Volume -DriveLetter {letter} {}{} | Out-Null\nSay '@@fim 0'", fmt(*fs, label), if *quick { "" } else { " -Full" })
        }
        Job::Letter { letter, to: Some(t) } => format!("Get-Partition -DriveLetter {letter} -ErrorAction Stop | Set-Partition -NewDriveLetter {t} -ErrorAction Stop\nSay '@@fim 0'"),
        Job::Letter { letter, to: None } => {
            format!("Get-Partition -DriveLetter {letter} -ErrorAction Stop | Remove-PartitionAccessPath -AccessPath '{letter}:\\' -ErrorAction Stop\nSay '@@fim 0'")
        }
        Job::Delete(l) => format!("Remove-Partition -DriveLetter {l} -Confirm:$false -ErrorAction Stop\nSay '@@fim 0'"),
        Job::Create { disk, gb, fs, label } => {
            let size = gb.map_or("-UseMaximumSize".to_string(), |g| format!("-Size {}", gb_bytes(g)));
            format!(
                "if ((Get-Disk -Number {disk} -ErrorAction Stop).PartitionStyle -eq 'RAW') {{ Initialize-Disk -Number {disk} -PartitionStyle GPT -ErrorAction Stop }}\n\
                 New-Partition -DiskNumber {disk} {size} -AssignDriveLetter -ErrorAction Stop | Format-Volume {} | Out-Null\nSay '@@fim 0'",
                fmt(*fs, label)
            )
        }
        Job::Resize { letter, gb } => format!(
            "$p = Get-Partition -DriveLetter {letter} -ErrorAction Stop\n$s = $p | Get-PartitionSupportedSize -ErrorAction Stop\nSay (\"@@faixa {{0}} {{1}}\" -f $s.SizeMin, $s.SizeMax)\n\
             $p | Resize-Partition -Size {} -ErrorAction Stop\nSay '@@fim 0'",
            gb_bytes(*gb)
        ),
    };
    Ok(format!(
        "$log = {}\nfunction Say($t) {{ Add-Content -LiteralPath $log -Value $t -Encoding UTF8 }}\ntry {{\n{body}\n}} catch {{ Say $_.Exception.Message; Say '@@fim -1' }}\n",
        ps_quote(log)
    ))
}

/// Roda o script como administrador, escondido. O Windows pergunta; recusado, devolve false.
pub fn run_job(script: &str) -> bool {
    let ps = crate::sys::system_tool("powershell").display().to_string();
    let args = format!("-NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -EncodedCommand {}", crate::sys::quick::utf16_b64(script));
    crate::sys::quick::run_admin(&ps, &args, &crate::sys::system_dir().display().to_string())
}

/// O que o log contou até agora.
#[derive(Default, Debug, PartialEq)]
pub struct Progress {
    pub lines: Vec<String>,
    /// O código de `@@fim`, quando terminou.
    pub done: Option<i32>,
    /// Tamanho mínimo e máximo da partição, em bytes, de Redimensionar.
    pub range: Option<(u64, u64)>,
}

/// O CHKDSK reescreve a linha do andamento com `\r`: fica o último pedaço de cada linha.
pub fn parse_log(text: &str) -> Progress {
    let mut p = Progress::default();
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.split('\r').map(str::trim).rfind(|s| !s.is_empty()).unwrap_or("");
        if let Some(code) = line.strip_prefix("@@fim ") {
            p.done = Some(code.trim().parse().unwrap_or(-1));
        } else if let Some(r) = line.strip_prefix("@@faixa ") {
            let mut n = r.split_whitespace().filter_map(|v| v.parse().ok());
            p.range = n.next().zip(n.next());
        } else if !line.is_empty() {
            p.lines.push(line.to_string());
        }
    }
    p
}

/// Letras sem volume, para Trocar letra (A e B ficam de fora, eram dos disquetes).
pub fn free_letters() -> Vec<char> {
    let mask = unsafe { GetLogicalDrives() };
    (2..26u8).filter(|i| mask & (1 << i) == 0).map(|i| (b'A' + i) as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYS: [char; 2] = ['C', 'D'];

    fn run(job: Job) -> std::result::Result<String, &'static str> {
        script(&job, 'C', &SYS, "C:\\t\\x.log")
    }

    #[test]
    fn windows_disk_cannot_be_formatted_or_deleted() {
        for l in ['C', 'D'] {
            assert!(run(Job::Format { letter: l, fs: 0, label: String::new(), quick: true }).is_err());
            assert!(run(Job::Delete(l)).is_err());
        }
        assert!(run(Job::Letter { letter: 'C', to: Some('X') }).is_err());
        assert!(run(Job::Letter { letter: 'C', to: None }).is_err());
        // Ler, reparar, redimensionar e trocar a letra de outro volume do mesmo disco continuam.
        assert!(run(Job::Scan('C')).is_ok());
        assert!(run(Job::Repair { letter: 'C', sectors: false }).is_ok());
        assert!(run(Job::Resize { letter: 'C', gb: 100 }).is_ok());
        assert!(run(Job::Letter { letter: 'D', to: Some('X') }).is_ok());
        assert!(run(Job::Delete('E')).is_ok());
    }

    #[test]
    fn rejects_bad_targets() {
        assert!(run(Job::Scan('e')).is_err());
        assert!(run(Job::Scan('1')).is_err());
        assert!(run(Job::Delete(';')).is_err());
        assert!(run(Job::Letter { letter: 'E', to: Some('E') }).is_err());
        assert!(run(Job::Letter { letter: 'E', to: Some('$') }).is_err());
        assert!(run(Job::Resize { letter: 'E', gb: 0 }).is_err());
        assert!(run(Job::Create { disk: 1, gb: Some(0), fs: 0, label: String::new() }).is_err());
        assert!(run(Job::Format { letter: 'E', fs: 9, label: String::new(), quick: true }).is_err());
    }

    #[test]
    fn labels() {
        assert!(check_label("Jogos", 2).is_ok());
        assert!(check_label("Jogos e mais", 2).is_err());
        assert!(check_label("Jogos e mais", 1).is_ok());
        assert!(check_label(&"x".repeat(33), 0).is_err());
        assert!(check_label("a|b", 0).is_err());
        assert!(check_label("a\nb", 0).is_err());
    }

    #[test]
    fn builds_commands() {
        let s = run(Job::Scan('E')).unwrap();
        assert!(s.contains("chkdsk.exe') E: 2>&1") && !s.contains("'Y' |"), "{s}");
        let s = run(Job::Repair { letter: 'E', sectors: true }).unwrap();
        assert!(s.contains("'Y' | & (Join-Path") && s.contains("E: /r /x"), "{s}");
        assert!(run(Job::Repair { letter: 'E', sectors: false }).unwrap().contains("E: /f /x"));
        let s = run(Job::Format { letter: 'E', fs: 1, label: "O'Neil".into(), quick: false }).unwrap();
        assert!(s.contains("Format-Volume -DriveLetter E -FileSystem exFAT -NewFileSystemLabel 'O''Neil' -Confirm:$false -ErrorAction Stop -Full"), "{s}");
        assert!(!run(Job::Format { letter: 'E', fs: 0, label: String::new(), quick: true }).unwrap().contains("-Full"));
        assert!(run(Job::Letter { letter: 'E', to: Some('G') }).unwrap().contains("Get-Partition -DriveLetter E -ErrorAction Stop | Set-Partition -NewDriveLetter G"));
        assert!(run(Job::Letter { letter: 'E', to: None }).unwrap().contains("Remove-PartitionAccessPath -AccessPath 'E:\\'"));
        assert!(run(Job::Delete('E')).unwrap().contains("Remove-Partition -DriveLetter E -Confirm:$false"));
        let s = run(Job::Create { disk: 2, gb: None, fs: 0, label: "Dados".into() }).unwrap();
        assert!(s.contains("New-Partition -DiskNumber 2 -UseMaximumSize -AssignDriveLetter") && s.contains("Initialize-Disk -Number 2 -PartitionStyle GPT"), "{s}");
        assert!(s.starts_with("$log = 'C:\\t\\x.log'"));
        assert!(run(Job::Create { disk: 2, gb: Some(1), fs: 0, label: String::new() }).unwrap().contains("-Size 999292928 "));
        assert!(run(Job::Resize { letter: 'E', gb: 1 }).unwrap().contains("Resize-Partition -Size 999292928 "));
    }

    #[test]
    fn reads_log() {
        let p = parse_log("\u{feff}Etapa 1\r\n10 %\r20 %\r\n\r\n@@faixa 100 2000\r\n@@fim 3\r\n");
        assert_eq!(p.lines, vec!["Etapa 1", "20 %"]);
        assert_eq!(p.done, Some(3));
        assert_eq!(p.range, Some((100, 2000)));
        assert_eq!(parse_log("a").done, None);
        assert_eq!(parse_log("@@fim x").done, Some(-1));
    }
}
