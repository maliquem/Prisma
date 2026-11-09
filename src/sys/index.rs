//! Pesquisa de arquivos pelo nome em todos os discos. Em NTFS com administrador, o índice sai
//! da tabela de arquivos do volume em segundos e fica em dia pelo diário de mudanças do
//! Windows (USN), sem varrer nada de novo. Sem NTFS ou sem administrador, percorre as pastas e
//! refaz de tempos em tempos. O índice fica salvo para a pesquisa responder logo ao abrir.

use std::collections::HashMap;
use std::ffi::c_void;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use super::mft;

const USED: u8 = 1;
const DIR: u8 = 2;
/// Raiz da tabela do NTFS.
const NTFS_ROOT: u32 = 5;
/// Pasta das tabelas internas do NTFS ($Extend): nada dela aparece.
const NTFS_EXTEND: u32 = 11;

const QUERY_USN: u32 = 0x0009_00f4;
const READ_USN: u32 = 0x0009_00bb;
const R_DATA: u32 = 0x1 | 0x2 | 0x4;
const R_CREATE: u32 = 0x100;
const R_DELETE: u32 = 0x200;
const R_RENAME_OLD: u32 = 0x1000;
const R_RENAME_NEW: u32 = 0x2000;
const R_BASIC: u32 = 0x8000;
const R_CLOSE: u32 = 0x8000_0000;

/// Sem o diário, o índice do disco é refeito nesse intervalo.
const RESCAN: Duration = Duration::from_secs(15 * 60);
const SAVE_EVERY: Duration = Duration::from_secs(30 * 60);
const POLL: Duration = Duration::from_secs(2);
/// Resultados guardados por pesquisa. O total encontrado continua contado.
pub const CAP: usize = 200_000;
/// Tamanhos lidos de novo por volta do diário. O resto fica para a próxima.
const REFRESH_MAX: usize = 4000;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Entry {
    parent: u32,
    name_off: u32,
    name_len: u16,
    flags: u8,
    _pad: u8,
    /// Modificado em, segundos desde 1970.
    mtime: u32,
    size: u64,
}

struct Vol {
    letter: char,
    /// Os números são os registros da tabela do NTFS, e o diário mantém tudo em dia.
    ntfs: bool,
    entries: Vec<Entry>,
    names: Vec<u8>,
    root: u32,
    serial: u32,
    journal: u64,
    next_usn: i64,
    /// Última leitura completa, em segundos desde 1970.
    built: u64,
    garbage: usize,
    dirty: bool,
    files: u32,
    /// O diário está sendo lido nesta sessão.
    live: bool,
}

impl Vol {
    fn name(&self, i: u32) -> &str {
        let e = &self.entries[i as usize];
        let b = self.names.get(e.name_off as usize..e.name_off as usize + e.name_len as usize).unwrap_or(&[]);
        std::str::from_utf8(b).unwrap_or("")
    }

    fn live_entry(&self, i: u32) -> bool {
        i != self.root && self.entries.get(i as usize).is_some_and(|e| e.flags & USED != 0)
    }

    fn set_name(&mut self, i: u32, s: &str) {
        let e = &mut self.entries[i as usize];
        self.garbage += e.name_len as usize;
        e.name_off = self.names.len() as u32;
        e.name_len = s.len().min(u16::MAX as usize) as u16;
        self.names.extend_from_slice(&s.as_bytes()[..e.name_len as usize]);
    }

    fn path(&self, i: u32) -> String {
        let mut parts = Vec::new();
        let mut c = i;
        while c != self.root && parts.len() < 400 {
            let Some(e) = self.entries.get(c as usize) else { break };
            parts.push(self.name(c));
            c = e.parent;
        }
        let mut s = format!("{}:", self.letter);
        if parts.is_empty() {
            s.push('\\');
        }
        for p in parts.iter().rev() {
            s.push('\\');
            s.push_str(p);
        }
        s
    }

    /// Junta os nomes vivos de novo: renomear e apagar deixam sobras no meio.
    fn compact(&mut self) {
        let mut names = Vec::with_capacity(self.names.len() - self.garbage.min(self.names.len()));
        for e in self.entries.iter_mut() {
            if e.flags & USED == 0 {
                e.name_len = 0;
                e.name_off = 0;
                continue;
            }
            let (o, l) = (e.name_off as usize, e.name_len as usize);
            e.name_off = names.len() as u32;
            names.extend_from_slice(self.names.get(o..o + l).unwrap_or(&[]));
        }
        self.names = names;
        self.garbage = 0;
    }

    fn memory(&self) -> usize {
        self.entries.capacity() * std::mem::size_of::<Entry>() + self.names.capacity()
    }
}

/// Apaga o que não tem caminho até a raiz: tabelas internas e restos de pastas apagadas.
fn prune(entries: &mut [Entry], root: u32) {
    let mut state = vec![0u8; entries.len()];
    let mut chain = Vec::new();
    for i in 0..entries.len() {
        if entries[i].flags & USED == 0 || state[i] != 0 {
            continue;
        }
        chain.clear();
        let mut c = i as u32;
        let ok = loop {
            if c == root {
                break 1;
            }
            let Some(e) = entries.get(c as usize) else { break 2 };
            if state[c as usize] != 0 {
                break state[c as usize];
            }
            if e.flags & USED == 0 || chain.len() > 400 {
                break 2;
            }
            chain.push(c);
            c = e.parent;
        };
        for &x in &chain {
            state[x as usize] = ok;
        }
    }
    for (i, e) in entries.iter_mut().enumerate() {
        if state[i] == 2 {
            e.flags = 0;
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn from_table(letter: char, t: mft::Table, serial: u32, journal: u64, next_usn: i64) -> Vol {
    let mut entries: Vec<Entry> = t
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let used = n.used() && n.name_len > 0;
            Entry {
                parent: n.parent,
                name_off: n.name_off,
                name_len: n.name_len,
                flags: if used { USED | (n.flags & mft::DIR) } else { 0 },
                _pad: 0,
                mtime: t.times.get(i).copied().unwrap_or(0),
                size: if n.is_dir() { 0 } else { n.size },
            }
        })
        .collect();
    for (i, e) in entries.iter_mut().enumerate().take(24) {
        if i as u32 != NTFS_ROOT {
            e.flags = 0;
        }
    }
    if let Some(r) = entries.get_mut(NTFS_ROOT as usize) {
        *r = Entry { parent: NTFS_ROOT, flags: USED | DIR, ..Default::default() };
    }
    if let Some(x) = entries.get_mut(NTFS_EXTEND as usize) {
        x.flags = 0;
    }
    prune(&mut entries, NTFS_ROOT);
    let files = entries.iter().filter(|e| e.flags & USED != 0).count() as u32;
    Vol {
        letter,
        ntfs: true,
        entries,
        names: t.names,
        root: NTFS_ROOT,
        serial,
        journal,
        next_usn,
        built: now_secs(),
        garbage: 0,
        dirty: true,
        files,
        live: journal != 0,
    }
}

fn mtime_of(m: &std::fs::Metadata) -> u32 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs().min(u32::MAX as u64) as u32)
}

/// Caminho lento: percorre as pastas. Atalhos de pasta (junções) não são seguidos.
fn walk(letter: char, serial: u32, p: &mft::Progress) -> Option<Vol> {
    let mut entries = vec![Entry { flags: USED | DIR, ..Default::default() }];
    let mut names: Vec<u8> = Vec::new();
    let mut stack = vec![(0u32, PathBuf::from(format!("{letter}:\\")))];
    while let Some((id, dir)) = stack.pop() {
        if p.cancel.load(Ordering::Relaxed) || !active() {
            return None;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            let name = e.file_name().to_string_lossy().to_string();
            let i = entries.len() as u32;
            let len = name.len().min(u16::MAX as usize);
            let mut n = Entry { parent: id, name_off: names.len() as u32, name_len: len as u16, flags: USED, mtime: mtime_of(&meta), ..Default::default() };
            names.extend_from_slice(&name.as_bytes()[..len]);
            if meta.is_dir() {
                n.flags |= DIR;
                if !meta.file_type().is_symlink() {
                    stack.push((i, e.path()));
                }
            } else {
                n.size = meta.len();
            }
            entries.push(n);
            p.done.fetch_add(1, Ordering::Relaxed);
        }
    }
    let files = entries.len() as u32 - 1;
    Some(Vol { letter, ntfs: false, entries, names, root: 0, serial, journal: 0, next_usn: 0, built: now_secs(), garbage: 0, dirty: true, files, live: false })
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn open_volume(letter: char) -> Option<Handle> {
    unsafe {
        CreateFileW(
            &HSTRING::from(format!("\\\\.\\{letter}:")),
            GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
        .ok()
        .map(Handle)
    }
}

/// Número de série e se o sistema de arquivos é NTFS.
fn volume_info(letter: char) -> Option<(u32, bool)> {
    let mut serial = 0u32;
    let mut fs = [0u16; 32];
    unsafe { GetVolumeInformationW(&HSTRING::from(format!("{letter}:\\")), None, Some(&mut serial), None, None, Some(&mut fs)).ok()? };
    let n = fs.iter().position(|&c| c == 0).unwrap_or(fs.len());
    Some((serial, String::from_utf16_lossy(&fs[..n]).eq_ignore_ascii_case("NTFS")))
}

/// Identificador do diário e o próximo número dele.
fn query_journal(h: &Handle) -> Option<(u64, i64)> {
    let mut out = [0u8; 80];
    let mut got = 0u32;
    unsafe { DeviceIoControl(h.0, QUERY_USN, None, 0, Some(out.as_mut_ptr() as *mut c_void), 80, Some(&mut got), None).ok()? };
    Some((u64::from_le_bytes(out[0..8].try_into().unwrap()), i64::from_le_bytes(out[16..24].try_into().unwrap())))
}

/// Tudo o que mudou desde `from`. Erro quer dizer que o diário foi refeito ou já esqueceu: lê de novo.
fn read_journal(h: &Handle, journal: u64, mut from: i64, out: &mut Vec<u8>) -> std::result::Result<i64, ()> {
    let mut buf = vec![0u8; 1 << 16];
    for _ in 0..4096 {
        let mut input = [0u8; 40];
        input[0..8].copy_from_slice(&from.to_le_bytes());
        input[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        input[32..40].copy_from_slice(&journal.to_le_bytes());
        let mut got = 0u32;
        let ok = unsafe { DeviceIoControl(h.0, READ_USN, Some(input.as_ptr() as *const c_void), 40, Some(buf.as_mut_ptr() as *mut c_void), buf.len() as u32, Some(&mut got), None) };
        if ok.is_err() || got < 8 {
            return Err(());
        }
        let next = i64::from_le_bytes(buf[0..8].try_into().unwrap());
        if got == 8 || next == from {
            return Ok(next);
        }
        out.extend_from_slice(&buf[8..got as usize]);
        from = next;
    }
    Ok(from)
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Aplica os registros do diário. Devolve os arquivos que precisam ter o tamanho lido de novo.
fn apply(v: &mut Vol, data: &[u8], refresh: &mut HashMap<u32, u64>) -> bool {
    let mut changed = false;
    let mut o = 0;
    while o + 60 <= data.len() {
        let len = u32_at(data, o) as usize;
        if len < 60 || o + len > data.len() {
            break;
        }
        let r = &data[o..o + len];
        o += len;
        if u16_at(r, 4) != 2 {
            continue;
        }
        let frn = u64_at(r, 8);
        let idx = frn as u32;
        let parent = u64_at(r, 16) as u32;
        let ts = u64_at(r, 32);
        let reason = u32_at(r, 40);
        let attr = u32_at(r, 52);
        let (nl, no) = (u16_at(r, 56) as usize, u16_at(r, 58) as usize);
        if no + nl > r.len() || idx < 24 || idx >= 1 << 28 {
            continue;
        }
        if idx as usize >= v.entries.len() {
            v.entries.resize(idx as usize + 1, Entry::default());
        }
        let was = v.entries[idx as usize].flags & USED != 0;
        if reason & R_DELETE != 0 {
            if reason & R_CLOSE != 0 && was {
                v.entries[idx as usize].flags = 0;
                v.garbage += v.entries[idx as usize].name_len as usize;
                v.files = v.files.saturating_sub(1);
                refresh.remove(&idx);
                changed = true;
            }
            continue;
        }
        if reason & R_RENAME_OLD != 0 && reason & R_RENAME_NEW == 0 {
            continue;
        }
        let w: Vec<u16> = r[no..no + nl].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let name = String::from_utf16_lossy(&w);
        let dir = attr & 0x10 != 0;
        let e = v.entries[idx as usize];
        if !was || e.parent != parent || v.name(idx) != name {
            v.set_name(idx, &name);
            let e = &mut v.entries[idx as usize];
            e.parent = parent;
            e.flags = USED | if dir { DIR } else { 0 };
            if !was {
                e.size = 0;
                e.mtime = mft::unix_secs(ts);
                v.files += 1;
            }
            changed = true;
        }
        if !dir && reason & (R_DATA | R_CREATE | R_BASIC) != 0 {
            v.entries[idx as usize].mtime = mft::unix_secs(ts);
            if reason & R_CLOSE != 0 {
                refresh.insert(idx, frn);
            }
            changed = true;
        }
    }
    if changed {
        v.dirty = true;
    }
    changed
}

/// Tamanho e data de um arquivo pelo número dele na tabela, sem precisar do caminho.
fn file_info(vol: &Handle, frn: u64) -> Option<(u64, u32)> {
    unsafe {
        let d = FILE_ID_DESCRIPTOR { dwSize: std::mem::size_of::<FILE_ID_DESCRIPTOR>() as u32, Type: FileIdType, Anonymous: FILE_ID_DESCRIPTOR_0 { FileId: frn as i64 } };
        let h = Handle(OpenFileById(vol.0, &d, FILE_READ_ATTRIBUTES.0, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, None, FILE_FLAG_BACKUP_SEMANTICS).ok()?);
        let mut bi = BY_HANDLE_FILE_INFORMATION::default();
        GetFileInformationByHandle(h.0, &mut bi).ok()?;
        let ft = ((bi.ftLastWriteTime.dwHighDateTime as u64) << 32) | bi.ftLastWriteTime.dwLowDateTime as u64;
        Some((((bi.nFileSizeHigh as u64) << 32) | bi.nFileSizeLow as u64, mft::unix_secs(ft)))
    }
}

// ---------- Gravação em disco ----------

const MAGIC: &[u8; 4] = b"PRX1";
const HEADER: usize = 56;

fn store_dir() -> PathBuf {
    let base = if crate::startup::is_installed() {
        crate::sys::known_folder(&windows::Win32::UI::Shell::FOLDERID_LocalAppData).join("Prisma")
    } else {
        crate::config::data_dir()
    };
    base.join("indice")
}

fn file_for(letter: char) -> PathBuf {
    store_dir().join(format!("{letter}.idx"))
}

fn save(v: &Vol) -> bool {
    let dir = store_dir();
    let _ = std::fs::create_dir_all(&dir);
    let tmp = dir.join(format!("{}.tmp", v.letter));
    let ok = (|| -> std::io::Result<()> {
        let mut f = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(&tmp)?);
        f.write_all(MAGIC)?;
        f.write_all(&[v.letter as u8, v.ntfs as u8, 0, 0])?;
        f.write_all(&v.root.to_le_bytes())?;
        f.write_all(&v.serial.to_le_bytes())?;
        f.write_all(&v.journal.to_le_bytes())?;
        f.write_all(&v.next_usn.to_le_bytes())?;
        f.write_all(&v.built.to_le_bytes())?;
        f.write_all(&v.files.to_le_bytes())?;
        f.write_all(&(v.entries.len() as u32).to_le_bytes())?;
        f.write_all(&(v.names.len() as u32).to_le_bytes())?;
        f.write_all(&[0u8; 4])?;
        // Entry é repr(C) sem ponteiros: os bytes dela são o formato do arquivo.
        let raw = unsafe { std::slice::from_raw_parts(v.entries.as_ptr() as *const u8, v.entries.len() * std::mem::size_of::<Entry>()) };
        f.write_all(raw)?;
        f.write_all(&v.names)?;
        f.flush()?;
        Ok(())
    })()
    .is_ok();
    ok && std::fs::rename(&tmp, file_for(v.letter)).is_ok()
}

fn load(letter: char) -> Option<Vol> {
    let b = std::fs::read(file_for(letter)).ok()?;
    if b.len() < HEADER || &b[0..4] != MAGIC || b[4] != letter as u8 {
        return None;
    }
    let n = u32_at(&b, 44) as usize;
    let nl = u32_at(&b, 48) as usize;
    let es = std::mem::size_of::<Entry>();
    if b.len() != HEADER + n * es + nl {
        return None;
    }
    let mut entries = vec![Entry::default(); n];
    unsafe { std::ptr::copy_nonoverlapping(b[HEADER..].as_ptr(), entries.as_mut_ptr() as *mut u8, n * es) };
    let names = b[HEADER + n * es..].to_vec();
    let root = u32_at(&b, 8);
    if root as usize >= n {
        return None;
    }
    Some(Vol {
        letter,
        ntfs: b[5] != 0,
        entries,
        names,
        root,
        serial: u32_at(&b, 12),
        journal: u64_at(&b, 16),
        next_usn: u64_at(&b, 24) as i64,
        built: u64_at(&b, 32),
        garbage: 0,
        dirty: false,
        files: u32_at(&b, 40),
        live: false,
    })
}

// ---------- Estado compartilhado ----------

#[derive(Clone, Copy, PartialEq)]
pub enum State {
    /// Na fila, esperando outro disco terminar.
    Waiting,
    Reading,
    Ready,
}

#[derive(Clone)]
pub struct Drive {
    pub letter: char,
    pub state: State,
    pub files: u32,
    /// Atualiza na hora pelo diário. Sem ele, refaz a cada 15 minutos.
    pub live: bool,
    pub done: u64,
    pub total: u64,
}

struct Index {
    vols: RwLock<Vec<Vol>>,
    /// Ligado e as letras escolhidas.
    wanted: Mutex<(bool, Vec<char>)>,
    version: AtomicU64,
    progress: mft::Progress,
    building: Mutex<Option<char>>,
    refresh: AtomicBool,
    /// Ao desligar, apaga o índice salvo em vez de gravar.
    wipe: AtomicBool,
    thread: Mutex<Option<std::thread::Thread>>,
}

static IX: LazyLock<Index> = LazyLock::new(|| Index {
    vols: RwLock::new(Vec::new()),
    wanted: Mutex::new((false, Vec::new())),
    version: AtomicU64::new(1),
    progress: mft::Progress::default(),
    building: Mutex::new(None),
    refresh: AtomicBool::new(false),
    wipe: AtomicBool::new(false),
    thread: Mutex::new(None),
});

fn active() -> bool {
    IX.wanted.lock().unwrap().0
}

fn bump() {
    IX.version.fetch_add(1, Ordering::Relaxed);
}

fn wake() {
    if let Some(t) = IX.thread.lock().unwrap().as_ref() {
        t.unpark();
    }
}

/// Liga ou desliga e diz quais discos entram. Desligar libera a memória.
pub fn set(on: bool, drives: Vec<char>) {
    {
        let mut w = IX.wanted.lock().unwrap();
        if w.0 == on && w.1 == drives {
            return;
        }
        *w = (on, drives);
    }
    if on {
        IX.wipe.store(false, Ordering::Relaxed);
        let mut t = IX.thread.lock().unwrap();
        if t.is_none() {
            if let Ok(h) = std::thread::Builder::new().name("indice".into()).spawn(run) {
                *t = Some(h.thread().clone());
            }
        }
    } else {
        IX.progress.cancel.store(true, Ordering::Relaxed);
    }
    wake();
}

/// Desliga e apaga o índice salvo.
pub fn turn_off() {
    IX.wipe.store(true, Ordering::Relaxed);
    set(false, Vec::new());
}

/// Lê todos os discos de novo.
pub fn refresh() {
    IX.refresh.store(true, Ordering::Relaxed);
    wake();
}

/// Grava o que mudou. Chamado ao sair do Prisma.
pub fn flush() {
    for v in IX.vols.read().unwrap().iter() {
        if v.dirty {
            save(v);
        }
    }
}

pub fn version() -> u64 {
    IX.version.load(Ordering::Relaxed)
}

pub fn memory() -> usize {
    IX.vols.read().unwrap().iter().map(|v| v.memory()).sum()
}

pub fn status() -> Vec<Drive> {
    let (on, wanted) = IX.wanted.lock().unwrap().clone();
    if !on {
        return Vec::new();
    }
    let vols = IX.vols.read().unwrap();
    let building = *IX.building.lock().unwrap();
    wanted
        .iter()
        .map(|&l| match vols.iter().find(|v| v.letter == l) {
            Some(v) => Drive { letter: l, state: State::Ready, files: v.files, live: v.live, done: 0, total: 0 },
            None if building == Some(l) => Drive {
                letter: l,
                state: State::Reading,
                files: 0,
                live: false,
                done: IX.progress.done.load(Ordering::Relaxed),
                total: IX.progress.total.load(Ordering::Relaxed),
            },
            None => Drive { letter: l, state: State::Waiting, files: 0, live: false, done: 0, total: 0 },
        })
        .collect()
}

fn present(letter: char) -> bool {
    let mask = unsafe { GetLogicalDrives() };
    let i = (letter as u8).wrapping_sub(b'A');
    i < 26 && mask & (1 << i) != 0
}

/// Lê o disco inteiro. Com administrador e NTFS, pela tabela; senão, pasta por pasta.
fn build(letter: char) -> Option<Vol> {
    let (serial, ntfs) = volume_info(letter)?;
    let p = &IX.progress;
    p.done.store(0, Ordering::Relaxed);
    p.total.store(0, Ordering::Relaxed);
    p.cancel.store(false, Ordering::Relaxed);
    *IX.building.lock().unwrap() = Some(letter);
    bump();
    let mut out = None;
    if ntfs && crate::startup::is_elevated() {
        // O diário antes da tabela: o que mudar durante a leitura chega pela próxima volta.
        let j = open_volume(letter).and_then(|h| query_journal(&h)).unwrap_or((0, 0));
        if let Some(t) = mft::read_table(letter, p, true) {
            out = Some(from_table(letter, t, serial, j.0, j.1));
        }
    }
    if out.is_none() && !p.cancel.load(Ordering::Relaxed) {
        out = walk(letter, serial, p);
    }
    *IX.building.lock().unwrap() = None;
    out
}

/// Índice salvo, conferido e posto em dia pelo diário. Nada disso serve: None.
fn restore(letter: char) -> Option<Vol> {
    let mut v = load(letter)?;
    let (serial, _) = volume_info(letter)?;
    if v.serial != serial {
        return None;
    }
    if v.ntfs && crate::startup::is_elevated() {
        let h = open_volume(letter)?;
        let (id, _) = query_journal(&h)?;
        if id != v.journal || id == 0 {
            return None;
        }
        let mut data = Vec::new();
        v.next_usn = read_journal(&h, v.journal, v.next_usn, &mut data).ok()?;
        let mut refresh = HashMap::new();
        apply(&mut v, &data, &mut refresh);
        for (idx, frn) in refresh.into_iter().take(REFRESH_MAX * 4) {
            if let Some((size, mtime)) = file_info(&h, frn) {
                let e = &mut v.entries[idx as usize];
                e.size = size;
                e.mtime = mtime;
            }
        }
        v.live = true;
        v.dirty = true;
    }
    Some(v)
}

fn insert(v: Vol) {
    let mut vols = IX.vols.write().unwrap();
    vols.retain(|x| x.letter != v.letter);
    vols.push(v);
    vols.sort_by_key(|x| x.letter);
    drop(vols);
    bump();
}

/// Uma volta do diário em cada disco NTFS.
fn poll() {
    let list: Vec<(char, u64, i64)> = IX.vols.read().unwrap().iter().filter(|v| v.live).map(|v| (v.letter, v.journal, v.next_usn)).collect();
    for (letter, journal, from) in list {
        let Some(h) = open_volume(letter) else { continue };
        let mut data = Vec::new();
        let next = read_journal(&h, journal, from, &mut data);
        let mut refresh = HashMap::new();
        let mut changed = false;
        {
            let mut vols = IX.vols.write().unwrap();
            let Some(v) = vols.iter_mut().find(|v| v.letter == letter) else { continue };
            match next {
                Ok(n) => {
                    if n != v.next_usn {
                        v.next_usn = n;
                        v.dirty = true;
                    }
                    changed = apply(v, &data, &mut refresh);
                }
                // O diário esqueceu o que faltava: a tabela é lida de novo.
                Err(()) => {
                    v.live = false;
                    v.built = 0;
                }
            }
        }
        if refresh.is_empty() {
            if changed {
                bump();
            }
            continue;
        }
        let got: Vec<(u32, u64, u32)> =
            refresh.into_iter().take(REFRESH_MAX).filter_map(|(idx, frn)| file_info(&h, frn).map(|(s, m)| (idx, s, m))).collect();
        let mut vols = IX.vols.write().unwrap();
        if let Some(v) = vols.iter_mut().find(|v| v.letter == letter) {
            for (idx, size, mtime) in got {
                if let Some(e) = v.entries.get_mut(idx as usize) {
                    e.size = size;
                    e.mtime = mtime;
                }
            }
        }
        drop(vols);
        bump();
    }
}

fn save_dirty() {
    let mut vols = IX.vols.write().unwrap();
    for v in vols.iter_mut() {
        if v.dirty {
            if v.garbage > v.names.len() / 4 {
                v.compact();
            }
            if save(v) {
                v.dirty = false;
            }
        }
    }
}

fn run() {
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
    let mut last_poll = Instant::now();
    let mut last_save = Instant::now();
    // Leitor de cartão sem cartão também tem letra: não insiste nele a cada volta.
    let mut failed: HashMap<char, Instant> = HashMap::new();
    loop {
        std::thread::park_timeout(Duration::from_millis(500));
        let (on, wanted) = IX.wanted.lock().unwrap().clone();
        if !on {
            if IX.wipe.swap(false, Ordering::Relaxed) {
                IX.vols.write().unwrap().clear();
                if let Ok(rd) = std::fs::read_dir(store_dir()) {
                    for e in rd.flatten() {
                        if e.path().extension().is_some_and(|x| x == "idx" || x == "tmp") {
                            let _ = std::fs::remove_file(e.path());
                        }
                    }
                }
                bump();
            } else if !IX.vols.read().unwrap().is_empty() {
                save_dirty();
                IX.vols.write().unwrap().clear();
                bump();
            }
            continue;
        }
        // Disco tirado da lista: sai da memória e do disco. Disco que sumiu: só da memória.
        let gone: Vec<(char, bool)> =
            IX.vols.read().unwrap().iter().filter(|v| !wanted.contains(&v.letter) || !present(v.letter)).map(|v| (v.letter, wanted.contains(&v.letter))).collect();
        for (l, keep) in gone {
            let mut vols = IX.vols.write().unwrap();
            if let Some(i) = vols.iter().position(|v| v.letter == l) {
                let v = vols.remove(i);
                if keep && v.dirty {
                    save(&v);
                } else if !keep {
                    let _ = std::fs::remove_file(file_for(l));
                }
            }
            drop(vols);
            bump();
        }
        if IX.refresh.swap(false, Ordering::Relaxed) {
            for v in IX.vols.write().unwrap().iter_mut() {
                v.built = 0;
                v.live = false;
            }
        }
        // Um disco por volta, para os outros continuarem em dia.
        let have: Vec<char> = IX.vols.read().unwrap().iter().map(|v| v.letter).collect();
        let retry = |l: &char| failed.get(l).is_none_or(|t| t.elapsed() > Duration::from_secs(60));
        if let Some(&l) = wanted.iter().find(|l| !have.contains(l) && present(**l) && retry(l)) {
            let v = restore(l).or_else(|| build(l));
            if v.is_none() {
                failed.insert(l, Instant::now());
            }
            if let Some(v) = v {
                let fresh = v.dirty;
                insert(v);
                if fresh {
                    save_dirty();
                }
            }
            continue;
        }
        if last_poll.elapsed() >= POLL {
            poll();
            last_poll = Instant::now();
        }
        // Sem o diário: lê de novo quando ficar velho.
        let stale = IX.vols.read().unwrap().iter().find(|v| !v.live && now_secs().saturating_sub(v.built) >= RESCAN.as_secs()).map(|v| v.letter);
        if let Some(l) = stale {
            if let Some(v) = build(l) {
                insert(v);
                save_dirty();
            } else {
                // Não deu para ler agora: tenta de novo depois do intervalo.
                if let Some(v) = IX.vols.write().unwrap().iter_mut().find(|v| v.letter == l) {
                    v.built = now_secs();
                }
            }
            continue;
        }
        if last_save.elapsed() >= SAVE_EVERY {
            save_dirty();
            last_save = Instant::now();
        }
    }
}

// ---------- Pesquisa ----------

/// Minúsculas e sem acento, para "relatorio" achar "Relatório".
pub fn fold(s: &str, out: &mut Vec<u8>) {
    for ch in s.chars() {
        if ch.is_ascii() {
            out.push((ch as u8).to_ascii_lowercase());
            continue;
        }
        for c in ch.to_lowercase() {
            let plain = match c {
                'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
                'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
                'ď' | 'đ' => "d",
                'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
                'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
                'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
                'ł' | 'ĺ' | 'ľ' => "l",
                'ñ' | 'ń' | 'ņ' | 'ň' => "n",
                'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
                'ŕ' | 'ř' => "r",
                'ś' | 'ŝ' | 'ş' | 'š' => "s",
                'ţ' | 'ť' => "t",
                'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
                'ý' | 'ÿ' => "y",
                'ź' | 'ż' | 'ž' => "z",
                'ß' => "ss",
                'æ' => "ae",
                'œ' => "oe",
                _ => {
                    let mut b = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
                    continue;
                }
            };
            out.extend_from_slice(plain.as_bytes());
        }
    }
}

fn has_ascii_ci(h: &[u8], n: &[u8]) -> bool {
    if n.is_empty() {
        return true;
    }
    if n.len() > h.len() {
        return false;
    }
    let f = n[0];
    (0..=h.len() - n.len()).any(|i| h[i].to_ascii_lowercase() == f && h[i..i + n.len()].iter().zip(n).all(|(a, b)| a.to_ascii_lowercase() == *b))
}

fn has(h: &[u8], n: &[u8]) -> bool {
    n.is_empty() || (n.len() <= h.len() && h.windows(n.len()).any(|w| w == n))
}

/// `*` vale qualquer trecho e `?` um caractere, no nome inteiro.
fn glob(p: &[u8], s: &[u8]) -> bool {
    let (mut pi, mut si) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while si < s.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            mark = si;
            pi += 1;
        } else if let Some(sp) = star {
            pi = sp + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Name,
    Glob,
    Path,
}

struct Term {
    not: bool,
    kind: Kind,
    text: Vec<u8>,
}

#[derive(Default)]
struct Query {
    terms: Vec<Term>,
    exts: Vec<Vec<u8>>,
    /// 1: só arquivos. 2: só pastas.
    only: u8,
}

impl Query {
    fn parse(q: &str) -> Query {
        let mut out = Query::default();
        let mut toks = Vec::new();
        let mut cur = String::new();
        let mut quoted = false;
        for c in q.chars() {
            match c {
                '"' => quoted = !quoted,
                c if c.is_whitespace() && !quoted => {
                    if !cur.is_empty() {
                        toks.push(std::mem::take(&mut cur));
                    }
                }
                c => cur.push(c),
            }
        }
        if !cur.is_empty() {
            toks.push(cur);
        }
        for t in toks {
            let (not, t) = match t.strip_prefix('!') {
                Some(r) => (true, r.to_string()),
                None => (false, t),
            };
            let low = t.to_lowercase();
            let mut rest = t.as_str();
            for (pre, only) in [("pasta:", 2u8), ("folder:", 2), ("arquivo:", 1), ("file:", 1)] {
                if low.starts_with(pre) {
                    out.only = only;
                    rest = &t[pre.len()..];
                }
            }
            if low.starts_with("ext:") {
                for e in t[4..].split([';', ',']) {
                    let mut f = Vec::new();
                    fold(e.trim_start_matches('.'), &mut f);
                    if !f.is_empty() {
                        out.exts.push(f);
                    }
                }
                continue;
            }
            if rest.is_empty() {
                continue;
            }
            let mut text = Vec::new();
            fold(&rest.replace('/', "\\"), &mut text);
            let kind = if text.contains(&b'*') || text.contains(&b'?') {
                Kind::Glob
            } else if text.contains(&b'\\') {
                Kind::Path
            } else {
                Kind::Name
            };
            out.terms.push(Term { not, kind, text });
        }
        out
    }

    fn empty(&self) -> bool {
        self.terms.is_empty() && self.exts.is_empty()
    }

    fn rank_term(&self) -> Option<&[u8]> {
        self.terms.iter().find(|t| !t.not && t.kind == Kind::Name).map(|t| t.text.as_slice())
    }
}

/// Caminho da pasta já em minúsculas, guardado por pasta durante uma pesquisa.
fn dir_key<'m>(v: &Vol, d: u32, memo: &'m mut HashMap<u32, Vec<u8>>) -> &'m [u8] {
    if !memo.contains_key(&d) {
        let mut chain = Vec::new();
        let mut c = d;
        while c != v.root && !memo.contains_key(&c) && chain.len() < 400 {
            chain.push(c);
            c = v.entries.get(c as usize).map_or(v.root, |e| e.parent);
        }
        let mut base = match memo.get(&c) {
            Some(b) if c != v.root => b.clone(),
            _ => vec![(v.letter as u8).to_ascii_lowercase(), b':'],
        };
        if chain.is_empty() {
            memo.insert(d, base);
        } else {
            for &x in chain.iter().rev() {
                base.push(b'\\');
                fold(v.name(x), &mut base);
                memo.insert(x, base.clone());
            }
        }
    }
    &memo[&d]
}

struct Scratch {
    folded: Vec<u8>,
    path: Vec<u8>,
    memo: HashMap<u32, Vec<u8>>,
}

/// Nota de relevância (menor é melhor), ou None se não serve.
fn matches(v: &Vol, i: u32, q: &Query, rank: Option<&[u8]>, s: &mut Scratch) -> Option<u32> {
    let e = &v.entries[i as usize];
    if e.flags & USED == 0 || i == v.root {
        return None;
    }
    let dir = e.flags & DIR != 0;
    if (q.only == 1 && dir) || (q.only == 2 && !dir) {
        return None;
    }
    let nb = v.name(i).as_bytes();
    if !q.exts.is_empty() {
        if dir {
            return None;
        }
        let dot = nb.iter().rposition(|&c| c == b'.')?;
        let ext = &nb[dot + 1..];
        if !q.exts.iter().any(|x| x.len() == ext.len() && ext.iter().zip(x).all(|(a, b)| a.to_ascii_lowercase() == *b)) {
            return None;
        }
    }
    let ascii = nb.is_ascii();
    let mut folded = false;
    for t in &q.terms {
        let hit = match t.kind {
            Kind::Name if ascii => has_ascii_ci(nb, &t.text),
            Kind::Name | Kind::Glob => {
                if !folded {
                    s.folded.clear();
                    fold(v.name(i), &mut s.folded);
                    folded = true;
                }
                if t.kind == Kind::Glob {
                    glob(&t.text, &s.folded)
                } else {
                    has(&s.folded, &t.text)
                }
            }
            Kind::Path => {
                s.path.clear();
                s.path.extend_from_slice(dir_key(v, e.parent, &mut s.memo));
                s.path.push(b'\\');
                fold(v.name(i), &mut s.path);
                has(&s.path, &t.text)
            }
        };
        if hit == t.not {
            return None;
        }
    }
    let len = (nb.len() as u32).min(0xFFFF);
    let Some(r) = rank else { return Some(len) };
    if !folded {
        s.folded.clear();
        fold(v.name(i), &mut s.folded);
    }
    let score = if s.folded == r {
        0
    } else if s.folded.starts_with(r) {
        1
    } else {
        2
    };
    Some(score << 16 | len)
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Sort {
    #[default]
    Best,
    Name,
    Folder,
    Size,
    Date,
}

pub struct Results {
    pub query: String,
    pub hits: Vec<u64>,
    pub total: usize,
    pub ms: u32,
}

pub fn key(letter: char, i: u32) -> u64 {
    (letter as u64) << 32 | i as u64
}

fn search(q: &str, sort: Sort, desc: bool, gen: &AtomicU64, my: u64) -> Option<Results> {
    let t0 = Instant::now();
    let query = Query::parse(q);
    if query.empty() {
        return Some(Results { query: q.to_string(), hits: Vec::new(), total: 0, ms: 0 });
    }
    let vols = IX.vols.read().unwrap();
    const CHUNK: usize = 1 << 15;
    let chunks: Vec<(usize, usize)> = vols.iter().enumerate().flat_map(|(vi, v)| (0..v.entries.len()).step_by(CHUNK).map(move |s| (vi, s))).collect();
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8);
    let rank = query.rank_term();
    let parts: Vec<Vec<(u32, u64)>> = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..threads)
            .map(|_| {
                sc.spawn(|| {
                    let mut out = Vec::new();
                    let mut s = Scratch { folded: Vec::new(), path: Vec::new(), memo: HashMap::new() };
                    let mut memo_vol = usize::MAX;
                    loop {
                        let c = next.fetch_add(1, Ordering::Relaxed);
                        if c >= chunks.len() || gen.load(Ordering::Relaxed) != my {
                            break;
                        }
                        let (vi, start) = chunks[c];
                        let v = &vols[vi];
                        if memo_vol != vi {
                            s.memo.clear();
                            memo_vol = vi;
                        }
                        let end = (start + CHUNK).min(v.entries.len());
                        for i in start as u32..end as u32 {
                            if let Some(score) = matches(v, i, &query, rank, &mut s) {
                                out.push((score, key(v.letter, i)));
                            }
                        }
                    }
                    out
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap_or_default()).collect()
    });
    if gen.load(Ordering::Relaxed) != my {
        return None;
    }
    let mut all: Vec<(u32, u64)> = parts.into_iter().flatten().collect();
    let total = all.len();
    if all.len() > CAP {
        all.select_nth_unstable(CAP);
        all.truncate(CAP);
    }
    let find = |k: u64| -> Option<(&Vol, u32)> {
        let l = (k >> 32) as u8 as char;
        vols.iter().find(|v| v.letter == l).map(|v| (v, k as u32))
    };
    let mut hits: Vec<u64> = match sort {
        Sort::Best => {
            all.sort_unstable();
            all.into_iter().map(|(_, k)| k).collect()
        }
        Sort::Size | Sort::Date => {
            let mut v: Vec<(u64, u64)> = all
                .into_iter()
                .map(|(_, k)| {
                    let x = find(k).map_or(0, |(v, i)| {
                        let e = &v.entries[i as usize];
                        if sort == Sort::Size {
                            e.size
                        } else {
                            e.mtime as u64
                        }
                    });
                    (x, k)
                })
                .collect();
            v.sort_unstable();
            v.into_iter().map(|(_, k)| k).collect()
        }
        Sort::Name | Sort::Folder => {
            let mut memo = HashMap::new();
            let mut v: Vec<(Vec<u8>, u64)> = all
                .into_iter()
                .map(|(_, k)| {
                    let mut s = Vec::new();
                    if let Some((v, i)) = find(k) {
                        if sort == Sort::Folder {
                            if memo.len() > 200_000 {
                                memo.clear();
                            }
                            s.extend_from_slice(dir_key(v, v.entries[i as usize].parent, &mut memo));
                            s.push(0);
                        }
                        fold(v.name(i), &mut s);
                    }
                    (s, k)
                })
                .collect();
            v.sort_unstable();
            v.into_iter().map(|(_, k)| k).collect()
        }
    };
    if desc {
        hits.reverse();
    }
    Some(Results { query: q.to_string(), hits, total, ms: t0.elapsed().as_millis() as u32 })
}

/// Uma pesquisa por vez: a nova cancela a anterior no meio.
#[derive(Clone, Default)]
pub struct Searcher {
    gen: Arc<AtomicU64>,
    pub out: Arc<Mutex<Option<Results>>>,
}

impl Searcher {
    /// O resultado chega em `out`, e `msg` avisa a janela `notify` com `tag`.
    pub fn run(&self, q: &str, sort: Sort, desc: bool, notify: isize, msg: u32, tag: usize) {
        let my = self.gen.fetch_add(1, Ordering::Relaxed) + 1;
        let (gen, out, q) = (self.gen.clone(), self.out.clone(), q.to_string());
        let _ = std::thread::Builder::new().name("pesquisa".into()).spawn(move || {
            if let Some(r) = search(&q, sort, desc, &gen, my) {
                if gen.load(Ordering::Relaxed) == my {
                    *out.lock().unwrap() = Some(r);
                    unsafe {
                        let _ = PostMessageW(Some(HWND(notify as *mut c_void)), msg, WPARAM(tag), LPARAM(0));
                    }
                }
            }
        });
    }

    pub fn clear(&self) {
        self.gen.fetch_add(1, Ordering::Relaxed);
        *self.out.lock().unwrap() = None;
    }
}

/// O que a lista mostra de cada resultado.
pub struct Item {
    pub name: String,
    pub folder: String,
    pub size: u64,
    pub mtime: u32,
    pub dir: bool,
}

impl Item {
    pub fn path(&self) -> String {
        if self.folder.ends_with('\\') {
            format!("{}{}", self.folder, self.name)
        } else {
            format!("{}\\{}", self.folder, self.name)
        }
    }
}

/// Os resultados pedidos, na mesma ordem. O que sumiu do disco desde a pesquisa fica None.
pub fn items(keys: &[u64]) -> Vec<Option<Item>> {
    let vols = IX.vols.read().unwrap();
    keys.iter()
        .map(|&k| {
            let l = (k >> 32) as u8 as char;
            let i = k as u32;
            let v = vols.iter().find(|v| v.letter == l)?;
            if !v.live_entry(i) {
                return None;
            }
            let e = &v.entries[i as usize];
            Some(Item { name: v.name(i).to_string(), folder: v.path(e.parent), size: e.size, mtime: e.mtime, dir: e.flags & DIR != 0 })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_accents() {
        let mut o = Vec::new();
        fold("Relatório Ação ÉÇ", &mut o);
        assert_eq!(o, b"relatorio acao ec");
    }

    #[test]
    fn globs() {
        assert!(glob(b"*.pdf", b"nota.pdf"));
        assert!(glob(b"a?c*", b"abcde"));
        assert!(!glob(b"*.pdf", b"nota.pdfx"));
        assert!(glob(b"*", b""));
    }

    #[test]
    fn parses() {
        let q = Query::parse("\"meu arquivo\" ext:pdf;JPG !velho pasta: docs\\rel");
        assert_eq!(q.terms.len(), 3);
        assert_eq!(q.terms[0].text, b"meu arquivo");
        assert!(q.terms[1].not);
        assert!(q.terms[2].kind == Kind::Path);
        assert_eq!(q.exts, vec![b"pdf".to_vec(), b"jpg".to_vec()]);
        assert_eq!(q.only, 2);
    }

    #[test]
    fn ascii_ci() {
        assert!(has_ascii_ci(b"MeuArquivo.TXT", b"arquivo.t"));
        assert!(!has_ascii_ci(b"abc", b"abcd"));
    }
}

/// Teste de ponta a ponta num disco: monta o índice, cria, muda, renomeia e apaga um arquivo em
/// `dir` e confere se a pesquisa acompanha. Com administrador, testa a tabela e o diário.
#[cfg(debug_assertions)]
pub fn self_test(letter: char, dir: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let had = file_for(letter).exists();
    let _ = writeln!(out, "administrador: {} · índice salvo antes: {had}", crate::startup::is_elevated());
    let t = Instant::now();
    set(true, vec![letter]);
    while !status().iter().any(|d| d.state == State::Ready) {
        if t.elapsed() > Duration::from_secs(900) {
            let _ = writeln!(out, "não ficou pronto em 15 minutos");
            return out;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let d = status().into_iter().next().unwrap();
    let _ = writeln!(out, "pronto em {} ms · {} itens · ao vivo: {} · memória {} MB", t.elapsed().as_millis(), d.files, d.live, memory() >> 20);
    let find = |q: &str| search(q, Sort::Best, false, &AtomicU64::new(0), 0).unwrap();
    for q in ["readme", "a", "ext:dll", "*.json", r"windows\system32\drivers\etc"] {
        let r = find(q);
        let _ = writeln!(out, "pesquisa {q:?}: {} em {} ms", r.total, r.ms);
    }
    let wait = || std::thread::sleep(Duration::from_secs(6));
    let name = format!("prisma_teste_{}.txt", now_secs());
    let path = PathBuf::from(dir).join(&name);
    let _ = std::fs::write(&path, b"abc");
    wait();
    let r = find(&name);
    let it = items(&r.hits).into_iter().flatten().next();
    let _ = writeln!(out, "criado: {} achado(s) · pasta {:?} · tamanho {:?}", r.total, it.as_ref().map(|i| i.folder.clone()), it.as_ref().map(|i| i.size));
    let _ = std::fs::write(&path, vec![b'x'; 50_000]);
    wait();
    let r = find(&name);
    let it = items(&r.hits).into_iter().flatten().next();
    let _ = writeln!(out, "aumentado para 50000: tamanho {:?}", it.map(|i| i.size));
    let renamed = PathBuf::from(dir).join(format!("renomeado_{name}"));
    let _ = std::fs::rename(&path, &renamed);
    wait();
    let _ = writeln!(out, "renomeado: nome velho {} · nome novo {}", find(&format!("\"{name}\" !renomeado")).total, find(&format!("renomeado_{name}")).total);
    let _ = std::fs::remove_file(&renamed);
    wait();
    let _ = writeln!(out, "apagado: {} achado(s)", find(&format!("renomeado_{name}")).total);
    let t = Instant::now();
    flush();
    let size = std::fs::metadata(file_for(letter)).map_or(0, |m| m.len());
    let _ = writeln!(out, "gravado em {} ms · arquivo {} MB", t.elapsed().as_millis(), size >> 20);
    out
}
