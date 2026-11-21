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

const MARK: &str = "cursor-escondido.txt";

/// Na pasta em que este processo escreve: a do serviço só para o ajudante SYSTEM (para a conta ela
/// é só leitura, e o Mouse e teclado daqui, aberto pela conta, não deixaria a marca).
fn mark() -> std::path::PathBuf {
    let dir = if super::SERVICE.load(Ordering::Relaxed) { super::service_dir() } else { super::user_dir() };
    dir.join(MARK)
}

/// Esconde o cursor deste PC. Sem efeito se já está escondido.
pub fn hide() {
    if HIDDEN.swap(true, Ordering::SeqCst) {
        return;
    }
    let mark = mark();
    if let Some(d) = mark.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&mark, std::process::id().to_string());
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
    restore_at(&mark());
}

fn restore_at(mark: &std::path::Path) {
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW(SPI_SETCURSORS, 0, None, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
    let _ = std::fs::remove_file(mark);
}

/// Se o processo que escondeu o cursor não roda mais, devolve o cursor. O Prisma olha a marca da
/// conta; a do ajudante (pasta só leitura para a conta) o ajudante devolve quando abre de novo.
pub fn restore_orphan() {
    restore_orphan_at(&mark());
}

fn restore_orphan_at(mark: &std::path::Path) {
    let Ok(text) = std::fs::read_to_string(mark) else { return };
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
        restore_at(mark);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_fica_onde_a_conta_escreve() {
        // Fora do ajudante, mesmo com o serviço instalado: a pasta da conta, que ela pode gravar.
        assert!(!super::super::SERVICE.load(Ordering::Relaxed));
        assert_eq!(mark(), super::super::user_dir().join(MARK));
    }

    #[test]
    fn marca_de_processo_vivo_fica_e_de_morto_devolve() {
        let dir = std::env::temp_dir().join(format!("prisma-cursor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mark = dir.join(MARK);
        std::fs::write(&mark, std::process::id().to_string()).unwrap();
        restore_orphan_at(&mark);
        assert!(mark.exists(), "o processo que escondeu ainda roda");
        // Um número que não é de processo nenhum (os números do Windows são múltiplos de 4).
        std::fs::write(&mark, "4294967293").unwrap();
        restore_orphan_at(&mark);
        assert!(!mark.exists(), "processo morto: o cursor volta e a marca sai");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
