//! Codificador para transmitir: o NVENC direto quando existe (só com `--features stream`), senão o
//! AMF direto nas placas AMD, e por último o Media Foundation. `PRISMA_ENC=mf` força o Media Foundation,
//! `PRISMA_ENC=nvenc` só aceita o NVENC e `PRISMA_ENC=amf` só aceita o AMF.

use windows::core::*;
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::Graphics::Direct3D11::*;

use super::amf::Amf;
use super::encoder::Encoder;
use super::mux::{Codec, Pkt};
#[cfg(feature = "stream")]
use super::nvenc::Nvenc;

/// Formato do vídeo além do comum (4:2:0, 8 bits, SDR). Os dois só existem no NVENC direto.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct VideoFmt {
    /// 10 bits, BT.2020 com PQ: a tela HDR do host chega HDR em quem assiste.
    pub hdr10: bool,
    /// Cor inteira em cada pixel: texto pequeno e linhas finas sem borrão de cor.
    pub yuv444: bool,
    /// 10 bits com a tela SDR (BT.709): degradês sem faixas. Só 4:2:0, só HEVC e AV1.
    pub deep: bool,
}

pub enum LiveEncoder {
    Mf(Encoder),
    Amf(Amf),
    #[cfg(feature = "stream")]
    Nv(Nvenc),
}

impl LiveEncoder {
    pub fn open(dev: &ID3D11Device, codec: Codec, w: u32, h: u32, fps: u32, bitrate: u32, fmt: VideoFmt) -> Result<Self> {
        let pick = std::env::var("PRISMA_ENC").unwrap_or_default();
        let plain = fmt == VideoFmt::default();
        #[cfg(feature = "stream")]
        if pick != "mf" && pick != "amf" {
            match unsafe { Nvenc::new(dev, codec, w, h, fps, bitrate, fmt) } {
                Ok(n) => return Ok(LiveEncoder::Nv(n)),
                Err(e) if pick == "nvenc" || !plain => return Err(e),
                Err(_) => {}
            }
        }
        if pick == "nvenc" || !plain {
            return Err(Error::new(E_FAIL, "NVENC direto (e HDR10 ou 4:4:4) só existe compilando com --features stream"));
        }
        // Sem placa AMD (ou com o AMF recusando algo), segue para o Media Foundation.
        if pick != "mf" {
            match unsafe { Amf::new(dev, codec, w, h, fps, bitrate) } {
                Ok(a) => return Ok(LiveEncoder::Amf(a)),
                Err(e) if pick == "amf" => return Err(e),
                Err(_) => {}
            }
        }
        Encoder::live(dev, codec, w, h, fps, bitrate).map(LiveEncoder::Mf)
    }

    pub fn name(&self) -> String {
        match self {
            LiveEncoder::Mf(e) => format!("{} (Media Foundation)", e.name),
            LiveEncoder::Amf(a) => a.name.clone(),
            #[cfg(feature = "stream")]
            LiveEncoder::Nv(n) => n.name.clone(),
        }
    }

    /// Ajustes que o codificador recusou (só o Media Foundation recusa).
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    pub fn refused(&self) -> Vec<&'static str> {
        match self {
            LiveEncoder::Mf(e) => e.refused.clone(),
            LiveEncoder::Amf(_) => Vec::new(),
            #[cfg(feature = "stream")]
            LiveEncoder::Nv(_) => Vec::new(),
        }
    }

    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    pub fn header(&self) -> Option<Vec<u8>> {
        match self {
            LiveEncoder::Mf(e) => e.header(),
            LiveEncoder::Amf(_) => None,
            #[cfg(feature = "stream")]
            LiveEncoder::Nv(_) => None,
        }
    }

    /// Pode receber um quadro agora.
    pub fn ready(&self) -> bool {
        match self {
            LiveEncoder::Mf(e) => e.need > 0,
            LiveEncoder::Amf(_) => true,
            #[cfg(feature = "stream")]
            LiveEncoder::Nv(_) => true,
        }
    }

    pub fn pump(&mut self, out: &mut Vec<Pkt>) -> Result<()> {
        match self {
            LiveEncoder::Mf(e) => e.pump(out),
            LiveEncoder::Amf(a) => {
                a.pump(out);
                Ok(())
            }
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
            LiveEncoder::Amf(a) => unsafe { a.feed(tex, pts) },
            #[cfg(feature = "stream")]
            LiveEncoder::Nv(n) => unsafe { n.feed(tex, pts) },
        }
    }

    pub fn force_key(&self) {
        match self {
            LiveEncoder::Mf(e) => e.force_key(),
            LiveEncoder::Amf(a) => a.force_key(),
            #[cfg(feature = "stream")]
            LiveEncoder::Nv(n) => n.force_key(),
        }
    }

    /// Troca a taxa sem reabrir (controle de taxa da transmissão). Falso se o codificador recusou.
    pub fn set_bitrate(&mut self, bitrate: u32) -> bool {
        match self {
            LiveEncoder::Mf(e) => e.set_bitrate(bitrate),
            LiveEncoder::Amf(a) => unsafe { a.set_bitrate(bitrate) },
            #[cfg(feature = "stream")]
            LiveEncoder::Nv(n) => unsafe { n.set_bitrate(bitrate) },
        }
    }

    /// Sabe se recuperar de um quadro perdido sem mandar quadro-chave.
    pub fn recovers(&self) -> bool {
        match self {
            LiveEncoder::Mf(_) | LiveEncoder::Amf(_) => false,
            #[cfg(feature = "stream")]
            LiveEncoder::Nv(n) => n.can_invalidate,
        }
    }

    /// Deixa de usar como referência o quadro com esta hora.
    #[cfg_attr(not(feature = "stream"), allow(unused_variables))]
    pub fn invalidate(&self, pts: i64) -> bool {
        match self {
            LiveEncoder::Mf(_) | LiveEncoder::Amf(_) => false,
            #[cfg(feature = "stream")]
            LiveEncoder::Nv(n) => unsafe { n.invalidate(pts) },
        }
    }
}
