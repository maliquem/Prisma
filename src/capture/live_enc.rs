//! Codificador para transmitir: o NVENC direto quando existe (só com `--features stream`), senão o
//! Media Foundation. `PRISMA_ENC=mf` força o Media Foundation e `PRISMA_ENC=nvenc` só aceita o NVENC.

use windows::core::*;
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::Graphics::Direct3D11::*;

use super::encoder::Encoder;
use super::mux::{Codec, Pkt};
#[cfg(feature = "stream")]
use super::nvenc::Nvenc;

pub enum LiveEncoder {
    Mf(Encoder),
    #[cfg(feature = "stream")]
    Nv(Nvenc),
}

impl LiveEncoder {
    pub fn open(dev: &ID3D11Device, codec: Codec, w: u32, h: u32, fps: u32, bitrate: u32) -> Result<Self> {
        let pick = std::env::var("PRISMA_ENC").unwrap_or_default();
        #[cfg(feature = "stream")]
        if pick != "mf" {
            match unsafe { Nvenc::new(dev, codec, w, h, fps, bitrate) } {
                Ok(n) => return Ok(LiveEncoder::Nv(n)),
                Err(e) if pick == "nvenc" => return Err(e),
                Err(_) => {}
            }
        }
        if pick == "nvenc" {
            return Err(Error::new(E_FAIL, "NVENC direto só existe compilando com --features stream"));
        }
        Encoder::live(dev, codec, w, h, fps, bitrate).map(LiveEncoder::Mf)
    }

    pub fn name(&self) -> String {
        match self {
            LiveEncoder::Mf(e) => format!("{} (Media Foundation)", e.name),
            #[cfg(feature = "stream")]
            LiveEncoder::Nv(n) => n.name.clone(),
        }
    }

    /// Ajustes que o codificador recusou (só o Media Foundation recusa).
    pub fn refused(&self) -> Vec<&'static str> {
        match self {
            LiveEncoder::Mf(e) => e.refused.clone(),
            #[cfg(feature = "stream")]
            LiveEncoder::Nv(_) => Vec::new(),
        }
    }

    pub fn header(&self) -> Option<Vec<u8>> {
        match self {
            LiveEncoder::Mf(e) => e.header(),
            #[cfg(feature = "stream")]
            LiveEncoder::Nv(_) => None,
        }
    }

    /// Pode receber um quadro agora.
    pub fn ready(&self) -> bool {
        match self {
            LiveEncoder::Mf(e) => e.need > 0,
            #[cfg(feature = "stream")]
            LiveEncoder::Nv(_) => true,
        }
    }

    pub fn pump(&mut self, out: &mut Vec<Pkt>) -> Result<()> {
        match self {
            LiveEncoder::Mf(e) => e.pump(out),
            #[cfg(feature = "stream")]
            LiveEncoder::Nv(n) => {
                n.pump(out);
                Ok(())
            }
        }
    }

    pub fn feed(&mut self, tex: &ID3D11Texture2D, pts: i64) -> Result<()> {
        match self {
            LiveEncoder::Mf(e) => e.feed(tex, pts),
            #[cfg(feature = "stream")]
            LiveEncoder::Nv(n) => unsafe { n.feed(tex, pts) },
        }
    }

    pub fn force_key(&self) {
        match self {
            LiveEncoder::Mf(e) => e.force_key(),
            #[cfg(feature = "stream")]
            LiveEncoder::Nv(n) => n.force_key(),
        }
    }

    /// Sabe se recuperar de um quadro perdido sem mandar quadro-chave.
    pub fn recovers(&self) -> bool {
        match self {
            LiveEncoder::Mf(_) => false,
            #[cfg(feature = "stream")]
            LiveEncoder::Nv(n) => n.can_invalidate,
        }
    }

    /// Deixa de usar como referência o quadro com esta hora.
    #[cfg_attr(not(feature = "stream"), allow(unused_variables))]
    pub fn invalidate(&self, pts: i64) -> bool {
        match self {
            LiveEncoder::Mf(_) => false,
            #[cfg(feature = "stream")]
            LiveEncoder::Nv(n) => unsafe { n.invalidate(pts) },
        }
    }
}
