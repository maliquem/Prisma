//! Ajudante de 32 bits do modo Injeção. O `SetWindowsHookEx` só põe no jogo uma DLL da mesma arquitetura de
//! quem chama: o motor (64 bits) roda este para os jogos de 32 bits. Uso: `prisma_hook32 <dll> <export> <tid>`.
//! Responde `ok` quando o gancho está posto e o solta quando o motor fecha a entrada (ou some).
#![windows_subsystem = "windows"]

use std::io::{Read, Write};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::UI::WindowsAndMessaging::*;

fn main() {
    std::process::exit(if run().is_some() { 0 } else { 1 })
}

fn run() -> Option<()> {
    let a: Vec<String> = std::env::args().collect();
    let (dll, name, tid) = (a.get(1)?, std::ffi::CString::new(a.get(2)?.as_str()).ok()?, a.get(3)?.parse::<u32>().ok()?);
    unsafe {
        let module = LoadLibraryExW(&HSTRING::from(dll.as_str()), None, DONT_RESOLVE_DLL_REFERENCES).ok()?;
        let proc = GetProcAddress(module, PCSTR(name.as_ptr() as *const u8))?;
        let hook = SetWindowsHookExW(WH_GETMESSAGE, Some(std::mem::transmute(proc)), Some(module.into()), tid).ok()?;
        let _ = PostThreadMessageW(tid, WM_NULL, WPARAM(0), LPARAM(0));
        let mut out = std::io::stdout();
        let _ = out.write_all(b"ok\n").and_then(|_| out.flush());
        // Fim da entrada: o motor viu a DLL presa (ou desistiu).
        let _ = std::io::stdin().read(&mut [0u8; 1]);
        let _ = UnhookWindowsHookEx(hook);
    }
    Some(())
}
