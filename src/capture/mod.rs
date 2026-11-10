//! Captura com a API do Windows. Foto: um quadro levado para a CPU.

#[cfg(debug_assertions)]
pub mod decoder;
pub mod encoder;
pub mod engine;
pub mod hdr;
#[cfg(debug_assertions)]
pub mod live;
#[cfg(debug_assertions)]
pub mod live_enc;
#[cfg(all(debug_assertions, feature = "stream"))]
pub mod nvenc;
pub mod mux;
pub mod shot;
pub mod sound;

use windows::core::*;
use windows::Graphics::Capture::*;
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Devices::Display::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::WinRT::Direct3D11::*;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::*;

pub struct D3d {
    pub dev: ID3D11Device,
    pub ctx: ID3D11DeviceContext,
    pub rt: IDirect3DDevice,
}

pub fn d3d() -> Result<D3d> {
    unsafe {
        let mut dev = None;
        let mut ctx = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut dev),
            None,
            Some(&mut ctx),
        )?;
        let dev: ID3D11Device = dev.unwrap();
        let dxgi: IDXGIDevice = dev.cast()?;
        let insp = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?;
        Ok(D3d { dev, ctx: ctx.unwrap(), rt: insp.cast()? })
    }
}

pub fn monitor_item(mon: HMONITOR) -> Result<GraphicsCaptureItem> {
    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    unsafe { interop.CreateForMonitor(mon) }
}

pub fn window_item(hwnd: HWND) -> Result<GraphicsCaptureItem> {
    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    unsafe { interop.CreateForWindow(hwnd) }
}

/// Informações de cor do monitor: se está em HDR e o branco do SDR, em nits.
#[derive(Clone, Copy, Debug)]
pub struct MonitorColor {
    pub hdr: bool,
    pub white_nits: f32,
}

pub fn monitor_color(mon: HMONITOR) -> MonitorColor {
    let mut out = MonitorColor { hdr: false, white_nits: 80.0 };
    unsafe {
        let Ok(f) = CreateDXGIFactory1::<IDXGIFactory1>() else { return out };
        let mut a = 0;
        while let Ok(ad) = f.EnumAdapters1(a) {
            let mut i = 0;
            while let Ok(o) = ad.EnumOutputs(i) {
                if let Ok(o6) = o.cast::<IDXGIOutput6>() {
                    if let Ok(d) = o6.GetDesc1() {
                        if d.Monitor == mon {
                            out.hdr = d.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
                            out.white_nits = sdr_white(&d.DeviceName).unwrap_or(80.0);
                            return out;
                        }
                    }
                }
                i += 1;
            }
            a += 1;
        }
    }
    out
}

fn sdr_white(device: &[u16; 32]) -> Option<f32> {
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm).ok().ok()?;
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None)
            .ok()
            .ok()?;
        for p in &paths[..np as usize] {
            let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            src.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            src.header.size = std::mem::size_of_val(&src) as u32;
            src.header.adapterId = p.sourceInfo.adapterId;
            src.header.id = p.sourceInfo.id;
            if DisplayConfigGetDeviceInfo(&mut src.header) != 0 || src.viewGdiDeviceName != *device {
                continue;
            }
            let mut w = DISPLAYCONFIG_SDR_WHITE_LEVEL::default();
            w.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL;
            w.header.size = std::mem::size_of_val(&w) as u32;
            w.header.adapterId = p.targetInfo.adapterId;
            w.header.id = p.targetInfo.id;
            if DisplayConfigGetDeviceInfo(&mut w.header) == 0 {
                return Some(w.SDRWhiteLevel as f32 / 1000.0 * 80.0);
            }
        }
        None
    }
}

/// Quadro capturado. `bgra` é sempre SDR; `half` guarda o original em HDR.
pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
    pub half: Option<Vec<u16>>,
}

fn half_table() -> Vec<f32> {
    (0..=65535u32)
        .map(|v| {
            let s = if v & 0x8000 != 0 { -1.0f32 } else { 1.0 };
            let e = ((v >> 10) & 31) as i32;
            let m = (v & 1023) as f32;
            s * match e {
                0 => m / 16777216.0,
                31 => 0.0,
                _ => (1.0 + m / 1024.0) * 2f32.powi(e - 15),
            }
        })
        .collect()
}

/// Curva de HDR para SDR: mantém igual o que é SDR e dobra só o que passa do branco.
#[inline]
pub fn rolloff(l: f32) -> f32 {
    const KNEE: f32 = 0.8;
    if l <= KNEE {
        l
    } else {
        KNEE + (1.0 - KNEE) * (1.0 - (-(l - KNEE) / (1.0 - KNEE)).exp())
    }
}

#[inline]
fn srgb(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let c = if v <= 0.0031308 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (c * 255.0 + 0.5) as u8
}

fn tonemap(half: &[u16], w: usize, h: usize, white_nits: f32) -> Vec<u8> {
    let table = half_table();
    let scale = 80.0 / white_nits.max(80.0);
    let mut out = vec![0u8; w * h * 4];
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let rows = (h / threads).max(1);
    std::thread::scope(|s| {
        for (src, dst) in half.chunks(rows * w * 4).zip(out.chunks_mut(rows * w * 4)) {
            let table = &table;
            s.spawn(move || {
                for (p, q) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                    let r = table[p[0] as usize].max(0.0) * scale;
                    let g = table[p[1] as usize].max(0.0) * scale;
                    let b = table[p[2] as usize].max(0.0) * scale;
                    let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                    let k = if l > 0.0 { rolloff(l) / l } else { 1.0 };
                    q[0] = srgb(b * k);
                    q[1] = srgb(g * k);
                    q[2] = srgb(r * k);
                    q[3] = 255;
                }
            });
        }
    });
    out
}

/// Monitor em que está o cursor.
pub fn cursor_monitor() -> HMONITOR {
    unsafe {
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        MonitorFromPoint(p, MONITOR_DEFAULTTOPRIMARY)
    }
}

/// Tira um quadro do monitor.
pub fn grab(mon: HMONITOR) -> Result<Frame> {
    let color = monitor_color(mon);
    let d = d3d()?;
    let item = monitor_item(mon)?;
    let size = item.Size()?;
    let fmt = if color.hdr { DirectXPixelFormat::R16G16B16A16Float } else { DirectXPixelFormat::B8G8R8A8UIntNormalized };
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&d.rt, fmt, 1, size)?;
    let session = pool.CreateCaptureSession(&item)?;
    let _ = session.SetIsCursorCaptureEnabled(false);
    let _ = session.SetIsBorderRequired(false);
    session.StartCapture()?;
    let mut frame = None;
    for _ in 0..200 {
        if let Ok(f) = pool.TryGetNextFrame() {
            frame = Some(f);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let frame = frame.ok_or_else(|| Error::new(E_FAIL, "sem quadro"))?;
    let (w, h) = (size.Width as usize, size.Height as usize);
    let surf = frame.Surface()?;
    let access: IDirect3DDxgiInterfaceAccess = surf.cast()?;
    let out = unsafe {
        let tex: ID3D11Texture2D = access.GetInterface()?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        tex.GetDesc(&mut desc);
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut st = None;
        d.dev.CreateTexture2D(&desc, None, Some(&mut st))?;
        let st = st.unwrap();
        d.ctx.CopyResource(&st, &tex);
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        d.ctx.Map(&st, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
        let px = if color.hdr { 8 } else { 4 };
        let mut raw = vec![0u8; w * h * px];
        for y in 0..h {
            let src = (m.pData as *const u8).add(y * m.RowPitch as usize);
            std::ptr::copy_nonoverlapping(src, raw.as_mut_ptr().add(y * w * px), w * px);
        }
        d.ctx.Unmap(&st, 0);
        if color.hdr {
            let half: Vec<u16> = raw.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
            let bgra = tonemap(&half, w, h, color.white_nits);
            Frame { w: w as u32, h: h as u32, bgra, half: Some(half) }
        } else {
            Frame { w: w as u32, h: h as u32, bgra: raw, half: None }
        }
    };
    let _ = frame.Close();
    let _ = session.Close();
    let _ = pool.Close();
    Ok(out)
}

fn stamp() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}-{:02}-{:02}_{:02}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

pub fn new_path(dir: &str, ext: &str) -> String {
    let _ = std::fs::create_dir_all(dir);
    format!("{dir}\\Prisma_{}.{ext}", stamp())
}

/// Salva BGRA como PNG pelo WIC, que já vem no Windows.
pub fn save_png(path: &str, w: u32, h: u32, bgra: &[u8]) -> Result<()> {
    encode(path, &GUID_ContainerFormatPng, w, h, GUID_WICPixelFormat32bppBGRA, bgra, w * 4)
}

/// Salva o quadro HDR original em JPEG XR.
pub fn save_jxr(path: &str, w: u32, h: u32, half: &[u16]) -> Result<()> {
    let bytes = unsafe { std::slice::from_raw_parts(half.as_ptr() as *const u8, half.len() * 2) };
    encode(path, &GUID_ContainerFormatWmp, w, h, GUID_WICPixelFormat64bppRGBAHalf, bytes, w * 8)
}

fn encode(path: &str, container: &GUID, w: u32, h: u32, fmt: GUID, data: &[u8], stride: u32) -> Result<()> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream = wic.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(path), GENERIC_WRITE.0)?;
        let enc = wic.CreateEncoder(container, std::ptr::null())?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        enc.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.unwrap();
        frame.Initialize(None)?;
        frame.SetSize(w, h)?;
        let mut f = fmt;
        frame.SetPixelFormat(&mut f)?;
        frame.WritePixels(h, stride, data)?;
        frame.Commit()?;
        enc.Commit()
    }
}

/// Copia BGRA para a área de transferência como DIB de 32 bits.
pub fn copy_to_clipboard(owner: HWND, w: u32, h: u32, bgra: &[u8]) -> bool {
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    const CF_DIB: u32 = 8;
    unsafe {
        let head = std::mem::size_of::<BITMAPINFOHEADER>();
        let size = head + bgra.len();
        let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, size) else { return false };
        let p = GlobalLock(mem) as *mut u8;
        if p.is_null() {
            return false;
        }
        let bi = BITMAPINFOHEADER {
            biSize: head as u32,
            biWidth: w as i32,
            biHeight: h as i32,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        std::ptr::copy_nonoverlapping(&bi as *const _ as *const u8, p, head);
        let row = w as usize * 4;
        for y in 0..h as usize {
            let s = bgra.as_ptr().add((h as usize - 1 - y) * row);
            std::ptr::copy_nonoverlapping(s, p.add(head + y * row), row);
        }
        let _ = GlobalUnlock(mem);
        if OpenClipboard(Some(owner)).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        let ok = SetClipboardData(CF_DIB, Some(HANDLE(mem.0))).is_ok();
        let _ = CloseClipboard();
        ok
    }
}

/// Copia um texto para a área de transferência.
pub fn copy_text(owner: HWND, text: &str) -> bool {
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
        let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) else { return false };
        let p = GlobalLock(mem) as *mut u16;
        if p.is_null() {
            return false;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
        let _ = GlobalUnlock(mem);
        if OpenClipboard(Some(owner)).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        let ok = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(mem.0))).is_ok();
        let _ = CloseClipboard();
        ok
    }
}

/// Texto copiado, numa linha só.
pub fn clipboard_text(owner: HWND) -> Option<String> {
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        OpenClipboard(Some(owner)).ok()?;
        let out = (|| {
            let h = GetClipboardData(CF_UNICODETEXT).ok()?;
            let p = GlobalLock(HGLOBAL(h.0)) as *const u16;
            if p.is_null() {
                return None;
            }
            let mut n = 0;
            while *p.add(n) != 0 && n < 4096 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            let _ = GlobalUnlock(HGLOBAL(h.0));
            Some(s.split(['\r', '\n']).next().unwrap_or("").to_string())
        })();
        let _ = CloseClipboard();
        out
    }
}

/// Lê a imagem copiada, em BGRA de cima para baixo.
pub fn clipboard_image() -> Option<(u32, u32, Vec<u8>)> {
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    const CF_DIB: u32 = 8;
    unsafe {
        OpenClipboard(None).ok()?;
        let read = || {
            let mem = HGLOBAL(GetClipboardData(CF_DIB).ok()?.0);
            let p = GlobalLock(mem) as *const u8;
            if p.is_null() {
                return None;
            }
            let bi = &*(p as *const BITMAPINFOHEADER);
            let (w, h) = (bi.biWidth as usize, bi.biHeight.unsigned_abs() as usize);
            let bytes = bi.biBitCount as usize / 8;
            let bitfields = bi.biCompression == BI_BITFIELDS.0 && bi.biSize == 40;
            let fine = w > 0 && h > 0 && matches!(bytes, 3 | 4) && (bi.biCompression == BI_RGB.0 || bi.biCompression == BI_BITFIELDS.0);
            let mut out = None;
            if fine {
                let start = p.add(bi.biSize as usize + if bitfields { 12 } else { 0 });
                let row = (w * bytes + 3) & !3;
                let mut px = vec![255u8; w * h * 4];
                for y in 0..h {
                    let from = if bi.biHeight > 0 { h - 1 - y } else { y };
                    let line = start.add(from * row);
                    for x in 0..w {
                        // O alfa do Windows costuma vir zerado: a foto é sempre opaca.
                        std::ptr::copy_nonoverlapping(line.add(x * bytes), px.as_mut_ptr().add((y * w + x) * 4), 3);
                    }
                }
                out = Some((w as u32, h as u32, px));
            }
            let _ = GlobalUnlock(mem);
            out
        };
        let image = read();
        let _ = CloseClipboard();
        image
    }
}

/// Abre uma imagem do disco em BGRA.
pub fn load_image(path: &str) -> Option<(u32, u32, Vec<u8>)> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let dec = wic.CreateDecoderFromFilename(&HSTRING::from(path), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).ok()?;
        let frame = dec.GetFrame(0).ok()?;
        let conv = wic.CreateFormatConverter().ok()?;
        conv.Initialize(&frame, &GUID_WICPixelFormat32bppBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let (mut w, mut h) = (0, 0);
        conv.GetSize(&mut w, &mut h).ok()?;
        let mut px = vec![0u8; w as usize * h as usize * 4];
        conv.CopyPixels(std::ptr::null(), w * 4, &mut px).ok()?;
        Some((w, h, px))
    }
}

/// Janela do Windows para escolher pasta.
pub fn choose_folder(owner: HWND) -> Option<String> {
    use windows::Win32::UI::Shell::*;
    unsafe {
        let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let o = d.GetOptions().ok()?;
        d.SetOptions(o | FOS_PICKFOLDERS).ok()?;
        d.Show(Some(owner)).ok()?;
        let item = d.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

/// Janela do Windows para escolher um programa.
pub fn choose_file(owner: HWND, kind: &str, spec: PCWSTR) -> Option<String> {
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::*;
    unsafe {
        let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let name = HSTRING::from(crate::i18n::tr(kind).as_ref());
        d.SetFileTypes(&[COMDLG_FILTERSPEC { pszName: PCWSTR(name.as_ptr()), pszSpec: spec }]).ok()?;
        d.Show(Some(owner)).ok()?;
        let p = d.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

pub fn save_file(owner: HWND, kind: &str, spec: PCWSTR, file: &str) -> Option<String> {
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::*;
    unsafe {
        let d: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let name = HSTRING::from(crate::i18n::tr(kind).as_ref());
        d.SetFileTypes(&[COMDLG_FILTERSPEC { pszName: PCWSTR(name.as_ptr()), pszSpec: spec }]).ok()?;
        d.SetDefaultExtension(w!("pow")).ok()?;
        d.SetFileName(&HSTRING::from(file)).ok()?;
        d.Show(Some(owner)).ok()?;
        let p = d.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

pub fn choose_exe(owner: HWND) -> Option<String> {
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::*;
    unsafe {
        let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let name = HSTRING::from(crate::i18n::tr("Programas").as_ref());
        let f = [COMDLG_FILTERSPEC { pszName: PCWSTR(name.as_ptr()), pszSpec: w!("*.exe") }];
        d.SetFileTypes(&f).ok()?;
        d.Show(Some(owner)).ok()?;
        let item = d.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

/// Deixa uma janela nossa de fora de fotos e vídeos.
pub fn hide_from_capture(h: HWND) {
    unsafe {
        let _ = SetWindowDisplayAffinity(h, WDA_EXCLUDEFROMCAPTURE);
    }
}
