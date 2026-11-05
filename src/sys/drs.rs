//! Perfil do driver NVIDIA por jogo: limite de FPS e preset do DLSS, sem tocar no jogo.

use std::ffi::c_void;

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
const SETTING_VALUE_AT: usize = SETTING_ID_AT + 20 + 4100;
const LOCATION_CURRENT: u32 = 0;
const NOT_FOUND: [i32; 2] = [-163, -166];

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

struct Api {
    create: unsafe extern "C" fn(*mut H) -> i32,
    destroy: unsafe extern "C" fn(H) -> i32,
    load: unsafe extern "C" fn(H) -> i32,
    save: unsafe extern "C" fn(H) -> i32,
    find_app: unsafe extern "C" fn(H, *const u16, *mut H, *mut u8) -> i32,
    find_profile: unsafe extern "C" fn(H, *const u16, *mut H) -> i32,
    new_profile: unsafe extern "C" fn(H, *const u8, *mut H) -> i32,
    new_app: unsafe extern "C" fn(H, H, *mut u8) -> i32,
    set: unsafe extern "C" fn(H, H, *const u8) -> i32,
    get: unsafe extern "C" fn(H, H, u32, *mut u8, *mut u32) -> i32,
    delete: unsafe extern "C" fn(H, H, u32) -> i32,
    delete_profile: unsafe extern "C" fn(H, H) -> i32,
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
        find_app: f!(0xeee5_66b2),
        find_profile: f!(0x7e4a_9a0b),
        new_profile: f!(0xcc17_6068),
        new_app: f!(0x4347_a9de),
        set: f!(0x577d_d202),
        get: f!(0x73bf_8338),
        delete: f!(0xe4a2_6362),
        delete_profile: f!(0x1709_3206),
    })
}

/// Abre o banco de perfis, faz o trabalho e fecha. Sem driver NVIDIA devolve None.
fn with_session<T>(work: impl FnOnce(&Api, H) -> Option<T>) -> Option<T> {
    unsafe {
        let module = LoadLibraryExW(w!("nvapi64.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
        let run = || -> Option<T> {
            let query: Query = std::mem::transmute(GetProcAddress(module, s!("nvapi_QueryInterface"))?);
            let a = api(query)?;
            let mut session: H = std::ptr::null_mut();
            if (a.create)(&mut session) != 0 {
                return None;
            }
            let out = if (a.load)(session) == 0 { work(&a, session) } else { None };
            (a.destroy)(session);
            out
        };
        let out = run();
        let _ = FreeLibrary(module);
        out
    }
}

unsafe fn profile_of(a: &Api, s: H, exe: &str, create: bool) -> Option<H> {
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

/// Grava valores no perfil do jogo, criando o perfil se preciso. `None` volta ao padrão.
pub fn apply(exe: &str, values: &[(u32, Option<u32>)]) -> bool {
    with_session(|a, s| unsafe {
        let wants_value = values.iter().any(|v| v.1.is_some());
        let Some(p) = profile_of(a, s, exe, wants_value) else {
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

/// Valor gravado neste jogo, sem contar o que vem de perfis globais ou do driver.
pub fn read(exe: &str, id: u32) -> Option<u32> {
    with_session(|a, s| unsafe {
        let p = profile_of(a, s, exe, false)?;
        let mut set = vec![0u8; SETTING_SIZE];
        set[..4].copy_from_slice(&version(SETTING_SIZE, 1).to_le_bytes());
        let mut flag = 0u32;
        if (a.get)(s, p, id, set.as_mut_ptr(), &mut flag) != 0 {
            return None;
        }
        let location = u32::from_le_bytes(set[SETTING_LOCATION_AT..SETTING_LOCATION_AT + 4].try_into().ok()?);
        let kind = u32::from_le_bytes(set[SETTING_TYPE_AT..SETTING_TYPE_AT + 4].try_into().ok()?);
        (location == LOCATION_CURRENT && kind == 0).then(|| u32::from_le_bytes(set[SETTING_VALUE_AT..SETTING_VALUE_AT + 4].try_into().unwrap()))
    })
}

/// Tira o que o Prisma gravou no jogo. Perfil criado por ele sai junto, já vazio.
pub fn forget(exe: &str) {
    apply(exe, &[(FRAME_LIMIT, None), (DLSS_PRESET, None), (DLSS_OVERRIDE, None)]);
    let _ = with_session(|a, s| unsafe {
        let title = wide_name(&format!("Prisma {}", exe.to_lowercase()));
        let mut p: H = std::ptr::null_mut();
        if (a.find_profile)(s, title.as_ptr(), &mut p) == 0 && (a.delete_profile)(s, p) == 0 {
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

/// Teste: tira o perfil criado pelo Prisma.
#[cfg(debug_assertions)]
pub fn test_clear(exe: &str) -> String {
    forget(exe);
    format!("agora {:?}", read(exe, FRAME_LIMIT))
}
