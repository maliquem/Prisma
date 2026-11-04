//! Interface imediata desenhada só quando algo muda: cada redesenho descreve a tela
//! e os controles respondem ao mouse nesse mesmo passe.

pub mod gfx;
pub mod icons;
pub mod layered;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::config::Hotkey;
use gfx::*;

fn hash_id(key: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    h.finish() | 1
}

struct Popup {
    id: u64,
    rect: Rect,
    options: Vec<String>,
    selected: usize,
}

#[derive(Default)]
pub struct UiState {
    pub mx: f32,
    pub my: f32,
    pub down: bool,
    pub pressed: bool,
    pub released: bool,
    pub wheel: f32,
    active: u64,
    popup: Option<Popup>,
    popup_pick: Option<(u64, usize)>,
    pub capturing: u64,
    pub key_event: Option<Hotkey>,
    pub key_cancel: bool,
    scroll: HashMap<u64, f32>,
    pub needs_redraw: bool,
    icons: HashMap<String, Option<windows::Win32::Graphics::Direct2D::ID2D1Bitmap>>,
    /// Campo de texto com o foco, e o que foi digitado desde o último desenho.
    pub focus: u64,
    pub typed: String,
    pub backspace: u32,
}

impl UiState {
    fn end_frame(&mut self) {
        self.pressed = false;
        self.released = false;
        self.wheel = 0.0;
        self.key_event = None;
        self.key_cancel = false;
    }
}

pub struct Ui<'a> {
    pub c: &'a Canvas,
    pub s: &'a mut UiState,
    dy: f32,
    blocked: bool,
    clip: Option<Rect>,
}

impl<'a> Ui<'a> {
    pub fn begin(c: &'a Canvas, s: &'a mut UiState) -> Self {
        let blocked = s.popup.is_some();
        Self { c, s, dy: 0.0, blocked, clip: None }
    }

    fn hover(&self, r: Rect) -> bool {
        if self.blocked {
            return false;
        }
        if let Some(c) = self.clip {
            if !c.contains(self.s.mx, self.s.my) {
                return false;
            }
        }
        r.contains(self.s.mx, self.s.my + self.dy)
    }

    /// Devolve true no clique que começou e terminou sobre este controle.
    fn clickable(&mut self, id: u64, r: Rect) -> bool {
        let hover = self.hover(r);
        if hover && self.s.pressed {
            self.s.active = id;
        }
        let clicked = hover && self.s.released && self.s.active == id;
        if self.s.released && self.s.active == id {
            self.s.active = 0;
        }
        clicked
    }

    pub fn is_hover(&self, r: Rect) -> bool {
        self.hover(r)
    }

    /// Área rolável. O conteúdo desenhado depois disso usa coordenadas sem rolagem.
    pub fn begin_scroll(&mut self, key: &str, r: Rect, content_h: f32) {
        let id = hash_id(key);
        let max = (content_h - r.h).max(0.0);
        let mut v = self.s.scroll.get(&id).copied().unwrap_or(0.0);
        if !self.blocked && r.contains(self.s.mx, self.s.my) && self.s.wheel != 0.0 {
            v -= self.s.wheel * 72.0;
            self.s.needs_redraw = true;
        }
        v = v.clamp(0.0, max);
        self.s.scroll.insert(id, v);
        self.dy = v;
        self.clip = Some(r);
        self.c.push_clip(r);
        self.c.translate(0.0, -v);
    }

    pub fn end_scroll(&mut self) {
        self.c.translate(0.0, 0.0);
        self.c.pop_clip();
        self.dy = 0.0;
        self.clip = None;
    }

    /// Volta uma lista ao topo, ao trocar de pasta ou de aba.
    pub fn reset_scroll(&mut self, key: &str) {
        self.s.scroll.remove(&hash_id(key));
    }

    pub fn scroll_y(&self) -> f32 {
        self.dy
    }

    pub fn card(&self, r: Rect) {
        self.c.round(r, 12.0, CARD);
    }

    pub fn card_on(&self, r: Rect) {
        self.c.round(r, 12.0, CARD2);
    }

    pub fn title(&self, r: Rect, s: &str) {
        self.c.text(r, s, 15.0, TEXT, Align::Left, Weight::Semibold);
    }

    pub fn label(&self, r: Rect, s: &str, color: Color) {
        self.c.text(r, s, 13.0, color, Align::Left, Weight::Regular);
    }

    pub fn button(&mut self, key: &str, r: Rect, label: &str, primary: bool) -> bool {
        self.button_ex(key, r, label, primary, true)
    }

    pub fn button_ex(&mut self, key: &str, r: Rect, label: &str, primary: bool, on: bool) -> bool {
        let id = hash_id(key);
        let hover = on && self.hover(r);
        let clicked = on && self.clickable(id, r);
        if primary {
            let bg = if !on {
                LINE
            } else if hover {
                rgb(0xa9d9ff)
            } else {
                ACCENT
            };
            self.c.round(r, 8.0, bg);
            let fg = if on { ON_ACCENT } else { MUTED };
            self.c.text(r, label, 14.0, fg, Align::Center, Weight::Semibold);
        } else {
            if hover {
                self.c.round(r, 8.0, CARD2);
            }
            self.c.round_line(r, 8.0, if on { BORDER2 } else { BORDER }, 1.0);
            self.c.text(r, label, 14.0, if on { TEXT } else { MUTED }, Align::Center, Weight::Regular);
        }
        clicked
    }

    pub fn check(&mut self, key: &str, r: Rect, label: &str, val: &mut bool) -> bool {
        let id = hash_id(key);
        let clicked = self.clickable(id, r);
        if clicked {
            *val = !*val;
        }
        let bx = rect(r.x, r.y + (r.h - 18.0) / 2.0, 18.0, 18.0);
        if *val {
            self.c.round(bx, 4.0, ACCENT);
            self.c.icon(bx.x, bx.y, 18.0, &[&[(6.0, 12.5), (10.0, 16.5), (18.0, 7.5)]], ON_ACCENT);
        } else {
            self.c.round_line(bx, 4.0, BORDER2, 1.0);
        }
        let t = rect(r.x + 28.0, r.y, r.w - 28.0, r.h);
        self.c.text(t, label, 14.0, TEXT, Align::Left, Weight::Regular);
        clicked
    }

    pub fn select(&mut self, key: &str, r: Rect, options: &[String], sel: &mut usize) -> bool {
        let id = hash_id(key);
        let mut changed = false;
        if let Some((pid, v)) = self.s.popup_pick {
            if pid == id {
                *sel = v;
                changed = true;
                self.s.popup_pick = None;
            }
        }
        let hover = self.hover(r);
        if self.clickable(id, r) {
            let h = options.len() as f32 * 36.0 + 8.0;
            let y = r.y - self.dy;
            let top = y + r.h + 4.0;
            self.s.popup = Some(Popup {
                id,
                rect: rect(r.x, top, r.w, h),
                options: options.to_vec(),
                selected: *sel,
            });
            self.s.needs_redraw = true;
        }
        self.c.round(r, 8.0, SIDE);
        self.c.round_line(r, 8.0, if hover { BORDER2 } else { BORDER }, 1.0);
        let label = options.get(*sel).map(String::as_str).unwrap_or("");
        let t = rect(r.x + 10.0, r.y, r.w - 34.0, r.h);
        self.c.text(t, label, 14.0, TEXT, Align::Left, Weight::Regular);
        let (ax, ay) = (r.right() - 24.0, r.y + r.h / 2.0 - 2.0);
        self.c.line(ax, ay, ax + 5.0, ay + 5.0, MUTED, 1.5);
        self.c.line(ax + 5.0, ay + 5.0, ax + 10.0, ay, MUTED, 1.5);
        changed
    }

    pub fn slider(&mut self, key: &str, r: Rect, val: &mut f32) -> bool {
        let id = hash_id(key);
        let hover = self.hover(r);
        if hover && self.s.pressed {
            self.s.active = id;
        }
        let mut changed = false;
        if self.s.active == id {
            *val = ((self.s.mx - r.x) / r.w).clamp(0.0, 1.0);
            changed = true;
            if self.s.released || !self.s.down {
                self.s.active = 0;
            }
        }
        let cy = r.y + r.h / 2.0;
        self.c.round(rect(r.x, cy - 2.0, r.w, 4.0), 2.0, BORDER);
        self.c.round(rect(r.x, cy - 2.0, r.w * *val, 4.0), 2.0, ACCENT);
        self.c.circle(r.x + r.w * *val, cy, 7.0, if hover { WHITE } else { ACCENT });
        changed
    }

    /// Campo que captura a próxima combinação de teclas. Esc cancela e Backspace limpa.
    pub fn hotkey(&mut self, key: &str, r: Rect, hk: &mut Hotkey) -> bool {
        let id = hash_id(key);
        let mut changed = false;
        if self.clickable(id, r) {
            self.s.capturing = id;
            self.s.needs_redraw = true;
        }
        let active = self.s.capturing == id;
        if active {
            if let Some(k) = self.s.key_event {
                *hk = k;
                changed = true;
                self.s.capturing = 0;
            } else if self.s.key_cancel {
                self.s.capturing = 0;
            }
        }
        self.c.round(r, 8.0, SIDE);
        self.c.round_line(r, 8.0, if active { ACCENT } else { BORDER }, 1.0);
        let txt = if active { "Pressione as teclas".to_string() } else { hk.label() };
        let color = if active { ACCENT } else { TEXT };
        self.c.text(r.inset_xy(10.0, 0.0), &txt, 14.0, color, Align::Left, Weight::Regular);
        changed
    }

    /// Ícone do programa em `path`. Sem ícone, desenha a inicial de `name`.
    pub fn file_icon(&mut self, path: &str, name: &str, r: Rect) {
        let key = path.to_ascii_lowercase();
        if !self.s.icons.contains_key(&key) {
            let b = icons::load(path).and_then(|(w, h, px)| self.c.bitmap(w, h, &px));
            self.s.icons.insert(key.clone(), b);
        }
        match self.s.icons.get(&key).and_then(|b| b.as_ref()) {
            Some(b) => self.c.draw_bitmap(b, r),
            None => {
                self.c.round(r, 6.0, LINE);
                let l: String = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                self.c.text(r, &l, 11.0, TEXT2, Align::Center, Weight::Semibold);
            }
        }
    }

    /// Campo de busca. Clique dá o foco; digitação chega pela janela.
    pub fn search(&mut self, key: &str, r: Rect, text: &mut String, hint: &str) -> bool {
        let id = hash_id(key);
        if self.clickable(id, r) {
            self.s.focus = id;
        } else if self.s.released && !self.hover(r) && self.s.focus == id {
            self.s.focus = 0;
        }
        let on = self.s.focus == id;
        let mut changed = false;
        if on {
            if !self.s.typed.is_empty() {
                text.push_str(&self.s.typed);
                self.s.typed.clear();
                changed = true;
            }
            for _ in 0..self.s.backspace {
                changed |= text.pop().is_some();
            }
            self.s.backspace = 0;
        }
        self.c.round(r, 8.0, SIDE);
        self.c.round_line(r, 8.0, if on { ACCENT } else { BORDER }, 1.0);
        let t = r.inset_xy(10.0, 0.0);
        if text.is_empty() && !on {
            self.c.text(t, hint, 14.0, MUTED, Align::Left, Weight::Regular);
        } else {
            let shown = if on { format!("{text}|") } else { text.clone() };
            self.c.text(t, &shown, 14.0, TEXT, Align::Left, Weight::Regular);
        }
        changed
    }

    /// Botão com área invisível, para linhas clicáveis.
    pub fn hit(&mut self, key: &str, r: Rect) -> bool {
        self.clickable(hash_id(key), r)
    }

    pub fn end(mut self) {
        self.c.translate(0.0, 0.0);
        let mut close = false;
        let mut pick = None;
        if let Some(p) = &self.s.popup {
            let shadow = rect(p.rect.x, p.rect.y, p.rect.w, p.rect.h);
            self.c.round(shadow, 10.0, CARD2);
            self.c.round_line(shadow, 10.0, BORDER2, 1.0);
            for (i, o) in p.options.iter().enumerate() {
                let r = rect(p.rect.x + 4.0, p.rect.y + 4.0 + i as f32 * 36.0, p.rect.w - 8.0, 36.0);
                let hover = r.contains(self.s.mx, self.s.my);
                if hover {
                    self.c.round(r, 6.0, BORDER);
                }
                let color = if i == p.selected { ACCENT } else { TEXT };
                self.c.text(r.inset_xy(8.0, 0.0), o, 14.0, color, Align::Left, Weight::Regular);
                if hover && self.s.released {
                    pick = Some((p.id, i));
                }
            }
            if self.s.released {
                close = true;
            }
        }
        if close {
            self.s.popup = None;
            self.s.popup_pick = pick;
            self.s.needs_redraw = true;
        }
        self.blocked = false;
        self.s.end_frame();
    }
}
