//! Quem assiste: recebe o vídeo e o som, decodifica na GPU e mostra numa janela.
//! Com a permissão, o mouse e o teclado da janela vão para o host, e os controles daqui viram
//! controles virtuais lá (só com a janela em primeiro plano, a não ser pela opção).
//! As opções vêm de `cliente.txt` (página Compartilhar tela, aba Cliente); as variáveis
//! `PRISMA_PAD_BACKGROUND`, `PRISMA_PAD_KEYS`, `PRISMA_MUTE` e `PRISMA_444` ainda valem por cima.
//! Atalhos padrão: Ctrl+Alt+Shift+F tela cheia (F11 também, sem a permissão de teclado),
//! Ctrl+Alt+Shift+P o menu da conexão, Ctrl+Alt+End o Ctrl+Alt+Del. Fechar a janela avisa o host.

use std::collections::VecDeque;
use std::net::{ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicIsize, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Media::timeBeginPeriod;
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Input::XboxController::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::*;
use crate::capture::decoder::{can_decode, Decoded, Decoder};
use crate::capture::mux::{Codec, Pkt};
use crate::capture::{d3d, sound, D3d};

struct Frame {
    key: bool,
    pts: i64,
    data: Vec<u8>,
}

enum Event {
    Info { codec: Codec, w: u32, h: u32, fps: u32, perms: u8, hdr10: bool, yuv444: bool },
    Frame(Frame),
}

/// O que o host disse de um controle daqui: jogador (0 sem vaga), vaga do XInput lá e motivo.
#[derive(Clone, Copy, PartialEq)]
struct PadView {
    player: u8,
    slot: u8,
    why: u8,
}

const NO_PAD: PadView = PadView { player: 0, slot: 255, why: PAD_OK };

/// O que a linha de recepção divide com a janela.
struct Shared {
    queue: Mutex<VecDeque<Event>>,
    wake: isize,
    quit: AtomicBool,
    /// O canal fechou do outro lado (só na internet).
    closed: AtomicBool,
    /// O decodificador falhou: espera um quadro-chave.
    want_key: AtomicBool,
    /// Quadros perdidos que o host consertou sem quadro-chave.
    recovered: AtomicU32,
    got_info: AtomicBool,
    /// Ida e volta da rede, em 100 ns, da melhor medida recente.
    rtt: AtomicI64,
    /// Relógio do host menos o daqui, em 100 ns.
    offset: AtomicI64,
    lost: AtomicU32,
    bytes: AtomicU64,
    /// Som recebido esperando para tocar, em amostras de 16 bits intercaladas.
    audio: Mutex<VecDeque<i16>>,
    mute: bool,
    /// Calado pelo menu ou pela opção: o som chega e é jogado fora.
    silenced: AtomicBool,
    audio_lost: AtomicU32,
    audio_blocks: AtomicU32,
    /// Vezes que o som acabou antes do próximo bloco chegar.
    underruns: AtomicU32,
    /// Som no buffer na hora de tocar, em quadros, para saber a latência do som.
    audio_fill: AtomicU32,
    /// Permissões que o host deu, do último Info.
    perms: AtomicU8,
    pads: Mutex<[PadView; PADS]>,
}

/// Junta os pedaços de um quadro.
struct Asm {
    frame: u32,
    count: u16,
    got: u16,
    key: bool,
    pts: i64,
    parts: Vec<Option<Vec<u8>>>,
}

fn newer(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

fn receive(link: Arc<dyn Link>, hello: Vec<u8>, sh: Arc<Shared>) {
    let mut buf = vec![0u8; 2048];
    let mut out = Vec::with_capacity(64);
    let mut cur: Option<Asm> = None;
    let mut expect: Option<u32> = None;
    let mut waiting_key = true;
    let mut last_req = Instant::now() - Duration::from_secs(1);
    let mut last_ping = Instant::now() - Duration::from_secs(1);
    let mut last_hello = Instant::now() - Duration::from_secs(2);
    // Medidas recentes de ida e volta, com a diferença de relógio de cada uma.
    let mut samples: VecDeque<(i64, i64)> = VecDeque::new();
    let mut audio_next: Option<u32> = None;
    // O host se recupera de perda sem quadro-chave (NVENC direto).
    let mut recover = false;
    // Área de transferência, aberta quando o host dá a permissão.
    let mut clip: Option<super::clip::Clip> = None;
    let mut clip_in = super::clip::Pieces::default();
    let mut clip_id = 0u16;

    let push = |e: Event| {
        sh.queue.lock().unwrap().push_back(e);
        unsafe {
            let _ = SetEvent(HANDLE(sh.wake as *mut _));
        }
    };

    while !sh.quit.load(Ordering::Relaxed) {
        if !sh.got_info.load(Ordering::Relaxed) && last_hello.elapsed() > Duration::from_secs(1) {
            link.send(&hello);
            last_hello = Instant::now();
        }
        if last_ping.elapsed() > Duration::from_millis(500) {
            write(&mut out, &Msg::Ping { t: now() });
            link.send(&out);
            last_ping = Instant::now();
        }
        if sh.want_key.swap(false, Ordering::Relaxed) {
            waiting_key = true;
        }
        if clip.is_none() && sh.perms.load(Ordering::Relaxed) & COPY != 0 {
            clip = Some(super::clip::start());
        }
        if let Some(c) = &clip {
            while let Ok(t) = c.changed.try_recv() {
                clip_id = clip_id.wrapping_add(1);
                let pk = super::clip::packets(&t, clip_id);
                for p in pk.iter().chain(pk.iter()) {
                    link.send(p);
                }
            }
        }
        if waiting_key && sh.got_info.load(Ordering::Relaxed) && last_req.elapsed() > Duration::from_millis(150) {
            write(&mut out, &Msg::KeyReq);
            link.send(&out);
            last_req = Instant::now();
        }
        let n = match link.recv(&mut buf, Duration::from_millis(20)) {
            Poll::Got(n) => n,
            Poll::Closed => {
                sh.closed.store(true, Ordering::Relaxed);
                unsafe {
                    let _ = SetEvent(HANDLE(sh.wake as *mut _));
                }
                break;
            }
            _ => continue,
        };
        sh.bytes.fetch_add(n as u64, Ordering::Relaxed);
        match read(&buf[..n]) {
            // O host desconectou esta pessoa (pela página dele, ou banida).
            Some(Msg::Bye) => {
                eprintln!("o outro PC encerrou a conexão");
                sh.closed.store(true, Ordering::Relaxed);
                unsafe {
                    let _ = SetEvent(HANDLE(sh.wake as *mut _));
                }
                break;
            }
            Some(Msg::Info { codec, w, h, fps, perms, recover: r, hdr10, yuv444 }) => {
                recover = r;
                sh.got_info.store(true, Ordering::Relaxed);
                sh.perms.store(perms, Ordering::Relaxed);
                push(Event::Info { codec, w, h, fps, perms, hdr10, yuv444 });
            }
            Some(Msg::Clip { id, part, parts, data }) => {
                if let (Some(c), Some(t)) = (&clip, clip_in.add(id, part, parts, data)) {
                    let _ = c.set.send(t);
                }
            }
            Some(Msg::Rumble { pad, low, high }) => {
                if pad < 4 {
                    let v = XINPUT_VIBRATION { wLeftMotorSpeed: low as u16 * 257, wRightMotorSpeed: high as u16 * 257 };
                    unsafe {
                        XInputSetState(pad as u32, &v);
                    }
                }
            }
            Some(Msg::PadInfo { pad, player, slot, why }) => {
                if let Some(p) = sh.pads.lock().unwrap().get_mut(pad as usize) {
                    let new = PadView { player, slot, why };
                    if *p != new {
                        match player {
                            0 => eprintln!("controle {}: {}", pad + 1, pad_why(why)),
                            n => eprintln!("controle {}: jogador {n} no host", pad + 1),
                        }
                        *p = new;
                    }
                }
            }
            Some(Msg::Audio { seq, data, .. }) => {
                sh.audio_blocks.fetch_add(1, Ordering::Relaxed);
                if let Some(n) = audio_next {
                    let gap = seq.wrapping_sub(n);
                    if gap > 0 && gap < 1000 {
                        sh.audio_lost.fetch_add(gap, Ordering::Relaxed);
                    }
                }
                audio_next = Some(seq.wrapping_add(1));
                if !sh.mute {
                    let mut q = sh.audio.lock().unwrap();
                    q.extend(data.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
                }
            }
            Some(Msg::Pong { t, host }) => {
                let back = now();
                let rtt = back - t;
                samples.push_back((rtt, host - (t + back) / 2));
                if samples.len() > 20 {
                    samples.pop_front();
                }
                if let Some(&(r, o)) = samples.iter().min_by_key(|s| s.0) {
                    sh.rtt.store(r, Ordering::Relaxed);
                    sh.offset.store(o, Ordering::Relaxed);
                }
            }
            Some(Msg::Video { key, idx, count, frame, pts, data }) => {
                if count == 0 || idx >= count || expect.is_some_and(|e| newer(e, frame)) {
                    continue;
                }
                if let Some(a) = &cur {
                    if a.frame != frame {
                        if !newer(frame, a.frame) {
                            continue;
                        }
                        // Chegou pedaço de um quadro mais novo com este incompleto: perdeu-se algo.
                        sh.lost.fetch_add(1, Ordering::Relaxed);
                        if recover && !waiting_key {
                            write(&mut out, &Msg::Nack { frame: a.frame });
                            link.send(&out);
                            sh.recovered.fetch_add(1, Ordering::Relaxed);
                        } else {
                            waiting_key = true;
                        }
                        cur = None;
                    }
                }
                let a = cur.get_or_insert_with(|| Asm { frame, count, got: 0, key, pts, parts: vec![None; count as usize] });
                if a.count != count {
                    continue;
                }
                let slot = &mut a.parts[idx as usize];
                if slot.is_none() {
                    *slot = Some(data.to_vec());
                    a.got += 1;
                }
                if a.got == a.count {
                    let a = cur.take().unwrap();
                    if let Some(e) = expect.filter(|&e| e != a.frame) {
                        // Um quadro inteiro sumiu no caminho.
                        sh.lost.fetch_add(1, Ordering::Relaxed);
                        if recover && !waiting_key {
                            write(&mut out, &Msg::Nack { frame: e });
                            link.send(&out);
                            sh.recovered.fetch_add(1, Ordering::Relaxed);
                        } else {
                            waiting_key = true;
                        }
                    }
                    expect = Some(a.frame.wrapping_add(1));
                    if a.key {
                        waiting_key = false;
                    }
                    if !waiting_key {
                        let data = a.parts.into_iter().flatten().flatten().collect();
                        push(Event::Frame(Frame { key: a.key, pts: a.pts, data }));
                    }
                }
            }
            _ => {}
        }
    }
}

/// Som que fica esperando antes de tocar: 15 ms. Pouco o bastante para não atrasar,
/// e o bastante para a rede variar um pouco sem o som falhar.
const AUDIO_TARGET: usize = 720;

/// Toca o som recebido na saída padrão, com o menor período que o Windows deixa.
fn play(sh: Arc<Shared>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        if let Err(e) = play_loop(&sh) {
            eprintln!("som: {e:?}");
        }
    }
}

unsafe fn play_loop(sh: &Shared) -> Result<()> {
    let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let dev = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
    let f = sound::format();
    let mut client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
    let mut low = false;
    if let Ok(c3) = client.cast::<IAudioClient3>() {
        let (mut def, mut fund, mut min, mut max) = (0, 0, 0, 0);
        if c3.GetSharedModeEnginePeriod(&f.Format, &mut def, &mut fund, &mut min, &mut max).is_ok() {
            low = c3.InitializeSharedAudioStream(AUDCLNT_STREAMFLAGS_EVENTCALLBACK, min, &f.Format, None).is_ok();
        }
    }
    if !low {
        client = dev.Activate(CLSCTX_ALL, None)?;
        let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 100_000, 0, &f.Format, None)?;
    }
    let ev = CreateEventW(None, false, false, None)?;
    client.SetEventHandle(ev)?;
    let size = client.GetBufferSize()?;
    let render: IAudioRenderClient = client.GetService()?;
    client.Start()?;
    let mut priming = true;
    while !sh.quit.load(Ordering::Relaxed) {
        let _ = WaitForSingleObject(ev, 100);
        let pad = client.GetCurrentPadding()?;
        let free = size.saturating_sub(pad);
        if free == 0 {
            continue;
        }
        let ptr = render.GetBuffer(free)? as *mut f32;
        let out = std::slice::from_raw_parts_mut(ptr, free as usize * 2);
        let mut q = sh.audio.lock().unwrap();
        let have = q.len() / 2;
        sh.audio_fill.store((have as u32).wrapping_add(pad), Ordering::Relaxed);
        if priming && have >= AUDIO_TARGET {
            priming = false;
        }
        // Atrasou demais (o relógio do outro PC anda um pouco mais rápido): descarta o excesso.
        if have > AUDIO_TARGET + 2400 {
            q.drain(..(have - AUDIO_TARGET) * 2);
        }
        if sh.silenced.load(Ordering::Relaxed) {
            q.clear();
            priming = true;
        }
        let mut i = 0;
        if !priming {
            while i < out.len() {
                let Some(v) = q.pop_front() else { break };
                out[i] = v as f32 / 32768.0;
                i += 1;
            }
            if i < out.len() {
                priming = true;
                sh.underruns.fetch_add(1, Ordering::Relaxed);
            }
        }
        drop(q);
        out[i..].fill(0.0);
        render.ReleaseBuffer(free, 0)?;
    }
    let _ = client.Stop();
    let _ = CloseHandle(ev);
    Ok(())
}

/// Mouse e teclado da janela, guardados pela função da janela até a próxima volta.
struct Keys {
    keyboard: bool,
    mouse: bool,
    /// O teclado vira controle: as teclas do mapa não vão como teclado.
    pad_keys: bool,
    video: (u32, u32),
    pending: Vec<Input>,
    held: Vec<(u16, bool)>,
    buttons: [bool; 5],
    /// Ctrl+Alt+End aqui pede Ctrl+Alt+Del lá (o Windows não deixa a janela ver o Ctrl+Alt+Del).
    sas: bool,
    /// O atalho do menu foi apertado.
    menu: bool,
    key_menu: opts::Keys,
    key_full: opts::Keys,
    key_cad: opts::Keys,
    key_disc: opts::Keys,
    key_imm: opts::Keys,
    key_meter: opts::Keys,
    key_sound: opts::Keys,
    /// Atalhos apertados que a volta da janela ainda vai fazer.
    acts: Vec<hud::Pick>,
    /// Modo imersivo: bit 1 o mouse fica na janela, bit 2 o Alt+Tab e a tecla Windows vão ao host.
    immersive: u8,
}

static KEYS: Mutex<Keys> = Mutex::new(Keys {
    keyboard: false,
    mouse: false,
    pad_keys: false,
    video: (0, 0),
    pending: Vec::new(),
    held: Vec::new(),
    buttons: [false; 5],
    sas: false,
    menu: false,
    key_menu: (0, 0),
    key_full: (0, 0),
    key_cad: (0, 0),
    key_disc: (0, 0),
    key_imm: (0, 0),
    key_meter: (0, 0),
    key_sound: (0, 0),
    acts: Vec::new(),
    immersive: 0,
});

/// A janela de quem assiste, para o gancho do teclado.
static VIEW_HWND: AtomicIsize = AtomicIsize::new(0);

/// Modo imersivo: com a janela em primeiro plano, a tecla Windows, o Alt+Tab, o Alt+Esc e o
/// Ctrl+Esc vão para o host em vez de agir aqui. O Ctrl+Alt+Del o Windows não deixa pegar.
unsafe extern "system" fn immersive_hook(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code >= 0 {
        let k = &*(l.0 as *const KBDLLHOOKSTRUCT);
        let mine = GetForegroundWindow().0 as isize == VIEW_HWND.load(Ordering::Relaxed);
        let alt = k.flags.0 & LLKHF_ALTDOWN.0 != 0;
        let ctrl = GetAsyncKeyState(VK_CONTROL.0 as i32) < 0;
        let vk = VIRTUAL_KEY(k.vkCode as u16);
        let grab = matches!(vk, VK_LWIN | VK_RWIN) || (vk == VK_TAB && alt) || (vk == VK_ESCAPE && (alt || ctrl));
        if mine && grab && k.flags.0 & LLKHF_INJECTED.0 == 0 {
            if let Ok(mut keys) = KEYS.try_lock() {
                if keys.keyboard && keys.immersive & 2 != 0 {
                    let down = !matches!(w.0 as u32, WM_KEYUP | WM_SYSKEYUP);
                    let (scan, extended) = (k.scanCode as u16, k.flags.0 & LLKHF_EXTENDED.0 != 0);
                    if down {
                        if !keys.held.contains(&(scan, extended)) {
                            keys.held.push((scan, extended));
                        }
                    } else {
                        keys.held.retain(|x| *x != (scan, extended));
                    }
                    keys.pending.push(Input::Key { scan, extended, down });
                    return LRESULT(1);
                }
            }
        }
    }
    CallNextHookEx(None, code, w, l)
}

/// Área da janela onde está a imagem, com as faixas pretas de fora.
fn video_rect(iw: u32, ih: u32, ow: u32, oh: u32) -> RECT {
    let scale = (ow as f64 / iw.max(1) as f64).min(oh as f64 / ih.max(1) as f64);
    let (dw, dh) = ((iw as f64 * scale).round() as i32, (ih as f64 * scale).round() as i32);
    let (x, y) = ((ow as i32 - dw) / 2, (oh as i32 - dh) / 2);
    RECT { left: x, top: y, right: x + dw, bottom: y + dh }
}

unsafe fn key_event(h: HWND, k: &mut Keys, m: u32, w: WPARAM, l: LPARAM) -> bool {
    let down = m == WM_KEYDOWN || m == WM_SYSKEYDOWN;
    let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
    let alt = GetKeyState(VK_MENU.0 as i32) < 0;
    let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
    let combo = (opts::mods_now(ctrl, alt, shift), w.0 as u32);
    if combo == k.key_full {
        if down {
            toggle_full(h);
        }
        return true;
    }
    if combo == k.key_menu {
        if down {
            k.menu = true;
        }
        return true;
    }
    // Os atalhos de quem assiste ficam aqui e não vão para o host.
    for (keys, act) in [(k.key_disc, hud::Pick::Disconnect), (k.key_imm, hud::Pick::ImmersiveToggle), (k.key_meter, hud::Pick::MeterCycle), (k.key_sound, hud::Pick::Sound)] {
        if keys.1 != 0 && combo == keys {
            if down && l.0 & (1 << 30) == 0 {
                k.acts.push(act);
            }
            return true;
        }
    }
    if k.keyboard && combo == k.key_cad {
        if down {
            k.sas = true;
        }
        return true;
    }
    let scan = ((l.0 >> 16) & 0xFF) as u16;
    let extended = (l.0 >> 24) & 1 != 0;
    if k.pad_keys && KEY_PAD.iter().any(|m| (m.0, m.1) == (scan, extended)) {
        return true;
    }
    if !k.keyboard {
        if down && w.0 as u16 == VK_F11.0 {
            toggle_full(h);
            return true;
        }
        return false;
    }
    if down {
        if !k.held.contains(&(scan, extended)) {
            k.held.push((scan, extended));
        }
    } else {
        k.held.retain(|x| *x != (scan, extended));
    }
    k.pending.push(Input::Key { scan, extended, down });
    true
}

/// Solta no host tudo que está apertado aqui, quando a janela perde o foco.
fn release_all(k: &mut Keys) {
    let held: Vec<(u16, bool)> = std::mem::take(&mut k.held);
    for (scan, extended) in held {
        k.pending.push(Input::Key { scan, extended, down: false });
    }
    for b in 0..5 {
        if std::mem::take(&mut k.buttons[b]) {
            k.pending.push(Input::Button { button: b as u8, down: false });
        }
    }
}

/// Converte o quadro decodificado (NV12, ou P010 no HDR10) para a tela e encaixa com faixas pretas.
struct Scaler {
    vd: ID3D11VideoDevice,
    vc: ID3D11VideoContext1,
    en: ID3D11VideoProcessorEnumerator,
    vp: ID3D11VideoProcessor,
    size: (u32, u32, u32, u32),
    hdr: bool,
}

impl Scaler {
    unsafe fn new(d: &D3d, iw: u32, ih: u32, ow: u32, oh: u32, fps: u32, hdr: bool) -> Result<Self> {
        let vd: ID3D11VideoDevice = d.dev.cast()?;
        let vc: ID3D11VideoContext1 = d.ctx.cast()?;
        let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL { Numerator: fps, Denominator: 1 },
            InputWidth: iw,
            InputHeight: ih,
            OutputFrameRate: DXGI_RATIONAL { Numerator: fps, Denominator: 1 },
            OutputWidth: ow,
            OutputHeight: oh,
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };
        let en = vd.CreateVideoProcessorEnumerator(&desc)?;
        let vp = vd.CreateVideoProcessor(&en, 0)?;
        // HDR10: o PQ e o BT.2020 passam direto para a janela, que também está em HDR10.
        let (cin, cout) = if hdr {
            (DXGI_COLOR_SPACE_YCBCR_STUDIO_G2084_LEFT_P2020, DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020)
        } else {
            (DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709)
        };
        vc.VideoProcessorSetStreamColorSpace1(&vp, 0, cin);
        vc.VideoProcessorSetOutputColorSpace1(&vp, cout);
        vc.VideoProcessorSetStreamFrameFormat(&vp, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
        vc.VideoProcessorSetStreamAutoProcessingMode(&vp, 0, false);
        let black = D3D11_VIDEO_COLOR { Anonymous: D3D11_VIDEO_COLOR_0 { RGBA: D3D11_VIDEO_COLOR_RGBA { R: 0.0, G: 0.0, B: 0.0, A: 1.0 } } };
        vc.VideoProcessorSetOutputBackgroundColor(&vp, false, &black);
        // Tamanho certo do vídeo: o decodificador pode entregar a textura com sobra.
        let src = RECT { left: 0, top: 0, right: iw as i32, bottom: ih as i32 };
        vc.VideoProcessorSetStreamSourceRect(&vp, 0, true, Some(&src));
        let scale = (ow as f64 / iw as f64).min(oh as f64 / ih as f64);
        let (dw, dh) = ((iw as f64 * scale).round() as i32, (ih as f64 * scale).round() as i32);
        let (x, y) = ((ow as i32 - dw) / 2, (oh as i32 - dh) / 2);
        vc.VideoProcessorSetStreamDestRect(&vp, 0, true, Some(&RECT { left: x, top: y, right: x + dw, bottom: y + dh }));
        vc.VideoProcessorSetOutputTargetRect(&vp, true, Some(&RECT { left: 0, top: 0, right: ow as i32, bottom: oh as i32 }));
        Ok(Self { vd, vc, en, vp, size: (iw, ih, ow, oh), hdr })
    }

    unsafe fn blt(&self, tex: &ID3D11Texture2D, slice: u32, target: &ID3D11Texture2D) -> Result<()> {
        let iv = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: slice } },
        };
        let mut input = None;
        self.vd.CreateVideoProcessorInputView(tex, &self.en, &iv, Some(&mut input))?;
        let ov = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } },
        };
        let mut output = None;
        self.vd.CreateVideoProcessorOutputView(target, &self.en, &ov, Some(&mut output))?;
        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: std::mem::ManuallyDrop::new(input),
            ..Default::default()
        };
        let mut streams = [stream];
        let r = self.vc.VideoProcessorBlt(&self.vp, output.as_ref().unwrap(), 0, &streams);
        std::mem::ManuallyDrop::drop(&mut streams[0].pInputSurface);
        r
    }
}

static RESIZED: AtomicBool = AtomicBool::new(false);
static SAVED: Mutex<Option<(isize, RECT)>> = Mutex::new(None);

/// Tela cheia sem borda e volta.
unsafe fn toggle_full(h: HWND) {
    let mut saved = SAVED.lock().unwrap();
    if let Some((style, r)) = saved.take() {
        SetWindowLongPtrW(h, GWL_STYLE, style);
        let _ = SetWindowPos(h, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_FRAMECHANGED | SWP_NOZORDER);
    } else {
        let mut r = RECT::default();
        let _ = GetWindowRect(h, &mut r);
        *saved = Some((GetWindowLongPtrW(h, GWL_STYLE), r));
        let mon = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let m = mi.rcMonitor;
        SetWindowLongPtrW(h, GWL_STYLE, (WS_POPUP | WS_VISIBLE).0 as isize);
        let _ = SetWindowPos(h, None, m.left, m.top, m.right - m.left, m.bottom - m.top, SWP_FRAMECHANGED | SWP_NOZORDER);
    }
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP => {
            let used = key_event(h, &mut KEYS.lock().unwrap(), m, w, l);
            // Teclas com Alt não abrem o menu da janela nem fecham com Alt+F4: vão para o host.
            if used {
                LRESULT(0)
            } else {
                DefWindowProcW(h, m, w, l)
            }
        }
        WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN | WM_MBUTTONUP | WM_XBUTTONDOWN | WM_XBUTTONUP
        | WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let mut k = KEYS.lock().unwrap();
            if !k.mouse {
                return DefWindowProcW(h, m, w, l);
            }
            let hi = (w.0 >> 16) as u16;
            match m {
                WM_MOUSEMOVE => {
                    let (x, y) = ((l.0 & 0xFFFF) as i16 as i32, ((l.0 >> 16) & 0xFFFF) as i16 as i32);
                    let (ow, oh) = client_size(h);
                    let r = video_rect(k.video.0, k.video.1, ow, oh);
                    let nx = ((x - r.left) as i64 * 65535 / (r.right - r.left - 1).max(1) as i64).clamp(0, 65535) as u16;
                    let ny = ((y - r.top) as i64 * 65535 / (r.bottom - r.top - 1).max(1) as i64).clamp(0, 65535) as u16;
                    // Só a posição mais nova importa.
                    if let Some(Input::Move { .. }) = k.pending.last() {
                        k.pending.pop();
                    }
                    k.pending.push(Input::Move { x: nx, y: ny });
                }
                WM_MOUSEWHEEL | WM_MOUSEHWHEEL => k.pending.push(Input::Wheel { delta: hi as i16, horizontal: m == WM_MOUSEHWHEEL }),
                _ => {
                    let (button, down) = match m {
                        WM_LBUTTONDOWN => (0, true),
                        WM_LBUTTONUP => (0, false),
                        WM_RBUTTONDOWN => (1, true),
                        WM_RBUTTONUP => (1, false),
                        WM_MBUTTONDOWN => (2, true),
                        WM_MBUTTONUP => (2, false),
                        WM_XBUTTONDOWN => (if hi == 1 { 3 } else { 4 }, true),
                        _ => (if hi == 1 { 3 } else { 4 }, false),
                    };
                    k.buttons[button as usize] = down;
                    k.pending.push(Input::Button { button, down });
                    // Segura o mouse na janela enquanto um botão está apertado.
                    if down {
                        SetCapture(h);
                    } else if !k.buttons.iter().any(|b| *b) {
                        let _ = ReleaseCapture();
                    }
                }
            }
            LRESULT(if m == WM_XBUTTONDOWN || m == WM_XBUTTONUP { 1 } else { 0 })
        }
        WM_KILLFOCUS => {
            release_all(&mut KEYS.lock().unwrap());
            DefWindowProcW(h, m, w, l)
        }
        // O cursor do host já vem na imagem: o daqui some em cima dela.
        WM_SETCURSOR if (l.0 & 0xFFFF) as u32 == HTCLIENT && KEYS.lock().unwrap().mouse => {
            SetCursor(None);
            LRESULT(1)
        }
        WM_SIZE => {
            RESIZED.store(true, Ordering::Relaxed);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(h, m, w, l),
    }
}

// ---------- Controles ----------

/// Teclado virando controle, por código de varredura (a posição da tecla, não a letra):
/// WASD analógico esquerdo, setas direcional, J A, K B, U X, I Y, Q LB, E RB, Z LT, C RT,
/// F e H os cliques dos analógicos, Enter Start, Backspace Back.
const KEY_PAD: [(u16, bool, u16); 20] = [
    (0x11, false, 0), // W
    (0x1E, false, 0), // A
    (0x1F, false, 0), // S
    (0x20, false, 0), // D
    (0x48, true, XINPUT_GAMEPAD_DPAD_UP.0),
    (0x50, true, XINPUT_GAMEPAD_DPAD_DOWN.0),
    (0x4B, true, XINPUT_GAMEPAD_DPAD_LEFT.0),
    (0x4D, true, XINPUT_GAMEPAD_DPAD_RIGHT.0),
    (0x24, false, XINPUT_GAMEPAD_A.0),
    (0x25, false, XINPUT_GAMEPAD_B.0),
    (0x16, false, XINPUT_GAMEPAD_X.0),
    (0x17, false, XINPUT_GAMEPAD_Y.0),
    (0x10, false, XINPUT_GAMEPAD_LEFT_SHOULDER.0),
    (0x12, false, XINPUT_GAMEPAD_RIGHT_SHOULDER.0),
    (0x2C, false, 0), // Z: gatilho esquerdo
    (0x2E, false, 0), // C: gatilho direito
    (0x21, false, XINPUT_GAMEPAD_LEFT_THUMB.0),
    (0x23, false, XINPUT_GAMEPAD_RIGHT_THUMB.0),
    (0x1C, false, XINPUT_GAMEPAD_START.0),
    (0x0E, false, XINPUT_GAMEPAD_BACK.0),
];

/// Estado do controle feito pelo teclado agora.
fn keyboard_pad() -> PadState {
    let held = |scan: u16, ext: bool| unsafe {
        let vk = if ext {
            match scan {
                0x48 => VK_UP.0 as u32,
                0x50 => VK_DOWN.0 as u32,
                0x4B => VK_LEFT.0 as u32,
                _ => VK_RIGHT.0 as u32,
            }
        } else {
            MapVirtualKeyW(scan as u32, MAPVK_VSC_TO_VK_EX)
        };
        vk != 0 && GetAsyncKeyState(vk as i32) < 0
    };
    let mut s = PadState::default();
    for &(scan, ext, bit) in &KEY_PAD {
        if bit != 0 && held(scan, ext) {
            s.buttons |= bit;
        }
    }
    let axis = |neg: bool, pos: bool| match (neg, pos) {
        (true, false) => -32768,
        (false, true) => 32767,
        _ => 0,
    };
    s.lx = axis(held(0x1E, false), held(0x20, false));
    s.ly = axis(held(0x1F, false), held(0x11, false));
    s.lt = if held(0x2C, false) { 255 } else { 0 };
    s.rt = if held(0x2E, false) { 255 } else { 0 };
    s
}

/// Lê os controles daqui e manda para o host: na hora quando mudam, e a cada 100 ms para o host saber
/// que continuam ali. Vagas vazias do XInput são conferidas uma vez por segundo, porque perguntar a
/// uma vaga vazia demora.
fn pads(link: Arc<dyn Link>, sh: Arc<Shared>, hwnd: isize) {
    let co = ClientOpts::load();
    let background = std::env::var("PRISMA_PAD_BACKGROUND").map(|v| v == "1").unwrap_or(co.pad_background);
    let keys = std::env::var("PRISMA_PAD_KEYS").map(|v| v == "1").unwrap_or(co.pad_keys);
    let mut last: [Option<PadState>; PADS] = [None; PADS];
    let mut seq = [0u16; PADS];
    let mut sent = [Instant::now(); PADS];
    let mut look = [Instant::now(); 4];
    // Quantas vezes ainda repetir o aviso de controle que saiu, contra perda.
    let mut off = [0u8; PADS];
    let mut out = Vec::with_capacity(32);
    while !sh.quit.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(4));
        if sh.perms.load(Ordering::Relaxed) & PAD == 0 {
            continue;
        }
        let focused = background || unsafe { GetForegroundWindow() }.0 as isize == hwnd;
        // No mesmo PC, os controles virtuais do host aparecem aqui também: esses não voltam.
        let theirs = if link.same_pc() { sh.pads.lock().unwrap().iter().filter(|p| p.player != 0 && p.slot < 4).fold(0u8, |m, p| m | 1 << p.slot) } else { 0 };
        for i in 0..PADS {
            let mut cur = if i == 4 {
                keys.then(keyboard_pad)
            } else if theirs & (1 << i) != 0 || (last[i].is_none() && Instant::now() < look[i]) {
                None
            } else {
                let mut st = XINPUT_STATE::default();
                if unsafe { XInputGetState(i as u32, &mut st) } == 0 {
                    let g = st.Gamepad;
                    Some(PadState { buttons: g.wButtons.0, lt: g.bLeftTrigger, rt: g.bRightTrigger, lx: g.sThumbLX, ly: g.sThumbLY, rx: g.sThumbRX, ry: g.sThumbRY })
                } else {
                    look[i] = Instant::now() + Duration::from_secs(1);
                    None
                }
            };
            // Fora da janela, o controle fica solto no host.
            if !focused {
                cur = cur.map(|_| PadState::default());
            }
            if cur.is_none() && last[i].is_some() {
                off[i] = 3;
            }
            let due = sent[i].elapsed() > Duration::from_millis(100);
            let send = match cur {
                Some(s) => last[i] != Some(s) || due,
                None => off[i] > 0 && (last[i].is_some() || due),
            };
            if send {
                seq[i] = seq[i].wrapping_add(1);
                if cur.is_none() {
                    off[i] -= 1;
                }
                write(&mut out, &Msg::Pad { pad: i as u8, seq: seq[i], on: cur.is_some(), state: cur.unwrap_or_default() });
                link.send(&out);
                sent[i] = Instant::now();
            }
            last[i] = cur;
        }
    }
}

/// Como os controles daqui estão no host, para o título.
fn pads_text(sh: &Shared) -> String {
    let p = *sh.pads.lock().unwrap();
    let mut parts = Vec::new();
    for (i, v) in p.iter().enumerate() {
        let who = if i == 4 { "teclado".to_string() } else { format!("controle {}", i + 1) };
        match (v.player, v.why) {
            (0, PAD_OK) => {}
            (0, why) => parts.push(format!("{who}: {}", pad_why(why))),
            (n, _) => parts.push(format!("{who}: jogador {n}")),
        }
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" · {}", parts.join(", "))
    }
}

fn client_size(h: HWND) -> (u32, u32) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(h, &mut r);
    }
    (((r.right - r.left).max(1)) as u32, ((r.bottom - r.top).max(1)) as u32)
}

#[derive(Default)]
struct Stats {
    shown: u32,
    e2e: Vec<i64>,
    decode: Vec<i64>,
    rtt: Vec<i64>,
}

fn pct(v: &mut [i64], q: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_unstable();
    v[((v.len() - 1) as f64 * q).round() as usize] as f64 / 10_000.0
}

pub fn run(arg: &str) {
    let mut parts = arg.split('|');
    let target = parts.next().unwrap_or("127.0.0.1");
    let out_file = parts.next().filter(|s| !s.is_empty()).map(str::to_string);
    let secs: Option<u64> = parts.next().and_then(|s| s.parse().ok());
    let host = if target.contains(':') { target.to_socket_addrs().ok().and_then(|mut a| a.next()) } else { (target, PORT).to_socket_addrs().ok().and_then(|mut a| a.next()) };
    let Some(host) = host else {
        eprintln!("endereço inválido: {target}");
        return;
    };
    let sock = match UdpSocket::bind(("0.0.0.0", 0)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sem socket: {e}");
            return;
        }
    };
    open(Arc::new(Udp::new(sock, host)), out_file.as_deref(), secs);
}

/// Abre a janela que assiste num canal já aberto.
pub(super) fn open(link: Arc<dyn Link>, out_file: Option<&str>, secs: Option<u64>) {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_LITE);
        let _ = timeBeginPeriod(1);
        if let Err(e) = watch(link, out_file, secs) {
            eprintln!("parou: {e:?}");
        }
        let _ = MFShutdown();
    }
}

unsafe fn watch(link: Arc<dyn Link>, out_file: Option<&str>, secs: Option<u64>) -> Result<()> {
    let mut co = ClientOpts::load();
    {
        let mut k = KEYS.lock().unwrap();
        k.key_menu = co.key_menu;
        k.key_full = co.key_full;
        k.key_cad = co.key_cad;
        k.key_disc = co.key_disc;
        k.key_imm = co.key_imm;
        k.key_meter = co.key_meter;
        k.key_sound = co.key_sound;
        k.acts.clear();
        k.immersive = co.immersive;
    }
    let d = d3d()?;
    if let Ok(mt) = d.ctx.cast::<ID3D11Multithread>() {
        let _ = mt.SetMultithreadProtected(true);
    }

    // Janela no monitor onde está o mouse, com 70% do tamanho dele.
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY);
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let _ = GetMonitorInfoW(mon, &mut mi);
    let m = mi.rcMonitor;
    let (mw, mh) = ((m.right - m.left) as u32, (m.bottom - m.top) as u32);
    let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u32>().ok());
    // Para testar como outra tela: PRISMA_VIEW_H e PRISMA_VIEW_HZ.
    let screen_h = env("PRISMA_VIEW_H").unwrap_or(mh);
    let screen_w = if screen_h == mh { mw } else { screen_h * 16 / 9 };
    let hz = env("PRISMA_VIEW_HZ").unwrap_or_else(|| monitor_hz(mon));

    let inst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
    let class = w!("PrismaAssistir");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: inst,
        lpszClassName: class,
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        ..Default::default()
    };
    RegisterClassW(&wc);
    let (ww, wh) = ((mw * 7 / 10) as i32, (mh * 7 / 10) as i32);
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class,
        w!("Prisma · conectando"),
        WS_OVERLAPPEDWINDOW | WS_VISIBLE,
        m.left + (mw as i32 - ww) / 2,
        m.top + (mh as i32 - wh) / 2,
        ww,
        wh,
        None,
        None,
        Some(inst),
        None,
    )?;
    VIEW_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
    // Medições sem ninguém olhando abrem em janela, como sempre.
    if co.fullscreen && out_file.is_none() {
        toggle_full(hwnd);
    }
    // O modo imersivo muda durante a conexão (menu e atalho); o gancho do teclado só existe com ele.
    let mut immersive = co.immersive;
    let hook_for = |imm: u8, hook: Option<HHOOK>| -> Option<HHOOK> {
        match (imm & 2 != 0, hook) {
            (true, None) => SetWindowsHookExW(WH_KEYBOARD_LL, Some(immersive_hook), None, 0).ok(),
            (false, Some(h)) => {
                let _ = UnhookWindowsHookEx(h);
                None
            }
            (_, h) => h,
        }
    };
    let mut hook = hook_for(immersive, None);
    let mut hud = hud::Hud::new(hwnd);
    let mut clipped = false;

    // Cadeia de troca com tearing permitido: mostra o quadro assim que ele fica pronto.
    let factory: IDXGIFactory2 = d.dev.cast::<IDXGIDevice>()?.GetAdapter()?.GetParent()?;
    let mut allow = BOOL(0);
    let tearing = factory
        .cast::<IDXGIFactory5>()
        .and_then(|f5| f5.CheckFeatureSupport(DXGI_FEATURE_PRESENT_ALLOW_TEARING, &mut allow as *mut _ as *mut _, std::mem::size_of::<BOOL>() as u32))
        .is_ok()
        && allow.as_bool();
    let flags = if tearing { DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0 as u32 } else { 0 };
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: 0,
        Height: 0,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        Stereo: false.into(),
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
        AlphaMode: DXGI_ALPHA_MODE_UNSPECIFIED,
        Flags: flags,
    };
    let swap = factory.CreateSwapChainForHwnd(&d.dev, hwnd, &desc, None, None)?;
    let _ = factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);
    // VSync ligado espera a atualização da tela: sem rasgos, um pouco mais de atraso.
    let present_flags = if tearing && !co.vsync { DXGI_PRESENT_ALLOW_TEARING } else { DXGI_PRESENT(0) };
    let sync = co.vsync as u32;

    let mut codecs = 0u8;
    for c in [Codec::H264, Codec::Hevc, Codec::Av1] {
        if can_decode(c) {
            codecs |= codec_bit(c);
        }
    }
    // Tela em HDR: o host manda HDR10 se a dele também estiver. 4:4:4 só pedindo, porque abre o
    // CUDA aqui e gasta mais rede.
    let mut flags = 0u8;
    if crate::capture::monitor_color(mon).hdr {
        flags |= SEES_HDR;
    }
    #[cfg(feature = "stream")]
    if std::env::var("PRISMA_444").map(|v| v == "1").unwrap_or(co.ask444) {
        if crate::capture::nvdec::can_444() {
            flags |= DECODES_444;
        } else {
            eprintln!("4:4:4: esta placa não decodifica HEVC 4:4:4; fica o 4:2:0");
        }
    }
    let mut hello = Vec::new();
    // Quadros por segundo pedidos: o automático manda os Hz desta tela.
    let ask_hz = |fps: u32| if fps > 0 { fps.min(hz.max(30)) } else { hz };
    write(&mut hello, &Msg::Hello { w: screen_w, h: screen_h, hz: ask_hz(co.fps), codecs, pid: std::process::id(), flags });
    let wake = CreateEventW(None, false, false, None)?;
    let sh = Arc::new(Shared {
        queue: Mutex::new(VecDeque::new()),
        wake: wake.0 as isize,
        quit: AtomicBool::new(false),
        closed: AtomicBool::new(false),
        want_key: AtomicBool::new(false),
        recovered: AtomicU32::new(0),
        got_info: AtomicBool::new(false),
        rtt: AtomicI64::new(0),
        offset: AtomicI64::new(0),
        lost: AtomicU32::new(0),
        bytes: AtomicU64::new(0),
        audio: Mutex::new(VecDeque::new()),
        mute: std::env::var("PRISMA_MUTE").is_ok_and(|v| v == "1"),
        silenced: AtomicBool::new(co.mute),
        audio_lost: AtomicU32::new(0),
        audio_blocks: AtomicU32::new(0),
        underruns: AtomicU32::new(0),
        audio_fill: AtomicU32::new(0),
        perms: AtomicU8::new(0),
        pads: Mutex::new([NO_PAD; PADS]),
    });
    let player = if sh.mute {
        None
    } else {
        let sh2 = sh.clone();
        std::thread::Builder::new().name("assistir-som".into()).spawn(move || play(sh2)).ok()
    };
    let rx = {
        let (l2, sh2, hello2) = (link.clone(), sh.clone(), hello.clone());
        std::thread::Builder::new().name("assistir-rede".into()).spawn(move || receive(l2, hello2, sh2)).unwrap()
    };
    let pad_thread = {
        let (l2, sh2, h) = (link.clone(), sh.clone(), hwnd.0 as isize);
        std::thread::Builder::new().name("assistir-controles".into()).spawn(move || pads(l2, sh2, h)).ok()
    };

    let mut dec: Option<Decoder> = None;
    let mut info: Option<(Codec, u32, u32, u32)> = None;
    // HDR10 e 4:4:4 do vídeo atual; a janela em HDR10.
    let mut vfmt = (false, false);
    let mut hdr_out = false;
    // 4:4:4: o NVDEC e o desenho, com a hora do último quadro novo.
    #[cfg(feature = "stream")]
    let mut nv: Option<(crate::capture::nvdec::Nvdec, crate::capture::nvdec::Draw444)> = None;
    #[cfg(feature = "stream")]
    let mut nv_pts: Option<i64> = None;
    // Eventos de entrada recentes, repetidos em cada pacote: o primeiro tem o número `input_first`.
    let mut history: VecDeque<Input> = VecDeque::new();
    let mut input_first = 0u32;
    let mut last_input = Instant::now() - Duration::from_secs(10);
    let mut last_send = Instant::now();
    let mut input_pkt = Vec::with_capacity(256);
    // O pedido de Ctrl+Alt+Del vai 3 vezes, contra perda; o host junta as repetidas.
    let (mut sas_left, mut sas_at, mut sas_pkt) = (0u8, Instant::now(), Vec::new());
    let mut fill: Vec<i64> = Vec::new();
    let mut scaler: Option<Scaler> = None;
    let mut decoded: Vec<Decoded> = Vec::new();
    let start = Instant::now();
    let warm = Duration::from_secs(2);
    let mut total = Stats::default();
    let mut second = Stats::default();
    let mut tick = Instant::now();
    let mut bytes_mark = 0u64;
    let mut lost_mark = 0u32;
    let mut msg = MSG::default();

    'ui: loop {
        let _ = MsgWaitForMultipleObjects(Some(&[wake]), false, 100, QS_ALLINPUT);
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                break 'ui;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if sh.closed.load(Ordering::Relaxed) || secs.is_some_and(|s| start.elapsed() > Duration::from_secs(s)) {
            let _ = DestroyWindow(hwnd);
            continue;
        }
        if RESIZED.swap(false, Ordering::Relaxed) {
            scaler = None;
            let _ = swap.ResizeBuffers(0, 0, 0, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(flags as i32));
            if let Some(h) = &mut hud {
                h.update(hwnd, co.meter, co.warn, None);
            }
        }
        // Modo imersivo: em tela cheia, com a janela na frente, o mouse não sai dela.
        let want_clip = immersive & 1 != 0 && SAVED.lock().unwrap().is_some() && GetForegroundWindow() == hwnd && KEYS.lock().unwrap().mouse;
        if want_clip != clipped {
            let mut r = RECT::default();
            let _ = GetWindowRect(hwnd, &mut r);
            let _ = ClipCursor(want_clip.then_some(&r as *const RECT));
            clipped = want_clip;
        }
        let mut picks: Vec<hud::Pick> = std::mem::take(&mut KEYS.lock().unwrap().acts);
        if std::mem::take(&mut KEYS.lock().unwrap().menu) {
            if clipped {
                let _ = ClipCursor(None);
                clipped = false;
            }
            let extra = match vfmt {
                (true, _) => " HDR10",
                (_, true) => " 4:4:4",
                _ => "",
            };
            let label = |k: opts::Keys| if k.1 == 0 { String::new() } else { crate::config::Hotkey::new(k.0, k.1).label() };
            let st = hud::MenuState {
                fullscreen: SAVED.lock().unwrap().is_some(),
                immersive,
                fps: co.fps,
                max_hz: hz,
                meter: co.meter,
                warn: co.warn,
                sound: !sh.silenced.load(Ordering::Relaxed),
                keyboard: KEYS.lock().unwrap().keyboard,
                video: info.map(|(c, w, h, fps)| format!("{w}x{h} {}{extra} · {fps} {} · {}", codec_name(c), crate::i18n::tr("q/s"), link.route())).unwrap_or_default(),
                keys: [label(co.key_menu), label(co.key_full), label(co.key_imm), label(co.key_meter), label(co.key_sound), label(co.key_cad), label(co.key_disc)],
            };
            if let Some(p) = hud::menu(hwnd, st) {
                picks.push(p);
            }
            if let Some(h) = &mut hud {
                h.update(hwnd, co.meter, co.warn, None);
            }
        }
        for p in picks {
            match p {
                hud::Pick::Fullscreen => toggle_full(hwnd),
                hud::Pick::Immersive(m) => {
                    immersive = m;
                    co.immersive = m;
                    let _ = co.save();
                }
                // O atalho liga e desliga só nesta conexão; ligar volta ao modo escolhido nas opções.
                hud::Pick::ImmersiveToggle => immersive = if immersive != 0 { 0 } else if co.immersive != 0 { co.immersive } else { 3 },
                hud::Pick::Fps(f) => {
                    if f != co.fps {
                        co.fps = f;
                        let _ = co.save();
                        // A descrição nova da tela faz o host recomeçar o vídeo nos quadros pedidos.
                        let mut again = Vec::new();
                        write(&mut again, &Msg::Hello { w: screen_w, h: screen_h, hz: ask_hz(f), codecs, pid: std::process::id(), flags });
                        link.send(&again);
                        link.send(&again);
                    }
                }
                hud::Pick::Meter(m) => {
                    co.meter = m;
                    let _ = co.save();
                }
                hud::Pick::MeterCycle => {
                    co.meter = (co.meter + 1) % 3;
                    let _ = co.save();
                }
                hud::Pick::Warn => {
                    co.warn = !co.warn;
                    let _ = co.save();
                }
                hud::Pick::Sound => {
                    sh.silenced.fetch_xor(true, Ordering::Relaxed);
                }
                hud::Pick::CtrlAltDel => KEYS.lock().unwrap().sas = true,
                hud::Pick::Disconnect => {
                    let _ = DestroyWindow(hwnd);
                }
            }
            if let Some(h) = &mut hud {
                h.update(hwnd, co.meter, co.warn, None);
            }
        }
        if KEYS.lock().unwrap().immersive != immersive {
            KEYS.lock().unwrap().immersive = immersive;
            hook = hook_for(immersive, hook);
        }

        let events: Vec<Event> = sh.queue.lock().unwrap().drain(..).collect();
        let mut last: Option<Decoded> = None;
        for e in events {
            match e {
                Event::Info { codec, w, h, fps, perms, hdr10, yuv444 } => {
                    {
                        let mut k = KEYS.lock().unwrap();
                        k.keyboard = perms & KEYBOARD != 0;
                        k.mouse = perms & MOUSE != 0;
                        k.pad_keys = perms & PAD != 0 && std::env::var("PRISMA_PAD_KEYS").map(|v| v == "1").unwrap_or(co.pad_keys);
                        k.video = (w, h);
                    }
                    if info != Some((codec, w, h, fps)) || vfmt != (hdr10, yuv444) {
                        info = Some((codec, w, h, fps));
                        vfmt = (hdr10, yuv444);
                        scaler = None;
                        dec = None;
                        #[cfg(feature = "stream")]
                        {
                            nv = None;
                        }
                        // A janela muda para HDR10 (10 bits, PQ, BT.2020) e volta.
                        if hdr10 != hdr_out {
                            let fmt = if hdr10 { DXGI_FORMAT_R10G10B10A2_UNORM } else { DXGI_FORMAT_B8G8R8A8_UNORM };
                            let cs = if hdr10 { DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020 } else { DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709 };
                            let done = swap.ResizeBuffers(0, 0, 0, fmt, DXGI_SWAP_CHAIN_FLAG(flags as i32)).and_then(|_| swap.cast::<IDXGISwapChain3>()?.SetColorSpace1(cs));
                            match done {
                                Ok(()) => hdr_out = hdr10,
                                Err(e) => eprintln!("janela HDR10: {e:?}"),
                            }
                        }
                        if yuv444 {
                            #[cfg(feature = "stream")]
                            {
                                use crate::capture::nvdec::{Draw444, Nvdec};
                                nv = match Nvdec::new(&d.dev, w, h).and_then(|n| Ok((n, Draw444::new(&d.dev, h)?))) {
                                    Ok(x) => Some(x),
                                    Err(e) => {
                                        eprintln!("sem NVDEC para o 4:4:4: {e:?}");
                                        None
                                    }
                                };
                            }
                            #[cfg(not(feature = "stream"))]
                            eprintln!("o 4:4:4 só existe compilando com --features stream");
                        } else {
                            dec = match Decoder::new(&d.dev, codec, w, h, fps, None, hdr10) {
                                Ok(x) => Some(x),
                                Err(e) => {
                                    eprintln!("sem decodificador para {}: {e:?}", codec_name(codec));
                                    None
                                }
                            };
                        }
                    }
                }
                #[cfg(feature = "stream")]
                Event::Frame(f) if nv.is_some() => {
                    let Some((n, _)) = &mut nv else { continue };
                    let t = now();
                    match n.decode(&d.ctx, &f.data, f.pts) {
                        Ok(true) => {
                            second.decode.push(now() - t);
                            nv_pts = Some(f.pts);
                        }
                        Ok(false) => {}
                        Err(_) => sh.want_key.store(true, Ordering::Relaxed),
                    }
                }
                Event::Frame(f) => {
                    let Some(dc) = &mut dec else { continue };
                    let t = now();
                    decoded.clear();
                    let p = Pkt { data: f.data, pts: f.pts, key: f.key };
                    if dc.decode(Some(&p), &mut decoded).is_err() {
                        sh.want_key.store(true, Ordering::Relaxed);
                        continue;
                    }
                    second.decode.push(now() - t);
                    // Só o mais novo vai para a tela.
                    if let Some(x) = decoded.pop() {
                        last = Some(x);
                    }
                }
            }
        }
        // Mouse e teclado novos vão na hora; os recentes se repetem por um tempo, contra perda.
        if std::mem::take(&mut KEYS.lock().unwrap().sas) {
            eprintln!("Ctrl+Alt+Del pedido ao host");
            sas_left = 3;
        }
        if sas_left > 0 && sas_at.elapsed() >= Duration::from_millis(30) {
            write(&mut sas_pkt, &Msg::Sas);
            link.send(&sas_pkt);
            sas_left -= 1;
            sas_at = Instant::now();
        }
        let fresh: Vec<Input> = std::mem::take(&mut KEYS.lock().unwrap().pending);
        if !fresh.is_empty() {
            for e in fresh {
                history.push_back(e);
                if history.len() > INPUT_KEEP {
                    history.pop_front();
                    input_first = input_first.wrapping_add(1);
                }
            }
            last_input = Instant::now();
        }
        if !history.is_empty() && (last_input > last_send || (last_input.elapsed() < Duration::from_millis(300) && last_send.elapsed() > Duration::from_millis(30))) {
            let list: Vec<Input> = history.iter().copied().collect();
            write_input(&mut input_pkt, input_first, &list);
            link.send(&input_pkt);
            last_send = Instant::now();
        }

        #[cfg(feature = "stream")]
        if let (Some(pts), Some((n, draw))) = (nv_pts.take(), &nv) {
            let (ow, oh) = client_size(hwnd);
            let bb: ID3D11Texture2D = swap.GetBuffer(0)?;
            if draw.draw(&d.dev, &d.ctx, n, &bb, ow, oh).is_ok() && swap.Present(sync, present_flags).is_ok() {
                let shown = now();
                second.shown += 1;
                second.e2e.push(shown + sh.offset.load(Ordering::Relaxed) - pts);
            }
        }
        if let (Some(f), Some((_, w, h, fps))) = (last, info) {
            if let Some((tex, slice)) = f.texture() {
                let (ow, oh) = client_size(hwnd);
                if scaler.as_ref().is_none_or(|s| s.size != (w, h, ow, oh) || s.hdr != hdr_out) {
                    scaler = Scaler::new(&d, w, h, ow, oh, fps, hdr_out).ok();
                }
                if let Some(s) = &scaler {
                    let bb: ID3D11Texture2D = swap.GetBuffer(0)?;
                    if s.blt(&tex, slice, &bb).is_ok() && swap.Present(sync, present_flags).is_ok() {
                        let shown = now();
                        second.shown += 1;
                        second.e2e.push(shown + sh.offset.load(Ordering::Relaxed) - f.pts);
                    }
                }
            }
        }

        if tick.elapsed() >= Duration::from_secs(1) {
            let bytes = sh.bytes.load(Ordering::Relaxed);
            let mbps = (bytes - bytes_mark) as f64 * 8.0 / tick.elapsed().as_secs_f64() / 1e6;
            bytes_mark = bytes;
            let rtt = sh.rtt.load(Ordering::Relaxed);
            second.rtt.push(rtt);
            if start.elapsed() > warm && !sh.mute {
                fill.push(sh.audio_fill.load(Ordering::Relaxed) as i64);
            }
            let fps_now = second.shown as f64 / tick.elapsed().as_secs_f64();
            let lost_total = sh.lost.load(Ordering::Relaxed);
            if let Some(h) = &mut hud {
                let sample = hud::Sample {
                    rtt_ms: rtt as f64 / 10_000.0,
                    e2e_ms: pct(&mut second.e2e.clone(), 0.5),
                    decode_ms: pct(&mut second.decode.clone(), 0.5),
                    fps: fps_now,
                    mbps,
                    lost: lost_total.saturating_sub(lost_mark),
                    lost_total,
                    shown: second.shown,
                    video: info.map(|(c, w, h, _)| format!("{w}x{h} {}", codec_name(c))).unwrap_or_default(),
                    route: link.route(),
                };
                h.update(hwnd, co.meter, co.warn, info.is_some().then_some(&sample));
            }
            lost_mark = lost_total;
            let extra = match vfmt {
                (true, _) => " HDR10",
                (_, true) => " 4:4:4",
                _ => "",
            };
            let title = match info {
                Some((c, w, h, _)) => format!(
                    "Prisma · {}x{} {}{} · {:.0} q/s · {:.1} Mbit/s · rede {:.1} ms · ponta a ponta {:.1} ms · perdidos {} · {}{}",
                    w,
                    h,
                    codec_name(c),
                    extra,
                    fps_now,
                    mbps,
                    rtt as f64 / 10_000.0,
                    pct(&mut second.e2e.clone(), 0.5),
                    sh.lost.load(Ordering::Relaxed),
                    link.route(),
                    pads_text(&sh)
                ),
                None => format!("Prisma · conectando ({})", link.route()),
            };
            let _ = SetWindowTextW(hwnd, &HSTRING::from(title));
            if start.elapsed() > warm {
                total.shown += second.shown;
                total.e2e.append(&mut second.e2e);
                total.decode.append(&mut second.decode);
                total.rtt.append(&mut second.rtt);
            }
            second = Stats::default();
            tick = Instant::now();
        }
    }

    sh.quit.store(true, Ordering::Relaxed);
    if clipped {
        let _ = ClipCursor(None);
    }
    if let Some(h) = hook {
        let _ = UnhookWindowsHookEx(h);
    }
    drop(hud);
    if let Some(p) = player {
        let _ = p.join();
    }
    let mut bye = Vec::new();
    write(&mut bye, &Msg::Bye);
    link.send(&bye);
    let _ = rx.join();
    if let Some(t) = pad_thread {
        let _ = t.join();
    }
    let _ = CloseHandle(wake);

    if let Some(path) = out_file {
        let secs = start.elapsed().saturating_sub(warm).as_secs_f64().max(0.001);
        let mut s = String::from("Assistir pela rede (fase 1)\n");
        if let Some((c, w, h, fps)) = info {
            s += &format!("vídeo: {w}x{h} a {fps} quadros/s, {}\n", codec_name(c));
        }
        s += &format!("caminho: {}
", link.route());
        s += &format!("tela daqui: {screen_w}x{screen_h} a {hz} Hz, tearing {}\n", if tearing { "permitido" } else { "não" });
        s += &format!(
            "quadros mostrados: {:.1}/s, perdidos: {} ({} consertados sem quadro-chave)\n",
            total.shown as f64 / secs,
            sh.lost.load(Ordering::Relaxed),
            sh.recovered.load(Ordering::Relaxed)
        );
        s += &format!("recebido: {:.1} Mbit/s em média\n", sh.bytes.load(Ordering::Relaxed) as f64 * 8.0 / start.elapsed().as_secs_f64() / 1e6);
        s += &format!("rede (ida e volta): mediana {:.2} ms\n", pct(&mut total.rtt, 0.5));
        s += &format!("decodificação: mediana {:.2} ms, p95 {:.2} ms\n", pct(&mut total.decode, 0.5), pct(&mut total.decode, 0.95));
        s += &format!(
            "ponta a ponta (da tela do host até mostrar aqui): mediana {:.2} ms, p95 {:.2} ms, máx {:.2} ms\n",
            pct(&mut total.e2e, 0.5),
            pct(&mut total.e2e, 0.95),
            pct(&mut total.e2e, 1.0)
        );
        let blocks = sh.audio_blocks.load(Ordering::Relaxed);
        if sh.mute {
            s += &format!("som: {blocks} blocos de 5 ms recebidos, {} perdidos, sem tocar\n", sh.audio_lost.load(Ordering::Relaxed));
        } else {
            fill.sort_unstable();
            let med = fill.get(fill.len() / 2).copied().unwrap_or(0) as f64 * 1000.0 / AUDIO_RATE as f64;
            s += &format!(
                "som: {blocks} blocos de 5 ms recebidos, {} perdidos, {} vezes acabou antes do próximo, espera na saída {:.1} ms (mediana)\n",
                sh.audio_lost.load(Ordering::Relaxed),
                sh.underruns.load(Ordering::Relaxed),
                med
            );
        }
        let _ = std::fs::write(path, s);
    }
    Ok(())
}
