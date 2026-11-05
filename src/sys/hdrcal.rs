//! Calibração própria: grava o perfil de cor com MHC2, ativa e volta atrás.
//! O pico e o preto gravados não mudam nenhum pixel: mudam o que o Windows informa aos jogos.

use std::sync::{Arc, Mutex};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::Security::Cryptography::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::SystemInformation::GetSystemTime;
use windows::Win32::UI::ColorSystem::*;

use super::hdr::{self, Light, Matrix};
use super::patterns::{self, Measured};

/// Acima disso a TV está suavizando, não cortando: a forma nunca some de verdade.
pub const HONEST_LIMIT: f32 = 4000.0;

#[derive(Clone, Default, Debug)]
pub struct Spec {
    pub file: String,
    pub light: Light,
    pub primaries: [[f32; 2]; 3],
    /// Mesma escala do app do Windows: 0 é neutro.
    pub saturation: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Target {
    pub adapter: LUID,
    pub source: u32,
}

#[derive(Clone, Debug, Default)]
pub enum Phase {
    #[default]
    Idle,
    Measuring,
    Review(Measured),
    Saving,
    /// O segundo campo diz se o Windows já informa os valores novos aos jogos.
    Saved(Spec, bool),
    Failed(&'static str),
}

/// `previous`: perfil ativo antes de a calibração começar.
#[derive(Default)]
pub struct State {
    pub phase: Phase,
    pub previous: String,
}

pub type Shared = Arc<Mutex<State>>;

fn fixed(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

fn xyz_tag(v: [f64; 3]) -> Vec<u8> {
    let mut t = b"XYZ \0\0\0\0".to_vec();
    v.iter().for_each(|c| t.extend(fixed(*c)));
    t
}

fn text_tag(s: &str) -> Vec<u8> {
    let text: Vec<u8> = s.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut t = b"mluc\0\0\0\0".to_vec();
    t.extend(1u32.to_be_bytes());
    t.extend(12u32.to_be_bytes());
    t.extend(b"enUS");
    t.extend((text.len() as u32).to_be_bytes());
    t.extend(28u32.to_be_bytes());
    t.extend(text);
    t
}

fn curve_tag() -> Vec<u8> {
    let mut t = b"curv\0\0\0\0".to_vec();
    t.extend(1u32.to_be_bytes());
    t.extend([0x02, 0x33, 0, 0]);
    t
}

fn chad_tag() -> Vec<u8> {
    let mut t = b"sf32\0\0\0\0".to_vec();
    for k in 0..9 {
        t.extend(fixed((k % 4 == 0) as u8 as f64));
    }
    t
}

fn mhc2_tag(l: Light, m: &Matrix) -> Vec<u8> {
    const MATRIX: u32 = 36;
    const CURVE: u32 = 16;
    let mut t = b"MHC2\0\0\0\0".to_vec();
    t.extend(2u32.to_be_bytes());
    t.extend(fixed(l.black as f64));
    t.extend(fixed(l.peak as f64));
    t.extend(MATRIX.to_be_bytes());
    for i in 0..3 {
        t.extend((MATRIX + 48 + i * CURVE).to_be_bytes());
    }
    for row in m {
        row.iter().for_each(|v| t.extend(fixed(*v)));
        t.extend(fixed(0.0));
    }
    for _ in 0..3 {
        t.extend(b"sf32\0\0\0\0");
        t.extend(fixed(0.0));
        t.extend(fixed(1.0));
    }
    t
}

/// Mesma ordem e mesmos campos do arquivo que o app do Windows grava.
pub fn build(spec: &Spec) -> Option<Vec<u8>> {
    let p: [[f64; 2]; 3] = spec.primaries.map(|c| [c[0] as f64, c[1] as f64]);
    let xyz = hdr::to_xyz(p)?;
    let column = |c: usize| [xyz[0][c], xyz[1][c], xyz[2][c]];
    let matrix = hdr::saturation_matrix(spec.saturation as f64)?;
    let name = spec.file.trim_end_matches(".icc");
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"cprt", text_tag("Prisma")),
        (b"rTRC", curve_tag()),
        (b"gTRC", curve_tag()),
        (b"bTRC", curve_tag()),
        (b"chad", chad_tag()),
        (b"rXYZ", xyz_tag(column(0))),
        (b"gXYZ", xyz_tag(column(1))),
        (b"bXYZ", xyz_tag(column(2))),
        (b"wtpt", xyz_tag([0.3127 / 0.329, 1.0, (1.0 - 0.3127 - 0.329) / 0.329])),
        (b"MSCA", b"text\0\0\0\0{'Appversion':'1.0.152.0','D65Adapted':True}\0".to_vec()),
        (b"lumi", xyz_tag([spec.light.full as f64; 3])),
        (b"MHC2", mhc2_tag(spec.light, &matrix)),
        (b"desc", text_tag(name)),
    ];
    let mut body = Vec::new();
    let mut table = (tags.len() as u32).to_be_bytes().to_vec();
    let start = 132 + tags.len() * 12;
    let mut seen: Vec<(Vec<u8>, usize)> = Vec::new();
    for (sig, data) in &tags {
        // As três curvas apontam para o mesmo bloco, como no arquivo do Windows.
        let at = match seen.iter().find(|s| s.0 == *data) {
            Some(s) => s.1,
            None => {
                let at = start + body.len();
                body.extend(data);
                body.resize(body.len().next_multiple_of(4), 0);
                seen.push((data.clone(), at));
                at
            }
        };
        table.extend(*sig);
        table.extend((at as u32).to_be_bytes());
        table.extend((data.len() as u32).to_be_bytes());
    }
    let mut icc = vec![0u8; 128];
    icc[0..4].copy_from_slice(&((start + body.len()) as u32).to_be_bytes());
    icc[4..8].copy_from_slice(b"SICC");
    icc[8] = 4;
    icc[9] = 0x40;
    icc[12..16].copy_from_slice(b"mntr");
    icc[16..20].copy_from_slice(b"RGB ");
    icc[20..24].copy_from_slice(b"XYZ ");
    let now = unsafe { GetSystemTime() };
    for (i, v) in [now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond].iter().enumerate() {
        icc[24 + i * 2..26 + i * 2].copy_from_slice(&v.to_be_bytes());
    }
    icc[36..40].copy_from_slice(b"acsp");
    icc[40..44].copy_from_slice(b"MSFT");
    icc[67] = 1;
    icc[68..80].copy_from_slice(&[0, 0, 0xf6, 0xd6, 0, 1, 0, 0, 0, 0, 0xd3, 0x2d]);
    icc[80..84].copy_from_slice(b"MSFT");
    icc.extend(table);
    icc.extend(body);
    // A identidade do perfil é o MD5 com estes campos zerados.
    let mut clean = icc.clone();
    clean[44..48].fill(0);
    clean[64..68].fill(0);
    clean[84..100].fill(0);
    let mut id = [0u8; 16];
    unsafe { BCryptHash(BCRYPT_MD5_ALG_HANDLE, None, &clean, &mut id) }.ok().ok()?;
    icc[84..100].copy_from_slice(&id);
    Some(icc)
}

/// Copia para a pasta de perfis do Windows. Ele não sobrescreve: o nome precisa ser novo.
fn install(file: &str, icc: &[u8]) -> bool {
    let path = std::env::temp_dir().join(file);
    if std::fs::write(&path, icc).is_err() {
        return false;
    }
    let ok = unsafe { InstallColorProfileW(PCWSTR::null(), &HSTRING::from(path.as_os_str())) }.as_bool();
    let _ = std::fs::remove_file(&path);
    ok
}

fn scope(t: Target) -> WCS_PROFILE_MANAGEMENT_SCOPE {
    unsafe { ColorProfileGetDisplayUserScope(t.adapter, t.source) }.unwrap_or(WCS_PROFILE_MANAGEMENT_SCOPE_CURRENT_USER)
}

fn default_is(t: Target, file: &str) -> bool {
    hdr::default_profile(t.adapter, t.source).is_some_and(|n| n.eq_ignore_ascii_case(file))
}

/// Gravar na lista errada dá sucesso e não aplica nada: só a leitura de volta confirma.
pub fn activate(t: Target, file: &str) -> bool {
    let name = HSTRING::from(file);
    unsafe {
        let _ = ColorProfileAddDisplayAssociation(scope(t), &name, t.adapter, t.source, true, true);
    }
    if default_is(t, file) {
        return true;
    }
    unsafe {
        let _ = ColorProfileSetDisplayDefaultAssociation(scope(t), &name, CPT_ICC, CPST_EXTENDED_DISPLAY_COLOR_MODE, t.adapter, t.source);
    }
    default_is(t, file)
}

/// Tira o perfil da tela e apaga o arquivo.
pub fn remove(t: Target, file: &str) {
    let name = HSTRING::from(file);
    unsafe {
        let _ = ColorProfileRemoveDisplayAssociation(scope(t), &name, t.adapter, t.source, true);
        let _ = UninstallColorProfileW(PCWSTR::null(), &name, true);
    }
}

/// O Windows só repassa o perfil à tela quando esta tarefa roda, e ela exige administrador.
fn reload() -> bool {
    use std::os::windows::process::CommandExt;
    let tool = std::path::PathBuf::from(std::env::var("SystemRoot").unwrap_or_default()).join("System32\\schtasks.exe");
    let task = std::process::Command::new(tool).args(["/Run", "/TN", "\\Microsoft\\Windows\\WindowsColorSystem\\Calibration Loader"]).creation_flags(0x0800_0000).output();
    task.is_ok_and(|o| o.status.success())
}

fn agrees(a: Light, b: Light) -> bool {
    (a.peak - b.peak).abs() < 1.0 && (a.full - b.full).abs() < 1.0 && (a.black - b.black).abs() < 0.01
}

/// Com HDR ligado, confere se o Windows já informa os valores novos aos jogos.
fn applied(gdi: &str, l: Light) -> bool {
    for _ in 0..30 {
        if hdr::windows_light().get(gdi).is_some_and(|r| agrees(*r, l)) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}

fn ping(window: isize) {
    unsafe {
        let _ = InvalidateRect(Some(HWND(window as *mut _)), None, false);
    }
}

fn set(shared: &Shared, phase: Phase, window: isize) {
    shared.lock().unwrap().phase = phase;
    ping(window);
}

pub struct Start {
    pub gdi: String,
    pub active: String,
}

/// Abre os padrões em tela cheia.
pub fn begin(shared: Shared, s: Start, window: isize) {
    {
        let mut st = shared.lock().unwrap();
        st.previous = s.active.clone();
        st.phase = Phase::Measuring;
    }
    let _ = std::thread::Builder::new().name("calibracao".into()).spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let result = patterns::run(&s.gdi);
        CoUninitialize();
        match result {
            Ok(m) => set(&shared, Phase::Review(m), window),
            Err(e) => set(&shared, e.map_or(Phase::Idle, Phase::Failed), window),
        }
    });
}

/// Grava e ativa. Se o Windows recusar, o perfil anterior continua.
pub fn save(shared: Shared, spec: Spec, t: Target, gdi: String, window: isize) {
    let previous = {
        let mut st = shared.lock().unwrap();
        st.phase = Phase::Saving;
        st.previous.clone()
    };
    let _ = std::thread::Builder::new().name("perfil".into()).spawn(move || {
        const WRONG: &str = "O Windows não aceitou o perfil. O perfil anterior foi mantido.";
        let ok = build(&spec).is_some_and(|icc| install(&spec.file, &icc)) && activate(t, &spec.file);
        if !ok {
            if !previous.is_empty() {
                activate(t, &previous);
            }
            remove(t, &spec.file);
            return set(&shared, Phase::Failed(WRONG), window);
        }
        reload();
        let live = applied(&gdi, spec.light);
        set(&shared, Phase::Saved(spec, live), window);
    });
}

pub fn discard(shared: &Shared) {
    shared.lock().unwrap().phase = Phase::Idle;
}

/// Nome de arquivo novo para o modo de imagem escolhido.
pub fn file_name(mode: &str) -> String {
    let now = unsafe { GetSystemTime() };
    format!("Prisma HDR {mode} {:04}-{:02}-{:02} {:02}{:02}{:02}.icc", now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond)
}

/// Teste: tira um perfil e volta ao anterior.
#[cfg(debug_assertions)]
pub fn restore_test(t: Target, previous: &str) -> String {
    let mut out = String::new();
    let active = hdr::default_profile(t.adapter, t.source).unwrap_or_default();
    let back = activate(t, previous);
    if !active.eq_ignore_ascii_case(previous) {
        remove(t, &active);
    }
    out += &format!("tirou {active}, voltou {back}, padrao {:?}\n", hdr::default_profile(t.adapter, t.source));
    out
}

/// Teste: grava um perfil com os valores dados, ativa, confere, e deixa tudo como estava.
#[cfg(debug_assertions)]
pub fn activation_test(t: Target, gdi: &str, active: &str, light: Light, primaries: [[f32; 2]; 3], saturation: f32) -> String {
    let file = "Prisma HDR teste.icc".to_string();
    let light = Light { peak: light.peak + 50.0, ..light };
    let spec = Spec { file: file.clone(), light, primaries, saturation };
    let mut out = format!("antes: {active}\n");
    remove(t, &file);
    let Some(mut icc) = build(&spec) else { return out + "build falhou" };
    if let Ok(path) = std::env::var("PRISMA_ICC") {
        icc = std::fs::read(path).unwrap();
    }
    out += &format!("tamanho {} leitura {:?}\n", icc.len(), hdr::parse_profile(&icc));
    let done = install(&file, &icc);
    out += &format!("instalou {done}\n");
    let on = done && activate(t, &file);
    out += &format!("ativou {on}, padrao agora {:?}\n", hdr::default_profile(t.adapter, t.source));
    if on {
        reload();
        out += &format!("windows aplicou {} lido {:?}\n", applied(gdi, light), hdr::windows_light().get(gdi));
    }
    let back = activate(t, active);
    reload();
    remove(t, &file);
    out += &format!("final lido {:?}\n", hdr::windows_light().get(gdi));
    out += &format!("voltou {back}, padrao agora {:?}\n", hdr::default_profile(t.adapter, t.source));
    out
}
