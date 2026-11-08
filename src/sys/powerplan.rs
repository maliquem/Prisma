//! Edição completa de um plano de energia: tudo o que o Windows registra, inclusive o oculto.

use std::os::windows::process::CommandExt;
use std::sync::{Arc, Mutex};

use windows::core::GUID;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::System::Power::*;

use super::power::parse_guid;

/// Opções do Windows que não pertencem a nenhum grupo.
const NO_GROUP: GUID = GUID::from_u128(0xfea3413e_7e05_4911_9a71_700331f1c294);
const HIDDEN: u32 = 1;
const NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug)]
pub enum Kind {
    Options(Vec<(String, u32)>),
    Range { min: u32, max: u32, step: u32, unit: String },
}

#[derive(Clone, Debug)]
pub struct Setting {
    pub group: String,
    pub sub: GUID,
    pub id: GUID,
    pub name: String,
    pub hidden: bool,
    pub kind: Kind,
    pub ac: u32,
    pub dc: u32,
}

/// `None` enquanto a lista é montada.
pub type Shared = Arc<Mutex<Option<Vec<Setting>>>>;

fn text(buf: &[u16]) -> String {
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn name_of(sub: Option<&GUID>, setting: Option<&GUID>) -> String {
    unsafe {
        let mut size = 1024u32;
        let mut buf = vec![0u16; size as usize / 2];
        let r = PowerReadFriendlyName(None, None, sub.map(|g| g as *const _), setting.map(|g| g as *const _), Some(buf.as_mut_ptr() as *mut u8), &mut size);
        if r.0 == 0 {
            text(&buf)
        } else {
            String::new()
        }
    }
}

fn guids(sub: Option<&GUID>, access: POWER_DATA_ACCESSOR) -> Vec<GUID> {
    let mut out = Vec::new();
    unsafe {
        for i in 0.. {
            let mut g = GUID::zeroed();
            let mut size = std::mem::size_of::<GUID>() as u32;
            let r = PowerEnumerate(None, None, sub.map(|g| g as *const _), access, i, Some(&mut g as *mut GUID as *mut u8), &mut size);
            if r.0 != 0 {
                break;
            }
            out.push(g);
        }
    }
    out
}

/// Alguns drivers dão só o nome de cada opção: aí o valor é a própria posição.
fn options(sub: &GUID, id: &GUID) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    unsafe {
        for i in 0..256u32 {
            let mut len = 512u32;
            let mut buf = vec![0u16; len as usize / 2];
            if PowerReadPossibleFriendlyName(None, Some(sub), Some(id), i, Some(buf.as_mut_ptr() as *mut u8), &mut len).0 != 0 {
                break;
            }
            let (mut value, mut size) = (i, 4u32);
            let _ = PowerReadPossibleValue(None, Some(sub), Some(id), None, i, Some(&mut value as *mut u32 as *mut u8), &mut size);
            out.push((text(&buf), value));
        }
    }
    out
}

/// Faixa de valores. Quem não tem máximo é opção de lista.
fn range(sub: &GUID, id: &GUID) -> Option<Kind> {
    let (mut min, mut max, mut step) = (0u32, 0u32, 1u32);
    let mut size = 256u32;
    let mut buf = vec![0u16; size as usize / 2];
    unsafe {
        if PowerReadValueMax(None, Some(sub), Some(id), &mut max).0 != 0 {
            return None;
        }
        let _ = PowerReadValueMin(None, Some(sub), Some(id), &mut min);
        let _ = PowerReadValueIncrement(None, Some(sub), Some(id), &mut step);
        let _ = PowerReadValueUnitsSpecifier(None, Some(sub), Some(id), Some(buf.as_mut_ptr() as *mut u8), &mut size);
    }
    let unit = text(&buf);
    Some(Kind::Range { min, max: max.max(min), step: step.max(1), unit: if unit.is_empty() { unit } else { format!(" {unit}") } })
}

pub fn read(scheme: &GUID) -> Vec<Setting> {
    let mut out = Vec::new();
    let mut groups = guids(None, ACCESS_SUBGROUP);
    groups.push(NO_GROUP);
    for sub in groups {
        let group = if sub == NO_GROUP { "Geral".to_string() } else { name_of(Some(&sub), None) };
        for id in guids(Some(&sub), ACCESS_INDIVIDUAL_SETTING) {
            let hidden = unsafe { PowerReadSettingAttributes(Some(&sub), Some(&id)) } & HIDDEN != 0;
            let kind = range(&sub, &id).unwrap_or_else(|| Kind::Options(options(&sub, &id)));
            let (mut ac, mut dc) = (0u32, 0u32);
            unsafe {
                let _ = PowerReadACValueIndex(None, Some(scheme), Some(&sub), Some(&id), &mut ac);
                let _ = PowerReadDCValueIndex(None, Some(scheme), Some(&sub), Some(&id), &mut dc);
            }
            let name = name_of(Some(&sub), Some(&id));
            if !name.is_empty() {
                out.push(Setting { group: group.clone(), sub, id, name, hidden, kind, ac, dc });
            }
        }
    }
    out
}

pub fn load(shared: Shared, scheme: GUID, window: isize) {
    *shared.lock().unwrap() = None;
    let _ = std::thread::Builder::new().name("plano".into()).spawn(move || {
        let list = read(&scheme);
        *shared.lock().unwrap() = Some(list);
        unsafe {
            let _ = InvalidateRect(Some(HWND(window as *mut _)), None, false);
        }
    });
}

/// Grava um valor. No plano ativo, o Windows só aplica ao ativar de novo.
pub fn write(scheme: &GUID, s: &Setting, dc: bool, value: u32, active: bool) -> bool {
    unsafe {
        let done = if dc {
            PowerWriteDCValueIndex(None, scheme, Some(&s.sub), Some(&s.id), value) == 0
        } else {
            PowerWriteACValueIndex(None, scheme, Some(&s.sub), Some(&s.id), value).0 == 0
        };
        if !done {
            return false;
        }
        if active {
            let _ = PowerSetActiveScheme(None, Some(scheme));
        }
        true
    }
}

pub fn has_battery() -> bool {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    const NO_BATTERY: u8 = 128;
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s) }.is_ok() && s.BatteryFlag & NO_BATTERY == 0
}

fn powercfg(args: &[&str]) -> Option<String> {
    let tool = super::system_tool("powercfg");
    let out = std::process::Command::new(tool).args(args).creation_flags(NO_WINDOW).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

/// Exportar exige administrador. Sem isso o Windows deixa um arquivo vazio, que aqui não fica.
pub fn export(path: &str, id: &str) -> bool {
    let done = powercfg(&["/export", path, id]).is_some() && std::fs::metadata(path).is_ok_and(|m| m.len() > 0);
    if !done {
        let _ = std::fs::remove_file(path);
    }
    done
}

/// Devolve o id do plano novo.
pub fn import(path: &str) -> Option<String> {
    if std::fs::metadata(path).map_or(true, |m| m.len() == 0) {
        return None;
    }
    let out = powercfg(&["/import", path])?;
    out.split(|c: char| !c.is_ascii_hexdigit() && c != '-').find(|p| p.len() == 36 && parse_guid(p).is_some()).map(str::to_ascii_lowercase)
}

/// Guarda o plano como ele estava antes da primeira edição.
pub fn backup(dir: &std::path::Path, id: &str) {
    let path = dir.join(format!("{id}.pow"));
    if !path.exists() && std::fs::create_dir_all(dir).is_ok() {
        export(&path.display().to_string(), id);
    }
}
