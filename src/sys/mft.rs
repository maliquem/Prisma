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

use super::{u16_at, u32_at, u64_at};

const NTFS_VOLUME_DATA: u32 = 0x90064;
const VOLUME_EXTENTS: u32 = 0x560000;
const CHUNK: usize = 8 << 20;
const ROOT: u32 = 5;

#[derive(Clone, Copy, Default)]
pub struct Node {
    pub parent: u32,
    pub(crate) name_off: u32,
    pub(crate) name_len: u16,
    pub(crate) flags: u8,
    pub size: u64,
    pub alloc: u64,
    pub files: u32,
}

pub(crate) const USED: u8 = 1;
pub(crate) const DIR: u8 = 2;
const DOS_NAME: u8 = 4;

impl Node {
    pub fn is_dir(&self) -> bool {
        self.flags & DIR != 0
    }
    pub(crate) fn used(&self) -> bool {
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
    /// Muda quando uma pasta é lida de novo ou a análise é outra: listas guardadas fora ficam velhas.
    pub version: u32,
}

static VERSION: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

fn next_version() -> u32 {
    VERSION.fetch_add(1, Ordering::Relaxed)
}

/// Extensão como a lista de tipos agrupa (sem o ponto); vazia quando não tem ou é longa demais.
fn ext(name: &str) -> &str {
    name.rsplit_once('.').map(|e| e.1).filter(|e| e.len() <= 12).unwrap_or("")
}

/// Pastas e arquivos lidos do disco, com a pasta lida no índice 0.
pub(crate) struct Walked {
    nodes: Vec<Node>,
    names: Vec<u8>,
    /// Pastas que não abriram (sem permissão ou apagadas no meio da leitura).
    denied: Vec<u32>,
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

    /// Pasta pelo caminho completo, sem diferença de maiúsculas, se ela está neste disco.
    pub fn find(&self, path: &str) -> Option<u32> {
        let (letter, rest) = path.split_once(':')?;
        if !letter.eq_ignore_ascii_case(&self.drive.to_string()) {
            return None;
        }
        let mut cur = self.root;
        for part in rest.split('\\').filter(|p| !p.is_empty()) {
            let part = part.to_lowercase();
            cur = *self.children(cur).iter().find(|&&k| self.exists(k) && self.nodes[k as usize].is_dir() && self.name(k).to_lowercase() == part)?;
        }
        Some(cur)
    }

    /// Soma tudo pelas pastas e monta os índices depois de ler os arquivos.
    fn finish(&mut self) {
        let n = self.nodes.len();
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
        self.index();
        self.count_types();
        self.top_files();
    }

    /// Conta os arquivos e soma os tipos (os 100 maiores) de tudo que está na análise.
    fn count_types(&mut self) {
        let mut types: HashMap<String, (u64, u32)> = HashMap::new();
        self.files = 0;
        for i in 0..self.nodes.len() as u32 {
            let nd = self.nodes[i as usize];
            if !nd.used() || nd.is_dir() {
                continue;
            }
            self.files += 1;
            let e = types.entry(ext(self.name(i)).to_ascii_lowercase()).or_default();
            e.0 += nd.size;
            e.1 += 1;
        }
        let mut t: Vec<TypeStat> = types.into_iter().map(|(ext, v)| TypeStat { ext, size: v.0, files: v.1 }).collect();
        t.sort_unstable_by_key(|s| std::cmp::Reverse(s.size));
        t.truncate(100);
        self.types = t;
    }

    /// Filhos de cada pasta, do maior para o menor.
    fn index(&mut self) {
        let n = self.nodes.len();
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
    }

    /// Os 500 maiores arquivos.
    fn top_files(&mut self) {
        let nodes = &self.nodes;
        let mut files: Vec<u32> = (0..nodes.len() as u32).filter(|i| nodes[*i as usize].used() && !nodes[*i as usize].is_dir()).collect();
        let key = |i: &u32| std::cmp::Reverse(nodes[*i as usize].size);
        if files.len() > 500 {
            files.select_nth_unstable_by_key(499, key);
            files.truncate(500);
        }
        files.sort_unstable_by_key(key);
        self.largest = files;
    }

    pub fn exists(&self, i: u32) -> bool {
        self.nodes[i as usize].used()
    }

    /// Tira um item da análise depois de apagado, sem refazer a leitura.
    pub fn forget(&mut self, i: u32) {
        let n = self.nodes[i as usize];
        if !n.used() {
            return;
        }
        let count = if n.is_dir() { n.files } else { 1 };
        let mut p = n.parent;
        for _ in 0..512 {
            let d = &mut self.nodes[p as usize];
            d.size = d.size.saturating_sub(n.size);
            d.alloc = d.alloc.saturating_sub(n.alloc);
            d.files = d.files.saturating_sub(count);
            if p == self.root {
                break;
            }
            p = d.parent;
        }
        self.files = self.files.saturating_sub(count as u64);
        self.unmark(i, &[]);
        self.sort_types();
        let nodes = &self.nodes;
        self.largest.retain(|f| nodes[*f as usize].used());
        self.version = next_version();
    }

    /// Desmarca o item e tudo dentro dele (menos `keep`), tirando os arquivos da lista de tipos.
    fn unmark(&mut self, i: u32, keep: &[u32]) {
        let mut stack = vec![i];
        while let Some(c) = stack.pop() {
            if keep.contains(&c) {
                continue;
            }
            let n = self.nodes[c as usize];
            self.nodes[c as usize].flags &= !USED;
            if n.is_dir() {
                stack.extend_from_slice(self.children(c));
            } else if n.used() {
                let e = ext(self.name(c)).to_ascii_lowercase();
                if let Some(t) = self.types.iter_mut().find(|t| t.ext == e) {
                    t.size = t.size.saturating_sub(n.size);
                    t.files = t.files.saturating_sub(1);
                }
            }
        }
    }

    fn sort_types(&mut self) {
        self.types.retain(|t| t.files > 0);
        self.types.sort_unstable_by_key(|s| std::cmp::Reverse(s.size));
    }

    /// Troca o conteúdo da pasta `dir` pelo que foi lido de novo do disco. A diferença de
    /// tamanho sobe pelas pastas acima até a raiz; o resto da análise fica como estava.
    /// shortcut: a leitura por pastas conta cada hard link e usa o tamanho lógico; numa análise pela
    /// tabela do NTFS a pasta pode mudar um pouco de tamanho. Analisar o disco de novo acerta.
    pub(crate) fn replace(&mut self, dir: u32, w: Walked) {
        if w.denied.contains(&0) {
            return; // a própria pasta não abriu: fica como estava
        }
        let base = self.nodes.len() as u32;
        // Subpastas que não abriram agora (sem permissão) ficam com o que a análise já tinha.
        let here = self.path(dir);
        let twins: Vec<(u32, u32)> = w
            .denied
            .iter()
            .filter_map(|&l| {
                let mut rel = Vec::new();
                let mut i = l;
                while i != 0 {
                    let n = w.nodes[i as usize];
                    rel.push(std::str::from_utf8(&w.names[n.name_off as usize..][..n.name_len as usize]).unwrap_or("?"));
                    i = n.parent;
                }
                rel.reverse();
                self.find(&format!("{here}\\{}", rel.join("\\"))).map(|o| (base + l - 1, o))
            })
            .collect();
        let keep: Vec<u32> = twins.iter().flat_map(|t| self.children(t.1).iter().copied()).collect();
        for k in self.children(dir).to_vec() {
            self.unmark(k, &keep);
        }
        let old = self.nodes[dir as usize];
        let off = self.names.len() as u32;
        self.names.extend_from_slice(&w.names);
        for mut n in w.nodes.into_iter().skip(1) {
            n.parent = if n.parent == 0 { dir } else { base + n.parent - 1 };
            n.name_off += off;
            self.nodes.push(n);
        }
        for &(n, o) in &twins {
            for c in self.children(o).to_vec() {
                self.nodes[c as usize].parent = n;
            }
            let od = self.nodes[o as usize];
            let d = &mut self.nodes[n as usize];
            (d.size, d.alloc, d.files) = (od.size, od.alloc, od.files);
        }
        let d = &mut self.nodes[dir as usize];
        (d.size, d.alloc, d.files) = (0, 0, 0);
        // Na leitura, cada filho vem depois da pasta dele: de trás para frente, as somas fecham.
        for i in (base as usize..self.nodes.len()).rev() {
            let n = self.nodes[i];
            let p = &mut self.nodes[n.parent as usize];
            p.size += n.size;
            p.alloc += n.alloc;
            p.files += if n.is_dir() { n.files } else { 1 };
        }
        let new = self.nodes[dir as usize];
        let mut p = dir;
        for _ in 0..512 {
            if p == self.root {
                break;
            }
            p = self.nodes[p as usize].parent;
            let a = &mut self.nodes[p as usize];
            a.size = a.size.saturating_sub(old.size) + new.size;
            a.alloc = a.alloc.saturating_sub(old.alloc) + new.alloc;
            a.files = a.files.saturating_sub(old.files) + new.files;
        }
        self.index();
        self.count_types();
        self.top_files();
        self.version = next_version();
    }

    /// Arquivos com a extensão `e` (como em `types`), do maior para o menor.
    pub fn files_of(&self, e: &str) -> Vec<u32> {
        let nodes = &self.nodes;
        let mut v: Vec<u32> = (0..nodes.len() as u32)
            .filter(|&i| nodes[i as usize].used() && !nodes[i as usize].is_dir() && ext(self.name(i)).eq_ignore_ascii_case(e))
            .collect();
        v.sort_unstable_by_key(|i| std::cmp::Reverse(nodes[*i as usize].size));
        v
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

/// Arquivo que o Prisma não apaga para sempre: os protegidos e tudo dentro das pastas de programas.
pub fn system_file(path: &str) -> bool {
    let low = path.to_ascii_lowercase();
    let first = low.get(3..).unwrap_or("").split('\\').next().unwrap_or("");
    protected(path) || matches!(first, "program files" | "program files (x86)" | "programdata")
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

fn fixup(r: &mut [u8]) -> bool {
    if r.len() < 48 || &r[..4] != b"FILE" {
        return false;
    }
    let off = u16_at(r, 4) as usize;
    let count = u16_at(r, 6) as usize;
    if count == 0 || off + count * 2 > r.len() {
        return false;
    }
    for i in 1..count {
        let end = i * 512;
        if end > r.len() {
            break;
        }
        // Setor que não termina com o número da gravação: registro lido no meio de uma escrita.
        if r[end - 2..end] != r[off..off + 2] {
            return false;
        }
        r[end - 2] = r[off + i * 2];
        r[end - 1] = r[off + i * 2 + 1];
    }
    true
}

/// Atributos do registro: (tipo, início, comprimento).
fn attrs(r: &[u8]) -> Vec<(u32, usize, usize)> {
    let mut out = Vec::with_capacity(8);
    let mut o = u16_at(r, 20) as usize;
    while o + 16 <= r.len() {
        let t = u32_at(r, o);
        if t == 0xFFFF_FFFF {
            break;
        }
        let len = u32_at(r, o + 4) as usize;
        // Cabeçalho inteiro ou nada: residente tem 24 bytes, não residente 64.
        let min = if r[o + 8] == 0 { 24 } else { 64 };
        if len < min || o + len > r.len() {
            break;
        }
        out.push((t, o, len));
        o += len;
    }
    out
}

/// `n` bytes a partir de `at`: a leitura direta só aceita blocos alinhados de 4 KB.
fn read_span(raw: &Raw, buf: &mut Aligned, at: u64, n: usize) -> Option<Vec<u8>> {
    let cap = buf.raw.len() - 4096;
    let (low, skip) = (at & !4095, (at & 4095) as usize);
    if n > cap {
        return None;
    }
    let len = (skip + n + 4095) & !4095;
    if len > cap || !raw.read(low, buf.get(len)) {
        return None;
    }
    Some(buf.get(len)[skip..skip + n].to_vec())
}

/// Onde fica no volume o byte `pos` da tabela, pelos pedaços dela.
fn locate(runs: &[(i64, u64)], mut pos: u64, cluster: u64) -> Option<u64> {
    for &(lcn, clusters) in runs {
        let bytes = clusters.saturating_mul(cluster);
        if pos < bytes {
            return (lcn >= 0).then(|| (lcn as u64).saturating_mul(cluster) + pos);
        }
        pos -= bytes;
    }
    None
}

/// Pedaços dos dados da tabela depois do VCN 0, pela lista de atributos: (registro, VCN inicial).
/// Um registro pode ter mais de um pedaço, e cada um aparece na lista.
fn list_extents(l: &[u8]) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 26 <= l.len() {
        let rl = u16_at(l, i + 4) as usize;
        if rl == 0 {
            break;
        }
        let vcn = u64_at(l, i + 8);
        if u32_at(l, i) == 0x80 && vcn != 0 {
            out.push((u64_at(l, i + 16) & 0xFFFF_FFFF_FFFF, vcn));
        }
        i += rl;
    }
    out
}

fn decode_runs(a: &[u8], out: &mut Vec<(i64, u64)>) {
    let mut o = u16_at(a, 32) as usize;
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
    /// Data de modificação em segundos desde 1970, só quando quem pediu precisa.
    times: Vec<u32>,
}

/// FILETIME em segundos desde 1970.
pub(crate) fn unix_secs(ft: u64) -> u32 {
    (ft / 10_000_000).saturating_sub(11_644_473_600).min(u32::MAX as u64) as u32
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
                0x10 if a[8] == 0 && !self.times.is_empty() => {
                    let v = u16_at(a, 20) as usize;
                    if v + 16 <= a.len() {
                        self.times[id] = unix_secs(u64_at(a, v + 8));
                    }
                }
                0x30 if a[8] == 0 => {
                    let v = u16_at(a, 20) as usize;
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

/// A tabela crua do volume, sem as somas da análise.
pub(crate) struct Table {
    pub nodes: Vec<Node>,
    pub names: Vec<u8>,
    pub times: Vec<u32>,
}

fn scan_mft(drive: char, p: &Progress) -> Option<Scan> {
    let t0 = Instant::now();
    let t = read_table(drive, p, false)?;
    let mut s = Scan {
        drive,
        nodes: t.nodes,
        root: ROOT,
        names: t.names,
        first: Vec::new(),
        list: Vec::new(),
        types: Vec::new(),
        files: 0,
        elapsed: Duration::ZERO,
        used_mft: true,
        largest: Vec::new(),
        version: next_version(),
    };
    s.nodes[ROOT as usize].flags |= USED | DIR;
    s.nodes[ROOT as usize].parent = ROOT;
    s.finish();
    s.elapsed = t0.elapsed();
    Some(s)
}

pub(crate) fn read_table(drive: char, p: &Progress, with_times: bool) -> Option<Table> {
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
        // Tabela muito fragmentada continua em registros de extensão, listados no atributo 0x20.
        if let Some((o, len)) = list_attr {
            let a = &first[o..o + len];
            let list = if a[8] == 0 {
                let v = (u16_at(a, 20) as usize).min(a.len());
                a[v..(v + u32_at(a, 16) as usize).min(a.len())].to_vec()
            } else {
                // Lista grande demais para o registro: fica nos pedaços dela no disco.
                let mut lr = Vec::new();
                decode_runs(a, &mut lr);
                let mut l = Vec::new();
                for (lcn, clusters) in lr {
                    if lcn < 0 {
                        return None;
                    }
                    l.extend(read_span(raw, &mut buf, (lcn as u64).saturating_mul(cluster), clusters.saturating_mul(cluster) as usize)?);
                }
                l.truncate(u64_at(a, 48) as usize);
                l
            };
            for (number, vcn) in list_extents(&list) {
                // Extensão ilegível deixaria a tabela pela metade sem aviso: o caminho lento cobre.
                let mut r = read_span(raw, &mut buf, locate(&runs, number * rec as u64, cluster)?, rec)?;
                if !fixup(&mut r) {
                    return None;
                }
                // Só o pedaço que a entrada aponta: ler o registro inteiro repetiria os outros.
                for (t, o, l) in attrs(&r) {
                    if t == 0x80 && r[o + 8] != 0 && u64_at(&r, o + 16) == vcn {
                        decode_runs(&r[o..o + l], &mut runs);
                    }
                }
            }
        }
        if runs.iter().fold(0u64, |s, r| s.saturating_add(r.1)).saturating_mul(cluster) < mft_len {
            return None;
        }

        let total = (mft_len / rec as u64) as usize;
        p.total.store(total as u64, Ordering::Relaxed);
        let times = if with_times { vec![0u32; total + 1] } else { Vec::new() };
        let mut parser = Parser { nodes: vec![Node::default(); total + 1], names: Vec::with_capacity(total * 24), times };
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
        Some(Table { nodes: parser.nodes, names: parser.names, times: parser.times })
    }
}

/// Lê a pasta `root` e tudo dentro dela. Para no meio se pedirem para cancelar.
fn walk(root: std::path::PathBuf, p: &Progress) -> Walked {
    let mut nodes = vec![Node { flags: USED | DIR, ..Default::default() }];
    let mut names: Vec<u8> = Vec::new();
    let mut denied = Vec::new();
    let mut stack = vec![(0u32, root)];
    while let Some((id, dir)) = stack.pop() {
        if p.cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            denied.push(id);
            continue;
        };
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
    Walked { nodes, names, denied }
}

/// Caminho lento: para discos que não são NTFS ou sem administrador.
fn scan_walk(drive: char, p: &Progress) -> Scan {
    let t0 = Instant::now();
    let Walked { nodes, names, .. } = walk(std::path::PathBuf::from(format!("{drive}:\\")), p);
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
        version: next_version(),
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

/// Lê de novo só a pasta `path` (o item `dir` da análise) e troca o conteúdo dela no `slot`,
/// sem analisar o disco todo. Vale também para uma análise feita pela tabela do NTFS.
pub fn refresh(path: String, dir: u32, p: Arc<Progress>, slot: Slot) {
    p.done.store(0, Ordering::Relaxed);
    p.total.store(0, Ordering::Relaxed);
    p.cancel.store(false, Ordering::Relaxed);
    p.running.store(true, Ordering::Relaxed);
    let _ = std::thread::Builder::new().name("analise-pasta".into()).spawn(move || {
        let w = walk(std::path::PathBuf::from(&path), &p);
        if !p.cancel.load(Ordering::Relaxed) {
            // Outra análise pode ter chegado no meio: só troca se a pasta ainda é a mesma.
            if let Some(s) = slot.lock().unwrap().as_mut().filter(|s| s.find(&path) == Some(dir)) {
                s.replace(dir, w);
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Pasta temporária só do teste, apagada no fim.
    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("prisma-mft-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(dir: &Path, name: &str, len: usize) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(name), vec![0u8; len]).unwrap();
    }

    fn scan_of(root: &Path) -> Scan {
        let Walked { nodes, names, .. } = walk(root.to_path_buf(), &Progress::default());
        let mut s = Scan {
            drive: 'X',
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
            version: next_version(),
        };
        s.finish();
        s
    }

    fn child(s: &Scan, parent: u32, name: &str) -> u32 {
        *s.children(parent).iter().find(|k| s.exists(**k) && s.name(**k) == name).unwrap()
    }

    fn ty(s: &Scan, e: &str) -> (u64, u32) {
        s.types.iter().find(|t| t.ext == e).map_or((0, 0), |t| (t.size, t.files))
    }

    #[test]
    fn refresh_moves_the_difference_up_to_the_root() {
        let root = temp("refresh");
        put(&root, "top.bin", 1000);
        put(&root.join("a"), "a.bin", 200);
        put(&root.join("a").join("b"), "old.log", 10);
        put(&root.join("a").join("b"), "keep.log", 5);
        let mut s = scan_of(&root);
        let a = child(&s, s.root, "a");
        let b = child(&s, a, "b");
        assert_eq!(s.nodes[s.root as usize].size, 1215);
        let v = s.version;

        let bp = root.join("a").join("b");
        fs::remove_file(bp.join("old.log")).unwrap();
        put(&bp.join("c"), "new.log", 100);
        put(&bp, "new.iso", 3000);
        s.replace(b, walk(bp.clone(), &Progress::default()));

        assert_eq!(s.nodes[b as usize].size, 3105);
        assert_eq!(s.nodes[b as usize].files, 3);
        assert_eq!(s.nodes[a as usize].size, 3305);
        assert_eq!(s.nodes[a as usize].files, 4);
        assert_eq!(s.nodes[s.root as usize].size, 4305);
        assert_eq!(s.nodes[s.root as usize].files, 5);
        assert_eq!(ty(&s, "log"), (105, 2));
        assert_eq!(ty(&s, "iso"), (3000, 1));
        assert_eq!(s.largest.first().map(|f| s.name(*f)), Some("new.iso"));
        assert_ne!(s.version, v);
        assert_eq!(s.files, 5);
        // A pasta nova entra na árvore, com os filhos dela.
        let c = child(&s, b, "c");
        assert_eq!(s.name(child(&s, c, "new.log")), "new.log");
        assert_eq!(s.children(b).first().map(|k| s.name(*k)), Some("new.iso"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn refresh_keeps_folders_that_did_not_open() {
        let root = temp("denied");
        put(&root, "a.log", 5);
        put(&root.join("sub"), "x.iso", 3000);
        let mut s = scan_of(&root);
        put(&root, "b.log", 7);
        // Simula "sem permissão" em sub: a leitura nova não tem nada dentro dela.
        let mut w = walk(root.clone(), &Progress::default());
        let sub = w.nodes.iter().skip(1).position(|n| n.is_dir()).unwrap() as u32 + 1;
        let kept = w.nodes.iter().filter(|n| n.parent == 0).count();
        w.nodes.truncate(kept);
        w.denied = vec![sub];
        s.replace(s.root, w);
        assert_eq!(s.nodes[s.root as usize].size, 3012);
        assert_eq!(s.files, 3);
        assert_eq!(ty(&s, "iso"), (3000, 1));
        let sub = child(&s, s.root, "sub");
        assert_eq!(s.nodes[sub as usize].size, 3000);
        assert_eq!(s.name(child(&s, sub, "x.iso")), "x.iso");
        // A própria pasta não abriu: nada muda.
        let v = s.version;
        s.replace(s.root, walk(root.join("nao-existe"), &Progress::default()));
        assert_eq!((s.version, s.nodes[s.root as usize].size), (v, 3012));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn files_of_a_type_from_largest_and_forget_updates_types() {
        let root = temp("types");
        put(&root, "x.log", 30);
        put(&root, "y.LOG", 50);
        put(&root, "z.txt", 99);
        put(&root.join("sub"), "w.log", 40);
        put(&root.join("sub"), "noext", 7);
        let mut s = scan_of(&root);
        let logs = s.files_of("log");
        let sizes: Vec<u64> = logs.iter().map(|i| s.nodes[*i as usize].size).collect();
        assert_eq!(sizes, [50, 40, 30]);
        assert_eq!(s.files_of("").iter().map(|i| s.name(*i)).collect::<Vec<_>>(), ["noext"]);
        assert_eq!(ty(&s, "log"), (120, 3));

        s.forget(logs[0]);
        assert_eq!(ty(&s, "log"), (70, 2));
        assert_eq!(s.files_of("log").len(), 2);
        // Apagar a pasta tira os arquivos de dentro dos tipos também.
        s.forget(child(&s, s.root, "sub"));
        assert_eq!(ty(&s, "log"), (30, 1));
        assert_eq!(ty(&s, ""), (0, 0));
        assert_eq!(s.nodes[s.root as usize].size, 129);
        assert_eq!(s.files, 2);
        assert!(s.largest.iter().all(|f| s.exists(*f)));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn system_files_are_not_deleted() {
        for p in [
            r"C:\Windows\System32\kernel32.dll",
            r"C:\pagefile.sys",
            r"C:\hiberfil.sys",
            r"C:\swapfile.sys",
            r"C:\Program Files\App\app.exe",
            r"D:\Program Files (x86)\Jogo\data.pak",
            r"C:\ProgramData\x\y.db",
            r"C:\$Recycle.Bin\a",
        ] {
            assert!(system_file(p), "{p}");
        }
        for p in [r"K:\src\build\a.obj", r"C:\Users\eu\Downloads\big.iso", r"D:\Jogos\Windows\x.bin"] {
            assert!(!system_file(p), "{p}");
        }
    }

    /// Registro FILE de 1024 bytes como o NTFS grava: números da gravação no fim de cada setor.
    fn record_with(attrs: &[Vec<u8>]) -> Vec<u8> {
        let mut r = vec![0u8; 1024];
        r[..4].copy_from_slice(b"FILE");
        r[4..6].copy_from_slice(&48u16.to_le_bytes());
        r[6..8].copy_from_slice(&3u16.to_le_bytes());
        r[20..22].copy_from_slice(&56u16.to_le_bytes());
        r[22..24].copy_from_slice(&1u16.to_le_bytes());
        let mut o = 56;
        for a in attrs {
            r[o..o + a.len()].copy_from_slice(a);
            o += a.len();
        }
        r[o..o + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        // O fim de cada setor vai para a lista e dá lugar ao número 0xABCD.
        for (i, end) in [512usize, 1024].into_iter().enumerate() {
            let (a, b) = (r[end - 2], r[end - 1]);
            r[50 + i * 2] = a;
            r[51 + i * 2] = b;
            r[end - 2..end].copy_from_slice(&[0xCD, 0xAB]);
        }
        r[48..50].copy_from_slice(&[0xCD, 0xAB]);
        r
    }

    fn resident(t: u32, value: &[u8]) -> Vec<u8> {
        let len = (24 + value.len() + 7) & !7;
        let mut a = vec![0u8; len];
        a[..4].copy_from_slice(&t.to_le_bytes());
        a[4..8].copy_from_slice(&(len as u32).to_le_bytes());
        a[16..20].copy_from_slice(&(value.len() as u32).to_le_bytes());
        a[20..22].copy_from_slice(&24u16.to_le_bytes());
        a[24..24 + value.len()].copy_from_slice(value);
        a
    }

    fn file_name(parent: u64, ns: u8, name: &str) -> Vec<u8> {
        let w: Vec<u16> = name.encode_utf16().collect();
        let mut d = vec![0u8; 66];
        d[..8].copy_from_slice(&parent.to_le_bytes());
        d[64] = w.len() as u8;
        d[65] = ns;
        d.extend(w.iter().flat_map(|c| c.to_le_bytes()));
        resident(0x30, &d)
    }

    /// $DATA fora do registro: um pedaço de 0x10 clusters no cluster 0x20.
    fn data_runs(size: u64) -> Vec<u8> {
        let mut a = vec![0u8; 72];
        a[..4].copy_from_slice(&0x80u32.to_le_bytes());
        a[4..8].copy_from_slice(&72u32.to_le_bytes());
        a[8] = 1;
        a[32..34].copy_from_slice(&64u16.to_le_bytes());
        a[40..48].copy_from_slice(&8192u64.to_le_bytes());
        a[48..56].copy_from_slice(&size.to_le_bytes());
        a[64..67].copy_from_slice(&[0x11, 0x10, 0x20]);
        a
    }

    fn parse(r: &mut [u8]) -> Option<(String, Node)> {
        if !fixup(r) {
            return None;
        }
        let mut p = Parser { nodes: vec![Node::default(); 8], names: Vec::new(), times: Vec::new() };
        p.record(3, r);
        let n = p.nodes[3];
        Some((String::from_utf8(p.names[n.name_off as usize..][..n.name_len as usize].to_vec()).unwrap(), n))
    }

    #[test]
    fn reads_a_file_record() {
        let mut r = record_with(&[file_name(5, 2, "PROGRA~1.TXT"), file_name(5, 1, "Programas.txt"), data_runs(5000)]);
        // O que o setor tinha no fim antes de dar lugar ao número da gravação.
        r[50..52].copy_from_slice(&[0x11, 0x22]);
        let (name, n) = parse(&mut r).expect("registro bom");
        // O nome longo vence o 8.3, e o fim dos setores volta ao que era.
        assert_eq!(name, "Programas.txt");
        assert_eq!((n.parent, n.size, n.alloc, n.used()), (5, 5000, 8192, true));
        assert_eq!(&r[510..512], &[0x11, 0x22]);
        let mut runs = Vec::new();
        decode_runs(&data_runs(0), &mut runs);
        assert_eq!(runs, vec![(0x20, 0x10)]);
    }

    #[test]
    fn torn_record_is_skipped() {
        let mut r = record_with(&[file_name(5, 1, "a.txt")]);
        // O segundo setor ainda é da gravação anterior.
        r[1022] = 0x01;
        assert!(parse(&mut r).is_none());
        let mut r = record_with(&[]);
        r[6..8].copy_from_slice(&600u16.to_le_bytes());
        assert!(!fixup(&mut r));
    }

    #[test]
    fn short_attributes_do_not_panic() {
        // 0x30 residente com 16 bytes e 0x80 não residente com 24: cabeçalhos cortados.
        let mut short = file_name(5, 1, "a.txt");
        short[4..8].copy_from_slice(&16u32.to_le_bytes());
        let mut r = record_with(&[short[..16].to_vec()]);
        assert_eq!(parse(&mut r).unwrap().0, "");
        let mut d = data_runs(1);
        d[4..8].copy_from_slice(&24u32.to_le_bytes());
        let mut r = record_with(&[d[..24].to_vec()]);
        assert_eq!(parse(&mut r).unwrap().1.size, 0);
        // Valor apontando para fora do atributo.
        let mut far = file_name(5, 1, "a.txt");
        far[20..22].copy_from_slice(&900u16.to_le_bytes());
        let mut r = record_with(&[far]);
        assert_eq!(parse(&mut r).unwrap().0, "");
    }

    #[test]
    fn attribute_list_points_to_extents() {
        let entry = |t: u32, vcn: u64, rec: u64| {
            let mut e = vec![0u8; 32];
            e[..4].copy_from_slice(&t.to_le_bytes());
            e[4..6].copy_from_slice(&32u16.to_le_bytes());
            e[8..16].copy_from_slice(&vcn.to_le_bytes());
            e[16..24].copy_from_slice(&(rec | 7 << 48).to_le_bytes());
            e
        };
        let l: Vec<u8> = [entry(0x10, 0, 0), entry(0x80, 0, 0), entry(0x80, 900, 17), entry(0x80, 5000, 18)].concat();
        assert_eq!(list_extents(&l), vec![(17, 900), (18, 5000)]);
        assert!(list_extents(&l[..40]).is_empty());
        // Byte 5000 da tabela: segundo pedaço (depois de 1 cluster de 4096), cluster 100.
        let runs = [(10, 1), (-1, 1), (100, 2)];
        assert_eq!(locate(&runs, 100, 4096), Some(10 * 4096 + 100));
        assert_eq!(locate(&runs, 5000, 4096), None);
        assert_eq!(locate(&runs, 8192 + 5, 4096), Some(100 * 4096 + 5));
        assert_eq!(locate(&runs, 1 << 30, 4096), None);
    }
}
