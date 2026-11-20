//! Menu do HDR do Prisma por cima do jogo: o atalho abre, os ajustes vão ao vivo pela memória
//! compartilhada e, ao fechar com mudanças, ele pergunta se salva no perfil do jogo. O jogo segue
//! com o foco: as teclas do menu chegam como atalhos registrados só enquanto ele está aberto.

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::overlays::{focus_monitor, register};
use crate::sys::inject::{Hdr, HdrLive};
use crate::ui::gfx::*;
use crate::ui::layered::{create_overlay, Layered};
use crate::ui::{Ui, UiState};

const W: f32 = 460.0;
const PAD: f32 = 16.0;
/// Os ajustes, um por linha.
const KNOBS_H: f32 = crate::pages::hdr::KNOBS.len() as f32 * 48.0;
/// Teclas do menu; o atalho de cada uma é a posição + 1.
const KEYS: [VIRTUAL_KEY; 6] = [VK_ESCAPE, VK_RETURN, VK_UP, VK_DOWN, VK_LEFT, VK_RIGHT];
/// Confere duas vezes por segundo se o jogo ainda está aberto e em foco.
const TIMER_GAME: usize = 1;

enum Act {
    Close,
    Save,
    Discard,
}

pub struct HdrMenu {
    surface: Layered,
    ui: UiState,
    live: Option<HdrLive>,
    /// Posição do jogo em `hdr_games`; sem ela não há perfil onde salvar.
    game: Option<usize>,
    pid: u32,
    name: String,
    start: Hdr,
    now: Hdr,
    calibrated: Hdr,
    asking: bool,
    /// Linha do ajuste que as setas mudam.
    row: usize,
}

unsafe extern "system" fn menu_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP => {
            let scale = windows::Win32::UI::HiDpi::GetDpiForWindow(h) as f32 / 96.0;
            let (x, y) = ((l.0 & 0xffff) as i16 as f32 / scale, ((l.0 >> 16) & 0xffff) as i16 as f32 / scale);
            crate::app::with(|a| a.hdr_menu_mouse(m, x, y));
            LRESULT(0)
        }
        WM_HOTKEY => {
            if let Some(&k) = KEYS.get(w.0.wrapping_sub(1)) {
                crate::app::with(|a| a.hdr_menu_keydown(k));
            }
            LRESULT(0)
        }
        WM_TIMER => {
            crate::app::with(|a| a.hdr_menu.watch());
            LRESULT(0)
        }
        _ => DefWindowProcW(h, m, w, l),
    }
}

impl HdrMenu {
    pub fn new() -> Self {
        register(w!("PrismaHdrMenu"), menu_proc);
        let hwnd = create_overlay(w!("PrismaHdrMenu"), false);
        // Fora de fotos e vídeos: o menu é só para quem joga.
        crate::capture::hide_from_capture(hwnd);
        let h = Hdr::from_profile(None);
        Self { surface: Layered::new(hwnd), ui: UiState::default(), live: None, game: None, pid: 0, name: String::new(), start: h, now: h, calibrated: h, asking: false, row: 0 }
    }

    pub fn is_open(&self) -> bool {
        self.live.is_some()
    }

    fn open(&mut self, live: HdrLive, game: Option<usize>, pid: u32, name: String, calibrated: Hdr) {
        self.start = live.get();
        self.now = self.start;
        self.live = Some(live);
        self.game = game;
        self.pid = pid;
        self.name = name;
        self.calibrated = calibrated;
        self.asking = false;
        self.row = 0;
        self.ui = UiState::default();
        self.redraw();
        let hwnd = self.surface.hwnd;
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            for (i, k) in KEYS.into_iter().enumerate() {
                // Esc e Enter sem repetir: segurar não fecha e já salva. As setas repetem.
                let once = if i < 2 { MOD_NOREPEAT } else { HOT_KEY_MODIFIERS(0) };
                let _ = RegisterHotKey(Some(hwnd), i as i32 + 1, once, k.0 as u32);
            }
            SetTimer(Some(hwnd), TIMER_GAME, 500, None);
        }
    }

    fn hide(&mut self) {
        self.live = None;
        let hwnd = self.surface.hwnd;
        unsafe {
            for i in 1..=KEYS.len() as i32 {
                let _ = UnregisterHotKey(Some(hwnd), i);
            }
            let _ = KillTimer(Some(hwnd), TIMER_GAME);
            let _ = ReleaseCapture();
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }

    /// O jogo fechou ou saiu da frente: o menu some e solta as teclas. Os ajustes ficam valendo.
    fn watch(&mut self) {
        let front = crate::sys::procs::window_pid(unsafe { GetForegroundWindow() });
        if self.is_open() && (front != self.pid || !crate::sys::procs::is_alive(self.pid)) {
            self.hide();
        }
    }

    /// Setas para os lados: um centésimo da faixa do ajuste escolhido.
    fn step(&mut self, dir: f32) {
        let (_, lo, hi) = crate::pages::hdr::KNOBS[self.row];
        let mut h = self.now;
        let v = crate::pages::hdr::knob(&mut h, self.row);
        *v = (*v + dir * (hi - lo) / 100.0).clamp(lo, hi);
        self.set(h);
    }

    fn set(&mut self, h: Hdr) {
        self.now = h;
        if let Some(l) = &self.live {
            l.set(&h);
        }
    }

    fn mouse(&mut self, msg: u32, x: f32, y: f32) -> Option<Act> {
        self.ui.mx = x;
        self.ui.my = y;
        match msg {
            WM_LBUTTONDOWN => {
                self.ui.down = true;
                self.ui.pressed = true;
                unsafe { SetCapture(self.surface.hwnd) };
            }
            WM_LBUTTONUP => {
                self.ui.down = false;
                self.ui.released = true;
                unsafe {
                    let _ = ReleaseCapture();
                }
            }
            _ => {}
        }
        self.redraw()
    }

    fn redraw(&mut self) -> Option<Act> {
        let mon = focus_monitor();
        let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(self.surface.hwnd) } as f32 / 96.0;
        // A aba HDR pode ter mudado os valores do jogo com o menu aberto.
        if let Some(l) = &self.live {
            self.now = l.get();
        }
        let h = if self.asking { 150.0 } else { 54.0 + KNOBS_H + 8.0 + 34.0 + PAD };
        // Canto superior esquerdo: o medidor fica no direito.
        let (x, y) = (mon.left + (24.0 * scale) as i32, mon.top + (24.0 * scale) as i32);
        let (mut act, mut now, mut moved) = (None, self.now, false);
        let (asking, calibrated, can_save, row) = (self.asking, self.calibrated, self.game.is_some(), self.row);
        let title = format!("{} · {}", crate::i18n::tr("HDR do Prisma"), self.name);
        let ui = &mut self.ui;
        self.surface.paint(x, y, W, h, |c, r| {
            c.round(r, 14.0, rgb(0x14161c).alpha(0.96));
            let mut u = Ui::begin(c, ui);
            if asking {
                u.c.text(rect(PAD, 12.0, W - 2.0 * PAD, 26.0), "Salvar no perfil do jogo?", 14.0, WHITE, Align::Left, Weight::Semibold);
                u.c.text_wrap(rect(PAD, 42.0, W - 2.0 * PAD, 40.0), "Os ajustes já valem agora. Salvar guarda para as próximas vezes que o jogo abrir.", 13.0, TEXT2, Weight::Regular);
                if u.button("hm-save", rect(PAD, h - 50.0, 120.0, 34.0), "Salvar", true) {
                    act = Some(Act::Save);
                }
                if u.button("hm-discard", rect(PAD + 130.0, h - 50.0, 120.0, 34.0), "Descartar", false) {
                    act = Some(Act::Discard);
                }
            } else {
                u.c.text(rect(PAD, 12.0, W - 2.0 * PAD, 26.0), &title, 14.0, WHITE, Align::Left, Weight::Semibold);
                u.c.round(rect(PAD - 8.0, 54.0 + row as f32 * 48.0 - 6.0, W - 2.0 * PAD + 16.0, 42.0), 8.0, rgb(0x262a33));
                moved = crate::pages::hdr::knobs(&mut u, "hm-k", rect(PAD, 54.0, W - 2.0 * PAD, KNOBS_H), 1, &mut now);
                let by = h - PAD - 34.0;
                if u.button("hm-cal", rect(PAD, by, 200.0, 34.0), "Voltar à calibração", false) {
                    now = Hdr { on: now.on, ..calibrated };
                    moved = true;
                }
                if u.button("hm-close", rect(W - PAD - 110.0, by, 110.0, 34.0), "Fechar", true) {
                    act = Some(Act::Close);
                }
            }
            u.end();
        });
        if moved {
            self.set(now);
        }
        // Sem perfil no config não há o que perguntar.
        if matches!(act, Some(Act::Close)) && !can_save {
            act = None;
            self.hide();
        }
        act
    }

    fn changed(&self) -> bool {
        self.now != self.start
    }
}

impl crate::app::App {
    /// O atalho: abre o menu sobre o jogo em foco ou, aberto, fecha (perguntando se mudou algo).
    pub fn hdr_menu_key(&mut self) {
        if self.hdr_menu.is_open() {
            return self.hdr_menu_close();
        }
        let Some((front, pid, exe)) = self.fg_info() else { return };
        let Some(live) = HdrLive::connect(pid) else {
            self.toast.info("Este jogo não está com o HDR do Prisma: abra-o com ele pela aba HDR.");
            return;
        };
        let game = self.cfg.hdr_games.iter().position(|g| g.exe.eq_ignore_ascii_case(&exe));
        // A calibração da tela onde o jogo está, a mesma que a aba HDR mostra para ela.
        let calibrated = crate::sys::autohdr::calibrated(&self.cfg, &crate::overlays::monitor_name(front));
        let name = crate::pages::app_name(&exe).to_string();
        self.hdr_menu.open(live, game, pid, name, calibrated);
    }

    fn hdr_menu_close(&mut self) {
        let m = &mut self.hdr_menu;
        if m.asking {
            return;
        }
        if m.changed() && m.game.is_some() {
            m.asking = true;
            m.ui = UiState::default();
            m.redraw();
        } else {
            m.hide();
        }
    }

    pub fn hdr_menu_mouse(&mut self, msg: u32, x: f32, y: f32) {
        if let Some(act) = self.hdr_menu.mouse(msg, x, y) {
            self.hdr_menu_act(act);
        }
    }

    /// Esc e Enter fecham; na pergunta, Enter salva e Esc descarta. Setas: ajuste e valor.
    fn hdr_menu_keydown(&mut self, k: VIRTUAL_KEY) {
        let m = &mut self.hdr_menu;
        let n = crate::pages::hdr::KNOBS.len();
        match (k, m.asking) {
            (VK_RETURN, true) => return self.hdr_menu_act(Act::Save),
            (VK_ESCAPE, true) => return self.hdr_menu_act(Act::Discard),
            (VK_ESCAPE | VK_RETURN, false) => return self.hdr_menu_close(),
            (VK_UP, false) => m.row = (m.row + n - 1) % n,
            (VK_DOWN, false) => m.row = (m.row + 1) % n,
            (VK_LEFT, false) => m.step(-1.0),
            (VK_RIGHT, false) => m.step(1.0),
            _ => return,
        }
        m.redraw();
    }

    fn hdr_menu_act(&mut self, act: Act) {
        match act {
            Act::Close => self.hdr_menu_close(),
            Act::Save => {
                let m = &mut self.hdr_menu;
                if let Some(g) = m.game.and_then(|i| self.cfg.hdr_games.get_mut(i)) {
                    g.prisma = Some(m.now);
                    self.cfg.save();
                }
                m.hide();
                self.invalidate();
            }
            Act::Discard => {
                let m = &mut self.hdr_menu;
                let start = m.start;
                m.set(start);
                m.hide();
            }
        }
    }
}
