//! Fullscreenizer: tira as bordas e cobre o monitor da janela. O mesmo atalho desfaz.

use std::collections::HashMap;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy)]
struct Saved {
    style: i32,
    ex: i32,
    rect: RECT,
    maximized: bool,
}

#[derive(Default)]
pub struct Fullscreenizer {
    saved: HashMap<isize, Saved>,
}

const STYLE_OFF: u32 = WS_CAPTION.0 | WS_THICKFRAME.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0 | WS_SYSMENU.0;
const EX_OFF: u32 = WS_EX_DLGMODALFRAME.0 | WS_EX_WINDOWEDGE.0 | WS_EX_CLIENTEDGE.0 | WS_EX_STATICEDGE.0;

fn monitor_rect(hwnd: HWND) -> Option<RECT> {
    unsafe {
        let m = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        GetMonitorInfoW(m, &mut mi).as_bool().then_some(mi.rcMonitor)
    }
}

impl Fullscreenizer {
    pub fn is_on(&self, hwnd: HWND) -> bool {
        self.saved.contains_key(&(hwnd.0 as isize))
    }

    /// Alterna a janela. Devolve Some(true) ao ligar e Some(false) ao restaurar.
    pub fn toggle(&mut self, hwnd: HWND) -> Option<bool> {
        if hwnd.is_invalid() {
            return None;
        }
        unsafe {
            let shell = GetShellWindow();
            let desktop = GetDesktopWindow();
            if hwnd == shell || hwnd == desktop {
                return None;
            }
        }
        if self.is_on(hwnd) {
            self.restore(hwnd);
            Some(false)
        } else {
            self.apply(hwnd).then_some(true)
        }
    }

    pub fn apply(&mut self, hwnd: HWND) -> bool {
        unsafe {
            let Some(mon) = monitor_rect(hwnd) else { return false };
            let style = GetWindowLongW(hwnd, GWL_STYLE);
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE);
            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect).is_err() {
                return false;
            }
            let maximized = IsZoomed(hwnd).as_bool();
            if maximized {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = GetWindowRect(hwnd, &mut rect);
            }
            self.saved.insert(hwnd.0 as isize, Saved { style, ex, rect, maximized });
            SetWindowLongW(hwnd, GWL_STYLE, style & !(STYLE_OFF as i32));
            SetWindowLongW(hwnd, GWL_EXSTYLE, ex & !(EX_OFF as i32));
            SetWindowPos(
                hwnd,
                Some(HWND_TOP),
                mon.left,
                mon.top,
                mon.right - mon.left,
                mon.bottom - mon.top,
                SWP_FRAMECHANGED | SWP_SHOWWINDOW,
            )
            .is_ok()
        }
    }

    pub fn restore(&mut self, hwnd: HWND) {
        let Some(s) = self.saved.remove(&(hwnd.0 as isize)) else { return };
        unsafe {
            SetWindowLongW(hwnd, GWL_STYLE, s.style);
            SetWindowLongW(hwnd, GWL_EXSTYLE, s.ex);
            let _ = SetWindowPos(
                hwnd,
                None,
                s.rect.left,
                s.rect.top,
                s.rect.right - s.rect.left,
                s.rect.bottom - s.rect.top,
                SWP_FRAMECHANGED | SWP_NOZORDER | SWP_SHOWWINDOW,
            );
            if s.maximized {
                let _ = ShowWindow(hwnd, SW_MAXIMIZE);
            }
        }
    }

    /// Esquece janelas que já fecharam.
    pub fn sweep(&mut self) {
        self.saved.retain(|h, _| unsafe { IsWindow(Some(HWND(*h as *mut _))).as_bool() });
    }
}

/// True quando a janela cobre o monitor inteiro, com ou sem bordas.
pub fn covers_monitor(hwnd: HWND) -> bool {
    unsafe {
        let Some(mon) = monitor_rect(hwnd) else { return false };
        let mut r = RECT::default();
        if GetWindowRect(hwnd, &mut r).is_err() {
            return false;
        }
        r.left <= mon.left && r.top <= mon.top && r.right >= mon.right && r.bottom >= mon.bottom
    }
}
