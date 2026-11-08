//! Estado do HDR, do perfil de calibração, do Dolby Vision e do som. Só lê: nada aqui muda o PC.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use windows::core::*;
use windows::Win32::Devices::Display::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::UI::ColorSystem::*;

use super::audio;
use super::reg::{self, Hive};

#[derive(Clone, Copy, Default, Debug)]
pub struct Light {
    pub peak: f32,
    pub full: f32,
    pub black: f32,
}

#[derive(Clone, Default, Debug)]
pub struct DolbyVision {
    /// Bloco versão 2: quais modos a tela aceita, de 0 a 3.
    pub interface: Option<u8>,
    pub light: Option<Light>,
}

/// O que uma ficha de tela (EDID) declara.
#[derive(Clone, Default, Debug)]
pub struct Edid {
    pub primaries: [[f32; 2]; 3],
    pub light: Option<Light>,
    pub dv: Option<DolbyVision>,
    /// Nome do formato de som e canais.
    pub audio: Vec<(&'static str, u8)>,
}

#[derive(Clone, Default, Debug)]
pub struct Profile {
    pub name: String,
    pub date: String,
    pub light: Light,
    pub primaries: [[f32; 2]; 3],
    /// Reforço de saturação, de 0 a 1 na escala do app do Windows. `None`: matriz de outro tipo.
    pub saturation: Option<f32>,
    pub neutral_curve: bool,
}

#[derive(Clone, Default, Debug)]
pub struct Screen {
    pub name: String,
    pub size: (u32, u32),
    pub hz: f32,
    pub bits: u32,
    /// 0 SDR, 1 cor ampla, 2 HDR.
    pub mode: u32,
    pub hdr_support: bool,
    pub sdr_white: f32,
    /// O que o Windows informa aos jogos agora.
    pub windows: Option<Light>,
    pub hardware: Option<Edid>,
    pub active: Option<Edid>,
    pub overridden: bool,
    pub hdr_flavor: Option<u32>,
    pub profile: Option<Profile>,
    pub adapter: LUID,
    pub source: u32,
    pub gdi: String,
}

/// Primeira tela que aceita HDR: adaptador, saída e se o HDR está ligado.
pub fn hdr_output() -> Option<(LUID, u32, bool)> {
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm).ok().ok()?;
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None).ok().ok()?;
        paths[..np as usize].iter().find_map(|p| {
            let (adapter, id) = (p.targetInfo.adapterId, p.targetInfo.id);
            let mut color = ColorInfo2::default();
            (info_type(&mut color, DISPLAYCONFIG_DEVICE_INFO_TYPE(15), adapter, id) && color.value & 0x10 != 0).then_some((adapter, id, color.mode == 2))
        })
    }
}

/// Liga ou desliga o HDR e espera a tela chegar lá.
pub fn set_hdr(adapter: LUID, output: u32, on: bool) -> bool {
    #[repr(C)]
    struct SetState {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
        value: u32,
    }
    let mut s = SetState { header: DISPLAYCONFIG_DEVICE_INFO_HEADER::default(), value: on as u32 };
    s.header.r#type = DISPLAYCONFIG_DEVICE_INFO_TYPE(16);
    s.header.size = std::mem::size_of::<SetState>() as u32;
    s.header.adapterId = adapter;
    s.header.id = output;
    if unsafe { DisplayConfigSetDeviceInfo(&s.header) } != 0 {
        return false;
    }
    for _ in 0..200 {
        let mut color = ColorInfo2::default();
        if info_type(&mut color, DISPLAYCONFIG_DEVICE_INFO_TYPE(15), adapter, output) && (color.mode == 2) == on {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
}

#[derive(Clone, Default, Debug)]
pub struct Info {
    pub screens: Vec<Screen>,
    pub sound: Option<audio::Output>,
    pub dv_app: bool,
    pub dolby_app: bool,
}

#[derive(Default)]
pub struct State {
    pub info: Option<Arc<Info>>,
    pub busy: bool,
}

pub type Shared = Arc<Mutex<State>>;

/// Nits de um código PQ de 12 bits.
fn pq(code: f32) -> f32 {
    let (m1, m2, c1, c2, c3) = (0.159_301_76_f32, 78.843_75_f32, 0.835_937_5_f32, 18.851_563_f32, 18.687_5_f32);
    let e = (code / 4095.0).powf(1.0 / m2);
    10000.0 * ((e - c1).max(0.0) / (c2 - c3 * e)).powf(1.0 / m1)
}

fn hdr_block(b: &[u8]) -> Option<Light> {
    let peak = 50.0 * 2f32.powf(*b.get(3)? as f32 / 32.0);
    let full = b.get(4).map_or(0.0, |v| 50.0 * 2f32.powf(*v as f32 / 32.0));
    let black = b.get(5).map_or(0.0, |v| peak * (*v as f32 / 255.0).powi(2) / 100.0);
    Some(Light { peak, full, black })
}

fn dv_block(b: &[u8]) -> DolbyVision {
    let x = &b[4..];
    if x.len() < 3 || x[0] >> 5 != 2 {
        return DolbyVision::default();
    }
    let black = pq(20.0 * (x[1] >> 3) as f32);
    let peak = pq(2055.0 + 65.0 * (x[2] >> 3) as f32);
    DolbyVision { interface: Some(x[2] & 3), light: Some(Light { peak, full: 0.0, black }) }
}

fn audio_name(b: &[u8]) -> Option<&'static str> {
    Some(match (b[0] >> 3) & 15 {
        1 => "PCM",
        2 => "Dolby Digital",
        7 => "DTS",
        10 if b[2] & 1 != 0 => "Dolby Digital Plus Atmos",
        10 => "Dolby Digital Plus",
        11 => "DTS-HD",
        12 if b[2] & 1 != 0 => "Dolby MAT Atmos",
        12 => "Dolby MAT",
        _ => return None,
    })
}

pub fn parse(e: &[u8]) -> Edid {
    let mut out = Edid::default();
    if e.len() < 128 {
        return out;
    }
    // Cada coordenada tem 10 bits: os 2 de baixo nos bytes 25 e 26, os 8 de cima depois.
    let low = |byte: usize, shift: u32| ((e[byte] >> shift) & 3) as u32;
    let xy = |hi: usize, byte: usize, shift: u32| (((e[hi] as u32) << 2) | low(byte, shift)) as f32 / 1024.0;
    out.primaries = [
        [xy(27, 25, 6), xy(28, 25, 4)],
        [xy(29, 25, 2), xy(30, 25, 0)],
        [xy(31, 26, 6), xy(32, 26, 4)],
    ];
    for ext in e[128..].chunks_exact(128).filter(|c| c[0] == 2) {
        let end = (ext[2] as usize).clamp(4, 127);
        let mut i = 4;
        while i < end {
            let (tag, len) = (ext[i] >> 5, (ext[i] & 31) as usize);
            let Some(b) = ext.get(i + 1..i + 1 + len) else { break };
            match tag {
                1 => {
                    for sad in b.chunks_exact(3) {
                        if let Some(name) = audio_name(sad) {
                            let item = (name, (sad[0] & 7) + 1);
                            if !out.audio.contains(&item) {
                                out.audio.push(item);
                            }
                        }
                    }
                }
                7 if b.first() == Some(&6) => out.light = hdr_block(b),
                7 if b.len() > 4 && b[..4] == [1, 0x46, 0xd0, 0x00] => out.dv = Some(dv_block(b)),
                _ => {}
            }
            i += 1 + len;
        }
    }
    out
}

pub type Matrix = [[f64; 3]; 3];

fn inverse(m: &Matrix) -> Option<Matrix> {
    let c = |r1: usize, c1: usize, r2: usize, c2: usize| m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1];
    let det = m[0][0] * c(1, 1, 2, 2) - m[0][1] * c(1, 0, 2, 2) + m[0][2] * c(1, 0, 2, 1);
    if det.abs() < 1e-9 {
        return None;
    }
    Some([
        [c(1, 1, 2, 2) / det, c(0, 2, 2, 1) / det, c(0, 1, 1, 2) / det],
        [c(1, 2, 2, 0) / det, c(0, 0, 2, 2) / det, c(0, 2, 1, 0) / det],
        [c(1, 0, 2, 1) / det, c(0, 1, 2, 0) / det, c(0, 0, 1, 1) / det],
    ])
}

fn product(a: &Matrix, b: &Matrix) -> Matrix {
    let mut out = [[0.0; 3]; 3];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    out
}

/// Matriz de RGB para XYZ com branco D65.
pub fn to_xyz(p: [[f64; 2]; 3]) -> Option<Matrix> {
    let m: Matrix = [
        [p[0][0] / p[0][1], p[1][0] / p[1][1], p[2][0] / p[2][1]],
        [1.0, 1.0, 1.0],
        [(1.0 - p[0][0] - p[0][1]) / p[0][1], (1.0 - p[1][0] - p[1][1]) / p[1][1], (1.0 - p[2][0] - p[2][1]) / p[2][1]],
    ];
    let inv = inverse(&m)?;
    let w = [0.3127 / 0.329, 1.0, (1.0 - 0.3127 - 0.329) / 0.329];
    let s: Vec<f64> = (0..3).map(|r| (0..3).map(|c| inv[r][c] * w[c]).sum()).collect();
    Some(std::array::from_fn(|r| std::array::from_fn(|c| m[r][c] * s[c])))
}

/// O app do Windows mistura as primárias de BT.709 para P3 por um fator.
pub fn saturation_matrix(t: f64) -> Option<Matrix> {
    const REC709: [[f64; 2]; 3] = [[0.64, 0.33], [0.3, 0.6], [0.15, 0.06]];
    const P3: [[f64; 2]; 3] = [[0.68, 0.32], [0.265, 0.69], [0.15, 0.06]];
    let mix: [[f64; 2]; 3] = std::array::from_fn(|p| std::array::from_fn(|c| REC709[p][c] + (P3[p][c] - REC709[p][c]) * t));
    Some(product(&to_xyz(mix)?, &inverse(&to_xyz(REC709)?)?))
}

/// Acha o fator de uma matriz gravada no perfil.
fn saturation(m: &Matrix) -> Option<f32> {
    let mut best = (1.0, 0.0);
    for i in 0..=150 {
        let t = i as f64 / 100.0;
        let fit = saturation_matrix(t)?;
        let error = (0..9).map(|k| (fit[k / 3][k % 3] - m[k / 3][k % 3]).abs()).fold(0.0, f64::max);
        if error < best.0 {
            best = (error, t);
        }
    }
    (best.0 < 0.003).then_some(best.1 as f32)
}

fn fixed(b: &[u8], at: usize) -> Option<f32> {
    let v = i32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?);
    Some(v as f32 / 65536.0)
}

fn word(b: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?) as usize)
}

fn tag<'a>(icc: &'a [u8], name: &[u8; 4]) -> Option<&'a [u8]> {
    for i in 0..word(icc, 128)?.min(64) {
        let at = 132 + i * 12;
        if icc.get(at..at + 4)? == name {
            let (start, size) = (word(icc, at + 4)?, word(icc, at + 8)?);
            return icc.get(start..start + size);
        }
    }
    None
}

fn primary(icc: &[u8], name: &[u8; 4]) -> Option<[f32; 2]> {
    let t = tag(icc, name)?;
    let (x, y, z) = (fixed(t, 8)?, fixed(t, 12)?, fixed(t, 16)?);
    Some([x / (x + y + z), y / (x + y + z)])
}

fn is_neutral(mhc2: &[u8], count: usize, at: usize) -> bool {
    (0..count).all(|i| fixed(mhc2, at + 8 + i * 4).is_some_and(|v| (v - i as f32 / (count - 1).max(1) as f32).abs() < 0.002))
}

/// Lê a calibração: pico e preto vêm do bloco MHC2, tela cheia do bloco lumi.
pub fn parse_profile(icc: &[u8]) -> Option<Profile> {
    let m = tag(icc, b"MHC2")?;
    let count = word(m, 8)?;
    let (matrix, red, green, blue) = (word(m, 20)?, word(m, 24)?, word(m, 28)?, word(m, 32)?);
    let cell = |r: usize, c: usize| fixed(m, matrix + (r * 4 + c) * 4).unwrap_or(0.0) as f64;
    let mat: Matrix = std::array::from_fn(|r| std::array::from_fn(|c| cell(r, c)));
    let flat = (0..9).all(|k| (mat[k / 3][k % 3] - (k % 4 == 0) as u8 as f64).abs() < 0.002);
    let full = tag(icc, b"lumi").and_then(|t| fixed(t, 12)).unwrap_or(0.0);
    Some(Profile {
        light: Light { peak: fixed(m, 16)?, full, black: fixed(m, 12)? },
        primaries: [primary(icc, b"rXYZ")?, primary(icc, b"gXYZ")?, primary(icc, b"bXYZ")?],
        saturation: if flat { Some(0.0) } else { saturation(&mat) },
        neutral_curve: [red, green, blue].iter().all(|at| is_neutral(m, count, *at)),
        ..Default::default()
    })
}

fn file_date(path: &std::path::Path) -> String {
    let secs = std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
    // Dias desde 1970 para ano, mês e dia.
    let z = secs / 86_400 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    format!("{:04}-{m:02}-{d:02}", yoe + era * 400 + (m <= 2) as i64)
}

/// Nome do arquivo do perfil de HDR que está valendo nesta tela.
pub fn default_profile(adapter: LUID, source: u32) -> Option<String> {
    unsafe {
        let scopes = [WCS_PROFILE_MANAGEMENT_SCOPE_CURRENT_USER, WCS_PROFILE_MANAGEMENT_SCOPE_SYSTEM_WIDE];
        let name = scopes.iter().find_map(|s| ColorProfileGetDisplayDefault(*s, adapter, source, CPT_ICC, CPST_EXTENDED_DISPLAY_COLOR_MODE).ok().filter(|n| !n.is_null()))?;
        let file = name.to_string().unwrap_or_default();
        let _ = windows::Win32::Foundation::LocalFree(Some(HLOCAL(name.0 as *mut _)));
        Some(file)
    }
}

fn active_profile(adapter: LUID, source: u32) -> Option<Profile> {
    let file = default_profile(adapter, source)?;
    let dir = crate::sys::system_dir().join("spool\\drivers\\color");
    let path = dir.join(&file);
    let mut p = parse_profile(&std::fs::read(&path).ok()?)?;
    p.date = file_date(&path);
    p.name = file;
    Some(p)
}

/// O que o Windows informa aos jogos, por nome de saída.
pub fn windows_light() -> HashMap<String, Light> {
    let mut out = HashMap::new();
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return out };
        let mut a = 0;
        while let Ok(adapter) = factory.EnumAdapters1(a) {
            a += 1;
            let mut o = 0;
            while let Ok(output) = adapter.EnumOutputs(o) {
                o += 1;
                let Ok(output) = output.cast::<IDXGIOutput6>() else { continue };
                let Ok(d) = output.GetDesc1() else { continue };
                let name = String::from_utf16_lossy(&d.DeviceName).trim_end_matches('\0').to_string();
                out.insert(name, Light { peak: d.MaxLuminance, full: d.MaxFullFrameLuminance, black: d.MinLuminance });
            }
        }
    }
    out
}

/// Ficha que a própria tela envia, pelo driver NVIDIA. O registro mostra o override, não a tela.
fn nvidia_edid(gdi: &str) -> Option<Vec<u8>> {
    #[repr(C)]
    struct Data {
        version: u32,
        edid: *mut u8,
        size: u32,
        rest: [u8; 36],
    }
    type Query = unsafe extern "C" fn(u32) -> *mut core::ffi::c_void;
    type Init = unsafe extern "C" fn() -> i32;
    type IdByName = unsafe extern "C" fn(*const u8, *mut u32) -> i32;
    type GetEdid = unsafe extern "C" fn(u32, *mut Data, *mut i32) -> i32;
    const HARDWARE: i32 = 5;
    unsafe {
        let module = LoadLibraryExW(w!("nvapi64.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
        let read = || -> Option<Vec<u8>> {
            let query: Query = std::mem::transmute(GetProcAddress(module, s!("nvapi_QueryInterface"))?);
            let init = query(0x0150_e828);
            let by_name = query(0xae45_7190);
            let get = query(0x436c_ed76);
            if init.is_null() || by_name.is_null() || get.is_null() {
                return None;
            }
            let (init, by_name, get): (Init, IdByName, GetEdid) = (std::mem::transmute(init), std::mem::transmute(by_name), std::mem::transmute(get));
            if init() != 0 {
                return None;
            }
            let mut id = 0;
            let name = format!("{gdi}\0");
            if by_name(name.as_ptr(), &mut id) != 0 {
                return None;
            }
            let mut buf = vec![0u8; 2048];
            let mut data = Data { version: 56 | (2 << 16), edid: buf.as_mut_ptr(), size: buf.len() as u32, rest: [0; 36] };
            let mut flag = HARDWARE;
            if get(id, &mut data, &mut flag) != 0 || flag != HARDWARE {
                return None;
            }
            buf.truncate(data.size as usize);
            Some(buf)
        };
        let out = read();
        let _ = FreeLibrary(module);
        out
    }
}

/// A leitura direta passa pelo cabo: uma vez por tela basta enquanto o Prisma estiver aberto.
static HARDWARE: Mutex<Option<HashMap<String, Vec<u8>>>> = Mutex::new(None);

fn hardware_edid(gdi: &str, device: &str) -> Option<Edid> {
    let mut seen = HARDWARE.lock().unwrap();
    let map = seen.get_or_insert_with(HashMap::new);
    if !map.contains_key(device) {
        map.insert(device.to_string(), nvidia_edid(gdi)?);
    }
    Some(parse(map.get(device)?))
}

/// Valor sem documentação: 1 tem sido visto como HDR10 e 2 como Dolby Vision.
fn hdr_flavor(monitor: &str) -> Option<u32> {
    let store = "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers\\MonitorDataStore";
    let newest = reg::subkeys(Hive::Machine, store).into_iter().filter(|k| k.starts_with(monitor)).max_by_key(|k| reg::written(Hive::Machine, &format!("{store}\\{k}")))?;
    reg::dword(Hive::Machine, &format!("{store}\\{newest}"), "HdrFlavor")
}

fn info_type<T>(h: &mut T, kind: DISPLAYCONFIG_DEVICE_INFO_TYPE, adapter: LUID, id: u32) -> bool {
    unsafe {
        let head = h as *mut T as *mut DISPLAYCONFIG_DEVICE_INFO_HEADER;
        (*head).r#type = kind;
        (*head).size = std::mem::size_of::<T>() as u32;
        (*head).adapterId = adapter;
        (*head).id = id;
        DisplayConfigGetDeviceInfo(head) == 0
    }
}

fn wide_text(s: &[u16]) -> String {
    String::from_utf16_lossy(&s[..s.iter().position(|c| *c == 0).unwrap_or(s.len())])
}

#[repr(C)]
#[derive(Default)]
struct ColorInfo2 {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    value: u32,
    encoding: i32,
    bits: u32,
    mode: u32,
}

fn screens() -> Vec<Screen> {
    let mut out = Vec::new();
    let reported = windows_light();
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm).ok().is_err() {
            return out;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None).ok().is_err() {
            return out;
        }
        for p in &paths[..np as usize] {
            let (adapter, target) = (p.targetInfo.adapterId, p.targetInfo.id);
            let mut s = Screen::default();
            let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            let mut name = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            let gdi = if info_type(&mut source, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, p.sourceInfo.adapterId, p.sourceInfo.id) { wide_text(&source.viewGdiDeviceName) } else { String::new() };
            if !info_type(&mut name, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, adapter, target) {
                continue;
            }
            s.name = wide_text(&name.monitorFriendlyDeviceName);
            let device = wide_text(&name.monitorDevicePath);
            let parts: Vec<&str> = device.split('#').collect();
            let mode = p.targetInfo.Anonymous.modeInfoIdx as usize;
            if mode < nm as usize {
                let signal = modes[mode].Anonymous.targetMode.targetVideoSignalInfo;
                s.size = (signal.activeSize.cx, signal.activeSize.cy);
            }
            let rate = p.targetInfo.refreshRate;
            s.hz = rate.Numerator as f32 / rate.Denominator.max(1) as f32;
            let mut color = ColorInfo2::default();
            if info_type(&mut color, DISPLAYCONFIG_DEVICE_INFO_TYPE(15), adapter, target) {
                s.mode = color.mode;
                s.bits = color.bits;
                s.hdr_support = color.value & 0x10 != 0;
            }
            let mut white = DISPLAYCONFIG_SDR_WHITE_LEVEL::default();
            if info_type(&mut white, DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, adapter, target) {
                s.sdr_white = white.SDRWhiteLevel as f32 / 1000.0 * 80.0;
            }
            s.windows = reported.get(&gdi).copied();
            s.adapter = p.sourceInfo.adapterId;
            s.source = p.sourceInfo.id;
            s.gdi = gdi.clone();
            s.hardware = hardware_edid(&gdi, &device);
            if parts.len() > 2 {
                let key = format!("SYSTEM\\CurrentControlSet\\Enum\\DISPLAY\\{}\\{}\\Device Parameters", parts[1], parts[2]);
                s.overridden = reg::exists(Hive::Machine, &format!("{key}\\EDID_OVERRIDE"));
                s.active = reg::read(Hive::Machine, &key, "EDID").map(|(_, e)| parse(&e));
                s.hdr_flavor = hdr_flavor(parts[1]);
            }
            s.profile = active_profile(p.sourceInfo.adapterId, p.sourceInfo.id);
            out.push(s);
        }
    }
    // Quem aceita HDR vem primeiro.
    out.sort_by_key(|s| !s.hdr_support);
    out
}

fn package_exists(prefix: &str) -> bool {
    reg::subkeys(Hive::User, "Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\CurrentVersion\\AppModel\\Repository\\Packages").iter().any(|k| k.starts_with(prefix))
}

pub fn read() -> Info {
    Info {
        screens: screens(),
        sound: audio::default_output(),
        dv_app: package_exists("DolbyLaboratories.DolbyVisionAccess_"),
        dolby_app: package_exists("DolbyLaboratories.DolbyAccess_"),
    }
}

/// Lê em segundo plano e mantém a leitura anterior na tela até terminar.
pub fn load(shared: Shared, window: isize) {
    {
        let mut s = shared.lock().unwrap();
        if s.busy {
            return;
        }
        s.busy = true;
    }
    let _ = std::thread::Builder::new().name("hdr".into()).spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let info = read();
        CoUninitialize();
        let mut s = shared.lock().unwrap();
        s.info = Some(Arc::new(info));
        s.busy = false;
        drop(s);
        let _ = InvalidateRect(Some(HWND(window as *mut _)), None, false);
    });
}
