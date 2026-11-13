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
use std::sync::Mutex;
use std::time::{Duration, Instant};

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
pub const A: u32 = 1;
pub const B: u32 = 1 << 1;
pub const X: u32 = 1 << 2;
pub const Y: u32 = 1 << 3;
pub const LB: u32 = 1 << 4;
pub const RB: u32 = 1 << 5;
pub const BACK: u32 = 1 << 6;
pub const START: u32 = 1 << 7;
pub const LS: u32 = 1 << 8;
pub const RS: u32 = 1 << 9;
pub const UP: u32 = 1 << 10;
pub const DOWN: u32 = 1 << 11;
pub const LEFT: u32 = 1 << 12;
pub const RIGHT: u32 = 1 << 13;
pub const GUIDE: u32 = 1 << 14;
/// Clique do touchpad (DualSense e DualShock 4).
pub const PAD: u32 = 1 << 15;
/// Botão de mudo do DualSense.
pub const MUTE: u32 = 1 << 16;

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
    pub buttons: u32,
    /// Analógicos de -1 a 1, com Y para cima positivo.
    pub lx: f32,
    pub ly: f32,
    pub rx: f32,
    pub ry: f32,
    /// Gatilhos de 0 a 1.
    pub lt: f32,
    pub rt: f32,
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
}

pub enum Event {
    Connected { name: String, link: Link },
    Disconnected { name: String },
    BatteryLow { name: String, battery: Battery },
    Jitter { name: String, p99_ms: f32 },
    /// Mapeamento: perfil trocado pelo programa em foco.
    Profile { name: String },
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
    Turbo { button: u32, on: bool, pad: String },
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
    PADS.lock().unwrap().clone()
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
        Some(_) => match IG.lock().unwrap().as_slice() {
            [(p, _, Link::Bluetooth, _)] => p.clone(),
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
            out.push(Dev { path, vid: a.VendorID, pid: a.ProductID, name: hid_string(h, HidD_GetProductString), serial: hid_string(h, HidD_GetSerialNumberString), input_len: caps.InputReportByteLength });
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

fn hat_bits(h: u8) -> u32 {
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
fn sony_buttons(b0: u8, b1: u8, b2: u8) -> u32 {
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
        })
    };
    match (f, link, *r.first()?) {
        (Family::DualSense, Link::Usb, 0x01) => Some((at(1, true)?, Some(ds_battery(get(53)?)))),
        (Family::DualSense, _, 0x31) => Some((at(2, true)?, Some(ds_battery(get(54)?)))),
        (Family::DualSense | Family::Ds4, Link::Bluetooth, 0x01) => Some((at(1, false)?, None)),
        (Family::Ds4, Link::Usb, 0x01) => Some((at(1, false)?, Some(ds4_battery(get(30)?)))),
        (Family::Ds4, _, 0x11) => Some((at(3, false)?, Some(ds4_battery(get(32)?)))),
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
                jitter_warned: false,
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
        let parsed = match &generic {
            Some((axes, _)) => Some((generic_input(pre, axes, &mut buf[..got as usize]), None)),
            None => sony(fam, link, &buf[..got as usize]),
        };
        if !shown {
            show(true);
            shown = true;
        }
        let fed = parsed.as_ref().map(|x| x.0);
        let mut pads = PADS.lock().unwrap();
        let Some(p) = pads.iter_mut().find(|p| p.key == d.path) else { break };
        if let Some((inp, bat)) = parsed {
            p.input = inp;
            if let Some(b) = bat {
                p.battery = b;
            }
        }
        if rate.is_some() {
            p.rate = rate;
        }
        let events = check_warnings(p);
        drop(pads);
        if let Some(inp) = fed {
            super::remap::feed(&d.path, &inp, now);
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
    Input { buttons: b, lx: s(g.sThumbLX), ly: s(g.sThumbLY), rx: s(g.sThumbRX), ry: s(g.sThumbRY), lt: g.bLeftTrigger as f32 / 255.0, rt: g.bRightTrigger as f32 / 255.0 }
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
                    jitter_warned: false,
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
    fn short_report_is_ignored() {
        assert!(sony(Family::DualSense, Link::Usb, &[1, 2, 3]).is_none());
        assert!(sony(Family::Generic, Link::Usb, &[1; 64]).is_none());
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
            jitter_warned: false,
        };
        assert_eq!(check_warnings(&mut p).len(), 1);
        assert_eq!(check_warnings(&mut p).len(), 0);
        p.battery = Battery::Percent { pct: 5, charging: true };
        assert_eq!(check_warnings(&mut p).len(), 0);
        p.battery = Battery::Percent { pct: 5, charging: false };
        assert_eq!(check_warnings(&mut p).len(), 1);
    }
}
