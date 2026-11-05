//! Caixa de texto do próprio Windows: seleção, desfazer e colar prontos, para quando escrever é o trabalho.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::OnceLock;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::gfx::Rect;

const EM_SETSEL: u32 = 0xB1;
const EM_LINESCROLL: u32 = 0xB6;
const EM_SETLIMITTEXT: u32 = 0xC5;
const EM_SETMARGINS: u32 = 0xD3;
const PAPER: COLORREF = COLORREF(0x00221d1b);
const INK: COLORREF = COLORREF(0x00eeeae8);

static OLD_PROC: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // Ctrl+A chega como o caractere 1, e esta versão do controle não seleciona sozinha.
    if m == WM_CHAR && w.0 == 1 {
        SendMessageW(h, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        return LRESULT(0);
    }
    // Sem barra de rolagem clara no tema escuro, a roda precisa mover o texto por conta própria.
    if m == WM_MOUSEWHEEL {
        let lines = -((w.0 >> 16) as i16 as isize) / 120 * 3;
        SendMessageW(h, EM_LINESCROLL, Some(WPARAM(0)), Some(LPARAM(lines)));
        return LRESULT(0);
    }
    CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(OLD_PROC.load(Ordering::Relaxed)), h, m, w, l)
}

/// Cor da caixa, respondida à mensagem WM_CTLCOLOREDIT da janela mãe.
pub fn paint_colors(hdc: HDC) -> LRESULT {
    static BRUSH: OnceLock<isize> = OnceLock::new();
    unsafe {
        SetTextColor(hdc, INK);
        SetBkColor(hdc, PAPER);
        LRESULT(*BRUSH.get_or_init(|| CreateSolidBrush(PAPER).0 as isize))
    }
}

pub struct TextBox {
    hwnd: HWND,
    font: HFONT,
    scale: f32,
    at: Rect,
    shown: bool,
}

impl TextBox {
    pub fn new(parent: HWND) -> Option<Self> {
        unsafe {
            let inst = GetModuleHandleW(None).ok()?;
            let style = WS_CHILD.0 | WS_CLIPSIBLINGS.0 | ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32 | ES_WANTRETURN as u32;
            let hwnd = CreateWindowExW(WS_EX_NOPARENTNOTIFY, w!("EDIT"), None, WINDOW_STYLE(style), 0, 0, 10, 10, Some(parent), None, Some(inst.into()), None).ok()?;
            SendMessageW(hwnd, EM_SETLIMITTEXT, Some(WPARAM(crate::report::TEXT_MAX)), Some(LPARAM(0)));
            let old = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, proc as *const () as isize);
            OLD_PROC.store(old, Ordering::Relaxed);
            Some(Self { hwnd, font: HFONT::default(), scale: 0.0, at: Rect::default(), shown: false })
        }
    }

    /// Coloca a caixa sobre o espaço desenhado. Nada muda se ela já está ali.
    pub fn place(&mut self, r: Rect, scale: f32) {
        unsafe {
            if self.scale != scale {
                self.scale = scale;
                let old = self.font;
                self.font = CreateFontW(-(14.0 * scale).round() as i32, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, 0, w!("Segoe UI Variable Text"));
                SendMessageW(self.hwnd, WM_SETFONT, Some(WPARAM(self.font.0 as usize)), Some(LPARAM(1)));
                if !old.is_invalid() {
                    let _ = DeleteObject(old.into());
                }
                let margin = (2.0 * scale) as usize;
                SendMessageW(self.hwnd, EM_SETMARGINS, Some(WPARAM(3)), Some(LPARAM((margin | margin << 16) as isize)));
                self.at = Rect::default();
            }
            if self.at != r || !self.shown {
                self.at = r;
                self.shown = true;
                let px = |v: f32| (v * scale).round() as i32;
                let _ = SetWindowPos(self.hwnd, Some(HWND_TOP), px(r.x), px(r.y), px(r.w), px(r.h), SWP_SHOWWINDOW | SWP_NOACTIVATE);
            }
        }
    }

    pub fn hide(&mut self) {
        if self.shown {
            self.shown = false;
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }

    pub fn focus(&self) {
        unsafe {
            let _ = SetFocus(Some(self.hwnd));
        }
    }

    pub fn text(&self) -> String {
        unsafe {
            let mut buf = vec![0u16; GetWindowTextLengthW(self.hwnd) as usize + 1];
            let n = GetWindowTextW(self.hwnd, &mut buf) as usize;
            String::from_utf16_lossy(&buf[..n])
        }
    }

    pub fn set_text(&self, text: &str) {
        unsafe {
            let _ = SetWindowTextW(self.hwnd, &HSTRING::from(text));
        }
    }
}

impl Drop for TextBox {
    fn drop(&mut self) {
        unsafe {
            if IsWindow(Some(self.hwnd)).as_bool() {
                let _ = DestroyWindow(self.hwnd);
            }
            if !self.font.is_invalid() {
                let _ = DeleteObject(self.font.into());
            }
        }
    }
}
