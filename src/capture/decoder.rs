//! Decodificador de vídeo do Media Foundation, na GPU quando ele aceita o Direct3D 11.
//! Serve à bancada da transmissão e a quem assiste.

use std::collections::VecDeque;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;

use super::encoder::friendly;
use super::mux::{Codec, Pkt};

pub fn subtype(c: Codec) -> GUID {
    match c {
        Codec::Av1 => MFVideoFormat_AV1,
        Codec::Hevc => MFVideoFormat_HEVC,
        Codec::H264 => MFVideoFormat_H264,
    }
}

fn packed(a: u32, b: u32) -> u64 {
    (a as u64) << 32 | b as u64
}

/// Diz se este PC tem decodificador para o codec, sem criar nenhum.
pub fn can_decode(c: Codec) -> bool {
    unsafe {
        let i = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: subtype(c) };
        let o = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_NV12 };
        let mut list = std::ptr::null_mut();
        let mut n = 0u32;
        let flags = MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER;
        if MFTEnumEx(MFT_CATEGORY_VIDEO_DECODER, flags, Some(&i), Some(&o), &mut list, &mut n).is_err() || list.is_null() {
            return false;
        }
        // Solta cada ativação antes de liberar a lista.
        for i in 0..n as usize {
            drop(std::ptr::read(list.add(i)));
        }
        CoTaskMemFree(Some(list as *const _));
        n > 0
    }
}

/// Um quadro que saiu do decodificador. A textura vale enquanto `sample` existir.
pub struct Decoded {
    pub pts: i64,
    pub sample: IMFSample,
}

impl Decoded {
    /// Textura do quadro e a fatia dela onde está a imagem.
    pub fn texture(&self) -> Option<(ID3D11Texture2D, u32)> {
        unsafe {
            let b = self.sample.GetBufferByIndex(0).ok()?;
            let dx: IMFDXGIBuffer = b.cast().ok()?;
            let mut p: *mut std::ffi::c_void = std::ptr::null_mut();
            dx.GetResource(&ID3D11Texture2D::IID, &mut p).ok()?;
            let tex = ID3D11Texture2D::from_raw(p);
            Some((tex, dx.GetSubresourceIndex().ok()?))
        }
    }
}

/// Decodificador do Media Foundation, na GPU quando ele aceita o Direct3D 11.
pub struct Decoder {
    mft: IMFTransform,
    events: Option<IMFMediaEventGenerator>,
    need: u32,
    queue: VecDeque<Pkt>,
    provides: bool,
    size: u32,
    fps: u32,
    pub name: String,
    pub gpu: bool,
}

impl Decoder {
    pub unsafe fn new(dev: &ID3D11Device, codec: Codec, w: u32, h: u32, fps: u32, header: Option<&[u8]>) -> Result<Self> {
        let i = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: subtype(codec) };
        let o = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_NV12 };
        let mut list = std::ptr::null_mut();
        let mut n = 0u32;
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_DECODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&i),
            Some(&o),
            &mut list,
            &mut n,
        )?;
        if n == 0 || list.is_null() {
            return Err(Error::new(E_FAIL, "sem decodificador"));
        }
        let items = std::slice::from_raw_parts(list, n as usize);
        let act = items[0].as_ref().unwrap();
        let name = friendly(act);
        let mft = act.ActivateObject::<IMFTransform>();
        CoTaskMemFree(Some(list as *const _));
        let mft = mft?;

        let attrs = mft.GetAttributes()?;
        let is_async = attrs.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 0;
        if is_async {
            attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
        }
        let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
        let mut gpu = false;
        if attrs.GetUINT32(&MF_SA_D3D11_AWARE).unwrap_or(0) != 0 {
            let mut token = 0;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.unwrap();
            manager.ResetDevice(dev, token)?;
            gpu = mft.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize).is_ok();
        }

        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, &subtype(codec))?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, packed(w, h))?;
        t.SetUINT64(&MF_MT_FRAME_RATE, packed(fps, 1))?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, packed(1, 1))?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        if let Some(hd) = header {
            let _ = t.SetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, hd);
        }
        mft.SetInputType(0, &t, 0)?;
        let events = if is_async { Some(mft.cast::<IMFMediaEventGenerator>()?) } else { None };
        let mut s = Self { mft, events, need: 0, queue: VecDeque::new(), provides: false, size: 0, fps, name, gpu };
        // Alguns só dizem a saída depois do primeiro quadro; aí ela vem na troca de formato.
        let _ = s.pick_output();
        s.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
        s.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        Ok(s)
    }

    unsafe fn pick_output(&mut self) -> Result<()> {
        let mut i = 0;
        loop {
            let t = self.mft.GetOutputAvailableType(0, i)?;
            if t.GetGUID(&MF_MT_SUBTYPE)? == MFVideoFormat_NV12 {
                self.mft.SetOutputType(0, &t, 0)?;
                break;
            }
            i += 1;
        }
        let info = self.mft.GetOutputStreamInfo(0)?;
        let own = (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32;
        self.provides = info.dwFlags & own != 0;
        self.size = info.cbSize;
        Ok(())
    }

    unsafe fn sample(&self, p: &Pkt) -> Result<IMFSample> {
        let len = p.data.len() as u32;
        let b = MFCreateMemoryBuffer(len)?;
        let mut ptr = std::ptr::null_mut();
        b.Lock(&mut ptr, None, None)?;
        std::ptr::copy_nonoverlapping(p.data.as_ptr(), ptr, p.data.len());
        b.Unlock()?;
        b.SetCurrentLength(len)?;
        let s = MFCreateSample()?;
        s.AddBuffer(&b)?;
        s.SetSampleTime(p.pts)?;
        s.SetSampleDuration(10_000_000 / self.fps as i64)?;
        if p.key {
            s.SetUINT32(&MFSampleExtension_CleanPoint, 1)?;
        }
        Ok(s)
    }

    /// Tira um quadro pronto. Devolve falso quando o decodificador quer mais entrada.
    unsafe fn pull(&mut self, out: &mut Vec<Decoded>) -> Result<bool> {
        let mut buf = [MFT_OUTPUT_DATA_BUFFER::default()];
        if !self.provides {
            let s = MFCreateSample()?;
            s.AddBuffer(&MFCreateMemoryBuffer(self.size.max(1))?)?;
            buf[0].pSample = std::mem::ManuallyDrop::new(Some(s));
        }
        let mut status = 0;
        let r = self.mft.ProcessOutput(0, &mut buf, &mut status);
        let sample = std::mem::ManuallyDrop::take(&mut buf[0].pSample);
        let _ = std::mem::ManuallyDrop::into_inner(std::mem::replace(&mut buf[0].pEvents, std::mem::ManuallyDrop::new(None)));
        match r {
            Ok(()) => {
                if let Some(s) = sample {
                    out.push(Decoded { pts: s.GetSampleTime()?, sample: s });
                }
                Ok(true)
            }
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => Ok(false),
            Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                self.pick_output()?;
                Ok(true)
            }
            Err(e) => Err(e),
        }
    }

    /// Entrega um pacote (ou só trata o que estiver pendente) e diz quais quadros saíram.
    pub unsafe fn decode(&mut self, p: Option<&Pkt>, out: &mut Vec<Decoded>) -> Result<()> {
        if self.events.is_none() {
            if let Some(p) = p {
                let s = self.sample(p)?;
                let mut tries = 0;
                loop {
                    match self.mft.ProcessInput(0, &s, 0) {
                        Ok(()) => break,
                        Err(e) if e.code() == MF_E_NOTACCEPTING && tries < 4 => {
                            tries += 1;
                            while self.pull(out)? {}
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
            while self.pull(out)? {}
            return Ok(());
        }
        if let Some(p) = p {
            self.queue.push_back(p.clone());
        }
        loop {
            while self.need > 0 {
                let Some(p) = self.queue.pop_front() else { break };
                let s = self.sample(&p)?;
                self.mft.ProcessInput(0, &s, 0)?;
                self.need -= 1;
            }
            let ev = match self.events.as_ref().unwrap().GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                Ok(e) => e,
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => return Ok(()),
                Err(e) => return Err(e),
            };
            match ev.GetType()? {
                t if t == METransformNeedInput.0 as u32 => self.need += 1,
                t if t == METransformHaveOutput.0 as u32 => {
                    self.pull(out)?;
                }
                _ => {}
            }
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            if let Ok(s) = self.mft.cast::<IMFShutdown>() {
                let _ = s.Shutdown();
            }
        }
    }
}
