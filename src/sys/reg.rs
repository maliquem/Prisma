//! Acesso ao registro usado pelo catálogo de ajustes e pela auditoria.

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Registry::*;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hive {
    User,
    Machine,
}

impl Hive {
    fn key(self) -> HKEY {
        match self {
            Hive::User => HKEY_CURRENT_USER,
            Hive::Machine => HKEY_LOCAL_MACHINE,
        }
    }
    pub fn tag(self) -> &'static str {
        match self {
            Hive::User => "HKCU",
            Hive::Machine => "HKLM",
        }
    }
    pub fn parse(s: &str) -> Hive {
        if s == "HKLM" {
            Hive::Machine
        } else {
            Hive::User
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Tipo e bytes do valor, ou None quando ele não existe.
pub fn read(h: Hive, key: &str, name: &str) -> Option<(u32, Vec<u8>)> {
    unsafe {
        let (k, n) = (wide(key), wide(name));
        let mut ty = REG_VALUE_TYPE(0);
        let mut size = 0u32;
        let r = RegGetValueW(
            h.key(),
            PCWSTR(k.as_ptr()),
            PCWSTR(n.as_ptr()),
            RRF_RT_ANY | RRF_NOEXPAND,
            Some(&mut ty),
            None,
            Some(&mut size),
        );
        if r != ERROR_SUCCESS && r != ERROR_MORE_DATA {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        let r = RegGetValueW(
            h.key(),
            PCWSTR(k.as_ptr()),
            PCWSTR(n.as_ptr()),
            RRF_RT_ANY | RRF_NOEXPAND,
            Some(&mut ty),
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        );
        (r == ERROR_SUCCESS).then(|| {
            buf.truncate(size as usize);
            (ty.0, buf)
        })
    }
}

pub fn dword(h: Hive, key: &str, name: &str) -> Option<u32> {
    let (t, b) = read(h, key, name)?;
    (t == REG_DWORD.0 && b.len() >= 4).then(|| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

pub fn text(h: Hive, key: &str, name: &str) -> Option<String> {
    let (_, b) = read(h, key, name)?;
    let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let end = w.iter().position(|c| *c == 0).unwrap_or(w.len());
    Some(String::from_utf16_lossy(&w[..end]))
}

pub fn write(h: Hive, key: &str, name: &str, ty: u32, data: &[u8]) -> bool {
    unsafe {
        let (k, n) = (wide(key), wide(name));
        RegSetKeyValueW(
            h.key(),
            PCWSTR(k.as_ptr()),
            PCWSTR(n.as_ptr()),
            ty,
            Some(data.as_ptr() as *const _),
            data.len() as u32,
        ) == ERROR_SUCCESS
    }
}

pub fn write_dword(h: Hive, key: &str, name: &str, v: u32) -> bool {
    write(h, key, name, REG_DWORD.0, &v.to_le_bytes())
}

pub fn delete(h: Hive, key: &str, name: &str) -> bool {
    unsafe {
        let (k, n) = (wide(key), wide(name));
        let r = RegDeleteKeyValueW(h.key(), PCWSTR(k.as_ptr()), PCWSTR(n.as_ptr()));
        r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND
    }
}

pub fn exists(h: Hive, key: &str) -> bool {
    unsafe {
        let k = wide(key);
        let mut out = HKEY::default();
        let ok = RegOpenKeyExW(h.key(), PCWSTR(k.as_ptr()), None, KEY_READ, &mut out).is_ok();
        if ok {
            let _ = RegCloseKey(out);
        }
        ok
    }
}

pub fn subkeys(h: Hive, key: &str) -> Vec<String> {
    let mut list = Vec::new();
    unsafe {
        let k = wide(key);
        let mut out = HKEY::default();
        if RegOpenKeyExW(h.key(), PCWSTR(k.as_ptr()), None, KEY_READ, &mut out).is_err() {
            return list;
        }
        let mut i = 0;
        loop {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            if RegEnumKeyExW(out, i, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None).is_err() {
                break;
            }
            list.push(String::from_utf16_lossy(&name[..len as usize]));
            i += 1;
        }
        let _ = RegCloseKey(out);
    }
    list
}

/// Quando a chave foi escrita pela última vez.
pub fn written(h: Hive, key: &str) -> u64 {
    unsafe {
        let k = wide(key);
        let mut out = HKEY::default();
        if RegOpenKeyExW(h.key(), PCWSTR(k.as_ptr()), None, KEY_READ, &mut out).is_err() {
            return 0;
        }
        let mut t = FILETIME::default();
        let _ = RegQueryInfoKeyW(out, None, None, None, None, None, None, None, None, None, None, Some(&mut t));
        let _ = RegCloseKey(out);
        ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64
    }
}

pub fn subkey_count(h: Hive, key: &str) -> u32 {
    unsafe {
        let k = wide(key);
        let mut out = HKEY::default();
        if RegOpenKeyExW(h.key(), PCWSTR(k.as_ptr()), None, KEY_READ, &mut out).is_err() {
            return 0;
        }
        let mut n = 0u32;
        let _ = RegQueryInfoKeyW(out, None, None, None, Some(&mut n), None, None, None, None, None, None, None);
        let _ = RegCloseKey(out);
        n
    }
}
