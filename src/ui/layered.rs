//! Janela transparente sem GPU: o widget e os avisos desenham num bitmap e o
//! Windows compõe. Quase não custa nada enquanto nada muda.

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::gfx::{factory, rect, Canvas, Rect};

pub struct Layered {
    pub hwnd: HWND,
    mem: HDC,
    dib: HBITMAP,
    old: HGDIOBJ,
    size: (i32, i32),
    canvas: Option<Canvas>,
    dc_rt: Option<ID2D1DCRenderTarget>,
    pub scale: f32,
    #[cfg(debug_assertions)]
    bits: *mut u8,
}

impl Layered {
    pub fn new(hwnd: HWND) -> Self {
        let mem = unsafe { CreateCompatibleDC(None) };
        Self {
            hwnd,
            mem,
            dib: HBITMAP::default(),
            old: HGDIOBJ::default(),
            size: (0, 0),
            canvas: None,
            dc_rt: None,
            scale: 1.0,
            #[cfg(debug_assertions)]
            bits: std::ptr::null_mut(),
        }
    }

    fn ensure(&mut self, w: i32, h: i32) {
        if self.size == (w, h) && self.dc_rt.is_some() {
            return;
        }
        unsafe {
            if !self.dib.is_invalid() {
                SelectObject(self.mem, self.old);
                let _ = DeleteObject(self.dib.into());
            }
            let bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            self.dib = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0)
                .expect("bitmap do widget");
            #[cfg(debug_assertions)]
            {
                self.bits = bits as *mut u8;
            }
            self.old = SelectObject(self.mem, self.dib.into());
            self.size = (w, h);
            let dpi = 96.0 * self.scale;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: dpi,
                dpiY: dpi,
                ..Default::default()
            };
            let rt = factory().CreateDCRenderTarget(&props).expect("alvo do widget");
            let bounds = RECT { left: 0, top: 0, right: w, bottom: h };
            let _ = rt.BindDC(self.mem, &bounds);
            let base: ID2D1RenderTarget = rt.cast().unwrap();
            self.canvas = Some(Canvas::new(base));
            self.dc_rt = Some(rt);
        }
    }

    /// Desenha `w`×`h` DIPs e publica no canto `x`,`y` da tela, em pixels.
    pub fn paint(&mut self, x: i32, y: i32, w: f32, h: f32, draw: impl FnOnce(&Canvas, Rect)) {
        unsafe {
            self.scale = GetDpiForWindow(self.hwnd) as f32 / 96.0;
        }
        let (pw, ph) = ((w * self.scale).ceil() as i32, (h * self.scale).ceil() as i32);
        self.ensure(pw, ph);
        let c = self.canvas.as_ref().unwrap();
        unsafe {
            c.rt.BeginDraw();
            c.rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            draw(c, rect(0.0, 0.0, w, h));
            if c.rt.EndDraw(None, None).is_err() {
                self.dc_rt = None;
                self.canvas = None;
                return;
            }
            #[cfg(debug_assertions)]
            if let Ok(dir) = std::env::var("PRISMA_LAYER_DUMP") {
                let px = std::slice::from_raw_parts(self.bits, (pw * ph * 4) as usize);
                let _ = crate::capture::save_png(&format!("{dir}\\camada_{}.png", pw), pw as u32, ph as u32, px);
            }
            let pos = POINT { x, y };
            let size = SIZE { cx: pw, cy: ph };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&pos),
                Some(&size),
                Some(self.mem),
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
        }
    }
}

impl Drop for Layered {
    fn drop(&mut self) {
        unsafe {
            self.canvas = None;
            self.dc_rt = None;
            if !self.dib.is_invalid() {
                SelectObject(self.mem, self.old);
                let _ = DeleteObject(self.dib.into());
            }
            let _ = DeleteDC(self.mem);
        }
    }
}

/// Cria uma janela flutuante que não rouba o foco do jogo.
pub fn create_overlay(class: PCWSTR, click_through: bool) -> HWND {
    unsafe {
        let mut ex = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        if click_through {
            ex |= WS_EX_TRANSPARENT;
        }
        CreateWindowExW(
            ex,
            class,
            w!("Prisma"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            None,
            None,
        )
        .expect("janela flutuante")
    }
}
