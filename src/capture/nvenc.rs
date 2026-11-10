//! Codificador NVENC direto, para transmitir (fase de testes, `--features stream`).
//!
//! O Media Foundation recusa "baixa latência" e "sem quadros B" no codificador da NVIDIA e solta um
//! quadro-chave enorme a cada 2 segundos. Aqui o NVENC é chamado direto, pela DLL que vem com o
//! driver (`nvEncodeAPI64.dll`, ligações da `mini-nvcodec`, MIT ou Apache-2.0):
//! - preset P1 com ajuste de latência ultrabaixa, taxa constante e buffer de um quadro;
//! - sem quadro B e sem quadro-chave periódico: a imagem se renova aos poucos (intra refresh);
//! - quando quem assiste perde um quadro, o codificador para de usá-lo como referência
//!   (invalidação), em vez de mandar um quadro-chave inteiro.
//!
//! A textura NV12 do conversor entra direto, sem cópia, e cada pacote sai na hora (modo síncrono).

use std::cell::Cell;
use std::ffi::c_void;

use mini_nvcodec::guids::*;
use mini_nvcodec::nvenc_sys::*;
use mini_nvcodec::{require_nvenc, NvencApi, NvencApiVersion, API_VERSION_LADDER};
use windows::core::{Error, Interface, Result};
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::Graphics::Direct3D11::*;

use super::mux::{Codec, Pkt};

pub struct Nvenc {
    api: &'static NvencApi,
    ver: NvencApiVersion,
    enc: *mut c_void,
    out: NV_ENC_OUTPUT_PTR,
    /// Texturas já registradas: endereço da textura e o registro no NVENC.
    regs: Vec<(usize, NV_ENC_REGISTERED_PTR, ID3D11Texture2D)>,
    w: u32,
    h: u32,
    idx: u32,
    force: Cell<bool>,
    ready: Vec<Pkt>,
    pub name: String,
    /// Renova a imagem aos poucos, sem quadro-chave periódico.
    pub intra_refresh: bool,
    /// Sabe deixar de usar um quadro perdido como referência.
    pub can_invalidate: bool,
}

fn fail(what: &str, st: NVENCSTATUS) -> Error {
    Error::new(E_FAIL, format!("NVENC: {what} falhou ({})", st.0))
}

fn ok(what: &str, st: NVENCSTATUS) -> Result<()> {
    if st == NVENCSTATUS::SUCCESS {
        Ok(())
    } else {
        Err(fail(what, st))
    }
}

impl Nvenc {
    pub unsafe fn new(dev: &ID3D11Device, codec: Codec, w: u32, h: u32, fps: u32, bitrate: u32) -> Result<Self> {
        let api = require_nvenc().map_err(|e| Error::new(E_FAIL, format!("sem NVENC: {e}")))?;
        // Abre a sessão na versão mais nova da API que o driver aceita.
        let mut enc: *mut c_void = std::ptr::null_mut();
        let mut ver = None;
        for rung in API_VERSION_LADDER.iter().copied().filter(|r| *r <= api.created_with) {
            let mut p: NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS = std::mem::zeroed();
            p.version = rung.open_encode_session_ex_params_ver();
            p.deviceType = NV_ENC_DEVICE_TYPE::DIRECTX;
            p.device = dev.as_raw();
            p.apiVersion = rung.nvencapi_version();
            let st = (api.nvEncOpenEncodeSessionEx)(&mut p, &mut enc);
            if st == NVENCSTATUS::ERR_INVALID_VERSION {
                continue;
            }
            ok("abrir a sessão", st)?;
            ver = Some(rung);
            break;
        }
        let Some(ver) = ver else {
            return Err(Error::new(E_FAIL, "NVENC: o driver não aceita nenhuma versão conhecida da API"));
        };
        let mut me = Nvenc {
            api,
            ver,
            enc,
            out: std::ptr::null_mut(),
            regs: Vec::new(),
            w,
            h,
            idx: 0,
            force: Cell::new(false),
            ready: Vec::new(),
            name: String::new(),
            intra_refresh: false,
            can_invalidate: false,
        };
        // Daqui em diante, um erro fecha a sessão pelo Drop.
        me.init(codec, fps, bitrate)?;
        let (maj, min) = ver.major_minor();
        me.name = format!(
            "NVENC direto (API {maj}.{min}{}{})",
            if me.intra_refresh { ", renovação gradual" } else { "" },
            if me.can_invalidate { ", invalida referência perdida" } else { "" }
        );
        Ok(me)
    }

    unsafe fn cap(&self, guid: GUID, c: NV_ENC_CAPS) -> i32 {
        let mut p: NV_ENC_CAPS_PARAM = std::mem::zeroed();
        p.version = self.ver.caps_param_ver();
        p.capsToQuery = c;
        let mut v = 0;
        if (self.api.nvEncGetEncodeCaps)(self.enc, guid, &mut p, &mut v) == NVENCSTATUS::SUCCESS {
            v
        } else {
            0
        }
    }

    unsafe fn init(&mut self, codec: Codec, fps: u32, bitrate: u32) -> Result<()> {
        let guid = match codec {
            Codec::H264 => NV_ENC_CODEC_H264_GUID,
            Codec::Hevc => NV_ENC_CODEC_HEVC_GUID,
            Codec::Av1 => NV_ENC_CODEC_AV1_GUID,
        };
        self.intra_refresh = self.cap(guid, NV_ENC_CAPS::SUPPORT_INTRA_REFRESH) != 0;
        self.can_invalidate = self.cap(guid, NV_ENC_CAPS::SUPPORT_REF_PIC_INVALIDATION) != 0;

        let tuning = NV_ENC_TUNING_INFO::ULTRA_LOW_LATENCY;
        let mut pc: NV_ENC_PRESET_CONFIG = std::mem::zeroed();
        pc.version = self.ver.preset_config_ver();
        pc.presetCfg.version = self.ver.config_ver();
        ok("ler o preset", (self.api.nvEncGetEncodePresetConfigEx)(self.enc, guid, NV_ENC_PRESET_P1_GUID, tuning, &mut pc))?;
        let mut cfg = pc.presetCfg;
        cfg.version = self.ver.config_ver();
        cfg.gopLength = NVENC_INFINITE_GOPLENGTH;
        cfg.frameIntervalP = 1;
        let rc = &mut cfg.rcParams;
        rc.rateControlMode = NV_ENC_PARAMS_RC_MODE::CBR;
        rc.averageBitRate = bitrate;
        rc.maxBitRate = bitrate;
        // Um quadro de buffer: nenhum quadro fica muito maior que a média.
        rc.vbvBufferSize = bitrate / fps.max(1);
        rc.vbvInitialDelay = rc.vbvBufferSize;
        rc.set_enableLookahead(0);
        rc.lookaheadDepth = 0;
        rc.set_zeroReorderDelay(1);

        // Uma volta de renovação a cada 2 segundos, espalhada por um quarto de segundo.
        let period = fps * 2;
        let cnt = (fps / 4).max(4);
        let ir = self.intra_refresh;
        match codec {
            Codec::H264 => {
                let c = &mut cfg.encodeCodecConfig.h264Config;
                c.idrPeriod = NVENC_INFINITE_GOPLENGTH;
                c.set_repeatSPSPPS(1);
                if ir {
                    c.set_enableIntraRefresh(1);
                    c.intraRefreshPeriod = period;
                    c.intraRefreshCnt = cnt;
                }
            }
            Codec::Hevc => {
                let c = &mut cfg.encodeCodecConfig.hevcConfig;
                c.idrPeriod = NVENC_INFINITE_GOPLENGTH;
                c.set_repeatSPSPPS(1);
                if ir {
                    c.set_enableIntraRefresh(1);
                    c.intraRefreshPeriod = period;
                    c.intraRefreshCnt = cnt;
                }
            }
            Codec::Av1 => {
                let c = &mut cfg.encodeCodecConfig.av1Config;
                c.idrPeriod = NVENC_INFINITE_GOPLENGTH;
                c.set_repeatSeqHdr(1);
                if ir {
                    c.set_enableIntraRefresh(1);
                    c.intraRefreshPeriod = period;
                    c.intraRefreshCnt = cnt;
                }
            }
        }

        let mut ip: NV_ENC_INITIALIZE_PARAMS = std::mem::zeroed();
        ip.version = self.ver.initialize_params_ver();
        ip.encodeGUID = guid;
        ip.presetGUID = NV_ENC_PRESET_P1_GUID;
        ip.encodeWidth = self.w;
        ip.encodeHeight = self.h;
        ip.darWidth = self.w;
        ip.darHeight = self.h;
        ip.frameRateNum = fps;
        ip.frameRateDen = 1;
        ip.enableEncodeAsync = 0;
        ip.enablePTD = 1;
        ip.encodeConfig = &mut cfg;
        ip.tuningInfo = tuning;
        let st = (self.api.nvEncInitializeEncoder)(self.enc, &mut ip);
        if st != NVENCSTATUS::SUCCESS && ir {
            // Alguma placa recusa a renovação gradual com estes ajustes: tenta sem ela.
            self.intra_refresh = false;
            let mut cfg2 = pc.presetCfg;
            cfg2.version = self.ver.config_ver();
            cfg2.gopLength = NVENC_INFINITE_GOPLENGTH;
            cfg2.frameIntervalP = 1;
            cfg2.rcParams = cfg.rcParams;
            ip.encodeConfig = &mut cfg2;
            ok("iniciar o codificador", (self.api.nvEncInitializeEncoder)(self.enc, &mut ip))?;
        } else {
            ok("iniciar o codificador", st)?;
        }

        let mut bb: NV_ENC_CREATE_BITSTREAM_BUFFER = std::mem::zeroed();
        bb.version = self.ver.create_bitstream_buffer_ver();
        ok("criar a saída", (self.api.nvEncCreateBitstreamBuffer)(self.enc, &mut bb))?;
        self.out = bb.bitstreamBuffer;
        Ok(())
    }

    unsafe fn registered(&mut self, tex: &ID3D11Texture2D) -> Result<NV_ENC_REGISTERED_PTR> {
        let key = tex.as_raw() as usize;
        if let Some(r) = self.regs.iter().find(|r| r.0 == key) {
            return Ok(r.1);
        }
        let mut r: NV_ENC_REGISTER_RESOURCE = std::mem::zeroed();
        r.version = self.ver.register_resource_ver();
        r.resourceType = NV_ENC_INPUT_RESOURCE_TYPE::DIRECTX;
        r.width = self.w;
        r.height = self.h;
        r.resourceToRegister = tex.as_raw();
        r.bufferFormat = NV_ENC_BUFFER_FORMAT::NV12;
        r.bufferUsage = NV_ENC_BUFFER_USAGE::INPUT_IMAGE;
        ok("registrar a textura", (self.api.nvEncRegisterResource)(self.enc, &mut r))?;
        // Guarda a textura junto, para ela não sumir enquanto está registrada.
        self.regs.push((key, r.registeredResource, tex.clone()));
        Ok(r.registeredResource)
    }

    /// Codifica um quadro NV12 do tamanho combinado. O pacote sai em `pump`.
    pub unsafe fn feed(&mut self, tex: &ID3D11Texture2D, pts: i64) -> Result<()> {
        let reg = self.registered(tex)?;
        let mut m: NV_ENC_MAP_INPUT_RESOURCE = std::mem::zeroed();
        m.version = self.ver.map_input_resource_ver();
        m.registeredResource = reg;
        ok("mapear a textura", (self.api.nvEncMapInputResource)(self.enc, &mut m))?;
        let r = self.encode(m.mappedResource, m.mappedBufferFmt, pts);
        let _ = (self.api.nvEncUnmapInputResource)(self.enc, m.mappedResource);
        r
    }

    unsafe fn encode(&mut self, input: NV_ENC_INPUT_PTR, fmt: NV_ENC_BUFFER_FORMAT, pts: i64) -> Result<()> {
        let mut p: NV_ENC_PIC_PARAMS = std::mem::zeroed();
        p.version = self.ver.pic_params_ver();
        p.inputWidth = self.w;
        p.inputHeight = self.h;
        p.inputPitch = self.w;
        if self.force.replace(false) {
            p.encodePicFlags = NV_ENC_PIC_FLAG_FORCEIDR | NV_ENC_PIC_FLAG_OUTPUT_SPSPPS;
        }
        p.frameIdx = self.idx;
        self.idx = self.idx.wrapping_add(1);
        p.inputTimeStamp = pts as u64;
        p.inputBuffer = input;
        p.outputBitstream = self.out;
        p.bufferFmt = fmt;
        p.pictureStruct = NV_ENC_PIC_STRUCT::FRAME;
        let st = (self.api.nvEncEncodePicture)(self.enc, &mut p);
        if st == NVENCSTATUS::ERR_NEED_MORE_INPUT {
            return Ok(());
        }
        ok("codificar", st)?;

        let mut l: NV_ENC_LOCK_BITSTREAM = std::mem::zeroed();
        l.version = self.ver.lock_bitstream_ver();
        l.outputBitstream = self.out;
        ok("ler o pacote", (self.api.nvEncLockBitstream)(self.enc, &mut l))?;
        let data = std::slice::from_raw_parts(l.bitstreamBufferPtr as *const u8, l.bitstreamSizeInBytes as usize).to_vec();
        let key = l.pictureType == NV_ENC_PIC_TYPE::IDR || l.pictureType == NV_ENC_PIC_TYPE::I;
        let pts = l.outputTimeStamp as i64;
        let _ = (self.api.nvEncUnlockBitstream)(self.enc, self.out);
        self.ready.push(Pkt { data, pts, key });
        Ok(())
    }

    pub fn pump(&mut self, out: &mut Vec<Pkt>) {
        out.append(&mut self.ready);
    }

    /// O próximo quadro sai como quadro-chave.
    pub fn force_key(&self) {
        self.force.set(true);
    }

    /// Deixa de usar como referência o quadro com esta hora. Falso se o NVENC não aceitou.
    pub unsafe fn invalidate(&self, pts: i64) -> bool {
        self.can_invalidate && (self.api.nvEncInvalidateRefFrames)(self.enc, pts as u64) == NVENCSTATUS::SUCCESS
    }
}

impl Drop for Nvenc {
    fn drop(&mut self) {
        unsafe {
            for (_, r, _) in self.regs.drain(..) {
                let _ = (self.api.nvEncUnregisterResource)(self.enc, r);
            }
            if !self.out.is_null() {
                let _ = (self.api.nvEncDestroyBitstreamBuffer)(self.enc, self.out);
            }
            if !self.enc.is_null() {
                let _ = (self.api.nvEncDestroyEncoder)(self.enc);
            }
        }
    }
}
