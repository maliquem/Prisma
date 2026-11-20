//! Shaders do RetroArch (.slangp) no Prisma, a parte que o motor por cima (`prisma.exe --shader-profile`) e a
//! DLL de injeção (`shader-hook`) dividem: o formato do perfil, a junção dos presets, o detector de blocos de
//! pixels e a cadeia D3D11 (`chain::Engine`).

pub mod blocks;
pub mod chain;
pub mod link;
pub mod profile;

use std::sync::OnceLock;

use windows::core::*;
use windows::Win32::Graphics::Direct3D::Fxc::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

pub type Res<T> = std::result::Result<T, Box<dyn std::error::Error>>;

static LOG: OnceLock<fn(&str)> = OnceLock::new();

/// Para onde vão as mensagens: a saída de erro (o app manda para o log do motor) ou o que `set_log` escolher
/// (a DLL, dentro do jogo, não tem saída de erro).
pub fn set_log(f: fn(&str)) {
    let _ = LOG.set(f);
}

pub fn say(s: &str) {
    match LOG.get() {
        Some(f) => f(s),
        None => eprintln!("{s}"),
    }
}

#[macro_export]
macro_rules! say {
    ($($t:tt)*) => { $crate::say(&format!($($t)*)) };
}

/// Grava num temporário ao lado e troca: o arquivo nunca fica pela metade se o PC cair no meio.
pub fn write_safe(path: &std::path::Path, data: impl AsRef<[u8]>) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data.as_ref())?;
        // No disco antes de trocar: uma queda logo depois não deixa o arquivo vazio.
        f.sync_all()?;
    }
    std::fs::rename(tmp, path)
}

/// Triângulo que cobre a tela, sem buffer de vértices.
pub const VS: &str = r#"
struct O { float4 p : SV_Position; float2 t : T; };
O main(uint id : SV_VertexID) {
    O o;
    o.t = float2((id << 1) & 2, id & 2);
    o.p = float4(o.t * float2(2, -2) + float2(-1, 1), 0, 1);
    return o;
}
"#;

pub unsafe fn compile(src: &str, target: PCSTR) -> Result<Vec<u8>> {
    let mut blob = None;
    let mut err = None;
    let r = D3DCompile(src.as_ptr() as *const _, src.len(), None, None, None, s!("main"), target, D3DCOMPILE_OPTIMIZATION_LEVEL3, 0, &mut blob, Some(&mut err));
    if let Some(e) = &err {
        let m = std::slice::from_raw_parts(e.GetBufferPointer() as *const u8, e.GetBufferSize());
        say!("shader: {}", String::from_utf8_lossy(m));
    }
    r?;
    let b: ID3DBlob = blob.unwrap();
    Ok(std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()).to_vec())
}

/// Um ponto do centro de cada bloco: (bloco x, bloco y, começo x, começo y), em pixels da entrada.
pub const PS_REDUCE: &str = r#"
Texture2D tex : register(t0);
cbuffer C : register(b0) { float4 k; };
float4 main(float4 p : SV_Position, float2 t : T) : SV_Target {
    return tex.Load(int3(p.xy * k.xy + k.zw, 0));
}
"#;

/// Cópia esticada sem filtro (prévia sem efeito, cadeia vazia, troca de formato).
pub const PS_COPY: &str = r#"
Texture2D tex : register(t0);
SamplerState smp : register(s0);
float4 main(float4 p : SV_Position, float2 t : T) : SV_Target {
    return float4(tex.Sample(smp, t).rgb, 1);
}
"#;

/// Textura com leitura pelo shader; `rt` também deixa desenhar nela.
pub fn texture(dev: &ID3D11Device, w: u32, h: u32, fmt: DXGI_FORMAT, rt: bool) -> Res<(ID3D11Texture2D, ID3D11ShaderResourceView)> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: fmt,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | if rt { D3D11_BIND_RENDER_TARGET.0 } else { 0 }) as u32,
        ..Default::default()
    };
    unsafe {
        let mut tex = None;
        dev.CreateTexture2D(&desc, None, Some(&mut tex))?;
        let tex = tex.unwrap();
        let mut srv = None;
        dev.CreateShaderResourceView(&tex, None, Some(&mut srv))?;
        Ok((tex, srv.unwrap()))
    }
}

pub fn rtv(dev: &ID3D11Device, t: &ID3D11Texture2D) -> Res<ID3D11RenderTargetView> {
    let mut v = None;
    unsafe { dev.CreateRenderTargetView(t, None, Some(&mut v))? };
    Ok(v.unwrap())
}

/// Tempo de GPU da cadeia por consultas de carimbo: lidas alguns quadros depois, sem esperar a GPU.
pub struct GpuTimer {
    slots: Vec<[ID3D11Query; 3]>,
    used: Vec<bool>,
}

impl GpuTimer {
    pub fn new(dev: &ID3D11Device) -> Res<Self> {
        let make = |q| -> Res<ID3D11Query> {
            let mut out = None;
            unsafe { dev.CreateQuery(&D3D11_QUERY_DESC { Query: q, MiscFlags: 0 }, Some(&mut out))? };
            Ok(out.ok_or("sem consulta")?)
        };
        let mut slots = Vec::new();
        for _ in 0..4 {
            slots.push([make(D3D11_QUERY_TIMESTAMP_DISJOINT)?, make(D3D11_QUERY_TIMESTAMP)?, make(D3D11_QUERY_TIMESTAMP)?]);
        }
        Ok(Self { used: vec![false; slots.len()], slots })
    }

    /// Lê o resultado antigo da vaga (se pronto), em ms, e começa a medir nela.
    pub fn begin(&mut self, ctx: &ID3D11DeviceContext, n: usize) -> Option<f64> {
        let i = n % self.slots.len();
        let old = if self.used[i] { self.read(ctx, i) } else { None };
        let [dis, a, _] = &self.slots[i];
        unsafe {
            ctx.Begin(dis);
            ctx.End(a);
        }
        self.used[i] = true;
        old
    }

    pub fn end(&self, ctx: &ID3D11DeviceContext, n: usize) {
        let [dis, _, b] = &self.slots[n % self.slots.len()];
        unsafe {
            ctx.End(b);
            ctx.End(dis);
        }
    }

    // O GetData do windows-rs junta S_FALSE (ainda não pronto) com S_OK: a vtable devolve o HRESULT cru.
    fn get<T: Default>(ctx: &ID3D11DeviceContext, q: &ID3D11Query) -> Option<T> {
        let mut v = T::default();
        let hr = unsafe {
            (Interface::vtable(ctx).GetData)(Interface::as_raw(ctx), Interface::as_raw(q), &mut v as *mut T as *mut _, std::mem::size_of::<T>() as u32, D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32)
        };
        (hr == windows::Win32::Foundation::S_OK).then_some(v)
    }

    fn read(&self, ctx: &ID3D11DeviceContext, i: usize) -> Option<f64> {
        let [dis, a, b] = &self.slots[i];
        let d: D3D11_QUERY_DATA_TIMESTAMP_DISJOINT = Self::get(ctx, dis)?;
        let (t0, t1): (u64, u64) = (Self::get(ctx, a)?, Self::get(ctx, b)?);
        (!d.Disjoint.as_bool() && d.Frequency > 0).then(|| (t1.saturating_sub(t0)) as f64 * 1000.0 / d.Frequency as f64)
    }
}

/// Média que anda (o menu mostra o consumo sem tremer); `prev` 0 começa do valor novo.
pub fn ema(prev: f64, x: f64) -> f64 {
    if prev <= 0.0 { x } else { prev * 0.9 + x * 0.1 }
}

/// Parte da GPU que a cadeia usa, em %: o tempo dela dividido pelo intervalo entre quadros (até 100).
pub fn gpu_share(ms: f64, frame_ms: f64) -> Option<f64> {
    (ms > 0.0 && frame_ms > 0.0).then(|| (ms / frame_ms * 100.0).min(100.0))
}

/// Um desenho de tela inteira com um pixel shader nosso e 4 constantes, amostragem por ponto.
pub struct FullPass {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    smp: ID3D11SamplerState,
    cb: ID3D11Buffer,
}

impl FullPass {
    pub fn new(dev: &ID3D11Device, ps_src: &str) -> Res<Self> {
        unsafe {
            let (vb, pb) = (compile(VS, s!("vs_5_0"))?, compile(ps_src, s!("ps_5_0"))?);
            let (mut vs, mut ps, mut smp, mut cb) = (None, None, None, None);
            dev.CreateVertexShader(&vb, None, Some(&mut vs))?;
            dev.CreatePixelShader(&pb, None, Some(&mut ps))?;
            let sd = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_POINT,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            dev.CreateSamplerState(&sd, Some(&mut smp))?;
            let bd = D3D11_BUFFER_DESC { ByteWidth: 16, Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32, ..Default::default() };
            dev.CreateBuffer(&bd, None, Some(&mut cb))?;
            Ok(Self { vs: vs.unwrap(), ps: ps.unwrap(), smp: smp.unwrap(), cb: cb.unwrap() })
        }
    }

    /// Desenha `input` em `out`, no retângulo `vp` (x, y, largura, altura).
    pub fn draw(&self, ctx: &ID3D11DeviceContext, input: &ID3D11ShaderResourceView, out: &ID3D11RenderTargetView, vp: [f32; 4], k: [f32; 4]) -> Res<()> {
        unsafe {
            ctx.UpdateSubresource(&self.cb, 0, None, k.as_ptr() as *const _, 0, 0);
            ctx.OMSetRenderTargets(Some(&[Some(out.clone())]), None);
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: vp[0], TopLeftY: vp[1], Width: vp[2], Height: vp[3], MinDepth: 0.0, MaxDepth: 1.0 }]));
            ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.IASetInputLayout(None);
            ctx.VSSetShader(&self.vs, None);
            ctx.PSSetShader(&self.ps, None);
            ctx.PSSetShaderResources(0, Some(&[Some(input.clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.smp.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
            ctx.Draw(3, 0);
            // Solta a entrada e a saída: o librashader liga as dele em seguida.
            ctx.PSSetShaderResources(0, Some(&[None]));
            ctx.OMSetRenderTargets(None, None);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumo_da_gpu() {
        assert_eq!(gpu_share(2.0, 16.0), Some(12.5));
        assert_eq!(gpu_share(40.0, 16.0), Some(100.0));
        assert_eq!(gpu_share(0.0, 16.0), None);
        assert_eq!(gpu_share(2.0, 0.0), None);
        assert_eq!(ema(0.0, 3.0), 3.0);
        assert!((ema(2.0, 12.0) - 3.0).abs() < 1e-9);
    }
}
