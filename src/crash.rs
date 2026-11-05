//! Se o Prisma cair, guarda um relatório local que mostra onde foi. Só um resumo sai daqui, em Reportar bug.

use std::path::PathBuf;
use std::sync::OnceLock;

use windows::core::HSTRING;
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Diagnostics::Debug::*;
use windows::Win32::System::Threading::*;

// Caminho pronto de antemão: depois da queda, alocar memória pode travar.
static DUMP: OnceLock<HSTRING> = OnceLock::new();

fn dump_path() -> PathBuf {
    crate::config::data_dir().join("ultima_falha.dmp")
}

unsafe extern "system" fn on_crash(info: *const EXCEPTION_POINTERS) -> i32 {
    if let Some(path) = DUMP.get() {
        if let Ok(f) = CreateFileW(path, FILE_GENERIC_WRITE.0, FILE_SHARE_NONE, None, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, None) {
            let e = MINIDUMP_EXCEPTION_INFORMATION {
                ThreadId: GetCurrentThreadId(),
                ExceptionPointers: info as *mut _,
                ClientPointers: false.into(),
            };
            let _ = MiniDumpWriteDump(GetCurrentProcess(), GetCurrentProcessId(), f, MINIDUMP_TYPE(MiniDumpNormal.0 | MiniDumpWithThreadInfo.0), Some(&e), None, None);
            let _ = CloseHandle(f);
        }
    }
    0
}

pub fn install() {
    let _ = std::fs::create_dir_all(crate::config::data_dir());
    let _ = DUMP.set(HSTRING::from(dump_path().as_path()));
    unsafe {
        SetUnhandledExceptionFilter(Some(on_crash));
    }
    std::panic::set_hook(Box::new(|p| {
        use std::io::Write;
        let path = crate::config::data_dir().join("acoes.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{} erro interno: {}", crate::sys::tools::now_stamp(), p.to_string().replace('\n', " "));
        }
    }));
}

/// Resume a última queda a partir do relatório local: código do erro e ponto do programa.
pub fn last() -> Option<String> {
    let path = dump_path().with_file_name("falha_anterior.dmp");
    let when = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
    let days = when.elapsed().map_or(0, |e| e.as_secs() / 86400);
    if days > 14 {
        return None;
    }
    let d = std::fs::read(&path).ok()?;
    let u32at = |o: usize| Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?));
    let u64at = |o: usize| Some(u64::from_le_bytes(d.get(o..o + 8)?.try_into().ok()?));
    let (count, dir) = (u32at(8)? as usize, u32at(12)? as usize);
    let stream = |kind: u32| (0..count).find_map(|i| (u32at(dir + i * 12)? == kind).then(|| u32at(dir + i * 12 + 8)).flatten()).map(|v| v as usize);
    // Fluxo de exceção: linha de execução, alinhamento e o registro, com código em +0 e endereço em +16.
    let ex = stream(6)?;
    let (code, addr) = (u32at(ex + 8)?, u64at(ex + 24)?);
    let list = stream(4)?;
    let place = (0..u32at(list)? as usize).find_map(|i| {
        let m = list + 4 + i * 108;
        let (base, size) = (u64at(m)?, u32at(m + 8)? as u64);
        if addr < base || addr >= base + size {
            return None;
        }
        let name = u32at(m + 20)? as usize;
        let bytes = d.get(name + 4..name + 4 + u32at(name)? as usize)?;
        let wide: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let full = String::from_utf16_lossy(&wide);
        Some(format!("{}+{:#x}", full.rsplit(['\\', '/']).next().unwrap_or(""), addr - base))
    });
    Some(format!("{days} day(s) ago, code {code:#x} at {}", place.unwrap_or_else(|| format!("{addr:#x}"))))
}

/// Há um relatório novo da última execução? Guarda com outro nome para avisar uma vez só.
pub fn pending() -> bool {
    let p = dump_path();
    p.exists() && std::fs::rename(&p, p.with_file_name("falha_anterior.dmp")).is_ok()
}
