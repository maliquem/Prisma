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

/// Copiar ajustes: cada linha que `from` tem no Medidor e Energia, HDR, Som e Fullscreenizer passa
/// para `to`, trocando a dele; o que `from` não tem fica como estava em `to`. Os perfis de controle
/// ganham `to` onde `from` está. Devolve os perfis que mudaram, para gravar.
pub fn copy_settings(cfg: &mut Config, profiles: Vec<Profile>, from: &str, to: &str, to_path: &str) -> Vec<Profile> {
    fn take<T: Clone>(list: &mut Vec<T>, exe: impl Fn(&T) -> &str, from: &str, to: &str, set: impl Fn(&mut T)) {
        if let Some(mut t) = list.iter().find(|x| exe(x).eq_ignore_ascii_case(from)).cloned() {
            set(&mut t);
            list.retain(|x| !exe(x).eq_ignore_ascii_case(to));
            list.push(t);
        }
    }
    let (exe, path) = (to.to_string(), to_path.to_string());
    // O que é fato do jogo, não escolha, fica o de `to`: o último dia, a injeção que caiu ou achou
    // anti-cheat e o HDR próprio. O automático mede de novo no outro jogo.
    let (last, inject_off) = cfg.meter_games.iter().find(|g| g.exe.eq_ignore_ascii_case(to)).map_or((0, false), |g| (g.last, g.inject_off));
    let native = cfg.hdr_games.iter().find(|g| g.exe.eq_ignore_ascii_case(to)).map_or(autohdr::NATIVE_UNKNOWN, |g| g.native);
    take(&mut cfg.meter_games, |g| &g.exe, from, to, |g| (g.exe, g.path, g.measured, g.last, g.inject_off) = (exe.clone(), path.clone(), false, last, inject_off));
    take(&mut cfg.hdr_games, |g| &g.exe, from, to, |g| (g.exe, g.path, g.scan, g.mods, g.ac_off, g.native) = (exe.clone(), path.clone(), None, String::new(), false, native));
    take(&mut cfg.snd_apps, |a| &a.exe, from, to, |a| (a.exe, a.path) = (exe.clone(), path.clone()));
    take(&mut cfg.eq_apps, |a| &a.0, from, to, |a| a.0 = exe.clone());
    take(&mut cfg.fs_apps, |a| &a.exe, from, to, |a| (a.exe, a.path) = (exe.clone(), path.clone()));
    profiles
        .into_iter()
        .filter(|p| p.games.iter().any(|g| g.eq_ignore_ascii_case(from)) && !p.games.iter().any(|g| g.eq_ignore_ascii_case(to)))
        .map(|mut p| {
            p.games.push(to.to_lowercase());
            p
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
    // Quem só tem perfil e é programa de apoio (vigia de travamento, lançador) não é jogo.
    rows.retain(|r| r.listed || !crate::sys::power::is_helper(&r.exe));
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

/// A API das dlls carregadas no jogo rodando (nomes em minúsculas). Uma só: o jogo DX12 costuma carregar
/// o d3d11 também, e o opengl32 vem junto com muita coisa.
pub fn seen_api<'a>(names: impl IntoIterator<Item = &'a str>) -> u8 {
    let names: Vec<&str> = names.into_iter().collect();
    let order = [("d3d12.dll", DX12), ("vulkan-1.dll", VULKAN), ("d3d11.dll", DX11), ("d3d10.dll", DX10), ("d3d10_1.dll", DX10), ("d3d9.dll", DX9), ("opengl32.dll", OPENGL)];
    order.iter().find(|(n, _)| names.contains(n)).map_or(0, |(_, b)| *b)
}

/// Guarda a API vista rodando (`game_api` no config). Verdadeiro se mudou.
pub fn see_api(list: &mut Vec<(String, u8)>, exe: &str, api: u8) -> bool {
    if exe.is_empty() || api == 0 {
        return false;
    }
    match list.iter_mut().find(|(e, _)| e.eq_ignore_ascii_case(exe)) {
        Some((_, a)) if *a == api => false,
        Some((_, a)) => {
            *a = api;
            true
        }
        None => {
            list.push((exe.to_string(), api));
            true
        }
    }
}

/// O jogo rodando (`path`) saiu de um posto à mão com outro exe (um iniciador): o da lista passa a ser
/// ele. Vale quando o pai do processo (`parent`) é o exe da lista, ou quando o jogo está dentro da pasta
/// dele, que não é a raiz de um disco nem uma pasta de primeiro nível. Devolve o nome do jogo trocado.
pub fn seen_exe(list: &mut [Game], path: &str, parent: Option<&str>) -> Option<String> {
    let low = path.to_lowercase();
    if path.is_empty() || list.iter().any(|g| g.path.to_lowercase() == low) {
        return None;
    }
    let inside = |g: &Game| {
        let Some(dir) = Path::new(&g.path).parent() else { return false };
        let deep = dir.components().filter(|c| matches!(c, std::path::Component::Normal(_))).count() >= 2;
        deep && low.starts_with(&format!("{}\\", dir.to_string_lossy().to_lowercase()))
    };
    let by_parent = |g: &Game| parent.is_some_and(|p| p.eq_ignore_ascii_case(&g.path));
    let mut hits = list.iter_mut().filter(|g| g.store.is_empty() && (by_parent(g) || inside(g)));
    let g = hits.next()?;
    if hits.next().is_some() {
        return None;
    }
    g.path = path.to_string();
    Some(g.name.clone())
}

/// O que se sabe do executável, medido fora da tela.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Info {
    pub exists: bool,
    pub api: u8,
    /// A API veio do texto ou dos arquivos da pasta, não dos imports.
    pub guessed: bool,
    /// O anti-cheat achado: o nome, ou vazio quando o sinal não diz qual.
    pub anti_cheat: Option<&'static str>,
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
                let i = Info { exists, api, guessed, anti_cheat: if exists { crate::meter::anti_cheat_name(&p) } else { None } };
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

/// O que a IA achou na PCGamingWiki. Só sugere: a pessoa aceita ou não.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct AiInfo {
    pub exe: String,
    pub api: u8,
    /// None: a wiki não diz. Some(""): diz que não tem.
    pub anti_cheat: Option<String>,
    pub hdr: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ai {
    Asking,
    Done(AiInfo),
    /// O motivo curto do `safeupd` (rede, chave, limite, resposta).
    Failed(String),
}

static AI: Mutex<Option<HashMap<String, Ai>>> = Mutex::new(None);

pub fn ai_prompt(name: &str, exe: &str) -> String {
    format!(
        "Look up the PC game \"{name}\" (its executable may be {exe}) on PCGamingWiki (pcgamingwiki.com) with a web search. Answer ONLY with one JSON object, no other text: {{\"exe\":\"main game executable file name, for example Game-Win64-Shipping.exe\",\"apis\":[\"DirectX 9|DirectX 10|DirectX 11|DirectX 12|Vulkan|OpenGL\", ...],\"anti_cheat\":\"anti-cheat name, or empty if it has none\",\"native_hdr\":true|false|null}}. Use null when PCGamingWiki does not say."
    )
}

/// A resposta da IA em `AiInfo`; None quando não veio o JSON pedido.
pub fn parse_ai(text: &str) -> Option<AiInfo> {
    let v = crate::sys::safeupd::json_in(text)?;
    let api = v["apis"].as_array().map_or(0, |a| {
        a.iter().filter_map(|s| s.as_str()).fold(0, |m, s| {
            let s = s.to_lowercase();
            m | if s.contains("vulkan") {
                VULKAN
            } else if s.contains("opengl") {
                OPENGL
            } else if s.contains("12") {
                DX12
            } else if s.contains("11") {
                DX11
            } else if s.contains("10") {
                DX10
            } else if s.contains('9') {
                DX9
            } else {
                0
            }
        })
    });
    let exe = v["exe"].as_str().map(|e| e.trim().rsplit(['\\', '/']).next().unwrap_or("").to_string()).filter(|e| e.to_lowercase().ends_with(".exe")).unwrap_or_default();
    Some(AiInfo { exe, api, anti_cheat: v["anti_cheat"].as_str().map(|s| s.trim().to_string()), hdr: v["native_hdr"].as_bool() })
}

/// O exe com esse nome na pasta do jogo (até 3 pastas abaixo, como o escanear) ou direto numa das duas
/// acima: a IA dá o exe principal, que pode estar acima de `Binaries\Win64`. Não sobe até a raiz do disco
/// nem a uma pasta de primeiro nível, onde outro jogo pode ter um exe com o mesmo nome.
pub fn find_exe(dir: &Path, name: &str) -> Option<PathBuf> {
    let mut found = Vec::new();
    crate::sys::stores::walk(dir, 0, &mut found, &mut 0);
    let same = |p: &Path| p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name));
    found.into_iter().map(|f| f.0).find(|p| same(p)).or_else(|| {
        dir.ancestors().skip(1).take(2).filter(|d| d.components().filter(|c| matches!(c, std::path::Component::Normal(_))).count() >= 2).map(|d| d.join(name)).find(|p| p.is_file())
    })
}

/// Pergunta fora da tela; a resposta sai em `ai_state` e a aba é avisada para redesenhar.
pub fn ask_ai(key: &str, name: &str, exe: &str, hub: isize) {
    let id = exe.to_lowercase();
    AI.lock().unwrap().get_or_insert_with(HashMap::new).insert(id.clone(), Ai::Asking);
    let (key, prompt) = (key.to_string(), ai_prompt(name, exe));
    let _ = std::thread::Builder::new().name("perguntar-ia".into()).spawn(move || {
        let got = match crate::sys::safeupd::ask_gemini(&key, &prompt) {
            Ok(t) => parse_ai(&t).map_or(Ai::Failed("resposta".into()), Ai::Done),
            Err(e) => Ai::Failed(e),
        };
        AI.lock().unwrap().get_or_insert_with(HashMap::new).insert(id, got);
        redraw(hub);
    });
}

pub fn ai_state(exe: &str) -> Option<Ai> {
    AI.lock().unwrap().as_ref()?.get(&exe.to_lowercase()).cloned()
}

/// Foto de teste (`--test-ai`): uma resposta pronta, sem chamar o Gemini.
#[cfg(debug_assertions)]
pub fn fake_ai(exe: &str) {
    let i = AiInfo { exe: "Jogo-Win64-Shipping.exe".into(), api: DX11 | DX12, anti_cheat: Some("Easy Anti-Cheat".into()), hdr: Some(true) };
    AI.lock().unwrap().get_or_insert_with(HashMap::new).insert(exe.to_lowercase(), Ai::Done(i));
}

pub fn forget_ai(exe: &str) {
    if let Some(m) = AI.lock().unwrap().as_mut() {
        m.remove(&exe.to_lowercase());
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
    /// Como em `Info::anti_cheat`.
    pub anti_cheat: Option<&'static str>,
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
/// Com o disco no índice da Busca, os executáveis saem dele sem andar pelas pastas.
pub fn scan(root: &Path, programs: &[(String, String)]) -> Vec<Found> {
    if let Some(list) = crate::sys::index::exes_under(root) {
        return scan_indexed(root, list, programs);
    }
    let entries: Vec<_> = std::fs::read_dir(root).into_iter().flatten().flatten().collect();
    let direct = entries.iter().any(|e| e.file_type().is_ok_and(|t| t.is_file()) && e.file_name().to_string_lossy().to_lowercase().ends_with(".exe"));
    let mut groups: Vec<PathBuf> = if direct {
        vec![root.to_path_buf()]
    } else {
        entries.iter().filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && !crate::sys::stores::SKIP_DIRS.contains(&e.file_name().to_string_lossy().to_lowercase().as_str())).map(|e| e.path()).collect()
    };
    groups.sort();
    groups
        .iter()
        .filter_map(|g| {
            let mut found = Vec::new();
            crate::sys::stores::walk(g, 0, &mut found, &mut 0);
            group(g, found, programs)
        })
        .collect()
}

/// O mesmo que `walk` faria com os .exe do índice: até 3 pastas abaixo, sem as de
/// redistribuíveis e sem os executáveis que não são jogo, agrupados como em `scan`.
fn scan_indexed(root: &Path, list: Vec<(PathBuf, u64)>, programs: &[(String, String)]) -> Vec<Found> {
    let base = root.to_string_lossy().trim_end_matches('\\').to_string();
    // O índice acha sem caixa; a parte depois da pasta mantém a caixa do disco, porque o nome da pasta vira o nome do jogo.
    let mut rel: Vec<(Vec<String>, PathBuf, u64)> = list
        .into_iter()
        .filter_map(|(p, size)| {
            let s = p.to_str()?;
            // Depois da raiz vem `\`: `D:\Jogos` não é pai de `D:\Jogos Antigos\x.exe`.
            let rest = s.get(base.len()..)?;
            if !s[..base.len()].eq_ignore_ascii_case(&base) || !(rest.is_empty() || rest.starts_with('\\')) {
                return None;
            }
            let parts: Vec<String> = rest.split('\\').filter(|c| !c.is_empty()).map(String::from).collect();
            let name = parts.last()?.to_lowercase();
            let dirs = &parts[..parts.len() - 1];
            let ok = dirs.len() <= 4 && !dirs.iter().any(|d| crate::sys::stores::SKIP_DIRS.contains(&d.to_lowercase().as_str())) && !crate::sys::stores::NOT_GAME.iter().any(|n| name.contains(n));
            ok.then_some((parts, p, size))
        })
        .collect();
    if rel.iter().any(|(parts, ..)| parts.len() == 1) {
        rel.retain(|(parts, ..)| parts.len() <= 4);
        let found = rel.into_iter().map(|(parts, p, s)| (p, s, parts.len() - 1)).collect();
        return group(root, found, programs).into_iter().collect();
    }
    let mut by: std::collections::BTreeMap<String, (PathBuf, Vec<(PathBuf, u64, usize)>)> = Default::default();
    for (parts, p, s) in rel {
        by.entry(parts[0].to_lowercase()).or_insert_with(|| (root.join(&parts[0]), Vec::new())).1.push((p, s, parts.len() - 2));
    }
    by.into_values().filter_map(|(dir, found)| group(&dir, found, programs)).collect()
}

fn group(dir: &Path, found: Vec<(PathBuf, u64, usize)>, programs: &[(String, String)]) -> Option<Found> {
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
    Some(Found { name, folder: dir.to_path_buf(), on: cands[0].api != 0 || emulator, emulator, anti_cheat: crate::meter::anti_cheat_name(&best), cands, pick: 0 })
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
        // O vigia de travamento do Unity tem ajuste no Medidor, mas não é jogo.
        cfg.meter_games.push(crate::config::MeterGame { exe: "UnityCrashHandler64.exe".into(), show: 1, ..Default::default() });
        let extra = Extra { shaders: vec!["jogo".into()], pads: vec![("Perfil".into(), "pad.exe".into())], ..Default::default() };
        let all = rows(&cfg, &extra, u16::MAX);
        let by = |exe: &str| all.iter().find(|r| r.exe.eq_ignore_ascii_case(exe)).cloned();
        assert_eq!(by("jogo.exe").map(|r| r.funcs), Some(METER | ENERGY | SHADER));
        assert!(by("visto.exe").is_none());
        assert!(by("UnityCrashHandler64.exe").is_none());
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
        // Pelo índice sai o mesmo que andando pelas pastas.
        let exes: Vec<(PathBuf, u64)> = [r"Alfa\alfa.exe", r"Alfa\unins000.exe", r"Beta\bin\beta.exe", r"_CommonRedist\x.exe", r"Fundo\a\b\c\d\longe.exe"].iter().map(|p| (root.join(p), 2)).collect();
        let by_index = scan_indexed(&root, exes.clone(), &progs);
        assert_eq!(by_index.iter().map(|f| (f.name.as_str(), f.cands.len())).collect::<Vec<_>>(), [("Alfa: Edição Completa", 1), ("Beta", 1)]);
        assert_eq!(by_index[1].folder, root.join("Beta"));
        let beta: Vec<_> = exes.into_iter().filter(|e| e.0.starts_with(root.join("Beta"))).collect();
        assert_eq!(scan_indexed(&root.join("Beta"), beta, &[]).len(), 1);
        // Pasta irmã com o mesmo começo de nome não é filha da raiz.
        let irma = vec![(PathBuf::from(format!(r"{}Antigos\jogo.exe", root.display())), 2)];
        assert!(scan_indexed(&root, irma, &[]).is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn api_and_exe_seen_running() {
        // O DX12 carrega o d3d11 também: vence o 12.
        assert_eq!(seen_api(["kernel32.dll", "d3d11.dll", "d3d12.dll"]), DX12);
        assert_eq!(seen_api(["kernel32.dll"]), 0);
        let mut list = Vec::new();
        assert!(see_api(&mut list, "Jogo.exe", DX11));
        assert!(!see_api(&mut list, "jogo.exe", DX11));
        assert!(see_api(&mut list, "jogo.exe", VULKAN));
        assert!(!see_api(&mut list, "jogo.exe", 0));
        assert_eq!(list, [("Jogo.exe".to_string(), VULKAN)]);
        let g = |name: &str, path: &str, store: &str| Game { name: name.into(), path: path.into(), store: store.into(), size: 0 };
        let mut games = vec![g("Jogo", r"D:\Jogos\Jogo\Launcher.exe", ""), g("Outro", r"D:\Jogos\Outro\Game.exe", ""), g("Loja", r"D:\Steam\X\x.exe", "Steam")];
        // Dentro da pasta do posto à mão: troca e diz o nome.
        assert_eq!(seen_exe(&mut games, r"D:\Jogos\Jogo\bin\Jogo.exe", None), Some("Jogo".into()));
        assert_eq!(games[0].path, r"D:\Jogos\Jogo\bin\Jogo.exe");
        // Já na lista, de loja, ou fora de qualquer pasta: nada muda.
        assert_eq!(seen_exe(&mut games, r"D:\Jogos\Jogo\bin\Jogo.exe", None), None);
        assert_eq!(seen_exe(&mut games, r"D:\Steam\X\y.exe", None), None);
        assert_eq!(seen_exe(&mut games, r"E:\Solto\z.exe", None), None);
        // Pelo processo pai, mesmo fora da pasta.
        assert_eq!(seen_exe(&mut games, r"E:\Solto\z.exe", Some(r"d:\jogos\outro\game.exe")), Some("Outro".into()));
        // Pasta de primeiro nível não conta: dois jogos em D:\Raiz pegariam qualquer exe de lá.
        let mut raiz = vec![g("R", r"D:\Raiz\r.exe", "")];
        assert_eq!(seen_exe(&mut raiz, r"D:\Raiz\sub\s.exe", None), None);
    }

    #[test]
    fn copy_settings_replaces_only_what_from_has() {
        use crate::config::{FsApp, MeterGame};
        let mut cfg = Config::default();
        cfg.meter_games.push(MeterGame { exe: "a.exe".into(), path: r"C:\a\a.exe".into(), limit_mode: 2, limit_fps: 90, measured: true, ..Default::default() });
        cfg.meter_games.push(MeterGame { exe: "B.exe".into(), path: r"C:\b\B.exe".into(), limit_mode: 1, last: 77, inject_off: true, ..Default::default() });
        cfg.hdr_games.push(HdrGame { exe: "a.exe".into(), mode: 1, native: autohdr::NATIVE_YES, ..Default::default() });
        cfg.fs_apps.push(FsApp { exe: "b.exe".into(), path: r"C:\b\B.exe".into(), auto: true, kind: 1 });
        cfg.eq_apps.push(("a.exe".into(), "Graves".into()));
        let p = |name: &str, games: &[&str]| Profile { games: games.iter().map(|g| g.to_string()).collect(), ..Profile::empty(name) };
        let pads = copy_settings(&mut cfg, vec![p("A", &["a.exe"]), p("B", &["b.exe", "a.exe"]), p("C", &["c.exe"])], "A.exe", "b.exe", r"C:\b\B.exe");
        let b = cfg.meter_games.iter().find(|g| g.exe == "b.exe").unwrap();
        assert_eq!((b.limit_mode, b.limit_fps, b.measured, b.last, b.path.as_str()), (2, 90, false, 77, r"C:\b\B.exe"));
        assert!(b.inject_off);
        let h = cfg.hdr_games.iter().find(|g| g.exe == "b.exe").unwrap();
        assert_eq!((h.mode, h.native), (1, autohdr::NATIVE_UNKNOWN));
        assert_eq!(cfg.meter_games.len(), 2);
        // O de `from` fica, o Fullscreenizer de `to` também (o outro não tinha).
        assert!(cfg.meter_games.iter().any(|g| g.exe == "a.exe" && g.limit_fps == 90));
        assert_eq!(cfg.fs_apps.len(), 1);
        assert_eq!(cfg.eq_apps, [("a.exe".to_string(), "Graves".to_string()), ("b.exe".into(), "Graves".into())]);
        assert_eq!(pads.iter().map(|p| (p.name.as_str(), p.games.clone())).collect::<Vec<_>>(), [("A", vec!["a.exe".to_string(), "b.exe".into()])]);
    }

    #[test]
    fn ai_answer() {
        let t = r#"Aqui está:
```json
{"exe":"Binaries\\Win64\\Jogo-Win64-Shipping.exe","apis":["DirectX 11","Direct3D 12","Vulkan"],"anti_cheat":"Easy Anti-Cheat","native_hdr":true}
```"#;
        assert_eq!(parse_ai(t), Some(AiInfo { exe: "Jogo-Win64-Shipping.exe".into(), api: DX11 | DX12 | VULKAN, anti_cheat: Some("Easy Anti-Cheat".into()), hdr: Some(true) }));
        let t = r#"{"exe":"não sei","apis":["DirectX 9.0c"],"anti_cheat":"","native_hdr":null}"#;
        assert_eq!(parse_ai(t), Some(AiInfo { exe: String::new(), api: DX9, anti_cheat: Some(String::new()), hdr: None }));
        assert_eq!(parse_ai("sem json"), None);
    }
}
