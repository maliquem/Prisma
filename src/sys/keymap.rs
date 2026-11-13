//! Gancho do teclado e do mouse para as ligações de teclado do perfil em uso (`remap::key_event`).
//! Só existe enquanto o perfil em uso tem ligação de teclado ou mouse e o mapeamento está ligado; o
//! de mouse, só com ligação de botão do mouse (todo movimento do mouse passaria por ele).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;

/// O que o perfil em uso precisa: 1 teclado, 2 mouse.
static NEED: AtomicU8 = AtomicU8::new(0);
static THREAD: AtomicU32 = AtomicU32::new(0);
static STARTING: AtomicBool = AtomicBool::new(false);

/// Chamado quando o perfil, o mapeamento ou a parada mudam.
pub fn sync(need: u8) {
    NEED.store(need, Ordering::SeqCst);
    if need != 0 && THREAD.load(Ordering::SeqCst) == 0 && !STARTING.swap(true, Ordering::SeqCst) {
        std::thread::spawn(run);
    }
}

unsafe extern "system" fn key_hook(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let k = &*(l.0 as *const KBDLLHOOKSTRUCT);
        // O que o Prisma (ou outro programa) manda passa direto.
        if k.flags.0 & LLKHF_INJECTED.0 == 0 && (1..=254).contains(&k.vkCode) {
            let down = matches!(w.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            if super::remap::key_event(k.vkCode as u16, down) {
                return LRESULT(1);
            }
        }
    }
    CallNextHookEx(None, code, w, l)
}

unsafe extern "system" fn mouse_hook(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let m = &*(l.0 as *const MSLLHOOKSTRUCT);
        // Só o botão do meio e os laterais; os cliques esquerdo e direito nunca mudam.
        let side = if (m.mouseData >> 16) as u16 == 1 { 5 } else { 6 };
        let ev = match w.0 as u32 {
            WM_MBUTTONDOWN => Some((4, true)),
            WM_MBUTTONUP => Some((4, false)),
            WM_XBUTTONDOWN => Some((side, true)),
            WM_XBUTTONUP => Some((side, false)),
            _ => None,
        };
        if let Some((vk, down)) = ev.filter(|_| m.flags & LLMHF_INJECTED == 0) {
            if super::remap::key_event(vk, down) {
                return LRESULT(1);
            }
        }
    }
    CallNextHookEx(None, code, w, l)
}

/// Thread do gancho: liga e desliga cada gancho conforme `NEED` e, a cada 30 ms, deixa o motor ver
/// o tempo passar (segurar, toque duplo). Sai quando nada mais precisa dele.
fn run() {
    use windows::Win32::System::Threading::GetCurrentThreadId;
    unsafe {
        let mut msg = MSG::default();
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
        THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
        STARTING.store(false, Ordering::SeqCst);
        let timer = SetTimer(None, 0, 30, None);
        let (mut kb, mut ms) = (None, None);
        loop {
            let need = NEED.load(Ordering::SeqCst);
            match (need & 1 != 0, kb) {
                (true, None) => kb = SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_hook), None, 0).ok(),
                (false, Some(h)) => {
                    let _ = UnhookWindowsHookEx(h);
                    kb = None;
                }
                _ => {}
            }
            match (need & 2 != 0, ms) {
                (true, None) => ms = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0).ok(),
                (false, Some(h)) => {
                    let _ = UnhookWindowsHookEx(h);
                    ms = None;
                }
                _ => {}
            }
            if need == 0 {
                THREAD.store(0, Ordering::SeqCst);
                // Pedido de novo enquanto saía: fica.
                if NEED.load(Ordering::SeqCst) == 0 || STARTING.swap(true, Ordering::SeqCst) {
                    break;
                }
                THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
                STARTING.store(false, Ordering::SeqCst);
                continue;
            }
            if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                break;
            }
            if msg.message == WM_TIMER {
                super::remap::key_tick();
            }
        }
        for h in [kb, ms].into_iter().flatten() {
            let _ = UnhookWindowsHookEx(h);
        }
        let _ = KillTimer(None, timer);
    }
}
