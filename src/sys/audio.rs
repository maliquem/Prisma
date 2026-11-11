//! Volume por programa com as sessões de áudio do Windows.

use std::collections::HashMap;

use windows::core::*;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
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
                // O fluxo que mantém o som acordado não é um programa com som.
                if pid == 0 || pid == std::process::id() {
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

pub struct Device {
    pub id: String,
    pub name: String,
}

/// Aparelhos ligados agora, de saída ou de entrada.
pub fn devices(capture: bool) -> Vec<Device> {
    let flow = if capture { eCapture } else { eRender };
    let mut out = Vec::new();
    unsafe {
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return out };
        let Ok(list) = en.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) else { return out };
        for i in 0..list.GetCount().unwrap_or(0) {
            let Ok(d) = list.Item(i) else { continue };
            let Ok(raw) = d.GetId() else { continue };
            let id = raw.to_string().unwrap_or_default();
            CoTaskMemFree(Some(raw.0 as *const _));
            let name = d
                .OpenPropertyStore(STGM_READ)
                .and_then(|s| s.GetValue(&PKEY_Device_FriendlyName))
                .map(|v| v.to_string())
                .unwrap_or_else(|_| id.clone());
            out.push(Device { id, name });
        }
    }
    out
}

// O Windows não publica esta interface: é a mesma que a tela Volume e preferências de
// dispositivo usa. Só importam os métodos depois dos 19 reservados, na ordem abaixo.
const POLICY_IIDS: [GUID; 2] = [
    GUID::from_u128(0xab3d4648_e242_459f_b02f_541c70306324),
    GUID::from_u128(0x2a59116d_6c4f_45e0_a74f_707e3fef9258),
];
const SET_ENDPOINT: usize = 25;
const GET_ENDPOINT: usize = 26;

type FactoryFn = unsafe extern "system" fn(*mut core::ffi::c_void, *const GUID, *mut *mut core::ffi::c_void) -> HRESULT;
type SetFn = unsafe extern "system" fn(*mut core::ffi::c_void, u32, i32, i32, *mut core::ffi::c_void) -> HRESULT;
type GetFn = unsafe extern "system" fn(*mut core::ffi::c_void, u32, i32, i32, *mut *mut core::ffi::c_void) -> HRESULT;

fn policy() -> Option<*mut core::ffi::c_void> {
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    let class = HSTRING::from("Windows.Media.Internal.AudioPolicyConfig");
    let get: FactoryFn = unsafe { std::mem::transmute(GetProcAddress(LoadLibraryW(w!("combase.dll")).ok()?, s!("RoGetActivationFactory"))?) };
    for iid in &POLICY_IIDS {
        let mut out = std::ptr::null_mut();
        let hr = unsafe { get(std::mem::transmute_copy(&class), iid, &mut out) };
        if hr.is_ok() && !out.is_null() {
            return Some(out);
        }
    }
    None
}

unsafe fn method(this: *mut core::ffi::c_void, index: usize) -> *const core::ffi::c_void {
    let table = *(this as *const *const *const core::ffi::c_void);
    *table.add(index)
}

unsafe fn release(this: *mut core::ffi::c_void) {
    let f: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32 = std::mem::transmute(method(this, 2));
    f(this);
}

/// A saída padrão do Windows e o formato em que ela está agora.
#[derive(Clone, Default, Debug)]
pub struct Output {
    pub name: String,
    pub channels: u16,
    pub rate: u32,
    pub spatial_support: bool,
    /// Nome do formato de som espacial ativo. Vazio: desligado.
    pub spatial: String,
}

fn spatial_name(guid: &str) -> String {
    match guid.trim_matches(|c| c == '{' || c == '}').to_ascii_uppercase().as_str() {
        "" | "00000000-0000-0000-0000-000000000000" => "",
        "A289735D-FA3E-4E35-9D7D-B6F896ACB2E7" => "Dolby Atmos for Home Theater",
        "1459AC38-3875-49BF-BB59-0FE80F4D395D" => "Dolby Atmos for Headphones",
        "4C81E564-C8EF-4AD9-9F2C-9EF995533790" => "Dolby Atmos for Speakers",
        "B53D940C-B846-4831-9F76-D102B9B725A0" => "Windows Sonic",
        "10201B4A-3322-4967-BF40-2CAA9BAFCA44" => "DTS:X",
        _ => "Outro formato",
    }
    .to_string()
}

pub fn default_output() -> Option<Output> {
    use windows::Media::Audio::SpatialAudioDeviceConfiguration;
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let dev = en.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        let raw = dev.GetId().ok()?;
        let id = raw.to_string().unwrap_or_default();
        CoTaskMemFree(Some(raw.0 as *const _));
        let mut out = Output::default();
        if let Ok(store) = dev.OpenPropertyStore(STGM_READ) {
            if let Ok(v) = store.GetValue(&PKEY_Device_FriendlyName) {
                out.name = v.to_string();
            }
        }
        if let Ok(client) = dev.Activate::<IAudioClient>(CLSCTX_ALL, None) {
            if let Ok(f) = client.GetMixFormat() {
                out.channels = (*f).nChannels;
                out.rate = (*f).nSamplesPerSec;
                CoTaskMemFree(Some(f as *const _));
            }
        }
        if let Ok(c) = SpatialAudioDeviceConfiguration::GetForDeviceId(&HSTRING::from(endpoint_path(&id, false))) {
            out.spatial_support = c.IsSpatialAudioSupported().unwrap_or(false);
            out.spatial = spatial_name(&c.ActiveSpatialAudioFormat().unwrap_or_default().to_string());
        }
        Some(out)
    }
}

fn endpoint_path(id: &str, capture: bool) -> String {
    let kind = if capture { "{2eef81be-33fa-4800-9670-1cd474972c3f}" } else { "{e6327cad-dcec-4949-ae8a-991e976a79d2}" };
    format!("\\\\?\\SWD#MMDEVAPI#{id}#{kind}")
}

/// Escolhe o aparelho de um processo. Sem id, volta ao padrão do Windows.
pub fn set_route(pid: u32, capture: bool, id: &str) -> bool {
    let Some(p) = policy() else { return false };
    let flow = if capture { eCapture } else { eRender };
    let path = if id.is_empty() { HSTRING::new() } else { HSTRING::from(endpoint_path(id, capture)) };
    let mut ok = true;
    unsafe {
        let f: SetFn = std::mem::transmute(method(p, SET_ENDPOINT));
        for role in [eConsole, eMultimedia, eCommunications] {
            ok &= f(p, pid, flow.0, role.0, std::mem::transmute_copy(&path)).is_ok();
        }
        release(p);
    }
    ok
}

/// Teste: lê o que o Windows guardou para o processo.
#[cfg(debug_assertions)]
pub fn route_of(pid: u32, capture: bool) -> Option<String> {
    let p = policy()?;
    let flow = if capture { eCapture } else { eRender };
    unsafe {
        let f: GetFn = std::mem::transmute(method(p, GET_ENDPOINT));
        let mut out = std::ptr::null_mut();
        let hr = f(p, pid, flow.0, eMultimedia.0, &mut out);
        release(p);
        if hr.is_err() {
            return None;
        }
        let h: HSTRING = std::mem::transmute(out);
        Some(h.to_string())
    }
}

// Aparelho padrão do Windows: a interface que a tela Som do Painel de Controle usa, também
// sem documentação. `SetDefaultEndpoint` é o 14º método (3 do IUnknown e 10 antes dele).
const POLICY_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
const POLICY_CONFIG: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
const SET_DEFAULT: usize = 13;

/// Torna `id` o aparelho padrão do Windows (saída ou entrada, para os três usos).
pub fn set_default(id: &str) -> bool {
    if id.is_empty() {
        return false;
    }
    unsafe {
        let Ok(unk) = CoCreateInstance::<_, IUnknown>(&POLICY_CLIENT, None, CLSCTX_ALL) else { return false };
        let mut p = std::ptr::null_mut();
        if unk.query(&POLICY_CONFIG, &mut p).is_err() || p.is_null() {
            return false;
        }
        let f: unsafe extern "system" fn(*mut core::ffi::c_void, PCWSTR, i32) -> HRESULT = std::mem::transmute(method(p, SET_DEFAULT));
        let wide = HSTRING::from(id);
        let mut ok = true;
        for role in [eConsole, eMultimedia, eCommunications] {
            ok &= f(p, PCWSTR(wide.as_ptr()), role.0).is_ok();
        }
        release(p);
        ok
    }
}
