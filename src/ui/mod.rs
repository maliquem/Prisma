//! Interface imediata desenhada só quando algo muda: cada redesenho descreve a tela
//! e os controles respondem ao mouse nesse mesmo passe.

pub mod edit;
pub mod gfx;
pub mod icons;
pub mod layered;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::config::Hotkey;
use gfx::*;

/// Faixa da barra de rolagem, na margem à direita da lista.
const BAR_GAP: f32 = 2.0;
const BAR_W: f32 = 8.0;

/// Deixa um campo pronto para digitar.
pub fn focus_on(s: &mut UiState, key: &str) {
    s.focus = hash_id(key);
}

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
    /// Opções de menu que aparecem apagadas e não respondem.
    off: Vec<bool>,
}

/// Pergunta no meio da tela: nada atrás dela responde até escolher.
struct Ask {
    id: u64,
    title: String,
    body: String,
    /// Só avisa: mostra OK em vez de Sim e Não.
    notice: bool,
    /// Caixa de marcar abaixo do texto: rótulo e estado.
    check: Option<(String, bool)>,
}

struct Bar {
    r: Rect,
    y: f32,
    h: f32,
    id: u64,
}

#[derive(Default)]
pub struct UiState {
    pub mx: f32,
    pub my: f32,
    pub down: bool,
    pub pressed: bool,
    pub released: bool,
    pub right_released: bool,
    pub wheel: f32,
    active: u64,
    popup: Option<Popup>,
    /// A lista acabou de abrir: o clique que a abriu não pode fechá-la.
    popup_new: bool,
    pub view_h: f32,
    pub view_w: f32,
    ask: Option<Ask>,
    ask_new: bool,
    answer: Option<u64>,
    /// Estado da caixa da pergunta que acabou de fechar.
    ask_mark: Option<(u64, bool)>,
    grab: f32,
    popup_pick: Option<(u64, usize)>,
    pub capturing: u64,
    pub key_event: Option<Hotkey>,
    pub key_cancel: bool,
    scroll: HashMap<u64, f32>,
    scroll_max: HashMap<u64, f32>,
    stick: u64,
    pub needs_redraw: bool,
    icons: HashMap<String, Option<windows::Win32::Graphics::Direct2D::ID2D1Bitmap>>,
    /// Campo de texto com o foco, e o que foi digitado desde o último desenho.
    pub focus: u64,
    pub typed: String,
    pub backspace: u32,
    /// Ctrl+Backspace: apaga a última palavra.
    pub kill_word: u32,
    /// Setas, Enter, Esc e Ctrl+C com um campo de texto em foco: (tecla, Ctrl apertado).
    pub nav: Vec<(u32, u8)>,
}

impl UiState {
    /// Há lista ou pergunta na frente: janelas filhas do Windows ficariam por cima delas.
    pub fn covered(&self) -> bool {
        self.popup.is_some() || self.ask.is_some()
    }

    /// Fecha a pergunta aberta como se a resposta fosse não.
    pub fn close_ask(&mut self) -> bool {
        self.ask.take().is_some()
    }

    fn end_frame(&mut self) {
        self.pressed = false;
        self.released = false;
        self.right_released = false;
        self.wheel = 0.0;
        self.key_event = None;
        self.key_cancel = false;
        self.nav.clear();
    }
}

pub struct Ui<'a> {
    pub c: &'a Canvas,
    pub s: &'a mut UiState,
    dy: f32,
    blocked: bool,
    clip: Option<Rect>,
    bar: Option<Bar>,
    tip: Option<(Rect, String)>,
}

impl<'a> Ui<'a> {
    pub fn begin(c: &'a Canvas, s: &'a mut UiState) -> Self {
        let blocked = s.popup.is_some() || s.ask.is_some();
        Self { c, s, dy: 0.0, blocked, clip: None, bar: None, tip: None }
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
        if self.s.stick == id {
            v = max;
            self.s.stick = 0;
        }
        self.s.scroll_max.insert(id, max);
        if !self.blocked && r.contains(self.s.mx, self.s.my) && self.s.wheel != 0.0 {
            v -= self.s.wheel * 72.0;
            self.s.needs_redraw = true;
        }
        self.bar = None;
        if max > 0.0 {
            let bar_id = id ^ 2;
            let h = (r.h * r.h / content_h).max(32.0);
            let span = r.h - h;
            let top = r.y + span * (v.clamp(0.0, max) / max);
            let over = self.s.mx >= r.right() && self.s.mx <= r.right() + BAR_W + 2.0 && self.s.my >= r.y && self.s.my <= r.bottom();
            if !self.blocked && over && self.s.pressed {
                self.s.active = bar_id;
                // Clicar fora do trilho claro leva a barra até ali.
                let on_thumb = self.s.my >= top && self.s.my <= top + h;
                self.s.grab = if on_thumb { self.s.my - top } else { h / 2.0 };
            }
            if self.s.active == bar_id {
                v = (self.s.my - self.s.grab - r.y) / span * max;
                self.s.needs_redraw = true;
                if self.s.released || !self.s.down {
                    self.s.active = 0;
                }
            }
            v = v.clamp(0.0, max);
            self.bar = Some(Bar { r, y: r.y + span * (v / max), h, id: bar_id });
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
        if let Some(b) = self.bar.take() {
            let hot = self.s.active == b.id;
            let near = !self.blocked
                && self.s.mx >= b.r.right()
                && self.s.mx <= b.r.right() + BAR_W + 2.0
                && self.s.my >= b.r.y
                && self.s.my <= b.r.bottom();
            let color = if hot { ACCENT } else if near { TEXT2 } else { BORDER2 };
            let w = if hot || near { 5.0 } else { 3.0 };
            self.c.round(rect(b.r.right() + BAR_GAP, b.y, w, b.h), w / 2.0, color);
        }
    }

    /// Se a lista está no fim, continua no fim quando crescer. Rolar para cima solta.
    pub fn stick_bottom(&mut self, key: &str) {
        let id = hash_id(key);
        let v = self.s.scroll.get(&id).copied().unwrap_or(0.0);
        if v >= self.s.scroll_max.get(&id).copied().unwrap_or(0.0) - 2.0 {
            self.s.stick = id;
        }
    }

    /// Volta uma lista ao topo, ao trocar de pasta ou de aba.
    pub fn reset_scroll(&mut self, key: &str) {
        self.s.scroll.remove(&hash_id(key));
    }

    /// Leva uma lista direto a um ponto do conteúdo.
    pub fn set_scroll(&mut self, key: &str, y: f32) {
        self.s.scroll.insert(hash_id(key), y);
        self.s.needs_redraw = true;
    }

    /// Dica que aparece com o mouse em cima, por cima de tudo.
    pub fn tip(&mut self, r: Rect, text: &str) {
        if self.hover(r) {
            self.tip = Some((r, text.to_string()));
        }
    }

    /// Rolagem guardada de uma lista, antes de desenhar.
    pub fn scroll_y_of(&self, key: &str) -> f32 {
        self.s.scroll.get(&hash_id(key)).copied().unwrap_or(0.0)
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
            let mut top = y + r.h + 4.0;
            // Sem espaço embaixo, a lista abre para cima.
            if self.s.view_h > 0.0 && top + h > self.s.view_h - 8.0 {
                top = (y - h - 4.0).max(8.0);
            }
            self.s.popup = Some(Popup {
                id,
                rect: rect(r.x, top, r.w, h),
                options: options.to_vec(),
                selected: *sel,
                off: Vec::new(),
            });
            self.s.popup_new = true;
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

    /// Abre um menu onde o mouse está. A escolha chega depois por `menu_pick`.
    pub fn menu(&mut self, key: &str, items: &[(&str, bool)]) {
        let w = 230.0;
        let h = items.len() as f32 * 36.0 + 8.0;
        let mut x = self.s.mx;
        let mut y = self.s.my;
        if self.s.view_w > 0.0 && x + w > self.s.view_w - 8.0 {
            x = self.s.view_w - w - 8.0;
        }
        if self.s.view_h > 0.0 && y + h > self.s.view_h - 8.0 {
            y = (y - h).max(8.0);
        }
        self.s.popup = Some(Popup {
            id: hash_id(key),
            rect: rect(x, y, w, h),
            options: items.iter().map(|i| i.0.to_string()).collect(),
            selected: usize::MAX,
            off: items.iter().map(|i| !i.1).collect(),
        });
        self.s.popup_new = true;
        self.s.needs_redraw = true;
    }

    pub fn menu_pick(&mut self, key: &str) -> Option<usize> {
        let id = hash_id(key);
        match self.s.popup_pick {
            Some((pid, v)) if pid == id => {
                self.s.popup_pick = None;
                Some(v)
            }
            _ => None,
        }
    }

    /// Janela própria de uma página. Enquanto ela estiver aberta, a página chama `block` antes de
    /// desenhar o resto (que deixa de receber cliques) e `sheet` no fim, para desenhar o fundo dela;
    /// os controles desenhados depois de `sheet` voltam a responder.
    #[cfg_attr(not(feature = "stream"), allow(dead_code))]
    pub fn block(&mut self) {
        self.blocked = true;
    }

    #[cfg_attr(not(feature = "stream"), allow(dead_code))]
    pub fn sheet(&mut self, w: f32, h: f32) -> Rect {
        let (vw, vh) = (self.s.view_w, self.s.view_h);
        self.c.translate(0.0, 0.0);
        self.dy = 0.0;
        self.clip = None;
        self.c.fill(rect(0.0, 0.0, vw, vh), rgb(0x000000).alpha(0.6));
        let w = w.min(vw - 48.0);
        let b = rect(((vw - w) / 2.0).floor(), ((vh - h) / 2.0).floor(), w, h);
        self.c.round(b, 14.0, CARD2);
        self.c.round_line(b, 14.0, BORDER2, 1.0);
        self.blocked = self.s.popup.is_some() || self.s.ask.is_some();
        b
    }

    /// Pergunta Sim ou Não. A resposta chega depois por `answered`, com a mesma chave.
    pub fn ask(&mut self, key: &str, title: &str, body: &str) {
        self.s.ask = Some(Ask { id: hash_id(key), title: title.to_string(), body: body.to_string(), notice: false, check: None });
        self.s.ask_new = true;
        self.s.needs_redraw = true;
    }

    /// Pergunta com uma caixa de marcar. O estado dela chega por `ask_mark`, com qualquer resposta.
    pub fn ask_check(&mut self, key: &str, title: &str, body: &str, label: &str) {
        self.ask(key, title, body);
        if let Some(a) = &mut self.s.ask {
            a.check = Some((label.to_string(), false));
        }
    }

    pub fn ask_mark(&mut self, key: &str) -> Option<bool> {
        let id = hash_id(key);
        match self.s.ask_mark {
            Some((i, v)) if i == id => {
                self.s.ask_mark = None;
                Some(v)
            }
            _ => None,
        }
    }

    /// Aviso com um botão só.
    pub fn notice(&mut self, title: &str, body: &str) {
        self.s.ask = Some(Ask { id: 0, title: title.to_string(), body: body.to_string(), notice: true, check: None });
        self.s.ask_new = true;
        self.s.needs_redraw = true;
    }

    pub fn answered(&mut self, key: &str) -> bool {
        let id = hash_id(key);
        if self.s.answer == Some(id) {
            self.s.answer = None;
            return true;
        }
        false
    }

    /// Número com botões de menos e mais.
    pub fn stepper(&mut self, key: &str, r: Rect, val: &mut u32, range: (u32, u32), step: u32, unit: &str) -> bool {
        let mut changed = false;
        self.c.round(r, 8.0, SIDE);
        self.c.round_line(r, 8.0, BORDER, 1.0);
        let less = rect(r.x, r.y, 30.0, r.h);
        let more = rect(r.right() - 30.0, r.y, 30.0, r.h);
        if self.clickable(hash_id(&format!("{key}-")), less) {
            *val = val.saturating_sub(step).max(range.0);
            changed = true;
        }
        if self.clickable(hash_id(&format!("{key}+")), more) {
            *val = (*val + step).min(range.1);
            changed = true;
        }
        for (b, s) in [(less, "−"), (more, "+")] {
            let color = if self.hover(b) { ACCENT } else { TEXT2 };
            self.c.text(b, s, 16.0, color, Align::Center, Weight::Semibold);
        }
        self.c.text(r, &format!("{val}{unit}"), 13.0, TEXT, Align::Center, Weight::Regular);
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
    /// Imagem embutida no programa, em pixels prontos para a tela.
    pub fn image(&mut self, key: &str, size: u32, px: &[u8], r: Rect) {
        if !self.s.icons.contains_key(key) {
            let b = self.c.bitmap(size, size, px);
            self.s.icons.insert(key.to_string(), b);
        }
        if let Some(b) = self.s.icons.get(key).and_then(|b| b.as_ref()) {
            self.c.draw_bitmap(b, r);
        }
    }

    /// PNG embutido. Só abre quando aparece, e sai da memória ao fechar a janela.
    pub fn png(&mut self, key: &str, data: &[u8], r: Rect) {
        if !self.s.icons.contains_key(key) {
            let b = icons::decode_png(data).and_then(|(w, h, px)| self.c.bitmap(w, h, &px));
            self.s.icons.insert(key.to_string(), b);
        }
        if let Some(b) = self.s.icons.get(key).and_then(|b| b.as_ref()) {
            self.c.draw_bitmap(b, r);
        }
    }

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

    /// Ícone pelo tipo do arquivo. Programas e atalhos usam o ícone deles mesmos.
    pub fn kind_icon(&mut self, path: &str, name: &str, dir: bool, r: Rect) {
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        if !dir && matches!(ext.as_str(), "exe" | "lnk" | "ico" | "url" | "msc" | "cpl" | "scr") {
            return self.file_icon(path, name, r);
        }
        let key = if dir { "<pasta>".to_string() } else { format!("<tipo>.{ext}") };
        if !self.s.icons.contains_key(&key) {
            let probe = if dir { "pasta".to_string() } else { format!("arquivo.{ext}") };
            let b = icons::load_kind(&probe, dir).and_then(|(w, h, px)| self.c.bitmap(w, h, &px));
            self.s.icons.insert(key.clone(), b);
        }
        match self.s.icons.get(&key).and_then(|b| b.as_ref()) {
            Some(b) => self.c.draw_bitmap(b, r),
            None => self.c.round(r, 4.0, LINE),
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
            for _ in 0..self.s.kill_word {
                while text.ends_with(' ') {
                    text.pop();
                }
                while text.chars().last().is_some_and(|c| c != ' ') {
                    text.pop();
                }
                changed = true;
            }
            self.s.kill_word = 0;
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

    /// Clique com o botão direito numa área.
    pub fn right_hit(&mut self, r: Rect) -> bool {
        self.s.right_released && self.hover(r)
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
                let off = p.off.get(i).copied().unwrap_or(false);
                let hover = !off && r.contains(self.s.mx, self.s.my);
                if hover {
                    self.c.round(r, 6.0, BORDER);
                }
                let color = if off {
                    MUTED
                } else if i == p.selected {
                    ACCENT
                } else {
                    TEXT
                };
                self.c.text(r.inset_xy(8.0, 0.0), o, 14.0, color, Align::Left, Weight::Regular);
                if hover && self.s.released && !self.s.popup_new {
                    pick = Some((p.id, i));
                }
            }
            if (self.s.released || self.s.right_released) && !self.s.popup_new {
                close = true;
            }
            self.s.popup_new = false;
        }
        if close {
            self.s.popup = None;
            self.s.popup_pick = pick;
            self.s.needs_redraw = true;
        }
        if let Some((r, text)) = self.tip.take() {
            let w = measure(&text, 13.0, Weight::Regular, None).0 + 20.0;
            let b = rect(r.x.min(self.s.view_w - w - 8.0), r.bottom() + 6.0, w, 30.0);
            self.c.round(b, 8.0, CARD2);
            self.c.round_line(b, 8.0, BORDER2, 1.0);
            self.c.text(b, &text, 13.0, TEXT, Align::Center, Weight::Regular);
        }
        self.draw_ask();
        self.blocked = false;
        self.s.end_frame();
    }

    fn draw_ask(&mut self) {
        let Some(a) = &self.s.ask else { return };
        let (vw, vh) = (self.s.view_w, self.s.view_h);
        self.c.fill(rect(0.0, 0.0, vw, vh), rgb(0x000000).alpha(0.6));
        let w = 460.0f32.min(vw - 48.0);
        let body_h = measure(&a.body, 14.0, Weight::Regular, Some(w - 48.0)).1;
        let h = 76.0 + body_h + 76.0 + if a.check.is_some() { 40.0 } else { 0.0 };
        let b = rect((vw - w) / 2.0, (vh - h) / 2.0, w, h);
        self.c.round(b, 14.0, CARD2);
        self.c.round_line(b, 14.0, BORDER2, 1.0);
        self.c.text(rect(b.x + 24.0, b.y + 22.0, b.w - 48.0, 28.0), &a.title, 17.0, TEXT, Align::Left, Weight::Semibold);
        self.c.text_wrap(rect(b.x + 24.0, b.y + 62.0, b.w - 48.0, body_h + 4.0), &a.body, 14.0, TEXT2, Weight::Regular);
        let by = b.bottom() - 60.0;
        let yes = rect(b.right() - 24.0 - 112.0, by, 112.0, 40.0);
        let no = rect(yes.x - 120.0, by, 112.0, 40.0);
        let (mx, my) = (self.s.mx, self.s.my);
        let mut toggle = false;
        if let Some((label, on)) = &a.check {
            let row = rect(b.x + 24.0, by - 38.0, b.w - 48.0, 28.0);
            let bx = rect(row.x, row.y + 5.0, 18.0, 18.0);
            if *on {
                self.c.round(bx, 4.0, ACCENT);
                self.c.icon(bx.x, bx.y, 18.0, &[&[(6.0, 12.5), (10.0, 16.5), (18.0, 7.5)]], ON_ACCENT);
            } else {
                self.c.round_line(bx, 4.0, BORDER2, 1.0);
            }
            self.c.text(rect(row.x + 28.0, row.y, row.w - 28.0, row.h), label, 14.0, TEXT, Align::Left, Weight::Regular);
            toggle = self.s.released && !self.s.ask_new && row.contains(mx, my);
        }
        let bg = if yes.contains(mx, my) { rgb(0xa9d9ff) } else { ACCENT };
        self.c.round(yes, 8.0, bg);
        self.c.text(yes, if a.notice { "OK" } else { "Sim" }, 14.0, ON_ACCENT, Align::Center, Weight::Semibold);
        if !a.notice {
            if no.contains(mx, my) {
                self.c.round(no, 8.0, BORDER);
            }
            self.c.round_line(no, 8.0, BORDER2, 1.0);
            self.c.text(no, "Não", 14.0, TEXT, Align::Center, Weight::Regular);
        }
        if self.s.released && !self.s.ask_new {
            let id = a.id;
            let notice = a.notice;
            let mark = a.check.as_ref().map(|c| (id, c.1));
            if yes.contains(mx, my) {
                self.s.ask = None;
                self.s.ask_mark = mark;
                if !notice {
                    self.s.answer = Some(id);
                }
                self.s.needs_redraw = true;
            } else if !notice && no.contains(mx, my) {
                self.s.ask = None;
                self.s.ask_mark = mark;
                self.s.needs_redraw = true;
            }
        }
        if toggle {
            if let Some((_, on)) = self.s.ask.as_mut().and_then(|a| a.check.as_mut()) {
                *on = !*on;
            }
        }
        self.s.ask_new = false;
    }
}
