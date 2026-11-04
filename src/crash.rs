//! Se o Prisma cair, guarda um relatório local que mostra onde foi. Nada é enviado.

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
            let _ = writeln!(f, "{} erro interno: {p}", crate::sys::tools::now_stamp());
        }
    }));
}

/// Há um relatório novo da última execução? Guarda com outro nome para avisar uma vez só.
pub fn pending() -> bool {
    let p = dump_path();
    p.exists() && std::fs::rename(&p, p.with_file_name("falha_anterior.dmp")).is_ok()
}
