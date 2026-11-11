//! O link do convite: `prisma://parear/K7M2-QX9P` abre o Prisma e pareia, depois de perguntar. A
//! página do link (`PAGE`) só repassa o código para o `prisma://`, porque os aplicativos de conversa
//! não deixam clicar num endereço que não seja da web.

use windows::core::*;
use windows::Win32::System::Registry::*;

use super::code;

/// A página que leva ao `prisma://` (com o código depois do #, que não vai para o servidor). Um
/// endereço curto, numa página do GitHub só para isso; a antiga (`maliquem.github.io/Prisma/c/`)
/// continua no ar para os links das versões anteriores.
pub const PAGE: &str = "https://prismashare.github.io/c/#";
const KEY: PCWSTR = w!("Software\\Classes\\prisma");

pub fn web_link(shown: &str) -> String {
    format!("{PAGE}{shown}")
}

fn pending_path() -> std::path::PathBuf {
    super::user_dir().join("link.txt")
}

/// Na abertura: um argumento `prisma://...` vira um pedido para a página. Devolve true se havia.
pub fn take_arg() -> bool {
    let Some(arg) = std::env::args().skip(1).find(|a| a.to_ascii_lowercase().starts_with("prisma:")) else { return false };
    let Some(raw) = code::normalize(&arg) else { return false };
    let _ = std::fs::create_dir_all(super::user_dir());
    std::fs::write(pending_path(), code::show(&raw)).is_ok()
}

/// O código que chegou por link há pouco, uma vez só.
pub fn take_pending() -> Option<String> {
    let p = pending_path();
    let fresh = std::fs::metadata(&p).ok()?.modified().ok()?.elapsed().map_or(false, |d| d.as_secs() < 600);
    let s = std::fs::read_to_string(&p).ok();
    let _ = std::fs::remove_file(&p);
    let raw = code::normalize(&s?)?;
    fresh.then(|| code::show(&raw))
}

pub fn has_pending() -> bool {
    pending_path().exists()
}

fn command() -> String {
    let exe = std::env::current_exe().unwrap_or_default();
    format!("\"{}\" \"%1\"", exe.display())
}

/// Registra o `prisma://` para este usuário, apontando para este exe. Só escreve se mudou.
pub fn register() {
    if cfg!(debug_assertions) {
        return;
    }
    let want = command();
    unsafe {
        let cmd_key = w!("Software\\Classes\\prisma\\shell\\open\\command");
        let mut buf = [0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        let cur = RegGetValueW(HKEY_CURRENT_USER, cmd_key, None, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as _), Some(&mut len));
        if cur.is_ok() && String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]) == want {
            return;
        }
        let set = |key: PCWSTR, name: PCWSTR, value: &str| {
            let v: Vec<u16> = value.encode_utf16().chain([0]).collect();
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, key, name, REG_SZ.0, Some(v.as_ptr() as _), (v.len() * 2) as u32);
        };
        set(KEY, PCWSTR::null(), "URL:Prisma");
        set(KEY, w!("URL Protocol"), "");
        set(cmd_key, PCWSTR::null(), &want);
    }
}

/// Na desinstalação.
pub fn unregister() {
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, KEY);
    }
}

/// Abre a janela Compartilhar do Windows com o convite. Devolve false se o Windows recusou.
pub fn share_sheet(hwnd: windows::Win32::Foundation::HWND, title: &str, text: &str, link: &str) -> bool {
    use std::sync::Mutex;
    use windows::ApplicationModel::DataTransfer::*;
    use windows::Foundation::{TypedEventHandler, Uri};
    use windows::Win32::UI::Shell::IDataTransferManagerInterop;

    static CURRENT: Mutex<Option<(String, String, String)>> = Mutex::new(None);
    static HOOKED: Mutex<Vec<isize>> = Mutex::new(Vec::new());
    *CURRENT.lock().unwrap() = Some((title.to_string(), text.to_string(), link.to_string()));
    let run = || -> Result<()> {
        let interop: IDataTransferManagerInterop = factory::<DataTransferManager, IDataTransferManagerInterop>()?;
        let dtm: DataTransferManager = unsafe { interop.GetForWindow(hwnd)? };
        // Um tratador por janela: ele lê o convite do momento.
        let mut hooked = HOOKED.lock().unwrap();
        if !hooked.contains(&(hwnd.0 as isize)) {
            dtm.DataRequested(&TypedEventHandler::new(|_, args: windows_core::Ref<DataRequestedEventArgs>| {
                let Some(args) = args.as_ref() else { return Ok(()) };
                let Some((title, text, link)) = CURRENT.lock().unwrap().clone() else { return Ok(()) };
                let data = args.Request()?.Data()?;
                data.Properties()?.SetTitle(&HSTRING::from(title))?;
                data.SetText(&HSTRING::from(text))?;
                if let Ok(uri) = Uri::CreateUri(&HSTRING::from(link)) {
                    let _ = data.SetWebLink(&uri);
                }
                Ok(())
            }))?;
            hooked.push(hwnd.0 as isize);
        }
        unsafe { interop.ShowShareUIForWindow(hwnd) }
    };
    run().is_ok()
}
