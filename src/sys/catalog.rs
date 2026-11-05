//! Programas para escolher: tudo o que o menu Iniciar mostra, inclusive os da Microsoft Store,
//! mais os que estão na lista de instalados e os que estão abertos agora.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};

use windows::core::{Interface, GUID, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::*;

#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub path: String,
}

/// `None` enquanto a lista ainda está sendo montada.
pub type Shared = Arc<Mutex<Option<Vec<Entry>>>>;

const MODEL: GUID = GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3);
const LINK: GUID = GUID::from_u128(0xb9b4b3fc_2b51_4a42_b5d8_324146afcf25);
const APP_ID: PROPERTYKEY = PROPERTYKEY { fmtid: MODEL, pid: 5 };
const PACKAGE_PATH: PROPERTYKEY = PROPERTYKEY { fmtid: MODEL, pid: 15 };
const LINK_TARGET: PROPERTYKEY = PROPERTYKEY { fmtid: LINK, pid: 2 };

fn taken(p: PWSTR) -> String {
    unsafe {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

/// Executável de um aplicativo da Loja, que o pacote declara no manifesto.
/// Alguns declaram só um iniciador: o programa em si leva o nome do aplicativo.
fn package_exe(dir: &str, app_id: &str, name: &str) -> Option<String> {
    let manifest = std::fs::read_to_string(Path::new(dir).join("AppxManifest.xml")).ok()?;
    let id = app_id.split('!').nth(1)?;
    let at = manifest.find(&format!("Id=\"{id}\""))?;
    let start = manifest[..at].rfind('<')?;
    let tag = &manifest[start..start + manifest[start..].find('>')?];
    let exe = &tag[tag.find("Executable=\"")? + 12..];
    let path = Path::new(dir).join(&exe[..exe.find('"')?]);
    let own = path.with_file_name(format!("{name}.exe"));
    Some(if own.is_file() { own } else { path }.display().to_string())
}

/// Instaladores e desinstaladores não são o programa.
fn is_program(name: &str, path: &str) -> bool {
    let low = path.to_ascii_lowercase();
    let file = low.rsplit('\\').next().unwrap_or_default();
    let title = name.to_lowercase();
    low.ends_with(".exe")
        && !low.contains("\\installer\\")
        && !low.contains("\\package cache\\")
        && !file.starts_with("unins")
        && !file.contains("setup")
        && !file.contains("uninstall")
        && !matches!(file, "rundll32.exe" | "mmc.exe" | "control.exe")
        && !title.contains("uninstall")
        && !title.contains("desinstal")
        && Path::new(path).is_file()
}

fn start_menu(out: &mut Vec<Entry>) {
    unsafe {
        let Ok(folder) = SHGetKnownFolderItem::<IShellItem>(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None) else { return };
        let Ok(items) = folder.BindToHandler::<_, IEnumShellItems>(None, &BHID_EnumItems) else { return };
        loop {
            let mut one = [None];
            let mut got = 0;
            if items.Next(&mut one, Some(&mut got)).is_err() || got == 0 {
                break;
            }
            let Some(item) = one[0].take() else { break };
            let Ok(item) = item.cast::<IShellItem2>() else { continue };
            let Ok(name) = item.GetDisplayName(SIGDN_NORMALDISPLAY).map(|p| taken(p)) else { continue };
            let mut path = item.GetString(&LINK_TARGET).map(|p| taken(p)).unwrap_or_default();
            if path.is_empty() {
                let dir = item.GetString(&PACKAGE_PATH).map(|p| taken(p)).unwrap_or_default();
                let id = item.GetString(&APP_ID).map(|p| taken(p)).unwrap_or_default();
                path = package_exe(&dir, &id, &name).unwrap_or_default();
            }
            out.push(Entry { name, path });
        }
    }
}

/// `open` traz nome do executável e caminho do que está aberto agora.
pub fn list(open: Vec<(String, String)>) -> Vec<Entry> {
    let mut all = Vec::new();
    start_menu(&mut all);
    for p in crate::sys::uninstall::list() {
        all.push(Entry { name: p.name, path: p.icon });
    }
    for (exe, path) in open {
        all.push(Entry { name: crate::pages::app_name(&exe).to_string(), path });
    }
    let mut seen = HashSet::new();
    all.retain(|e| is_program(&e.name, &e.path) && seen.insert(e.path.to_lowercase()));
    all.sort_by_key(|e| e.name.to_lowercase());
    all
}

pub fn load(shared: Shared, open: Vec<(String, String)>, window: isize) {
    let _ = std::thread::Builder::new().name("programas".into()).spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let found = list(open);
        CoUninitialize();
        *shared.lock().unwrap() = Some(found);
        let _ = InvalidateRect(Some(HWND(window as *mut _)), None, false);
    });
}
