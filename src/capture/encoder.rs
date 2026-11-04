//! Codificador de vídeo da GPU pelo Media Foundation, sem biblioteca externa.

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Variant::VARIANT;

use super::mux::{Codec, Pkt};

pub struct Encoder {
    mft: IMFTransform,
    events: IMFMediaEventGenerator,
    api: Option<ICodecAPI>,
    pub need: u32,
    fps: u32,
}

fn packed(a: u32, b: u32) -> u64 {
    (a as u64) << 32 | b as u64
}

fn subtype(c: Codec) -> GUID {
    match c {
        Codec::Av1 => MFVideoFormat_AV1,
        Codec::Hevc => MFVideoFormat_HEVC,
        Codec::H264 => MFVideoFormat_H264,
    }
}

unsafe fn find(c: Codec, input: GUID) -> Result<IMFTransform> {
    let i = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: input };
    let o = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: subtype(c) };
    let mut list = std::ptr::null_mut();
    let mut n = 0u32;
    MFTEnumEx(
        MFT_CATEGORY_VIDEO_ENCODER,
        MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
        Some(&i),
        Some(&o),
        &mut list,
        &mut n,
    )?;
    if n == 0 || list.is_null() {
        return Err(Error::new(E_FAIL, "sem codificador de hardware"));
    }
    let items = std::slice::from_raw_parts(list, n as usize);
    let r = items[0].as_ref().unwrap().ActivateObject::<IMFTransform>();
    CoTaskMemFree(Some(list as *const _));
    r
}

impl Encoder {
    pub fn new(dev: &ID3D11Device, codec: Codec, w: u32, h: u32, fps: u32, bitrate: u32, ten_bit: bool) -> Result<Self> {
        unsafe {
            let input = if ten_bit { MFVideoFormat_P010 } else { MFVideoFormat_NV12 };
            let mft = find(codec, input)?;
            let attrs = mft.GetAttributes()?;
            attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
            let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);

            let mut token = 0;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.unwrap();
            manager.ResetDevice(dev, token)?;
            mft.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)?;

            let out = MFCreateMediaType()?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out.SetGUID(&MF_MT_SUBTYPE, &subtype(codec))?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
            out.SetUINT64(&MF_MT_FRAME_SIZE, packed(w, h))?;
            out.SetUINT64(&MF_MT_FRAME_RATE, packed(fps, 1))?;
            out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, packed(1, 1))?;
            out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            if ten_bit {
                // Perfil de 10 bits e rótulos de HDR10.
                out.SetUINT32(&MF_MT_VIDEO_PROFILE, 2)?;
                out.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT2020.0 as u32)?;
                out.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_2084.0 as u32)?;
                out.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT2020_10.0 as u32)?;
            }
            mft.SetOutputType(0, &out, 0)?;

            let mut i = 0;
            let mut set = false;
            while let Ok(t) = mft.GetInputAvailableType(0, i) {
                if t.GetGUID(&MF_MT_SUBTYPE)? == input {
                    t.SetUINT64(&MF_MT_FRAME_SIZE, packed(w, h))?;
                    t.SetUINT64(&MF_MT_FRAME_RATE, packed(fps, 1))?;
                    t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, packed(1, 1))?;
                    t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
                    mft.SetInputType(0, &t, 0)?;
                    set = true;
                    break;
                }
                i += 1;
            }
            if !set {
                return Err(Error::new(E_FAIL, "formato de entrada recusado"));
            }

            let api: Option<ICodecAPI> = mft.cast().ok();
            if let Some(a) = &api {
                let _ = a.SetValue(&CODECAPI_AVEncCommonRateControlMode, &VARIANT::from(eAVEncCommonRateControlMode_UnconstrainedVBR.0 as u32));
                let _ = a.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &VARIANT::from(bitrate));
                let _ = a.SetValue(&CODECAPI_AVEncCommonLowLatency, &VARIANT::from(true));
                let _ = a.SetValue(&CODECAPI_AVEncMPVGOPSize, &VARIANT::from(fps * 2));
            }
            mft.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            let events: IMFMediaEventGenerator = mft.cast()?;
            Ok(Self { mft, events, api, need: 0, fps })
        }
    }

    pub fn force_key(&self) {
        if let Some(a) = &self.api {
            unsafe {
                let _ = a.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &VARIANT::from(1u32));
            }
        }
    }

    /// Trata os eventos pendentes e entrega o que saiu do codificador.
    pub fn pump(&mut self, out: &mut Vec<Pkt>) -> Result<()> {
        unsafe {
            loop {
                let ev = match self.events.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                    Ok(e) => e,
                    Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => return Ok(()),
                    Err(e) => return Err(e),
                };
                match ev.GetType()? {
                    t if t == METransformNeedInput.0 as u32 => self.need += 1,
                    t if t == METransformHaveOutput.0 as u32 => self.take(out)?,
                    _ => {}
                }
            }
        }
    }

    unsafe fn take(&mut self, out: &mut Vec<Pkt>) -> Result<()> {
        let mut buf = [MFT_OUTPUT_DATA_BUFFER::default()];
        let mut status = 0;
        let r = self.mft.ProcessOutput(0, &mut buf, &mut status);
        let sample = std::mem::ManuallyDrop::take(&mut buf[0].pSample);
        let _ = std::mem::ManuallyDrop::into_inner(std::mem::replace(&mut buf[0].pEvents, std::mem::ManuallyDrop::new(None)));
        if let Err(e) = r {
            if e.code() == MF_E_TRANSFORM_STREAM_CHANGE {
                if let Ok(t) = self.mft.GetOutputAvailableType(0, 0) {
                    self.mft.SetOutputType(0, &t, 0)?;
                }
                return Ok(());
            }
            return Err(e);
        }
        let Some(s) = sample else { return Ok(()) };
        let mb = s.ConvertToContiguousBuffer()?;
        let mut ptr = std::ptr::null_mut();
        let mut len = 0;
        mb.Lock(&mut ptr, None, Some(&mut len))?;
        let data = std::slice::from_raw_parts(ptr, len as usize).to_vec();
        mb.Unlock()?;
        let key = s.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0;
        out.push(Pkt { data, pts: s.GetSampleTime()?, key });
        Ok(())
    }

    /// Entrega uma textura. Chame só com `need` maior que zero.
    pub fn feed(&mut self, tex: &ID3D11Texture2D, pts: i64) -> Result<()> {
        unsafe {
            let buf = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, tex, 0, false)?;
            let b2: IMF2DBuffer = buf.cast()?;
            buf.SetCurrentLength(b2.GetContiguousLength()?)?;
            let s = MFCreateSample()?;
            s.AddBuffer(&buf)?;
            s.SetSampleTime(pts)?;
            s.SetSampleDuration(10_000_000 / self.fps as i64)?;
            self.mft.ProcessInput(0, &s, 0)?;
            self.need -= 1;
            Ok(())
        }
    }

    pub fn drain(&mut self, out: &mut Vec<Pkt>) {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0);
            for _ in 0..50 {
                match self.events.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                    Ok(ev) => match ev.GetType() {
                        Ok(t) if t == METransformHaveOutput.0 as u32 => {
                            let _ = self.take(out);
                        }
                        Ok(t) if t == METransformDrainComplete.0 as u32 => break,
                        _ => {}
                    },
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(4)),
                }
            }
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            if let Ok(s) = self.mft.cast::<IMFShutdown>() {
                let _ = s.Shutdown();
            }
        }
    }
}
