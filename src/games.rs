//! A aba Jogos: a lista dos jogos e o que cada aba guarda para cada um. Os jogos das lojas (Steam, Epic,
//! GOG, Xbox) vêm do Prisma Launcher pelo `prisma-link`; os outros a pessoa põe à mão (um .exe ou .dll, ou
//! uma pasta escaneada). Os perfis ficam onde já estavam, em cada aba, ligados pelo nome do executável.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use prisma_link::StoreGame;

use crate::config::{Config, HdrGame};
use crate::sys::autohdr;
use crate::sys::remap::Profile;

#[derive(Clone, Default, Debug, PartialEq)]
pub struct Game {
    pub name: String,
    /// O executável inteiro; é a chave da lista.
    pub path: String,
    /// A loja ("Steam", "Epic"…); vazio é posto à mão.
    pub store: String,
    /// Tamanho da pasta, em bytes; 0 é desconhecido.
    pub size: u64,
}

impl Game {
    /// `loja|tamanho|caminho|nome`: o nome por último, que pode ter `|`.
    pub fn parse(v: &str) -> Option<Self> {
        let p: Vec<&str> = v.splitn(4, '|').collect();
        (p.len() == 4 && !p[2].is_empty()).then(|| Game { store: p[0].into(), size: p[1].parse().unwrap_or(0), path: p[2].into(), name: p[3].into() })
    }

    pub fn line(&self) -> String {
        format!("{}|{}|{}|{}", self.store, self.size, self.path, self.name)
    }

    pub fn exe(&self) -> &str {
        exe_of(&self.path)
    }
}

pub fn exe_of(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// O nome do jogo pelo arquivo, sem a extensão.
pub fn title(exe: &str) -> String {
    let low = exe.to_ascii_lowercase();
    if low.ends_with(".exe") || low.ends_with(".dll") {
        exe[..exe.len() - 4].to_string()
    } else {
        exe.to_string()
    }
}

/// Numa linha só (o config guarda um item por linha), sem `|` (separa os campos) e sem espaço nas pontas.
fn one_line(s: &str) -> String {
    s.replace(['\r', '\n', '\t', '|'], " ").trim().to_string()
}

/// Jogos novos das lojas desde que a aba mostrou o aviso pela última vez.
pub static FRESH: AtomicUsize = AtomicUsize::new(0);
static BEFORE: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// A lista do Prisma Launcher: a parte com `first` troca a das lojas, as outras somam.
pub fn receive(list: &mut Vec<Game>, games: Vec<StoreGame>, first: bool) {
    let fresh = merge(list, games, first, &mut BEFORE.lock().unwrap());
    FRESH.fetch_add(fresh, Ordering::Relaxed);
}

/// Um jogo posto à mão com o mesmo caminho vira o da loja. Devolve quantos não estavam na lista anterior
/// das lojas; na primeira lista de todas, nenhum (seriam todos).
fn merge(list: &mut Vec<Game>, games: Vec<StoreGame>, first: bool, before: &mut Vec<String>) -> usize {
    if first {
        *before = list.iter().filter(|g| !g.store.is_empty()).map(|g| g.path.to_lowercase()).collect();
        list.retain(|g| g.store.is_empty());
    }
    let mut fresh = 0;
    for s in games {
        let path = one_line(&s.exe.replace('/', "\\"));
        if path.is_empty() || list.iter().any(|g| !g.store.is_empty() && g.path.to_lowercase() == path.to_lowercase()) {
            continue;
        }
        list.retain(|g| g.path.to_lowercase() != path.to_lowercase());
        if !before.is_empty() && !before.contains(&path.to_lowercase()) {
            fresh += 1;
        }
        let name = one_line(&s.name);
        let store = one_line(&s.store);
        list.push(Game { name: if name.is_empty() { title(exe_of(&path)) } else { name }, store: if store.is_empty() { "Prisma Launcher".into() } else { store }, path, size: s.size });
    }
    fresh
}

/// Põe um .exe ou .dll à mão. Falso se já está na lista.
pub fn add(list: &mut Vec<Game>, path: &str, name: &str) -> bool {
    if path.is_empty() || list.iter().any(|g| g.path.eq_ignore_ascii_case(path)) {
        return false;
    }
    let name = one_line(name);
    list.push(Game { name: if name.is_empty() { title(exe_of(path)) } else { name }, path: path.to_string(), ..Default::default() });
    true
}

// O que cada aba guarda para o jogo.
pub const METER: u16 = 1;
pub const ENERGY: u16 = 1 << 1;
pub const HDR: u16 = 1 << 2;
pub const PAD: u16 = 1 << 3;
pub const SHADER: u16 = 1 << 4;
pub const FULL: u16 = 1 << 5;
pub const SOUND: u16 = 1 << 6;
pub const NVIDIA: u16 = 1 << 7;

#[derive(Clone, Default, Debug, PartialEq)]
pub struct Row {
    pub name: String,
    pub exe: String,
    /// Vazio quando só um perfil conhece o jogo, pelo nome do exe.
    pub path: String,
    pub store: String,
    pub size: u64,
    /// Está na lista (loja ou posto à mão); senão só tem perfil.
    pub listed: bool,
    pub funcs: u16,
}

/// Perfis guardados fora do config: os de shader (um arquivo por jogo) e os de controle.
#[derive(Clone, Default, Debug)]
pub struct Extra {
    /// Nome do arquivo de perfil de shader, sem `.txt`, em minúsculas: o nome do exe sem extensão.
    pub shaders: Vec<String>,
    /// (perfil de controle, exe), os padrões primeiro: o primeiro de cada exe é o que abre com ele.
    pub pads: Vec<(String, String)>,
    /// Nomes de todos os perfis de controle, na ordem da aba Controles.
    pub pad_names: Vec<String>,
}

static EXTRA: Mutex<Option<(Instant, Extra)>> = Mutex::new(None);

/// Os perfis de fora do config, lidos de novo a cada dois segundos no máximo (a tela redesenha muito).
pub fn extra() -> Extra {
    let mut g = EXTRA.lock().unwrap();
    if let Some((t, e)) = g.as_ref() {
        if t.elapsed() < Duration::from_secs(2) {
            return e.clone();
        }
    }
    let shaders = std::fs::read_dir(crate::shader_profile::profiles_dir()).into_iter().flatten().flatten().filter_map(|e| e.file_name().to_string_lossy().to_lowercase().strip_suffix(".txt").map(str::to_string)).collect();
    let profiles = crate::sys::remap::profiles();
    let pad_names = profiles.iter().map(|p| p.name.clone()).collect();
    let pads = profiles.iter().filter(|p| p.main).chain(profiles.iter().filter(|p| !p.main)).flat_map(|p| p.games.iter().map(|g| (p.name.clone(), g.clone()))).collect();
    let e = Extra { shaders, pads, pad_names };
    *g = Some((Instant::now(), e.clone()));
    e
}

/// Depois de mudar um perfil de fora do config, para a lista já mostrar.
pub fn forget_extra() {
    *EXTRA.lock().unwrap() = None;
}

/// O perfil de controle `pick` passa a abrir com o jogo: fica só com ele (como na aba Controles) e vira o
/// padrão dele; os outros perfis do jogo continuam com ele, sem ser o padrão. None tira o jogo de todos.
/// Devolve os perfis que mudaram, para gravar.
pub fn set_pad(profiles: Vec<Profile>, exe: &str, pick: Option<&str>) -> Vec<Profile> {
    let same = |g: &String| g.eq_ignore_ascii_case(exe);
    profiles
        .into_iter()
        .filter_map(|mut p| {
            let before = (p.games.clone(), p.main);
            if Some(p.name.as_str()) == pick {
                p.games = vec![exe.to_lowercase()];
                p.main = true;
            } else if p.games.iter().any(same) {
                if pick.is_none() {
                    p.games.retain(|g| !same(g));
                } else {
                    p.main = false;
                }
            }
            ((p.games.clone(), p.main) != before).then_some(p)
        })
        .collect()
}

/// O HDR do jogo saiu do padrão.
pub fn hdr_adjusted(g: &HdrGame) -> bool {
    g.mode != autohdr::DEFAULT || matches!(g.native, autohdr::NATIVE_YES | autohdr::NATIVE_NO) || g.prisma.is_some()
}

/// A lista da aba: os jogos da lista mais os que só têm perfil, cada um com o que as abas ligadas
/// (`on`, bits acima) guardam para ele. Uma aba desligada não marca nem traz jogo.
pub fn rows(cfg: &Config, extra: &Extra, on: u16) -> Vec<Row> {
    let mut rows: Vec<Row> = cfg.games.iter().map(|g| Row { name: g.name.clone(), exe: g.exe().to_string(), path: g.path.clone(), store: g.store.clone(), size: g.size, listed: true, funcs: 0 }).collect();
    let mut mark = |bit: u16, exe: &str, path: &str| mark_row(&mut rows, on, bit, exe, path, false);
    for g in &cfg.meter_games {
        if g.limit_mode != 0 || g.hdr != 0 || g.preset != 0 || g.show != 0 {
            mark(METER, &g.exe, &g.path);
        }
        if !g.power.is_empty() {
            mark(ENERGY, &g.exe, &g.path);
        }
    }
    for g in cfg.hdr_games.iter().filter(|g| hdr_adjusted(g)) {
        mark(HDR, &g.exe, &g.path);
    }
    for a in cfg.fs_apps.iter().filter(|a| a.auto || a.kind != 0) {
        mark(FULL, &a.exe, &a.path);
    }
    for a in cfg.snd_apps.iter().filter(|a| a.pinned || !a.output.is_empty() || !a.input.is_empty()) {
        mark(SOUND, &a.exe, &a.path);
    }
    for (exe, _) in &cfg.eq_apps {
        mark(SOUND, exe, "");
    }
    for exe in &cfg.nv_games {
        mark(NVIDIA, exe, "");
    }
    for (_, exe) in &extra.pads {
        mark(PAD, exe, "");
    }
    for stem in &extra.shaders {
        mark_row(&mut rows, on, SHADER, &format!("{stem}.exe"), "", true);
    }
    rows
}

/// Marca o jogo com o exe (`by_stem`: com o nome sem extensão, o dos perfis de shader), ou o traz.
fn mark_row(rows: &mut Vec<Row>, on: u16, bit: u16, exe: &str, path: &str, by_stem: bool) {
    if on & bit == 0 || exe.is_empty() {
        return;
    }
    let same = |a: &str| if by_stem { title(a).eq_ignore_ascii_case(&title(exe)) } else { a.eq_ignore_ascii_case(exe) };
    let mut hit = false;
    for r in rows.iter_mut().filter(|r| same(&r.exe)) {
        r.funcs |= bit;
        if r.path.is_empty() {
            r.path = path.to_string();
        }
        hit = true;
    }
    if !hit {
        rows.push(Row { name: title(exe), exe: exe.to_string(), path: path.to_string(), funcs: bit, ..Default::default() });
    }
}

/// Ordem por nome (0), loja (1, os postos à mão no fim) ou tamanho (2, o maior primeiro).
pub fn sort(rows: &mut [Row], by: usize) {
    let name = |r: &Row| r.name.to_lowercase();
    match by {
        1 => rows.sort_by_cached_key(|r| (r.store.is_empty(), r.store.to_lowercase(), name(r))),
        2 => rows.sort_by_cached_key(|r| (std::cmp::Reverse(r.size), name(r))),
        _ => rows.sort_by_cached_key(name),
    }
}

/// A busca acha pelo nome ou pelo exe, sem acento nem caixa (`q` já dobrado com `index::fold`).
pub fn matches(r: &Row, q: &[u8]) -> bool {
    if q.is_empty() {
        return true;
    }
    let mut s = Vec::new();
    crate::sys::index::fold(&r.name, &mut s);
    s.push(b' ');
    crate::sys::index::fold(&r.exe, &mut s);
    s.windows(q.len()).any(|w| w == q)
}

// APIs de imagem.
pub const DX9: u8 = 1;
pub const DX10: u8 = 1 << 1;
pub const DX11: u8 = 1 << 2;
pub const DX12: u8 = 1 << 3;
pub const VULKAN: u8 = 1 << 4;
pub const OPENGL: u8 = 1 << 5;
pub const APIS: [(u8, &str); 6] = [(DX9, "DirectX 9"), (DX10, "DirectX 10"), (DX11, "DirectX 11"), (DX12, "DirectX 12"), (VULKAN, "Vulkan"), (OPENGL, "OpenGL")];

const DLLS: [(&[u8], u8); 7] = [(b"d3d9.dll", DX9), (b"d3d10.dll", DX10), (b"d3d10_1.dll", DX10), (b"d3d11.dll", DX11), (b"d3d12.dll", DX12), (b"vulkan-1.dll", VULKAN), (b"opengl32.dll", OPENGL)];

/// Os nomes das APIs, juntos ("DirectX 11, Vulkan").
pub fn api_names(bits: u8) -> String {
    APIS.iter().filter(|(b, _)| bits & b != 0).map(|(_, n)| *n).collect::<Vec<_>>().join(", ")
}

fn le16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// As dlls que um PE importa (e as de import atrasado), em minúsculas. `at(posição, tamanho)` lê o
/// arquivo; pode devolver menos no fim dele.
fn pe_imports(at: &mut dyn FnMut(u64, usize) -> Option<Vec<u8>>) -> Vec<String> {
    let mut names = Vec::new();
    let Some(dos) = at(0, 64) else { return names };
    let Some(pe) = le32(&dos, 0x3c).filter(|_| dos.starts_with(b"MZ")).map(u64::from) else { return names };
    let Some(head) = at(pe, 24).filter(|h| h.starts_with(b"PE\0\0")) else { return names };
    let (Some(sections), Some(opt_size)) = (le16(&head, 6), le16(&head, 20)) else { return names };
    let Some(opt) = at(pe + 24, opt_size as usize) else { return names };
    let dirs = match le16(&opt, 0) {
        Some(0x10b) => 96,
        Some(0x20b) => 112,
        _ => return names,
    };
    let Some(table) = at(pe + 24 + opt_size as u64, sections.min(96) as usize * 40) else { return names };
    let map = |rva: u32| {
        table.chunks_exact(40).find_map(|s| {
            let (size, va, raw, ptr) = (le32(s, 8)?, le32(s, 12)?, le32(s, 16)?, le32(s, 20)?);
            (rva >= va && rva < va.saturating_add(size.max(raw))).then(|| (rva - va) as u64 + ptr as u64)
        })
    };
    // Imports (diretório 1, 20 bytes cada, nome em +12) e imports atrasados (13, 32 bytes, nome em +4).
    for (dir, step, name_at) in [(1usize, 20usize, 12usize), (13, 32, 4)] {
        let Some(off) = le32(&opt, dirs + dir * 8).filter(|r| *r != 0).and_then(|r| map(r)) else { continue };
        let Some(list) = at(off, step * 512) else { continue };
        for d in list.chunks_exact(step) {
            let Some(n) = le32(d, name_at).filter(|n| *n != 0) else { break };
            if let Some(name) = map(n).and_then(|o| at(o, 64)) {
                let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
                names.push(String::from_utf8_lossy(&name[..end]).to_ascii_lowercase());
            }
        }
    }
    names
}

/// Nomes das dlls de imagem no meio do texto do exe (em minúsculas), em ASCII ou UTF-16: os jogos que
/// carregam a API sozinhos, sem import.
fn text_hits(b: &[u8]) -> u8 {
    let wide = |s: &[u8], name: &[u8]| name.iter().enumerate().all(|(k, c)| s.get(2 * k) == Some(c) && s.get(2 * k + 1) == Some(&0));
    let mut bits = 0;
    for i in 0..b.len() {
        if !matches!(b[i], b'd' | b'v' | b'o') {
            continue;
        }
        for (name, bit) in DLLS {
            if name[0] == b[i] && (b[i..].starts_with(name) || wide(&b[i..], name)) {
                bits |= bit;
            }
        }
    }
    bits
}

/// As APIs do executável e se é só provável: pelos imports; sem nenhum, pelo texto dele e pelos arquivos
/// da pasta.
pub fn detect(path: &Path) -> (u8, bool) {
    let Ok(mut f) = File::open(path) else { return (0, false) };
    let mut at = |off: u64, len: usize| {
        f.seek(SeekFrom::Start(off)).ok()?;
        let mut b = Vec::with_capacity(len);
        (&mut f).take(len as u64).read_to_end(&mut b).ok()?;
        Some(b)
    };
    let bits = pe_imports(&mut at).iter().fold(0, |m, n| m | DLLS.iter().find(|(d, _)| d == &n.as_bytes()).map_or(0, |(_, b)| *b));
    if bits != 0 {
        return (bits, false);
    }
    let mut bits = 0;
    if f.metadata().is_ok_and(|m| m.len() <= 128 << 20) && f.seek(SeekFrom::Start(0)).is_ok() {
        let mut buf = vec![0u8; 4 << 20];
        let mut tail: Vec<u8> = Vec::new();
        while let Ok(n) = f.read(&mut buf) {
            if n == 0 {
                break;
            }
            // O fim do pedaço anterior entra no próximo: um nome cortado no meio ainda é achado.
            let mut chunk = std::mem::take(&mut tail);
            chunk.extend_from_slice(&buf[..n]);
            chunk.make_ascii_lowercase();
            bits |= text_hits(&chunk);
            tail = chunk[chunk.len().saturating_sub(32)..].to_vec();
        }
    }
    if let Some(dir) = path.parent() {
        if dir.join("D3D12").join("D3D12Core.dll").is_file() {
            bits |= DX12;
        }
        if dir.join("vulkan-1.dll").is_file() {
            bits |= VULKAN;
        }
    }
    (bits, bits != 0)
}

/// O que se sabe do executável, medido fora da tela.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Info {
    pub exists: bool,
    pub api: u8,
    /// A API veio do texto ou dos arquivos da pasta, não dos imports.
    pub guessed: bool,
    pub anti_cheat: bool,
}

#[derive(Default)]
struct Infos {
    map: HashMap<String, Option<Info>>,
    queue: Vec<String>,
    busy: bool,
}

static INFO: Mutex<Option<Infos>> = Mutex::new(None);

/// Até medir, None; uma thread só mede a fila e pede para redesenhar a aba (`hub`).
pub fn info(path: &str, hub: isize) -> Option<Info> {
    if path.is_empty() {
        return Some(Info::default());
    }
    let mut g = INFO.lock().unwrap();
    let c = g.get_or_insert_with(Infos::default);
    if let Some(v) = c.map.get(path) {
        return *v;
    }
    c.map.insert(path.to_string(), None);
    c.queue.push(path.to_string());
    if !c.busy {
        c.busy = true;
        let _ = std::thread::Builder::new().name("jogos".into()).spawn(move || {
            let mut done = 0;
            loop {
                let next = {
                    let mut g = INFO.lock().unwrap();
                    let c = g.get_or_insert_with(Infos::default);
                    let next = c.queue.pop();
                    c.busy = next.is_some();
                    next
                };
                let Some(p) = next else { break };
                let exists = Path::new(&p).is_file();
                let (api, guessed) = if exists { detect(Path::new(&p)) } else { (0, false) };
                let i = Info { exists, api, guessed, anti_cheat: exists && crate::meter::anti_cheat(&p) };
                if let Some(c) = INFO.lock().unwrap().as_mut() {
                    c.map.insert(p, Some(i));
                }
                done += 1;
                if done % 16 == 0 {
                    redraw(hub);
                }
            }
            redraw(hub);
        });
    }
    None
}

/// A aba abriu: mede de novo (o jogo pode ter sido instalado ou apagado).
pub fn forget_info() {
    if let Some(c) = INFO.lock().unwrap().as_mut() {
        if !c.busy {
            c.map.clear();
        }
    }
}

fn redraw(hub: isize) {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(Some(HWND(hub as _)), crate::app::WM_PICK_GAMES, WPARAM(crate::pages::games::REDRAW), LPARAM(0));
    }
}

/// Um candidato a jogo numa pasta escaneada.
#[derive(Clone, Default, Debug)]
pub struct Cand {
    pub path: PathBuf,
    pub score: i32,
    /// Por que parece o executável do jogo (textos para traduzir).
    pub why: Vec<&'static str>,
    pub api: u8,
    pub guessed: bool,
}

/// Uma pasta escaneada: o jogo que parece ter, com os executáveis do melhor para o pior.
#[derive(Clone, Default, Debug)]
pub struct Found {
    pub name: String,
    pub folder: PathBuf,
    pub cands: Vec<Cand>,
    pub pick: usize,
    pub anti_cheat: bool,
    pub emulator: bool,
    /// Marcado para entrar na lista ("é jogo").
    pub on: bool,
}

/// Começo do nome dos emuladores conhecidos.
const EMULATORS: [&str; 30] = [
    "retroarch", "pcsx2", "rpcs3", "dolphin", "duckstation", "ppsspp", "cemu", "yuzu", "ryujinx", "citra", "xemu", "xenia", "melonds", "mgba", "snes9x", "mesen", "flycast", "redream", "vita3k", "shadps4", "project64", "mame", "bsnes", "ares", "dosbox", "scummvm", "epsxe", "nestopia", "fceux", "lime3ds",
];

pub fn is_emulator(exe: &str) -> bool {
    let low = exe.to_ascii_lowercase();
    EMULATORS.iter().any(|e| low.starts_with(e))
}

/// Pontos do executável para ser o do jogo, e por quê. `key` é o nome da pasta do jogo só com letras e
/// números, em minúsculas.
fn score(path: &Path, depth: usize, size: u64, max: u64, key: &str, api: u8) -> (i32, Vec<&'static str>) {
    let low = path.to_string_lossy().to_lowercase();
    let raw = path.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let stem: String = raw.chars().filter(|c| c.is_alphanumeric()).collect();
    let parent = path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let (mut s, mut why) = (0, Vec::new());
    if raw.ends_with("-shipping") || low.contains("\\binaries\\win64\\") {
        s += 60;
        why.push("executável do Unreal");
    }
    if depth == 0 || matches!(parent.as_str(), "bin" | "bin64" | "x64" | "game") {
        s += 10;
        why.push("na pasta principal");
    }
    if api != 0 {
        s += 40;
        why.push("usa uma API de imagem");
    }
    if !key.is_empty() && (stem.starts_with(key) || key.starts_with(&stem) && stem.len() >= 4) {
        s += 30;
        why.push("nome parecido com a pasta");
    }
    s += (size.min(max) * 20 / max.max(1)) as i32;
    if raw.contains("launcher") {
        s -= 50;
        why.push("parece um iniciador");
    }
    (s, why)
}

/// Os jogos de uma pasta: ela mesma, se já tem executáveis; senão cada subpasta (uma pasta de jogos).
/// `programs` são (nome, pasta) dos programas instalados, que dão o nome certo ao jogo.
// shortcut: anda pelas pastas em vez de usar o índice da Busca; trocar se escanear disco inteiro ficar lento.
pub fn scan(root: &Path, programs: &[(String, String)]) -> Vec<Found> {
    let entries: Vec<_> = std::fs::read_dir(root).into_iter().flatten().flatten().collect();
    let direct = entries.iter().any(|e| e.file_type().is_ok_and(|t| t.is_file()) && e.file_name().to_string_lossy().to_lowercase().ends_with(".exe"));
    let mut groups: Vec<PathBuf> = if direct {
        vec![root.to_path_buf()]
    } else {
        entries.iter().filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && !crate::sys::stores::SKIP_DIRS.contains(&e.file_name().to_string_lossy().to_lowercase().as_str())).map(|e| e.path()).collect()
    };
    groups.sort();
    groups.iter().filter_map(|g| group(g, programs)).collect()
}

fn group(dir: &Path, programs: &[(String, String)]) -> Option<Found> {
    let mut found = Vec::new();
    crate::sys::stores::walk(dir, 0, &mut found, &mut 0);
    if found.is_empty() {
        return None;
    }
    let folder = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let key: String = folder.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
    let max = found.iter().map(|f| f.1).max().unwrap_or(1);
    let mut cands: Vec<Cand> = found
        .into_iter()
        .map(|(path, size, depth)| {
            let (api, guessed) = detect(&path);
            let (score, why) = score(&path, depth, size, max, &key, api);
            Cand { path, score, why, api, guessed }
        })
        .collect();
    cands.sort_by_key(|c| std::cmp::Reverse(c.score));
    cands.truncate(8);
    let best = cands[0].path.to_string_lossy().to_string();
    let name = programs.iter().find(|(_, loc)| !loc.is_empty() && crate::sys::stores::same_dir(Path::new(loc), dir)).map_or(folder, |(n, _)| n.clone());
    let emulator = is_emulator(exe_of(&best));
    Some(Found { name, folder: dir.to_path_buf(), on: cands[0].api != 0 || emulator, emulator, anti_cheat: crate::meter::anti_cheat(&best), cands, pick: 0 })
}

static SCANNING: AtomicBool = AtomicBool::new(false);
static SCANNED: Mutex<Option<Vec<Found>>> = Mutex::new(None);

pub fn scanning() -> bool {
    SCANNING.load(Ordering::Relaxed)
}

/// Escaneia fora da tela; o resultado sai em `take_scan` e a aba é avisada para redesenhar.
pub fn start_scan(root: PathBuf, hub: isize) {
    if SCANNING.swap(true, Ordering::Relaxed) {
        return;
    }
    let _ = std::thread::Builder::new().name("escanear".into()).spawn(move || {
        let programs: Vec<(String, String)> = crate::sys::uninstall::list().into_iter().map(|p| (p.name, p.location)).collect();
        let found = scan(&root, &programs);
        *SCANNED.lock().unwrap() = Some(found);
        SCANNING.store(false, Ordering::Relaxed);
        redraw(hub);
    });
}

pub fn take_scan() -> Option<Vec<Found>> {
    SCANNED.lock().unwrap().take()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str, exe: &str) -> StoreGame {
        StoreGame { name: name.into(), exe: exe.into(), folder: String::new(), store: "Steam".into(), size: 7 }
    }

    #[test]
    fn line_round_trip() {
        let g = Game { name: "A | B".into(), path: "C:\\g\\a.exe".into(), store: "GOG".into(), size: 42 };
        assert_eq!(Game::parse(&g.line()), Some(g.clone()));
        assert_eq!(g.exe(), "a.exe");
        assert_eq!(Game::parse("x|1||nome"), None);
        assert_eq!(title("Jogo.EXE"), "Jogo");
    }

    #[test]
    fn merge_replaces_store_list() {
        let mut list = vec![Game { name: "Manual".into(), path: "C:\\g\\a.exe".into(), ..Default::default() }];
        let mut before = Vec::new();
        // O da loja com o mesmo caminho troca o posto à mão; a primeira lista de todas não tem novos.
        assert_eq!(merge(&mut list, vec![store("A", "C:/g/A.exe")], true, &mut before), 0);
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].name.as_str(), list[0].store.as_str(), list[0].path.as_str()), ("A", "Steam", "C:\\g\\A.exe"));
        // A parte seguinte soma e não repete.
        merge(&mut list, vec![store("A de novo", "C:\\g\\a.exe"), store("", "C:\\b\\b.exe")], false, &mut before);
        assert_eq!(list.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["A", "b"]);
        // Na próxima lista inteira, só o que não estava conta como novo, e o que sumiu da loja sai.
        assert_eq!(merge(&mut list, vec![store("A", "C:\\g\\a.exe"), store("C", "C:\\c\\c.exe")], true, &mut before), 1);
        assert_eq!(list.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["A", "C"]);
    }

    #[test]
    fn rows_join_profiles() {
        let mut cfg = Config::default();
        cfg.games = vec![Game { name: "Jogo".into(), path: "C:\\j\\jogo.exe".into(), store: "Steam".into(), size: 1 }];
        cfg.meter_games.push(crate::config::MeterGame { exe: "JOGO.exe".into(), limit_mode: 1, power: "p".into(), ..Default::default() });
        // Visto rodando, mas sem nada mudado: não é perfil.
        cfg.meter_games.push(crate::config::MeterGame { exe: "visto.exe".into(), ..Default::default() });
        cfg.hdr_games.push(HdrGame { exe: "outro.exe".into(), path: "D:\\o\\outro.exe".into(), mode: autohdr::DEFAULT, ..Default::default() });
        cfg.nv_games.push("outro.exe".into());
        let extra = Extra { shaders: vec!["jogo".into()], pads: vec![("Perfil".into(), "pad.exe".into())], ..Default::default() };
        let all = rows(&cfg, &extra, u16::MAX);
        let by = |exe: &str| all.iter().find(|r| r.exe.eq_ignore_ascii_case(exe)).cloned();
        assert_eq!(by("jogo.exe").map(|r| r.funcs), Some(METER | ENERGY | SHADER));
        assert!(by("visto.exe").is_none());
        let outro = by("outro.exe").unwrap();
        assert_eq!((outro.funcs, outro.listed), (NVIDIA, false));
        assert_eq!(by("pad.exe").map(|r| r.funcs), Some(PAD));
        // Aba desligada não marca nem traz jogo.
        let some = rows(&cfg, &extra, METER);
        assert_eq!(some.len(), 1);
        assert_eq!(some[0].funcs, METER);
    }

    #[test]
    fn sort_and_search() {
        let r = |n: &str, s: &str, size| Row { name: n.into(), exe: format!("{n}.exe"), store: s.into(), size, ..Default::default() };
        let mut v = vec![r("Bravo", "", 5), r("alfa", "Steam", 1), r("Café", "Epic", 9)];
        sort(&mut v, 0);
        assert_eq!(v.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["alfa", "Bravo", "Café"]);
        sort(&mut v, 1);
        assert_eq!(v[2].name, "Bravo");
        sort(&mut v, 2);
        assert_eq!(v[0].name, "Café");
        let mut q = Vec::new();
        crate::sys::index::fold("CAFE", &mut q);
        assert!(matches(&v[0], &q));
        assert!(!matches(&v[1], &q));
    }

    #[test]
    fn score_prefers_game() {
        let key = "meujogo";
        let (ship, _) = score(Path::new("C:\\g\\MeuJogo\\Binaries\\Win64\\MeuJogo-Win64-Shipping.exe"), 2, 50, 100, key, DX12);
        let (boot, _) = score(Path::new("C:\\g\\MeuJogo.exe"), 0, 1, 100, key, 0);
        let (launch, why) = score(Path::new("C:\\g\\MeuJogoLauncher.exe"), 0, 100, 100, key, DX11);
        assert!(ship > boot && ship > launch, "{ship} {boot} {launch}");
        assert!(why.contains(&"parece um iniciador"));
        assert!(is_emulator("PCSX2-qt.exe") && !is_emulator("game.exe"));
    }

    /// PE64 de mentira: um import (d3d12) e um import atrasado (vulkan-1).
    fn fake_pe() -> Vec<u8> {
        let mut f = vec![0u8; 0x400];
        let put = |f: &mut Vec<u8>, at: usize, b: &[u8]| f[at..at + b.len()].copy_from_slice(b);
        put(&mut f, 0, b"MZ");
        put(&mut f, 0x3c, &0x80u32.to_le_bytes());
        put(&mut f, 0x80, b"PE\0\0");
        put(&mut f, 0x86, &1u16.to_le_bytes());
        put(&mut f, 0x94, &240u16.to_le_bytes());
        put(&mut f, 0x98, &0x20bu16.to_le_bytes());
        put(&mut f, 0x98 + 112 + 8, &0x1000u32.to_le_bytes());
        put(&mut f, 0x98 + 112 + 13 * 8, &0x1100u32.to_le_bytes());
        let sec = 0x98 + 240;
        put(&mut f, sec, b".idata\0\0");
        for (i, v) in [0x200u32, 0x1000, 0x200, 0x200].iter().enumerate() {
            put(&mut f, sec + 8 + i * 4, &v.to_le_bytes());
        }
        put(&mut f, 0x200 + 12, &0x1040u32.to_le_bytes());
        put(&mut f, 0x240, b"D3D12.dll\0");
        put(&mut f, 0x300, &1u32.to_le_bytes());
        put(&mut f, 0x304, &0x1140u32.to_le_bytes());
        put(&mut f, 0x340, b"vulkan-1.dll\0");
        f
    }

    #[test]
    fn reads_pe_imports() {
        let f = fake_pe();
        let mut at = |o: u64, n: usize| f.get(o as usize..).map(|s| s[..n.min(s.len())].to_vec());
        assert_eq!(pe_imports(&mut at), ["d3d12.dll", "vulkan-1.dll"]);
        let dir = std::env::temp_dir().join(format!("prisma-jogos-pe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("g.exe"), &f).unwrap();
        assert_eq!(detect(&dir.join("g.exe")), (DX12 | VULKAN, false));
        // Sem import: acha pelo texto, como provável.
        std::fs::write(dir.join("t.exe"), b"MZ....o\0p\0e\0n\0g\0l\x003\x002\0.\0d\0l\0l\0..D3D11.DLL").unwrap();
        assert_eq!(detect(&dir.join("t.exe")), (OPENGL | DX11, true));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn set_pad_makes_pick_main() {
        let p = |name: &str, games: &[&str], main: bool| Profile { games: games.iter().map(|g| g.to_string()).collect(), main, ..Profile::empty(name) };
        // O jogo está em A (padrão); escolher B leva B a padrão e A só deixa de ser.
        let list = vec![p("A", &["jogo.exe", "x.exe"], true), p("B", &["y.exe"], false), p("C", &["z.exe"], false)];
        let got = set_pad(list.clone(), "JOGO.exe", Some("B"));
        assert_eq!(got.iter().map(|p| (p.name.as_str(), p.games.clone(), p.main)).collect::<Vec<_>>(), [("A", vec!["jogo.exe".to_string(), "x.exe".into()], false), ("B", vec!["jogo.exe".into()], true)]);
        // Nenhum: sai de todos, e só muda quem tinha o jogo.
        let got = set_pad(list, "jogo.exe", None);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].name.as_str(), got[0].games.clone(), got[0].main), ("A", vec!["x.exe".to_string()], true));
    }

    #[test]
    fn scan_groups() {
        let root = std::env::temp_dir().join(format!("prisma-jogos-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for p in ["Alfa\\alfa.exe", "Alfa\\unins000.exe", "Beta\\bin\\beta.exe", "Vazio\\leia.txt", "_CommonRedist\\x.exe"] {
            let p = root.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"MZ").unwrap();
        }
        let progs = vec![("Alfa: Edição Completa".to_string(), root.join("Alfa").display().to_string())];
        let found = scan(&root, &progs);
        assert_eq!(found.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Alfa: Edição Completa", "Beta"]);
        assert_eq!(found[0].cands.len(), 1);
        assert!(!found[0].on);
        // Uma pasta com exe direto é um jogo só.
        assert_eq!(scan(&root.join("Beta"), &[]).len(), 1);
        std::fs::remove_dir_all(&root).ok();
    }
}
