//! Volume por programa com as sessões de áudio do Windows.

use std::collections::HashMap;

use windows::core::*;
use windows::Win32::Foundation::S_OK;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

use super::procs;

pub struct Session {
    pub pid: u32,
    pub exe: String,
    pub name: String,
    pub volume: f32,
    pub muted: bool,
    pub active: bool,
    ctl: ISimpleAudioVolume,
}

impl Session {
    pub fn set_volume(&self, v: f32) {
        unsafe {
            let _ = self.ctl.SetMasterVolume(v.clamp(0.0, 1.0), std::ptr::null());
        }
    }
    pub fn set_mute(&self, m: bool) {
        unsafe {
            let _ = self.ctl.SetMute(m, std::ptr::null());
        }
    }
}

/// Todas as sessões de todas as saídas ativas. Sessões do mesmo programa vêm separadas.
pub fn sessions() -> Vec<Session> {
    let mut out = Vec::new();
    unsafe {
        let Ok(en): Result<IMMDeviceEnumerator> =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
        else {
            return out;
        };
        let Ok(devs) = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else { return out };
        let n = devs.GetCount().unwrap_or(0);
        for i in 0..n {
            let Ok(dev) = devs.Item(i) else { continue };
            let Ok(mgr) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
            let Ok(list) = mgr.GetSessionEnumerator() else { continue };
            for j in 0..list.GetCount().unwrap_or(0) {
                let Ok(ctl) = list.GetSession(j) else { continue };
                let Ok(ctl2) = ctl.cast::<IAudioSessionControl2>() else { continue };
                if ctl2.IsSystemSoundsSession() == S_OK {
                    continue;
                }
                let pid = ctl2.GetProcessId().unwrap_or(0);
                if pid == 0 {
                    continue;
                }
                let Ok(vol) = ctl.cast::<ISimpleAudioVolume>() else { continue };
                let state = ctl.GetState().unwrap_or(AudioSessionStateInactive);
                if state == AudioSessionStateExpired {
                    continue;
                }
                let exe = procs::image_path(pid).map(|p| procs::exe_name(&p)).unwrap_or_default();
                let name = exe.trim_end_matches(".exe").trim_end_matches(".EXE").to_string();
                out.push(Session {
                    pid,
                    exe,
                    name,
                    volume: vol.GetMasterVolume().unwrap_or(1.0),
                    muted: vol.GetMute().map(|b| b.as_bool()).unwrap_or(false),
                    active: state == AudioSessionStateActive,
                    ctl: vol,
                });
            }
        }
    }
    out
}

/// Sessões do programa em foco: o próprio processo, seus filhos ou o mesmo executável.
pub fn for_process(pid: u32) -> Vec<Session> {
    let exe = procs::image_path(pid).map(|p| procs::exe_name(&p)).unwrap_or_default();
    let parents: HashMap<u32, u32> = procs::parent_map();
    sessions()
        .into_iter()
        .filter(|s| {
            s.pid == pid
                || procs::is_descendant(&parents, s.pid, pid)
                || (!exe.is_empty() && s.exe.eq_ignore_ascii_case(&exe))
        })
        .collect()
}

pub enum Change {
    Step(i32),
    ToggleMute,
}

/// Aplica a mudança e devolve (volume, mudo) para o aviso na tela.
pub fn change_focused(pid: u32, ch: Change) -> Option<(f32, bool)> {
    let list = for_process(pid);
    let first = list.first()?;
    let mut vol = first.volume;
    let mut mute = list.iter().all(|s| s.muted);
    match ch {
        Change::Step(step) => {
            vol = (((vol * 100.0).round() + step as f32) / 100.0).clamp(0.0, 1.0);
            mute = false;
            for s in &list {
                s.set_volume(vol);
                s.set_mute(false);
            }
        }
        Change::ToggleMute => {
            mute = !mute;
            for s in &list {
                s.set_mute(mute);
            }
        }
    }
    Some((vol, mute))
}
