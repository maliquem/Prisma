//! O que o próprio arquivo diz sobre si: nome do produto e assinatura digital.

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Security::Cryptography::*;
use windows::Win32::Security::WinTrust::*;
use windows::Win32::Storage::FileSystem::*;

/// Descrição gravada no executável, como "Google Chrome".
pub fn description(path: &str) -> Option<String> {
    unsafe {
        let wide = HSTRING::from(path);
        let size = GetFileVersionInfoSizeW(&wide, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(&wide, None, size, data.as_mut_ptr() as *mut _).ok()?;
        let mut p = std::ptr::null_mut();
        let mut n = 0;
        if !VerQueryValueW(data.as_ptr() as *const _, w!("\\VarFileInfo\\Translation"), &mut p, &mut n).as_bool() || n < 4 {
            return None;
        }
        let lang = p as *const u16;
        let (l, c) = (*lang, *lang.add(1));
        let key = HSTRING::from(format!("\\StringFileInfo\\{l:04x}{c:04x}\\FileDescription"));
        if !VerQueryValueW(data.as_ptr() as *const _, &key, &mut p, &mut n).as_bool() || n == 0 {
            return None;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p as *const u16, n as usize));
        // Alguns programas informam um tamanho maior que o texto: vale só até o primeiro zero.
        let s = s.split('\0').next().unwrap_or_default().trim().to_string();
        (!s.is_empty()).then_some(s)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Signature {
    /// Assinatura válida, com o nome de quem assinou.
    Valid(String),
    Invalid,
    None,
}

impl Signature {
    pub fn label(&self) -> String {
        match self {
            Signature::Valid(n) => format!("Assinado por {n}"),
            Signature::Invalid => "Assinatura inválida".to_string(),
            Signature::None => "Sem assinatura própria".to_string(),
        }
    }
}

unsafe fn signer(state: HANDLE) -> Option<String> {
    let prov = WTHelperProvDataFromStateData(state);
    if prov.is_null() {
        return None;
    }
    let sgnr = WTHelperGetProvSignerFromChain(prov, 0, false, 0);
    if sgnr.is_null() {
        return None;
    }
    let cert = WTHelperGetProvCertFromChain(sgnr, 0);
    if cert.is_null() {
        return None;
    }
    let mut buf = [0u16; 256];
    let n = CertGetNameStringW((*cert).pCert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, Some(&mut buf));
    (n > 1).then(|| String::from_utf16_lossy(&buf[..n as usize - 1]))
}

pub fn check(path: &str) -> Signature {
    unsafe {
        let wide = HSTRING::from(path);
        let mut file = WINTRUST_FILE_INFO {
            cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: PCWSTR(wide.as_ptr()),
            ..Default::default()
        };
        let mut data = WINTRUST_DATA {
            cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
            dwUIChoice: WTD_UI_NONE,
            fdwRevocationChecks: WTD_REVOKE_NONE,
            dwUnionChoice: WTD_CHOICE_FILE,
            dwStateAction: WTD_STATEACTION_VERIFY,
            dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL,
            ..Default::default()
        };
        data.Anonymous.pFile = &mut file;
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        let r = WinVerifyTrust(HWND::default(), &mut action, &mut data as *mut _ as *mut _);
        let out = if r == 0 {
            Signature::Valid(signer(data.hWVTStateData).unwrap_or_else(|| "desconhecido".into()))
        } else if r as u32 == 0x800B0100 {
            Signature::None
        } else {
            Signature::Invalid
        };
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        WinVerifyTrust(HWND::default(), &mut action, &mut data as *mut _ as *mut _);
        out
    }
}
