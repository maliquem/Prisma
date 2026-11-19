use crate::error;
use crate::error::assume_d3d_init;
use crate::texture::D3D9InputTexture;

use librashader_presets::TextureMeta;
use librashader_runtime::image::{Image, BGRA8};

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D9::{
    IDirect3DDevice9, IDirect3DDevice9Ex, D3DFMT_A8R8G8B8, D3DLOCKED_RECT, D3DPOOL_DEFAULT, D3DPOOL_MANAGED,
    D3DPOOL_SYSTEMMEM, D3DUSAGE_AUTOGENMIPMAP,
};

#[derive(Debug, Clone)]
pub(crate) struct LutTexture(D3D9InputTexture);

impl AsRef<D3D9InputTexture> for LutTexture {
    fn as_ref(&self) -> &D3D9InputTexture {
        &self.0
    }
}

impl LutTexture {
    pub fn new(
        device: &IDirect3DDevice9,
        source: &Image<BGRA8>,
        config: &TextureMeta,
    ) -> error::Result<LutTexture> {
        let mut texture = None;
        // Prisma: o D3D9Ex não tem D3DPOOL_MANAGED (INVALIDCALL): a imagem vai para a memória do sistema e sobe
        // por UpdateTexture para uma textura em D3DPOOL_DEFAULT.
        let ex = device.cast::<IDirect3DDevice9Ex>().is_ok();
        unsafe {
            device.CreateTexture(
                source.size.width,
                source.size.height,
                if config.mipmap && !ex { 0 } else { 1 },
                0,
                D3DFMT_A8R8G8B8,
                if ex { D3DPOOL_SYSTEMMEM } else { D3DPOOL_MANAGED },
                &mut texture,
                std::ptr::null_mut(),
            )?;
        }
        assume_d3d_init!(mut texture, "CreateTexture");

        unsafe {
            let mut lock = D3DLOCKED_RECT::default();
            texture.LockRect(0, &mut lock, std::ptr::null_mut(), 0)?;
            std::ptr::copy_nonoverlapping(
                source.bytes.as_ptr(),
                lock.pBits.cast(),
                source.bytes.len(),
            );
            texture.UnlockRect(0)?;

            if ex {
                let mut gpu = None;
                device.CreateTexture(
                    source.size.width,
                    source.size.height,
                    if config.mipmap { 0 } else { 1 },
                    if config.mipmap { D3DUSAGE_AUTOGENMIPMAP as u32 } else { 0 },
                    D3DFMT_A8R8G8B8,
                    D3DPOOL_DEFAULT,
                    &mut gpu,
                    std::ptr::null_mut(),
                )?;
                assume_d3d_init!(gpu, "CreateTexture");
                device.UpdateTexture(&texture, &gpu)?;
                texture = gpu;
            }

            if config.mipmap {
                texture.GenerateMipSubLevels();
            }
        }

        Ok(LutTexture(D3D9InputTexture {
            handle: texture,
            filter: config.filter_mode,
            wrap: config.wrap_mode,
            mipmode: config.filter_mode,
            is_srgb: false,
        }))
    }
}
