//! Menu do HDR do Prisma por cima do jogo: o atalho abre, os ajustes vão ao vivo pela memória
//! compartilhada e, ao fechar com mudanças, ele pergunta se salva no perfil do jogo. O jogo segue
//! com o foco: as teclas do menu chegam como atalhos registrados só enquanto ele está aberto, e o
//! controle navega por `nav::menu_feed` (direcional como as setas, A como Enter, B como Esc).
//! Analisar mede por uns segundos o que o jogo entrega e refaz o Automático com isso.

use std::time::{Duration, Instant};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::HdrGame;
use crate::overlays::{focus_monitor, register};
use crate::sys::autohdr;
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
/// Analisar: quanto tempo mede e quantos quadros precisam contar (fora pausa e carregamento).
const SCAN_TIME: Duration = Duration::from_secs(10);
const SCAN_FRAMES: u32 = 60;

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
    /// Calibração da tela e a base do Automático do jogo (com a análise).
    cal: Hdr,
    base: Hdr,
    /// Automático: o que mudar aqui vira ajuste fino sobre `base`. Fora dele, os ajustes do Manual.
    auto: bool,
    start_auto: bool,
    /// Medida do Analisar em curso e quando começou.
    scanning: Option<(u32, Instant)>,
    /// O que a última análise mediu, ou por que não mediu.
    note: String,
    asking: bool,
    /// Linha do ajuste que as setas mudam; a seguinte à do último ajuste é a dos botões.
    row: usize,
    /// Na linha dos botões, o escolhido (`BUTTONS`); `press` o aperta no próximo desenho.
    col: usize,
    press: bool,
}

/// Botões da linha de baixo: Voltar ao automático, Analisar (só com perfil do jogo) e Fechar.
const BUTTONS: usize = 3;

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
            crate::app::with(|a| a.hdr_menu_watch());
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
        Self {
            surface: Layered::new(hwnd),
            ui: UiState::default(),
            live: None,
            game: None,
            pid: 0,
            name: String::new(),
            start: h,
            now: h,
            cal: h,
            base: h,
            auto: true,
            start_auto: true,
            scanning: None,
            note: String::new(),
            asking: false,
            row: 0,
            col: BUTTONS - 1,
            press: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.live.is_some()
    }

    fn open(&mut self, live: HdrLive, game: Option<(usize, &HdrGame)>, pid: u32, name: String, cal: Hdr) {
        self.start = live.get();
        self.now = self.start;
        self.live = Some(live);
        self.game = game.map(|g| g.0);
        self.pid = pid;
        self.name = name;
        self.cal = cal;
        self.base = game.map_or(cal, |g| autohdr::auto_base(cal, g.1));
        self.auto = game.is_none_or(|g| g.1.prisma.is_none());
        self.start_auto = self.auto;
        self.scanning = None;
        self.note = game.and_then(|g| g.1.scan).map_or(String::new(), |s| crate::pages::hdr::scan_text(&s));
        self.asking = false;
        (self.row, self.col, self.press) = (0, BUTTONS - 1, false);
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
        crate::sys::nav::set_menu(true);
    }

    fn hide(&mut self) {
        if let (Some(l), Some(_)) = (&self.live, self.scanning.take()) {
            l.scan_set(0);
        }
        self.live = None;
        crate::sys::nav::set_menu(false);
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
        let h = if self.asking { 150.0 } else { 54.0 + KNOBS_H + 8.0 + 26.0 + 34.0 + PAD };
        // Canto superior esquerdo: o medidor fica no direito.
        let (x, y) = (mon.left + (24.0 * scale) as i32, mon.top + (24.0 * scale) as i32);
        let (mut act, mut now, mut moved, mut to_auto, mut scan) = (None, self.now, false, false, false);
        let (asking, base, auto, can_save, row) = (self.asking, self.base, self.auto, self.game.is_some(), self.row);
        let on_buttons = row == crate::pages::hdr::KNOBS.len();
        let (col, press) = (on_buttons.then_some(self.col), std::mem::take(&mut self.press));
        // O botão escolhido pelas setas ou pelo controle: contorno, e Enter (A) aperta.
        let pick = |u: &mut Ui, i: usize, r: Rect| {
            if col == Some(i) {
                u.c.round(rect(r.x - 3.0, r.y - 3.0, r.w + 6.0, r.h + 6.0), 10.0, ACCENT);
            }
            press && col == Some(i)
        };
        let title = format!("{} · {}", crate::i18n::tr("HDR do Prisma"), self.name);
        let note = match (self.scanning, &self.live) {
            (Some((gen, _)), Some(l)) => format!("Analisando: {} quadros", l.scan(gen).map_or(0, |s| s.frames)),
            _ if self.note.is_empty() && can_save => "Sem análise ainda.".to_string(),
            _ => self.note.clone(),
        };
        let scanning = self.scanning.is_some();
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
                u.c.text(rect(PAD, 12.0, W - 2.0 * PAD - 110.0, 26.0), &title, 14.0, WHITE, Align::Left, Weight::Semibold);
                let (way, color) = if auto { ("Automático", ACCENT) } else { ("Manual", TEXT2) };
                u.c.text(rect(W - PAD - 110.0, 12.0, 110.0, 26.0), way, 13.0, color, Align::Right, Weight::Regular);
                if !on_buttons {
                    u.c.round(rect(PAD - 8.0, 54.0 + row as f32 * 48.0 - 6.0, W - 2.0 * PAD + 16.0, 42.0), 8.0, rgb(0x262a33));
                }
                moved = crate::pages::hdr::knobs(&mut u, "hm-k", rect(PAD, 54.0, W - 2.0 * PAD, KNOBS_H), 1, &mut now, auto.then_some(base));
                let by = h - PAD - 34.0;
                u.c.text(rect(PAD, by - 30.0, W - 2.0 * PAD, 24.0), &note, 12.0, TEXT2, Align::Left, Weight::Regular);
                let r = rect(PAD, by, 180.0, 34.0);
                if pick(&mut u, 0, r) | u.button("hm-cal", r, "Voltar ao automático", false) {
                    now = Hdr { on: now.on, ..base };
                    (moved, to_auto) = (true, true);
                }
                let r = rect(PAD + 190.0, by, 120.0, 34.0);
                if can_save && (pick(&mut u, 1, r) | u.button("hm-scan", r, if scanning { "Analisando" } else { "Analisar" }, false)) {
                    scan = !scanning;
                }
                let r = rect(W - PAD - 110.0, by, 110.0, 34.0);
                if pick(&mut u, 2, r) | u.button("hm-close", r, "Fechar", true) {
                    act = Some(Act::Close);
                }
            }
            u.end();
        });
        if moved {
            self.set(now);
        }
        self.auto |= to_auto;
        if let (true, Some(l)) = (scan, &self.live) {
            // Número novo a cada medida, também entre aberturas do Prisma: o gancho recomeça a soma.
            let gen = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_millis() as u32) | 1;
            l.scan_set(gen);
            self.scanning = Some((gen, Instant::now()));
        }
        // Sem perfil no config não há o que perguntar.
        if matches!(act, Some(Act::Close)) && !can_save {
            act = None;
            self.hide();
        }
        act
    }

    fn changed(&self) -> bool {
        self.now != self.start || self.auto != self.start_auto
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
        let game = self.cfg.hdr_games.iter().enumerate().find(|g| g.1.exe.eq_ignore_ascii_case(&exe));
        // A calibração da tela onde o jogo está, a mesma que a aba HDR mostra para ela.
        let cal = autohdr::calibrated(&self.cfg, &crate::overlays::monitor_name(front));
        let name = crate::pages::app_name(&exe).to_string();
        self.hdr_menu.open(live, game, pid, name, cal);
    }

    /// Meio segundo: o menu some sem o jogo na frente e o Analisar mostra quantos quadros já somou. No
    /// fim guarda a medida no perfil do jogo e refaz o Automático com ela, mantendo o ajuste fino.
    pub(crate) fn hdr_menu_watch(&mut self) {
        let m = &mut self.hdr_menu;
        m.watch();
        let (Some((gen, since)), Some(live)) = (m.scanning, &m.live) else { return };
        if since.elapsed() < SCAN_TIME {
            m.redraw();
            return;
        }
        let got = live.scan(gen).filter(|s| s.frames >= SCAN_FRAMES);
        live.scan_set(0);
        m.scanning = None;
        match (got, m.game.and_then(|i| self.cfg.hdr_games.get_mut(i))) {
            (Some(sc), Some(g)) => {
                g.scan = Some(sc);
                let base = autohdr::auto_base(m.cal, g);
                let now = if m.auto { autohdr::tuned(base, &autohdr::tune_of(m.base, m.now)) } else { Hdr { src_peak: base.src_peak, src_black: base.src_black, src_paper: base.src_paper, ..m.now } };
                let quiet = !m.changed();
                (m.base, m.note) = (base, crate::pages::hdr::scan_text(&sc));
                m.set(now);
                // Sem mudança pendente a medida já vai para o disco; com ela, vai junto ao salvar ou descartar.
                if quiet {
                    m.start = m.now;
                    self.cfg.save();
                }
            }
            _ => m.note = "Não deu para medir: jogue uns segundos com a cena andando e tente de novo.".to_string(),
        }
        m.redraw();
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

    /// Esc fecha; Enter fecha ou, na linha dos botões, aperta o escolhido. Na pergunta, Enter salva e
    /// Esc descarta. Setas: para cima e para baixo a linha (ajustes e, por último, os botões), para os
    /// lados o valor ou o botão.
    fn hdr_menu_keydown(&mut self, k: VIRTUAL_KEY) {
        let m = &mut self.hdr_menu;
        let n = crate::pages::hdr::KNOBS.len();
        // Sem perfil do jogo não há Analisar.
        let scan = m.game.is_some();
        match (k, m.asking) {
            (VK_RETURN, true) => return self.hdr_menu_act(Act::Save),
            (VK_ESCAPE, true) => return self.hdr_menu_act(Act::Discard),
            (VK_RETURN, false) if m.row == n => m.press = true,
            (VK_ESCAPE | VK_RETURN, false) => return self.hdr_menu_close(),
            (VK_UP, false) => m.row = (m.row + n) % (n + 1),
            (VK_DOWN, false) => m.row = (m.row + 1) % (n + 1),
            (VK_LEFT, false) if m.row == n => m.col = if m.col == BUTTONS - 1 && scan { 1 } else { 0 },
            (VK_RIGHT, false) if m.row == n => m.col = if m.col == 0 && scan { 1 } else { BUTTONS - 1 },
            (VK_LEFT, false) => m.step(-1.0),
            (VK_RIGHT, false) => m.step(1.0),
            _ => return,
        }
        if let Some(act) = m.redraw() {
            self.hdr_menu_act(act);
        }
    }

    /// O controle no menu: o direcional faz as setas, A o Enter e B o Esc.
    pub(crate) fn hdr_menu_pad(&mut self, c: crate::sys::nav::Cmd) {
        use crate::sys::nav::Cmd;
        if !self.hdr_menu.is_open() {
            return;
        }
        let k = match c {
            Cmd::Move(d) => [VK_LEFT, VK_UP, VK_RIGHT, VK_DOWN][d as usize & 3],
            Cmd::Ok => VK_RETURN,
            Cmd::Back => VK_ESCAPE,
            _ => return,
        };
        self.hdr_menu_keydown(k);
    }

    fn hdr_menu_act(&mut self, act: Act) {
        match act {
            Act::Close => self.hdr_menu_close(),
            Act::Save => {
                let m = &mut self.hdr_menu;
                if let Some(g) = m.game.and_then(|i| self.cfg.hdr_games.get_mut(i)) {
                    if m.auto {
                        (g.prisma, g.tune) = (None, autohdr::tune_of(m.base, m.now));
                    } else {
                        g.prisma = Some(m.now);
                    }
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
                // Uma análise feita com mudanças pendentes fica no perfil.
                self.cfg.save();
            }
        }
    }
}
