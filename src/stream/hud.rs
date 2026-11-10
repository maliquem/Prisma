//! Na janela de quem assiste: o medidor de latência, o aviso de conexão ruim e o menu da conexão.
//!
//! O medidor e o aviso ficam numa camada transparente presa à janela (some com ela e não recebe
//! cliques). O menu é o menu do próprio Windows, que funciona com o mouse e com o teclado.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::i18n::tr;
use crate::ui::gfx::{measure, rect, rgb, Align, Weight, MUTED, TEXT, WARN};
use crate::ui::layered::Layered;

/// O que a janela mediu no último segundo.
#[derive(Default, Clone)]
pub struct Sample {
    pub rtt_ms: f64,
    pub e2e_ms: f64,
    pub decode_ms: f64,
    pub fps: f64,
    pub mbps: f64,
    /// Quadros perdidos neste segundo e no total.
    pub lost: u32,
    pub lost_total: u32,
    pub shown: u32,
    /// Ex.: `1920x1080 AV1 HDR10`.
    pub video: String,
    pub route: String,
}

/// Quanto tempo o aviso fica na tela.
const WARN_FOR: Duration = Duration::from_secs(6);

pub struct Hud {
    layer: Layered,
    visible: bool,
    warn: Option<(&'static str, Instant)>,
    rtts: VecDeque<f64>,
    rising: u32,
}

fn green() -> crate::ui::gfx::Color {
    rgb(0x6bd38a)
}

impl Hud {
    pub fn new(owner: HWND) -> Option<Self> {
        unsafe {
            let inst: HINSTANCE = GetModuleHandleW(None).ok()?.into();
            let class = w!("PrismaAssistirMedidor");
            unsafe extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
                DefWindowProcW(h, m, w, l)
            }
            let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: inst, lpszClassName: class, ..Default::default() };
            RegisterClassW(&wc);
            let ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
            // Dona é a janela de quem assiste: a camada fica sempre logo acima dela, e só dela.
            let h = CreateWindowExW(ex, class, w!(""), WS_POPUP, 0, 0, 1, 1, Some(owner), None, Some(inst), None).ok()?;
            Some(Hud { layer: Layered::new(h), visible: false, warn: None, rtts: VecDeque::new(), rising: 0 })
        }
    }

    /// Confere a conexão no segundo que passou. Devolve o aviso novo, se houver.
    fn check(&mut self, s: &Sample) -> Option<&'static str> {
        let frames = s.shown + s.lost;
        if frames >= 10 && s.lost as f64 / frames as f64 > 0.02 {
            return Some("Conexão instável: a rede está perdendo quadros");
        }
        if s.rtt_ms > 0.0 {
            self.rtts.push_back(s.rtt_ms);
            if self.rtts.len() > 5 {
                self.rtts.pop_front();
            }
            let (lo, hi) = self.rtts.iter().fold((f64::MAX, 0.0f64), |(l, h), &v| (l.min(v), h.max(v)));
            let prev = self.rtts.iter().rev().nth(1).copied();
            self.rising = match prev {
                Some(p) if s.rtt_ms > p + 1.0 => self.rising + 1,
                _ => 0,
            };
            if self.rtts.len() >= 4 && hi - lo > 10.0 {
                return Some("Conexão instável: a latência está variando");
            }
            if self.rising >= 3 {
                self.rising = 0;
                return Some("Conexão piorando: a latência está subindo");
            }
        }
        None
    }

    /// Uma vez por segundo, e quando a janela muda.
    pub fn update(&mut self, owner: HWND, meter: u8, warn_on: bool, s: Option<&Sample>) {
        if let Some(s) = s {
            if let Some(w) = self.check(s) {
                if warn_on {
                    self.warn = Some((w, Instant::now()));
                }
            }
        }
        if self.warn.is_some_and(|(_, t)| t.elapsed() > WARN_FOR) || !warn_on {
            self.warn = None;
        }
        let iconic = unsafe { IsIconic(owner).as_bool() };
        let mut lines: Vec<(String, crate::ui::gfx::Color, f32, Weight)> = Vec::new();
        if let Some(s) = s.filter(|_| meter > 0) {
            let ms = if s.e2e_ms > 0.0 { s.e2e_ms } else { s.rtt_ms };
            if meter == 1 {
                lines.push((format!("{ms:.0} ms"), TEXT, 15.0, Weight::Semibold));
            } else {
                let c = crate::i18n::comma();
                let n = |v: f64, d: usize| format!("{v:.d$}").replace('.', c);
                lines.push((format!("{} ms {} · {} {} ms", n(s.e2e_ms, 1), tr("ponta a ponta"), tr("rede"), n(s.rtt_ms, 1)), TEXT, 14.0, Weight::Semibold));
                lines.push((format!("{} · {:.0} {} · {} Mbit/s", s.video, s.fps, tr("q/s"), n(s.mbps, 1)), TEXT, 13.0, Weight::Regular));
                lines.push((format!("{} {} ms · {} {}", tr("decodificação"), n(s.decode_ms, 2), tr("perdidos"), s.lost_total), MUTED, 13.0, Weight::Regular));
                lines.push((s.route.clone(), MUTED, 13.0, Weight::Regular));
            }
        }
        if let Some((w, _)) = self.warn {
            lines.push((tr(w).into_owned(), WARN, 14.0, Weight::Semibold));
        }
        if lines.is_empty() || iconic {
            if self.visible {
                unsafe {
                    let _ = ShowWindow(self.layer.hwnd, SW_HIDE);
                }
                self.visible = false;
            }
            return;
        }
        let dot = meter == 1;
        let w = lines.iter().map(|(t, _, size, wt)| measure(t, *size, *wt, None).0).fold(0.0f32, f32::max) + 28.0 + if dot { 18.0 } else { 0.0 };
        let h = lines.iter().map(|l| l.2 + 8.0).sum::<f32>() + 16.0;
        let mut p = POINT { x: 12, y: 12 };
        unsafe {
            let _ = ClientToScreen(owner, &mut p);
        }
        let ms = s.map_or(0.0, |s| if s.e2e_ms > 0.0 { s.e2e_ms } else { s.rtt_ms });
        let dot_color = if ms < 30.0 {
            green()
        } else if ms < 60.0 {
            WARN
        } else {
            rgb(0xff5a52)
        };
        self.layer.paint(p.x, p.y, w, h, |c, r| {
            c.round(r, 10.0, rgb(0x101317).alpha(0.82));
            let mut y = r.y + 8.0;
            for (i, (t, col, size, wt)) in lines.iter().enumerate() {
                let lh = size + 8.0;
                let mut x = r.x + 14.0;
                if dot && i == 0 {
                    c.circle(x + 5.0, y + lh / 2.0, 5.0, dot_color);
                    x += 18.0;
                }
                c.text_raw(rect(x, y, r.right() - x - 10.0, lh), t, *size, *col, Align::Left, *wt);
                y += lh;
            }
        });
        if !self.visible {
            unsafe {
                let _ = ShowWindow(self.layer.hwnd, SW_SHOWNOACTIVATE);
            }
            self.visible = true;
        }
    }
}

impl Drop for Hud {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.layer.hwnd);
        }
    }
}

/// O que o menu mostra.
pub struct MenuState {
    pub fullscreen: bool,
    pub meter: u8,
    pub warn: bool,
    pub sound: bool,
    pub keyboard: bool,
    pub video: String,
}

pub enum Pick {
    Fullscreen,
    Meter(u8),
    Warn,
    Sound,
    CtrlAltDel,
    Disconnect,
}

/// O menu da conexão, onde está o mouse.
pub fn menu(owner: HWND, st: &MenuState) -> Option<Pick> {
    unsafe {
        let m = CreatePopupMenu().ok()?;
        let sub = CreatePopupMenu().ok()?;
        let s = |t: &str| HSTRING::from(tr(t).as_ref());
        let flag = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
        for (i, name) in ["Desligado", "Simples", "Detalhado"].iter().enumerate() {
            let _ = AppendMenuW(sub, MF_STRING | flag(st.meter == i as u8), 10 + i, &s(name));
        }
        let _ = AppendMenuW(m, MF_STRING | flag(st.fullscreen), 1, &s("Tela cheia"));
        let _ = AppendMenuW(m, MF_POPUP, sub.0 as usize, &s("Medidor de latência"));
        let _ = AppendMenuW(m, MF_STRING | flag(st.warn), 2, &s("Avisar quando a conexão piorar"));
        let _ = AppendMenuW(m, MF_STRING | flag(st.sound), 3, &s("Som"));
        let _ = AppendMenuW(m, MF_STRING | if st.keyboard { MF_ENABLED } else { MF_GRAYED }, 4, &s("Mandar Ctrl+Alt+Del"));
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        if !st.video.is_empty() {
            let _ = AppendMenuW(m, MF_STRING | MF_GRAYED, 0, &HSTRING::from(st.video.as_str()));
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        }
        let _ = AppendMenuW(m, MF_STRING, 5, &s("Desconectar"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // O menu só fecha direito com a janela dona em primeiro plano.
        let _ = SetForegroundWindow(owner);
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, pt.x, pt.y, Some(0), owner, None);
        let _ = DestroyMenu(m);
        match cmd.0 {
            1 => Some(Pick::Fullscreen),
            2 => Some(Pick::Warn),
            3 => Some(Pick::Sound),
            4 => Some(Pick::CtrlAltDel),
            5 => Some(Pick::Disconnect),
            10..=12 => Some(Pick::Meter((cmd.0 - 10) as u8)),
            _ => None,
        }
    }
}
