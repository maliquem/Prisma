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
/// Botões de trás e extras (8BitDo: L4, R4 e as duas pás). Depois dos virtuais do `remap`.
pub const L4: Buttons = 1 << 40;
pub const R4: Buttons = 1 << 41;
pub const PL: Buttons = 1 << 42;
pub const PR: Buttons = 1 << 43;
/// Botões que vêm do controle (o resto é virtual, do `remap`): até o 17, e do 40 em diante, onde
/// ficam os de trás e os botões 15 a 32 do controle genérico.
pub const PHYSICAL: Buttons = 0x1ffff | !((1 << 40) - 1);

/// Bit do botão `n` (de 0) do controle genérico: 1 a 10 nos botões do desenho comum, 11 a 14 nos
/// de trás (L4, R4, PL, PR) e 15 a 32 do bit 44 em diante.
pub fn generic_bit(n: u8) -> Buttons {
    match n {
        0..=9 => 1 << n,
        10..=13 => L4 << (n - 10),
        14..=31 => 1 << (44 + n as u32 - 14),
        _ => 0,
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Family {
    DualSense,
    Ds4,
    /// Pelo XInput: Xbox e os que se apresentam como Xbox.
    Xbox,
    /// 8BitDo pelo canal do fabricante (modo D): botões com nome, botões de trás, gyro e bateria.
    EightBitDo,
    /// Pro Controller do Switch e os que se passam por ele (o 8BitDo Pro 3 no modo S): relatório
    /// completo 0x30 com gyro, bateria e sticks calibrados.
    Switch,
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
    /// em g. DualSense e DualShock 4 no relatório completo e o 8BitDo no relatório do fabricante;
    /// `motion` diz se veio.
    pub gyro: [f32; 3],
    pub accel: [f32; 3],
    pub motion: bool,
    /// Touchpad: até dois dedos, de 0 a 1 (x da esquerda, y de cima).
    pub touch: [Option<(f32, f32)>; 2],
    /// Controle genérico: cada eixo de 0 a 1, na ordem de `Pad::generic_axes`.
    pub axes: [f32; 8],
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
    /// Eixos do controle genérico, como (página, uso) do HID, até 8; os hats não entram.
    pub generic_axes: Vec<(u16, u16)>,
    low_warned: bool,
    jitter_warned: bool,
    pub gyro_cal: GyroCal,
}

#[derive(Debug)]
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
    (0x2dc8, 0x6009, "8BitDo Pro 3"),
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

/// Quantos botões numerados o editor de perfis oferece para controles genéricos: o maior entre os
/// conectados, ou 32 quando nenhum está.
pub fn generic_buttons() -> u8 {
    list().iter().filter(|p| p.family == Family::Generic).map(|p| p.generic_buttons).max().filter(|n| *n > 0).unwrap_or(32)
}

/// Fotos de teste (`--test-pad=ds|ds4|xbox|8bitdo|switch|generic`, só no build de depuração): um controle de mentira com
/// Cruz/A e R1/RB apertados, o gatilho esquerdo no meio e o stick esquerdo fora do centro.
#[cfg(debug_assertions)]
static FAKE: Mutex<Option<Pad>> = Mutex::new(None);

#[cfg(debug_assertions)]
pub fn fake(kind: &str) {
    let family = match kind {
        "ds" => Family::DualSense,
        "ds4" => Family::Ds4,
        "8bitdo" => Family::EightBitDo,
        "switch" => Family::Switch,
        "generic" => Family::Generic,
        _ => Family::Xbox,
    };
    let extra = if family == Family::EightBitDo { L4 | PR } else { 0 };
    let input = Input { buttons: A | RB | extra, lt: 0.6, lx: 0.6, ly: 0.4, gyro: [24.0, -90.0, 6.0], motion: family != Family::Xbox, touch: [Some((0.3, 0.45)), None], ..Default::default() };
    let pad = Pad { key: "test".into(), name: nice_name(family, ""), family, link: Link::Usb, vid: 0, pid: 0, battery: Battery::Percent { pct: 82, charging: false }, input, rate: None, generic_buttons: 0, generic_axes: vec![], low_warned: false, jitter_warned: false, gyro_cal: GyroCal::default() };
    let pad = if family != Family::Generic {
        pad
    } else {
        // Um manche: 16 botões, X e Y, manete, leme e um eixo de disco.
        let input = Input { buttons: generic_bit(0) | generic_bit(14) | UP, axes: [0.8, 0.3, 0.65, 0.5, 0.1, 0.0, 0.0, 0.0], ..pad.input };
        Pad { generic_buttons: 16, generic_axes: vec![(1, 0x30), (1, 0x31), (2, 0xBB), (2, 0xBA), (1, 0x37)], input, ..pad }
    };
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

/// Só na versão de depuração: com `PRISMA_PAD_TRACE=<arquivo>`, grava os botões que chegam, as
/// teclas e ações que saem e os eventos dos controles, com a hora, para ver o que dispara o quê.
pub fn trace(line: impl FnOnce() -> String) {
    #[cfg(debug_assertions)]
    {
        use std::io::Write;
        static PATH: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        if let Some(p) = PATH.get_or_init(|| std::env::var("PRISMA_PAD_TRACE").ok()) {
            let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                let _ = writeln!(f, "{:02}:{:02}:{:02}.{:03} {}", t.wHour, t.wMinute, t.wSecond, t.wMilliseconds, line());
            }
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = line;
}

pub(crate) fn emit(e: Event) {
    trace(|| format!("evento {e:?}"));
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
        let ms = if super::remap::live() { 4 } else if LIVE.load(Ordering::Relaxed) || super::nav::window_open() { 8 } else { 1000 };
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
/// 8BitDo: ler o relatório de recurso 0x06 (0x30 no Ultimate 3) liga o relatório completo, com
/// gyro e bateria, como o driver do SDL faz ao abrir. O Ultimate 2 e o 2C já mandam sem pedir.
unsafe fn eightbitdo_mode(path: &str, pid: u16) {
    if matches!(pid, 0x6012 | 0x301b) {
        return;
    }
    let Some(h) = open(path, FILE_GENERIC_READ.0) else { return };
    let mut buf = [0u8; 64];
    buf[0] = if pid == 0x202f { 0x30 } else { 0x06 };
    let _ = HidD_GetFeature(h, buf.as_mut_ptr() as *mut _, buf.len() as u32);
    let _ = CloseHandle(h);
}

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
pub(crate) fn crc32(seed: u8, data: &[u8]) -> u32 {
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
/// `setup`: só na primeira cor depois de conectar, encerra a animação azul do começo. Repetido a
/// cada escrita, o DualSense refaz o apagar da luz e ela pisca (visto no Bluetooth).
pub fn out_report(f: Family, link: Link, rgb: Option<[u8; 3]>, trig: [Trig; 2], rumble: [u8; 2], seq: u8, setup: bool) -> Option<Vec<u8>> {
    let (mut r, at) = match (f, link) {
        (Family::DualSense, Link::Usb) => (vec![0u8; 48], 1),
        (Family::DualSense, Link::Bluetooth) => (vec![0u8; 78], 3),
        (Family::Ds4, Link::Usb) => (vec![0u8; 32], 1),
        (Family::Ds4, Link::Bluetooth) => (vec![0u8; 78], 3),
        // 8BitDo: só vibração, relatório 0x05 de 5 bytes (fraco, forte e os dos gatilhos), como o SDL.
        (Family::EightBitDo, _) => return Some(vec![0x05, rumble[0], rumble[1], 0, 0]),
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
            // Liga o controle da luz e põe a cor; a animação azul do começo só se encerra uma vez,
            // como no driver do Linux.
            r[at + 1] = 0x04;
            if setup {
                r[at + 38] = 0x02;
                r[at + 41] = 0x02;
            }
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
/// se ela falhar na hora, pelo pedido de controle do HID. A escrita espera no máximo meio segundo:
/// pelo Bluetooth, um controle que acabou de desligar deixa a escrita parada sem fim, e quem
/// escreve é a mesma linha que lê e nota o controle desligado.
pub(crate) unsafe fn write_report(path: &str, r: &[u8], out_len: usize) -> bool {
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
    use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    trace(|| format!("escreve {:#04x} de {} bytes", r.first().copied().unwrap_or(0), r.len()));
    let Ok(h) = CreateFileW(&HSTRING::from(path), (FILE_GENERIC_READ | FILE_GENERIC_WRITE).0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_FLAG_OVERLAPPED, None) else { return false };
    let Ok(ev) = CreateEventW(None, true, false, None) else {
        let _ = CloseHandle(h);
        return false;
    };
    let mut full = r.to_vec();
    full.resize(out_len.max(r.len()), 0);
    let mut ov = OVERLAPPED { hEvent: ev, ..Default::default() };
    let mut n = 0u32;
    let ok = match WriteFile(h, Some(&full), None, Some(&mut ov)) {
        Ok(()) => true,
        Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
            if WaitForSingleObject(ev, 500) == WAIT_OBJECT_0 {
                GetOverlappedResult(h, &ov, &mut n, false).is_ok()
            } else {
                trace(|| "escrita sem resposta em 500 ms: cancelada".into());
                let _ = CancelIoEx(h, Some(&ov));
                let _ = GetOverlappedResult(h, &ov, &mut n, true);
                false
            }
        }
        Err(_) => HidD_SetOutputReport(h, r.as_ptr() as *const _, r.len() as u32),
    };
    let _ = CloseHandle(ev);
    let _ = CloseHandle(h);
    ok
}

/// Háptico de verdade do DualSense pelo Bluetooth: os atuadores recebem áudio de 3 kHz, 8 bits,
/// estéreo, 32 quadros por relatório 0x32, um relatório a cada 10,67 ms. Formato lido no SAxense
/// (github.com/egormanga/SAxense, MPL-2.0, de Sdore) e conferido no dsz (MaximumFreak/dsz, MIT),
/// que também usa amostras com sinal; nada copiado.
/// `n` é o contador que o controle pede, e a onda é um seno de 170 Hz com a força `level`, somado
/// ao som do PC (`audio`, já em 3 kHz).
fn coil_report(n: u8, phase: &mut f32, level: u8, audio: &[i8; 32]) -> [u8; 142] {
    let mut r = [0u8; 142];
    r[0] = 0x32;
    // Sequência de 0 a 15 na metade alta, como o 0x31 (SAxense e dsz, MIT, concordam).
    r[1] = (n & 15) << 4;
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
            // Acorda pelo `unpark` de `PadOut::update`; o limite só serve para notar o controle que saiu.
            std::thread::park_timeout(Duration::from_millis(500));
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
/// falha só se repete 3 s depois: um controle que recusou uma vez (Bluetooth ocupado) volta sozinho.
struct PadOut {
    path: String,
    fam: Family,
    link: Link,
    out_len: usize,
    sent: Want,
    seq: u8,
    failed: Option<Instant>,
    /// Botões da leitura anterior e até quando dura o toque do último aperto (háptico).
    prev: Buttons,
    tap: Option<(Instant, u8)>,
    /// Atuadores do DualSense pelo Bluetooth, ligados na primeira vez que o háptico pede.
    coil: Option<(Arc<Coil>, std::thread::Thread)>,
    /// A animação azul do começo já foi encerrada nesta conexão (`out_report`, `setup`).
    light_setup: bool,
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
        // Vibração do jogo pela ponte: no DualSense vira háptico (o mesmo caminho do `feel`).
        let mut game = super::xbridge::rumble(&self.path);
        if self.fam == Family::DualSense {
            feel = feel.max(game.0.max(game.1));
            game = (0, 0);
        }
        let mut sound_done = false;
        // DualSense pelo Bluetooth: o háptico vai aos atuadores. Se o controle recusar o
        // relatório, volta ao motor fraco, como no cabo e no DualShock 4.
        if self.fam == Family::DualSense && self.link == Link::Bluetooth {
            if self.coil.is_none() && (feel > 0 || h.audio > 0) {
                let c = Arc::new((AtomicU8::new(0), AtomicBool::new(false), AtomicBool::new(false)));
                let (path, len, weak) = (self.path.clone(), self.out_len, Arc::downgrade(&c));
                if let Ok(t) = std::thread::Builder::new().name("háptico".into()).spawn(move || coil_thread(path, len, weak)) {
                    self.coil = Some((c, t.thread().clone()));
                }
            }
            if let Some((c, t)) = self.coil.as_ref().filter(|(c, _)| !c.1.load(Ordering::Relaxed)) {
                c.0.store(feel, Ordering::Relaxed);
                c.2.store(h.audio > 0, Ordering::Relaxed);
                if feel > 0 || h.audio > 0 {
                    t.unpark();
                }
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
            // O jogo pede (grave, agudo): o agudo vai ao motor fraco e o grave ao forte.
            rumble: { let b = buzzing(); [b[0].max(feel).max(game.1), b[1].max(game.0)] },
        };
        if want == self.sent || self.failed.is_some_and(|t| t.elapsed() < Duration::from_secs(3)) {
            return;
        }
        self.seq = (self.seq + 1) & 15;
        // A luz que o perfil deixou de mandar volta ao padrão nesta mesma escrita.
        let rgb = want.rgb.or_else(|| self.sent.rgb.map(|_| LIGHT_DEFAULT));
        // Gatilho que passa ao jogo depois de um efeito do Prisma: solta uma vez, e daí não mexe mais.
        let trig = [0, 1].map(|i| if want.trig[i] == Trig::Pass && self.sent.trig[i] != Trig::Pass { Trig::Off } else { want.trig[i] });
        let setup = rgb.is_some() && !self.light_setup;
        let Some(r) = out_report(self.fam, self.link, rgb, trig, want.rumble, self.seq, setup) else { return };
        if !write_report(&self.path, &r, self.out_len) {
            self.failed = Some(Instant::now());
            return;
        }
        self.sent = want;
        self.failed = None;
        self.light_setup |= setup;
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
        if let Some(r) = out_report(f, link, lit.then_some(LIGHT_DEFAULT), trig, [0, 0], 0, false) {
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
        // Os do driver do SDL com o relatório novo (Pro 2, Pro 3, Ultimate 2, 2C e 3). SF30 e
        // SN30 Pro têm firmware com outro formato e seguem como genéricos.
        (0x2dc8, 0x6003 | 0x6006 | 0x6009 | 0x6012 | 0x301b | 0x202f) => Family::EightBitDo,
        (0x057e, 0x2009) => Family::Switch,
        _ => Family::Generic,
    }
}

/// Nome curto para a tela e os avisos: os controles Sony se chamam "Wireless Controller" no USB.
fn nice_name(f: Family, name: &str) -> String {
    match f {
        Family::DualSense => "DualSense".into(),
        Family::Ds4 => "DualShock 4".into(),
        Family::EightBitDo if name.is_empty() => "8BitDo".into(),
        Family::Switch if name.is_empty() => "Pro Controller".into(),
        _ if name.is_empty() => "Controle".into(),
        _ => name.to_string(),
    }
}

unsafe fn scan() {
    let devs = devices();
    let serials: Vec<String> = devs.iter().map(|d| d.serial.chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>().to_ascii_lowercase()).collect();
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
    check_bluetooth(&serials);
    let w = WATCH.load(Ordering::Relaxed);
    if w != 0 {
        set_timer(HWND(w as *mut _));
    }
}

/// Controles Bluetooth conectados que o Windows não ligou ao serviço de controle (HID): nenhum
/// programa consegue lê-los assim. Visto num DualSense em 2026-11-12; parear de novo resolve às vezes.
/// shortcut: o endereço é comparado com o número de série HID, que é o endereço nos Sony e Xbox; um
/// controle de outra marca com série diferente apareceria aqui por engano.
unsafe fn check_bluetooth(serials: &[String]) {
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

/// Eixo do HID genérico e o lugar dele no desenho comum.
struct Axis {
    page: u16,
    usage: u16,
    link: u16,
    min: i32,
    max: i32,
    role: Option<Role>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Role {
    Lx,
    Ly,
    Rx,
    Ry,
    Lt,
    Rt,
    /// O primeiro hat vai nas setas; o segundo, no stick direito até o fim.
    Hat,
    Hat2,
}

/// Lugar de cada eixo (página, uso): X e Y no analógico esquerdo, Z e Rz no direito, Rx e Ry nos
/// gatilhos (a ordem mais comum no DirectInput). Volante e manche ocupam o que sobrou: o volante no
/// stick esquerdo, acelerador e manete no gatilho direito, freio no esquerdo, leme no stick direito.
/// shortcut: um controle com outra ordem (pedais em Y e Rz, por exemplo) aparece trocado nos
/// sticks; todos os eixos aparecem crus na lista do teste e o perfil pode ler os botões.
fn roles(axes: &[(u16, u16)]) -> Vec<Option<Role>> {
    use Role::*;
    let mut out: Vec<Option<Role>> = axes
        .iter()
        .map(|a| match a {
            (1, 0x30) => Some(Lx),
            (1, 0x31) => Some(Ly),
            (1, 0x32) => Some(Rx),
            (1, 0x35) => Some(Ry),
            (1, 0x33) => Some(Lt),
            (1, 0x34) => Some(Rt),
            _ => None,
        })
        .collect();
    let mut hats = 0;
    for (i, a) in axes.iter().enumerate() {
        if *a == (1, 0x39) {
            out[i] = [Some(Hat), Some(Hat2)].get(hats).copied().flatten();
            hats += 1;
        }
    }
    let spare: [((u16, u16), &[Role]); 7] = [
        ((2, 0xC8), &[Lx]),
        ((2, 0xC4), &[Rt]),
        ((2, 0xC5), &[Lt]),
        ((2, 0xBB), &[Rt, Lt]),
        ((1, 0x36), &[Rt, Lt]),
        ((1, 0x37), &[Lt, Rt]),
        ((2, 0xBA), &[Rx]),
    ];
    for (usage, want) in spare {
        for i in 0..axes.len() {
            if axes[i] == usage && out[i].is_none() {
                out[i] = want.iter().copied().find(|w| !out.contains(&Some(*w)));
            }
        }
    }
    out
}

unsafe fn generic_axes(pre: PHIDP_PREPARSED_DATA) -> (Vec<Axis>, u8) {
    let mut caps = HIDP_CAPS::default();
    let _ = HidP_GetCaps(pre, &mut caps);
    let mut n = caps.NumberInputValueCaps;
    let mut vc = vec![HIDP_VALUE_CAPS::default(); n as usize];
    let mut axes = vec![];
    if n > 0 && HidP_GetValueCaps(HidP_Input, vc.as_mut_ptr(), &mut n, pre).is_ok() {
        for v in &vc[..n as usize] {
            // Generic Desktop (sticks, hats) e Simulation (volante, pedais, leme, manete).
            if (v.UsagePage == 1 || v.UsagePage == 2) && !v.IsRange {
                axes.push(Axis { page: v.UsagePage, usage: v.Anonymous.NotRange.Usage, link: v.LinkCollection, min: v.LogicalMin, max: v.LogicalMax, role: None });
            }
        }
    }
    let r = roles(&axes.iter().map(|a| (a.page, a.usage)).collect::<Vec<_>>());
    for (a, r) in axes.iter_mut().zip(r) {
        a.role = r;
    }
    let buttons = HidP_MaxUsageListLength(HidP_Input, Some(9), pre).min(32) as u8;
    (axes, buttons)
}

/// Eixos que a lista do teste mostra: todos menos os hats, até 8.
fn raw_axes(axes: &[Axis]) -> impl Iterator<Item = &Axis> {
    axes.iter().filter(|a| (a.page, a.usage) != (1, 0x39)).take(8)
}

/// Relatório de um controle genérico: botões numerados (`generic_bit`), eixos pelo `roles` e cada
/// eixo cru de 0 a 1 em `Input::axes`.
unsafe fn generic_input(pre: PHIDP_PREPARSED_DATA, axes: &[Axis], report: &mut [u8]) -> Input {
    let mut inp = Input::default();
    let mut usages = [0u16; 32];
    let mut n = usages.len() as u32;
    if HidP_GetUsages(HidP_Input, 9, None, usages.as_mut_ptr(), &mut n, pre, report).is_ok() {
        for u in &usages[..n as usize] {
            if (1..=32).contains(u) {
                inp.buttons |= generic_bit((u - 1) as u8);
            }
        }
    }
    let mut raw = 0;
    for a in axes {
        let mut v = 0u32;
        if HidP_GetUsageValue(HidP_Input, a.page, Some(a.link), a.usage, &mut v, pre, report).is_err() {
            continue;
        }
        let span = (a.max - a.min).max(1) as f32;
        let unit = ((v as i32 - a.min) as f32 / span).clamp(0.0, 1.0);
        let stick = unit * 2.0 - 1.0;
        if (a.page, a.usage) != (1, 0x39) && raw < 8 {
            inp.axes[raw] = unit;
            raw += 1;
        }
        let hat = || hat_bits(u8::try_from(v as i32 - a.min).unwrap_or(u8::MAX));
        match a.role {
            Some(Role::Lx) => inp.lx = stick,
            Some(Role::Ly) => inp.ly = -stick,
            Some(Role::Rx) => inp.rx = stick,
            Some(Role::Ry) => inp.ry = -stick,
            Some(Role::Lt) => inp.lt = unit,
            Some(Role::Rt) => inp.rt = unit,
            // O direcional não colide com os botões numerados: o 11 em diante fica nos bits altos.
            Some(Role::Hat) => inp.buttons |= hat(),
            Some(Role::Hat2) => {
                let h = hat();
                if h != 0 {
                    let s = |plus: Buttons, minus: Buttons| (h & plus != 0) as i32 as f32 - (h & minus != 0) as i32 as f32;
                    (inp.rx, inp.ry) = (s(RIGHT, LEFT), s(UP, DOWN));
                }
            }
            None => {}
        }
    }
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
        // No Bluetooth, o 0x31 também traz outros pacotes: com o alto-falante tocando, o controle
        // manda alguns com a marca 2 na metade baixa do byte 1, e lidos como entrada viram botões
        // aleatórios. A entrada de verdade tem a marca 1.
        (Family::DualSense, _, 0x31) if get(1)? & 0x0f == 0x01 => Some((at_full(2, true)?, Some(ds_battery(get(54)?)))),
        (Family::DualSense | Family::Ds4, Link::Bluetooth, 0x01) => Some((at(1, false)?, None)),
        (Family::Ds4, Link::Usb, 0x01) => Some((at_full(1, false)?, Some(ds4_battery(get(30)?)))),
        (Family::Ds4, _, 0x11) => Some((at_full(3, false)?, Some(ds4_battery(get(32)?)))),
        _ => None,
    }
}

/// Relatório do 8BitDo (driver do SDL, zlib): 0x04 no USB e no Pro 3 pelo Bluetooth, 0x01 no
/// Bluetooth dos outros, 0x03 no firmware sem o modo completo. Byte 1 direcional; 2 a 5 sticks; 6 RT
/// e 7 LT; 8 a 10 botões; 14 bateria (bit 7 carregando); 15 a 26 acelerômetro e gyro (x, y, z),
/// ±2000 °/s e 4096 por g, com os eixos do controle virados para os do DualSense.
pub fn eightbitdo(pid: u16, r: &[u8]) -> Option<(Input, Option<Battery>)> {
    if !matches!(r.first()?, 0x01 | 0x03 | 0x04) || r.len() < 10 {
        return None;
    }
    let mut b = hat_bits(r[1]);
    // O Ultimate 2C troca as pás pelos L4 e R4 nos mesmos bits.
    let (p4, p20) = if pid == 0x301b { (L4, R4) } else { (PR, PL) };
    for (bit, v) in [(0x01, A), (0x02, B), (0x08, X), (0x10, Y), (0x40, LB), (0x80, RB), (0x04, p4), (0x20, p20)] {
        if r[8] & bit != 0 {
            b |= v;
        }
    }
    for (bit, v) in [(0x10, GUIDE), (0x04, BACK), (0x08, START), (0x20, LS), (0x40, RS)] {
        if r[9] & bit != 0 {
            b |= v;
        }
    }
    if let Some(&x) = r.get(10) {
        b |= if x & 0x01 != 0 { L4 } else { 0 } | if x & 0x02 != 0 { R4 } else { 0 };
    }
    let mut i = Input { buttons: b, lx: stick(r[2]), ly: -stick(r[3]), rx: stick(r[4]), ry: -stick(r[5]), rt: r[6] as f32 / 255.0, lt: r[7] as f32 / 255.0, ..Default::default() };
    let le = |at: usize| Some(i16::from_le_bytes([*r.get(at)?, *r.get(at + 1)?]) as f32);
    if let (Some(ax), Some(ay), Some(az), Some(gx), Some(gy), Some(gz)) = (le(15), le(17), le(19), le(21), le(23), le(25)) {
        let dps = 2000.0 / 32767.0;
        i.gyro = [-gy * dps, gz * dps, -gx * dps];
        i.accel = [-ay / 4096.0, az / 4096.0, -ax / 4096.0];
        i.motion = true;
    }
    let bat = r.get(14).filter(|_| pid != 0x301b).map(|&v| {
        let pct = (v & 0x7f).min(100);
        Battery::Percent { pct, charging: v & 0x80 != 0 && pct < 100 }
    });
    Some((i, bat))
}

/// Calibração dos sticks do Pro Controller (eixos lx, ly, rx, ry): centro e o alcance de cada lado,
/// que cresce quando o stick passa dele, como no driver do SDL.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct StickCal {
    center: [f32; 4],
    lo: [f32; 4],
    hi: [f32; 4],
}

impl Default for StickCal {
    fn default() -> Self {
        // Sem calibração lida: centro 2048 e 70% de 70% do meio, como o SDL faz com 0xFFF.
        StickCal { center: [2048.0; 4], lo: [2048.0 * 0.49; 4], hi: [2048.0 * 0.49; 4] }
    }
}

impl StickCal {
    /// Calibração de fábrica (18 bytes do endereço 0x603D da memória do controle): valores de 12
    /// bits empacotados, em ordens diferentes nos dois sticks. 0xFFF é campo vazio.
    fn from_spi(b: &[u8]) -> StickCal {
        let mut c = StickCal::default();
        if b.len() < 18 {
            return c;
        }
        let six = |p: &[u8]| {
            let (lo, hi) = (|i: usize| ((p[i + 1] as u16) << 8 & 0xF00) | p[i] as u16, |i: usize| (p[i + 2] as u16) << 4 | (p[i + 1] as u16) >> 4);
            [lo(0), hi(0), lo(3), hi(3), lo(6), hi(6)]
        };
        let (l, r) = (six(&b[..9]), six(&b[9..18]));
        // Esquerdo: máx x, máx y, centro x, centro y, mín x, mín y. Direito: centro, mín, máx.
        for (axis, (center, min, max)) in [(0, (l[2], l[4], l[0])), (1, (l[3], l[5], l[1])), (2, (r[0], r[2], r[4])), (3, (r[1], r[3], r[5]))] {
            let ctr = if center == 0xFFF { 2048.0 } else { center as f32 };
            let side = |v: u16| if v == 0xFFF { ctr * 0.7 } else { v as f32 } * 0.7;
            c.center[axis] = ctr;
            c.lo[axis] = side(min);
            c.hi[axis] = side(max);
        }
        c
    }

    fn axis(&mut self, axis: usize, raw: u16) -> f32 {
        let v = raw as f32 - self.center[axis];
        if v >= 0.0 {
            self.hi[axis] = self.hi[axis].max(v);
            v / self.hi[axis].max(1.0)
        } else {
            self.lo[axis] = self.lo[axis].max(-v);
            v / self.lo[axis].max(1.0)
        }
    }
}

/// Pro Controller pelo Bluetooth: liga o relatório completo (0x30) com gyro e lê a calibração dos
/// sticks, pelos subcomandos do relatório de saída 0x01 (protocolo lido no driver do SDL, zlib;
/// nada copiado). Cada subcomando espera a resposta 0x21 por até 100 ms, três vezes. Só a
/// calibração de fábrica é lida.
/// shortcut: a calibração do usuário (0x8010) fica de fora; ler se algum controle vier descalibrado.
unsafe fn switch_setup(path: &str, in_len: usize, out_len: usize) -> StickCal {
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
    use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    let mut cal = StickCal::default();
    let Ok(h) = CreateFileW(&HSTRING::from(path), (FILE_GENERIC_READ | FILE_GENERIC_WRITE).0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_FLAG_OVERLAPPED, None) else { return cal };
    let Ok(ev) = CreateEventW(None, true, false, None) else {
        let _ = CloseHandle(h);
        return cal;
    };
    // Uma leitura ou escrita com prazo; devolve os bytes, ou None se não veio a tempo.
    let io = |write: bool, buf: &mut [u8], ms: u32| -> Option<usize> {
        let mut ov = OVERLAPPED { hEvent: ev, ..Default::default() };
        let r = if write { WriteFile(h, Some(buf), None, Some(&mut ov)) } else { ReadFile(h, Some(buf), None, Some(&mut ov)) };
        if let Err(e) = r {
            if e.code() != ERROR_IO_PENDING.to_hresult() {
                return None;
            }
        }
        let mut n = 0u32;
        if WaitForSingleObject(ev, ms) != WAIT_OBJECT_0 {
            let _ = CancelIoEx(h, Some(&ov));
            let _ = GetOverlappedResult(h, &ov, &mut n, true);
            return None;
        }
        GetOverlappedResult(h, &ov, &mut n, false).ok().map(|_| n as usize)
    };
    let mut seq = 0u8;
    let mut ask = |id: u8, args: &[u8]| -> Option<Vec<u8>> {
        let mut inbuf = vec![0u8; in_len.max(64)];
        for _ in 0..3 {
            let mut out = vec![0u8; out_len.max(49)];
            out[0] = 0x01;
            out[1] = seq & 0x0F;
            seq = seq.wrapping_add(1);
            // Vibração parada nos dois motores.
            out[2..10].copy_from_slice(&[0x00, 0x01, 0x40, 0x40, 0x00, 0x01, 0x40, 0x40]);
            out[10] = id;
            out[11..11 + args.len()].copy_from_slice(args);
            io(true, &mut out, 500)?;
            let until = Instant::now() + Duration::from_millis(100);
            while let Some(left) = until.checked_duration_since(Instant::now()) {
                match io(false, &mut inbuf, left.as_millis() as u32 + 1) {
                    Some(n) if n > 15 && inbuf[0] == 0x21 && inbuf[14] == id => return Some(inbuf[15..n].to_vec()),
                    Some(_) => {}
                    None => break,
                }
            }
        }
        trace(|| format!("Pro Controller: subcomando {id:#04x} sem resposta"));
        None
    };
    std::thread::sleep(Duration::from_millis(250));
    // Resposta da leitura da memória: endereço (4 bytes) e tamanho (1) ecoados, depois os dados.
    if let Some(r) = ask(0x10, &[0x3D, 0x60, 0, 0, 18]).filter(|r| r.len() >= 23 && r[..4] == [0x3D, 0x60, 0, 0]) {
        cal = StickCal::from_spi(&r[5..23]);
    }
    ask(0x48, &[1]); // vibração ligada
    ask(0x03, &[0x30]); // relatório completo
    ask(0x40, &[1]); // gyro e acelerômetro
    ask(0x30, &[0x01]); // primeira luz de jogador
    let _ = CloseHandle(ev);
    let _ = CloseHandle(h);
    cal
}

/// Relatório do Pro Controller: 0x30 completo (e 0x21, a resposta de subcomando, com o mesmo
/// começo) ou 0x3F simples, o de antes de pedir o completo. Letras na posição da Nintendo: o B
/// de baixo vira o A do Prisma. ZL e ZR são digitais.
pub fn switch_pro(cal: &mut StickCal, r: &[u8]) -> Option<(Input, Option<Battery>)> {
    let set = |byte: u8, map: &[(u8, Buttons)]| map.iter().filter(|m| byte & m.0 != 0).fold(0, |b, m| b | m.1);
    let trig = |on: bool| if on { 1.0 } else { 0.0 };
    match *r.first()? {
        0x30 | 0x21 if r.len() >= 12 => {
            let b = set(r[3], &[(0x01, X), (0x02, Y), (0x04, A), (0x08, B), (0x40, RB)])
                | set(r[4], &[(0x01, BACK), (0x02, START), (0x04, RS), (0x08, LS), (0x10, GUIDE), (0x20, PAD)])
                | set(r[5], &[(0x01, DOWN), (0x02, UP), (0x04, RIGHT), (0x08, LEFT), (0x40, LB)]);
            let x = |p: usize| r[p] as u16 | (r[p + 1] as u16 & 0xF) << 8;
            let y = |p: usize| (r[p + 1] as u16) >> 4 | (r[p + 2] as u16) << 4;
            // Y do Switch já cresce para cima, como no Prisma.
            let mut i = Input { buttons: b, lx: cal.axis(0, x(6)), ly: cal.axis(1, y(6)), rx: cal.axis(2, x(9)), ry: cal.axis(3, y(9)), lt: trig(r[5] & 0x80 != 0), rt: trig(r[3] & 0x80 != 0), ..Default::default() };
            for v in [&mut i.lx, &mut i.ly, &mut i.rx, &mut i.ry] {
                *v = v.clamp(-1.0, 1.0);
            }
            // Três amostras de 12 bytes a partir do 13; a primeira é a mais nova.
            if r[0] == 0x30 && r.len() >= 25 {
                let le = |at: usize| i16::from_le_bytes([r[at], r[at + 1]]) as f32;
                let (ax, ay, az, gx, gy, gz) = (le(13), le(15), le(17), le(19), le(21), le(23));
                if ax != 0.0 || ay != 0.0 || az != 0.0 {
                    let dps = 1.0 / 14.2842;
                    i.gyro = [-gy * dps, gz * dps, -gx * dps];
                    i.accel = [-ay / 4096.0, az / 4096.0, -ax / 4096.0];
                    i.motion = true;
                }
            }
            let level = (r[2] & 0xE0) >> 4;
            let bat = Battery::Percent { pct: (level as u16 * 100 / 8).min(100) as u8, charging: r[2] & 0x10 != 0 && level < 8 };
            Some((i, Some(bat)))
        }
        0x3F if r.len() >= 12 => {
            let b = hat_bits(r[3])
                | set(r[1], &[(0x01, A), (0x02, B), (0x04, X), (0x08, Y), (0x10, LB), (0x20, RB)])
                | set(r[2], &[(0x01, BACK), (0x02, START), (0x04, LS), (0x08, RS), (0x10, GUIDE), (0x20, PAD)]);
            // shortcut: alcance fixo de meio curso, sem crescer; dura só até o relatório completo.
            let s = |p: usize| ((u16::from_le_bytes([r[p], r[p + 1]]) as f32 - 32768.0) / 16384.0).clamp(-1.0, 1.0);
            Some((Input { buttons: b, lx: s(4), ly: -s(6), rx: s(8), ry: -s(10), lt: trig(r[1] & 0x40 != 0), rt: trig(r[1] & 0x80 != 0), ..Default::default() }, None))
        }
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
    let streams = matches!(fam, Family::DualSense | Family::Ds4 | Family::EightBitDo | Family::Switch);
    let mut sticks = StickCal::default();
    if fam == Family::EightBitDo {
        eightbitdo_mode(&d.path, d.pid);
    } else if fam == Family::Switch {
        sticks = switch_setup(&d.path, d.input_len as usize, d.output_len as usize);
    } else if streams && link == Link::Bluetooth {
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
                generic_axes: generic.as_ref().map_or(vec![], |g| raw_axes(&g.0).map(|a| (a.page, a.usage)).collect()),
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
        failed: None,
        prev: 0,
        tap: None,
        coil: None,
        light_setup: false,
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
            None if fam == Family::EightBitDo => eightbitdo(d.pid, &buf[..got as usize]),
            None if fam == Family::Switch => switch_pro(&mut sticks, &buf[..got as usize]),
            None => sony(fam, link, &buf[..got as usize]),
        };
        #[cfg(debug_assertions)]
        {
            let r = &buf[..got as usize];
            let b = parsed.as_ref().map(|p| p.0.buttons);
            trace(|| format!("{} relatório {:#04x} de {} bytes, botões {:?}, começo {:02x?}", d.name, r.first().copied().unwrap_or(0), r.len(), b.map(|x| format!("{x:#x}")), &r[..r.len().min(16)]));
        }
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

/// Bateria baixa uma vez por descarga, e variação alta uma vez por conexão (no Bluetooth ela oscila
/// em volta do limite e o aviso voltava a cada janela de 10 s).
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
            // O Xbox virtual da ponte não é controle de verdade: lido, voltaria para ela mesma.
            if super::xbridge::own_slot(i) || get(i as u32, &mut st) != 0 {
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
                    generic_axes: vec![],
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
    fn generic_roles() {
        use Role::*;
        // Gamepad: o mapeamento direto, dois hats.
        let pad = [(1, 0x30), (1, 0x31), (1, 0x32), (1, 0x35), (1, 0x39), (1, 0x39), (1, 0x39)];
        assert_eq!(roles(&pad), [Some(Lx), Some(Ly), Some(Rx), Some(Ry), Some(Hat), Some(Hat2), None]);
        // Volante: direção no stick esquerdo, acelerador e freio nos gatilhos, embreagem só crua.
        let wheel = [(2, 0xC8), (2, 0xC4), (2, 0xC5), (2, 0xC6)];
        assert_eq!(roles(&wheel), [Some(Lx), Some(Rt), Some(Lt), None]);
        // Manche: manete no gatilho livre, leme no stick direito, dois deslizantes dividem os gatilhos.
        let stick = [(1, 0x30), (1, 0x31), (1, 0x33), (2, 0xBB), (2, 0xBA), (1, 0x36), (1, 0x36)];
        assert_eq!(roles(&stick), [Some(Lx), Some(Ly), Some(Lt), Some(Rt), Some(Rx), None, None]);
        assert_eq!(roles(&[(1, 0x36), (1, 0x36)]), [Some(Rt), Some(Lt)]);
    }

    #[test]
    fn generic_bits() {
        assert_eq!(generic_bit(0), A);
        assert_eq!(generic_bit(9), RS);
        assert_eq!(generic_bit(10), L4);
        assert_eq!(generic_bit(13), PR);
        assert_eq!(generic_bit(31), 1 << 61);
        assert_eq!(generic_bit(32), 0);
        let all = (0..32).fold(0, |b, n| b | generic_bit(n));
        assert_eq!(all.count_ones(), 32);
        assert_eq!(all & (UP | DOWN | LEFT | RIGHT), 0);
        assert_eq!(all & !PHYSICAL, 0);
    }

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

    /// Começos reais vistos com o alto-falante tocando pelo Bluetooth (rastro de 2026-11-13): a
    /// entrada (marca 1) vale; o outro pacote (marca 2) não vira botão nem bateria.
    #[test]
    fn eightbitdo_pro3_report() {
        // Relatório 0x04 do Pro 3 pelo Bluetooth, parado na mesa, com a bateria cheia.
        let mut r = [0x04, 0x0f, 0x80, 0x7f, 0x80, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x09, 0x60, 0x64, 0x5a, 0xfc, 0x68, 0x00, 0x7b, 0x0f, 0xfa, 0xff, 0x06, 0x00, 0xff, 0xff, 0x6d, 0xcc, 0xe8, 0x05, 0x05];
        let (i, bat) = eightbitdo(0x6009, &r).unwrap();
        assert_eq!((i.buttons, bat), (0, Some(Battery::Percent { pct: 100, charging: false })));
        assert!(i.motion && i.lx.abs() < 0.01 && i.ly.abs() < 0.01);
        // Parado: perto de 1 g no total, gyro perto de zero.
        let g = (i.accel[0].powi(2) + i.accel[1].powi(2) + i.accel[2].powi(2)).sqrt();
        assert!((g - 1.0).abs() < 0.15, "{g}");
        assert!(i.gyro.iter().all(|v| v.abs() < 1.0), "{:?}", i.gyro);
        // Direcional para baixo, B (embaixo), L, as pás, Home e o L4.
        (r[1], r[8], r[9], r[10], r[14], r[6]) = (4, 0x01 | 0x40 | 0x04 | 0x20, 0x10, 0x01, 0x80 | 40, 255);
        let (i, bat) = eightbitdo(0x6009, &r).unwrap();
        assert_eq!(i.buttons, DOWN | A | LB | PR | PL | GUIDE | L4);
        assert_eq!((i.rt, bat), (1.0, Some(Battery::Percent { pct: 40, charging: true })));
        // O 2C não tem as pás: os mesmos bits são L4 e R4, e não informa bateria.
        r[10] = 0;
        let (i, bat) = eightbitdo(0x301b, &r[..11]).unwrap();
        assert_eq!((i.buttons & (L4 | R4 | PL | PR), bat), (L4 | R4, None));
        assert!(eightbitdo(0x6009, &[0x02; 32]).is_none());
    }

    #[test]
    fn switch_pro_report() {
        // Calibração de fábrica: centro 2000 e 1500 para cada lado nos quatro eixos.
        let pack = |a: u16, b: u16| [a as u8, (a >> 8) as u8 & 0x0F | (b << 4) as u8, (b >> 4) as u8];
        let (c, s) = (2000u16, 1500u16);
        let spi: Vec<u8> = [pack(s, s), pack(c, c), pack(s, s), pack(c, c), pack(s, s), pack(s, s)].concat();
        let mut cal = StickCal::from_spi(&spi);
        assert_eq!((cal.center, cal.hi[0]), ([2000.0; 4], 1050.0));
        // 0x30 com o stick esquerdo todo à direita e para cima, B (embaixo), ZR, + e Home.
        let mut r = [0u8; 49];
        r[0] = 0x30;
        r[2] = 0x50; // nível 4 de 8, carregando
        (r[3], r[4], r[5]) = (0x04 | 0x80, 0x02 | 0x10, 0x02);
        r[6..9].copy_from_slice(&pack(3100, 3100));
        r[9..12].copy_from_slice(&pack(2000, 2000));
        r[18] = 0x10; // acelerômetro z = 4096: 1 g
        let (i, bat) = switch_pro(&mut cal, &r).unwrap();
        assert_eq!(i.buttons, A | START | GUIDE | UP);
        assert_eq!((i.lx, i.ly, i.rx, i.rt, i.lt), (1.0, 1.0, 0.0, 1.0, 0.0));
        assert_eq!(cal.hi[0], 1100.0); // o alcance cresceu
        assert_eq!(bat, Some(Battery::Percent { pct: 50, charging: true }));
        assert!(i.motion && (i.accel[1] - 1.0).abs() < 1e-6);
        // 0x3F simples: hat para a esquerda, A (à direita) e o stick esquerdo para baixo.
        let mut s = [0u8; 12];
        s[0] = 0x3F;
        (s[1], s[3]) = (0x02, 6);
        s[4..12].copy_from_slice(&[0, 0x80, 0xFF, 0xFF, 0, 0x80, 0, 0x80]);
        let (i, _) = switch_pro(&mut cal, &s).unwrap();
        assert_eq!((i.buttons, i.lx, i.ly), (B | LEFT, 0.0, -1.0));
        assert!(switch_pro(&mut cal, &[0x31; 49]).is_none());
    }

    #[test]
    fn dualsense_bt_ignores_non_input_0x31() {
        let mut r = vec![0u8; 78];
        r[..14].copy_from_slice(&[0x31, 0x51, 0x81, 0x7b, 0x7f, 0x7b, 0x00, 0x00, 0x01, 0x08, 0x00, 0x00, 0x00, 0x03]);
        assert_eq!(sony(Family::DualSense, Link::Bluetooth, &r).unwrap().0.buttons, 0);
        r[..16].copy_from_slice(&[0x31, 0x32, 0x00, 0xd4, 0x7d, 0x7e, 0xc4, 0xf5, 0x1f, 0x09, 0x86, 0x59, 0xa5, 0x64, 0x29, 0x7d]);
        assert!(sony(Family::DualSense, Link::Bluetooth, &r).is_none());
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
        let ds = out_report(Family::DualSense, Link::Usb, Some([1, 2, 3]), off, [0, 0], 0, true).unwrap();
        assert_eq!((ds.len(), ds[0], ds[2], &ds[45..48]), (48, 0x02, 0x04, &[1u8, 2, 3][..]));
        let bt = out_report(Family::DualSense, Link::Bluetooth, Some([1, 2, 3]), off, [0, 0], 5, true).unwrap();
        assert_eq!((bt.len(), bt[0], bt[1], bt[2], &bt[47..50]), (78, 0x31, 0x50, 0x10, &[1u8, 2, 3][..]));
        assert_eq!(u32::from_le_bytes(bt[74..].try_into().unwrap()), crc32(0xA2, &bt[..74]));
        // A animação azul do começo só se encerra na primeira cor: repetida, a luz pisca.
        assert_eq!((bt[3 + 38], bt[3 + 41]), (0x02, 0x02));
        let again = out_report(Family::DualSense, Link::Bluetooth, Some([1, 2, 3]), off, [0, 0], 6, false).unwrap();
        assert_eq!((again[3 + 1], again[3 + 38], again[3 + 41], &again[47..50]), (0x04, 0, 0, &[1u8, 2, 3][..]));
        let ds4 = out_report(Family::Ds4, Link::Usb, Some([1, 2, 3]), off, [0, 0], 0, true).unwrap();
        assert_eq!((ds4.len(), ds4[0], ds4[1], &ds4[6..9]), (32, 0x05, 0x02, &[1u8, 2, 3][..]));
        let ds4bt = out_report(Family::Ds4, Link::Bluetooth, Some([1, 2, 3]), off, [0, 0], 0, true).unwrap();
        assert_eq!((ds4bt[0], ds4bt[1], ds4bt[3], &ds4bt[8..11]), (0x11, 0xC0, 0x02, &[1u8, 2, 3][..]));
        assert!(out_report(Family::Xbox, Link::Usb, Some([0; 3]), off, [0, 0], 0, true).is_none());
        // Sem cor, a luz fica como está: nenhum dos bytes dela é mexido.
        let r = out_report(Family::DualSense, Link::Usb, None, off, [0, 0], 0, true).unwrap();
        assert_eq!((r[2], r[39], r[42], &r[45..48]), (0, 0, 0, &[0u8, 0, 0][..]));
    }

    #[test]
    fn coil_report_layout() {
        let mut ph = 0.0;
        let r = coil_report(7, &mut ph, 255, &[0; 32]);
        assert_eq!((r[0], r[1], r[2], r[3], r[10], r[11], r[12]), (0x32, 0x70, 0x91, 0x07, 7, 0x92, 64));
        assert_eq!(coil_report(18, &mut 0.0, 0, &[0; 32])[1], 0x20);
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
        let r = out_report(Family::DualSense, Link::Usb, None, [Trig::Pass; 2], [0x40, 0x80], 0, true).unwrap();
        assert_eq!((r[1], r[3], r[4]), (0x03, 0x40, 0x80));
        let d4 = out_report(Family::Ds4, Link::Usb, None, [Trig::Pass; 2], [0x40, 0x80], 0, true).unwrap();
        assert_eq!((d4[1], d4[4], d4[5]), (0x01, 0x40, 0x80));
        // Solto liga o bit e manda o modo 0x05: é o que tira um efeito que já estava no gatilho.
        let r = out_report(Family::DualSense, Link::Usb, None, [Trig::Off, Trig::Pass], [0, 0], 0, true).unwrap();
        assert_eq!((r[1], r[22]), (0x08, 0x05));
        // L2 resiste da metade do curso, R2 passa ao jogo: só o bit do L2 liga.
        let t = [Trig::Rigid { start: 5, power: 8 }, Trig::Pass];
        let r = out_report(Family::DualSense, Link::Usb, None, t, [0, 0], 0, true).unwrap();
        assert_eq!((r[1], r[11], r[22]), (0x08, 0x00, 0x21));
        // Zonas 5 a 9 ligadas, com 3 bits de força (8 vira 7) em cada uma.
        assert_eq!(u16::from_le_bytes(r[23..25].try_into().unwrap()), 0b11_1110_0000);
        let f = u32::from_le_bytes(r[25..29].try_into().unwrap());
        assert!((5..10).all(|i| (f >> (3 * i)) & 7 == 7) && (0..5).all(|i| (f >> (3 * i)) & 7 == 0));
        // A arma resiste numa zona e cede na seguinte; a vibração leva a frequência.
        let r = out_report(Family::DualSense, Link::Usb, None, [Trig::Pass, Trig::Click { start: 3, power: 1 }], [0, 0], 0, true).unwrap();
        assert_eq!((r[1], r[11], u16::from_le_bytes(r[12..14].try_into().unwrap()), r[14]), (0x04, 0x25, 0b1_1000, 0));
        let r = out_report(Family::DualSense, Link::Usb, None, [Trig::Pass, Trig::Buzz { start: 0, power: 4, hz: 60 }], [0, 0], 0, true).unwrap();
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
            generic_axes: vec![],
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
