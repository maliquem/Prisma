//! Conversão de cor na GPU quando o monitor está em HDR.
//! SDR: mesma curva da foto, em shader. HDR10: leva para BT.2020 e PQ.

use windows::core::*;
use windows::Win32::Graphics::Direct3D::Fxc::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
#[cfg(debug_assertions)]
use windows::Win32::Graphics::Dxgi::Common::*;

pub(crate) const VS: &str = r#"
struct O { float4 p : SV_Position; float2 t : T; };
O main(uint id : SV_VertexID) {
    O o;
    o.t = float2((id << 1) & 2, id & 2);
    o.p = float4(o.t * float2(2, -2) + float2(-1, 1), 0, 1);
    return o;
}
"#;

const PS_SDR: &str = r#"
Texture2D tex : register(t0);
SamplerState smp : register(s0);
cbuffer C : register(b0) { float scale; };
float roll(float l) {
    const float k = 0.8;
    return l <= k ? l : k + (1 - k) * (1 - exp(-(l - k) / (1 - k)));
}
float srgb(float v) {
    v = saturate(v);
    return v <= 0.0031308 ? 12.92 * v : 1.055 * pow(v, 1 / 2.4) - 0.055;
}
float4 main(float4 p : SV_Position, float2 t : T) : SV_Target {
    float3 c = max(tex.Sample(smp, t).rgb, 0) * scale;
    float l = dot(c, float3(0.2126, 0.7152, 0.0722));
    if (l > 0) c *= roll(l) / l;
    return float4(srgb(c.r), srgb(c.g), srgb(c.b), 1);
}
"#;

const PS_HDR10: &str = r#"
Texture2D tex : register(t0);
SamplerState smp : register(s0);
cbuffer C : register(b0) { float scale; };
float3 pq(float3 v) {
    const float m1 = 0.1593017578125, m2 = 78.84375, c1 = 0.8359375, c2 = 18.8515625, c3 = 18.6875;
    float3 p = pow(max(v, 0), m1);
    return pow((c1 + c2 * p) / (1 + c3 * p), m2);
}
float4 main(float4 p : SV_Position, float2 t : T) : SV_Target {
    float3 c = tex.Sample(smp, t).rgb;
    float3x3 m = float3x3(0.6274, 0.3293, 0.0433, 0.0691, 0.9195, 0.0114, 0.0164, 0.0880, 0.8956);
    c = mul(m, c) * 80.0 / 10000.0;
    return float4(pq(c), 1);
}
"#;

pub(crate) unsafe fn compile(src: &str, target: PCSTR) -> Result<Vec<u8>> {
    let mut blob = None;
    let mut err = None;
    let r = D3DCompile(
        src.as_ptr() as *const _,
        src.len(),
        None,
        None,
        None,
        s!("main"),
        target,
        D3DCOMPILE_OPTIMIZATION_LEVEL3,
        0,
        &mut blob,
        Some(&mut err),
    );
    if let Some(e) = &err {
        let m = std::slice::from_raw_parts(e.GetBufferPointer() as *const u8, e.GetBufferSize());
        eprintln!("shader: {}", String::from_utf8_lossy(m));
    }
    r?;
    let b: ID3DBlob = blob.unwrap();
    Ok(std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()).to_vec())
}

#[cfg(debug_assertions)]
/// RGB para YUV 4:4:4 (BT.709, faixa limitada), gravado como AYUV: na memória V, U, Y e A.
const PS_AYUV: &str = r#"
Texture2D tex : register(t0);
SamplerState smp : register(s0);
float4 main(float4 p : SV_Position, float2 t : T) : SV_Target {
    float3 c = tex.Sample(smp, t).rgb;
    float y = dot(c, float3(0.2126, 0.7152, 0.0722));
    float u = (c.b - y) / 1.8556;
    float v = (c.r - y) / 1.5748;
    return float4((128 + 224 * v) / 255, (128 + 224 * u) / 255, (16 + 219 * y) / 255, 1);
}
"#;

#[cfg(debug_assertions)]
/// Prepara o quadro do 4:4:4: escala e converte a cor num shader, porque o processador de vídeo do
/// Windows não grava AYUV e o NVENC, recebendo RGB, gasta uns 10 ms convertendo um quadro 4K.
pub struct ToAyuv {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    smp: ID3D11SamplerState,
    srv: ID3D11ShaderResourceView,
    outs: Vec<(ID3D11Texture2D, ID3D11RenderTargetView)>,
    next: usize,
    w: u32,
    h: u32,
}

#[cfg(debug_assertions)]
impl ToAyuv {
    pub fn new(dev: &ID3D11Device, src: &ID3D11Texture2D, w: u32, h: u32) -> Result<Self> {
        unsafe {
            let vb = compile(VS, s!("vs_5_0"))?;
            let pb = compile(PS_AYUV, s!("ps_5_0"))?;
            let (mut vs, mut ps) = (None, None);
            dev.CreateVertexShader(&vb, None, Some(&mut vs))?;
            dev.CreatePixelShader(&pb, None, Some(&mut ps))?;
            let sd = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            let mut smp = None;
            dev.CreateSamplerState(&sd, Some(&mut smp))?;
            let mut srv = None;
            dev.CreateShaderResourceView(src, None, Some(&mut srv))?;
            let mut outs = Vec::new();
            // Vários na roda: o NVENC ainda pode estar lendo o anterior.
            for _ in 0..16 {
                let td = D3D11_TEXTURE2D_DESC {
                    Width: w,
                    Height: h,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_AYUV,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                    ..Default::default()
                };
                let mut t = None;
                dev.CreateTexture2D(&td, None, Some(&mut t))?;
                let t = t.unwrap();
                let rd = D3D11_RENDER_TARGET_VIEW_DESC {
                    Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_RTV { MipSlice: 0 } },
                };
                let mut rtv = None;
                dev.CreateRenderTargetView(&t, Some(&rd), Some(&mut rtv))?;
                outs.push((t, rtv.unwrap()));
            }
            Ok(Self { vs: vs.unwrap(), ps: ps.unwrap(), smp: smp.unwrap(), srv: srv.unwrap(), outs, next: 0, w, h })
        }
    }

    pub fn convert(&mut self, ctx: &ID3D11DeviceContext) -> Result<ID3D11Texture2D> {
        let (tex, rtv) = &self.outs[self.next];
        self.next = (self.next + 1) % self.outs.len();
        unsafe {
            ctx.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            let vp = D3D11_VIEWPORT { Width: self.w as f32, Height: self.h as f32, MaxDepth: 1.0, ..Default::default() };
            ctx.RSSetViewports(Some(&[vp]));
            ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.IASetInputLayout(None);
            ctx.VSSetShader(&self.vs, None);
            ctx.PSSetShader(&self.ps, None);
            ctx.PSSetShaderResources(0, Some(&[Some(self.srv.clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.smp.clone())]));
            ctx.Draw(3, 0);
            ctx.PSSetShaderResources(0, Some(&[None]));
            ctx.OMSetRenderTargets(None, None);
        }
        Ok(tex.clone())
    }
}

pub struct ToneMap {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    rtv: ID3D11RenderTargetView,
    smp: ID3D11SamplerState,
    cb: ID3D11Buffer,
    cached: std::cell::RefCell<Option<(*mut std::ffi::c_void, ID3D11ShaderResourceView)>>,
}

impl ToneMap {
    pub fn new(dev: &ID3D11Device, target: &ID3D11Texture2D, hdr10: bool, white_nits: f32) -> Result<Self> {
        unsafe {
            let vb = compile(VS, s!("vs_5_0"))?;
            let pb = compile(if hdr10 { PS_HDR10 } else { PS_SDR }, s!("ps_5_0"))?;
            let (mut vs, mut ps) = (None, None);
            dev.CreateVertexShader(&vb, None, Some(&mut vs))?;
            dev.CreatePixelShader(&pb, None, Some(&mut ps))?;
            let mut rtv = None;
            dev.CreateRenderTargetView(target, None, Some(&mut rtv))?;
            let sd = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_POINT,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            let mut smp = None;
            dev.CreateSamplerState(&sd, Some(&mut smp))?;
            let data = [80.0 / white_nits.max(80.0), 0.0, 0.0, 0.0f32];
            let bd = D3D11_BUFFER_DESC {
                ByteWidth: 16,
                Usage: D3D11_USAGE_IMMUTABLE,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                ..Default::default()
            };
            let init = D3D11_SUBRESOURCE_DATA { pSysMem: data.as_ptr() as *const _, ..Default::default() };
            let mut cb = None;
            dev.CreateBuffer(&bd, Some(&init), Some(&mut cb))?;
            Ok(Self {
                vs: vs.unwrap(),
                ps: ps.unwrap(),
                rtv: rtv.unwrap(),
                smp: smp.unwrap(),
                cb: cb.unwrap(),
                cached: std::cell::RefCell::new(None),
            })
        }
    }

    pub fn draw(&self, ctx: &ID3D11DeviceContext, dev: &ID3D11Device, src: &ID3D11Texture2D, w: u32, h: u32) -> Result<()> {
        unsafe {
            let mut cache = self.cached.borrow_mut();
            let key = src.as_raw();
            let hit = cache.as_ref().filter(|c| c.0 == key).map(|c| c.1.clone());
            let srv = match hit {
                Some(s) => s,
                None => {
                    let mut s = None;
                    dev.CreateShaderResourceView(src, None, Some(&mut s))?;
                    let s = s.unwrap();
                    *cache = Some((key, s.clone()));
                    s
                }
            };
            ctx.OMSetRenderTargets(Some(&[Some(self.rtv.clone())]), None);
            let vp = D3D11_VIEWPORT { Width: w as f32, Height: h as f32, MaxDepth: 1.0, ..Default::default() };
            ctx.RSSetViewports(Some(&[vp]));
            ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.IASetInputLayout(None);
            ctx.VSSetShader(&self.vs, None);
            ctx.PSSetShader(&self.ps, None);
            ctx.PSSetShaderResources(0, Some(&[Some(srv)]));
            ctx.PSSetSamplers(0, Some(&[Some(self.smp.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
            ctx.Draw(3, 0);
            ctx.PSSetShaderResources(0, Some(&[None]));
            ctx.OMSetRenderTargets(None, None);
            Ok(())
        }
    }
}
