use crate::error;
use crate::error::assume_d3d_init;

use crate::binding::{ConstantDescriptor, RegisterAssignment, RegisterSet};
use librashader_common::map::FastHashMap;
use windows::core::PCSTR;
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCompile, D3DCOMPILE_AVOID_FLOW_CONTROL};
use windows::Win32::Graphics::Direct3D::ID3DBlob;

// const fn d3d9_format_fallback_list(format: D3DFORMAT) -> Option<&'static [D3DFORMAT]> {
//     match format {
//         DXGI_FORMAT_R32G32B32A32_FLOAT => Some(&[
//             windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32G32B32A32_FLOAT,
//             DXGI_FORMAT_R16G16B16A16_FLOAT,
//             DXGI_FORMAT_R32G32B32_FLOAT,
//             DXGI_FORMAT_R11G11B10_FLOAT,
//             DXGI_FORMAT_UNKNOWN,
//         ]),
//         DXGI_FORMAT_R16G16B16A16_FLOAT => Some(&[
//             windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R16G16B16A16_FLOAT,
//             DXGI_FORMAT_R32G32B32A32_FLOAT,
//             DXGI_FORMAT_R32G32B32_FLOAT,
//             DXGI_FORMAT_R11G11B10_FLOAT,
//             DXGI_FORMAT_UNKNOWN,
//         ]),
//         DXGI_FORMAT_R8G8B8A8_UNORM => Some(&[
//             windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM,
//             DXGI_FORMAT_B8G8R8A8_UNORM,
//             DXGI_FORMAT_B8G8R8X8_UNORM,
//             DXGI_FORMAT_UNKNOWN,
//         ]),
//         DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => Some(&[
//             windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
//             DXGI_FORMAT_R8G8B8A8_UNORM,
//             DXGI_FORMAT_B8G8R8A8_UNORM,
//             DXGI_FORMAT_B8G8R8X8_UNORM,
//             DXGI_FORMAT_UNKNOWN,
//         ]),
//         DXGI_FORMAT_B8G8R8A8_UNORM => Some(&[
//             windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
//             DXGI_FORMAT_R8G8B8A8_UNORM,
//             DXGI_FORMAT_UNKNOWN,
//         ]),
//         DXGI_FORMAT_B8G8R8X8_UNORM => Some(&[
//             windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8X8_UNORM,
//             DXGI_FORMAT_B8G8R8A8_UNORM,
//             DXGI_FORMAT_R8G8B8A8_UNORM,
//             DXGI_FORMAT_UNKNOWN,
//         ]),
//         DXGI_FORMAT_B5G6R5_UNORM => Some(&[
//             windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B5G6R5_UNORM,
//             DXGI_FORMAT_B8G8R8X8_UNORM,
//             DXGI_FORMAT_B8G8R8A8_UNORM,
//             DXGI_FORMAT_R8G8B8A8_UNORM,
//             DXGI_FORMAT_UNKNOWN,
//         ]),
//         DXGI_FORMAT_EX_A4R4G4B4_UNORM | DXGI_FORMAT_B4G4R4A4_UNORM => Some(&[
//             windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B4G4R4A4_UNORM,
//             DXGI_FORMAT_B8G8R8A8_UNORM,
//             DXGI_FORMAT_R8G8B8A8_UNORM,
//             DXGI_FORMAT_UNKNOWN,
//         ]),
// //         DXGI_FORMAT_A8_UNORM => Some(&[
// //             windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_A8_UNORM,
// //             DXGI_FORMAT_R8_UNORM,
// //             DXGI_FORMAT_R8G8_UNORM,
// //             DXGI_FORMAT_R8G8B8A8_UNORM,
// //             DXGI_FORMAT_B8G8R8A8_UNORM,
// //             DXGI_FORMAT_UNKNOWN,
// //         ]),
// //         DXGI_FORMAT_R8_UNORM => Some(&[
// //             windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8_UNORM,
// //             DXGI_FORMAT_A8_UNORM,
// //             DXGI_FORMAT_R8G8_UNORM,
// //             DXGI_FORMAT_R8G8B8A8_UNORM,
// //             DXGI_FORMAT_B8G8R8A8_UNORM,
// //             DXGI_FORMAT_UNKNOWN,
// //         ]),
// //         _ => None,
// //     }
// // }
//
// pub fn d3d9_get_closest_format(
//     device: &IDirect3DDevice9,
//     format: D3DFORMAT,
//     restype: D3DRESOURCETYPE,
//     format_support_mask: i32,
// ) -> error::Result<D3DFORMAT> {
//     let d3d9 = unsafe { device.GetDirect3D()? };
//     let (devtype, ordinal) = unsafe {
//         let mut params = Default::default();
//         device.GetCreationParameters(&mut params)?;
//         (params.DeviceType, params.AdapterOrdinal)
//     };
//
//     let default_list = [format, D3DFMT_UNKNOWN];
//     let format_support_list = d3d9_format_fallback_list(format).unwrap_or(&default_list);
//     let format_support_mask = format_support_mask as u32;
//
//     for supported in format_support_list {
//         unsafe {
//             if let Ok(_) = d3d9.CheckDeviceFormat(
//                 ordinal,devtype,
//                 format, format_support_mask,
//                 restype,
//                 format)
//             {
//                 return Ok(*supported);
//             }
//         }
//     }
//
//     Ok(D3DFMT_UNKNOWN)
// }

pub fn d3d_compile_shader(source: &[u8], entry: &[u8], version: &[u8]) -> error::Result<ID3DBlob> {
    unsafe {
        let mut blob = None;
        let mut errs = None;

        let mut res = D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            None,
            None,
            None,
            PCSTR(entry.as_ptr()),
            PCSTR(version.as_ptr()),
            D3DCOMPILE_AVOID_FLOW_CONTROL,
            0,
            &mut blob,
            Some(&mut errs),
        );

        if res.is_err() {
            res = D3DCompile(
                source.as_ptr().cast(),
                source.len(),
                None,
                None,
                None,
                PCSTR(entry.as_ptr()),
                PCSTR(version.as_ptr()),
                0,
                0,
                &mut blob,
                Some(&mut errs),
            );
        }

        // let res = D3DXCompileShader(
        //     source.as_ptr().cast(),
        //     source.len(),
        //     None,
        //     None,
        //     PCSTR(entry.as_ptr()),
        //     PCSTR(version.as_ptr()),
        //     0,
        //     &mut blob,
        //     Some(&mut errs),
        //     None,
        // );

        // if let Some(errs) = errs {
        //     let str = std::slice::from_raw_parts(
        //         errs.GetBufferPointer().cast::<u8>(),
        //         errs.GetBufferSize(),
        //     );
        //     let str = std::ffi::CStr::from_bytes_until_nul(str).unwrap();
        //     // eprintln!("{}", str.to_str().unwrap());
        // }

        res?;
        assume_d3d_init!(blob, "D3DCompile");

        Ok(blob)
    }
}
/// Registradores de cada constante, lidos da tabela CTAB que o compilador deixa num comentário do bytecode SM3.
/// Prisma: no lugar do D3DXGetShaderConstantTable, para não depender do D3DX9_43.dll (DirectX de junho de 2010).
pub fn d3d_reflect_shader(
    shader: ID3DBlob,
) -> error::Result<FastHashMap<String, ConstantDescriptor>> {
    let code = unsafe {
        std::slice::from_raw_parts(shader.GetBufferPointer().cast::<u8>(), shader.GetBufferSize())
    };
    ctab(code).ok_or(error::FilterChainError::Direct3DOperationError("CTAB"))
}

fn ctab(code: &[u8]) -> Option<FastHashMap<String, ConstantDescriptor>> {
    let u32_at = |b: &[u8], o: usize| Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?));
    let u16_at = |b: &[u8], o: usize| Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?));
    // Depois do token de versão vêm os comentários (0xFFFE, tamanho em DWORDs nos bits 16-30); o CTAB é um deles.
    let mut at = 4;
    let t = loop {
        let tok = u32_at(code, at)?;
        if tok & 0xFFFF != 0xFFFE {
            return None;
        }
        let len = ((tok >> 16) & 0x7FFF) as usize * 4;
        let data = code.get(at + 4..at + 4 + len)?;
        if data.get(..4) == Some(b"CTAB") {
            break &data[4..];
        }
        at += 4 + len;
    };
    // D3DXSHADER_CONSTANTTABLE: Size, Creator, Version, Constants, ConstantInfo, ...; offsets a partir de `t`.
    let (n, info) = (u32_at(t, 12)? as usize, u32_at(t, 16)? as usize);
    let mut out = FastHashMap::default();
    for i in 0..n {
        // D3DXSHADER_CONSTANTINFO (20 bytes): Name, RegisterSet, RegisterIndex, RegisterCount, ...
        let c = info + i * 20;
        let name = t.get(u32_at(t, c)? as usize..)?;
        let name = std::ffi::CStr::from_bytes_until_nul(name).ok()?.to_str().ok()?.to_string();
        // Só cN e sN (D3DXRS_FLOAT4 = 2, D3DXRS_SAMPLER = 3).
        let set = match u16_at(t, c + 4)? {
            2 => RegisterSet::Float,
            3 => RegisterSet::Sampler,
            _ => continue,
        };
        let assignment = RegisterAssignment { index: u16_at(t, c + 6)? as u32, count: u16_at(t, c + 8)? as u32 };
        out.insert(name, ConstantDescriptor { assignment, set });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn ctab_igual_ao_d3dx() {
        // ps_3_0 com um float4, um array de 2 e um sampler, compilado pelo d3dcompiler_47 como a cadeia faz.
        let src = "float4 a; float4 b[2]; sampler2D s; float4 main(float2 uv : TEXCOORD0) : COLOR { return tex2D(s, uv) * a + b[1]; }";
        let blob = super::d3d_compile_shader(src.as_bytes(), b"main\0", b"ps_3_0\0").unwrap();
        let t = super::d3d_reflect_shader(blob).unwrap();
        let get = |n: &str| t.get(n).map(|c| (c.assignment.index, c.assignment.count, matches!(c.set, super::RegisterSet::Sampler)));
        assert_eq!(get("a").map(|x| (x.1, x.2)), Some((1, false)));
        assert_eq!(get("b").map(|x| (x.1, x.2)), Some((2, false)));
        assert_eq!(get("s"), Some((0, 1, true)));
    }
}
