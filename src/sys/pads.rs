//! Controles de jogo: leitura. O Prisma nunca grava nada no controle; o mapeamento (Fase 2) fica em
//! `remap`, que recebe cada relatório daqui.
//!
//! - Detecção pelo aviso do Windows (`WM_DEVICECHANGE`), sem consulta periódica: sem controle
//!   ligado, a thread fica parada esperando mensagem.
//! - Uma thread de leitura por controle HID (DualSense, DualShock 4 e controles genéricos), que
//!   termina quando o controle cai (erro 1167 ou 995).
//! - Controles Xbox e os que se apresentam como Xbox (o Manba One pelo dongle) pelo XInput, que lê
//!   sem foco e, pela função oculta, também o botão Guia. O XInput só é consultado quando existe
//!   um controle desses, porque consultar um lugar vazio custa caro.
//!
//! A bancada da Fase 0 (`research_notes/Controles e macros no Prisma/fase0.md`) mediu o que cada
//! controle entrega neste Windows.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use super::remap::Trig;

use windows::core::*;
use windows::Win32::Devices::Bluetooth::*;
use windows::Win32::Devices::DeviceAndDriverInstallation::*;
use windows::Win32::Devices::HumanInterfaceDevice::*;
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::UI::Input::XboxController::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// Botões no mesmo desenho para todos os controles (nomes do Xbox; no Sony, Cruz é A e assim por diante).
/// Máscara de botões: os físicos aqui e os virtuais do remapeamento (gatilhos, direções, gestos).
pub type Buttons = u64;
pub const A: Buttons = 1;
pub const B: Buttons = 1 << 1;
pub const X: Buttons = 1 << 2;
pub const Y: Buttons = 1 << 3;
pub const LB: Buttons = 1 << 4;
pub const RB: Buttons = 1 << 5;
pub const BACK: Buttons = 1 << 6;
pub const START: Buttons = 1 << 7;
pub const LS: Buttons = 1 << 8;
pub const RS: Buttons = 1 << 9;
pub const UP: Buttons = 1 << 10;
pub const DOWN: Buttons = 1 << 11;
pub const LEFT: Buttons = 1 << 12;
pub const RIGHT: Buttons = 1 << 13;
pub const GUIDE: Buttons = 1 << 14;
/// Clique do touchpad (DualSense e DualShock 4).
pub const PAD: Buttons = 1 << 15;
/// Botão de mudo do DualSense.
pub const MUTE: Buttons = 1 << 16;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Family {
    DualSense,
    Ds4,
    /// Pelo XInput: Xbox e os que se apresentam como Xbox.
    Xbox,
    /// HID genérico: botões numerados e eixos na ordem do controle.
    Generic,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Link {
    Usb,
    Bluetooth,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Battery {
    Unknown,
    /// No cabo, sem bateria a informar (o Xbox 360 com fio, o Manba One pelo dongle).
    Wired,
    Percent { pct: u8, charging: bool },
    /// Só em níveis, como o XInput dá: 0 vazia, 1 baixa, 2 média, 3 cheia.
    Level(u8),
}

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Input {
    pub buttons: Buttons,
    /// Analógicos de -1 a 1, com Y para cima positivo.
    pub lx: f32,
    pub ly: f32,
    pub rx: f32,
    pub ry: f32,
    /// Gatilhos de 0 a 1.
    pub lt: f32,
    pub rt: f32,
    /// Gyro em graus por segundo (inclinar, virar, girar), já sem o desvio parado, e acelerômetro
    /// em g. Só DualSense e DualShock 4 no relatório completo (USB, ou Bluetooth com o modo
    /// completo ligado por outro programa); `motion` diz se veio.
    pub gyro: [f32; 3],
    pub accel: [f32; 3],
    pub motion: bool,
    /// Touchpad: até dois dedos, de 0 a 1 (x da esquerda, y de cima).
    pub touch: [Option<(f32, f32)>; 2],
}

/// Calibração do gyro: com o controle parado (pouca variação por 1 s), a média vira o desvio que
/// sai de toda leitura. `ask` pede uma calibração já: aceita mais tremor e espera 1,5 s.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct GyroCal {
    pub bias: [f32; 3],
    pub ask: bool,
    /// Já calibrou pelo menos uma vez desde que o controle ligou.
    pub done: bool,
    sum: [f32; 3],
    lo: [f32; 3],
    hi: [f32; 3],
    n: u32,
    start: Option<Instant>,
}

impl GyroCal {
    /// Recebe o gyro cru (graus por segundo) e devolve sem o desvio.
    pub fn feed(&mut self, g: [f32; 3], now: Instant) -> [f32; 3] {
        let (range, need) = if self.ask { (6.0, Duration::from_millis(1500)) } else { (3.0, Duration::from_secs(1)) };
        if self.n == 0 {
            (self.lo, self.hi, self.sum, self.start) = (g, g, [0.0; 3], Some(now));
        }
        for i in 0..3 {
            self.lo[i] = self.lo[i].min(g[i]);
            self.hi[i] = self.hi[i].max(g[i]);
            self.sum[i] += g[i];
        }
        self.n += 1;
        let mean = self.sum.map(|v| v / self.n as f32);
        // Girando devagar e sempre igual não é desvio: o desvio de um controle fica abaixo de 15 °/s.
        let still = (0..3).all(|i| self.hi[i] - self.lo[i] < range && mean[i].abs() < 15.0);
        if !still {
            self.n = 0;
        } else if self.start.is_some_and(|t| now.saturating_duration_since(t) >= need) {
            self.bias = mean;
            (self.ask, self.done, self.n) = (false, true, 0);
        }
        [g[0] - self.bias[0], g[1] - self.bias[1], g[2] - self.bias[2]]
    }
}

/// Pedido da tela: calibrar o gyro de todos os controles na próxima vez que ficarem parados.
static CAL_ASK: AtomicBool = AtomicBool::new(false);

pub fn calibrate_gyro() {
    CAL_ASK.store(true, Ordering::Relaxed);
}

/// Intervalo entre relatórios, medido nos últimos 2 segundos.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rate {
    pub hz: f32,
    pub avg_ms: f32,
    pub p99_ms: f32,
    pub max_ms: f32,
    /// Só a taxa, contada pelo contador de pacotes do XInput com o controle em movimento: o
    /// Windows não entrega os relatórios do Xbox, então não há intervalos (média, p99, máximo).
    pub counted: bool,
}

#[derive(Clone, Debug)]
pub struct Pad {
    /// Caminho do aparelho, ou `xinput:N`.
    pub key: String,
    pub name: String,
    pub family: Family,
    pub link: Link,
    pub vid: u16,
    pub pid: u16,
    pub battery: Battery,
    pub input: Input,
    /// `None` quando o Windows não entrega os relatórios (o Xbox por Bluetooth).
    pub rate: Option<Rate>,
    /// Quantos botões o controle genérico declara.
    pub generic_buttons: u8,
    low_warned: bool,
    jitter_warned: bool,
    pub gyro_cal: GyroCal,
}

pub enum Event {
    Connected { name: String, link: Link },
    Disconnected { name: String },
    BatteryLow { name: String, battery: Battery },
    Jitter { name: String, p99_ms: f32 },
    /// Mapeamento: perfil trocado pelo programa em foco.
    Profile { name: String },
    /// O controle `key`, com perfil próprio, trocou para `name` (radial ou ligação): gravar.
    PadProfile { key: String, name: String },
    /// Parada de emergência ligada (`on`) ou desligada.
    Stopped { on: bool },
    /// Programa em foco roda como administrador: as teclas do Prisma não chegam nele.
    Elevated { exe: String },
    /// Botão mapeado para uma ação do Prisma (`remap::ACTIONS`), apertado no controle `pad`.
    Action { id: u8, pad: String },
    /// Botão mapeado para abrir um programa.
    Run { path: String },
    /// Aviso pronto, vindo de outra thread.
    Notice { text: String },
    /// O atalho do turbo ligou ou desligou o turbo de `button` no controle `pad`.
    Turbo { button: Buttons, on: bool, pad: String },
    /// O teclado do Prisma mudou: abriu, fechou, outro grupo ou outra camada.
    Keyboard,
    /// O radial do Prisma abriu, fechou ou aponta para outra ação.
    Radial,
    /// Prisma em foco: botão de navegação (`nav`).
    Nav(super::nav::Cmd),
}

static PADS: Mutex<Vec<Pad>> = Mutex::new(Vec::new());
static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());
/// Caminhos HID com thread de leitura aberta.
static OPEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);
/// Controles que o Windows reserva ao XInput (`IG_` no caminho): nome, conexão e taxa, se houver.
static IG: Mutex<Vec<(String, String, Link, Option<Rate>)>> = Mutex::new(Vec::new());
/// Controles Bluetooth pareados e conectados sem a parte de controle (HID).
static NO_HID: Mutex<Vec<String>> = Mutex::new(Vec::new());
static HUB: AtomicIsize = AtomicIsize::new(0);
static WATCH: AtomicIsize = AtomicIsize::new(0);
/// A tela Controles está aberta: o XInput é lido a cada 8 ms em vez de a cada segundo.
static LIVE: AtomicBool = AtomicBool::new(false);
/// Na primeira varredura não há aviso de "conectado": o controle já estava lá.
static QUIET: AtomicBool = AtomicBool::new(true);
static LOW_PCT: AtomicU8 = AtomicU8::new(10);
static JITTER_MS: AtomicU32 = AtomicU32::new(0);

/// Modelos em que as próximas fases poderão gravar (luz, vibração, gatilhos), por VID e PID.
/// Nenhum outro controle recebe escrita: um relatório de saída errado pode travar o firmware.
pub const WRITE_OK: &[(u16, u16, &str)] = &[
    (0x054c, 0x0ce6, "DualSense"),
    (0x054c, 0x0df2, "DualSense Edge"),
    (0x054c, 0x05c4, "DualShock 4"),
    (0x054c, 0x09cc, "DualShock 4 (2ª versão)"),
];

pub const WM_PADS: u32 = WM_APP + 93;
const TIMER_X: usize = 1;

/// Começa a vigiar os controles. `hub` recebe `WM_PADS` quando há aviso.
pub fn start(hub: HWND) {
    HUB.store(hub.0 as isize, Ordering::Relaxed);
    let _ = std::thread::Builder::new().name("controles".into()).spawn(|| unsafe { watch() });
}

pub fn list() -> Vec<Pad> {
    #[allow(unused_mut)]
    let mut v = PADS.lock().unwrap().clone();
    #[cfg(debug_assertions)]
    v.extend(FAKE.lock().unwrap().clone());
    v
}

/// Fotos de teste (`--test-pad=ds|ds4|xbox`, só no build de depuração): um controle de mentira com
/// Cruz/A e R1/RB apertados, o gatilho esquerdo no meio e o stick esquerdo fora do centro.
#[cfg(debug_assertions)]
static FAKE: Mutex<Option<Pad>> = Mutex::new(None);

#[cfg(debug_assertions)]
pub fn fake(kind: &str) {
    let family = match kind {
        "ds" => Family::DualSense,
        "ds4" => Family::Ds4,
        _ => Family::Xbox,
    };
    let input = Input { buttons: A | RB, lt: 0.6, lx: 0.6, ly: 0.4, gyro: [24.0, -90.0, 6.0], motion: family != Family::Xbox, touch: [Some((0.3, 0.45)), None], ..Default::default() };
    let pad = Pad { key: "test".into(), name: nice_name(family, ""), family, link: Link::Usb, vid: 0, pid: 0, battery: Battery::Percent { pct: 82, charging: false }, input, rate: None, generic_buttons: 0, low_warned: false, jitter_warned: false, gyro_cal: GyroCal::default() };
    *FAKE.lock().unwrap() = Some(pad);
}

pub fn take_events() -> Vec<Event> {
    std::mem::take(&mut *EVENTS.lock().unwrap())
}

/// Nomes dos controles Bluetooth conectados sem a parte de controle.
pub fn without_hid() -> Vec<String> {
    NO_HID.lock().unwrap().clone()
}

/// Limites dos avisos: bateria em % (0 desliga) e variação em ms (0 desliga).
pub fn set_limits(low_pct: u8, jitter_ms: u32) {
    LOW_PCT.store(low_pct, Ordering::Relaxed);
    JITTER_MS.store(jitter_ms, Ordering::Relaxed);
}

/// Tela Controles aberta ou fechada: muda a frequência do XInput.
pub fn set_live(on: bool) {
    if LIVE.swap(on, Ordering::Relaxed) != on {
        poke();
    }
}

pub(crate) fn poke() {
    let w = WATCH.load(Ordering::Relaxed);
    if w != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(w as *mut _)), WM_APP, WPARAM(0), LPARAM(0));
        }
    }
}

/// Desconecta um controle Bluetooth pelo rádio, sem administrador (medido na Fase 0). O endereço
/// vem do número de série do aparelho HID.
pub fn disconnect(key: &str) -> bool {
    let path = match key.strip_prefix("xinput:") {
        // O XInput não diz qual controle é qual lugar: só dá para saber quando um só está no Bluetooth.
        Some(_) => match IG.lock().unwrap().iter().filter(|e| e.2 == Link::Bluetooth).collect::<Vec<_>>().as_slice() {
            [(p, ..)] => p.clone(),
            _ => return false,
        },
        None => key.to_string(),
    };
    if link_of(&path) != Link::Bluetooth {
        return false;
    }
    unsafe {
        let Some(h) = open(&path, 0) else { return false };
        let serial = hid_string(h, HidD_GetSerialNumberString);
        let _ = CloseHandle(h);
        let hex: String = serial.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        let Ok(addr) = u64::from_str_radix(&hex, 16) else { return false };
        let params = BLUETOOTH_FIND_RADIO_PARAMS { dwSize: std::mem::size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32 };
        let mut radio = HANDLE::default();
        let Ok(f) = BluetoothFindFirstRadio(&params, &mut radio) else { return false };
        let mut ok;
        loop {
            // IOCTL_BTH_DISCONNECT_DEVICE
            ok = windows::Win32::System::IO::DeviceIoControl(radio, 0x41000c, Some(&addr as *const u64 as *const _), 8, None, 0, None, None).is_ok();
            let _ = CloseHandle(radio);
            if ok || BluetoothFindNextRadio(f, &mut radio).is_err() {
                break;
            }
        }
        let _ = BluetoothFindRadioClose(f);
        ok
    }
}

pub(crate) fn emit(e: Event) {
    if QUIET.load(Ordering::Relaxed) {
        return;
    }
    EVENTS.lock().unwrap().push(e);
    let h = HUB.load(Ordering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), WM_PADS, WPARAM(0), LPARAM(0));
        }
    }
}

// ---------- Vigia ----------

unsafe extern "system" fn watch_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_DEVICECHANGE if w.0 as u32 == DBT_DEVICEARRIVAL || w.0 as u32 == DBT_DEVICEREMOVECOMPLETE => {
            scan();
            LRESULT(1)
        }
        WM_TIMER => {
            poll_xinput();
            LRESULT(0)
        }
        WM_APP => {
            set_timer(h);
            LRESULT(0)
        }
        _ => DefWindowProcW(h, m, w, l),
    }
}

unsafe fn watch() {
    let inst = GetModuleHandleW(None).unwrap_or_default();
    let wc = WNDCLASSW { lpfnWndProc: Some(watch_proc), lpszClassName: w!("PrismaPads"), hInstance: inst.into(), ..Default::default() };
    RegisterClassW(&wc);
    let Ok(h) = CreateWindowExW(WINDOW_EX_STYLE(0), w!("PrismaPads"), w!("PrismaPads"), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(inst.into()), None) else {
        return;
    };
    WATCH.store(h.0 as isize, Ordering::Relaxed);
    let filter = DEV_BROADCAST_DEVICEINTERFACE_W {
        dbcc_size: std::mem::size_of::<DEV_BROADCAST_DEVICEINTERFACE_W>() as u32,
        dbcc_devicetype: DBT_DEVTYP_DEVICEINTERFACE.0,
        dbcc_classguid: HidD_GetHidGuid(),
        ..Default::default()
    };
    let _ = RegisterDeviceNotificationW(HANDLE(h.0), &filter as *const _ as *const _, DEVICE_NOTIFY_WINDOW_HANDLE);
    scan();
    QUIET.store(false, Ordering::Relaxed);
    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        DispatchMessageW(&msg);
    }
}

/// XInput: a cada 4 ms com o mapeamento ligado, 8 ms com a tela Controles ou o Prisma aberto, a cada segundo para bateria
/// e avisos, e nada sem controle.
unsafe fn set_timer(h: HWND) {
    if IG.lock().unwrap().is_empty() {
        let _ = KillTimer(Some(h), TIMER_X);
        poll_xinput();
    } else {
        let ms = if super::remap::enabled() { 4 } else if LIVE.load(Ordering::Relaxed) || super::nav::window_open() { 8 } else { 1000 };
        SetTimer(Some(h), TIMER_X, ms, None);
    }
}

/// Um aparelho HID que se declara controle (uso 1:4 joystick ou 1:5 gamepad).
struct Dev {
    path: String,
    vid: u16,
    pid: u16,
    name: String,
    serial: String,
    input_len: u16,
    output_len: u16,
}

fn link_of(path: &str) -> Link {
    let p = path.to_ascii_lowercase();
    if p.contains("00001124-0000-1000-8000-00805f9b34fb") || p.contains("00001812-0000-1000-8000-00805f9b34fb") {
        Link::Bluetooth
    } else {
        Link::Usb
    }
}

fn is_xinput(path: &str) -> bool {
    path.to_ascii_lowercase().contains("&ig_")
}

/// DualSense e DualShock 4 no Bluetooth começam no modo simples, sem gyro nem touchpad. Ler o
/// relatório de recurso 0x05 (a calibração de fábrica) passa o controle ao modo completo, como faz
/// o driver do Linux. Só lê: nada é gravado no controle. Vale até o controle desligar.
unsafe fn full_mode(path: &str) -> bool {
    let Some(h) = open(path, (FILE_GENERIC_READ | FILE_GENERIC_WRITE).0).or_else(|| open(path, FILE_GENERIC_READ.0)) else { return false };
    let mut buf = [0u8; 41];
    buf[0] = 0x05;
    let ok = HidD_GetFeature(h, buf.as_mut_ptr() as *mut _, buf.len() as u32);
    let _ = CloseHandle(h);
    ok
}

/// Vibração de confirmação: até quando vibrar e com que força (motor fraco, motor forte).
static BUZZ: Mutex<Option<(Instant, [u8; 2])>> = Mutex::new(None);

/// Vibra de leve para confirmar: 0 não vibra, 1 leve, 2 forte. Curta, para não atrapalhar quem
/// está jogando.
pub fn buzz(level: u8) {
    let v = match level {
        1 => [0x60, 0x00],
        2 => [0x00, 0xC0],
        _ => return,
    };
    *BUZZ.lock().unwrap() = Some((Instant::now() + Duration::from_millis(180), v));
}

fn buzzing() -> [u8; 2] {
    let mut g = BUZZ.lock().unwrap();
    match *g {
        Some((until, v)) if Instant::now() < until => v,
        Some(_) => {
            *g = None;
            [0, 0]
        }
        None => [0, 0],
    }
}

/// Os 11 bytes de um gatilho adaptativo do DualSense. Os modos são os oficiais do firmware
/// (0x05 solta, 0x21 resiste, 0x25 arma, 0x26 vibra); os modos simples antigos e os de depuração
/// (0xFC a 0xFE) ficam de fora: estes deixam o gatilho num estado que só o reset desfaz.
/// O curso vale por dez zonas, uma por décimo.
fn trig_block(t: Trig) -> [u8; 11] {
    let mut b = [0u8; 11];
    // As zonas do ponto escolhido até o fim do curso.
    let zones = |start: u8| !((1u16 << start.min(9)) - 1) & 0x03FF;
    // Três bits de força em cada zona ligada.
    let forces = |start: u8, power: u8| {
        let v = (power.clamp(1, 8) - 1) as u32;
        (start.min(9)..10).fold(0u32, |a, i| a | v << (3 * i))
    };
    match t {
        // Passar ao jogo não manda bloco nenhum: o bit do gatilho fica desligado.
        Trig::Pass => {}
        Trig::Off => b[0] = 0x05,
        Trig::Rigid { start, power } => {
            b[0] = 0x21;
            b[1..3].copy_from_slice(&zones(start).to_le_bytes());
            b[3..7].copy_from_slice(&forces(start, power).to_le_bytes());
        }
        Trig::Click { start, power } => {
            b[0] = 0x25;
            // A arma resiste numa zona e cede na seguinte.
            let s = start.clamp(2, 7);
            b[1..3].copy_from_slice(&((1u16 << s) | (1u16 << (s + 1))).to_le_bytes());
            b[3] = power.clamp(1, 8) - 1;
        }
        Trig::Buzz { start, power, hz } => {
            b[0] = 0x26;
            b[1..3].copy_from_slice(&zones(start).to_le_bytes());
            b[3..7].copy_from_slice(&forces(start, power).to_le_bytes());
            b[9] = hz.clamp(1, 40);
        }
    }
    b
}

/// Cor que o Prisma devolve ao controle quando deixa de mandar na luz.
const LIGHT_DEFAULT: [u8; 3] = [0, 0, 128];
/// Controles em que o Prisma mexeu (caminho, modelo, conexão, tamanho do relatório, se mexeu na
/// luz): ao fechar o Prisma soltam os gatilhos e voltam à cor padrão.
static LIT: Mutex<Vec<(String, Family, Link, usize, bool, [Trig; 2])>> = Mutex::new(Vec::new());

/// CRC-32 dos relatórios de saída pelo Bluetooth: conta um byte de cabeçalho (`seed`) e os dados.
fn crc32(seed: u8, data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in std::iter::once(&seed).chain(data) {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

/// Relatório de saída com a barra de luz, os gatilhos adaptativos e a vibração, no formato de cada
/// modelo e conexão (o mesmo que o driver do Linux usa). `rgb` em `None` não mexe na luz; `trig` é
/// L2 e R2, só do DualSense. `seq` é a sequência de 0 a 15 que o DualSense pede no Bluetooth.
pub fn out_report(f: Family, link: Link, rgb: Option<[u8; 3]>, trig: [Trig; 2], rumble: [u8; 2], seq: u8) -> Option<Vec<u8>> {
    let (mut r, at) = match (f, link) {
        (Family::DualSense, Link::Usb) => (vec![0u8; 48], 1),
        (Family::DualSense, Link::Bluetooth) => (vec![0u8; 78], 3),
        (Family::Ds4, Link::Usb) => (vec![0u8; 32], 1),
        (Family::Ds4, Link::Bluetooth) => (vec![0u8; 78], 3),
        _ => return None,
    };
    if f == Family::DualSense {
        r[0] = if link == Link::Usb { 0x02 } else { 0x31 };
        if link == Link::Bluetooth {
            r[1] = (seq & 15) << 4;
            r[2] = 0x10;
        }
        // Vibração: os dois bits do driver do Linux. Os bits dos gatilhos vêm do DSX e do
        // pydualsense; o driver do Linux não mexe neles.
        if rumble != [0, 0] {
            r[at] |= 0x03;
            r[at + 2] = rumble[0];
            r[at + 3] = rumble[1];
        }
        // Só os gatilhos que o Prisma manda: os de passar ao jogo ficam com o bit desligado.
        if trig[1] != Trig::Pass {
            r[at] |= 0x04;
        }
        if trig[0] != Trig::Pass {
            r[at] |= 0x08;
        }
        r[at + 10..at + 21].copy_from_slice(&trig_block(trig[1]));
        r[at + 21..at + 32].copy_from_slice(&trig_block(trig[0]));
        if let Some(c) = rgb {
            // Liga o controle da luz, encerra a animação azul do começo e põe a cor.
            r[at + 1] = 0x04;
            r[at + 38] = 0x02;
            r[at + 41] = 0x02;
            r[at + 44..at + 47].copy_from_slice(&c);
        }
    } else {
        r[0] = if link == Link::Usb { 0x05 } else { 0x11 };
        if link == Link::Bluetooth {
            r[1] = 0xC0;
        }
        if rumble != [0, 0] {
            r[at] |= 0x01;
            r[at + 3] = rumble[0];
            r[at + 4] = rumble[1];
        }
        if let Some(c) = rgb {
            r[at] |= 0x02;
            r[at + 5..at + 8].copy_from_slice(&c);
        }
    }
    if link == Link::Bluetooth {
        let c = crc32(0xA2, &r[..74]);
        r[74..].copy_from_slice(&c.to_le_bytes());
    }
    Some(r)
}

/// Manda um relatório de saída: pela escrita comum (completada até o tamanho que o Windows pede) e,
/// se falhar, pelo pedido de controle do HID.
unsafe fn write_report(path: &str, r: &[u8], out_len: usize) -> bool {
    let Some(h) = open(path, (FILE_GENERIC_READ | FILE_GENERIC_WRITE).0) else { return false };
    let mut full = r.to_vec();
    full.resize(out_len.max(r.len()), 0);
    let mut n = 0u32;
    let ok = WriteFile(h, Some(&full), Some(&mut n), None).is_ok() || HidD_SetOutputReport(h, r.as_ptr() as *const _, r.len() as u32);
    let _ = CloseHandle(h);
    ok
}

/// Háptico de verdade do DualSense pelo Bluetooth: os atuadores recebem áudio de 3 kHz, 8 bits,
/// estéreo, 32 quadros por relatório 0x32, um relatório a cada 10,67 ms. Formato lido no SAxense
/// (github.com/egormanga/SAxense, MPL-2.0, de Sdore), a única fonte que o descreve; nada copiado.
/// `n` é o contador que o controle pede, e a onda é um seno de 170 Hz com a força `level`, somado
/// ao som do PC (`audio`, já em 3 kHz).
fn coil_report(n: u8, phase: &mut f32, level: u8, audio: &[i8; 32]) -> [u8; 142] {
    let mut r = [0u8; 142];
    r[0] = 0x32;
    // Pacote 0x11 (cabeçalho fixo, com o contador no fim) e pacote 0x12 com 64 bytes de amostras.
    r[2..11].copy_from_slice(&[0x91, 0x07, 0xFE, 0, 0, 0, 0, 0xFF, n]);
    r[11] = 0x92;
    r[12] = 64;
    let amp = level as f32 / 255.0 * 127.0;
    let step = std::f32::consts::TAU * 170.0 / 3000.0;
    for f in 0..32 {
        // Com sinal (s8), como o exemplo do ffmpeg do SAxense: o silêncio é zero.
        let v = (amp * phase.sin() + audio[f] as f32).round().clamp(-127.0, 127.0) as i8 as u8;
        r[13 + 2 * f] = v;
        r[14 + 2 * f] = v;
        *phase = (*phase + step) % std::f32::consts::TAU;
    }
    let c = crc32(0xA2, &r[..138]);
    r[138..].copy_from_slice(&c.to_le_bytes());
    r
}

/// Força pedida aos atuadores, se a escrita falhou (o controle não aceita o 0x32) e se o perfil
/// pede o som do PC.
type Coil = (AtomicU8, AtomicBool, AtomicBool);

/// Manda o áudio dos atuadores enquanto o leitor do controle existir. Fica numa linha própria:
/// uma escrita pelo Bluetooth pode levar milissegundos, e a leitura do controle não pode esperar.
fn coil_thread(path: String, out_len: usize, coil: Weak<Coil>) {
    let period = Duration::from_nanos(10_666_667);
    let (mut n, mut phase, mut quiet, mut cursor) = (0u8, 0f32, 0u8, 0u64);
    let mut audio = [0i8; 32];
    let mut next = Instant::now();
    while let Some(c) = coil.upgrade() {
        let level = c.0.load(Ordering::Relaxed);
        let sound = c.2.load(Ordering::Relaxed);
        if level == 0 && !sound && quiet == 0 {
            drop(c);
            std::thread::sleep(Duration::from_millis(15));
            next = Instant::now();
            continue;
        }
        // Alguns relatórios mudos depois de parar: o atuador termina parado, sem ficar com o resto.
        quiet = if level > 0 || sound { 8 } else { quiet - 1 };
        if sound {
            super::audio_haptic::take(&mut cursor, &mut audio);
        } else {
            audio = [0; 32];
        }
        let r = coil_report(n, &mut phase, level, &audio);
        if !unsafe { write_report(&path, &r, out_len) } {
            c.1.store(true, Ordering::Relaxed);
            return;
        }
        drop(c);
        n = n.wrapping_add(1);
        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
}

/// O que o Prisma quer no controle agora. `rgb` em `None` é a luz que o perfil não mexe.
#[derive(PartialEq, Clone, Copy, Default)]
struct Want {
    rgb: Option<[u8; 3]>,
    trig: [Trig; 2],
    rumble: [u8; 2],
}

/// Saída de um controle (luz, gatilhos e vibração): escreve só quando algo muda. Uma escrita que
/// falha não se repete até o controle reconectar.
struct PadOut {
    path: String,
    fam: Family,
    link: Link,
    out_len: usize,
    sent: Want,
    seq: u8,
    failed: bool,
    /// Botões da leitura anterior e até quando dura o toque do último aperto (háptico).
    prev: Buttons,
    tap: Option<(Instant, u8)>,
    /// Atuadores do DualSense pelo Bluetooth, ligados na primeira vez que o háptico pede.
    coil: Option<Arc<Coil>>,
}

impl PadOut {
    unsafe fn update(&mut self, pct: Option<u8>, inp: Option<&Input>) {
        let h = super::remap::haptic(&self.path);
        let mut feel = 0u8;
        if let Some(inp) = inp {
            // Toque de 40 ms a cada botão novo: um relatório só (4 ms) não se sente.
            if h.buttons != 0 && inp.buttons & !self.prev != 0 {
                self.tap = Some((Instant::now() + Duration::from_millis(40), (h.buttons.min(8) as u16 * 255 / 8) as u8));
            }
            self.prev = inp.buttons;
            feel = super::remap::haptic_level(h, inp);
        }
        // Som do PC: a linha do som corre enquanto algum controle pede.
        super::audio_haptic::demand(h.audio);
        match self.tap {
            Some((until, v)) if Instant::now() < until => feel = feel.max(v),
            Some(_) => self.tap = None,
            None => {}
        }
        let mut sound_done = false;
        // DualSense pelo Bluetooth: o háptico vai aos atuadores. Se o controle recusar o
        // relatório, volta ao motor fraco, como no cabo e no DualShock 4.
        if self.fam == Family::DualSense && self.link == Link::Bluetooth {
            if self.coil.is_none() && (feel > 0 || h.audio > 0) {
                let c = Arc::new((AtomicU8::new(0), AtomicBool::new(false), AtomicBool::new(false)));
                let (path, len, weak) = (self.path.clone(), self.out_len, Arc::downgrade(&c));
                if std::thread::Builder::new().name("háptico".into()).spawn(move || coil_thread(path, len, weak)).is_ok() {
                    self.coil = Some(c);
                }
            }
            if let Some(c) = self.coil.as_ref().filter(|c| !c.1.load(Ordering::Relaxed)) {
                c.0.store(feel, Ordering::Relaxed);
                c.2.store(h.audio > 0, Ordering::Relaxed);
                feel = 0;
                sound_done = true;
            }
        }
        // Sem os atuadores pelo Bluetooth: o DualSense no cabo recebe o som nos canais do háptico;
        // o resto sente a força do som no motor fraco.
        if h.audio > 0 && !sound_done && !(self.fam == Family::DualSense && self.link == Link::Usb && super::audio_haptic::usb_active()) {
            feel = feel.max(super::audio_haptic::envelope());
        }
        let want = Want {
            rgb: super::remap::light(&self.path).filter(|l| self.link == Link::Usb || l.1).and_then(|l| {
                // Relógio comum: dois controles piscando ficam no mesmo passo.
                let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
                super::remap::light_color(l.0, pct, ms)
            }),
            // Gatilho adaptativo só existe no DualSense.
            trig: if self.fam == Family::DualSense { super::remap::triggers(&self.path) } else { [Trig::Pass; 2] },
            // O motor fraco é o mais seco: é ele que faz as vezes do háptico.
            rumble: { let b = buzzing(); [b[0].max(feel), b[1]] },
        };
        if want == self.sent || self.failed {
            return;
        }
        self.seq = (self.seq + 1) & 15;
        // A luz que o perfil deixou de mandar volta ao padrão nesta mesma escrita.
        let rgb = want.rgb.or_else(|| self.sent.rgb.map(|_| LIGHT_DEFAULT));
        // Gatilho que passa ao jogo depois de um efeito do Prisma: solta uma vez, e daí não mexe mais.
        let trig = [0, 1].map(|i| if want.trig[i] == Trig::Pass && self.sent.trig[i] != Trig::Pass { Trig::Off } else { want.trig[i] });
        let Some(r) = out_report(self.fam, self.link, rgb, trig, want.rumble, self.seq) else { return };
        if !write_report(&self.path, &r, self.out_len) {
            self.failed = true;
            return;
        }
        self.sent = want;
        let mut lit = LIT.lock().unwrap();
        lit.retain(|l| l.0 != self.path);
        if want != Want::default() {
            lit.push((self.path.clone(), self.fam, self.link, self.out_len, want.rgb.is_some(), want.trig));
        }
    }
}

/// Ao fechar o Prisma: os controles em que ele mexeu soltam os gatilhos e param de vibrar; os de
/// luz mudada voltam à cor padrão.
pub fn release_pads() {
    for (path, f, link, len, lit, trig) in LIT.lock().unwrap().drain(..) {
        // Solta só o gatilho que o Prisma mandava; o de passar ao jogo fica com o jogo.
        let trig = trig.map(|t| if t == Trig::Pass { Trig::Pass } else { Trig::Off });
        if let Some(r) = out_report(f, link, lit.then_some(LIGHT_DEFAULT), trig, [0, 0], 0) {
            unsafe { write_report(&path, &r, len) };
        }
    }
}

unsafe fn open(path: &str, access: u32) -> Option<HANDLE> {
    CreateFileW(&HSTRING::from(path), access, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None).ok()
}

unsafe fn hid_string(h: HANDLE, f: unsafe fn(HANDLE, *mut core::ffi::c_void, u32) -> bool) -> String {
    let mut b = [0u16; 127];
    if !f(h, b.as_mut_ptr() as *mut _, 254) {
        return String::new();
    }
    String::from_utf16_lossy(&b).trim_end_matches('\0').trim().to_string()
}

unsafe fn devices() -> Vec<Dev> {
    let mut out = vec![];
    let guid = HidD_GetHidGuid();
    let Ok(set) = SetupDiGetClassDevsW(Some(&guid), None, None, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE) else { return out };
    for i in 0.. {
        let mut d = SP_DEVICE_INTERFACE_DATA { cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32, ..Default::default() };
        if SetupDiEnumDeviceInterfaces(set, None, &guid, i, &mut d).is_err() {
            break;
        }
        let mut buf = vec![0u64; 260];
        let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
        (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
        if SetupDiGetDeviceInterfaceDetailW(set, &d, Some(detail), (buf.len() * 8) as u32, None, None).is_err() {
            continue;
        }
        let path = PCWSTR((*detail).DevicePath.as_ptr()).to_string().unwrap_or_default();
        let Some(h) = open(&path, 0) else { continue };
        let mut pre = PHIDP_PREPARSED_DATA::default();
        let mut caps = HIDP_CAPS::default();
        let ok = HidD_GetPreparsedData(h, &mut pre) && {
            let r = HidP_GetCaps(pre, &mut caps).is_ok();
            HidD_FreePreparsedData(pre);
            r
        };
        if ok && caps.UsagePage == 1 && (caps.Usage == 4 || caps.Usage == 5) {
            let mut a = HIDD_ATTRIBUTES { Size: std::mem::size_of::<HIDD_ATTRIBUTES>() as u32, ..Default::default() };
            let _ = HidD_GetAttributes(h, &mut a);
            out.push(Dev { path, vid: a.VendorID, pid: a.ProductID, name: hid_string(h, HidD_GetProductString), serial: hid_string(h, HidD_GetSerialNumberString), input_len: caps.InputReportByteLength, output_len: caps.OutputReportByteLength });
        }
        let _ = CloseHandle(h);
    }
    let _ = SetupDiDestroyDeviceInfoList(set);
    out
}

fn family(vid: u16, pid: u16) -> Family {
    match (vid, pid) {
        (0x054c, 0x0ce6 | 0x0df2) => Family::DualSense,
        (0x054c, 0x05c4 | 0x09cc | 0x0ba0) => Family::Ds4,
        _ => Family::Generic,
    }
}

/// Nome curto para a tela e os avisos: os controles Sony se chamam "Wireless Controller" no USB.
fn nice_name(f: Family, name: &str) -> String {
    match f {
        Family::DualSense => "DualSense".into(),
        Family::Ds4 => "DualShock 4".into(),
        _ if name.is_empty() => "Controle".into(),
        _ => name.to_string(),
    }
}

unsafe fn scan() {
    let devs = devices();
    {
        let mut ig = IG.lock().unwrap();
        let rates: Vec<_> = ig.drain(..).map(|(p, _, _, r)| (p, r)).collect();
        for d in devs.iter().filter(|d| is_xinput(&d.path)) {
            let rate = rates.iter().find(|(p, _)| *p == d.path).and_then(|(_, r)| *r);
            ig.push((d.path.clone(), if d.name.is_empty() { "Controle Xbox".into() } else { d.name.clone() }, link_of(&d.path), rate));
        }
    }
    let mut open_set = OPEN.lock().unwrap();
    let open = open_set.get_or_insert_with(HashSet::new);
    for d in devs {
        if open.insert(d.path.clone()) {
            let _ = std::thread::Builder::new().name("controle".into()).spawn(move || read_loop(d));
        }
    }
    drop(open_set);
    check_bluetooth();
    let w = WATCH.load(Ordering::Relaxed);
    if w != 0 {
        set_timer(HWND(w as *mut _));
    }
}

/// Controles Bluetooth conectados que o Windows não ligou ao serviço de controle (HID): nenhum
/// programa consegue lê-los assim. Visto num DualSense em 2026-11-12; parear de novo resolve às vezes.
/// shortcut: o endereço é comparado com o número de série HID, que é o endereço nos Sony e Xbox; um
/// controle de outra marca com série diferente apareceria aqui por engano.
unsafe fn check_bluetooth() {
    let serials: Vec<String> = devices().into_iter().map(|d| d.serial.chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>().to_ascii_lowercase()).collect();
    let params = BLUETOOTH_DEVICE_SEARCH_PARAMS {
        dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32,
        fReturnConnected: true.into(),
        fReturnRemembered: true.into(),
        fReturnAuthenticated: true.into(),
        ..Default::default()
    };
    let mut info = BLUETOOTH_DEVICE_INFO { dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32, ..Default::default() };
    let mut found = vec![];
    if let Ok(f) = BluetoothFindFirstDevice(&params, &mut info) {
        loop {
            let cod = info.ulClassofDevice;
            // Periférico (classe maior 5) do tipo joystick ou gamepad.
            let pad = (cod >> 8) & 0x1f == 5 && matches!((cod >> 2) & 0x0f, 1 | 2);
            let addr = format!("{:012x}", info.Address.Anonymous.ullLong);
            if pad && info.fConnected.as_bool() && !serials.contains(&addr) {
                found.push(String::from_utf16_lossy(&info.szName).trim_end_matches('\0').to_string());
            }
            info = BLUETOOTH_DEVICE_INFO { dwSize: info.dwSize, ..Default::default() };
            if BluetoothFindNextDevice(f, &mut info).is_err() {
                break;
            }
        }
        let _ = BluetoothFindDeviceClose(f);
    }
    *NO_HID.lock().unwrap() = found;
}

// ---------- Leitura HID ----------

/// Escala de um eixo do HID genérico.
struct Axis {
    usage: u16,
    min: i32,
    max: i32,
}

unsafe fn generic_axes(pre: PHIDP_PREPARSED_DATA) -> (Vec<Axis>, u8) {
    let mut caps = HIDP_CAPS::default();
    let _ = HidP_GetCaps(pre, &mut caps);
    let mut n = caps.NumberInputValueCaps;
    let mut vc = vec![HIDP_VALUE_CAPS::default(); n as usize];
    let mut axes = vec![];
    if n > 0 && HidP_GetValueCaps(HidP_Input, vc.as_mut_ptr(), &mut n, pre).is_ok() {
        for v in &vc[..n as usize] {
            if v.UsagePage == 1 && !v.IsRange {
                axes.push(Axis { usage: v.Anonymous.NotRange.Usage, min: v.LogicalMin, max: v.LogicalMax });
            }
        }
    }
    let buttons = HidP_MaxUsageListLength(HidP_Input, Some(9), pre).min(32) as u8;
    (axes, buttons)
}

/// Relatório de um controle genérico: botões numerados (bit 0 é o botão 1), X e Y no analógico
/// esquerdo, Z e Rz no direito, Rx e Ry nos gatilhos, e o direcional (hat) nas setas.
/// shortcut: é a ordem mais comum no DirectInput; um controle com outra ordem aparece trocado na
/// tela, e o mapeamento por controle fica para a Fase 2.
unsafe fn generic_input(pre: PHIDP_PREPARSED_DATA, axes: &[Axis], report: &mut [u8]) -> Input {
    let mut inp = Input::default();
    let mut usages = [0u16; 32];
    let mut n = usages.len() as u32;
    if HidP_GetUsages(HidP_Input, 9, None, usages.as_mut_ptr(), &mut n, pre, report).is_ok() {
        for u in &usages[..n as usize] {
            if (1..=32).contains(u) {
                inp.buttons |= 1 << (u - 1);
            }
        }
    }
    let mut hat = None;
    for a in axes {
        let mut v = 0u32;
        if HidP_GetUsageValue(HidP_Input, 1, None, a.usage, &mut v, pre, report).is_err() {
            continue;
        }
        let span = (a.max - a.min).max(1) as f32;
        let unit = ((v as i32 - a.min) as f32 / span).clamp(0.0, 1.0);
        let stick = unit * 2.0 - 1.0;
        match a.usage {
            0x30 => inp.lx = stick,
            0x31 => inp.ly = -stick,
            0x32 => inp.rx = stick,
            0x35 => inp.ry = -stick,
            0x33 => inp.lt = unit,
            0x34 => inp.rt = unit,
            0x39 => hat = Some(v as i32 - a.min),
            _ => {}
        }
    }
    // O direcional entra fora dos botões numerados, para não confundir com o botão 11 em diante.
    inp.buttons = (inp.buttons & 0x3ff) | hat.map_or(0, |h| hat_bits(h as u8));
    inp
}

fn hat_bits(h: u8) -> Buttons {
    match h {
        0 => UP,
        1 => UP | RIGHT,
        2 => RIGHT,
        3 => DOWN | RIGHT,
        4 => DOWN,
        5 => DOWN | LEFT,
        6 => LEFT,
        7 => UP | LEFT,
        _ => 0,
    }
}

fn stick(b: u8) -> f32 {
    (b as f32 - 127.5) / 127.5
}

/// Botões dos Sony: `b0` direcional e símbolos, `b1` ombros e menus, `b2` PS, touchpad e mudo.
fn sony_buttons(b0: u8, b1: u8, b2: u8) -> Buttons {
    let mut b = hat_bits(b0 & 0x0f);
    for (bit, v) in [(0x10, X), (0x20, A), (0x40, B), (0x80, Y)] {
        if b0 & bit != 0 {
            b |= v;
        }
    }
    for (bit, v) in [(0x01, LB), (0x02, RB), (0x10, BACK), (0x20, START), (0x40, LS), (0x80, RS)] {
        if b1 & bit != 0 {
            b |= v;
        }
    }
    for (bit, v) in [(0x01, GUIDE), (0x02, PAD), (0x04, MUTE)] {
        if b2 & bit != 0 {
            b |= v;
        }
    }
    b
}

/// Relatórios Sony, sem gravar nada no controle. Devolve a entrada e, quando o relatório traz, a
/// bateria. USB: relatório 0x01 completo. Bluetooth: 0x01 é o modo simples (sem bateria); 0x31
/// (DualSense) e 0x11 (DualShock 4) só chegam se outro programa ligou o modo completo.
/// Posições da bateria e fórmula como o driver do Linux (hid-playstation).
pub fn sony(f: Family, link: Link, r: &[u8]) -> Option<(Input, Option<Battery>)> {
    let get = |i: usize| r.get(i).copied();
    let le = |i: usize| -> Option<f32> { Some(i16::from_le_bytes([get(i)?, get(i + 1)?]) as f32) };
    // Relatório completo: gyro, acelerômetro e o primeiro relatório do touchpad. Posições como o
    // driver do Linux (hid-playstation). shortcut: escala nominal (±2000 °/s e 8192 por g), sem a
    // calibração de fábrica do controle (ler ela pede um relatório de recurso); medir com o
    // controle na mão se 90° viram 90°.
    let full = |mut i: Input, o: usize, ds: bool| -> Option<Input> {
        let (g, a, t, h) = if ds { (o + 15, o + 21, o + 32, 1080.0) } else { (o + 12, o + 18, o + 34, 942.0) };
        i.gyro = [le(g)? / 16.384, le(g + 2)? / 16.384, le(g + 4)? / 16.384];
        i.accel = [le(a)? / 8192.0, le(a + 2)? / 8192.0, le(a + 4)? / 8192.0];
        i.motion = true;
        for f in 0..2 {
            let p = t + f * 4;
            let (c, b1, b2, b3) = (get(p)?, get(p + 1)? as u16, get(p + 2)? as u16, get(p + 3)? as u16);
            // Bit de cima ligado: dedo fora.
            if c & 0x80 == 0 {
                let x = b1 | (b2 & 0x0f) << 8;
                let y = (b2 >> 4) | b3 << 4;
                i.touch[f] = Some(((x as f32 / 1920.0).min(1.0), (y as f32 / h).min(1.0)));
            }
        }
        Some(i)
    };
    let at = |o: usize, ds: bool| -> Option<Input> {
        // DualSense no USB e no modo completo: gatilhos logo depois dos analógicos, botões 3 bytes adiante.
        let (bt, tt) = if ds { (o + 7, o + 4) } else { (o + 4, o + 7) };
        Some(Input {
            lx: stick(get(o)?),
            ly: -stick(get(o + 1)?),
            rx: stick(get(o + 2)?),
            ry: -stick(get(o + 3)?),
            // No desenho do DualShock 4 (e no modo simples do Bluetooth) os 6 bits de cima do
            // terceiro byte são um contador, não botões: só o PS e o touchpad valem.
            buttons: sony_buttons(get(bt)?, get(bt + 1)?, get(bt + 2)? & if ds { 0xff } else { 0x03 }),
            lt: get(tt)? as f32 / 255.0,
            rt: get(tt + 1)? as f32 / 255.0,
            ..Default::default()
        })
    };
    // Sem o resto do relatório completo, fica a entrada sem gyro.
    let at_full = |o: usize, ds: bool| -> Option<Input> {
        let i = at(o, ds)?;
        Some(full(i, o, ds).unwrap_or(i))
    };
    match (f, link, *r.first()?) {
        (Family::DualSense, Link::Usb, 0x01) => Some((at_full(1, true)?, Some(ds_battery(get(53)?)))),
        (Family::DualSense, _, 0x31) => Some((at_full(2, true)?, Some(ds_battery(get(54)?)))),
        (Family::DualSense | Family::Ds4, Link::Bluetooth, 0x01) => Some((at(1, false)?, None)),
        (Family::Ds4, Link::Usb, 0x01) => Some((at_full(1, false)?, Some(ds4_battery(get(30)?)))),
        (Family::Ds4, _, 0x11) => Some((at_full(3, false)?, Some(ds4_battery(get(32)?)))),
        _ => None,
    }
}

fn ds_battery(b: u8) -> Battery {
    let level = b & 0x0f;
    match b >> 4 {
        0 => Battery::Percent { pct: (level * 10 + 5).min(100), charging: false },
        1 => Battery::Percent { pct: (level * 10 + 5).min(100), charging: true },
        2 => Battery::Percent { pct: 100, charging: false },
        _ => Battery::Unknown,
    }
}

fn ds4_battery(b: u8) -> Battery {
    let level = b & 0x0f;
    let cable = b & 0x10 != 0;
    let pct = if level >= 10 { 100 } else { level * 10 + 5 };
    Battery::Percent { pct, charging: cable && level < 11 }
}

/// Estatística do intervalo entre relatórios (microssegundos).
pub fn rate_of(gaps: &mut [u32], secs: f32) -> Option<Rate> {
    if gaps.len() < 2 || secs <= 0.0 {
        return None;
    }
    gaps.sort_unstable();
    let avg = gaps.iter().map(|&g| g as f64).sum::<f64>() / gaps.len() as f64;
    Some(Rate {
        hz: gaps.len() as f32 / secs,
        avg_ms: avg as f32 / 1000.0,
        p99_ms: gaps[(gaps.len() * 99 / 100).min(gaps.len() - 1)] as f32 / 1000.0,
        max_ms: gaps[gaps.len() - 1] as f32 / 1000.0,
        counted: false,
    })
}

/// Taxa pelo contador de pacotes do XInput, que sobe um a cada relatório que muda o estado.
/// Com o controle parado ele quase não sobe; só vale quando passa de 20 por segundo (sticks em
/// movimento), medido no Xbox One S por Bluetooth em 2026-11-12: 122/s, igual lendo a 4 kHz.
pub fn packet_rate(steps: u32, secs: f32) -> Option<Rate> {
    let hz = steps as f32 / secs;
    (secs > 0.0 && hz >= 20.0).then_some(Rate { hz, avg_ms: 1000.0 / hz, p99_ms: 0.0, max_ms: 0.0, counted: true })
}

/// Por vaga do XInput: último pacote, saltos somados, começo da janela e a última taxa medida.
static XRATE: Mutex<[(u32, u32, Option<Instant>, Option<Rate>); 4]> = Mutex::new([(0, 0, None, None); 4]);

unsafe fn read_loop(d: Dev) {
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
    use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    let xinput = is_xinput(&d.path);
    let link = link_of(&d.path);
    let fam = family(d.vid, d.pid);
    let h = CreateFileW(&HSTRING::from(d.path.as_str()), FILE_GENERIC_READ.0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_FLAG_OVERLAPPED, None);
    let (Ok(h), Ok(ev)) = (h, CreateEventW(None, true, false, None)) else {
        if let Some(s) = OPEN.lock().unwrap().as_mut() {
            s.remove(&d.path);
        }
        return;
    };
    let mut pre = PHIDP_PREPARSED_DATA::default();
    let generic = (fam == Family::Generic && !xinput && HidD_GetPreparsedData(h, &mut pre)).then(|| generic_axes(pre));
    let name = nice_name(fam, &d.name);
    // O DualSense e o DualShock 4 mandam relatórios sem parar, mesmo parados: 2 s de silêncio é
    // controle desligado. Por Bluetooth o Windows mantém o aparelho HID do controle pareado, e a
    // leitura não dá erro nenhum: sem isso o controle ficava na lista para sempre.
    // shortcut: o genérico pode só mandar relatório quando algo muda, então fica na lista até o
    // Windows tirar o aparelho; um genérico Bluetooth desligado pode ficar, até medir o 8BitDo.
    let streams = matches!(fam, Family::DualSense | Family::Ds4);
    if streams && link == Link::Bluetooth {
        full_mode(&d.path);
    }
    let mut shown = false;
    let show = |on: bool| {
        if on {
            PADS.lock().unwrap().push(Pad {
                key: d.path.clone(),
                name: name.clone(),
                family: fam,
                link,
                vid: d.vid,
                pid: d.pid,
                battery: Battery::Unknown,
                input: Input::default(),
                rate: None,
                generic_buttons: generic.as_ref().map_or(0, |g| g.1),
                low_warned: false,
                jitter_warned: false, gyro_cal: GyroCal::default(),
            });
            emit(Event::Connected { name: name.clone(), link });
        } else {
            PADS.lock().unwrap().retain(|p| p.key != d.path);
            super::remap::forget(&d.path);
            emit(Event::Disconnected { name: name.clone() });
        }
    };
    if !xinput && !streams {
        show(true);
        shown = true;
    }
    let len = d.input_len.max(1) as usize;
    let mut buf = vec![0u8; len.max(64)];
    let mut gaps: Vec<u32> = Vec::with_capacity(2048);
    let (mut prev, mut tick) = (Instant::now(), Instant::now());
    let mut ov = OVERLAPPED { hEvent: ev, ..Default::default() };
    let mut cal = GyroCal::default();
    // Só os modelos da lista de escrita recebem luz, gatilho e vibração.
    let mut light = (streams && WRITE_OK.iter().any(|w| w.0 == d.vid && w.1 == d.pid)).then(|| PadOut {
        path: d.path.clone(),
        fam,
        link,
        out_len: d.output_len as usize,
        sent: Want::default(),
        seq: 0,
        failed: false,
        prev: 0,
        tap: None,
        coil: None,
    });
    let mut pct = None;
    'read: loop {
        let mut got = 0u32;
        if let Err(e) = ReadFile(h, Some(&mut buf[..len]), None, Some(&mut ov)) {
            if e.code() != ERROR_IO_PENDING.to_hresult() {
                break;
            }
            // Espera em fatias de 1 s para notar o silêncio.
            while WaitForSingleObject(ev, 1000) == WAIT_TIMEOUT {
                if shown && streams && prev.elapsed() > Duration::from_secs(2) {
                    show(false);
                    shown = false;
                    gaps.clear();
                }
            }
        }
        if GetOverlappedResult(h, &ov, &mut got, false).is_err() {
            break 'read;
        }
        let now = Instant::now();
        // Depois de um silêncio, o intervalo longo não entra na taxa.
        if now - prev < Duration::from_secs(1) {
            gaps.push((now - prev).as_micros() as u32);
        }
        prev = now;
        let rate = if tick.elapsed() >= Duration::from_secs(2) {
            let r = rate_of(&mut gaps, tick.elapsed().as_secs_f32());
            gaps.clear();
            tick = now;
            r
        } else {
            None
        };
        if xinput {
            if rate.is_some() {
                if let Some(e) = IG.lock().unwrap().iter_mut().find(|e| e.0 == d.path) {
                    e.3 = rate;
                }
            }
            continue;
        }
        let mut parsed = match &generic {
            Some((axes, _)) => Some((generic_input(pre, axes, &mut buf[..got as usize]), None)),
            None => sony(fam, link, &buf[..got as usize]),
        };
        if let Some((inp, _)) = parsed.as_mut().filter(|p| p.0.motion) {
            if CAL_ASK.load(Ordering::Relaxed) {
                cal.ask = true;
            }
            inp.gyro = cal.feed(inp.gyro, now);
            // Pedido atendido por este controle; um segundo controle que ainda mede segue com o dele.
            if !cal.ask {
                CAL_ASK.store(false, Ordering::Relaxed);
            }
        }
        if !shown {
            show(true);
            shown = true;
        }
        let fed = parsed.as_ref().map(|x| x.0);
        let mut pads = PADS.lock().unwrap();
        let Some(p) = pads.iter_mut().find(|p| p.key == d.path) else { break };
        p.gyro_cal = cal;
        if let Some((inp, bat)) = parsed {
            p.input = inp;
            if let Some(b) = bat {
                p.battery = b;
            }
        }
        if let Battery::Percent { pct: v, .. } = p.battery {
            pct = Some(v);
        }
        if rate.is_some() {
            p.rate = rate;
        }
        let events = check_warnings(p);
        drop(pads);
        if let Some(inp) = fed {
            super::remap::feed(&d.path, &inp, now);
        }
        if let Some(l) = light.as_mut() {
            l.update(pct, fed.as_ref());
        }
        events.into_iter().for_each(emit);
    }
    // Uma leitura ainda pendente escreve no buffer: cancela e espera antes de soltar.
    let _ = CancelIoEx(h, Some(&ov));
    let mut got = 0u32;
    let _ = GetOverlappedResult(h, &ov, &mut got, true);
    let _ = CloseHandle(h);
    let _ = CloseHandle(ev);
    if pre.0 != 0 {
        let _ = HidD_FreePreparsedData(pre);
    }
    if let Some(s) = OPEN.lock().unwrap().as_mut() {
        s.remove(&d.path);
    }
    if xinput {
        IG.lock().unwrap().retain(|e| e.0 != d.path);
    } else if shown {
        show(false);
    }
}

/// Bateria baixa uma vez por descarga, e variação alta uma vez até voltar ao normal.
fn check_warnings(p: &mut Pad) -> Vec<Event> {
    let mut out = vec![];
    let low = LOW_PCT.load(Ordering::Relaxed);
    let is_low = match p.battery {
        Battery::Percent { pct, charging } => low > 0 && !charging && pct <= low,
        Battery::Level(l) => low > 0 && l <= 1,
        _ => false,
    };
    let recovered = match p.battery {
        Battery::Percent { pct, charging } => charging || pct > low.saturating_add(5),
        Battery::Level(l) => l >= 2,
        _ => false,
    };
    if is_low && !p.low_warned {
        p.low_warned = true;
        out.push(Event::BatteryLow { name: p.name.clone(), battery: p.battery });
    } else if recovered {
        p.low_warned = false;
    }
    let limit = JITTER_MS.load(Ordering::Relaxed) as f32;
    if let Some(r) = p.rate {
        if limit > 0.0 && r.p99_ms > limit && !p.jitter_warned {
            p.jitter_warned = true;
            out.push(Event::Jitter { name: p.name.clone(), p99_ms: r.p99_ms });
        } else if r.p99_ms <= limit * 0.8 {
            p.jitter_warned = false;
        }
    }
    out
}

// ---------- XInput ----------

type GetState = unsafe extern "system" fn(u32, *mut XINPUT_STATE) -> u32;

/// A função oculta (ordinal 100) lê também o botão Guia; sem ela, a comum.
fn xinput_get() -> Option<GetState> {
    static F: Mutex<Option<Option<GetState>>> = Mutex::new(None);
    *F.lock().unwrap().get_or_insert_with(|| unsafe {
        let m = LoadLibraryW(w!("xinput1_4.dll")).ok()?;
        let f = GetProcAddress(m, PCSTR(100usize as *const u8)).or_else(|| GetProcAddress(m, s!("XInputGetState")))?;
        Some(std::mem::transmute::<unsafe extern "system" fn() -> isize, GetState>(f))
    })
}

fn xinput_input(g: &XINPUT_GAMEPAD) -> Input {
    let w = g.wButtons.0;
    let mut b = 0;
    for (bit, v) in [(0x1, UP), (0x2, DOWN), (0x4, LEFT), (0x8, RIGHT), (0x10, START), (0x20, BACK), (0x40, LS), (0x80, RS), (0x100, LB), (0x200, RB), (0x400, GUIDE), (0x1000, A), (0x2000, B), (0x4000, X), (0x8000, Y)] {
        if w & bit != 0 {
            b |= v;
        }
    }
    let s = |v: i16| (v as f32 / 32767.0).clamp(-1.0, 1.0);
    Input { buttons: b, lx: s(g.sThumbLX), ly: s(g.sThumbLY), rx: s(g.sThumbRX), ry: s(g.sThumbRY), lt: g.bLeftTrigger as f32 / 255.0, rt: g.bRightTrigger as f32 / 255.0, ..Default::default() }
}

unsafe fn poll_xinput() {
    let ig = IG.lock().unwrap().clone();
    let get = if ig.is_empty() { None } else { xinput_get() };
    let mut seen = [false; 4];
    if let Some(get) = get {
        for (i, slot) in seen.iter_mut().enumerate() {
            let mut st = XINPUT_STATE::default();
            if get(i as u32, &mut st) != 0 {
                continue;
            }
            *slot = true;
            let key = format!("xinput:{i}");
            let mut bat = XINPUT_BATTERY_INFORMATION::default();
            let battery = if XInputGetBatteryInformation(i as u32, BATTERY_DEVTYPE_GAMEPAD, &mut bat) == 0 {
                match bat.BatteryType {
                    BATTERY_TYPE_WIRED => Battery::Wired,
                    BATTERY_TYPE_DISCONNECTED | BATTERY_TYPE_UNKNOWN => Battery::Unknown,
                    _ => Battery::Level(bat.BatteryLevel.0),
                }
            } else {
                Battery::Unknown
            };
            // shortcut: com um controle só, o nome, a conexão e a taxa vêm do aparelho HID dele; com
            // vários, o XInput não diz qual lugar é qual, e eles aparecem pelo número.
            let counted = {
                let mut xr = XRATE.lock().unwrap();
                let e = &mut xr[i];
                let now = Instant::now();
                match e.2 {
                    None => *e = (st.dwPacketNumber, 0, Some(now), e.3),
                    Some(t0) => {
                        e.1 = e.1.saturating_add(st.dwPacketNumber.wrapping_sub(e.0));
                        e.0 = st.dwPacketNumber;
                        if t0.elapsed() >= Duration::from_secs(2) {
                            // Parado, guarda a última taxa medida: é a do controle.
                            e.3 = packet_rate(e.1, t0.elapsed().as_secs_f32()).or(e.3);
                            (e.1, e.2) = (0, Some(now));
                        }
                    }
                }
                e.3
            };
            let (name, link, rate) = match ig.as_slice() {
                [(_, n, l, r)] => (n.clone(), *l, r.or(counted)),
                _ => (format!("Controle Xbox {}", i + 1), Link::Usb, counted),
            };
            let now = Instant::now();
            super::remap::feed(&key, &xinput_input(&st.Gamepad), now);
            let mut pads = PADS.lock().unwrap();
            let events = if let Some(p) = pads.iter_mut().find(|p| p.key == key) {
                p.input = xinput_input(&st.Gamepad);
                p.battery = battery;
                p.rate = rate;
                check_warnings(p)
            } else {
                pads.push(Pad {
                    key,
                    name: name.clone(),
                    family: Family::Xbox,
                    link,
                    vid: 0x045e,
                    pid: 0,
                    battery,
                    input: xinput_input(&st.Gamepad),
                    rate,
                    generic_buttons: 0,
                    low_warned: false,
                    jitter_warned: false, gyro_cal: GyroCal::default(),
                });
                vec![Event::Connected { name, link }]
            };
            drop(pads);
            events.into_iter().for_each(emit);
        }
    }
    for (i, s) in seen.iter().enumerate() {
        if !s {
            XRATE.lock().unwrap()[i] = (0, 0, None, None);
        }
    }
    let gone: Vec<(String, String)> = {
        let mut pads = PADS.lock().unwrap();
        let gone: Vec<(String, String)> = pads.iter().filter(|p| p.key.strip_prefix("xinput:").and_then(|n| n.parse::<usize>().ok()).is_some_and(|n| !seen[n.min(3)])).map(|p| (p.key.clone(), p.name.clone())).collect();
        pads.retain(|p| p.key.strip_prefix("xinput:").and_then(|n| n.parse::<usize>().ok()).is_none_or(|n| seen[n.min(3)]));
        gone
    };
    for (key, name) in gone {
        super::remap::forget(&key);
        emit(Event::Disconnected { name });
    }
}

// ---------- Medidas ----------

/// Stick parado: deslocamento do centro (drift) e tremida (desvio padrão), de 0 a 1.
pub fn rest(samples: &[(f32, f32)]) -> Option<(f32, f32)> {
    if samples.len() < 10 {
        return None;
    }
    let n = samples.len() as f32;
    let (mx, my) = samples.iter().fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
    let (mx, my) = (mx / n, my / n);
    let var = samples.iter().map(|(x, y)| (x - mx).powi(2) + (y - my).powi(2)).sum::<f32>() / n;
    Some(((mx * mx + my * my).sqrt(), var.sqrt()))
}

/// Stick girado encostando na borda, em 36 fatias de 10°: quantas fatias foram alcançadas (0 a 1),
/// o alcance médio na borda (1 é a borda inteira) e o quanto o contorno foge de um círculo
/// ((maior − menor) ÷ média, 0 é um círculo perfeito).
pub fn outer(samples: &[(f32, f32)]) -> Option<(f32, f32, f32)> {
    let mut best = [0f32; 36];
    for &(x, y) in samples {
        let r = (x * x + y * y).sqrt();
        if r < 0.5 {
            continue;
        }
        let a = y.atan2(x).to_degrees().rem_euclid(360.0);
        let i = ((a / 10.0) as usize).min(35);
        best[i] = best[i].max(r);
    }
    let hit: Vec<f32> = best.iter().copied().filter(|r| *r > 0.0).collect();
    if hit.len() < 18 {
        return None;
    }
    let mean = hit.iter().sum::<f32>() / hit.len() as f32;
    let (lo, hi) = hit.iter().fold((f32::MAX, 0f32), |(a, b), r| (a.min(*r), b.max(*r)));
    Some((hit.len() as f32 / 36.0, mean.min(1.0), (hi - lo) / mean))
}

/// Gatilho: valor parado (onde começa), o máximo alcançado (onde termina) e a tremida parado.
pub fn trigger(rest: &[f32], pressed: &[f32]) -> Option<(f32, f32, f32)> {
    if rest.len() < 10 || pressed.is_empty() {
        return None;
    }
    let n = rest.len() as f32;
    let m = rest.iter().sum::<f32>() / n;
    let sd = (rest.iter().map(|v| (v - m).powi(2)).sum::<f32>() / n).sqrt();
    let top = pressed.iter().fold(0f32, |a, v| a.max(*v));
    Some((m, top, sd))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dualsense_usb() {
        let mut r = [0u8; 64];
        r[0] = 1;
        r[1..5].copy_from_slice(&[0x7e, 0x6e, 0x80, 0x81]);
        r[5] = 255; // L2 até o fim
        r[8] = 0x28; // Cruz, direcional parado (8)
        r[9] = 0x01; // L1
        r[10] = 0x01; // PS
        r[53] = 0x11; // nível 1, carregando: os 15% medidos na bancada
        let (inp, bat) = sony(Family::DualSense, Link::Usb, &r).unwrap();
        assert_eq!(inp.buttons, A | LB | GUIDE);
        assert!((inp.lt - 1.0).abs() < 1e-6 && inp.rt == 0.0);
        assert!(inp.lx.abs() < 0.02 && inp.ly.abs() < 0.15);
        assert_eq!(bat, Some(Battery::Percent { pct: 15, charging: true }));
    }

    #[test]
    fn ds4_bluetooth_simple_has_no_battery() {
        // O começo real do relatório da bancada: 01 7e 7b 85 82 08 00 00 ...
        let mut r = vec![0u8; 547];
        r[..8].copy_from_slice(&[0x01, 0x7e, 0x7b, 0x85, 0x82, 0x08, 0x00, 0x00]);
        let (inp, bat) = sony(Family::Ds4, Link::Bluetooth, &r).unwrap();
        assert_eq!(inp.buttons, 0);
        assert_eq!(bat, None);
        r[5] = 0x20 | 0x02; // Cruz e direcional para a direita
        r[6] = 0x20; // Options
        r[7] = 0x14; // contador do relatório, visto no DualSense por Bluetooth: não é o Mudo
        assert_eq!(sony(Family::Ds4, Link::Bluetooth, &r).unwrap().0.buttons, A | RIGHT | START);
        // O DualSense no modo simples do Bluetooth (78 bytes) usa o mesmo desenho.
        assert_eq!(sony(Family::DualSense, Link::Bluetooth, &r[..78]).unwrap().0.buttons, A | RIGHT | START);
    }

    #[test]
    fn ds4_usb_battery() {
        let mut r = [0u8; 64];
        r[0] = 1;
        r[5] = 0x08;
        r[30] = 0x1b; // no cabo, nível 11: cheia
        assert_eq!(sony(Family::Ds4, Link::Usb, &r).unwrap().1, Some(Battery::Percent { pct: 100, charging: false }));
        r[30] = 0x03; // fora do cabo, nível 3
        assert_eq!(sony(Family::Ds4, Link::Usb, &r).unwrap().1, Some(Battery::Percent { pct: 35, charging: false }));
    }

    #[test]
    fn dualsense_usb_motion_and_touch() {
        let mut r = [0u8; 64];
        r[0] = 1;
        r[8] = 0x08;
        // Gyro (o + 15 = 16): virar a 100 °/s; acelerômetro (o + 21 = 22): 1 g no eixo y.
        r[18..20].copy_from_slice(&1638i16.to_le_bytes());
        r[24..26].copy_from_slice(&8192i16.to_le_bytes());
        // Touchpad (o + 32 = 33): dedo 1 no meio (960, 540), dedo 2 fora.
        r[33..37].copy_from_slice(&[0x01, 0xc0, 0xc3, 0x21]);
        r[37] = 0x80;
        let (inp, _) = sony(Family::DualSense, Link::Usb, &r).unwrap();
        assert!(inp.motion && (inp.gyro[1] - 100.0).abs() < 0.1 && inp.gyro[0] == 0.0);
        assert!((inp.accel[1] - 1.0).abs() < 1e-3);
        let (x, y) = inp.touch[0].unwrap();
        assert!((x - 0.5).abs() < 0.01 && (y - 0.5).abs() < 0.01, "{x} {y}");
        assert_eq!(inp.touch[1], None);
        // Modo simples do Bluetooth: sem gyro.
        assert!(!sony(Family::Ds4, Link::Bluetooth, &r[..10]).unwrap().0.motion);
    }

    #[test]
    fn gyro_calibrates_when_still() {
        let t0 = Instant::now();
        let mut c = GyroCal::default();
        // Parado com desvio de 1,5 °/s e um tremor pequeno: depois de 1 s o desvio sai.
        for k in 0..=110 {
            let wob = if k % 2 == 0 { 0.4 } else { -0.4 };
            c.feed([0.0, 1.5 + wob, 0.0], t0 + Duration::from_millis(k * 10));
        }
        assert!(c.done && (c.bias[1] - 1.5).abs() < 0.05, "{:?}", c.bias);
        // Movendo de verdade: o desvio não muda.
        for k in 0..200 {
            c.feed([0.0, (k as f32) * 2.0, 0.0], t0 + Duration::from_millis(1110 + k * 10));
        }
        assert!((c.bias[1] - 1.5).abs() < 0.05);
        assert!((c.feed([0.0, 11.5, 0.0], t0 + Duration::from_secs(5))[1] - 10.0).abs() < 0.05);
    }

    #[test]
    fn short_report_is_ignored() {
        assert!(sony(Family::DualSense, Link::Usb, &[1, 2, 3]).is_none());
        assert!(sony(Family::Generic, Link::Usb, &[1; 64]).is_none());
    }

    #[test]
    fn light_reports() {
        let off = [Trig::Pass; 2];
        // CRC-32 de referência: "123456789" dá 0xCBF43926.
        assert_eq!(crc32(b'1', b"23456789"), 0xCBF4_3926);
        let ds = out_report(Family::DualSense, Link::Usb, Some([1, 2, 3]), off, [0, 0], 0).unwrap();
        assert_eq!((ds.len(), ds[0], ds[2], &ds[45..48]), (48, 0x02, 0x04, &[1u8, 2, 3][..]));
        let bt = out_report(Family::DualSense, Link::Bluetooth, Some([1, 2, 3]), off, [0, 0], 5).unwrap();
        assert_eq!((bt.len(), bt[0], bt[1], bt[2], &bt[47..50]), (78, 0x31, 0x50, 0x10, &[1u8, 2, 3][..]));
        assert_eq!(u32::from_le_bytes(bt[74..].try_into().unwrap()), crc32(0xA2, &bt[..74]));
        let ds4 = out_report(Family::Ds4, Link::Usb, Some([1, 2, 3]), off, [0, 0], 0).unwrap();
        assert_eq!((ds4.len(), ds4[0], ds4[1], &ds4[6..9]), (32, 0x05, 0x02, &[1u8, 2, 3][..]));
        let ds4bt = out_report(Family::Ds4, Link::Bluetooth, Some([1, 2, 3]), off, [0, 0], 0).unwrap();
        assert_eq!((ds4bt[0], ds4bt[1], ds4bt[3], &ds4bt[8..11]), (0x11, 0xC0, 0x02, &[1u8, 2, 3][..]));
        assert!(out_report(Family::Xbox, Link::Usb, Some([0; 3]), off, [0, 0], 0).is_none());
        // Sem cor, a luz fica como está: nenhum dos bytes dela é mexido.
        let r = out_report(Family::DualSense, Link::Usb, None, off, [0, 0], 0).unwrap();
        assert_eq!((r[2], r[39], r[42], &r[45..48]), (0, 0, 0, &[0u8, 0, 0][..]));
    }

    #[test]
    fn coil_report_layout() {
        let mut ph = 0.0;
        let r = coil_report(7, &mut ph, 255, &[0; 32]);
        assert_eq!((r[0], r[2], r[3], r[10], r[11], r[12]), (0x32, 0x91, 0x07, 7, 0x92, 64));
        assert_eq!(u32::from_le_bytes(r[138..].try_into().unwrap()), crc32(0xA2, &r[..138]));
        // Força máxima: a onda chega perto de ±127 e os dois canais andam juntos.
        let s: Vec<i8> = r[13..77].iter().map(|&b| b as i8).collect();
        assert!(s.iter().any(|&v| v > 100) && s.iter().any(|&v| v < -100));
        assert!(s.chunks(2).all(|f| f[0] == f[1]));
        // Força zero é silêncio, e o resto do relatório fica zerado.
        let q = coil_report(0, &mut ph, 0, &[0; 32]);
        assert!(q[13..138].iter().all(|&b| b == 0));
        // O som do PC entra como veio, e a soma com a onda não passa de ±127.
        let mut a = [0i8; 32];
        a[0] = -50;
        a[1] = 127;
        let q = coil_report(0, &mut ph, 0, &a);
        assert_eq!((q[13] as i8, q[14] as i8, q[15] as i8), (-50, -50, 127));
        let q = coil_report(0, &mut ph, 255, &[127; 32]);
        assert!(q[13..77].iter().all(|&b| (b as i8) >= -1));
    }

    #[test]
    fn triggers_and_rumble_in_the_report() {
        // Vibração: os dois bits e os dois motores, cada um no seu byte.
        let r = out_report(Family::DualSense, Link::Usb, None, [Trig::Pass; 2], [0x40, 0x80], 0).unwrap();
        assert_eq!((r[1], r[3], r[4]), (0x03, 0x40, 0x80));
        let d4 = out_report(Family::Ds4, Link::Usb, None, [Trig::Pass; 2], [0x40, 0x80], 0).unwrap();
        assert_eq!((d4[1], d4[4], d4[5]), (0x01, 0x40, 0x80));
        // Solto liga o bit e manda o modo 0x05: é o que tira um efeito que já estava no gatilho.
        let r = out_report(Family::DualSense, Link::Usb, None, [Trig::Off, Trig::Pass], [0, 0], 0).unwrap();
        assert_eq!((r[1], r[22]), (0x08, 0x05));
        // L2 resiste da metade do curso, R2 passa ao jogo: só o bit do L2 liga.
        let t = [Trig::Rigid { start: 5, power: 8 }, Trig::Pass];
        let r = out_report(Family::DualSense, Link::Usb, None, t, [0, 0], 0).unwrap();
        assert_eq!((r[1], r[11], r[22]), (0x08, 0x00, 0x21));
        // Zonas 5 a 9 ligadas, com 3 bits de força (8 vira 7) em cada uma.
        assert_eq!(u16::from_le_bytes(r[23..25].try_into().unwrap()), 0b11_1110_0000);
        let f = u32::from_le_bytes(r[25..29].try_into().unwrap());
        assert!((5..10).all(|i| (f >> (3 * i)) & 7 == 7) && (0..5).all(|i| (f >> (3 * i)) & 7 == 0));
        // A arma resiste numa zona e cede na seguinte; a vibração leva a frequência.
        let r = out_report(Family::DualSense, Link::Usb, None, [Trig::Pass, Trig::Click { start: 3, power: 1 }], [0, 0], 0).unwrap();
        assert_eq!((r[1], r[11], u16::from_le_bytes(r[12..14].try_into().unwrap()), r[14]), (0x04, 0x25, 0b1_1000, 0));
        let r = out_report(Family::DualSense, Link::Usb, None, [Trig::Pass, Trig::Buzz { start: 0, power: 4, hz: 60 }], [0, 0], 0).unwrap();
        assert_eq!((r[11], r[20]), (0x26, 40));
    }

    #[test]
    fn battery_colors_slide() {
        use super::super::remap::{battery_color, Batt};
        let d = Batt::default();
        assert_eq!((battery_color(d, 0), battery_color(d, 50), battery_color(d, 100)), ([255, 0, 0], [255, 190, 0], [0, 255, 0]));
        // Passo de 1%: muda a cada por cento, sempre para um lado.
        let v: Vec<[u8; 3]> = (0..=100).map(|p| battery_color(d, p)).collect();
        assert!(v.windows(2).all(|w| w[0] != w[1]));
        assert!(v.windows(2).all(|w| w[1][1] >= w[0][1]));
        // Passo de 20%: só muda de 20 em 20, e as cores escolhidas valem.
        let b = Batt { low: [0, 0, 0], mid: [128, 128, 128], full: [255, 255, 255], step: 20 };
        assert_eq!(battery_color(b, 0), [0, 0, 0]);
        assert_eq!(battery_color(b, 100), [255, 255, 255]);
        assert_eq!(battery_color(b, 52), battery_color(b, 59));
        assert_ne!(battery_color(b, 52), battery_color(b, 72));
    }

    #[test]
    fn rate_stats() {
        let mut g = vec![4000u32; 99];
        g.push(5700);
        let r = rate_of(&mut g, 0.4).unwrap();
        assert_eq!(r.hz, 250.0);
        assert!((r.max_ms - 5.7).abs() < 1e-3 && (r.p99_ms - 5.7).abs() < 1e-3);
        assert!(rate_of(&mut [1], 1.0).is_none());
        // Xbox: 244 saltos em 2 s com os sticks em movimento; parado (10 em 2 s) não conta.
        let x = packet_rate(244, 2.0).unwrap();
        assert!(x.counted && (x.hz - 122.0).abs() < 1e-3);
        assert!(packet_rate(10, 2.0).is_none() && packet_rate(5, 0.0).is_none());
    }

    #[test]
    fn stick_measures() {
        let (drift, noise) = rest(&vec![(0.05, 0.0); 20]).unwrap();
        assert!((drift - 0.05).abs() < 1e-6 && noise < 1e-6);
        let circle: Vec<_> = (0..360).map(|d| ((d as f32).to_radians().cos(), (d as f32).to_radians().sin())).collect();
        let (cover, reach, round) = outer(&circle).unwrap();
        assert!((cover - 1.0).abs() < 1e-6 && reach > 0.99 && round < 0.01);
        // Guia quadrada: na diagonal o stick vai além do círculo.
        let square: Vec<_> = circle.iter().map(|(x, y)| (x / x.abs().max(y.abs()), y / x.abs().max(y.abs()))).collect();
        assert!(outer(&square).unwrap().2 > 0.3);
        assert!(outer(&circle[..90]).is_none(), "um quarto de volta não basta");
    }

    #[test]
    fn trigger_measures() {
        let (start, end, noise) = trigger(&[0.02; 20], &[0.5, 0.98]).unwrap();
        assert!((start - 0.02).abs() < 1e-6 && (end - 0.98).abs() < 1e-6 && noise < 1e-6);
    }

    #[test]
    fn battery_warns_once_per_discharge() {
        LOW_PCT.store(10, Ordering::Relaxed);
        let mut p = Pad {
            key: "k".into(),
            name: "DualSense".into(),
            family: Family::DualSense,
            link: Link::Usb,
            vid: 0,
            pid: 0,
            battery: Battery::Percent { pct: 5, charging: false },
            input: Input::default(),
            rate: None,
            generic_buttons: 0,
            low_warned: false,
            jitter_warned: false, gyro_cal: GyroCal::default(),
        };
        assert_eq!(check_warnings(&mut p).len(), 1);
        assert_eq!(check_warnings(&mut p).len(), 0);
        p.battery = Battery::Percent { pct: 5, charging: true };
        assert_eq!(check_warnings(&mut p).len(), 0);
        p.battery = Battery::Percent { pct: 5, charging: false };
        assert_eq!(check_warnings(&mut p).len(), 1);
    }
}
