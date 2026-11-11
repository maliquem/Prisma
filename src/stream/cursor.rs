//! Esconde o cursor do Windows enquanto o mouse está na tela de outro PC (Mouse e teclado).
//!
//! O Windows não tem um "esconder o cursor" para o sistema todo: cada cursor padrão vira um
//! cursor transparente, e o tema do Windows volta com `SPI_SETCURSORS`. Uma marca com o número
//! do processo fica na pasta deste usuário enquanto o cursor está escondido: se o processo cair
//! sem devolver, o Prisma vê a marca e devolve o cursor.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::UI::WindowsAndMessaging::*;

static HIDDEN: AtomicBool = AtomicBool::new(false);

/// Os cursores padrão do Windows (OCR_NORMAL, OCR_IBEAM, OCR_WAIT e os outros).
const CURSORS: [u32; 16] = [32512, 32513, 32514, 32515, 32516, 32642, 32643, 32644, 32645, 32646, 32648, 32649, 32650, 32651, 32671, 32672];

fn mark() -> std::path::PathBuf {
    super::user_dir().join("cursor-escondido.txt")
}

/// Esconde o cursor deste PC. Sem efeito se já está escondido.
pub fn hide() {
    if HIDDEN.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = std::fs::create_dir_all(super::user_dir());
    let _ = std::fs::write(mark(), std::process::id().to_string());
    // Máscara AND toda em 1 e XOR toda em 0: o pixel fica como está, ou seja, invisível.
    let and = [0xFFu8; 32 * 4];
    let xor = [0u8; 32 * 4];
    unsafe {
        let inst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).ok().map(|m| m.into());
        for id in CURSORS {
            // O Windows fica com o cursor passado: um novo para cada um.
            if let Ok(c) = CreateCursor(inst, 0, 0, 32, 32, and.as_ptr() as _, xor.as_ptr() as _) {
                let _ = SetSystemCursor(c, SYSTEM_CURSOR_ID(id));
            }
        }
    }
}

/// Devolve o cursor do tema do Windows. Sem efeito se não está escondido.
pub fn show() {
    if !HIDDEN.swap(false, Ordering::SeqCst) {
        return;
    }
    restore();
}

fn restore() {
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW(SPI_SETCURSORS, 0, None, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
    let _ = std::fs::remove_file(mark());
}

/// Para o Prisma: se o processo que escondeu o cursor não roda mais, devolve o cursor.
pub fn restore_orphan() {
    let Ok(text) = std::fs::read_to_string(mark()) else { return };
    let alive = text.trim().parse::<u32>().ok().is_some_and(|pid| unsafe {
        use windows::Win32::System::Threading::*;
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(h) => {
                let mut code = 0u32;
                let running = GetExitCodeProcess(h, &mut code).is_ok() && code == 259;
                let _ = windows::Win32::Foundation::CloseHandle(h);
                running
            }
            Err(_) => false,
        }
    });
    if !alive {
        restore();
    }
}
