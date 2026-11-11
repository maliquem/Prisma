//! Perfis do driver NVIDIA (DRS): o global e um por jogo, sem tocar no jogo.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::*;
use windows::Win32::Foundation::FreeLibrary;
use windows::Win32::System::LibraryLoader::*;

pub const FRAME_LIMIT: u32 = 0x1083_5002;
pub const DLSS_PRESET: u32 = 0x10E4_1DF3;
pub const DLSS_OVERRIDE: u32 = 0x10E4_1E01;

const NAME_BYTES: usize = 4096;
const SETTING_SIZE: usize = 0x3020;
const PROFILE_SIZE: usize = 0x1014;
const APP_SIZE: usize = 8 + NAME_BYTES * 4 + 4 + NAME_BYTES;
const SETTING_ID_AT: usize = 4 + NAME_BYTES;
const SETTING_TYPE_AT: usize = SETTING_ID_AT + 4;
const SETTING_LOCATION_AT: usize = SETTING_ID_AT + 8;
const SETTING_IS_PREDEFINED_AT: usize = SETTING_ID_AT + 12;
const SETTING_PREDEFINED_VALID_AT: usize = SETTING_ID_AT + 16;
const SETTING_PREDEFINED_AT: usize = SETTING_ID_AT + 20;
const SETTING_VALUE_AT: usize = SETTING_ID_AT + 20 + 4100;
/// NVDRS_SETTING_VALUES: cabeçalho, valor padrão e até 100 valores de 4100 bytes.
const VALUES_SIZE: usize = 12 + 4100 * 101;
const VALUES_DEFAULT_AT: usize = 12;
const PROFILE_SETTINGS_AT: usize = 4 + NAME_BYTES + 12;
const LOCATION_CURRENT: u32 = 0;
const NOT_FOUND: [i32; 2] = [-163, -166];
/// Cópias do banco de perfis guardadas antes de mudar algo; as mais velhas saem.
const BACKUPS_KEPT: usize = 10;

type H = *mut c_void;
type Query = unsafe extern "C" fn(u32) -> *mut c_void;

fn version(size: usize, v: u32) -> u32 {
    size as u32 | (v << 16)
}

fn wide_name(s: &str) -> Vec<u16> {
    let mut w: Vec<u16> = s.encode_utf16().collect();
    w.resize(2048, 0);
    w
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

struct Api {
    create: unsafe extern "C" fn(*mut H) -> i32,
    destroy: unsafe extern "C" fn(H) -> i32,
    load: unsafe extern "C" fn(H) -> i32,
    save: unsafe extern "C" fn(H) -> i32,
    load_file: unsafe extern "C" fn(H, *const u16) -> i32,
    save_file: unsafe extern "C" fn(H, *const u16) -> i32,
    find_app: unsafe extern "C" fn(H, *const u16, *mut H, *mut u8) -> i32,
    find_profile: unsafe extern "C" fn(H, *const u16, *mut H) -> i32,
    global: unsafe extern "C" fn(H, *mut H) -> i32,
    info: unsafe extern "C" fn(H, H, *mut u8) -> i32,
    new_profile: unsafe extern "C" fn(H, *const u8, *mut H) -> i32,
    new_app: unsafe extern "C" fn(H, H, *mut u8) -> i32,
    set: unsafe extern "C" fn(H, H, *const u8) -> i32,
    get: unsafe extern "C" fn(H, H, u32, *mut u8, *mut u32) -> i32,
    delete: unsafe extern "C" fn(H, H, u32) -> i32,
    delete_profile: unsafe extern "C" fn(H, H) -> i32,
    driver: unsafe extern "C" fn(*mut u32, *mut u8) -> i32,
    values: unsafe extern "C" fn(u32, *mut u32, *mut u8) -> i32,
}

unsafe fn api(query: Query) -> Option<Api> {
    macro_rules! f {
        ($id:expr) => {{
            let p = query($id);
            if p.is_null() {
                return None;
            }
            std::mem::transmute(p)
        }};
    }
    let init: unsafe extern "C" fn() -> i32 = f!(0x0150_e828);
    if init() != 0 {
        return None;
    }
    Some(Api {
        create: f!(0x0694_d52e),
        destroy: f!(0xdad9_cff8),
        load: f!(0x375d_bd6b),
        save: f!(0xfcbc_7e14),
        load_file: f!(0xd3ed_e889),
        save_file: f!(0x2be2_5df8),
        find_app: f!(0xeee5_66b2),
        find_profile: f!(0x7e4a_9a0b),
        global: f!(0x617b_ff9f),
        info: f!(0x61cd_6fd6),
        new_profile: f!(0xcc17_6068),
        new_app: f!(0x4347_a9de),
        set: f!(0x577d_d202),
        get: f!(0x73bf_8338),
        delete: f!(0xe4a2_6362),
        delete_profile: f!(0x1709_3206),
        driver: f!(0x2926_aaad),
        values: f!(0x2ec3_9f90),
    })
}

/// Carrega a NVAPI, faz o trabalho e solta. Sem driver NVIDIA devolve None.
fn with_api<T>(work: impl FnOnce(&Api) -> Option<T>) -> Option<T> {
    unsafe {
        let module = LoadLibraryExW(w!("nvapi64.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
        let run = || -> Option<T> {
            let query: Query = std::mem::transmute(GetProcAddress(module, s!("nvapi_QueryInterface"))?);
            work(&api(query)?)
        };
        let out = run();
        let _ = FreeLibrary(module);
        out
    }
}

/// Abre o banco de perfis, faz o trabalho e fecha.
fn with_session<T>(work: impl FnOnce(&Api, H) -> Option<T>) -> Option<T> {
    with_session_from(None, work)
}

/// Igual, mas com o banco lido de um arquivo em vez do que o driver usa.
fn with_session_from<T>(file: Option<&Path>, work: impl FnOnce(&Api, H) -> Option<T>) -> Option<T> {
    with_api(|a| unsafe {
        let mut session: H = std::ptr::null_mut();
        if (a.create)(&mut session) != 0 {
            return None;
        }
        let loaded = match file {
            Some(f) => (a.load_file)(session, wide_name(&f.to_string_lossy()).as_ptr()),
            None => (a.load)(session),
        };
        let out = if loaded == 0 { work(a, session) } else { None };
        (a.destroy)(session);
        out
    })
}

/// Versão do driver em centésimos: 58197 é o 581.97. None sem placa NVIDIA.
pub fn driver_version() -> Option<u32> {
    static V: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        with_api(|a| unsafe {
            let (mut v, mut branch) = (0u32, [0u8; 64]);
            ((a.driver)(&mut v, branch.as_mut_ptr()) == 0 && v > 0).then_some(v)
        })
    })
}

/// De quem são os valores: o perfil global ou o de um jogo (pelo nome do .exe).
#[derive(Clone, PartialEq, Debug)]
pub enum Profile {
    Global,
    Game(String),
}

unsafe fn global_of(a: &Api, s: H) -> Option<H> {
    let mut p: H = std::ptr::null_mut();
    ((a.global)(s, &mut p) == 0).then_some(p)
}

unsafe fn game_profile(a: &Api, s: H, exe: &str, create: bool) -> Option<H> {
    let name = wide_name(&exe.to_lowercase());
    let mut app = vec![0u8; APP_SIZE];
    app[..4].copy_from_slice(&version(APP_SIZE, 4).to_le_bytes());
    let mut p: H = std::ptr::null_mut();
    let found = (a.find_app)(s, name.as_ptr(), &mut p, app.as_mut_ptr());
    if found == 0 {
        return Some(p);
    }
    if !create || !NOT_FOUND.contains(&found) && found >= 0 {
        return None;
    }
    let title = wide_name(&format!("Prisma {}", exe.to_lowercase()));
    if (a.find_profile)(s, title.as_ptr(), &mut p) != 0 {
        let mut profile = vec![0u8; PROFILE_SIZE];
        profile[..4].copy_from_slice(&version(PROFILE_SIZE, 1).to_le_bytes());
        std::ptr::copy_nonoverlapping(title.as_ptr() as *const u8, profile.as_mut_ptr().add(4), NAME_BYTES);
        if (a.new_profile)(s, profile.as_ptr(), &mut p) != 0 {
            return None;
        }
    }
    let mut app = vec![0u8; APP_SIZE];
    app[..4].copy_from_slice(&version(APP_SIZE, 4).to_le_bytes());
    std::ptr::copy_nonoverlapping(name.as_ptr() as *const u8, app.as_mut_ptr().add(8), NAME_BYTES);
    ((a.new_app)(s, p, app.as_mut_ptr()) == 0).then_some(p)
}

unsafe fn profile_of(a: &Api, s: H, who: &Profile, create: bool) -> Option<H> {
    match who {
        Profile::Global => global_of(a, s),
        Profile::Game(exe) => game_profile(a, s, exe, create),
    }
}

/// Nome e número de opções gravadas no perfil.
unsafe fn info_of(a: &Api, s: H, p: H) -> Option<(String, u32)> {
    let mut b = vec![0u8; PROFILE_SIZE];
    b[..4].copy_from_slice(&version(PROFILE_SIZE, 1).to_le_bytes());
    if (a.info)(s, p, b.as_mut_ptr()) != 0 {
        return None;
    }
    let w: Vec<u16> = b[4..4 + NAME_BYTES].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|c| *c != 0).collect();
    Some((String::from_utf16_lossy(&w), u32_at(&b, PROFILE_SETTINGS_AT)))
}

/// Uma opção como o driver a vê neste perfil.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Value {
    /// O valor que vale agora, venha de onde vier.
    pub now: u32,
    /// Gravado neste perfil por alguém, e diferente do que a NVIDIA deixou nele.
    pub mine: bool,
    /// O que a NVIDIA deixou neste perfil, quando deixou algo.
    pub predefined: Option<u32>,
}

/// O que um perfil tem para cada opção pedida.
pub struct Snapshot {
    /// Nome do perfil no driver. Vazio quando o jogo ainda não tem perfil.
    pub name: String,
    /// Mesma ordem dos ids pedidos; None quando o driver não diz nada.
    pub values: Vec<Option<Value>>,
}

unsafe fn get_value(a: &Api, s: H, p: H, id: u32, own: bool) -> Option<Value> {
    let mut set = vec![0u8; SETTING_SIZE];
    set[..4].copy_from_slice(&version(SETTING_SIZE, 1).to_le_bytes());
    let mut flag = 0u32;
    if (a.get)(s, p, id, set.as_mut_ptr(), &mut flag) != 0 || u32_at(&set, SETTING_TYPE_AT) != 0 {
        return None;
    }
    let here = own && u32_at(&set, SETTING_LOCATION_AT) == LOCATION_CURRENT;
    let predefined = (here && u32_at(&set, SETTING_PREDEFINED_VALID_AT) != 0).then(|| u32_at(&set, SETTING_PREDEFINED_AT));
    Some(Value { now: u32_at(&set, SETTING_VALUE_AT), mine: here && u32_at(&set, SETTING_IS_PREDEFINED_AT) == 0, predefined })
}

/// O valor que o próprio driver usa quando ninguém gravou nada.
unsafe fn driver_default(a: &Api, id: u32) -> Option<u32> {
    let mut b = vec![0u8; VALUES_SIZE];
    b[..4].copy_from_slice(&version(VALUES_SIZE, 1).to_le_bytes());
    let mut n = 100u32;
    ((a.values)(id, &mut n, b.as_mut_ptr()) == 0 && u32_at(&b, 8) == 0).then(|| u32_at(&b, VALUES_DEFAULT_AT))
}

/// Lê as opções de um perfil. Jogo sem perfil mostra o que vem do global.
pub fn snapshot(who: &Profile, ids: &[u32]) -> Option<Snapshot> {
    with_session(|a, s| unsafe {
        let (p, own) = match profile_of(a, s, who, false) {
            Some(p) => (p, true),
            None => (global_of(a, s)?, false),
        };
        let name = if own { info_of(a, s, p).map(|i| i.0).unwrap_or_default() } else { String::new() };
        let values = ids
            .iter()
            .map(|id| get_value(a, s, p, *id, own).or_else(|| driver_default(a, *id).map(|now| Value { now, mine: false, predefined: None })))
            .collect();
        Some(Snapshot { name, values })
    })
}

static BACKED_UP: AtomicBool = AtomicBool::new(false);

pub fn backup_dir() -> PathBuf {
    crate::config::data_dir().join("nvidia")
}

/// Cópias guardadas, a mais nova primeiro.
pub fn backups() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(backup_dir())
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "nvdrs")).collect())
        .unwrap_or_default();
    v.sort();
    v.reverse();
    v
}

/// Guarda o banco como está, uma vez por abertura do Prisma, antes da primeira mudança.
unsafe fn backup_once(a: &Api, s: H) -> bool {
    if BACKED_UP.load(Ordering::Relaxed) {
        return true;
    }
    let dir = backup_dir();
    let _ = std::fs::create_dir_all(&dir);
    let stamp = crate::sys::tools::now_stamp().replace([':', ' '], "-");
    let file = dir.join(format!("perfis-{stamp}.nvdrs"));
    if (a.save_file)(s, wide_name(&file.to_string_lossy()).as_ptr()) != 0 {
        return false;
    }
    BACKED_UP.store(true, Ordering::Relaxed);
    for old in backups().into_iter().skip(BACKUPS_KEPT) {
        let _ = std::fs::remove_file(old);
    }
    true
}

/// Grava valores no perfil, criando o do jogo se preciso. `None` volta ao padrão.
/// Antes da primeira mudança desta abertura, guarda uma cópia do banco inteiro.
pub fn write(who: &Profile, values: &[(u32, Option<u32>)]) -> bool {
    write_with(who, values, true)
}

fn write_with(who: &Profile, values: &[(u32, Option<u32>)], backup: bool) -> bool {
    with_session(|a, s| unsafe {
        let wants_value = values.iter().any(|v| v.1.is_some());
        if backup && !backup_once(a, s) {
            return Some(false);
        }
        let Some(p) = profile_of(a, s, who, wants_value) else {
            // Sem perfil e sem valor a gravar, não há nada para limpar.
            return Some(!wants_value);
        };
        for (id, value) in values {
            match value {
                Some(v) => {
                    let mut set = vec![0u8; SETTING_SIZE];
                    set[..4].copy_from_slice(&version(SETTING_SIZE, 1).to_le_bytes());
                    set[SETTING_ID_AT..SETTING_ID_AT + 4].copy_from_slice(&id.to_le_bytes());
                    set[SETTING_VALUE_AT..SETTING_VALUE_AT + 4].copy_from_slice(&v.to_le_bytes());
                    if (a.set)(s, p, set.as_ptr()) != 0 {
                        return Some(false);
                    }
                }
                None => {
                    (a.delete)(s, p, *id);
                }
            }
        }
        Some((a.save)(s) == 0)
    })
    .unwrap_or(false)
}

/// Volta o banco inteiro a uma cópia guardada.
pub fn restore(file: &Path) -> bool {
    with_session_from(Some(file), |a, s| unsafe { Some((a.save)(s) == 0) }).unwrap_or(false)
}

/// Grava os valores do Medidor no perfil do jogo. `None` volta ao padrão.
pub fn apply(exe: &str, values: &[(u32, Option<u32>)]) -> bool {
    write_with(&Profile::Game(exe.to_string()), values, false)
}

/// Valor gravado no perfil deste jogo, sem contar o que vem do global ou do driver.
pub fn read(exe: &str, id: u32) -> Option<u32> {
    with_session(|a, s| unsafe {
        let p = game_profile(a, s, exe, false)?;
        get_value(a, s, p, id, true).filter(|v| v.mine || v.predefined.is_some()).map(|v| v.now)
    })
}

/// Tira o que o Prisma gravou no jogo. O perfil criado por ele sai junto se ficou vazio.
pub fn forget(exe: &str) {
    apply(exe, &[(FRAME_LIMIT, None), (DLSS_PRESET, None), (DLSS_OVERRIDE, None)]);
    let _ = with_session(|a, s| unsafe {
        let title = wide_name(&format!("Prisma {}", exe.to_lowercase()));
        let mut p: H = std::ptr::null_mut();
        if (a.find_profile)(s, title.as_ptr(), &mut p) == 0 && info_of(a, s, p).is_some_and(|i| i.1 == 0) && (a.delete_profile)(s, p) == 0 {
            (a.save)(s);
        }
        Some(())
    });
}

/// Teste: grava, lê e apaga. Devolve o que aconteceu em cada passo.
#[cfg(debug_assertions)]
pub fn test(exe: &str, fps: u32) -> String {
    let before = read(exe, FRAME_LIMIT);
    let wrote = apply(exe, &[(FRAME_LIMIT, Some(fps))]);
    format!("antes {before:?} gravou {wrote} agora {:?}", read(exe, FRAME_LIMIT))
}

/// Teste da tela Driver NVIDIA, só no perfil do .exe de teste: lê o global, grava duas opções,
/// confere na cópia guardada que ela é de antes e apaga tudo. Não volta a cópia ao driver.
#[cfg(debug_assertions)]
pub fn test_page(exe: &str) -> String {
    use crate::sys::nvset;
    let ids = nvset::ids();
    let mut out = format!("driver {:?}
", driver_version());
    let global = snapshot(&Profile::Global, &ids);
    out += &format!("global: {:?} lidas {}
", global.as_ref().map(|s| s.name.clone()), global.as_ref().map_or(0, |s| s.values.iter().flatten().count()));
    let game = Profile::Game(exe.to_string());
    let before = snapshot(&game, &ids).map(|s| s.name);
    let wrote = write(&game, &[(0x1057_EB71, Some(1)), (0x0064_B541, Some(1))]);
    let after = snapshot(&game, &[0x1057_EB71, 0x0064_B541, FRAME_LIMIT]);
    out += &format!("antes {before:?} gravou {wrote} depois {:?} {:?}
", after.as_ref().map(|s| s.name.clone()), after.as_ref().map(|s| s.values.clone()));
    let copy = backups().into_iter().next();
    let in_copy = copy.as_ref().and_then(|f| with_session_from(Some(f), |a, s| unsafe { Some(game_profile(a, s, exe, false).is_some()) }));
    out += &format!("cópia {:?} tem o perfil de teste: {in_copy:?}
", copy.map(|f| f.file_name().unwrap().to_string_lossy().into_owned()));
    write(&game, &[(0x1057_EB71, None), (0x0064_B541, None)]);
    forget(exe);
    out += &format!("no fim: {:?}
", snapshot(&game, &[0x1057_EB71]).map(|s| (s.name, s.values)));
    out
}

/// Teste: tira o perfil criado pelo Prisma.
#[cfg(debug_assertions)]
pub fn test_clear(exe: &str) -> String {
    forget(exe);
    format!("agora {:?}", read(exe, FRAME_LIMIT))
}
