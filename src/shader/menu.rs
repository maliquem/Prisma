//! Menu dentro do jogo: ligar e desligar, trocar de perfil, escolher um preset (os do jogador em `shaders\` ou
//! a biblioteca, pasta por pasta), passes com liga e desliga e os parâmetros do passe escolhido. Janela
//! transparente acima da janela por cima, sem foco: enquanto está aberto, ↑↓←→ Enter Esc viram atalhos globais (soltos ao fechar) e o controle XInput navega (Select+L1 abre e fecha, direcional, A, B).
//! Cada mudança grava no perfil, então a página do app vê na hora.

use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Input::XboxController::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use prisma_shader::chain::Engine;
use crate::shader_profile::Param;
use crate::ui::gfx::{self, rect, Align, Weight};
use crate::ui::layered::{create_overlay, Layered};

const HK_BASE: i32 = 100;
const NAV: [VIRTUAL_KEY; 6] = [VK_UP, VK_DOWN, VK_LEFT, VK_RIGHT, VK_RETURN, VK_ESCAPE];

#[derive(Clone, Copy, PartialEq)]
enum Key {
    Up,
    Down,
    Left,
    Right,
    Ok,
    Back,
}
const KEYS: [Key; 6] = [Key::Up, Key::Down, Key::Left, Key::Right, Key::Ok, Key::Back];

const W: f32 = 380.0;
const ROW: f32 = 28.0;
const VISIBLE: usize = 14;
/// Linhas fixas do menu principal (Shader, Perfil, Presets, Biblioteca); os passes vêm depois.
const FIRST_PASS: usize = 4;

#[derive(Clone, Copy, PartialEq)]
enum View {
    Main,
    /// Parâmetros do passe i.
    Params(usize),
    /// Lista de presets: `true` na biblioteca (com pastas), `false` nos do jogador.
    Files(bool),
}

pub struct Menu {
    layer: Option<Layered>,
    pub open: bool,
    keys_on: bool,
    sel: usize,
    view: View,
    /// Pasta e itens da lista de presets aberta, com `true` nas pastas (o menu redesenha a cada quadro).
    dir: std::path::PathBuf,
    files: Vec<(std::path::PathBuf, bool)>,
    params: Vec<Param>,
    params_of: Vec<std::path::PathBuf>,
    pad_prev: u16,
    nav: [super::Press; 6],
    shown: bool,
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    DefWindowProcW(h, m, w, l)
}

impl Menu {
    pub fn new() -> Self {
        // `--shader-menu`: o app abriu o motor pelo atalho do menu, que já vem aberto.
        let open = std::env::args().any(|a| a == "--shader-menu");
        Self { layer: None, open, keys_on: false, sel: 0, view: View::Main, dir: Default::default(), files: vec![], params: vec![], params_of: vec![], pad_prev: 0, nav: Default::default(), shown: false }
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
        (self.sel, self.view) = (0, View::Main);
    }

    /// Põe o menu de volta acima da janela por cima (que acabou de aparecer).
    pub fn raise(&self) {
        if let (true, Some(l)) = (self.shown, &self.layer) {
            unsafe {
                let _ = SetWindowPos(l.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }

    /// ↑↓←→ Enter Esc com o menu aberto e o jogo em foco, pelo estado da tecla (`Press`): os atalhos de
    /// `set_keys` só seguram a tecla longe do jogo, onde ele deixa.
    pub fn keys(&mut self, fg: bool, eng: &mut Engine, dev: &ID3D11Device) {
        for (i, (vk, k)) in NAV.iter().zip(KEYS).enumerate() {
            let down = fg && self.open && unsafe { GetAsyncKeyState(vk.0 as i32) } < 0;
            if self.nav[i].hit(down, matches!(k, Key::Up | Key::Down | Key::Left | Key::Right)) {
                self.key(k, eng, dev);
            }
        }
    }

    /// Controle: Select+L1 abre e fecha; com o menu aberto, direcional, A (confirma) e B (volta).
    /// shortcut: o jogo também recebe esses botões (o XInput não deixa segurar), por isso o par Select+L1.
    pub fn pad(&mut self, eng: &mut Engine, dev: &ID3D11Device) {
        let b = (0..4)
            .find_map(|i| {
                let mut st = XINPUT_STATE::default();
                (unsafe { XInputGetState(i, &mut st) } == 0).then_some(st.Gamepad.wButtons.0)
            })
            .unwrap_or(0);
        let new = b & !self.pad_prev;
        self.pad_prev = b;
        let combo = XINPUT_GAMEPAD_BACK.0 | XINPUT_GAMEPAD_LEFT_SHOULDER.0;
        if b & combo == combo && new & combo != 0 {
            self.toggle();
            return;
        }
        if !self.open {
            return;
        }
        let map = [
            (XINPUT_GAMEPAD_DPAD_UP, Key::Up),
            (XINPUT_GAMEPAD_DPAD_DOWN, Key::Down),
            (XINPUT_GAMEPAD_DPAD_LEFT, Key::Left),
            (XINPUT_GAMEPAD_DPAD_RIGHT, Key::Right),
            (XINPUT_GAMEPAD_A, Key::Ok),
            (XINPUT_GAMEPAD_B, Key::Back),
        ];
        for (btn, k) in map {
            if new & btn.0 != 0 {
                self.key(k, eng, dev);
            }
        }
    }

    fn key(&mut self, k: Key, eng: &mut Engine, dev: &ID3D11Device) {
        if !self.open {
            return;
        }
        let rows = self.rows(eng);
        match (k, self.view) {
            (Key::Up, _) => self.sel = (self.sel + rows.max(1) - 1) % rows.max(1),
            (Key::Down, _) => self.sel = (self.sel + 1) % rows.max(1),
            (Key::Back, View::Main) => self.toggle(),
            (Key::Back, View::Params(i)) => (self.view, self.sel) = (View::Main, FIRST_PASS + i),
            // Na biblioteca sobe uma pasta, com a de onde veio escolhida; da raiz volta ao menu.
            (Key::Back, View::Files(lib)) => match self.dir.parent().filter(|_| lib && self.dir != crate::shader_profile::lib_dir()).map(|d| d.to_path_buf()) {
                Some(up) => {
                    let from = self.dir.clone();
                    self.list(lib, up);
                    self.sel = self.files.iter().position(|(f, _)| *f == from).unwrap_or(0);
                }
                None => (self.view, self.sel) = (View::Main, if lib { 3 } else { 2 }),
            },
            (_, View::Main) => match self.sel {
                0 => eng.edit(dev, |p| p.on = !p.on),
                1 => eng.next(dev, if k == Key::Left { -1 } else { 1 }),
                2 if k == Key::Ok => self.list(false, crate::shader_profile::root()),
                3 if k == Key::Ok => self.list(true, crate::shader_profile::lib_dir()),
                2 | 3 => {}
                s if k == Key::Ok => (self.view, self.sel) = (View::Params(s - FIRST_PASS), 0),
                s => eng.edit(dev, |p| p.passes[s - FIRST_PASS].on ^= true),
            },
            (Key::Ok, View::Files(lib)) => match self.files.get(self.sel).cloned() {
                Some((d, true)) => self.list(lib, d),
                // O preset escolhido toma o lugar dos passes, como o Carregar preset da página.
                Some((f, false)) => {
                    eng.edit(dev, |p| {
                        p.passes.clear();
                        p.params.clear();
                        crate::shader_profile::add_pass(p, f);
                    });
                    (self.view, self.sel) = (View::Main, FIRST_PASS);
                }
                None => {}
            },
            (_, View::Files(_)) => {}
            (_, View::Params(i)) => {
                let Some(p) = self.params.iter().filter(|p| p.pass == i).nth(self.sel).cloned() else { return };
                let cur = eng.profile.param(&p.name).unwrap_or(p.default);
                let v = match k {
                    Key::Left => (cur - p.step).max(p.min),
                    Key::Right => (cur + p.step).min(p.max),
                    _ => p.default,
                };
                // De volta ao padrão some do arquivo, como na página do app.
                let v = (((v - p.default) / p.step.max(1e-6)).abs() > 0.01).then_some(v);
                eng.edit(dev, |pr| pr.set_param(&p.name, v));
            }
        }
    }

    fn rows(&self, eng: &Engine) -> usize {
        match self.view {
            View::Main => FIRST_PASS + eng.profile.passes.len(),
            View::Params(i) => self.params.iter().filter(|p| p.pass == i).count(),
            View::Files(_) => self.files.len(),
        }
    }

    /// Abre a lista de presets em `dir`: pastas primeiro (só na biblioteca, menos as `shaders` de cada uma, que
    /// têm os passes soltos), depois os .slangp (e os .slang da biblioteca).
    fn list(&mut self, lib: bool, dir: std::path::PathBuf) {
        let ext = |p: &std::path::Path, e: &str| p.extension().is_some_and(|x| x.eq_ignore_ascii_case(e));
        let mut v: Vec<(std::path::PathBuf, bool)> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| (e.path(), e.file_type().is_ok_and(|t| t.is_dir())))
            .filter(|(p, d)| if *d { lib && !p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("shaders")) } else { ext(p, "slangp") || (lib && ext(p, "slang")) })
            .collect();
        v.sort_by_cached_key(|(p, d)| (!d, p.file_stem().map(|n| n.to_string_lossy().to_lowercase())));
        (self.view, self.dir, self.files, self.sel) = (View::Files(lib), dir, v, 0);
    }

    /// Mostra, esconde e redesenha. Menu aberto com o jogo fora de foco fecha: as setas não ficam presas.
    pub fn update(&mut self, fg: bool, at: Option<POINT>, eng: &mut Engine) {
        if self.open && !fg {
            self.open = false;
        }
        self.set_keys(self.open);
        let Some(at) = at.filter(|_| self.open) else {
            if self.shown {
                if let Some(l) = &self.layer {
                    unsafe {
                        let _ = ShowWindow(l.hwnd, SW_HIDE);
                    }
                }
                self.shown = false;
            }
            return;
        };
        let paths: Vec<_> = eng.profile.passes.iter().map(|p| p.path.clone()).collect();
        if paths != self.params_of {
            self.params = crate::shader_profile::params(&eng.profile);
            self.params_of = paths;
        }
        self.sel = self.sel.min(self.rows(eng).saturating_sub(1));
        if matches!(self.view, View::Params(i) if i >= eng.profile.passes.len()) {
            (self.view, self.sel) = (View::Main, 0);
        }
        let lines = self.lines(eng);
        let layer = self.layer.get_or_insert_with(|| {
            crate::overlays::register(w!("PrismaShaderMenu"), wndproc);
            let l = Layered::new(create_overlay(w!("PrismaShaderMenu"), true));
            unsafe {
                // O menu nunca entra em gravação nem transmissão.
                let _ = SetWindowDisplayAffinity(l.hwnd, WDA_EXCLUDEFROMCAPTURE);
            }
            l
        });
        let h = 52.0 + lines.len().min(VISIBLE) as f32 * ROW + 30.0;
        let first = self.sel.saturating_sub(VISIBLE - 1);
        let (sel, title) = (self.sel, match self.view {
            View::Main => format!("Shaders · {}", eng.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()),
            View::Params(i) => eng.profile.passes.get(i).map(|p| stem(&p.path)).unwrap_or_default(),
            View::Files(false) => "Presets".into(),
            View::Files(true) => match self.dir.strip_prefix(crate::shader_profile::lib_dir()) {
                Ok(r) if !r.as_os_str().is_empty() => format!("Biblioteca · {}", r.display()),
                _ => "Biblioteca".into(),
            },
        });
        let hint = match self.view {
            View::Main => "↑↓ escolher · ←→ mudar · Enter abrir · Esc fechar",
            View::Params(_) => "←→ ajustar · Enter padrão · Esc voltar",
            View::Files(_) => "↑↓ escolher · Enter abrir · Esc voltar",
        };
        layer.paint(at.x + 24, at.y + 24, W, h, |c, r| {
            c.round(r, 10.0, gfx::BG.alpha(0.94));
            c.round_line(r.inset(0.5), 10.0, gfx::BORDER, 1.0);
            c.text(rect(16.0, 12.0, W - 32.0, 26.0), &title, 15.0, gfx::TEXT, Align::Left, Weight::Semibold);
            for (i, (label, value, changed)) in lines.iter().enumerate().skip(first).take(VISIBLE) {
                let y = 46.0 + (i - first) as f32 * ROW;
                if i == sel {
                    c.round(rect(8.0, y, W - 16.0, ROW), 6.0, gfx::CARD2);
                }
                let col = if i == sel { gfx::ACCENT } else { gfx::TEXT2 };
                c.text_fit(rect(16.0, y, W - 130.0, ROW), label, 13.0, col, Align::Left, Weight::Regular);
                c.text(rect(W - 124.0, y, 108.0, ROW), value, 13.0, if *changed { gfx::WARN } else { col }, Align::Right, Weight::Regular);
            }
            c.text_fit(rect(16.0, h - 30.0, W - 32.0, 22.0), hint, 11.0, gfx::MUTED, Align::Left, Weight::Regular);
        });
        if !self.shown {
            unsafe {
                let _ = SetWindowPos(layer.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
            }
            self.shown = true;
        }
    }

    /// (rótulo, valor, diferente do padrão) de cada linha da tela atual.
    fn lines(&self, eng: &Engine) -> Vec<(String, String, bool)> {
        let p = &eng.profile;
        let onoff = |b: bool| if b { "ligado" } else { "desligado" }.to_string();
        match self.view {
            View::Main => {
                let mut v = vec![("Shader".into(), onoff(p.on), false), ("Perfil".into(), format!("◂ {} ▸", stem(&eng.path)), false), ("Presets".into(), "▸".into(), false), ("Biblioteca".into(), "▸".into(), false)];
                v.extend(p.passes.iter().map(|x| (stem(&x.path), onoff(x.on), false)));
                v
            }
            View::Files(lib) if self.files.is_empty() => {
                let why = if lib && !crate::shader_profile::lib_dir().exists() { "Baixe a biblioteca na página Shaders" } else { "Nenhum preset aqui" };
                vec![(why.into(), String::new(), false)]
            }
            View::Files(_) => self.files.iter().map(|(f, d)| if *d { (f.file_name().unwrap_or_default().to_string_lossy().into_owned(), "▸".into(), false) } else { (stem(f), String::new(), false) }).collect(),
            View::Params(i) => self
                .params
                .iter()
                .filter(|x| x.pass == i)
                .map(|x| {
                    let v = p.param(&x.name);
                    let label = if x.desc.trim().is_empty() { x.name.clone() } else { x.desc.trim().to_string() };
                    (label, format!("{:.3}", v.unwrap_or(x.default)).trim_end_matches('0').trim_end_matches('.').to_string(), v.is_some())
                })
                .collect(),
        }
    }

    fn set_keys(&mut self, on: bool) {
        if on == self.keys_on {
            return;
        }
        self.keys_on = on;
        for (i, vk) in NAV.iter().enumerate() {
            unsafe {
                if on {
                    // Sem MOD_NOREPEAT: segurar a seta repete.
                    let _ = RegisterHotKey(None, HK_BASE + i as i32, HOT_KEY_MODIFIERS(0), vk.0 as u32);
                } else {
                    let _ = UnregisterHotKey(None, HK_BASE + i as i32);
                }
            }
        }
    }
}

impl Drop for Menu {
    fn drop(&mut self) {
        self.set_keys(false);
        if let Some(l) = self.layer.take() {
            let h = l.hwnd;
            drop(l);
            unsafe {
                let _ = DestroyWindow(h);
            }
        }
    }
}

fn stem(p: &std::path::Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}
