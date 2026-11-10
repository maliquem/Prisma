//! Captura pela Desktop Duplication, a que segue a tela segura do Windows.
//!
//! O WGC, usado no resto do tempo, não enxerga o UAC, a tela de bloqueio nem a do PIN. A Desktop
//! Duplication enxerga, desde que o processo rode como SYSTEM (o serviço) e a linha que captura
//! esteja presa na área de trabalho que recebe a entrada agora. Quando o Windows troca de área
//! (abre o UAC, bloqueia), a duplicação perde o acesso: a linha muda de área e duplica de novo.
//!
//! A Desktop Duplication não desenha o cursor: ele vem à parte, com posição e formato, e é
//! desenhado aqui por cima da imagem.

use std::time::{Duration, Instant};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::Performance::QueryPerformanceFrequency;
use windows::Win32::System::StationsAndDesktops::*;

use crate::capture::hdr::{compile, ToneMap, VS};
use crate::capture::D3d;

/// O que saiu de uma espera.
pub(super) enum Got {
    Nothing,
    /// Imagem nova em `mid` (a tela, o cursor ou os dois), com a hora em 100 ns.
    Frame(i64),
    /// A tela mudou de tamanho ou de formato: a sessão precisa recomeçar.
    Changed,
}

pub(super) struct Dda {
    dev: ID3D11Device,
    ctx: ID3D11DeviceContext,
    output: IDXGIOutput1,
    hdr: bool,
    size: (u32, u32),
    dup: Option<IDXGIOutputDuplication>,
    desk: Option<HDESK>,
    /// A área de trabalho atual é a segura (UAC, bloqueio, PIN).
    pub secure: bool,
    pub desk_name: String,
    /// Cópia em ponto flutuante para a conversão de cor (tela em HDR).
    raw: Option<ID3D11Texture2D>,
    /// A última imagem da tela, sem o cursor, no formato de `mid`.
    base: ID3D11Texture2D,
    tone: Option<ToneMap>,
    cursor: Cursor,
    retry: Instant,
    /// Vezes que a duplicação perdeu o acesso (troca de área de trabalho, modo da tela).
    pub lost: u32,
    pub cursor_shapes: u32,
}

/// Saída de vídeo do monitor.
unsafe fn output_of(d: &D3d, mon: HMONITOR) -> Result<IDXGIOutput1> {
    let ad = d.dev.cast::<IDXGIDevice>()?.GetAdapter()?;
    let mut i = 0;
    loop {
        let o = ad.EnumOutputs(i)?;
        if o.GetDesc()?.Monitor == mon {
            return o.cast();
        }
        i += 1;
    }
}

fn qpc_100ns(c: i64) -> i64 {
    let mut f = 0i64;
    unsafe {
        let _ = QueryPerformanceFrequency(&mut f);
    }
    (c as i128 * 10_000_000 / f.max(1) as i128) as i64
}

impl Dda {
    /// `mid` recebe a imagem final; `hdr` diz se a tela está em HDR, `hdr10` se `mid` é PQ de 10 bits.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn new(d: &D3d, mon: HMONITOR, hdr: bool, white_nits: f32, mid: &ID3D11Texture2D, hdr10: bool, w: u32, h: u32) -> Result<Self> {
        let output = output_of(d, mon)?;
        let mut md = D3D11_TEXTURE2D_DESC::default();
        mid.GetDesc(&mut md);
        let tex = |fmt: DXGI_FORMAT, bind: D3D11_BIND_FLAG| -> Result<ID3D11Texture2D> {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: w,
                Height: h,
                MipLevels: 1,
                ArraySize: 1,
                Format: fmt,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: bind.0 as u32,
                ..Default::default()
            };
            let mut t = None;
            d.dev.CreateTexture2D(&desc, None, Some(&mut t))?;
            Ok(t.unwrap())
        };
        let base = tex(md.Format, D3D11_BIND_RENDER_TARGET)?;
        let (raw, tone) = if hdr {
            let raw = tex(DXGI_FORMAT_R16G16B16A16_FLOAT, D3D11_BIND_SHADER_RESOURCE)?;
            (Some(raw), Some(ToneMap::new(&d.dev, &base, hdr10, white_nits)?))
        } else {
            (None, None)
        };
        let cursor = Cursor::new(&d.dev, mid, hdr10, white_nits)?;
        let mut me = Dda {
            dev: d.dev.clone(),
            ctx: d.ctx.clone(),
            output,
            hdr,
            size: (w, h),
            dup: None,
            desk: None,
            secure: false,
            desk_name: String::new(),
            raw,
            base,
            tone,
            cursor,
            retry: Instant::now() - Duration::from_secs(1),
            lost: 0,
            cursor_shapes: 0,
        };
        me.attach();
        Ok(me)
    }

    /// Prende esta linha na área de trabalho que recebe a entrada agora. Sem SYSTEM, a segura recusa,
    /// e a linha fica na de sempre.
    unsafe fn attach(&mut self) {
        let Ok(h) = OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_ACCESS_FLAGS(GENERIC_ALL.0)) else { return };
        if SetThreadDesktop(h).is_err() {
            let _ = CloseDesktop(h);
            return;
        }
        let mut name = [0u16; 64];
        let mut need = 0u32;
        let _ = GetUserObjectInformationW(HANDLE(h.0), UOI_NAME, Some(name.as_mut_ptr() as *mut _), (name.len() * 2) as u32, Some(&mut need));
        let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        self.desk_name = String::from_utf16_lossy(&name[..len]);
        self.secure = !self.desk_name.eq_ignore_ascii_case("Default");
        if let Some(old) = self.desk.replace(h) {
            let _ = CloseDesktop(old);
        }
    }

    unsafe fn duplicate(&self) -> Result<IDXGIOutputDuplication> {
        let fmts = [if self.hdr { DXGI_FORMAT_R16G16B16A16_FLOAT } else { DXGI_FORMAT_B8G8R8A8_UNORM }];
        match self.output.cast::<IDXGIOutput5>() {
            Ok(o5) => o5.DuplicateOutput1(&self.dev, 0, &fmts),
            Err(_) => self.output.DuplicateOutput(&self.dev),
        }
    }

    /// Espera até 1 ms por novidade e, se houver, deixa a imagem com o cursor em `mid`.
    pub unsafe fn poll(&mut self, mid: &ID3D11Texture2D) -> Result<Got> {
        let Some(dup) = self.dup.clone() else {
            // Sem acesso (troca de área, tela segura sem SYSTEM): tenta de novo a cada 100 ms.
            if self.retry.elapsed() < Duration::from_millis(100) {
                std::thread::sleep(Duration::from_millis(1));
                return Ok(Got::Nothing);
            }
            self.retry = Instant::now();
            self.attach();
            let Ok(dup) = self.duplicate() else { return Ok(Got::Nothing) };
            let desc = dup.GetDesc();
            if (desc.ModeDesc.Width, desc.ModeDesc.Height) != self.size {
                return Ok(Got::Changed);
            }
            self.dup = Some(dup);
            return Ok(Got::Nothing);
        };
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut res = None;
        match dup.AcquireNextFrame(1, &mut info, &mut res) {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(Got::Nothing),
            Err(_) => {
                // Acesso perdido: a área de trabalho trocou, ou o modo da tela mudou.
                self.dup = None;
                self.lost += 1;
                self.retry = Instant::now() - Duration::from_secs(1);
                return Ok(Got::Nothing);
            }
        }
        let mut changed = false;
        let mut pts = crate::stream::now();
        if info.LastPresentTime != 0 {
            if let Some(r) = res.as_ref().and_then(|r| r.cast::<ID3D11Texture2D>().ok()) {
                match (&self.raw, &self.tone) {
                    (Some(raw), Some(tone)) => {
                        self.ctx.CopyResource(raw, &r);
                        tone.draw(&self.ctx, &self.dev, raw, self.size.0, self.size.1)?;
                    }
                    _ => self.ctx.CopyResource(&self.base, &r),
                }
                pts = qpc_100ns(info.LastPresentTime);
                changed = true;
            }
        }
        if info.LastMouseUpdateTime != 0 {
            let p = info.PointerPosition;
            if (p.Position, p.Visible.as_bool()) != (self.cursor.pos, self.cursor.visible) {
                self.cursor.pos = p.Position;
                self.cursor.visible = p.Visible.as_bool();
                changed = true;
            }
        }
        if info.PointerShapeBufferSize > 0 {
            let mut buf = vec![0u8; info.PointerShapeBufferSize as usize];
            let mut need = 0u32;
            let mut si = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
            if dup.GetFramePointerShape(buf.len() as u32, buf.as_mut_ptr() as *mut _, &mut need, &mut si).is_ok() {
                if let Some(img) = cursor_image(si.Type, si.Width, si.Height, si.Pitch, &buf) {
                    self.cursor.set(&self.dev, &img)?;
                    self.cursor_shapes += 1;
                    changed = true;
                }
            }
        }
        drop(res);
        let _ = dup.ReleaseFrame();
        if !changed {
            return Ok(Got::Nothing);
        }
        self.ctx.CopyResource(mid, &self.base);
        self.cursor.draw(&self.ctx, self.size)?;
        Ok(Got::Frame(pts))
    }
}

impl Drop for Dda {
    fn drop(&mut self) {
        self.dup = None;
        if let Some(h) = self.desk.take() {
            unsafe {
                let _ = CloseDesktop(h);
            }
        }
    }
}

/// O formato do ponteiro, em BGRA: a cor com transparência e, se houver, a máscara de inversão
/// (branco onde a cor da tela se inverte, como no cursor de texto).
#[derive(Debug, PartialEq)]
pub(super) struct CursorImage {
    pub w: u32,
    pub h: u32,
    pub color: Vec<u8>,
    pub invert: Option<Vec<u8>>,
}

/// Converte o formato que a Desktop Duplication entrega: 1 monocromático (máscaras E e OU-exclusivo,
/// metade de cima e de baixo), 2 colorido com alfa, 4 colorido com máscara no alfa.
pub(super) fn cursor_image(kind: u32, w: u32, h: u32, pitch: u32, buf: &[u8]) -> Option<CursorImage> {
    let real_h = if kind == 1 { h / 2 } else { h };
    if w == 0 || real_h == 0 || w > 256 || real_h > 256 {
        return None;
    }
    let (wu, hu, p) = (w as usize, real_h as usize, pitch as usize);
    let mut color = vec![0u8; wu * hu * 4];
    let mut invert = vec![0u8; wu * hu * 4];
    let mut any_invert = false;
    for y in 0..hu {
        for x in 0..wu {
            let o = (y * wu + x) * 4;
            let (px, inv): ([u8; 4], bool) = match kind {
                1 => {
                    let bit = |row: usize| buf.get(row * p + x / 8).is_some_and(|b| b >> (7 - x % 8) & 1 != 0);
                    match (bit(y), bit(y + hu)) {
                        (false, false) => ([0, 0, 0, 255], false),
                        (false, true) => ([255, 255, 255, 255], false),
                        (true, false) => ([0; 4], false),
                        (true, true) => ([0; 4], true),
                    }
                }
                2 => {
                    let s = buf.get(y * p + x * 4..y * p + x * 4 + 4)?;
                    ([s[0], s[1], s[2], s[3]], false)
                }
                4 => {
                    let s = buf.get(y * p + x * 4..y * p + x * 4 + 4)?;
                    if s[3] == 0 {
                        ([s[0], s[1], s[2], 255], false)
                    } else {
                        // Máscara ligada: a cor se combina com a tela. Quando não é preto, inverte.
                        ([0; 4], s[..3] != [0, 0, 0])
                    }
                }
                _ => return None,
            };
            color[o..o + 4].copy_from_slice(&px);
            if inv {
                invert[o..o + 4].copy_from_slice(&[255; 4]);
                any_invert = true;
            }
        }
    }
    Some(CursorImage { w, h: real_h, color, invert: any_invert.then_some(invert) })
}

const PS_CURSOR: &str = r#"
Texture2D tex : register(t0);
SamplerState smp : register(s0);
cbuffer C : register(b0) { float pq_on; float white; };
float3 pq(float3 v) {
    const float m1 = 0.1593017578125, m2 = 78.84375, c1 = 0.8359375, c2 = 18.8515625, c3 = 18.6875;
    float3 p = pow(max(v, 0), m1);
    return pow((c1 + c2 * p) / (1 + c3 * p), m2);
}
float3 lin(float3 c) { return c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4); }
float4 main(float4 p : SV_Position, float2 t : T) : SV_Target {
    float4 c = tex.Sample(smp, t);
    if (pq_on > 0.5) {
        float3x3 m = float3x3(0.6274, 0.3293, 0.0433, 0.0691, 0.9195, 0.0114, 0.0164, 0.0880, 0.8956);
        c.rgb = pq(mul(m, lin(c.rgb)) * white / 10000.0);
    }
    return c;
}
"#;

/// O cursor desenhado por cima da imagem: uma passada com transparência e, para os cursores que
/// invertem a tela, outra que inverte (mistura 1 − destino).
struct Cursor {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    rtv: ID3D11RenderTargetView,
    smp: ID3D11SamplerState,
    cb: ID3D11Buffer,
    blend: ID3D11BlendState,
    blend_invert: ID3D11BlendState,
    shape: Option<(u32, u32, ID3D11ShaderResourceView, Option<ID3D11ShaderResourceView>)>,
    pos: POINT,
    visible: bool,
}

impl Cursor {
    unsafe fn new(dev: &ID3D11Device, target: &ID3D11Texture2D, hdr10: bool, white_nits: f32) -> Result<Self> {
        let vb = compile(VS, s!("vs_5_0"))?;
        let pb = compile(PS_CURSOR, s!("ps_5_0"))?;
        let (mut vs, mut ps, mut rtv, mut smp, mut cb) = (None, None, None, None, None);
        dev.CreateVertexShader(&vb, None, Some(&mut vs))?;
        dev.CreatePixelShader(&pb, None, Some(&mut ps))?;
        dev.CreateRenderTargetView(target, None, Some(&mut rtv))?;
        let sd = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_POINT,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            MaxLOD: f32::MAX,
            ..Default::default()
        };
        dev.CreateSamplerState(&sd, Some(&mut smp))?;
        let data = [if hdr10 { 1.0 } else { 0.0 }, white_nits.max(80.0), 0.0, 0.0f32];
        let bd = D3D11_BUFFER_DESC { ByteWidth: 16, Usage: D3D11_USAGE_IMMUTABLE, BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32, ..Default::default() };
        let init = D3D11_SUBRESOURCE_DATA { pSysMem: data.as_ptr() as *const _, ..Default::default() };
        dev.CreateBuffer(&bd, Some(&init), Some(&mut cb))?;
        let state = |src: D3D11_BLEND, dst: D3D11_BLEND| -> Result<ID3D11BlendState> {
            let mut d = D3D11_BLEND_DESC::default();
            d.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                SrcBlend: src,
                DestBlend: dst,
                BlendOp: D3D11_BLEND_OP_ADD,
                SrcBlendAlpha: D3D11_BLEND_ZERO,
                DestBlendAlpha: D3D11_BLEND_ONE,
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
            };
            let mut b = None;
            dev.CreateBlendState(&d, Some(&mut b))?;
            Ok(b.unwrap())
        };
        Ok(Cursor {
            vs: vs.unwrap(),
            ps: ps.unwrap(),
            rtv: rtv.unwrap(),
            smp: smp.unwrap(),
            cb: cb.unwrap(),
            blend: state(D3D11_BLEND_SRC_ALPHA, D3D11_BLEND_INV_SRC_ALPHA)?,
            // Com o branco da máscara: 1 − destino; com o preto: o destino fica.
            blend_invert: state(D3D11_BLEND_INV_DEST_COLOR, D3D11_BLEND_INV_SRC_COLOR)?,
            shape: None,
            pos: POINT::default(),
            visible: false,
        })
    }

    unsafe fn set(&mut self, dev: &ID3D11Device, img: &CursorImage) -> Result<()> {
        let srv = |px: &[u8]| -> Result<ID3D11ShaderResourceView> {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: img.w,
                Height: img.h,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_IMMUTABLE,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                ..Default::default()
            };
            let init = D3D11_SUBRESOURCE_DATA { pSysMem: px.as_ptr() as *const _, SysMemPitch: img.w * 4, ..Default::default() };
            let (mut t, mut v) = (None, None);
            dev.CreateTexture2D(&desc, Some(&init), Some(&mut t))?;
            dev.CreateShaderResourceView(&t.unwrap(), None, Some(&mut v))?;
            Ok(v.unwrap())
        };
        let inv = match &img.invert {
            Some(m) => Some(srv(m)?),
            None => None,
        };
        self.shape = Some((img.w, img.h, srv(&img.color)?, inv));
        Ok(())
    }

    unsafe fn draw(&self, ctx: &ID3D11DeviceContext, size: (u32, u32)) -> Result<()> {
        let Some((w, h, color, inv)) = &self.shape else { return Ok(()) };
        if !self.visible || self.pos.x >= size.0 as i32 || self.pos.y >= size.1 as i32 {
            return Ok(());
        }
        ctx.OMSetRenderTargets(Some(&[Some(self.rtv.clone())]), None);
        let vp = D3D11_VIEWPORT { TopLeftX: self.pos.x as f32, TopLeftY: self.pos.y as f32, Width: *w as f32, Height: *h as f32, MinDepth: 0.0, MaxDepth: 1.0 };
        ctx.RSSetViewports(Some(&[vp]));
        ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        ctx.IASetInputLayout(None);
        ctx.VSSetShader(&self.vs, None);
        ctx.PSSetShader(&self.ps, None);
        ctx.PSSetSamplers(0, Some(&[Some(self.smp.clone())]));
        ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
        for (srv, blend) in [(Some(color), &self.blend), (inv.as_ref(), &self.blend_invert)] {
            let Some(srv) = srv else { continue };
            ctx.OMSetBlendState(blend, None, 0xFFFF_FFFF);
            ctx.PSSetShaderResources(0, Some(&[Some(srv.clone())]));
            ctx.Draw(3, 0);
        }
        ctx.OMSetBlendState(None, None, 0xFFFF_FFFF);
        ctx.PSSetShaderResources(0, Some(&[None]));
        ctx.OMSetRenderTargets(None, None);
        Ok(())
    }
}

/// Bancada: captura pela Desktop Duplication por alguns segundos, como o host faz, e conta.
/// `--test-dda=saida.txt|segundos`.
pub fn bench(arg: &str) {
    use std::fmt::Write as _;
    use windows::Win32::Foundation::POINT as Pt;
    use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTOPRIMARY};
    let mut parts = arg.split('|');
    let out = parts.next().unwrap_or("dda.txt").to_string();
    let secs: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(5);
    let mut r = String::from("Captura pela Desktop Duplication (fase 6)\n");
    let run = || -> Result<String> {
        unsafe {
            let d = crate::capture::d3d()?;
            let mon = MonitorFromPoint(Pt { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let color = crate::capture::monitor_color(mon);
            let o = output_of(&d, mon)?;
            let desc = o.GetDesc()?;
            let rc = desc.DesktopCoordinates;
            let (w, h) = ((rc.right - rc.left) as u32 & !1, (rc.bottom - rc.top) as u32 & !1);
            let mid = crate::capture::engine::bgra_texture(&d.dev, w, h, DXGI_FORMAT_B8G8R8A8_UNORM)?;
            let mut dda = Dda::new(&d, mon, color.hdr, color.white_nits, &mid, false, w, h)?;
            let mut s = format!("tela {w}x{h}, HDR {}, área de trabalho «{}»\n", if color.hdr { "sim" } else { "não" }, dda.desk_name);
            let t = Instant::now();
            let mut frames = 0u32;
            let mut gaps = Vec::new();
            let mut last = Instant::now();
            let mut poll_ms = Vec::new();
            while t.elapsed() < Duration::from_secs(secs) {
                let p = Instant::now();
                match dda.poll(&mid)? {
                    Got::Frame(_) => {
                        poll_ms.push(p.elapsed().as_secs_f64() * 1000.0);
                        frames += 1;
                        gaps.push(last.elapsed().as_secs_f64() * 1000.0);
                        last = Instant::now();
                    }
                    Got::Changed => {
                        s += "a tela mudou de tamanho no meio\n";
                        break;
                    }
                    Got::Nothing => {}
                }
            }
            let pct = |v: &mut Vec<f64>, q: f64| {
                v.sort_by(|a, b| a.total_cmp(b));
                v.get(((v.len().max(1) - 1) as f64 * q).round() as usize).copied().unwrap_or(0.0)
            };
            let el = t.elapsed().as_secs_f64();
            let _ = writeln!(s, "imagens com tela ou cursor novos: {frames} em {el:.1} s ({:.1}/s)", frames as f64 / el);
            let _ = writeln!(s, "tempo de pegar e compor (mediana, p95): {:.3} ms, {:.3} ms", pct(&mut poll_ms.clone(), 0.5), pct(&mut poll_ms, 0.95));
            let _ = writeln!(s, "formatos de cursor recebidos: {}, acesso perdido: {} vezes", dda.cursor_shapes, dda.lost);
            let _ = writeln!(s, "maior espera entre imagens: {:.1} ms", pct(&mut gaps, 1.0));
            Ok(s)
        }
    };
    match run() {
        Ok(s) => r += &s,
        Err(e) => {
            let _ = writeln!(r, "falhou: {e:?}");
        }
    }
    let _ = std::fs::write(out, r);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monochrome_cursor() {
        // 8x2: máscara E (2 linhas) e OU-exclusivo (2 linhas), um byte por linha.
        let buf = [0b0011_0000, 0b1100_0000, 0b0101_0000, 0b1010_0000];
        let img = cursor_image(1, 8, 4, 1, &buf).unwrap();
        assert_eq!((img.w, img.h), (8, 2));
        let px = |i: usize| &img.color[i * 4..i * 4 + 4];
        // E=0, XOR=0: preto. E=0, XOR=1: branco. E=1, XOR=0: transparente. E=1, XOR=1: inverte.
        assert_eq!(px(0), [0, 0, 0, 255]);
        assert_eq!(px(1), [255, 255, 255, 255]);
        assert_eq!(px(2), [0, 0, 0, 0]);
        assert_eq!(px(3), [0, 0, 0, 0]);
        let inv = img.invert.unwrap();
        assert_eq!(&inv[3 * 4..3 * 4 + 4], [255; 4]);
        assert_eq!(&inv[2 * 4..2 * 4 + 4], [0; 4]);
        // Segunda linha: E=1 nas duas primeiras, XOR=1 na primeira e na terceira.
        assert_eq!(&img.color[8 * 4..8 * 4 + 4], [0; 4]);
        assert_eq!(&inv[8 * 4..8 * 4 + 4], [255; 4]);
        assert_eq!(&img.color[10 * 4..10 * 4 + 4], [255, 255, 255, 255]);
    }

    #[test]
    fn masked_and_color_cursor() {
        // 2x1 com passo de 12 bytes (sobra no fim da linha).
        let buf = [10, 20, 30, 0, 0, 0, 0, 255, 9, 9, 9, 9];
        let img = cursor_image(4, 2, 1, 12, &buf).unwrap();
        assert_eq!(&img.color[..4], [10, 20, 30, 255]);
        assert_eq!(&img.color[4..8], [0; 4]);
        assert!(img.invert.is_none());
        let buf = [1, 2, 3, 128, 4, 5, 6, 7];
        let img = cursor_image(2, 2, 1, 8, &buf).unwrap();
        assert_eq!(img.color, buf.to_vec());
        assert!(cursor_image(3, 2, 1, 8, &buf).is_none());
        assert!(cursor_image(2, 0, 1, 8, &buf).is_none());
        assert!(cursor_image(2, 4, 1, 8, &buf).is_none(), "buffer curto");
    }
}
