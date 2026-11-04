//! Ícones de programas, os mesmos que o Windows mostra no Explorador.

use windows::core::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const SIZE: i32 = 32;

/// Devolve o ícone como BGRA pré-multiplicado, em 32 por 32.
pub fn load(path: &str) -> Option<(u32, u32, Vec<u8>)> {
    unsafe {
        let mut info = SHFILEINFOW::default();
        let wide = HSTRING::from(path);
        let r = SHGetFileInfoW(
            &wide,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );
        if r == 0 || info.hIcon.is_invalid() {
            return None;
        }
        let out = rasterize(info.hIcon);
        let _ = DestroyIcon(info.hIcon);
        out
    }
}

unsafe fn rasterize(icon: HICON) -> Option<(u32, u32, Vec<u8>)> {
    let bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: SIZE,
            biHeight: -SIZE,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let dc = CreateCompatibleDC(None);
    let mut bits = std::ptr::null_mut();
    let dib = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
    let old = SelectObject(dc, dib.into());
    let _ = DrawIconEx(dc, 0, 0, icon, SIZE, SIZE, 0, None, DI_NORMAL);
    let n = (SIZE * SIZE * 4) as usize;
    let mut px = std::slice::from_raw_parts(bits as *const u8, n).to_vec();
    SelectObject(dc, old);
    let _ = DeleteObject(dib.into());
    let _ = DeleteDC(dc);
    // Ícones antigos não têm alfa: tudo zero significa opaco.
    if px.chunks_exact(4).all(|p| p[3] == 0) {
        for p in px.chunks_exact_mut(4) {
            if p[0] != 0 || p[1] != 0 || p[2] != 0 {
                p[3] = 255;
            }
        }
    }
    Some((SIZE as u32, SIZE as u32, px))
}
