//! Quem compartilha: captura a tela principal e o som, codifica e manda para quem assiste.
//! Várias pessoas podem assistir juntas (co-op): cada uma com as permissões do aparelho dela,
//! o mouse e o teclado aplicados aqui e os controles dela virando controles virtuais.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::*;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Media::Audio::eRender;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::Media::timeBeginPeriod;
use windows::Win32::System::Com::*;
use windows::Win32::System::Threading::*;
use windows::Win32::System::WinRT::Direct3D11::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::pads::{Bridge, VirtualPad};
use super::*;
use crate::capture::live_enc::{LiveEncoder, VideoFmt};
use crate::capture::engine::{bgra_texture, bitrate, even, ConvOut, Prep};
use crate::capture::hdr::ToneMap;
use crate::capture::mux::{Codec, Pkt};
use crate::capture::sound;
use crate::capture::{monitor_color, monitor_item, D3d};

/// Sem notícia de quem assiste por este tempo, ela sai da sessão.
const SILENCE: Duration = Duration::from_secs(5);
/// Controle sem notícia por este tempo fica parado (solto), sem sair.
const PAD_STALE: Duration = Duration::from_secs(1);
/// Vagas do XInput: quatro controles no Windows, contando os de verdade do host.
const MAX_PADS: usize = 4;

#[derive(Clone, Copy)]
pub(super) struct Hello {
    w: u32,
    h: u32,
    hz: u32,
    codecs: u8,
    pub(super) pid: u32,
    pub(super) flags: u16,
}

impl Hello {
    pub(super) fn from_msg(m: &Msg) -> Option<Self> {
        match *m {
            Msg::Hello { w, h, hz, codecs, pid, flags } => Some(Hello { w, h, hz, codecs, pid, flags }),
            _ => None,
        }
    }
}

/// Quem chegou e vai entrar na sessão.
pub(super) struct Joiner {
    pub link: Arc<dyn Link>,
    pub hello: Hello,
    pub perms: u8,
    pub name: String,
    /// Id do aparelho pareado (vazio pelo UDP direto, que não tem pareamento).
    pub id: String,
}

/// Um controle de quem assiste, ligado aqui.
struct Slot {
    dev: Box<dyn VirtualPad>,
    seq: u16,
    heard: Instant,
    /// Parado por falta de notícia.
    idle: bool,
    rumble: (u8, u8),
    rumble_sent: Instant,
}

impl Slot {
    fn player(&self, fallback: usize) -> u8 {
        self.dev.slot().map_or(fallback as u8 + 1, |s| s + 1)
    }
}

/// Uma pessoa assistindo.
struct Guest {
    link: Arc<dyn Link>,
    hello: Hello,
    perms: u8,
    name: String,
    id: String,
    input: Option<Injector>,
    pads: [Option<Slot>; PADS],
    /// Último motivo de recusa avisado para cada controle, para não repetir a cada pacote.
    refused: [u8; PADS],
    heard: Instant,
    gone: bool,
    clip_in: super::clip::Pieces,
    /// Imagem ou arquivos com esta pessoa.
    rich: super::clip::Rich,
    /// Perda de pacotes de vídeo que ela mediu no último segundo, em milésimos (`Msg::Loss`; zero
    /// enquanto não chega nenhuma, e sempre com versões que não medem).
    loss: u16,
}

impl Guest {
    fn new(j: Joiner) -> Self {
        Guest { link: j.link, hello: j.hello, perms: j.perms, name: j.name, id: j.id, input: None, pads: Default::default(), refused: [PAD_OK; PADS], heard: Instant::now(), gone: false, clip_in: Default::default(), rich: Default::default(), loss: 0 }
    }

    fn send(&self, m: &Msg, out: &mut Vec<u8>) {
        write(out, m);
        self.link.send(out);
    }

    /// Recebe a paridade do FEC: pediu, e não está no mesmo PC (sem rede para perder pacote).
    fn wants_fec(&self) -> bool {
        self.hello.flags & FEC_OK != 0 && !self.link.same_pc()
    }

    /// Como a página Compartilhar tela acha esta pessoa: o id do aparelho, ou o endereço pelo UDP.
    fn key(&self) -> &str {
        if self.id.is_empty() {
            &self.name
        } else {
            &self.id
        }
    }
}

/// Quem está na sala e a ponte dos controles. Vive entre uma sessão e outra, para os controles não
/// caírem quando o vídeo recomeça.
struct Room {
    guests: Vec<Guest>,
    bridge: Option<Bridge>,
    /// Por que a ponte não abriu, e quando tentar de novo.
    bridge_err: Option<(String, Instant)>,
    /// O jogo em primeiro plano tem anti-cheat: os controles virtuais saem.
    anti_cheat: bool,
    checked: Instant,
    /// A área de transferência do host, aberta quando entra alguém com a permissão `COPY`.
    clip: Option<super::clip::Clip>,
    clip_id: u16,
    /// Número do próximo quadro de vídeo, contínuo entre as sessões da sala.
    frame_id: u32,
    /// O PyroWave não abriu nesta sala: fica o automático até ela acabar (senão recomeçaria sem fim).
    wave_failed: bool,
}

impl Room {
    fn pads(&self) -> usize {
        self.guests.iter().map(|g| g.pads.iter().flatten().count()).sum()
    }

    /// Vagas do XInput dos controles virtuais daqui.
    fn virtual_slots(&self) -> u8 {
        self.guests.iter().flat_map(|g| g.pads.iter().flatten()).filter_map(|s| s.dev.slot()).fold(0, |m, s| m | 1 << s)
    }

    fn bridge(&mut self) -> std::result::Result<&Bridge, String> {
        if self.bridge.is_none() {
            if let Some((e, t)) = &self.bridge_err {
                if t.elapsed() < Duration::from_secs(10) {
                    return Err(e.clone());
                }
            }
            match Bridge::open() {
                Ok(b) => {
                    eprintln!("controles: ponte {} aberta", b.name);
                    self.bridge = Some(b);
                    self.bridge_err = None;
                }
                Err(e) => {
                    eprintln!("controles: {e}");
                    self.bridge_err = Some((e.clone(), Instant::now()));
                    return Err(e);
                }
            }
        }
        Ok(self.bridge.as_ref().unwrap())
    }
}

pub(super) enum End {
    /// Todo mundo saiu.
    Gone,
    /// Recomeça com quem está na sala (alguém reabriu, ou chegou alguém que pede outro vídeo).
    Again,
    /// Outro chegou pelo UDP (só na rede local, uma pessoa por vez).
    Stranger(Hello, SocketAddr),
}

/// Prepara o Windows e o Direct3D para compartilhar.
pub(super) fn prepare() -> Option<D3d> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_LITE);
        let _ = timeBeginPeriod(1);
    }
    match device(None) {
        Ok(d) => Some(d),
        Err(e) => {
            eprintln!("sem Direct3D: {e:?}");
            None
        }
    }
}

/// Direct3D para compartilhar, numa placa escolhida ou na padrão.
fn device(adapter: Option<&windows::Win32::Graphics::Dxgi::IDXGIAdapter1>) -> Result<D3d> {
    let d = crate::capture::d3d_on(adapter)?;
    unsafe {
        if let Ok(mt) = d.ctx.cast::<ID3D11Multithread>() {
            let _ = mt.SetMultithreadProtected(true);
        }
    }
    Ok(d)
}

/// Quantas pessoas assistem juntas: a opção da página (padrão 3), ou `PRISMA_GUESTS` por cima.
fn max_guests() -> usize {
    std::env::var("PRISMA_GUESTS").ok().and_then(|v| v.parse().ok()).unwrap_or_else(|| Opts::load().guests).clamp(1, 8)
}

/// HDR opção B: com a opção ligada, o HDR do Windows desliga enquanto alguém sem tela HDR assiste,
/// e volta quando a sala esvazia. A marca no disco faz ele voltar no próximo início do Prisma,
/// mesmo se o Prisma cair no meio.
struct HdrOff(Option<(windows::Win32::Foundation::LUID, u32)>);

fn hdr_mark() -> std::path::PathBuf {
    super::dir().join("hdr_desligado.txt")
}

impl HdrOff {
    fn new(first: &Hello) -> Self {
        let off = std::env::var("PRISMA_HDR").map(|v| v == "desligar").unwrap_or_else(|_| room_opts().hdr_off);
        if !off || first.flags & SEES_HDR != 0 {
            return HdrOff(None);
        }
        let Some((adapter, output, true)) = crate::sys::hdr::hdr_output() else { return HdrOff(None) };
        let mark = format!("{} {} {} {}\n", adapter.LowPart, adapter.HighPart, output, std::process::id());
        if std::fs::create_dir_all(super::dir()).and_then(|_| std::fs::write(hdr_mark(), mark)).is_err() {
            eprintln!("HDR: não deu para gravar a marca de volta; o HDR fica ligado");
            return HdrOff(None);
        }
        if crate::sys::hdr::set_hdr(adapter, output, false) {
            eprintln!("HDR do Windows desligado enquanto alguém assiste");
            HdrOff(Some((adapter, output)))
        } else {
            let _ = std::fs::remove_file(hdr_mark());
            eprintln!("HDR: o Windows não desligou; seguindo com a conversão");
            HdrOff(None)
        }
    }
}

impl Drop for HdrOff {
    fn drop(&mut self) {
        if let Some((adapter, output)) = self.0 {
            if crate::sys::hdr::set_hdr(adapter, output, true) {
                eprintln!("HDR do Windows ligado de novo");
            }
            let _ = std::fs::remove_file(hdr_mark());
        }
    }
}

/// Opção GPU em desempenho máximo: enquanto alguém assiste, a NVML trava o clock da placa NVIDIA
/// com o mínimo no clock base e o máximo livre (como `nvidia-smi -lgc tdp,unlimited`). Vale na hora,
/// sem reabrir nada; precisa do serviço ou do Prisma como administrador. A marca guarda o processo,
/// para destravar mesmo se o Prisma cair (veja [`restore_hdr`]).
// shortcut: só a placa 0 da NVML, e um travamento feito antes por fora é desfeito no fim; ler o de antes se alguém usar os dois.
struct GpuMax(bool);

fn gpu_lock_mark() -> std::path::PathBuf {
    super::dir().join("gpu_travada.txt")
}

/// O que dizer quando a NVML recusa (códigos de `nvmlReturn_t`).
fn gpu_lock_note(code: i32) -> &'static str {
    match code {
        3 => "esta placa ou este driver não deixa travar o clock",
        4 => "sem permissão: precisa do serviço ou do Prisma como administrador",
        _ => "erro da NVML",
    }
}

/// Destrava o clock da placa 0.
fn gpu_unlock() -> bool {
    crate::sys::nvml::Nvml::load().is_some_and(|nv| nv.reset_locked_clocks() == 0)
}

/// A marca é de outro processo que ainda está compartilhando (o serviço, com o Prisma aberto ao lado)?
fn held_by_other(pid: Option<u32>) -> bool {
    pid.is_some_and(|p| p != std::process::id() && crate::sys::procs::is_alive(p))
}

/// Marca da 1.9.3, que punha o perfil do .exe em "Preferir desempenho máximo" no DRS: o .exe, o
/// valor de antes (`-`: nada gravado) e o processo, um por linha. Só para devolver depois de uma queda.
fn gpu_mark_parse(s: &str) -> Option<(String, Option<u32>, Option<u32>)> {
    let mut l = s.lines().map(str::trim);
    let exe = l.next().filter(|e| !e.is_empty())?;
    let old = match l.next()? {
        "-" => None,
        v => Some(v.parse().ok()?),
    };
    Some((exe.to_string(), old, l.next().and_then(|p| p.parse().ok())))
}

/// Devolve o perfil do .exe ao valor de antes. Sem valor de antes, tira também o perfil vazio que o Prisma criou.
fn gpu_back(exe: &str, old: Option<u32>) -> bool {
    let ok = crate::sys::drs::apply(exe, &[(crate::sys::drs::PREFERRED_PSTATE, old)]);
    if ok && old.is_none() {
        crate::sys::drs::forget(exe);
    }
    ok
}

impl GpuMax {
    fn new() -> Self {
        if !Opts::load().gpu_max {
            return GpuMax(false);
        }
        let Some(nv) = crate::sys::nvml::Nvml::load() else {
            eprintln!("GPU em desempenho máximo: sem NVML (placa NVIDIA), a opção não faz nada aqui");
            return GpuMax(false);
        };
        if std::fs::create_dir_all(super::dir()).and_then(|_| std::fs::write(gpu_lock_mark(), format!("{}\n", std::process::id()))).is_err() {
            eprintln!("GPU em desempenho máximo: não deu para gravar a marca de volta; o clock fica livre");
            return GpuMax(false);
        }
        match nv.lock_base_clock() {
            0 => {
                eprintln!("GPU em desempenho máximo: clock da {} travado no mínimo do clock base enquanto alguém assiste", nv.name);
                GpuMax(true)
            }
            code => {
                let _ = std::fs::remove_file(gpu_lock_mark());
                eprintln!("GPU em desempenho máximo: a NVML recusou travar o clock (código {code}: {})", gpu_lock_note(code));
                GpuMax(false)
            }
        }
    }
}

impl Drop for GpuMax {
    fn drop(&mut self) {
        if self.0 {
            if gpu_unlock() {
                eprintln!("GPU em desempenho máximo: clock destravado");
                let _ = std::fs::remove_file(gpu_lock_mark());
            } else {
                eprintln!("GPU em desempenho máximo: a NVML não destravou o clock; tenta de novo na próxima abertura");
            }
        }
    }
}

/// Quem está conectado agora, para a página Compartilhar tela (vazio: ninguém). Uma pessoa por
/// linha: chave (id do aparelho), permissões, caminho e nome, separados por tabulação.
pub fn room_path() -> std::path::PathBuf {
    super::dir().join("sala.txt")
}

/// Uma pessoa da sala, lida pela página.
#[derive(Clone, PartialEq, Debug)]
pub struct Present {
    pub key: String,
    pub perms: u8,
    pub route: String,
    pub name: String,
}

pub fn room() -> Vec<Present> {
    parse_room(&std::fs::read_to_string(room_path()).unwrap_or_default())
}

fn parse_room(s: &str) -> Vec<Present> {
    s.lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let f: Vec<&str> = l.splitn(4, '\t').collect();
            match f[..] {
                [key, perms, route, name] => Present { key: key.into(), perms: perms.parse().unwrap_or(0), route: route.into(), name: name.into() },
                // Formato antigo: só o nome.
                _ => Present { key: l.into(), perms: 0, route: String::new(), name: l.into() },
            }
        })
        .collect()
}

/// Pedidos da página para a sala aberta: `desconectar <chave>` e `acesso <chave> <permissões>`.
pub fn commands_path() -> std::path::PathBuf {
    super::dir().join("comandos.txt")
}

/// A página pede: o host lê o arquivo em até um segundo.
#[cfg_attr(not(feature = "stream"), allow(dead_code))]
pub fn ask_room(line: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(commands_path())?;
    writeln!(f, "{line}")
}

/// Lê e apaga os pedidos (renomeia antes, para não perder um que chegue no meio).
fn take_commands() -> Vec<(String, String, Option<u8>)> {
    let path = commands_path();
    if !path.exists() {
        return Vec::new();
    }
    let read = path.with_extension("lendo");
    if std::fs::rename(&path, &read).is_err() {
        return Vec::new();
    }
    let s = std::fs::read_to_string(&read).unwrap_or_default();
    let _ = std::fs::remove_file(&read);
    s.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            let (cmd, key) = (w.next()?, w.next()?);
            Some((cmd.to_string(), key.to_string(), w.next().and_then(|p| p.parse().ok())))
        })
        .collect()
}

fn write_room(names: &str) {
    let _ = std::fs::write(room_path(), names);
    if names.is_empty() {
        let _ = std::fs::write(pads_path(), "");
    }
}

/// Os controles virtuais ligados agora, para a aba Controles: pessoa, controle dela, jogador, parado.
pub fn pads_path() -> std::path::PathBuf {
    super::dir().join("controles.txt")
}

/// Tecla de emergência do host: Ctrl+Alt+Shift+Q desconecta todo mundo na hora, fechando o
/// processo que compartilha (com o serviço, ele abre outro, sem ninguém conectado).
#[cfg_attr(not(feature = "stream"), allow(dead_code))]
pub fn emergency_key() {
    std::thread::Builder::new()
        .name("compartilhar-emergencia".into())
        .spawn(|| unsafe {
            use windows::Win32::UI::Input::KeyboardAndMouse::*;
            let (mods, vk) = Opts::load().key_panic;
            let label = crate::config::Hotkey::new(mods, vk).label();
            if RegisterHotKey(None, 1, HOT_KEY_MODIFIERS(mods) | MOD_NOREPEAT, vk).is_err() {
                eprintln!("a tecla de emergência ({label}) já está em uso por outro programa");
                return;
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if msg.message == WM_HOTKEY {
                    eprintln!("tecla de emergência: todo mundo desconectado");
                    super::notice("Tecla de emergência: todo mundo desconectado", "");
                    crate::config::log("compartilhar: tecla de emergência apertada, todo mundo desconectado");
                    write_room("");
                    restore_hdr();
                    std::process::exit(0);
                }
            }
        })
        .ok();
}

/// No início do Prisma: se uma sessão caiu com o HDR desligado, liga de volta. Pula as marcas de
/// outro processo vivo (a sessão ainda está aberta); as deste processo voltam (tecla de emergência).
pub fn restore_hdr() {
    crate::sys::vdd::restore();
    // A GPU em desempenho máximo também volta; sem conseguir (sem permissão), a marca fica para o serviço.
    for path in [super::user_dir().join("gpu_travada.txt"), super::service_dir().join("gpu_travada.txt")] {
        let Ok(s) = std::fs::read_to_string(&path) else { continue };
        if !held_by_other(s.trim().parse().ok()) && gpu_unlock() {
            crate::config::log("compartilhar: clock da GPU destravado depois de uma sessão interrompida");
            let _ = std::fs::remove_file(&path);
        }
    }
    // A marca da 1.9.3 (perfil do DRS).
    for path in [super::user_dir().join("gpu_maximo.txt"), super::service_dir().join("gpu_maximo.txt")] {
        let Some((exe, old, pid)) = std::fs::read_to_string(&path).ok().and_then(|s| gpu_mark_parse(&s)) else { continue };
        if !held_by_other(pid) && gpu_back(&exe, old) {
            crate::config::log("compartilhar: GPU de volta ao gerenciamento de energia de antes, depois de uma sessão interrompida");
            let _ = std::fs::remove_file(&path);
        }
    }
    for path in [super::user_dir().join("hdr_desligado.txt"), super::service_dir().join("hdr_desligado.txt")] {
        let Ok(s) = std::fs::read_to_string(&path) else { continue };
        let v: Vec<i64> = s.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        if held_by_other(v.get(3).map(|&p| p as u32)) {
            continue;
        }
        if let [low, high, output, ..] = v[..] {
            let adapter = windows::Win32::Foundation::LUID { LowPart: low as u32, HighPart: high as i32 };
            if crate::sys::hdr::set_hdr(adapter, output as u32, true) {
                crate::config::log("compartilhar: HDR ligado de volta depois de uma sessão interrompida");
            }
        }
        let _ = std::fs::remove_file(&path);
    }
}

/// Uma sala, do primeiro que chega até o último sair. Quem chega pelo `joins` entra junto.
/// Devolve o endereço de alguém novo que chegou pelo UDP, se for o caso.
pub(super) fn serve(d: &D3d, first: Joiner, joins: Option<&mpsc::Receiver<Joiner>>) -> Option<(Hello, SocketAddr)> {
    *LEAD.lock().unwrap() = first.id.clone();
    let _hdr = HdrOff::new(&first.hello);
    let _gpu = GpuMax::new();
    let h = &first.hello;
    let _virtual = crate::sys::vdd::Virtual::for_monitor(room_opts().monitor, h.w, h.h, h.hz, h.flags & SEES_HDR != 0);
    let mut room = Room { guests: vec![Guest::new(first)], bridge: None, bridge_err: None, anti_cheat: false, checked: Instant::now() - Duration::from_secs(10), clip: None, clip_id: 0, frame_id: 0, wave_failed: false };
    let g = &room.guests[0];
    eprintln!("chegou «{}»: tela {}x{} a {} Hz, {} ({})", g.name, g.hello.w, g.hello.h, g.hello.hz, perms_text(g.perms), g.link.route());
    loop {
        match unsafe { session(d, &mut room, joins) } {
            Ok(End::Gone) => {
                write_room("");
                eprintln!("todo mundo saiu");
                return None;
            }
            Ok(End::Again) => {}
            Ok(End::Stranger(h, from)) => return Some((h, from)),
            Err(e) => {
                eprintln!("a sessão parou: {e:?}");
                return None;
            }
        }
    }
}

#[cfg_attr(not(debug_assertions), allow(dead_code))]
pub fn run(port: u16, control: bool) {
    let sock = match UdpSocket::bind(("0.0.0.0", port)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("não deu para abrir a porta {port}: {e}");
            return;
        }
    };
    let Some(d) = prepare() else { return };
    let mode = if control { "com mouse, teclado e controles de quem assiste" } else { "só para ver" };
    eprintln!("Prisma compartilhando ({mode}): esperando quem assiste na porta UDP {port}");
    let mut buf = vec![0u8; 2048];
    let mut next: Option<(Hello, SocketAddr)> = None;
    loop {
        let (hello, from) = match next.take() {
            Some(h) => h,
            None => {
                let _ = sock.set_nonblocking(false);
                let _ = sock.set_read_timeout(None);
                let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
                match read(&buf[..n]).as_ref().and_then(Hello::from_msg) {
                    Some(h) => (h, from),
                    None => continue,
                }
            }
        };
        let Ok(s) = sock.try_clone() else { continue };
        let j = Joiner { link: Arc::new(Udp::new(s, from)), hello, perms: if control { TOTAL } else { SOUND }, name: from.to_string(), id: String::new() };
        next = serve(&d, j, None);
    }
}

/// Programas de chamada: o som deles sai do que vai para quem assiste, se a opção estiver ligada.
const CALL_APPS: [&str; 10] = ["discord.exe", "ms-teams.exe", "teams.exe", "zoom.exe", "whatsapp.exe", "whatsapp.root.exe", "skype.exe", "telegram.exe", "slack.exe", "signal.exe"];

pub fn is_call_app(exe: &str) -> bool {
    CALL_APPS.iter().any(|c| c.eq_ignore_ascii_case(exe))
}

/// Processos pelo nome do exe, só os de cima da árvore (a captura por processo leva a árvore toda).
fn top_pids(names: &[&str]) -> Vec<(u32, String)> {
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    let mut all: Vec<(u32, u32, String)> = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return Vec::new() };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                all.push((e.th32ProcessID, e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..n]).to_lowercase()));
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    let wanted = |s: &str| names.iter().any(|n| n.eq_ignore_ascii_case(s));
    all.iter()
        .filter(|(_, parent, name)| wanted(name) && !all.iter().any(|(p, _, pn)| p == parent && pn == name))
        .map(|(pid, _, name)| (*pid, name.clone()))
        .collect()
}

/// De onde vem o som: o PC todo, só um programa, o PC sem um programa, ou uma saída escolhida.
#[derive(Clone, PartialEq, Debug)]
enum SoundFrom {
    All,
    Only(u32),
    Without(u32),
    /// A saída escolhida nas opções (id do Windows): o que toca nela, sem tirar programa.
    Device(String),
}

/// O aparelho que conectou primeiro: as opções próprias dele valem para a sala toda.
static LEAD: Mutex<String> = Mutex::new(String::new());

/// As opções que valem agora: as do aparelho que lidera a sala, ou as de todos.
fn room_opts() -> Opts {
    Opts::load_for(&LEAD.lock().unwrap())
}

/// O nome de um aparelho de som pelo id, para o registro.
fn device_name(id: &str, capture: bool) -> String {
    crate::sys::audio::devices(capture).into_iter().find(|d| d.id == id).map_or_else(|| id.to_string(), |d| d.name)
}

/// O programa de chamada que sai do som. O Windows só deixa tirar um programa por captura: vale o
/// que está tocando agora (o Discord numa chamada, e não o Telegram aberto e calado). Se nenhum
/// toca, fica o que já estava fora; senão, o primeiro da lista (o Discord antes dos outros).
fn call_to_exclude(keep: Option<u32>) -> Option<(u32, String)> {
    let mut open = top_pids(&CALL_APPS);
    open.sort_by_key(|(_, n)| CALL_APPS.iter().position(|c| c == n).unwrap_or(usize::MAX));
    if open.len() < 2 {
        return open.pop();
    }
    let playing: Vec<String> = crate::sys::audio::sessions().into_iter().filter(|s| s.active).map(|s| s.exe.to_lowercase()).collect();
    let loud: Vec<&(u32, String)> = open.iter().filter(|(_, n)| playing.contains(n)).collect();
    if let Some(k) = keep.and_then(|k| open.iter().find(|(p, _)| *p == k)) {
        if loud.is_empty() || loud.iter().any(|(p, _)| *p == k.0) {
            return Some(k.clone());
        }
    }
    loud.first().map(|x| (*x).clone()).or_else(|| open.first().cloned())
}

/// A escolha do som e o texto para o registro. `keep`: o programa que já está fora do som.
fn sound_from(opts: &Opts, viewer_here: Option<u32>, keep: Option<u32>) -> (SoundFrom, String) {
    // Uma saída escolhida: o Windows só tira programa do som da saída padrão.
    if !opts.sound_out.is_empty() {
        if crate::sys::audio::devices(false).iter().any(|d| d.id == opts.sound_out) {
            return (SoundFrom::Device(opts.sound_out.clone()), format!("da saída «{}»", device_name(&opts.sound_out, false)));
        }
        eprintln!("a saída de som escolhida não está ligada: vai a padrão");
    }
    // Tirar o programa de chamada vale mais que escolher ele como o som que vai.
    let mut note = String::new();
    if opts.no_calls && is_call_app(&opts.sound_app) {
        note = format!(" ({} é programa de chamada e a opção tira ele do som)", opts.sound_app);
    } else if !opts.sound_app.is_empty() {
        match top_pids(&[opts.sound_app.as_str()]).first() {
            Some((pid, _)) => return (SoundFrom::Only(*pid), format!("só de {}", opts.sound_app)),
            None => note = format!(" ({} não está aberto)", opts.sound_app),
        }
    }
    // Quem assiste no mesmo PC: o som da janela dele voltaria em eco.
    if let Some(pid) = viewer_here {
        return (SoundFrom::Without(pid), format!("sem a janela que assiste neste PC{note}"));
    }
    if opts.no_calls {
        if let Some((pid, name)) = call_to_exclude(keep) {
            return (SoundFrom::Without(pid), format!("sem {name} (programa de chamada){note}"));
        }
    }
    (SoundFrom::All, format!("o PC todo{note}"))
}

/// Som do PC em blocos de 5 ms, para quem tem a permissão de som. A origem segue as opções e os
/// programas abertos durante a transmissão: outra linha decide a cada segundo, e esta só troca a
/// captura quando a decisão muda (procurar processos e sessões de som atrasaria os blocos).
/// Quem tem `true` ao lado recebe o som em Opus.
pub(super) fn audio(links: Arc<Mutex<Vec<(Arc<dyn Link>, bool)>>>, viewer_here: Option<u32>, stop: Arc<AtomicBool>) {
    // A origem do som, e a entrada que vai junto (vazio: nenhuma).
    type Choice = (SoundFrom, String, String);
    let choice: Arc<Mutex<Option<Choice>>> = Arc::new(Mutex::new(None));
    let chooser = {
        let (choice, stop) = (choice.clone(), stop.clone());
        std::thread::Builder::new().name("compartilhar-som-origem".into()).spawn(move || {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            let mut keep = None;
            while !stop.load(Ordering::Relaxed) {
                let opts = room_opts();
                let (from, text) = sound_from(&opts, viewer_here, keep);
                keep = match from {
                    SoundFrom::Without(p) => Some(p),
                    _ => None,
                };
                *choice.lock().unwrap() = Some((from, text, opts.sound_in.clone()));
                for _ in 0..10 {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        })
    };
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let mut src: Option<sound::Source> = None;
        let mut from: Option<SoundFrom> = None;
        let mut mic: Option<sound::Source> = None;
        let mut mic_from: Option<String> = None;
        let (mut a, mut m) = (Vec::<f32>::new(), Vec::<f32>::new());
        let mut pcm: Vec<f32> = Vec::new();
        let mut block = Vec::with_capacity(AUDIO_BLOCK * 4);
        let mut samples = Vec::with_capacity(AUDIO_BLOCK * 2);
        let mut enc = super::opus::Encoder::default();
        let mut out = Vec::with_capacity(MTU);
        let mut seq = 0u32;
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(2));
            if let Some((want, text, want_mic)) = choice.lock().unwrap().take() {
                if from.as_ref() != Some(&want) {
                    if let Some(s) = src.take() {
                        let _ = s.client.Stop();
                    }
                    a.clear();
                    let s = match &want {
                        SoundFrom::Only(pid) => sound::open_process_mode(*pid, false),
                        SoundFrom::Without(pid) => sound::open_process_mode(*pid, true),
                        SoundFrom::All => sound::open(eRender, true),
                        SoundFrom::Device(id) => sound::open_on(id, eRender, true),
                    };
                    from = Some(want);
                    match s {
                        Ok(s) => {
                            eprintln!("som: {text}");
                            src = Some(s);
                        }
                        Err(e) => eprintln!("sem som ({text}): {e:?}"),
                    }
                }
                if mic_from.as_ref() != Some(&want_mic) {
                    if let Some(s) = mic.take() {
                        let _ = s.client.Stop();
                    }
                    m.clear();
                    if !want_mic.is_empty() {
                        let id = if want_mic == super::MIC_DEFAULT { "" } else { want_mic.as_str() };
                        let name = if id.is_empty() { "a entrada padrão".to_string() } else { format!("«{}»", device_name(id, true)) };
                        match sound::open_on(id, windows::Win32::Media::Audio::eCapture, false) {
                            Ok(s) => {
                                eprintln!("entrada junto com o som: {name}");
                                mic = Some(s);
                            }
                            Err(e) => eprintln!("a entrada {name} não abriu: {e:?}"),
                        }
                    }
                    mic_from = Some(want_mic);
                }
            }
            if let Some(s) = &src {
                sound::read(s, &mut a);
            }
            match &mic {
                Some(s) => {
                    sound::read(s, &mut m);
                    mix(&mut a, &mut m, &mut pcm);
                }
                None => pcm.append(&mut a),
            }
            while pcm.len() >= AUDIO_BLOCK * 2 {
                samples.clear();
                samples.extend(pcm.drain(..AUDIO_BLOCK * 2).map(|v| (v.clamp(-1.0, 1.0) * 32767.0) as i16));
                let ls = links.lock().unwrap();
                // Sem Opus (o codificador não abriu), todo mundo recebe PCM.
                let coded = if ls.iter().any(|l| l.1) { enc.encode(&samples) } else { None };
                if let Some(c) = &coded {
                    write(&mut out, &Msg::Opus { seq, pts: now(), data: c });
                    ls.iter().filter(|l| l.1).for_each(|l| l.0.send(&out));
                }
                if ls.iter().any(|l| !l.1 || coded.is_none()) {
                    block.clear();
                    samples.iter().for_each(|v| block.extend_from_slice(&v.to_le_bytes()));
                    write(&mut out, &Msg::Audio { seq, pts: now(), data: &block });
                    ls.iter().filter(|l| !l.1 || coded.is_none()).for_each(|l| l.0.send(&out));
                }
                drop(ls);
                seq = seq.wrapping_add(1);
            }
        }
        if let Some(s) = src {
            let _ = s.client.Stop();
        }
    }
    if let Ok(t) = chooser {
        let _ = t.join();
    }
}

/// Folga entre o som e a entrada antes de uma seguir sozinha: 40 ms em estéreo a 48 kHz. Sem
/// nada tocando, o Windows não entrega blocos de som, e a voz não pode esperar por eles.
const MIX_SLACK: usize = 48 * 40 * 2;

/// Junta o som (`a`) e a entrada (`m`) em `out`, amostra por amostra, no que os dois têm. Quando
/// um passa da folga sem o outro, ele segue sozinho.
fn mix(a: &mut Vec<f32>, m: &mut Vec<f32>, out: &mut Vec<f32>) {
    let n = a.len().min(m.len());
    out.extend(a.drain(..n).zip(m.drain(..n)).map(|(x, y)| x + y));
    if a.len() > MIX_SLACK {
        out.append(a);
    }
    if m.len() > MIX_SLACK {
        out.append(m);
    }
}

/// Hora (ms desde o início do processo) do último mouse ou teclado de verdade deste PC.
static HOST_INPUT_MS: AtomicI64 = AtomicI64::new(i64::MIN / 2);
/// Quem está no mouse e no teclado agora, quando é um por vez: (injetor, ms).
static INPUT_OWNER: Mutex<Option<(u64, i64)>> = Mutex::new(None);
static INJECTORS: AtomicU64 = AtomicU64::new(1);
/// Os visitantes param por este tempo depois do mouse ou teclado daqui.
const HOST_FIRST_MS: i64 = 1000;
/// Um visitante parado por este tempo libera a vez.
const OWNER_IDLE_MS: i64 = 2000;

pub(super) fn ms_now() -> i64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as i64
}

/// Hora (ms desde o início do processo) do último movimento do mouse de verdade deste PC.
pub(super) static HOST_MOUSE_MS: AtomicI64 = AtomicI64::new(i64::MIN / 2);

/// Vigia o mouse e o teclado de verdade (o Windows marca o que foi injetado), numa linha própria.
pub(super) fn watch_host_input() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    ms_now();
    unsafe extern "system" fn kb(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
        if code >= 0 && (*(l.0 as *const KBDLLHOOKSTRUCT)).flags.0 & LLKHF_INJECTED.0 == 0 {
            HOST_INPUT_MS.store(ms_now(), Ordering::Relaxed);
        }
        CallNextHookEx(None, code, w, l)
    }
    unsafe extern "system" fn mouse(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
        if code >= 0 && (*(l.0 as *const MSLLHOOKSTRUCT)).flags & LLMHF_INJECTED == 0 {
            HOST_INPUT_MS.store(ms_now(), Ordering::Relaxed);
            if w.0 as u32 == WM_MOUSEMOVE {
                HOST_MOUSE_MS.store(ms_now(), Ordering::Relaxed);
            }
        }
        CallNextHookEx(None, code, w, l)
    }
    let _ = std::thread::Builder::new().name("compartilhar-host-primeiro".into()).spawn(|| unsafe {
        let a = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb), None, 0);
        let b = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse), None, 0);
        if a.is_err() || b.is_err() {
            eprintln!("não deu para vigiar o mouse e o teclado daqui: o host não tem prioridade");
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
    });
}

/// Aplica no PC o mouse e o teclado de quem assiste, só o que as permissões dela deixam.
pub(super) struct Injector {
    /// Monitor transmitido e área de todos os monitores, em pixels.
    mon: RECT,
    virt: RECT,
    perms: u8,
    next: Option<u32>,
    keys: HashSet<(u16, bool)>,
    buttons: [bool; 5],
    id: u64,
    one_at_a_time: bool,
    host_first: bool,
    /// Só esta janela (e o processo dela) recebe: a imagem é ela, e não o monitor.
    win: Option<(HWND, u32)>,
    /// Onde o mouse de quem assiste está, em pixels da área de trabalho.
    at: POINT,
}

/// Da imagem (0 a 65535) para a área transmitida em pixels, e daí para a escala de 0 a 65535 da
/// área de todos os monitores, que o `SendInput` usa.
fn to_desktop(x: u16, y: u16, area: RECT, virt: RECT) -> (POINT, i32, i32) {
    let mw = (area.right - area.left).max(1) as i64;
    let mh = (area.bottom - area.top).max(1) as i64;
    let px = area.left as i64 + x as i64 * (mw - 1) / 65535;
    let py = area.top as i64 + y as i64 * (mh - 1) / 65535;
    let vw = (virt.right - virt.left).max(2) as i64;
    let vh = (virt.bottom - virt.top).max(2) as i64;
    let dx = ((px - virt.left as i64) * 65535 / (vw - 1)) as i32;
    let dy = ((py - virt.top as i64) * 65535 / (vh - 1)) as i32;
    (POINT { x: px as i32, y: py as i32 }, dx, dy)
}

/// A área visível da janela, a mesma que o WGC captura (sem a borda invisível do Windows).
fn window_rect(h: HWND) -> RECT {
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
    let mut r = RECT::default();
    unsafe {
        if DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as *mut _, std::mem::size_of::<RECT>() as u32).is_err() {
            let _ = GetWindowRect(h, &mut r);
        }
    }
    r
}

/// Traz a janela para frente (e de volta do minimizado). Falso se o Windows não deixou: aí a
/// entrada não vai, para não cair noutro programa.
fn bring_front(h: HWND, pid: u32) -> bool {
    let pid_of = crate::sys::procs::window_pid;
    unsafe {
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        if pid_of(GetForegroundWindow()) == pid {
            return true;
        }
        if !SetForegroundWindow(h).as_bool() {
            // O Windows só passa o primeiro plano para quem recebeu a última entrada: um Alt
            // apertado e solto por aqui conta como essa entrada.
            let alt = |up: bool| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VK_MENU, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, time: 0, dwExtraInfo: 0 } },
            };
            SendInput(&[alt(false), alt(true)], std::mem::size_of::<INPUT>() as i32);
            let _ = SetForegroundWindow(h);
        }
        pid_of(GetForegroundWindow()) == pid
    }
}

impl Injector {
    /// Só a janela `win` recebe o mouse e o teclado, com as posições na área dela.
    pub(super) fn with_window(mut self, win: Option<HWND>) -> Self {
        self.win = win.map(|h| (h, crate::sys::procs::window_pid(h)));
        self
    }

    pub(super) fn new(mon: HMONITOR, perms: u8) -> Self {
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        unsafe {
            let _ = GetMonitorInfoW(mon, &mut mi);
        }
        let virt = unsafe {
            let (x, y) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
            RECT { left: x, top: y, right: x + GetSystemMetrics(SM_CXVIRTUALSCREEN), bottom: y + GetSystemMetrics(SM_CYVIRTUALSCREEN) }
        };
        let id = INJECTORS.fetch_add(1, Ordering::Relaxed);
        let mut me = Self { mon: mi.rcMonitor, virt, perms, next: None, keys: HashSet::new(), buttons: [false; 5], id, one_at_a_time: false, host_first: false, win: None, at: POINT::default() };
        me.rules(&Opts::load());
        me
    }

    /// As regras da página para o mouse e o teclado (valem na hora quando mudam).
    fn rules(&mut self, o: &Opts) {
        if o.host_first {
            watch_host_input();
        }
        self.one_at_a_time = o.one_at_a_time;
        self.host_first = o.host_first;
    }

    fn mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: i32) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
        }
    }

    fn key(scan: u16, extended: bool, down: bool) -> INPUT {
        let mut f = KEYEVENTF_SCANCODE;
        if extended {
            f |= KEYEVENTF_EXTENDEDKEY;
        }
        if !down {
            f |= KEYEVENTF_KEYUP;
        }
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: scan, dwFlags: f, time: 0, dwExtraInfo: 0 } },
        }
    }

    fn button(b: u8, down: bool) -> Option<INPUT> {
        let (f, data) = match (b, down) {
            (0, true) => (MOUSEEVENTF_LEFTDOWN, 0),
            (0, false) => (MOUSEEVENTF_LEFTUP, 0),
            (1, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
            (1, false) => (MOUSEEVENTF_RIGHTUP, 0),
            (2, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
            (2, false) => (MOUSEEVENTF_MIDDLEUP, 0),
            (3, true) => (MOUSEEVENTF_XDOWN, XBUTTON1 as i32),
            (3, false) => (MOUSEEVENTF_XUP, XBUTTON1 as i32),
            (4, true) => (MOUSEEVENTF_XDOWN, XBUTTON2 as i32),
            (4, false) => (MOUSEEVENTF_XUP, XBUTTON2 as i32),
            _ => return None,
        };
        Some(Self::mouse(f, 0, 0, data))
    }

    fn one(&mut self, e: Input) -> Option<INPUT> {
        let allowed = match e {
            Input::Away => return None,
            Input::Key { .. } | Input::Locks { .. } | Input::Mask => self.perms & KEYBOARD != 0,
            _ => self.perms & MOUSE != 0,
        };
        if !allowed {
            return None;
        }
        Some(match e {
            Input::Move { x, y } => {
                // A janela anda e muda de tamanho: a área dela é lida a cada movimento.
                let area = self.win.map_or(self.mon, |(h, _)| window_rect(h));
                let (at, dx, dy) = to_desktop(x, y, area, self.virt);
                self.at = at;
                Self::mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, dx, dy, 0)
            }
            Input::Button { button, down } => {
                let (inside, windowed) = (!down || self.inside(), self.win.is_some());
                let s = self.buttons.get_mut(button as usize)?;
                // Só a janela: clique em cima de outro programa não vai, e o soltar só vai se o
                // apertar foi.
                if !inside || (!down && !*s && windowed) {
                    return None;
                }
                *s = down;
                Self::button(button, down)?
            }
            Input::Wheel { .. } if !self.inside() => return None,
            Input::Wheel { delta, horizontal } => Self::mouse(if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL }, 0, 0, delta as i32),
            Input::Key { scan, extended, down } => {
                let changed = if down { self.keys.insert((scan, extended)) } else { self.keys.remove(&(scan, extended)) };
                // Para achar o Shift preso: o registro guarda cada Ctrl, Alt e Shift que desce e sobe.
                if let Some(name) = mod_name(scan, extended).filter(|_| changed) {
                    eprintln!("teclado: {name} {}", if down { "desce" } else { "sobe" });
                }
                Self::key(scan, extended, down)
            }
            Input::Away | Input::Locks { .. } | Input::Mask => return None,
        })
    }

    /// Toca uma tecla pelo código virtual (desce e sobe).
    fn tap(vk: VIRTUAL_KEY) -> [INPUT; 2] {
        // O Num Lock só troca com a marca de tecla estendida.
        let ext = if vk == VK_NUMLOCK { KEYEVENTF_EXTENDEDKEY } else { KEYBD_EVENT_FLAGS(0) };
        let k = |up: KEYBD_EVENT_FLAGS| INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: ext | up, time: 0, dwExtraInfo: 0 } } };
        [k(KEYBD_EVENT_FLAGS(0)), k(KEYEVENTF_KEYUP)]
    }

    /// O mouse está em cima da janela compartilhada (ou de um menu dela)? Sem janela, sempre.
    fn inside(&self) -> bool {
        let Some((_, pid)) = self.win else { return true };
        let under = unsafe { GetAncestor(WindowFromPoint(self.at), GA_ROOT) };
        crate::sys::procs::window_pid(under) == pid
    }

    /// Pode mexer agora? O host mexeu há pouco, ou outro visitante está com a vez.
    fn may_act(&mut self) -> bool {
        let now = ms_now();
        if self.host_first && now - HOST_INPUT_MS.load(Ordering::Relaxed) < HOST_FIRST_MS {
            return false;
        }
        if self.one_at_a_time {
            let mut owner = INPUT_OWNER.lock().unwrap();
            match *owner {
                Some((id, at)) if id != self.id && now - at < OWNER_IDLE_MS => return false,
                _ => *owner = Some((self.id, now)),
            }
        }
        true
    }

    /// Aplica só os eventos novos do pacote: os repetidos de pacotes anteriores são pulados.
    pub(super) fn apply(&mut self, first: u32, events: &[u8]) {
        let fresh = events.chunks(INPUT_SIZE).enumerate().any(|(i, _)| self.next.is_none_or(|n| (first.wrapping_add(i as u32).wrapping_sub(n) as i32) >= 0));
        if fresh && !self.may_act() {
            // Pula a vez, sem deixar nada apertado no host.
            self.release();
            let n = (events.len() / INPUT_SIZE) as u32;
            if n > 0 {
                self.next = Some(first.wrapping_add(n));
            }
            return;
        }
        // Só uma janela: ela vem para frente antes; se o Windows não deixar, nada vai, para o
        // teclado não cair noutro programa.
        if fresh && self.win.is_some_and(|(h, pid)| !bring_front(h, pid)) {
            // As subidas deste pacote também não vão: o que estava apertado sobe agora, senão
            // um Shift ficava preso até a pessoa sair.
            self.release();
            let n = (events.len() / INPUT_SIZE) as u32;
            if n > 0 {
                self.next = Some(first.wrapping_add(n));
            }
            return;
        }
        let mut list = Vec::new();
        for (i, chunk) in events.chunks(INPUT_SIZE).enumerate() {
            let seq = first.wrapping_add(i as u32);
            if self.next.is_some_and(|n| (seq.wrapping_sub(n) as i32) < 0) {
                continue;
            }
            if let Some(e) = Input::get(chunk) {
                if matches!(e, Input::Locks { .. } | Input::Mask) && self.perms & KEYBOARD != 0 {
                    // O mesmo GetKeyState do Console.CapsLock do .NET: vale fora da janela da frente.
                    let now = unsafe { (GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0, GetKeyState(VK_NUMLOCK.0 as i32) & 1 != 0) };
                    list.extend(keyboard_taps(e, now).into_iter().flat_map(Self::tap));
                } else if let Some(inp) = self.one(e) {
                    list.push(inp);
                }
            }
            self.next = Some(seq.wrapping_add(1));
        }
        if !list.is_empty() {
            unsafe {
                SendInput(&list, std::mem::size_of::<INPUT>() as i32);
            }
        }
    }

    /// Solta tudo que ficou apertado, para nenhuma tecla ficar presa quando quem assiste sai.
    pub(super) fn release(&mut self) {
        let mods: Vec<&str> = self.keys.iter().filter_map(|&(s, e)| mod_name(s, e)).collect();
        if !mods.is_empty() {
            eprintln!("teclado: solta tudo ({})", mods.join(", "));
        }
        let mut list: Vec<INPUT> = self.keys.drain().map(|(s, e)| Self::key(s, e, false)).collect();
        for (i, down) in self.buttons.iter_mut().enumerate() {
            if std::mem::take(down) {
                list.extend(Self::button(i as u8, false));
            }
        }
        if !list.is_empty() {
            unsafe {
                SendInput(&list, std::mem::size_of::<INPUT>() as i32);
            }
        }
    }
}

/// O nome de um Ctrl, Alt ou Shift pelo scan code, para o registro.
fn mod_name(scan: u16, extended: bool) -> Option<&'static str> {
    Some(match (scan, extended) {
        (0x2A, false) => "Shift esquerdo",
        (0x36, _) => "Shift direito",
        (0x1D, false) => "Ctrl esquerdo",
        (0x1D, true) => "Ctrl direito",
        (0x38, false) => "Alt esquerdo",
        (0x38, true) => "Alt direito",
        _ => return None,
    })
}

/// Teclas que o host toca por um pedido de teclado de quem assiste: as travas que estão diferentes
/// das dele (`now`: Caps e Num Lock daqui) ou, no atalho, uma sem uso (0xE8, como no AutoHotkey),
/// para o Windows não ver Alt+Shift nem Ctrl+Shift sozinhos ao soltar.
fn keyboard_taps(e: Input, now: (bool, bool)) -> Vec<VIRTUAL_KEY> {
    match e {
        Input::Locks { caps, num } => [(caps != now.0, VK_CAPITAL), (num != now.1, VK_NUMLOCK)].into_iter().filter(|x| x.0).map(|x| x.1).collect(),
        Input::Mask => vec![VIRTUAL_KEY(0xE8)],
        _ => Vec::new(),
    }
}

/// O programa em primeiro plano tem sinal de anti-cheat. `PRISMA_PADS_ANTICHEAT=1` ignora (por conta dela).
fn foreground_anti_cheat() -> bool {
    if std::env::var("PRISMA_PADS_ANTICHEAT").as_deref() == Ok("1") {
        return false;
    }
    let pid = crate::sys::procs::window_pid(unsafe { GetForegroundWindow() });
    pid != 0 && pid != std::process::id() && crate::sys::procs::image_path(pid).is_some_and(|p| crate::meter::anti_cheat(&p))
}

/// O que um controle de quem assiste mandou.
struct PadMsg {
    guest: usize,
    pad: u8,
    seq: u16,
    on: bool,
    state: PadState,
}

/// Liga, atualiza ou solta o controle virtual de um controle de quem assiste.
fn pad_msg(room: &mut Room, m: PadMsg, out: &mut Vec<u8>) {
    let i = m.pad as usize;
    if i >= PADS {
        return;
    }
    let refuse = |room: &mut Room, why: u8, out: &mut Vec<u8>| {
        let g = &mut room.guests[m.guest];
        g.pads[i] = None;
        if g.refused[i] != why {
            g.refused[i] = why;
            eprintln!("controle {} de «{}» recusado: {}", i + 1, g.name, pad_why(why));
            g.send(&Msg::PadInfo { pad: m.pad, player: 0, slot: 255, why }, out);
        }
    };
    if !m.on {
        let g = &mut room.guests[m.guest];
        if g.pads[i].take().is_some() {
            eprintln!("controle {} de «{}» saiu", i + 1, g.name);
        }
        g.refused[i] = PAD_OK;
        return;
    }
    if room.guests[m.guest].perms & PAD == 0 {
        return refuse(room, PAD_NO_PERMISSION, out);
    }
    if room.anti_cheat {
        return refuse(room, PAD_ANTI_CHEAT, out);
    }
    // No mesmo PC, quem assiste também enxerga os controles virtuais daqui: um deles voltando
    // viraria outro controle, sem fim.
    if room.guests[m.guest].link.same_pc() && i < 4 && room.virtual_slots() & (1 << i) != 0 {
        return refuse(room, PAD_LOOP, out);
    }
    if let Some(s) = &mut room.guests[m.guest].pads[i] {
        if (m.seq.wrapping_sub(s.seq) as i16) > 0 || s.idle {
            s.seq = m.seq;
            s.heard = Instant::now();
            s.idle = false;
            if !s.dev.update(&m.state) {
                room.guests[m.guest].pads[i] = None;
            }
        }
        return;
    }
    if room.pads() >= MAX_PADS {
        return refuse(room, PAD_FULL, out);
    }
    let dev = match room.bridge().and_then(|b| b.plug()) {
        Ok(d) => d,
        Err(e) => {
            if room.guests[m.guest].refused[i] != PAD_NO_DRIVER {
                eprintln!("controles: {e}");
            }
            return refuse(room, PAD_NO_DRIVER, out);
        }
    };
    let count = room.pads();
    let g = &mut room.guests[m.guest];
    let mut s = Slot { dev, seq: m.seq, heard: Instant::now(), idle: false, rumble: (0, 0), rumble_sent: Instant::now() };
    s.dev.update(&m.state);
    let player = s.player(count);
    eprintln!("controle {} de «{}» ligado: jogador {player}", i + 1, g.name);
    g.refused[i] = PAD_OK;
    g.send(&Msg::PadInfo { pad: m.pad, player, slot: s.dev.slot().unwrap_or(255), why: PAD_OK }, out);
    g.pads[i] = Some(s);
}

/// Vibração, controles parados e o aviso de vaga repetido de tempos em tempos.
fn pads_tick(room: &mut Room, out: &mut Vec<u8>, repeat_info: bool) {
    // Jogo com anti-cheat em primeiro plano: os controles virtuais saem do Windows.
    if room.checked.elapsed() > Duration::from_secs(2) {
        room.checked = Instant::now();
        let ac = room.guests.iter().any(|g| g.perms & PAD != 0) && foreground_anti_cheat();
        if ac != room.anti_cheat {
            room.anti_cheat = ac;
            if ac {
                eprintln!("controles: o jogo em primeiro plano tem anti-cheat; os controles virtuais saíram");
                for g in &mut room.guests {
                    for (i, p) in g.pads.iter_mut().enumerate() {
                        if p.take().is_some() {
                            g.refused[i] = PAD_ANTI_CHEAT;
                            write(out, &Msg::PadInfo { pad: i as u8, player: 0, slot: 255, why: PAD_ANTI_CHEAT });
                            g.link.send(out);
                        }
                    }
                }
            } else {
                eprintln!("controles: anti-cheat fora do primeiro plano; os controles voltam no próximo toque");
                for g in &mut room.guests {
                    g.refused = [PAD_OK; PADS];
                }
            }
        }
    }
    let mut n = 0;
    for g in &mut room.guests {
        for (i, p) in g.pads.iter_mut().enumerate() {
            let Some(s) = p else { continue };
            n += 1;
            if !s.idle && s.heard.elapsed() > PAD_STALE {
                s.idle = true;
                s.dev.update(&PadState::default());
            }
            let r = s.dev.rumble();
            if r != s.rumble || (r != (0, 0) && s.rumble_sent.elapsed() > Duration::from_millis(250)) {
                s.rumble = r;
                s.rumble_sent = Instant::now();
                write(out, &Msg::Rumble { pad: i as u8, low: r.0, high: r.1 });
                g.link.send(out);
            }
            if repeat_info {
                write(out, &Msg::PadInfo { pad: i as u8, player: s.player(n - 1), slot: s.dev.slot().unwrap_or(255), why: PAD_OK });
                g.link.send(out);
            }
        }
    }
    if repeat_info {
        let mut list = String::new();
        let mut k = 0;
        for g in &room.guests {
            for (i, p) in g.pads.iter().enumerate() {
                if let Some(s) = p {
                    list += &format!("{}	{}	{}	{}
", g.name, i + 1, s.player(k), s.idle as u8);
                    k += 1;
                }
            }
        }
        let _ = std::fs::write(pads_path(), list);
    }
}

/// O formato no registro: ", HDR10", ", 4:4:4", ", 10 bits" ou nada.
fn fmt_extra(vf: VideoFmt) -> &'static str {
    if vf.hdr10 {
        ", HDR10"
    } else if vf.yuv444 {
        ", 4:4:4"
    } else if vf.deep {
        ", 10 bits"
    } else {
        ""
    }
}

/// O vídeo que já está saindo serve para quem chegou? (O PyroWave também pede o caminho perto,
/// que quem chama confere pelo canal.)
fn fits(hello: &Hello, codec: VideoCodec, vf: VideoFmt) -> bool {
    let codec = match codec {
        VideoCodec::Wave => return hello.codecs & WAVE != 0 && (!vf.hdr10 || hello.flags & SEES_HDR != 0),
        VideoCodec::Std(c) => c,
    };
    if vf.deep && hello.flags & ASKS_10 == 0 {
        return false;
    }
    if vf.yuv444 {
        return hello.flags & DECODES_444 != 0;
    }
    hello.codecs & codec_bit(codec) != 0 && (!vf.hdr10 || hello.flags & SEES_HDR != 0)
}

/// Um quadro pronto para a rede, já em pedaços que cabem num pacote.
struct Ready {
    key: bool,
    pts: i64,
    chunks: super::wave::Chunks,
}

/// O codificador da sessão: o NVENC (depois da conversão para NV12, P010 ou AYUV) ou o PyroWave,
/// que lê a captura direto e faz cada quadro sozinho, sem quadro-chave.
enum Coder {
    Nv { enc: LiveEncoder, conv: Prep },
    Wave(super::wave::WaveEncoder),
}

impl Coder {
    fn ready(&self) -> bool {
        match self {
            Coder::Nv { enc, .. } => enc.ready(),
            Coder::Wave(_) => true,
        }
    }

    fn force_key(&self) {
        if let Coder::Nv { enc, .. } = self {
            enc.force_key();
        }
    }

    fn invalidate(&self, pts: i64) -> bool {
        match self {
            Coder::Nv { enc, .. } => enc.invalidate(pts),
            Coder::Wave(_) => true,
        }
    }

    /// Troca a taxa (só o NVENC e o Media Foundation; o PyroWave segue a dele).
    fn set_bitrate(&mut self, bits: u32) -> bool {
        match self {
            Coder::Nv { enc, .. } => enc.set_bitrate(bits),
            Coder::Wave(_) => false,
        }
    }

    fn recovers(&self) -> bool {
        match self {
            Coder::Nv { enc, .. } => enc.recovers(),
            Coder::Wave(_) => true,
        }
    }

    /// Codifica o quadro que está na textura do meio. O PyroWave devolve o quadro na hora, já em
    /// pacotes de até `space` bytes; o NVENC devolve depois, por [`Coder::pump`].
    unsafe fn feed(&mut self, ctx: &ID3D11DeviceContext, pts: i64, space: usize, out: &mut Vec<Ready>) -> Result<()> {
        match self {
            Coder::Nv { enc, conv } => {
                let t = conv.convert(ctx)?;
                enc.feed(&t, pts)
            }
            Coder::Wave(w) => {
                out.push(Ready { key: false, pts, chunks: w.encode(ctx, space)? });
                Ok(())
            }
        }
    }

    fn pump(&mut self, space: usize, pkts: &mut Vec<Pkt>, out: &mut Vec<Ready>) -> Result<()> {
        if let Coder::Nv { enc, .. } = self {
            enc.pump(pkts)?;
            out.extend(pkts.drain(..).map(|p| Ready { key: p.key, pts: p.pts, chunks: super::wave::Chunks::plain(&p.data, space) }));
        }
        Ok(())
    }
}

/// Intervalos guardados para o registro do ritmo: os primeiros 10 minutos a 60 por segundo.
const GAPS_KEPT: usize = 36_000;

/// Ritmo constante: quem lidera a sala (o primeiro a conectar) pediu. Quem assiste numa versão
/// sem o pedido fica no dinâmico.
fn asks_constant(guests: &[Guest]) -> bool {
    guests.first().is_some_and(|g| g.hello.flags & CONSTANT != 0)
}

fn pace_name(constant: bool) -> &'static str {
    if constant {
        "constante"
    } else {
        "dinâmico"
    }
}

/// Mudou só o pedido de ritmo: o vídeo segue sem recomeçar.
fn only_pace_changed(a: &Hello, b: &Hello) -> bool {
    (a.flags ^ b.flags) == CONSTANT && (a.w, a.h, a.hz, a.codecs, a.pid) == (b.w, b.h, b.hz, b.codecs, b.pid)
}

/// Hora de dar um quadro ao codificador, numa grade de horários (`due`) que anda um intervalo por
/// quadro. Um quadro novo vale até 0,1 intervalo antes da hora: a captura oscila em volta do vsync.
/// Contar do quadro anterior, e não da grade, deixava a tela de 119 Hz sair a ~64 de 60 por segundo
/// (o primeiro quadro depois de 0,9 intervalo); contar do fim do NVENC deixava a de 60 Hz sair a ~53.
/// No ritmo constante (`repeat`), sem quadro novo, o mesmo vai de novo na hora exata.
fn time_to_feed(now: i64, due: i64, interval: i64, fresh: bool, repeat: bool) -> bool {
    if fresh { now >= due - interval / 10 } else { repeat && now >= due }
}

/// A hora do quadro seguinte: um intervalo depois desta, mais 1/10 do atraso, para a grade seguir
/// devagar a fase da captura (sem isso, a 119,88 Hz ela escorrega e pula um quadro por segundo). Se o
/// vídeo ficou parado ou atrasou mais de um intervalo, 0,9 intervalo depois de agora (sem rajada).
fn next_due(now: i64, due: i64, interval: i64) -> i64 {
    let late = now - due;
    if late > interval { now + interval * 9 / 10 } else { due + interval + late.max(0) / 10 }
}

/// De onde vem a imagem: WGC (com o cursor do Windows) ou Desktop Duplication (no serviço, para
/// alcançar a tela segura; `PRISMA_CAPTURE=dda` também liga, para teste).
enum Cap {
    Wgc { item: GraphicsCaptureItem, pool: Option<(Direct3D11CaptureFramePool, GraphicsCaptureSession, HANDLE, i64)> },
    Dda(Box<super::screen::Dda>),
}

fn use_dda() -> bool {
    SERVICE.load(Ordering::Relaxed) || std::env::var("PRISMA_CAPTURE").as_deref() == Ok("dda")
}

/// Tamanho da tela principal, pela saída de vídeo (a Desktop Duplication não usa o WGC).
unsafe fn output_size(mon: HMONITOR) -> Result<(u32, u32)> {
    let Some((_, o)) = crate::capture::monitor_adapter(mon) else { return Err(Error::new(E_FAIL, "monitor sem placa de vídeo")) };
    let r = o.GetDesc()?.DesktopCoordinates;
    Ok(((r.right - r.left) as u32, (r.bottom - r.top) as u32))
}

/// Os monitores, na ordem do Windows.
pub fn monitors() -> Vec<HMONITOR> {
    unsafe extern "system" fn add(m: HMONITOR, _: HDC, _: *mut RECT, l: LPARAM) -> BOOL {
        (*(l.0 as *mut Vec<HMONITOR>)).push(m);
        TRUE
    }
    let mut list: Vec<HMONITOR> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(add), LPARAM(&mut list as *mut _ as isize));
    }
    list
}

/// O monitor das opções: 0 é o principal; 1, 2... a ordem do Windows.
pub(super) fn pick_monitor(n: u32) -> HMONITOR {
    let primary = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
    if n == 0 {
        return primary;
    }
    if n >= crate::sys::vdd::VIRTUAL {
        return crate::sys::vdd::monitor().unwrap_or(primary);
    }
    monitors().get(n as usize - 1).copied().unwrap_or(primary)
}

/// A janela das opções, se ainda existe e é do mesmo processo (o número de uma janela fechada
/// pode voltar noutra).
pub fn chosen_window(w: (u64, u32)) -> Option<HWND> {
    let h = HWND(w.0 as usize as *mut _);
    (w.0 != 0 && unsafe { IsWindow(Some(h)).as_bool() } && crate::sys::procs::window_pid(h) == w.1).then_some(h)
}

/// Uma janela que dá para compartilhar: hwnd, processo e título.
pub struct Shareable {
    pub hwnd: u64,
    pub pid: u32,
    pub title: String,
}

/// Fica na lista? As do próprio Prisma (página, host, avisos) saem.
fn shareable(exe: Option<&str>, own_exe: &str) -> bool {
    let name = |p: &str| p.rsplit(['\\', '/']).next().unwrap_or(p).to_ascii_lowercase();
    exe.is_none_or(|e| name(e) != name(own_exe))
}

/// As janelas abertas que dá para compartilhar, de cima para baixo: visíveis, com título, sem as
/// de ferramenta, as escondidas e as do Prisma (o filtro de `top_windows`, mais este).
pub fn share_windows() -> Vec<Shareable> {
    let own = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    crate::sys::procs::top_windows()
        .into_iter()
        .filter_map(|h| {
            let pid = crate::sys::procs::window_pid(h);
            let title = crate::sys::procs::window_title(h).trim().to_string();
            (pid != std::process::id() && !title.is_empty() && shareable(crate::sys::procs::image_path(pid).as_deref(), &own)).then(|| Shareable { hwnd: h.0 as usize as u64, pid, title })
        })
        .collect()
}

/// A janela compartilhada fechou: todo mundo sai e um aviso aparece aqui. Nunca troca sozinho
/// para a tela inteira; a página tira a escolha velha quando abre.
fn window_closed(room: &Room) -> End {
    eprintln!("a janela compartilhada fechou: o compartilhamento parou");
    super::notice("A janela compartilhada fechou: o compartilhamento parou", "");
    let mut out = Vec::new();
    write(&mut out, &Msg::Bye);
    for g in &room.guests {
        g.link.send(&out);
        g.link.send(&out);
    }
    End::Gone
}

unsafe fn session(d: &D3d, room: &mut Room, joins: Option<&mpsc::Receiver<Joiner>>) -> Result<End> {
    *LEAD.lock().unwrap() = room.guests[0].id.clone();
    let mut opts = room_opts();
    // Só uma janela, se foi escolhida. Fechada antes de começar: o compartilhamento para (nunca
    // cai sozinho para a tela inteira).
    let win = chosen_window(opts.window);
    if opts.window.0 != 0 && win.is_none() {
        return Ok(window_closed(room));
    }
    let mon = match win {
        Some(h) => {
            // Minimizada, a janela não tem imagem nem tamanho.
            if IsIconic(h).as_bool() {
                let _ = ShowWindow(h, SW_RESTORE);
            }
            MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST)
        }
        None => pick_monitor(opts.monitor),
    };
    let color = monitor_color(mon);
    // A placa que codifica: a das opções, ou a do monitor transmitido. Num notebook com duas, a
    // Desktop Duplication só lê a tela pela placa ligada a ela; a imagem passa para a outra.
    let screen_ad = crate::capture::monitor_adapter(mon).map(|(a, _)| a);
    let chosen = crate::capture::adapter_named(&opts.gpu);
    if !opts.gpu.is_empty() && chosen.is_none() {
        eprintln!("placa de vídeo «{}» não encontrada: vai a do monitor", opts.gpu);
    }
    let want = chosen.or_else(|| screen_ad.clone());
    let own = match &want {
        Some(a) if !crate::capture::same_adapter(a, &d.dev) => match device(Some(a)) {
            Ok(x) => Some(x),
            Err(e) => {
                eprintln!("não deu para usar a placa escolhida ({e:?}); vai a padrão");
                None
            }
        },
        _ => None,
    };
    let d: &D3d = own.as_ref().unwrap_or(d);
    // A janela vai sempre pelo WGC, mesmo no serviço (a Desktop Duplication só pega a tela toda).
    let mut cap = match win {
        Some(h) => {
            eprintln!("captura: só a janela «{}»", crate::sys::procs::window_title(h));
            Some(crate::capture::window_item(h)?)
        }
        None if use_dda() => None,
        None => Some(monitor_item(mon)?),
    };
    let (src_w, src_h) = match &cap {
        Some(item) => {
            let s = item.Size()?;
            (even(s.Width as u32), even(s.Height as u32))
        }
        None => {
            let (w, h) = output_size(mon)?;
            (even(w), even(h))
        }
    };
    // O tamanho e os Hz seguem quem chegou primeiro; os codecs e formatos, o que todos aceitam.
    let lead = room.guests[0].hello;
    let codecs = room.guests.iter().fold(0xFF, |m, g| m & g.hello.codecs);
    let flags = room.guests.iter().fold(0xFFFF, |m, g| m & g.hello.flags);
    // Automático: a altura da tela de quem assiste, sem passar da daqui nem do limite das opções.
    let want_h = if lead.h == 0 { src_h } else { lead.h };
    let want_h = if opts.res > 0 { want_h.min(opts.res) } else { want_h };
    let out_h = if want_h >= src_h { src_h } else { even(want_h.max(240)) };
    let out_w = even((src_w as u64 * out_h as u64 / src_h as u64) as u32);
    // Automático: o menor entre os Hz daqui e os de quem assiste (que também pode pedir menos pelo
    // menu da conexão). Um número fixo não passa dos Hz daqui nem do que quem assiste pediu.
    let guest_hz = room.guests.iter().map(|g| g.hello.hz).filter(|&z| z > 1).min().unwrap_or(60);
    let fps = if opts.fps > 0 { opts.fps.min(monitor_hz(mon)).min(guest_hz) } else { monitor_hz(mon).min(guest_hz) }.clamp(10, 240);

    let codec_opt = std::env::var("PRISMA_CODEC").unwrap_or_else(|_| opts.codec.clone());
    let wanted: Option<Codec> = match codec_opt.as_str() {
        "h264" => Some(Codec::H264),
        "hevc" => Some(Codec::Hevc),
        "av1" => Some(Codec::Av1),
        _ => None,
    };
    // Formato do vídeo, do melhor ao comum. HDR10 quando as telas estão em HDR; 4:4:4 quando quem
    // assiste pede, porque gasta mais rede; 10 bits com a tela SDR quando todos pedem. Se o NVENC
    // daqui não fizer, cai para o próximo.
    let want444 = flags & DECODES_444 != 0 && opts.allow444;
    let ten_bit = std::env::var("PRISMA_HDR10").map(|v| v != "0").unwrap_or(opts.ten_bit);
    let want_hdr = color.hdr && flags & SEES_HDR != 0 && ten_bit;
    let want_deep = !want_hdr && flags & ASKS_10 != 0 && ten_bit;
    let full = VideoFmt { yuv444: true, ..Default::default() };
    let ten = VideoFmt { hdr10: true, ..Default::default() };
    let deep = VideoFmt { deep: true, ..Default::default() };
    let mut tries: Vec<(Codec, VideoFmt)> = Vec::new();
    if want444 {
        tries.push((Codec::Hevc, full));
    }
    if want_hdr {
        tries.extend([(Codec::Av1, ten), (Codec::Hevc, ten)]);
    }
    if want_deep {
        tries.extend([(Codec::Av1, deep), (Codec::Hevc, deep)]);
    }
    tries.extend([Codec::Av1, Codec::Hevc, Codec::H264].map(|c| (c, VideoFmt::default())));
    let mbps = std::env::var("PRISMA_MBPS").ok().and_then(|v| v.parse::<u32>().ok()).or((opts.mbps > 0).then_some(opts.mbps));

    // PyroWave: só pedido, com todos numa versão que o tenha e todos na rede local ou na VPN (nunca pelo
    // relé). HDR10 nas mesmas condições do NVENC; 4:4:4 quando todos pedem e o host aceita, em
    // qualquer placa (o PyroWave não depende do NVDEC).
    let mut wave = None;
    if codec_opt == "pyrowave" && !room.wave_failed {
        let far: Vec<String> = room.guests.iter().filter(|g| !g.link.near()).map(|g| format!("«{}» ({})", g.name, g.link.route())).collect();
        if codecs & WAVE == 0 {
            eprintln!("PyroWave: quem assiste está numa versão sem ele; fica o automático");
        } else if !far.is_empty() {
            eprintln!("PyroWave só na rede local ou pela VPN, e {} não está: fica o automático", far.join(", "));
        } else {
            let vf = VideoFmt { hdr10: want_hdr, yuv444: flags & ASKS_444 != 0 && opts.allow444, deep: false };
            let bits = mbps.map(|m| m as u64 * 1_000_000).unwrap_or_else(|| super::wave::auto_bits(out_w, out_h, fps, vf));
            match super::wave::WaveEncoder::new(d, src_w, src_h, out_w, out_h, fps, bits, vf) {
                Ok(w) => {
                    eprintln!("vídeo: {}x{} a {} quadros/s, PyroWave {}, {:.0} Mbit/s ({})", out_w, out_h, fps, super::wave::fmt_name(vf), bits as f64 / 1e6, w.name());
                    wave = Some(w);
                }
                Err(e) => {
                    eprintln!("{}; fica o automático", e.message());
                    room.wave_failed = true;
                }
            }
        }
    }
    let mut made = None;
    // Teto da taxa do codificador aberto; o controle de taxa (`rate.rs`) só desce dele.
    let mut top_bits = 0u32;
    for (c, vf) in tries {
        if wave.is_some() {
            break;
        }
        // O 4:4:4 vai para o NVDEC de quem assiste, não para o Media Foundation da lista de codecs.
        if (!vf.yuv444 && codecs & codec_bit(c) == 0) || wanted.is_some_and(|w| w != c) {
            continue;
        }
        // Prioridade: menor atraso usa menos banda (quadros menores chegam antes); qualidade, mais.
        let factor: u64 = match opts.priority {
            0 => 3,
            2 => 6,
            _ => 4,
        };
        let bits = match mbps {
            Some(m) => m.min(super::NV_MAX_MBPS) * 1_000_000,
            None => (bitrate(out_w, out_h, fps, c) as u64 * factor / 2).min(150_000_000) as u32,
        };
        // O 4:4:4 leva cerca de metade a mais de dados pela mesma nitidez.
        let bits = if vf.yuv444 { (bits as u64 * 3 / 2).min(150_000_000) as u32 } else { bits };
        match LiveEncoder::open(&d.dev, c, out_w, out_h, fps, bits, vf) {
            Ok(e) => {
                let extra = fmt_extra(vf);
                eprintln!("vídeo: {}x{} a {} quadros/s, {}{}, {:.1} Mbit/s ({})", out_w, out_h, fps, codec_name(VideoCodec::Std(c)), extra, bits as f64 / 1e6, e.name());
                made = Some((e, c, vf));
                top_bits = bits;
                break;
            }
            Err(e) if vf != VideoFmt::default() => eprintln!("vídeo: {}{} recusado ({}); tentando o próximo", codec_name(VideoCodec::Std(c)), fmt_extra(vf).trim_start_matches(", "), e.message()),
            Err(_) => {}
        }
    }
    // A textura do meio recebe a captura: a do PyroWave é compartilhada com o Vulkan dele.
    // HDR10 sai em PQ de 10 bits; senão a tela em HDR vira SDR antes de codificar.
    let (mut enc, mid, codec, vf) = match (wave, made) {
        (Some(w), _) => {
            let (mid, vf) = (w.input().clone(), w.vf);
            (Coder::Wave(w), mid, VideoCodec::Wave, vf)
        }
        (None, Some((e, c, vf))) => {
            let mid = bgra_texture(&d.dev, src_w, src_h, if vf.hdr10 { DXGI_FORMAT_R10G10B10A2_UNORM } else { DXGI_FORMAT_B8G8R8A8_UNORM })?;
            let out_fmt = if vf.yuv444 {
                ConvOut::Ayuv
            } else if vf.hdr10 {
                ConvOut::P010
            } else if vf.deep {
                ConvOut::P010Sdr
            } else {
                ConvOut::Nv12
            };
            let conv = Prep::new(&d.dev, &d.ctx, &mid, src_w, src_h, out_w, out_h, fps, out_fmt)?;
            (Coder::Nv { enc: e, conv }, mid, VideoCodec::Std(c), vf)
        }
        (None, None) => return Err(Error::new(E_FAIL, "nenhum codec em comum com quem assiste")),
    };
    // Na Desktop Duplication, a conversão de cor e o cursor ficam com ela.
    let tone = if color.hdr && cap.is_some() { Some(ToneMap::new(&d.dev, &mid, vf.hdr10, color.white_nits)?) } else { None };

    let priority = opts.priority;
    // Desktop Duplication com a tela noutra placa: a captura fica lá, com a sua textura.
    let mut cross: Option<(D3d, ID3D11Texture2D, super::screen::CrossCopy)> = None;
    let mut cap = match cap.take() {
        Some(item) => {
            let fmt = if color.hdr { DirectXPixelFormat::R16G16B16A16Float } else { DirectXPixelFormat::B8G8R8A8UIntNormalized };
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&d.rt, fmt, 2, item.Size()?)?;
            let event = CreateEventW(None, false, false, None)?;
            let ev = event.0 as isize;
            let token = pool.FrameArrived(&TypedEventHandler::new(move |_, _| {
                let _ = SetEvent(HANDLE(ev as *mut _));
                Ok(())
            }))?;
            let capture = pool.CreateCaptureSession(&item)?;
            let _ = capture.SetIsCursorCaptureEnabled(true);
            let _ = capture.SetIsBorderRequired(false);
            let _ = capture.SetMinUpdateInterval(windows::Foundation::TimeSpan { Duration: 10_000_000 / fps as i64 });
            capture.StartCapture()?;
            Cap::Wgc { item, pool: Some((pool, capture, event, token)) }
        }
        None => {
            let fmt = if vf.hdr10 { DXGI_FORMAT_R10G10B10A2_UNORM } else { DXGI_FORMAT_B8G8R8A8_UNORM };
            if let Some(a) = screen_ad.as_ref().filter(|a| !crate::capture::same_adapter(a, &d.dev)) {
                let cd = device(Some(a))?;
                let cm = bgra_texture(&cd.dev, src_w, src_h, fmt)?;
                let cc = super::screen::CrossCopy::new(&cd, d, fmt, src_w, src_h)?;
                eprintln!("captura: a tela é de outra placa; a imagem passa pela memória do PC");
                cross = Some((cd, cm, cc));
            }
            let x = match &cross {
                Some((cd, cm, _)) => super::screen::Dda::new(cd, mon, color.hdr, color.white_nits, cm, vf.hdr10, src_w, src_h)?,
                None => super::screen::Dda::new(d, mon, color.hdr, color.white_nits, &mid, vf.hdr10, src_w, src_h)?,
            };
            eprintln!("captura: Desktop Duplication, área de trabalho «{}»", x.desk_name);
            Cap::Dda(Box::new(x))
        }
    };
    let mut fed_at = Instant::now() - Duration::from_secs(1);
    let mut due = 0i64;
    let mut was_secure = false;
    // Tamanho novo da janela compartilhada e desde quando.
    let mut resize: Option<((u32, u32), Instant)> = None;
    let mut first_size: Option<(u32, u32)> = None;

    // Som para quem tem a permissão. Mesmo PC: tira o som da janela que assiste.
    // Fora da rede local, quem decodifica Opus recebe o som em Opus.
    let hearing = |guests: &[Guest]| -> Vec<(Arc<dyn Link>, bool)> {
        guests.iter().filter(|g| g.perms & SOUND != 0).map(|g| (g.link.clone(), g.hello.flags & OPUS_OK != 0 && !g.link.near())).collect()
    };
    let audio_links = Arc::new(Mutex::new(hearing(&room.guests)));
    let exclude = room.guests.iter().find(|g| g.link.same_pc() && g.hello.pid != 0).map(|g| g.hello.pid);
    let stop_audio = Arc::new(AtomicBool::new(false));
    let audio_thread = {
        let (l, stop) = (audio_links.clone(), stop_audio.clone());
        std::thread::Builder::new().name("compartilhar-som".into()).spawn(move || audio(l, exclude, stop)).ok()
    };
    for g in &mut room.guests {
        g.input = (g.perms & (KEYBOARD | MOUSE) != 0).then(|| Injector::new(mon, g.perms).with_window(win));
        g.heard = Instant::now();
    }

    let mut out = Vec::with_capacity(MTU);
    let mut buf = vec![0u8; 2048];
    let info = |perms: u8| Msg::Info { codec, w: out_w, h: out_h, fps, perms, recover: enc.recovers(), hdr10: vf.hdr10, yuv444: vf.yuv444, priority, deep: vf.deep };
    let infos: Vec<Vec<u8>> = (0..=255u8).map(|p| {
        let mut b = Vec::new();
        write(&mut b, &info(p));
        b
    }).collect();
    // Quadros enviados há pouco, com a hora de cada um, para invalidar os que se perderam.
    let mut sent: VecDeque<(u32, i64)> = VecDeque::new();
    // Controle de taxa: um por pessoa (pela chave dela); o codificador segue a pior.
    let mut rates: HashMap<String, super::rate::Rate> = HashMap::new();
    let mut applied = top_bits;
    // O CABLE Input do VB-CABLE, aberto no primeiro bloco de microfone (uma tentativa por sessão).
    let mut cable: Option<Option<super::mic::Cable>> = None;
    let born = Instant::now();
    for g in &room.guests {
        g.link.send(&infos[g.perms as usize]);
    }

    let interval = 10_000_000i64 / fps as i64;
    // Quadros dados ao codificador; o ritmo constante só repete depois do primeiro.
    let mut fed = 0u32;
    let mut pending: Option<i64> = None;
    let mut pkts: Vec<Pkt> = Vec::new();
    let mut ready: Vec<Ready> = Vec::new();
    let pacer = super::pace::Pacer::new();
    // PyroWave pedido: se o caminho de alguém sair da rede local ou da VPN (ou voltar), o vídeo
    // recomeça com o codec que cabe.
    let wave_asked = codec_opt == "pyrowave" && !room.wave_failed;
    // A contagem continua de uma sessão para a outra: quem assiste descarta quadro "velho".
    let mut frame_id = room.frame_id;
    let mut result = End::Gone;
    let mut failed = None;
    // Intervalos entre quadros dados ao codificador, para o registro no fim.
    let mut fed_gaps: Vec<i64> = Vec::new();
    let mut captured = 0u32;
    let mut pad_msgs: Vec<PadMsg> = Vec::new();
    let mut info_tick = Instant::now();

    let mut room_names = String::from("\u{0}");
    let mut room_at = Instant::now() - Duration::from_secs(10);
    // Uso da GPU daqui, lido só enquanto alguém com o medidor detalhado pede.
    let gpu_use = crate::sys::gpuuse::Watch::default();
    let mut room_count = usize::MAX;
    let mut constant = asks_constant(&room.guests);
    eprintln!("ritmo: {}, pedido por quem assiste", pace_name(constant));
    let _ = std::fs::remove_file(commands_path());
    'run: loop {
        // A lista da sala, para a página: na hora quando alguém entra ou sai, e a cada segundo
        // pelo caminho, que muda (relé, direto) e custa uma consulta à conexão.
        if room.guests.len() != room_count || room_at.elapsed() > Duration::from_secs(1) {
            room_at = Instant::now();
            room_count = room.guests.len();
            let names: String = room.guests.iter().map(|g| format!("{}\t{}\t{}\t{}\n", g.key(), g.perms, g.link.route(), g.name)).collect();
            if names != room_names {
                write_room(&names);
                room_names = names;
            }
            // O que a página pediu: desconectar alguém ou mudar o acesso na hora.
            for (cmd, key, perms) in take_commands() {
                let Some(g) = room.guests.iter_mut().find(|g| g.key() == key) else { continue };
                match (cmd.as_str(), perms) {
                    ("desconectar", _) => {
                        eprintln!("«{}» desconectado por este PC", g.name);
                        write(&mut out, &Msg::Bye);
                        g.link.send(&out);
                        g.link.send(&out);
                        g.gone = true;
                    }
                    ("acesso", Some(p)) if p != g.perms => {
                        eprintln!("«{}» agora: {}", g.name, perms_text(p));
                        super::notice("Acesso de «{}» mudou na hora", &g.name);
                        g.perms = p;
                        if let Some(inj) = &mut g.input {
                            inj.release();
                        }
                        g.input = (p & (KEYBOARD | MOUSE) != 0).then(|| Injector::new(mon, p).with_window(win));
                        g.link.send(&infos[p as usize]);
                        *audio_links.lock().unwrap() = hearing(&room.guests);
                        room_count = usize::MAX;
                    }
                    _ => {}
                }
            }
            // O ritmo segue o que pede quem lidera a sala, sem recomeçar o vídeo.
            if constant != asks_constant(&room.guests) {
                constant = !constant;
                eprintln!("ritmo: {}, pedido por quem assiste", pace_name(constant));
            }
            // As opções da página valem na hora: o que o vídeo usa recomeça a transmissão (um
            // quadro-chave); mouse e teclado mudam aqui; o som se ajusta sozinho.
            let fresh = room_opts();
            if fresh != opts {
                super::notice("Opções do host aplicadas na conexão", "");
                if fresh.video_differs(&opts) {
                    eprintln!("opções do vídeo mudaram: recomeçando");
                    result = End::Again;
                    break 'run;
                }
                for g in &mut room.guests {
                    if let Some(inj) = &mut g.input {
                        inj.rules(&fresh);
                    }
                }
                opts = fresh;
            }
            if win.is_some_and(|h| !IsWindow(Some(h)).as_bool()) {
                result = window_closed(room);
                break 'run;
            }
            if wave_asked {
                let fit = room.guests.iter().all(|g| g.hello.codecs & WAVE != 0 && g.link.near());
                if fit != (codec == VideoCodec::Wave) {
                    eprintln!("{}", if fit { "todos na rede local ou na VPN: o vídeo volta ao PyroWave" } else { "alguém saiu da rede local ou da VPN: o vídeo sai do PyroWave" });
                    result = End::Again;
                    break 'run;
                }
            }
        }
        let secure = match &cap {
            Cap::Wgc { pool, .. } => {
                if let Some((_, _, event, _)) = pool {
                    let _ = WaitForSingleObject(*event, 1);
                }
                false
            }
            Cap::Dda(x) => x.secure,
        };
        if secure != was_secure {
            was_secure = secure;
            eprintln!("{}", if secure { "tela segura do Windows (UAC, bloqueio ou PIN)" } else { "de volta à área de trabalho" });
        }
        // Quem chegou entra; se o vídeo atual não serve para ela, recomeça com todo mundo.
        if let Some(rx) = joins {
            while let Ok(j) = rx.try_recv() {
                if room.guests.len() >= max_guests() {
                    eprintln!("«{}» não entrou: a sala já tem {} pessoas (PRISMA_GUESTS)", j.name, room.guests.len());
                    write(&mut out, &Msg::Bye);
                    j.link.send(&out);
                    continue;
                }
                eprintln!("chegou «{}»: tela {}x{} a {} Hz, {} ({})", j.name, j.hello.w, j.hello.h, j.hello.hz, perms_text(j.perms), j.link.route());
                let echo = j.link.same_pc() && exclude.is_none() && j.perms & SOUND != 0;
                let ok = fits(&j.hello, codec, vf) && (codec != VideoCodec::Wave || j.link.near()) && !echo;
                let mut g = Guest::new(j);
                if ok {
                    g.input = (g.perms & (KEYBOARD | MOUSE) != 0).then(|| Injector::new(mon, g.perms).with_window(win));
                    g.link.send(&infos[g.perms as usize]);
                    enc.force_key();
                    room.guests.push(g);
                    *audio_links.lock().unwrap() = hearing(&room.guests);
                } else {
                    room.guests.push(g);
                    result = End::Again;
                    break 'run;
                }
            }
        }
        // Texto copiado no host vai para quem tem a permissão (duas vezes, contra perda).
        if room.clip.is_none() && room.guests.iter().any(|g| g.perms & COPY != 0) {
            room.clip = Some(super::clip::start());
        }
        if let Some(c) = &room.clip {
            while let Ok(t) = c.changed.try_recv() {
                room.clip_id = room.clip_id.wrapping_add(1);
                let pk = super::clip::packets(&t, room.clip_id);
                for g in room.guests.iter().filter(|g| g.perms & COPY != 0) {
                    for p in pk.iter().chain(pk.iter()) {
                        g.link.send(p);
                    }
                }
            }
            while let Ok(b) = c.rich.try_recv() {
                room.clip_id = room.clip_id.wrapping_add(1);
                for g in room.guests.iter_mut().filter(|g| g.perms & COPY != 0) {
                    g.rich.offer(room.clip_id, b.clone(), &*g.link);
                }
            }
            let now = Instant::now();
            for g in room.guests.iter_mut().filter(|g| g.perms & COPY != 0) {
                g.rich.tick(&*g.link, now);
            }
        }
        // O que cada pessoa mandou.
        for (gi, g) in room.guests.iter_mut().enumerate() {
            loop {
                match g.link.recv(&mut buf, Duration::ZERO) {
                    Poll::Got(n) => {
                        let Some(m) = read(&buf[..n]) else { continue };
                        if let Some(h) = Hello::from_msg(&m) {
                            // Reabriu, ou pediu outros quadros por segundo: recomeça com a tela nova
                            // dela. A mesma descrição repetida (contra perda) não recomeça de novo.
                            let same = (h.w, h.h, h.hz, h.codecs, h.flags, h.pid) == (g.hello.w, g.hello.h, g.hello.hz, g.hello.codecs, g.hello.flags, g.hello.pid);
                            g.heard = Instant::now();
                            if same {
                                // Talvez a descrição do vídeo não tenha chegado: vai de novo.
                                g.link.send(&infos[g.perms as usize]);
                                continue;
                            }
                            if only_pace_changed(&h, &g.hello) {
                                super::notice("«{}» mudou o ritmo dos quadros", &g.name);
                                g.hello = h;
                                // O ritmo novo vale já na próxima volta, sem esperar o segundo.
                                room_at = Instant::now() - Duration::from_secs(10);
                                continue;
                            }
                            // Pediu outros quadros por segundo ou o 4:4:4: avisa aqui o que mudou.
                            if h.hz != g.hello.hz && h.w == g.hello.w && h.h == g.hello.h {
                                super::notice("«{}» mudou os quadros por segundo", &g.name);
                            } else if (h.flags ^ g.hello.flags) & ASKS_444 != 0 {
                                super::notice("«{}» mudou o pedido de 4:4:4", &g.name);
                            } else if (h.flags ^ g.hello.flags) & ASKS_10 != 0 {
                                super::notice("«{}» mudou o pedido de 10 bits", &g.name);
                            }
                            g.hello = h;
                            result = End::Again;
                            break 'run;
                        }
                        g.heard = Instant::now();
                        match m {
                            Msg::Ping { t, gpu } => g.send(&Msg::Pong { t, host: now(), gpu: if gpu { gpu_use.get() } else { None } }, &mut out),
                            Msg::KeyReq => enc.force_key(),
                            Msg::Loss { permille } => g.loss = permille.min(1000),
                            Msg::Nack { frame } => {
                                // Para de usar o quadro perdido e os enviados depois dele, que dependiam dele.
                                let mut ok = false;
                                let mut gone = 0;
                                for &(id, pts) in sent.iter() {
                                    if id == frame || (id.wrapping_sub(frame) as i32) > 0 {
                                        gone += 1;
                                        ok = enc.invalidate(pts) || ok;
                                    }
                                }
                                // Sem como invalidar, ou perda antiga demais: quadro-chave.
                                if !ok || gone == 0 || gone > 16 {
                                    enc.force_key();
                                }
                            }
                            Msg::Rate(mut r) if top_bits > 0 => {
                                // O Meter conta também os pedaços que o FEC refez; a perda de rede de verdade vem no Loss.
                                r.loss_pm = r.loss_pm.max(g.loss);
                                let t = born.elapsed().as_millis() as i64;
                                rates.entry(g.key().to_string()).or_insert_with(|| super::rate::Rate::new(top_bits)).feedback(r, t);
                            }
                            Msg::Input { first, events } => {
                                // Na tela segura, só quem tem a permissão de sistema.
                                if let Some(inj) = g.input.as_mut().filter(|_| !secure || g.perms & SYSTEM != 0) {
                                    inj.apply(first, events);
                                }
                            }
                            Msg::Clip { id, part, parts, data } if g.perms & COPY != 0 => {
                                if let (Some(c), Some(t)) = (&room.clip, g.clip_in.add(id, part, parts, data)) {
                                    let _ = c.set.send(t);
                                }
                            }
                            m @ (Msg::ClipOffer { .. } | Msg::ClipWant { .. } | Msg::ClipData { .. }) if g.perms & COPY != 0 => {
                                if let Some(c) = &room.clip {
                                    if let Some(b) = g.rich.on(&m, &*g.link) {
                                        let _ = c.set_rich.send(b);
                                    }
                                }
                            }
                            Msg::Sas => {
                                if g.perms & SYSTEM == 0 {
                                    eprintln!("Ctrl+Alt+Del de «{}» recusado: sem a permissão de sistema", g.name);
                                } else {
                                    #[cfg(feature = "stream")]
                                    super::service::request_sas(&g.name);
                                    #[cfg(not(feature = "stream"))]
                                    eprintln!("Ctrl+Alt+Del só existe com o serviço");
                                }
                            }
                            Msg::Pad { pad, seq, on, state } => pad_msgs.push(PadMsg { guest: gi, pad, seq, on, state }),
                            Msg::Mic(data) if g.perms & MIC != 0 => {
                                let c = cable.get_or_insert_with(|| {
                                    let c = super::mic::Cable::open();
                                    if c.is_none() {
                                        eprintln!("microfone de «{}»: o VB-CABLE não está instalado neste PC", g.name);
                                    }
                                    c
                                });
                                if let Some(c) = c {
                                    c.push(g.key(), data);
                                }
                            }
                            Msg::Bye => {
                                g.gone = true;
                                break;
                            }
                            _ => {}
                        }
                    }
                    Poll::Stranger(n, from) => {
                        // Outro chegou pelo UDP: passa a vez para ele.
                        if let Some(h) = read(&buf[..n]).as_ref().and_then(Hello::from_msg) {
                            result = End::Stranger(h, from);
                            break 'run;
                        }
                    }
                    Poll::Empty => break,
                    Poll::Closed => {
                        g.gone = true;
                        break;
                    }
                }
            }
            if g.heard.elapsed() > SILENCE {
                g.gone = true;
            }
        }
        // O mais lento manda; quando ele sai, os outros puxam de volta para cima. Recusado pelo
        // codificador: não tenta de novo a cada volta.
        let want = rates.values().map(|r| r.current()).min().unwrap_or(top_bits);
        if want != applied {
            let _ = enc.set_bitrate(want);
            applied = want;
        }
        for m in pad_msgs.drain(..) {
            pad_msg(room, m, &mut out);
        }
        let repeat = info_tick.elapsed() > Duration::from_secs(2);
        if repeat {
            info_tick = Instant::now();
        }
        pads_tick(room, &mut out, repeat);
        if room.guests.iter().any(|g| g.gone) {
            for g in room.guests.iter_mut().filter(|g| g.gone) {
                if let Some(inj) = &mut g.input {
                    inj.release();
                }
                eprintln!("«{}» saiu", g.name);
            }
            room.guests.retain(|g| !g.gone);
            rates.retain(|k, _| room.guests.iter().any(|g| g.key() == k));
            *audio_links.lock().unwrap() = hearing(&room.guests);
            if room.guests.is_empty() {
                break;
            }
        }

        let mut changed = false;
        // O mesmo vídeo para todo mundo, em pedaços que cabem no caminho mais estreito.
        let room_mtu = room.guests.iter().map(|g| g.link.mtu()).min().unwrap_or(MTU);
        // O PyroWave fica sem FEC: só sai na rede local, e os pedaços críticos já vão duas vezes.
        let fec_on = codec != VideoCodec::Wave && room.guests.iter().any(Guest::wants_fec);
        let space = room_mtu - VIDEO_HEAD - if fec_on { super::fec::EXTRA } else { 0 };
        let step = (|| -> Result<()> {
            let pool = match &mut cap {
                Cap::Wgc { pool: Some((pool, ..)), .. } => pool,
                Cap::Wgc { .. } => return Ok(()),
                Cap::Dda(x) => {
                    match x.poll(&mid)? {
                        super::screen::Got::Frame(pts) => {
                            captured += 1;
                            pending = Some(pts)
                        }
                        super::screen::Got::Changed => changed = true,
                        super::screen::Got::Nothing => {}
                    }
                    enc.pump(space, &mut pkts, &mut ready)?;
                    // A Desktop Duplication entrega cada mudança (o mouse também): o ritmo fica aqui.
                    let t = now();
                    if enc.ready() && time_to_feed(t, due, interval, pending.is_some(), constant && fed > 0) {
                        due = next_due(t, due, interval);
                        if fed_gaps.len() < GAPS_KEPT {
                            fed_gaps.push(fed_at.elapsed().as_nanos() as i64 / 100);
                        }
                        fed_at = Instant::now();
                        // Ritmo constante com a tela parada: o último quadro de novo.
                        let pts = match pending.take() {
                            Some(pts) => {
                                match &cross {
                                    Some((cd, cm, cc)) => {
                                        x.compose(cm)?;
                                        cc.copy(cd, cm, d, &mid)?;
                                    }
                                    None => x.compose(&mid)?,
                                }
                                pts
                            }
                            None => now(),
                        };
                        enc.feed(&d.ctx, pts, space, &mut ready)?;
                        fed += 1;
                    }
                    return Ok(());
                }
            };
            // Numa mudança grande (Alt+Tab, minimizar), chegam vários quadros de uma vez: só o
            // mais novo é copiado (ou convertido do HDR). Antes, cada um passava pela GPU e o
            // anterior era jogado fora. `mid` com `pending` é a caixa de um quadro até o codificador:
            // com ele ocupado (`!enc.ready()`), o quadro seguinte escreve por cima do que esperava.
            let mut newest = None;
            while let Ok(f) = pool.TryGetNextFrame() {
                let pts = f.SystemRelativeTime()?.Duration;
                captured += 1;
                if let Some((old, _)) = newest.replace((f, pts)) {
                    let _ = old.Close();
                }
            }
            if let Some((f, pts)) = newest {
                // Janela com outro tamanho: o vídeo recomeça quando ela para de mudar (abaixo).
                if win.is_some() {
                    let cs = f.ContentSize()?;
                    let now_size = (even(cs.Width.max(0) as u32), even(cs.Height.max(0) as u32));
                    // Comparado ao primeiro quadro, e não ao tamanho do item, para nunca recomeçar em
                    // laço se os dois diferirem.
                    let base = *first_size.get_or_insert(now_size);
                    if now_size == base || now_size.0 < 16 || now_size.1 < 16 {
                        resize = None;
                    } else if resize.is_none_or(|(s, _)| s != now_size) {
                        resize = Some((now_size, Instant::now()));
                    }
                }
                let access: IDirect3DDxgiInterfaceAccess = f.Surface()?.cast()?;
                let tex: ID3D11Texture2D = access.GetInterface()?;
                if let Some(t) = &tone {
                    t.draw(&d.ctx, &d.dev, &tex, src_w, src_h)?;
                } else {
                    let bx = D3D11_BOX { left: 0, top: 0, front: 0, right: src_w, bottom: src_h, back: 1 };
                    d.ctx.CopySubresourceRegion(&mid, 0, 0, 0, 0, &tex, 0, Some(&bx));
                }
                let _ = f.Close();
                pending = Some(pts);
            }
            if resize.is_some_and(|(_, t)| t.elapsed() > Duration::from_millis(500)) {
                changed = true;
            }
            enc.pump(space, &mut pkts, &mut ready)?;
            let t = now();
            if enc.ready() && time_to_feed(t, due, interval, pending.is_some(), constant && fed > 0) {
                due = next_due(t, due, interval);
                if fed_gaps.len() < GAPS_KEPT {
                    fed_gaps.push(fed_at.elapsed().as_nanos() as i64 / 100);
                }
                fed_at = Instant::now();
                // Ritmo constante com a tela parada: o último quadro de novo.
                let pts = pending.take().unwrap_or_else(now);
                enc.feed(&d.ctx, pts, space, &mut ready)?;
                fed += 1;
            }
            Ok(())
        })();
        if let Err(e) = step {
            failed = Some(e);
            break;
        }
        if changed {
            eprintln!("a tela mudou de tamanho: recomeçando");
            result = End::Again;
            break;
        }
        for p in ready.drain(..) {
            if p.key {
                // Repete a descrição com cada quadro-chave, caso a primeira tenha se perdido.
                for g in &room.guests {
                    g.link.send(&infos[g.perms as usize]);
                }
            }
            let c = &p.chunks;
            let count = c.parts.len();
            if count == 0 || count > u16::MAX as usize {
                continue;
            }
            let mut packets = Vec::with_capacity(count);
            for (idx, chunk) in c.parts.iter().enumerate() {
                write(&mut out, &Msg::Video { key: p.key, more: c.more[idx], idx: idx as u16, count: count as u16, frame: frame_id, pts: p.pts, data: chunk });
                // Os pedaços críticos do PyroWave vão duas vezes: sem eles, o quadro inteiro se perde.
                for _ in 0..if idx < c.critical { 2 } else { 1 } {
                    packets.push(out.clone());
                }
            }
            let parity_from = packets.len();
            if fec_on {
                // A paridade segue a pior perda medida entre quem recebe o FEC.
                let loss = room.guests.iter().filter(|g| g.wants_fec()).map(|g| g.loss).max().unwrap_or(0);
                packets.extend(super::fec::packets(&c.parts, frame_id, p.pts, p.key, super::fec::percent(loss)));
            }
            // Com os dois lados no mesmo PC não há roteador para encher: sai tudo de uma vez.
            let links: Vec<(Arc<dyn Link>, bool)> = room.guests.iter().map(|g| (g.link.clone(), g.wants_fec())).collect();
            let gap = if room.guests.iter().all(|g| g.link.same_pc()) { Duration::ZERO } else { Duration::from_nanos(interval as u64 * 100) };
            pacer.frame(links, packets, parity_from, gap);
            sent.push_back((frame_id, p.pts));
            if sent.len() > 32 {
                sent.pop_front();
            }
            frame_id = frame_id.wrapping_add(1);
        }
    }
    room.frame_id = frame_id;
    if fed_gaps.len() > 1 {
        eprint!("ritmo: {}, {captured} capturados, {fed} quadros ao codificador; {}", pace_name(constant), super::view::gap_line(&mut fed_gaps[1..], fps).replace("mostrados", "dados ao codificador"));
    }
    for g in &mut room.guests {
        if let Some(inj) = &mut g.input {
            inj.release();
        }
        g.input = None;
    }
    stop_audio.store(true, Ordering::Relaxed);
    if let Some(t) = audio_thread {
        let _ = t.join();
    }
    if let Cap::Wgc { item, pool: Some((pool, capture, event, token)) } = cap {
        let _ = pool.RemoveFrameArrived(token);
        let _ = capture.Close();
        let _ = pool.Close();
        let _ = CloseHandle(event);
        drop(item);
    }
    match failed {
        Some(e) => Err(e),
        None => Ok(result),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_mark_round_trip() {
        assert_eq!(gpu_mark_parse("prisma.exe\n5\n42\n"), Some(("prisma.exe".into(), Some(5), Some(42))));
        assert_eq!(gpu_mark_parse("prisma.exe\n-\n42\n"), Some(("prisma.exe".into(), None, Some(42))));
        // A recusa da NVML diz o porquê no registro.
        assert!(gpu_lock_note(4).contains("serviço") && gpu_lock_note(3).contains("não deixa"));
        // Marca de antes do processo: volta sempre.
        assert_eq!(gpu_mark_parse("prisma.exe\n-\n"), Some(("prisma.exe".into(), None, None)));
        assert!(!held_by_other(None) && !held_by_other(Some(std::process::id())));
        // Marca quebrada não mexe no driver.
        assert_eq!(gpu_mark_parse("prisma.exe\nx\n"), None);
        assert_eq!(gpu_mark_parse("\n-\n"), None);
        assert_eq!(gpu_mark_parse("prisma.exe"), None);
    }

    #[test]
    fn feeds_on_the_capture_rhythm_and_repeats_only_when_constant() {
        let i = 10_000_000 / 60;
        let ms = |v: i64| v * 10_000;
        // O quadro do vsync seguinte chega um pouco antes da hora: vale.
        assert!(time_to_feed(ms(16), i, i, true, false));
        assert!(!time_to_feed(ms(14), i, i, true, true));
        // Tela parada: só o ritmo constante repete, e só na hora.
        assert!(!time_to_feed(ms(40), i, i, false, false));
        assert!(!time_to_feed(ms(16), i, i, false, true));
        assert!(time_to_feed(ms(17), i, i, false, true));
        // Vídeo a 60: a 119 Hz sai um quadro sim, outro não (595 em 10 s, não ~640); a 144 Hz, perto de 600.
        let fed_at = |hz: i64| {
            let (mut due, mut fed) = (0, 0);
            for k in 0..hz * 10 {
                let t = k * 10_000_000 / hz;
                if time_to_feed(t, due, i, true, false) {
                    due = next_due(t, due, i);
                    fed += 1;
                }
            }
            fed
        };
        assert_eq!(fed_at(60), 600);
        assert_eq!(fed_at(119), 595);
        assert!((585..=600).contains(&fed_at(144)), "{}", fed_at(144));
        // Depois de a tela ficar parada, o quadro novo vai na hora e o seguinte não vem em rajada.
        assert_eq!(next_due(ms(5000), i, i), ms(5000) + i * 9 / 10);
    }

    struct FakeLink {
        sent: Mutex<Vec<Vec<u8>>>,
        local: bool,
    }

    impl Link for FakeLink {
        fn send(&self, b: &[u8]) {
            self.sent.lock().unwrap().push(b.to_vec());
        }
        fn recv(&self, _: &mut [u8], _: Duration) -> Poll {
            Poll::Empty
        }
        fn same_pc(&self) -> bool {
            self.local
        }
    }

    #[test]
    fn keyboard_taps_fix_locks_and_mask_shortcuts() {
        // Só a trava diferente troca; iguais, nada.
        assert_eq!(keyboard_taps(Input::Locks { caps: false, num: true }, (true, true)), vec![VK_CAPITAL]);
        assert_eq!(keyboard_taps(Input::Locks { caps: true, num: false }, (false, true)), vec![VK_CAPITAL, VK_NUMLOCK]);
        assert!(keyboard_taps(Input::Locks { caps: true, num: true }, (true, true)).is_empty());
        assert_eq!(keyboard_taps(Input::Mask, (false, false)), vec![VIRTUAL_KEY(0xE8)]);
        assert!(keyboard_taps(Input::Key { scan: 0x2A, extended: false, down: true }, (false, false)).is_empty());
    }

    #[test]
    fn pace_follows_the_lead_guest_without_restarting() {
        let (mut a, _) = guest(0, true);
        let (b, _) = guest(0, true);
        assert!(!asks_constant(&[]));
        let old = a.hello;
        a.hello.flags |= CONSTANT;
        // Só o ritmo mudou: segue sem recomeçar. Com outra mudança junto, recomeça.
        assert!(only_pace_changed(&a.hello, &old));
        assert!(!only_pace_changed(&Hello { hz: 30, ..a.hello }, &old));
        assert!(!only_pace_changed(&Hello { flags: CONSTANT | ASKS_444, ..a.hello }, &old));
        assert!(!only_pace_changed(&old, &old));
        // Vale o pedido de quem lidera (o primeiro); o do segundo não muda a sala.
        let guests = vec![a, b];
        assert!(asks_constant(&guests));
        assert!(!asks_constant(&guests[1..]));
    }

    fn guest(perms: u8, local: bool) -> (Guest, Arc<FakeLink>) {
        let link = Arc::new(FakeLink { sent: Mutex::new(Vec::new()), local });
        let hello = Hello { w: 1920, h: 1080, hz: 60, codecs: 7, pid: 0, flags: 0 };
        (Guest::new(Joiner { link: link.clone(), hello, perms, name: "teste".into(), id: String::new() }), link)
    }

    /// Os avisos de controle que a pessoa recebeu: (controle, jogador, motivo).
    fn infos(l: &FakeLink) -> Vec<(u8, u8, u8)> {
        l.sent.lock().unwrap().iter().filter_map(|b| match read(b) {
            Some(Msg::PadInfo { pad, player, why, .. }) => Some((pad, player, why)),
            _ => None,
        }).collect()
    }

    #[test]
    fn room_file() {
        let r = parse_room("a1b2\t3\tdireto pela internet, 1.2.3.4:5, 9 ms\tNotebook da sala\nvelho\n\n");
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].key.as_str(), r[0].perms, r[0].name.as_str()), ("a1b2", 3, "Notebook da sala"));
        assert!(r[0].route.ends_with("9 ms"));
        assert_eq!((r[1].key.as_str(), r[1].name.as_str()), ("velho", "velho"));
    }

    fn room(guests: Vec<Guest>) -> (Room, super::super::pads::FakeLog) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let r = Room { guests, bridge: Some(Bridge::fake(log.clone())), bridge_err: None, anti_cheat: false, checked: Instant::now(), clip: None, clip_id: 0, frame_id: 0, wave_failed: false };
        (r, log)
    }

    fn msg(guest: usize, pad: u8, seq: u16, buttons: u16) -> PadMsg {
        PadMsg { guest, pad, seq, on: true, state: PadState { buttons, ..Default::default() } }
    }

    #[test]
    fn pads_need_permission() {
        let (g, l) = guest(SOUND | KEYBOARD | MOUSE, false);
        let (mut r, _) = room(vec![g]);
        let mut out = Vec::new();
        pad_msg(&mut r, msg(0, 0, 1, 1), &mut out);
        pad_msg(&mut r, msg(0, 0, 2, 1), &mut out);
        assert_eq!(r.pads(), 0);
        // Um aviso só, não um por pacote.
        assert_eq!(infos(&l), vec![(0, 0, PAD_NO_PERMISSION)]);
    }

    #[test]
    fn pads_plug_in_order_and_ignore_old_packets() {
        let (g, l) = guest(SOUND | PAD, false);
        let (mut r, log) = room(vec![g]);
        let mut out = Vec::new();
        pad_msg(&mut r, msg(0, 2, 10, 0x1000), &mut out);
        assert_eq!(infos(&l), vec![(2, 1, PAD_OK)]);
        pad_msg(&mut r, msg(0, 2, 9, 0x2000), &mut out);
        pad_msg(&mut r, msg(0, 2, 11, 0x4000), &mut out);
        let seen: Vec<u16> = log.lock().unwrap().iter().map(|x| x.1.buttons).filter(|&b| b != 0xFFFF).collect();
        assert_eq!(seen, vec![0x1000, 0x4000]);
        // A sequência dá a volta sem confundir o mais novo.
        r.guests[0].pads[2].as_mut().unwrap().seq = 65535;
        pad_msg(&mut r, msg(0, 2, 0, 0x8000), &mut out);
        assert_eq!(log.lock().unwrap().last().unwrap().1.buttons, 0x8000);
        // Controle que saiu solta a vaga.
        pad_msg(&mut r, PadMsg { guest: 0, pad: 2, seq: 1, on: false, state: PadState::default() }, &mut out);
        assert_eq!(r.pads(), 0);
    }

    #[test]
    fn four_pads_at_most() {
        let (a, _) = guest(PAD, false);
        let (b, lb) = guest(PAD, false);
        let (mut r, _) = room(vec![a, b]);
        let mut out = Vec::new();
        for p in 0..3 {
            pad_msg(&mut r, msg(0, p, 1, 0), &mut out);
        }
        pad_msg(&mut r, msg(1, 0, 1, 0), &mut out);
        pad_msg(&mut r, msg(1, 1, 1, 0), &mut out);
        assert_eq!(r.pads(), 4);
        assert_eq!(infos(&lb), vec![(0, 4, PAD_OK), (1, 0, PAD_FULL)]);
    }

    #[test]
    fn same_pc_does_not_loop() {
        let (g, l) = guest(PAD, true);
        let (mut r, _) = room(vec![g]);
        let mut out = Vec::new();
        // O controle de verdade (vaga 1 lá) vira o virtual da vaga 0; o virtual, visto como
        // controle 0 por quem assiste, não pode virar outro.
        pad_msg(&mut r, msg(0, 1, 1, 0), &mut out);
        pad_msg(&mut r, msg(0, 0, 1, 0), &mut out);
        assert_eq!(r.pads(), 1);
        assert_eq!(infos(&l), vec![(1, 1, PAD_OK), (0, 0, PAD_LOOP)]);
    }

    #[test]
    fn joiner_fits_current_video() {
        let h = |codecs, flags| Hello { w: 0, h: 0, hz: 0, codecs, pid: 0, flags };
        let plain = VideoFmt::default();
        assert!(fits(&h(2, 0), VideoCodec::Std(Codec::Hevc), plain));
        assert!(!fits(&h(1, 0), VideoCodec::Std(Codec::Hevc), plain));
        assert!(!fits(&h(7, 0), VideoCodec::Std(Codec::Av1), VideoFmt { hdr10: true, yuv444: false, deep: false }));
        assert!(fits(&h(7, SEES_HDR), VideoCodec::Std(Codec::Av1), VideoFmt { hdr10: true, yuv444: false, deep: false }));
        assert!(!fits(&h(7, SEES_HDR), VideoCodec::Std(Codec::Hevc), VideoFmt { hdr10: false, yuv444: true, deep: false }));
        assert!(fits(&h(0, DECODES_444), VideoCodec::Std(Codec::Hevc), VideoFmt { hdr10: false, yuv444: true, deep: false }));
        assert!(fits(&h(WAVE, 0), VideoCodec::Wave, plain));
        assert!(!fits(&h(7, DECODES_444), VideoCodec::Wave, plain));
        // O PyroWave 4:4:4 serve a quem o tem, peça ou não; o HDR10 pede a tela em HDR.
        assert!(fits(&h(WAVE, 0), VideoCodec::Wave, VideoFmt { hdr10: false, yuv444: true, deep: false }));
        assert!(!fits(&h(WAVE, ASKS_444), VideoCodec::Wave, VideoFmt { hdr10: true, yuv444: true, deep: false }));
        assert!(fits(&h(WAVE, SEES_HDR), VideoCodec::Wave, VideoFmt { hdr10: true, yuv444: true, deep: false }));
        // Os 10 bits SDR só vão a quem pediu (quem não conhece o pedido decodifica em 8).
        let deep = VideoFmt { deep: true, ..plain };
        assert!(!fits(&h(7, 0), VideoCodec::Std(Codec::Hevc), deep));
        assert!(fits(&h(7, ASKS_10), VideoCodec::Std(Codec::Av1), deep));
        assert!(!fits(&h(1, ASKS_10), VideoCodec::Std(Codec::Av1), deep));
    }

    #[test]
    fn window_point_to_desktop() {
        let r = |l, t, r, b| RECT { left: l, top: t, right: r, bottom: b };
        // Janela de 800x600 em (100, 50), com um monitor à esquerda do principal.
        let (win, virt) = (r(100, 50, 900, 650), r(-1920, 0, 1920, 1080));
        let (p, dx, dy) = to_desktop(0, 0, win, virt);
        assert_eq!((p.x, p.y), (100, 50));
        assert_eq!((dx, dy), ((2020i64 * 65535 / 3839) as i32, (50i64 * 65535 / 1079) as i32));
        let (p, ..) = to_desktop(65535, 65535, win, virt);
        assert_eq!((p.x, p.y), (899, 649));
        let (p, ..) = to_desktop(32768, 32768, win, virt);
        assert_eq!((p.x, p.y), (499, 349));
        // Janela sem tamanho (fechando): fica no canto dela, sem dividir por zero.
        let (p, ..) = to_desktop(65535, 65535, r(10, 10, 10, 10), virt);
        assert_eq!((p.x, p.y), (10, 10));
    }

    #[test]
    fn prisma_windows_stay_out() {
        let own = r"C:\Program Files\Prisma\prisma.exe";
        assert!(!shareable(Some(r"C:\Program Files\Prisma\Prisma.EXE"), own));
        assert!(!shareable(Some(r"D:\outra\prisma.exe"), own));
        assert!(shareable(Some(r"C:\Windows\notepad.exe"), own));
        // Sem acesso ao caminho (programa elevado): fica na lista.
        assert!(shareable(None, own));
    }
}
