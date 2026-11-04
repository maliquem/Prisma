//! Analisador de disco. Em NTFS, lê direto a tabela de arquivos do volume em vez de
//! visitar pasta por pasta, que é o que torna a análise rápida. Precisa de administrador.
//! Fora disso, percorre as pastas do jeito normal.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::IO::DeviceIoControl;

const NTFS_VOLUME_DATA: u32 = 0x90064;
const VOLUME_EXTENTS: u32 = 0x560000;
const CHUNK: usize = 8 << 20;
const ROOT: u32 = 5;

#[derive(Clone, Copy, Default)]
pub struct Node {
    pub parent: u32,
    name_off: u32,
    name_len: u16,
    flags: u8,
    pub size: u64,
    pub alloc: u64,
    pub files: u32,
}

const USED: u8 = 1;
const DIR: u8 = 2;
const DOS_NAME: u8 = 4;

impl Node {
    pub fn is_dir(&self) -> bool {
        self.flags & DIR != 0
    }
    fn used(&self) -> bool {
        self.flags & USED != 0
    }
}

pub struct TypeStat {
    pub ext: String,
    pub size: u64,
    pub files: u32,
}

pub struct Scan {
    pub drive: char,
    pub nodes: Vec<Node>,
    pub root: u32,
    names: Vec<u8>,
    first: Vec<u32>,
    list: Vec<u32>,
    pub types: Vec<TypeStat>,
    pub files: u64,
    pub elapsed: Duration,
    pub used_mft: bool,
    /// Maiores arquivos do disco, do maior para o menor.
    pub largest: Vec<u32>,
}

impl Scan {
    pub fn name(&self, i: u32) -> &str {
        let n = &self.nodes[i as usize];
        let s = n.name_off as usize;
        std::str::from_utf8(&self.names[s..s + n.name_len as usize]).unwrap_or("?")
    }

    pub fn children(&self, i: u32) -> &[u32] {
        let (a, b) = (self.first[i as usize] as usize, self.first[i as usize + 1] as usize);
        &self.list[a..b]
    }

    pub fn path(&self, mut i: u32) -> String {
        let mut parts = Vec::new();
        for _ in 0..512 {
            if i == self.root {
                break;
            }
            parts.push(self.name(i).to_string());
            i = self.nodes[i as usize].parent;
        }
        parts.reverse();
        format!("{}:\\{}", self.drive, parts.join("\\"))
    }

    /// Soma tudo pelas pastas e monta os índices depois de ler os arquivos.
    fn finish(&mut self) {
        let n = self.nodes.len();
        let mut types: HashMap<String, (u64, u32)> = HashMap::new();
        for i in 0..n {
            let nd = self.nodes[i];
            if !nd.used() {
                continue;
            }
            if i as u32 != self.root && !self.nodes[nd.parent as usize].used() {
                self.nodes[i].parent = self.root;
            }
            if nd.is_dir() {
                continue;
            }
            self.files += 1;
            let name = self.name(i as u32);
            let ext = name.rsplit_once('.').map(|e| e.1.to_ascii_lowercase()).filter(|e| e.len() <= 12).unwrap_or_default();
            let e = types.entry(ext).or_default();
            e.0 += nd.size;
            e.1 += 1;
            let mut p = self.nodes[i].parent;
            for _ in 0..512 {
                let d = &mut self.nodes[p as usize];
                d.size += nd.size;
                d.alloc += nd.alloc;
                d.files += 1;
                if p == self.root {
                    break;
                }
                p = d.parent;
            }
        }
        let mut count = vec![0u32; n + 1];
        for (i, nd) in self.nodes.iter().enumerate() {
            if nd.used() && i as u32 != self.root {
                count[nd.parent as usize] += 1;
            }
        }
        let mut first = vec![0u32; n + 1];
        for i in 0..n {
            first[i + 1] = first[i] + count[i];
        }
        let mut list = vec![0u32; first[n] as usize];
        let mut pos = first.clone();
        for (i, nd) in self.nodes.iter().enumerate() {
            if nd.used() && i as u32 != self.root {
                list[pos[nd.parent as usize] as usize] = i as u32;
                pos[nd.parent as usize] += 1;
            }
        }
        let nodes = &self.nodes;
        for i in 0..n {
            let (a, b) = (first[i] as usize, first[i + 1] as usize);
            if b - a > 1 {
                list[a..b].sort_unstable_by_key(|c| std::cmp::Reverse(nodes[*c as usize].size));
            }
        }
        self.first = first;
        self.list = list;
        let mut t: Vec<TypeStat> = types.into_iter().map(|(ext, v)| TypeStat { ext, size: v.0, files: v.1 }).collect();
        t.sort_unstable_by_key(|s| std::cmp::Reverse(s.size));
        t.truncate(100);
        self.types = t;

        let mut files: Vec<u32> = (0..n as u32).filter(|i| {
            let v = &self.nodes[*i as usize];
            v.used() && !v.is_dir()
        }).collect();
        let nodes = &self.nodes;
        files.sort_unstable_by_key(|i| std::cmp::Reverse(nodes[*i as usize].size));
        files.truncate(500);
        self.largest = files;
    }

    pub fn exists(&self, i: u32) -> bool {
        self.nodes[i as usize].used()
    }

    /// Tira um item da análise depois de apagado, sem refazer a leitura.
    pub fn forget(&mut self, i: u32) {
        let root = self.root;
        let n = self.nodes[i as usize];
        let count = if n.is_dir() { n.files } else { 1 };
        let mut p = n.parent;
        for _ in 0..512 {
            let d = &mut self.nodes[p as usize];
            d.size = d.size.saturating_sub(n.size);
            d.alloc = d.alloc.saturating_sub(n.alloc);
            d.files = d.files.saturating_sub(count);
            if p == root {
                break;
            }
            p = d.parent;
        }
        self.nodes[i as usize].flags &= !USED;
        let nodes = &self.nodes;
        self.largest.retain(|f| {
            let mut c = *f;
            for _ in 0..512 {
                if !nodes[c as usize].used() {
                    return false;
                }
                if c == root {
                    return true;
                }
                c = nodes[c as usize].parent;
            }
            true
        });
    }
}

/// Itens que o Windows precisa: o Prisma não oferece apagar.
pub fn protected(path: &str) -> bool {
    let low = path.to_ascii_lowercase();
    let tail = low.get(3..).unwrap_or("");
    let first = tail.split('\\').next().unwrap_or("");
    tail.is_empty()
        || first.starts_with('$')
        || matches!(first, "windows" | "program files" | "program files (x86)" | "programdata" | "users" | "system volume information" | "recovery" | "boot" | "efi")
            && !tail.contains('\\')
        || tail.starts_with("windows\\")
        || matches!(first, "pagefile.sys" | "hiberfil.sys" | "swapfile.sys" | "bootmgr")
}

/// Leitura crua de um volume. Alguns volumes recusam leitura direta; nesses,
/// o mesmo conteúdo é lido pelo disco físico, somando o início da partição.
struct Raw {
    h: HANDLE,
    base: u64,
}

impl Raw {
    fn read(&self, off: u64, buf: &mut [u8]) -> bool {
        unsafe {
            if SetFilePointerEx(self.h, (self.base + off) as i64, None, FILE_BEGIN).is_err() {
                return false;
            }
            let mut got = 0;
            ReadFile(self.h, Some(buf), Some(&mut got), None).is_ok() && got as usize == buf.len()
        }
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.h);
        }
    }
}

fn open_raw(path: &str) -> Option<HANDLE> {
    unsafe {
        CreateFileW(
            &HSTRING::from(path),
            GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_NO_BUFFERING | FILE_FLAG_SEQUENTIAL_SCAN,
            None,
        )
        .ok()
    }
}

fn open_physical(volume: HANDLE) -> Option<Raw> {
    unsafe {
        let mut b = [0u8; 256];
        let mut got = 0;
        DeviceIoControl(volume, VOLUME_EXTENTS, None, 0, Some(b.as_mut_ptr() as *mut c_void), 256, Some(&mut got), None).ok()?;
        if u32_at(&b, 0) != 1 {
            return None;
        }
        let h = open_raw(&format!("\\\\.\\PhysicalDrive{}", u32_at(&b, 8)))?;
        Some(Raw { h, base: u64_at(&b, 16) })
    }
}

/// Buffer alinhado, exigência da leitura direta de volume.
struct Aligned {
    raw: Vec<u8>,
    off: usize,
}

impl Aligned {
    fn new(n: usize) -> Self {
        let raw = vec![0u8; n + 4096];
        let off = (4096 - raw.as_ptr() as usize % 4096) % 4096;
        Self { raw, off }
    }
    fn get(&mut self, n: usize) -> &mut [u8] {
        &mut self.raw[self.off..self.off + n]
    }
}

fn u16_at(b: &[u8], o: usize) -> usize {
    u16::from_le_bytes([b[o], b[o + 1]]) as usize
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn fixup(r: &mut [u8]) -> bool {
    if r.len() < 48 || &r[..4] != b"FILE" {
        return false;
    }
    let off = u16_at(r, 4);
    let count = u16_at(r, 6);
    if count == 0 || off + count * 2 > r.len() {
        return false;
    }
    for i in 1..count {
        let end = i * 512;
        if end > r.len() {
            break;
        }
        r[end - 2] = r[off + i * 2];
        r[end - 1] = r[off + i * 2 + 1];
    }
    true
}

/// Atributos do registro: (tipo, início, comprimento).
fn attrs(r: &[u8]) -> Vec<(u32, usize, usize)> {
    let mut out = Vec::with_capacity(8);
    let mut o = u16_at(r, 20);
    while o + 16 <= r.len() {
        let t = u32_at(r, o);
        if t == 0xFFFF_FFFF {
            break;
        }
        let len = u32_at(r, o + 4) as usize;
        if len < 16 || o + len > r.len() {
            break;
        }
        out.push((t, o, len));
        o += len;
    }
    out
}

fn decode_runs(a: &[u8], out: &mut Vec<(i64, u64)>) {
    let mut o = u16_at(a, 32);
    let mut lcn = 0i64;
    while o < a.len() && a[o] != 0 {
        let (l, f) = ((a[o] & 15) as usize, (a[o] >> 4) as usize);
        o += 1;
        if o + l + f > a.len() || l > 8 || f > 8 {
            return;
        }
        let mut len = 0u64;
        for i in 0..l {
            len |= (a[o + i] as u64) << (i * 8);
        }
        o += l;
        if f == 0 {
            out.push((-1, len));
        } else {
            let mut d = 0i64;
            for i in 0..f {
                d |= (a[o + i] as i64) << (i * 8);
            }
            if a[o + f - 1] & 0x80 != 0 && f < 8 {
                d |= -1i64 << (f * 8);
            }
            lcn += d;
            out.push((lcn, len));
        }
        o += f;
    }
}

struct Parser {
    nodes: Vec<Node>,
    names: Vec<u8>,
}

impl Parser {
    fn record(&mut self, idx: u32, r: &[u8]) {
        let flags = u16_at(r, 22);
        if flags & 1 == 0 {
            return;
        }
        let base = (u64_at(r, 32) & 0xFFFF_FFFF_FFFF) as usize;
        let id = if base != 0 { base } else { idx as usize };
        if id >= self.nodes.len() {
            return;
        }
        let node = &mut self.nodes[id];
        if base == 0 {
            node.flags |= USED;
            if flags & 2 != 0 {
                node.flags |= DIR;
            }
        }
        for (t, o, len) in attrs(r) {
            let a = &r[o..o + len];
            match t {
                0x30 if a[8] == 0 => {
                    let v = u16_at(a, 20);
                    let vl = u32_at(a, 16) as usize;
                    if vl < 66 || v + vl > a.len() {
                        continue;
                    }
                    let d = &a[v..v + vl];
                    let nl = d[64] as usize;
                    let ns = d[65];
                    if 66 + nl * 2 > d.len() {
                        continue;
                    }
                    let node = &mut self.nodes[id];
                    let replace = node.name_len == 0 || (node.flags & DOS_NAME != 0 && ns != 2);
                    if !replace {
                        continue;
                    }
                    node.parent = (u64_at(d, 0) & 0xFFFF_FFFF) as u32;
                    if ns == 2 {
                        node.flags |= DOS_NAME;
                    } else {
                        node.flags &= !DOS_NAME;
                    }
                    let w: Vec<u16> = d[66..66 + nl * 2].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                    let s = String::from_utf16_lossy(&w);
                    node.name_off = self.names.len() as u32;
                    node.name_len = s.len().min(u16::MAX as usize) as u16;
                    self.names.extend_from_slice(s.as_bytes());
                }
                0x80 if a[9] == 0 => {
                    let node = &mut self.nodes[id];
                    if a[8] == 0 {
                        let v = u32_at(a, 16) as u64;
                        node.size = v;
                        node.alloc = v;
                    } else if u64_at(a, 16) == 0 && a.len() >= 64 {
                        node.size = u64_at(a, 48);
                        let compressed = u16_at(a, 12) & 0x8001 != 0 && a.len() >= 72;
                        node.alloc = if compressed { u64_at(a, 64) } else { u64_at(a, 40) };
                    }
                }
                _ => {}
            }
        }
    }
}

#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub running: AtomicBool,
    pub cancel: AtomicBool,
}

fn scan_mft(drive: char, p: &Progress) -> Option<Scan> {
    let t0 = Instant::now();
    unsafe {
        let vol = Raw { h: open_raw(&format!("\\\\.\\{drive}:"))?, base: 0 };
        let mut info = [0u8; 128];
        let mut got = 0;
        DeviceIoControl(vol.h, NTFS_VOLUME_DATA, None, 0, Some(info.as_mut_ptr() as *mut c_void), 128, Some(&mut got), None).ok()?;
        let cluster = u32_at(&info, 44) as u64;
        let rec = u32_at(&info, 48) as usize;
        let mft_len = u64_at(&info, 56);
        let mft_lcn = u64_at(&info, 64);
        if rec == 0 || cluster == 0 {
            return None;
        }
        let mut buf = Aligned::new(CHUNK.max(rec * 8));
        let first_len = rec.max(4096);
        let start = mft_lcn * cluster;
        let mut direct = vol.read(start, buf.get(first_len));
        let physical = if direct { None } else { open_physical(vol.h) };
        if let Some(r) = &physical {
            direct = r.read(start, buf.get(first_len));
        }
        if !direct {
            return None;
        }
        let raw = physical.as_ref().unwrap_or(&vol);
        let mut first = buf.get(rec).to_vec();
        if !fixup(&mut first) {
            return None;
        }
        let mut runs: Vec<(i64, u64)> = Vec::new();
        let mut list_attr = None;
        for (t, o, len) in attrs(&first) {
            let a = &first[o..o + len];
            if t == 0x80 && a[8] != 0 && a[9] == 0 {
                decode_runs(a, &mut runs);
            } else if t == 0x20 {
                list_attr = Some((o, len));
            }
        }
        // Tabela muito fragmentada continua em registros de extensão.
        if let Some((o, len)) = list_attr {
            let a = &first[o..o + len];
            if a[8] == 0 {
                let v = u16_at(a, 20);
                let vl = u32_at(a, 16) as usize;
                let l = &a[v..(v + vl).min(a.len())];
                let mut i = 0;
                while i + 26 <= l.len() {
                    let tp = u32_at(l, i);
                    let rl = u16_at(l, i + 4);
                    let vcn = u64_at(l, i + 8);
                    let number = u64_at(l, i + 16) & 0xFFFF_FFFF_FFFF;
                    if rl == 0 {
                        break;
                    }
                    if tp == 0x80 && vcn != 0 {
                        let mut pos = number * rec as u64;
                        for (lcn, clusters) in &runs {
                            let bytes = clusters * cluster;
                            if pos < bytes {
                                let at = *lcn as u64 * cluster + pos;
                                let (low, skip) = (at & !4095, (at & 4095) as usize);
                                if *lcn >= 0 && raw.read(low, buf.get(4096 + first_len)) {
                                    let mut r = buf.get(4096 + first_len)[skip..skip + rec].to_vec();
                                    if fixup(&mut r) {
                                        for (t2, o2, l2) in attrs(&r) {
                                            if t2 == 0x80 && r[o2 + 8] != 0 {
                                                let mut more = Vec::new();
                                                decode_runs(&r[o2..o2 + l2], &mut more);
                                                runs.extend(more);
                                            }
                                        }
                                    }
                                }
                                break;
                            }
                            pos -= bytes;
                        }
                    }
                    i += rl;
                }
            }
        }

        let total = (mft_len / rec as u64) as usize;
        p.total.store(total as u64, Ordering::Relaxed);
        let mut parser = Parser { nodes: vec![Node::default(); total + 1], names: Vec::with_capacity(total * 24) };
        let mut idx = 0u32;
        let step = (CHUNK / rec) * rec;
        for (lcn, clusters) in &runs {
            let mut off = 0u64;
            let bytes = clusters * cluster;
            while off < bytes && (idx as usize) < total {
                if p.cancel.load(Ordering::Relaxed) {
                    return None;
                }
                let n = ((bytes - off) as usize).min(step);
                if *lcn < 0 {
                    idx += (n / rec) as u32;
                    off += n as u64;
                    continue;
                }
                // Setor defeituoso não derruba a análise: pula o trecho e segue.
                if !raw.read(*lcn as u64 * cluster + off, buf.get(n)) {
                    idx += (n / rec) as u32;
                    off += n as u64;
                    continue;
                }
                for r in buf.get(n).chunks_exact_mut(rec) {
                    if (idx as usize) < total && fixup(r) {
                        parser.record(idx, r);
                    }
                    idx += 1;
                }
                off += n as u64;
                p.done.store(idx as u64, Ordering::Relaxed);
            }
        }
        let mut s = Scan {
            drive,
            nodes: parser.nodes,
            root: ROOT,
            names: parser.names,
            first: Vec::new(),
            list: Vec::new(),
            types: Vec::new(),
            files: 0,
            elapsed: Duration::ZERO,
            used_mft: true,
            largest: Vec::new(),
        };
        s.nodes[ROOT as usize].flags |= USED | DIR;
        s.nodes[ROOT as usize].parent = ROOT;
        s.finish();
        s.elapsed = t0.elapsed();
        Some(s)
    }
}

/// Caminho lento: para discos que não são NTFS ou sem administrador.
fn scan_walk(drive: char, p: &Progress) -> Scan {
    let t0 = Instant::now();
    let mut nodes = vec![Node { flags: USED | DIR, ..Default::default() }];
    let mut names: Vec<u8> = Vec::new();
    let mut stack = vec![(0u32, std::path::PathBuf::from(format!("{drive}:\\")))];
    while let Some((id, dir)) = stack.pop() {
        if p.cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            let name = e.file_name().to_string_lossy().to_string();
            let i = nodes.len() as u32;
            let mut n = Node {
                parent: id,
                name_off: names.len() as u32,
                name_len: name.len().min(u16::MAX as usize) as u16,
                flags: USED,
                ..Default::default()
            };
            names.extend_from_slice(name.as_bytes());
            if meta.is_dir() {
                n.flags |= DIR;
                if !meta.file_type().is_symlink() {
                    stack.push((i, e.path()));
                }
            } else {
                n.size = meta.len();
                n.alloc = meta.len();
            }
            nodes.push(n);
            p.done.fetch_add(1, Ordering::Relaxed);
        }
    }
    let mut s = Scan {
        drive,
        nodes,
        root: 0,
        names,
        first: Vec::new(),
        list: Vec::new(),
        types: Vec::new(),
        files: 0,
        elapsed: Duration::ZERO,
        used_mft: false,
        largest: Vec::new(),
    };
    s.finish();
    s.elapsed = t0.elapsed();
    s
}

pub type Slot = Arc<Mutex<Option<Scan>>>;

pub fn start(drive: char, p: Arc<Progress>, slot: Slot) {
    p.done.store(0, Ordering::Relaxed);
    p.total.store(0, Ordering::Relaxed);
    p.cancel.store(false, Ordering::Relaxed);
    p.running.store(true, Ordering::Relaxed);
    let _ = std::thread::Builder::new().name("analise".into()).spawn(move || {
        let s = scan_mft(drive, &p).unwrap_or_else(|| scan_walk(drive, &p));
        *slot.lock().unwrap() = Some(s);
        p.running.store(false, Ordering::Relaxed);
    });
}

/// Unidades fixas e removíveis com letra.
pub fn drives() -> Vec<char> {
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| (b'A' + i) as char)
        .filter(|l| {
            let root = HSTRING::from(format!("{l}:\\"));
            let k = unsafe { GetDriveTypeW(&root) };
            k == 2 || k == 3
        })
        .collect()
}
