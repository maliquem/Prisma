use super::*;
use super::picker::{self, Target as Pick};
use crate::config::HdrProfile;
use crate::sys::autohdr;
use crate::sys::inject::{Hdr, Scan};
use crate::sys::hdr::{Edid, Info, Light, Profile, Screen};
use crate::sys::hdrcal::{self, Phase, Spec, Start, Target};
use crate::sys::patterns::{Measured, WINDOWS};

const ROW: f32 = 28.0;
const GAME_ROW: f32 = 44.0;

fn nits(v: f32) -> String {
    if v <= 0.0 {
        return "—".to_string();
    }
    let s = match v {
        v if v >= 10.0 => format!("{v:.0}"),
        v if v >= 0.1 => format!("{v:.2}"),
        v => format!("{v:.4}"),
    };
    s.replace('.', crate::i18n::comma())
}

fn row(ui: &Ui, r: Rect, y: f32, label: &str, value: &str, color: Color) {
    ui.c.text(rect(r.x + 16.0, y, 150.0, ROW), label, 13.0, MUTED, Align::Left, Weight::Regular);
    ui.c.text(rect(r.x + 170.0, y, r.w - 186.0, ROW), value, 13.0, color, Align::Left, Weight::Regular);
}

fn screen_card(ui: &Ui, r: Rect, s: &Screen) {
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Tela");
    let y = r.y + 42.0;
    let (mode, color) = match s.mode {
        2 => ("HDR ligado", ACCENT),
        1 => ("HDR desligado, cor ampla", TEXT),
        _ if s.hdr_support => ("HDR desligado", TEXT),
        _ => ("Sem suporte a HDR", MUTED),
    };
    row(ui, r, y, "Modo", mode, color);
    row(ui, r, y + ROW, "Nome", &s.name, TEXT);
    let hz = format!("{:.2}", s.hz).replace('.', crate::i18n::comma());
    row(ui, r, y + ROW * 2.0, "Imagem", &format!("{} × {} · {hz} Hz", s.size.0, s.size.1), TEXT);
    row(ui, r, y + ROW * 3.0, "Profundidade", &format!("{} bits", s.bits), TEXT);
    if s.mode == 2 {
        row(ui, r, y + ROW * 4.0, "Brilho do SDR", &format!("{} nits", nits(s.sdr_white)), TEXT);
    }
}

fn close(a: [[f32; 2]; 3], b: [[f32; 2]; 3]) -> bool {
    a.iter().flatten().zip(b.iter().flatten()).all(|(x, y)| (x - y).abs() < 0.01)
}

fn profile_card(ui: &Ui, r: Rect, s: &Screen) {
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Perfil de calibração");
    let y = r.y + 42.0;
    let Some(p) = &s.profile else {
        ui.c.text_wrap(rect(r.x + 16.0, y + 4.0, r.w - 32.0, 44.0), "Nenhum perfil de calibração ativo nesta tela.", 13.0, MUTED, Weight::Regular);
        return;
    };
    row(ui, r, y, "Arquivo", &p.name, TEXT);
    row(ui, r, y + ROW, "Data", &p.date, TEXT);
    let sat = match p.saturation {
        Some(v) if v < 0.01 => ("Neutra".to_string(), TEXT),
        Some(v) => (format!("Reforço de {:.0}%", v * 100.0), WARN),
        None => ("Matriz própria".to_string(), TEXT),
    };
    row(ui, r, y + ROW * 2.0, "Saturação", &sat.0, sat.1);
    row(ui, r, y + ROW * 3.0, "Curva", if p.neutral_curve { "Neutra" } else { "Corrigida" }, TEXT);
    let edid = s.hardware.as_ref().or(s.active.as_ref());
    let (text, color) = match edid {
        Some(e) if close(p.primaries, e.primaries) => ("Conferem com a tela", TEXT),
        Some(_) => ("Não conferem com a tela", WARN),
        None => ("Sem ficha para comparar", MUTED),
    };
    row(ui, r, y + ROW * 4.0, "Cores primárias", text, color);
}

fn light_row(ui: &Ui, r: Rect, y: f32, label: &str, l: Option<Light>, note: &str) {
    let (x, w) = (r.x + 16.0, r.w - 32.0);
    let col = |i: f32| rect(x + w * 0.42 + i * w * 0.19, y, w * 0.18, ROW);
    ui.c.text(rect(x, y, w * 0.41, ROW), label, 13.0, TEXT2, Align::Left, Weight::Regular);
    match l {
        Some(l) => {
            ui.c.text(col(0.0), &nits(l.peak), 13.0, TEXT, Align::Right, Weight::Regular);
            ui.c.text(col(1.0), &nits(l.full), 13.0, TEXT, Align::Right, Weight::Regular);
            ui.c.text(col(2.0), &nits(l.black), 13.0, TEXT, Align::Right, Weight::Regular);
        }
        None => ui.c.text(rect(x + w * 0.42, y, w * 0.58, ROW), note, 13.0, MUTED, Align::Left, Weight::Regular),
    }
}

fn light_card(ui: &Ui, r: Rect, s: &Screen) {
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w * 0.4, 22.0), "Brilho declarado, em nits");
    let (x, w) = (r.x + 16.0, r.w - 32.0);
    for (i, head) in ["Pico", "Tela cheia", "Preto"].iter().enumerate() {
        let c = rect(x + w * 0.42 + i as f32 * w * 0.19, r.y + 12.0, w * 0.18, 22.0);
        ui.c.text(c, head, 12.0, MUTED, Align::Right, Weight::Regular);
    }
    let y = r.y + 42.0;
    light_row(ui, r, y, "Ficha enviada pela tela", s.hardware.as_ref().and_then(|e| e.light), "Sem leitura direta nesta placa");
    let name = if s.overridden { "Override de EDID em uso" } else { "Ficha em uso no Windows" };
    light_row(ui, r, y + ROW, name, s.active.as_ref().and_then(|e| e.light), "Não informado");
    light_row(ui, r, y + ROW * 2.0, "Perfil de calibração", s.profile.as_ref().map(|p| p.light), "Sem perfil");
    let dv = s.hardware.as_ref().or(s.active.as_ref()).and_then(|e| e.dv.as_ref()).and_then(|d| d.light);
    light_row(ui, r, y + ROW * 3.0, "Bloco Dolby Vision", dv, "Não declarado");
    light_row(ui, r, y + ROW * 4.0, "O que o Windows informa aos jogos agora", s.windows, "Não informado");
}

fn dv_card(ui: &Ui, r: Rect, info: &Info, s: &Screen) {
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Dolby Vision");
    let y = r.y + 42.0;
    let dv = s.hardware.as_ref().or(s.active.as_ref()).and_then(|e| e.dv.as_ref());
    row(ui, r, y, "A tela declara", if dv.is_some() { "Sim" } else { "Não" }, TEXT);
    let modes = match dv.and_then(|d| d.interface) {
        Some(0) => "Guiado pela tela",
        Some(1) => "Baixa latência 4:2:2",
        Some(2) => "Guiado pela tela e baixa latência 4:2:2",
        Some(_) => "Guiado pela tela e baixa latência 4:4:4",
        None => "—",
    };
    row(ui, r, y + ROW, "Modos", modes, TEXT);
    let flavor = match s.hdr_flavor {
        Some(1) => "HDR10",
        Some(2) => "Dolby Vision",
        Some(_) => "Outro valor",
        None => "Sem valor salvo",
    };
    row(ui, r, y + ROW * 2.0, "HDR salvo no Windows", flavor, TEXT);
    row(ui, r, y + ROW * 3.0, "Extensão da Store", if info.dv_app { "Instalada" } else { "Não instalada" }, TEXT);
}

fn formats(e: &Edid) -> String {
    e.audio.iter().map(|(name, ch)| format!("{name} {ch}")).collect::<Vec<_>>().join(" · ")
}

fn sound_card(ui: &mut Ui, app: &App, r: Rect, info: &Info, s: &Screen) {
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 180.0, 22.0), "Som");
    let (running, failed, measured) = {
        let st = app.sync.lock().unwrap();
        (st.running, st.failed, st.results.clone())
    };
    if failed {
        ui.c.text(rect(r.x + 70.0, r.y + 12.0, r.w - 240.0, 22.0), "Não consegui abrir o teste.", 13.0, WARN, Align::Right, Weight::Regular);
    }
    let label = if running { "Medindo…" } else { "Medir atraso" };
    if ui.button("hdr-sync", rect(r.right() - 156.0, r.y + 8.0, 140.0, 32.0), label, false) && !running {
        let format = match info.sound.as_ref().map(|o| o.spatial.as_str()) {
            Some("") | None => "Sem som espacial".to_string(),
            Some(v) => v.to_string(),
        };
        crate::sys::avsync::run(app.sync.clone(), s.gdi.clone(), format, s.hz, app.main.as_ref().map_or(0, |m| m.hwnd.0 as isize));
    }
    let y = r.y + 42.0;
    match &info.sound {
        Some(o) => {
            row(ui, r, y, "Saída padrão", &o.name, TEXT);
            let spatial = match o.spatial.as_str() {
                "" if o.spatial_support => "Desligado",
                "" => "Sem suporte",
                v => v,
            };
            row(ui, r, y + ROW, "Som espacial", spatial, TEXT);
            let kind = format!("{} canais · {} kHz", o.channels, o.rate / 1000);
            row(ui, r, y + ROW * 2.0, "Mistura do Windows", &kind, TEXT);
        }
        None => row(ui, r, y, "Saída padrão", "Não encontrada", MUTED),
    }
    row(ui, r, y + ROW * 3.0, "App Dolby Access", if info.dolby_app { "Instalado" } else { "Não instalado" }, TEXT);
    let list = s.hardware.as_ref().or(s.active.as_ref()).map(formats).unwrap_or_default();
    ui.c.text(rect(r.x + 16.0, y + ROW * 4.0, 150.0, ROW), "Canais aceitos", 13.0, MUTED, Align::Left, Weight::Regular);
    let text = if list.is_empty() { "Não informado" } else { list.as_str() };
    ui.c.text_wrap(rect(r.x + 170.0, y + ROW * 4.0 + 6.0, r.w - 186.0, 38.0), text, 13.0, TEXT, Weight::Regular);
    for (i, (format, ms)) in measured.iter().enumerate() {
        row(ui, r, y + ROW * 5.5 + i as f32 * ROW, "Atraso medido", &format!("{format}: {ms} ms"), ACCENT);
    }
}

fn profile_alerts(out: &mut Vec<String>, p: &Profile, e: Option<&Edid>) {
    if let Some(v) = p.saturation.filter(|v| *v >= 0.01) {
        out.push(format!("O perfil reforça a saturação em {:.0}% e isso vale para todo o conteúdo HDR, inclusive jogos com HDR próprio.", v * 100.0));
    }
    if e.is_some_and(|e| !close(p.primaries, e.primaries)) {
        out.push("As cores primárias do perfil não são as desta tela. Isso costuma vir de um perfil feito em outra tela, e os programas que leem esses dados recebem a informação errada.".to_string());
    }
}

fn alerts(info: &Info, s: &Screen) -> Vec<String> {
    let mut out = Vec::new();
    let edid = s.hardware.as_ref().or(s.active.as_ref());
    if s.hdr_support && s.mode != 2 {
        out.push("O HDR está desligado. O perfil e o valor informado aos jogos mudam quando ele liga, então confira de novo com o HDR ligado.".to_string());
    }
    if let Some(p) = &s.profile {
        profile_alerts(&mut out, p, edid);
    }
    if s.overridden {
        let text = if s.hardware.is_some() { "Há um override de EDID nesta tela. O Windows usa os valores dele, não os que a tela envia." } else { "Há um override de EDID nesta tela, e esta placa não permite ler a ficha original." };
        out.push(text.to_string());
    }
    if edid.and_then(|e| e.dv.as_ref()).and_then(|d| d.interface).is_some_and(|i| i < 3) {
        out.push("A tela declara Dolby Vision, mas não o modo 4:4:4 de baixa latência. Segundo relatos, é isso que faz o Windows não mostrar a certificação Dolby Vision.".to_string());
    }
    let pcm = edid.map(|e| e.audio.iter().filter(|a| a.0 == "PCM").map(|a| a.1).max().unwrap_or(0)).unwrap_or(0);
    let spatial_off = info.sound.as_ref().is_some_and(|o| o.spatial.is_empty());
    if pcm == 2 && spatial_off {
        out.push("A tela só aceita PCM de 2 canais. Com o som espacial desligado, todo o som do PC sai em estéreo.".to_string());
    }
    out
}

const MODES: [(&str, &str); 6] = [("Jogo", "Jogo"), ("Filmmaker", "Filmmaker"), ("Cinema", "Cinema"), ("Padrão", "Padrao"), ("Vívido", "Vivido"), ("Outro", "Outro")];

fn target(s: &Screen) -> Target {
    Target { adapter: s.adapter, source: s.source }
}

fn start(app: &App, s: &Screen) {
    let active = s.profile.as_ref().map(|p| p.name.clone()).unwrap_or_default();
    let job = Start { gdi: s.gdi.clone(), active };
    hdrcal::begin(app.cal.clone(), job, app.main.as_ref().map_or(0, |m| m.hwnd.0 as isize));
}

fn start_card(ui: &mut Ui, app: &mut App, r: Rect, s: &Screen) {
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Calibração própria");
    let text = "Você ajusta os padrões a olho e o Prisma grava o resultado como perfil de HDR. Antes de começar, deixe a TV no modo de imagem em que você joga, desligue o mapeamento dinâmico (use HGIG se houver), desligue a luz noturna do Windows e deixe a sala como quando você joga. Leva cerca de 5 minutos e abre em tela cheia.";
    ui.c.text_wrap(rect(r.x + 16.0, r.y + 42.0, r.w - 32.0, 62.0), text, 13.0, TEXT2, Weight::Regular);
    let y = r.y + 110.0;
    ui.c.text(rect(r.x + 16.0, y, 150.0, 36.0), "Modo de imagem da TV", 13.0, MUTED, Align::Left, Weight::Regular);
    let names: Vec<String> = MODES.iter().map(|m| m.0.to_string()).collect();
    let mut sel = app.hdr_mode;
    if ui.select("hdr-mode", rect(r.x + 170.0, y, 200.0, 36.0), &names, &mut sel) {
        app.hdr_mode = sel;
    }
    if s.mode == 2 {
        if ui.button("hdr-start", rect(r.right() - 216.0, y, 200.0, 36.0), "Começar calibração", true) {
            start(app, s);
        }
    } else {
        ui.c.text(rect(r.right() - 316.0, y, 300.0, 36.0), "Ligue o HDR para calibrar.", 13.0, WARN, Align::Right, Weight::Regular);
    }
}

fn review_row(ui: &Ui, r: Rect, y: f32, label: &str, value: f32) {
    row(ui, r, y, label, &format!("{} nits", nits(value)), TEXT);
}

fn review_card(ui: &mut Ui, app: &mut App, r: Rect, s: &Screen, m: Measured) {
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Resultado da calibração");
    let half = rect(r.x, r.y, r.w / 2.0, r.h);
    let side = rect(r.x + r.w / 2.0, r.y, r.w / 2.0, r.h);
    let y = r.y + 42.0;
    review_row(ui, half, y, "Preto", m.black);
    for (i, w) in WINDOWS.iter().enumerate() {
        review_row(ui, half, y + ROW * (i + 1) as f32, &format!("Pico em {w}% da tela"), m.windows[i]);
    }
    for (i, name) in ["Vermelho", "Verde", "Azul"].iter().enumerate() {
        review_row(ui, side, y + ROW * i as f32, name, m.channels[i]);
    }
    let (peak, full) = (m.windows[1], m.windows[4]);
    let honest = peak < hdrcal::HONEST_LIMIT && full < hdrcal::HONEST_LIMIT;
    let current = s.profile.as_ref().and_then(|p| p.saturation).unwrap_or(0.0);
    let keep = current >= 0.01;
    let mut options = vec!["Saturação neutra".to_string()];
    if keep {
        options.push(format!("Manter reforço de {:.0}%", current * 100.0));
    }
    let mut sel = app.hdr_keep_sat.min(options.len() - 1);
    if ui.select("hdr-sat", rect(side.x + 16.0, y + ROW * 3.0 + 4.0, side.w - 32.0, 36.0), &options, &mut sel) {
        app.hdr_keep_sat = sel;
    }
    let by = r.bottom() - 48.0;
    if !honest {
        let text = "A forma não sumiu antes de 4000 nits: a TV está suavizando em vez de cortar, e o resultado não é confiável. Desligue o mapeamento dinâmico ou use HGIG e repita.";
        ui.c.text_wrap(rect(r.x + 16.0, by - 8.0, r.w - 148.0, 44.0), text, 13.0, WARN, Weight::Regular);
    } else if ui.button("hdr-save", rect(r.right() - 340.0, by, 200.0, 36.0), "Salvar e ativar", true) {
        let e = s.hardware.as_ref().or(s.active.as_ref());
        let spec = Spec {
            file: hdrcal::file_name(MODES[app.hdr_mode].1),
            light: Light { peak, full: full.min(peak), black: m.black },
            primaries: e.map(|e| e.primaries).unwrap_or_default(),
            saturation: if sel == 1 { current } else { 0.0 },
        };
        hdrcal::save(app.cal.clone(), spec, target(s), s.gdi.clone(), app.main.as_ref().map_or(0, |m| m.hwnd.0 as isize));
    }
    if ui.button("hdr-drop", rect(r.right() - 124.0, by, 108.0, 36.0), "Descartar", false) {
        hdrcal::discard(&app.cal);
    }
}

fn saved_card(ui: &mut Ui, app: &mut App, r: Rect, s: &Screen, list: &[HdrProfile], previous: &str) {
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Perfis do Prisma");
    let active = s.profile.as_ref().map_or("", |p| p.name.as_str());
    let mut y = r.y + 42.0;
    let mut pick = None;
    for (i, p) in list.iter().enumerate() {
        let x = r.x + 16.0;
        ui.c.text(rect(x, y, 170.0, 36.0), &p.mode, 13.0, TEXT, Align::Left, Weight::Regular);
        let text = format!("Pico {} · Tela cheia {} · Preto {}", nits(p.peak), nits(p.full), nits(p.black));
        ui.c.text(rect(x + 170.0, y, r.w - 330.0, 36.0), &text, 13.0, TEXT2, Align::Left, Weight::Regular);
        if p.file.eq_ignore_ascii_case(active) {
            ui.c.text(rect(r.right() - 124.0, y, 108.0, 36.0), "Ativo", 13.0, ACCENT, Align::Center, Weight::Regular);
        } else if ui.button(&format!("hdr-use{i}"), rect(r.right() - 124.0, y + 2.0, 108.0, 32.0), "Ativar", false) {
            pick = Some(p.file.clone());
        }
        y += 38.0;
    }
    let old = !previous.is_empty() && !previous.eq_ignore_ascii_case(active);
    if old {
        ui.c.text(rect(r.x + 16.0, y, r.w - 240.0, 36.0), &format!("Perfil anterior: {previous}"), 13.0, MUTED, Align::Left, Weight::Regular);
        if ui.button("hdr-prev", rect(r.right() - 124.0, y + 2.0, 108.0, 32.0), "Voltar", false) {
            pick = Some(previous.to_string());
        }
    }
    if let Some(file) = pick {
        if !hdrcal::activate(target(s), &file) {
            app.toast.info("O Windows não aceitou o perfil.");
        }
        app.load_hdr();
    }
}

/// Painel dos ajustes do HDR do Prisma, abaixo da linha do jogo.
const PRISMA_H: f32 = 394.0;
/// Linha do HDR próprio do jogo.
const NATIVE_ROW: f32 = 44.0;
/// Ajustes ao vivo: nome, mínimo e máximo.
pub const KNOBS: [(&str, f32, f32); 7] = [("Pico", 200.0, 4000.0), ("Paper white", 80.0, 400.0), ("Gama", 1.8, 2.6), ("Saturação", 0.5, 1.5), ("Realce dos claros", 0.0, 2.0), ("Preto", 0.0, 1.0), ("Brilho da interface", 0.0, 400.0)];

thread_local!(static PICK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) });

/// Jogo mostrado na lista suspensa: o último escolhido, ou o que acabou de entrar.
fn chosen(app: &App) -> Option<usize> {
    let n = app.cfg.hdr_games.len();
    let (i, seen) = PICK.get();
    let i = if n > seen { n - 1 } else { i.min(n.saturating_sub(1)) };
    PICK.set((i, n));
    (n > 0).then_some(i)
}

fn games_height(app: &App) -> f32 {
    80.0 + chosen(app).map_or(0.0, |i| GAME_ROW + NATIVE_ROW + if app.cfg.hdr_games[i].mode == autohdr::PRISMA { PRISMA_H } else { 0.0 })
}

/// Anti-cheat e jogo da Steam lidos uma vez por jogo: as buscas olham pastas inteiras.
fn game_facts(path: &str) -> (bool, Option<String>) {
    thread_local!(static SEEN: std::cell::RefCell<std::collections::HashMap<String, (bool, Option<String>)>> = Default::default());
    SEEN.with(|m| m.borrow_mut().entry(path.to_string()).or_insert_with(|| (crate::meter::anti_cheat(path), crate::sys::stores::steam_app_id(std::path::Path::new(path)))).clone())
}

pub(crate) fn knob(h: &mut Hdr, i: usize) -> &mut f32 {
    match i {
        0 => &mut h.peak,
        1 => &mut h.paper,
        2 => &mut h.gamma,
        3 => &mut h.sat,
        4 => &mut h.boost,
        5 => &mut h.black,
        _ => &mut h.ui,
    }
}

/// O quanto o Automático saiu da base, com sinal.
fn delta_text(i: usize, d: f32) -> String {
    let a = d.abs();
    let v = match i {
        0 | 1 | 6 => format!("{a:.0}"),
        2 => format!("{a:.2}"),
        3 => format!("{:.0}%", a * 100.0),
        4 => format!("{a:.1}x"),
        _ => format!("{a:.2}"),
    };
    format!("{}{v}", if d > 0.0 { "+" } else { "−" })
}

/// Uma linha com o que o Analisar mediu (aba e menu dentro do jogo).
pub(crate) fn scan_text(s: &Scan) -> String {
    if s.mode == 0 {
        return format!("Análise do SDR do jogo: {:.0}% da cena em branco de papel ou acima", s.white * 100.0);
    }
    let base = format!("Análise do HDR do jogo: pico {} nits, preto {} nits", nits(s.peak), nits(s.black));
    if s.ui > 0.0 {
        format!("{base}, interface {} nits", nits(s.ui))
    } else {
        base
    }
}

fn knob_text(i: usize, v: f32) -> String {
    match i {
        0 | 1 => format!("{v:.0} nits"),
        2 => format!("{v:.2}"),
        3 => format!("{:.0}%", v * 100.0),
        4 => format!("{v:.1}x"),
        5 => format!("{v:.2} nits"),
        // Abaixo de 1 nit a interface fica junto com a cena.
        _ if v < 1.0 => crate::i18n::tr("Junto").into_owned(),
        _ => format!("{v:.0} nits"),
    }
}

/// Os ajustes do HDR do Prisma em `cols` colunas, 48 de altura por linha (aba HDR e menu dentro do
/// jogo); com `base` (Automático), o quanto cada um saiu dela embaixo do valor. `true` quando algum mudou.
pub fn knobs(ui: &mut Ui, id: &str, p: Rect, cols: usize, h: &mut Hdr, base: Option<Hdr>) -> bool {
    let cw = (p.w - 24.0 * (cols - 1) as f32) / cols as f32;
    let mut moved = false;
    for (k, (name, lo, hi)) in KNOBS.into_iter().enumerate() {
        let kx = p.x + (k % cols) as f32 * (cw + 24.0);
        let ky = p.y + (k / cols) as f32 * 48.0;
        ui.c.text(rect(kx, ky, 150.0, 30.0), name, 13.0, TEXT2, Align::Left, Weight::Regular);
        let v = knob(h, k);
        let mut t = ((*v - lo) / (hi - lo)).clamp(0.0, 1.0);
        if ui.slider(&format!("{id}-{k}"), rect(kx + 150.0, ky + 3.0, cw - 250.0, 24.0), &mut t) {
            *v = lo + t * (hi - lo);
            moved = true;
        }
        ui.c.text(rect(kx + cw - 92.0, ky, 92.0, 30.0), &knob_text(k, *v), 13.0, TEXT, Align::Right, Weight::Regular);
        if let Some(mut b) = base {
            let d = *v - *knob(&mut b, k);
            if d.abs() > 1e-3 {
                ui.c.text(rect(kx + cw - 92.0, ky + 26.0, 92.0, 18.0), &delta_text(k, d), 11.0, MUTED, Align::Right, Weight::Regular);
            }
        }
    }
    moved
}

/// Os modos de HDR que o jogo pode usar. Com HDR próprio sobra deixar o do jogo ou melhorar com o Prisma;
/// sem ele, os HDR que criam um. A escolha de antes (`current`) fica à vista mesmo fora da lista.
pub(super) fn mode_options(native: u8, current: u8) -> Vec<(u8, &'static str)> {
    let all: Vec<(u8, &str)> = autohdr::MODES.iter().filter(|m| m.0 != autohdr::RTX || autohdr::nvidia()).map(|m| (m.0, m.1)).collect();
    let mut modes: Vec<(u8, &str)> = if autohdr::has_native(native) {
        vec![(autohdr::DEFAULT, "Deixar nativo"), (autohdr::PRISMA, "Melhorar com o Prisma")]
    } else if autohdr::lacks_native(native) {
        all.iter().copied().filter(|m| [autohdr::AUTO, autohdr::AUTO10, autohdr::RTX, autohdr::PRISMA].contains(&m.0)).collect()
    } else {
        all.clone()
    };
    if let Some(m) = all.iter().find(|m| m.0 == current).filter(|_| !modes.iter().any(|m| m.0 == current)) {
        modes.insert(0, *m);
    }
    modes
}

fn games_card(ui: &mut Ui, app: &mut App, r: Rect, s: &Screen) {
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 150.0, 22.0), "HDR do jogo");
    picker::add_button(ui, app, rect(r.right() - 124.0, r.y + 8.0, 108.0, 32.0), Pick::HdrGame);
    let (text, color) = if autohdr::global_on() {
        ("Um HDR por jogo. Vale a partir da próxima vez que o jogo abrir.", TEXT2)
    } else {
        ("Um HDR por jogo. O Auto HDR está desligado nas configurações do Windows: ligue lá para ele valer.", WARN)
    };
    ui.c.text(rect(r.x + 16.0, r.y + 44.0, r.w - 32.0, 22.0), text, 13.0, color, Align::Left, Weight::Regular);
    let calibrated = Hdr::from_profile(app.cfg.hdr_profiles.iter().rfind(|p| p.screen == s.name));
    let (mut change, mut remove, mut tuned, mut launch, mut hotkey_changed) = (None, None, false, None, false);
    let (mut mark, mut way, mut reset, mut ac, mut mods_off) = (None, None, None, None, None);
    let mut y = r.y + 72.0;
    for i in chosen(app).into_iter() {
        let g = &app.cfg.hdr_games[i];
        ui.file_icon(&g.path, &g.exe, rect(r.x + 16.0, y + 9.0, 26.0, 26.0));
        // Em ordem de nome: com muitos jogos, achar um é rolar a lista.
        let mut order: Vec<usize> = (0..app.cfg.hdr_games.len()).collect();
        order.sort_by_cached_key(|&k| app_name(&app.cfg.hdr_games[k].exe).to_lowercase());
        let names: Vec<String> = order.iter().map(|&k| app_name(&app.cfg.hdr_games[k].exe).to_string()).collect();
        let mut at = order.iter().position(|&k| k == i).unwrap_or(0);
        // O id de cada dropdown é só dele: a escolha da lista volta pelo id (`popup_pick`), e um id repetido
        // (o "hdr-mode" do modo de imagem da TV, ou o da linha de cima) a entregava ao primeiro que a lia.
        if ui.select(&format!("hdr-game{i}"), rect(r.x + 52.0, y + 4.0, r.w - 368.0, 36.0), &names, &mut at) {
            PICK.set((order[at], app.cfg.hdr_games.len()));
        }
        let modes = mode_options(g.native, g.mode);
        let options: Vec<String> = modes.iter().map(|m| m.1.to_string()).collect();
        let mut sel = modes.iter().position(|m| m.0 == g.mode).unwrap_or(0);
        if ui.select(&format!("hdr-gmode{i}"), rect(r.right() - 300.0, y + 4.0, 240.0, 36.0), &options, &mut sel) {
            change = Some((i, modes[sel].0));
        }
        let b = rect(r.right() - 44.0, y + 10.0, 24.0, 24.0);
        if ui.is_hover(b) {
            ui.c.round(b, 6.0, BORDER);
        }
        ui.c.icon(b.x + 4.0, b.y + 4.0, 16.0, &[&[(6., 6.), (18., 18.)], &[(18., 6.), (6., 18.)]], MUTED);
        if ui.hit(&format!("hdr-gx{i}"), b) {
            remove = Some(i);
        }
        y += GAME_ROW;
        let (said, color) = match g.native {
            autohdr::NATIVE_SEEN => ("HDR próprio: sim, visto no jogo.", TEXT2),
            autohdr::NATIVE_NOT_SEEN => ("HDR próprio: não visto. O jogo só saiu em SDR com o HDR do Prisma.", TEXT2),
            autohdr::NATIVE_YES => ("HDR próprio: sim, marcado à mão.", TEXT2),
            autohdr::NATIVE_NO => ("HDR próprio: não, marcado à mão.", TEXT2),
            _ => ("HDR próprio: ainda não se sabe. Abrir o jogo uma vez com o HDR do Prisma descobre; até lá, todas as opções.", MUTED),
        };
        ui.c.text_wrap(rect(r.x + 52.0, y, r.w - 368.0, 36.0), said, 12.0, color, Weight::Regular);
        let marks = ["Detectar sozinho", "Tem HDR próprio", "Não tem HDR próprio"].map(String::from);
        let mut m = match g.native {
            autohdr::NATIVE_YES => 1,
            autohdr::NATIVE_NO => 2,
            _ => 0,
        };
        if ui.select(&format!("hdr-native{i}"), rect(r.right() - 300.0, y, 240.0, 36.0), &marks, &mut m) {
            mark = Some((i, [autohdr::NATIVE_UNKNOWN, autohdr::NATIVE_YES, autohdr::NATIVE_NO][m]));
        }
        y += NATIVE_ROW;
        if g.mode != autohdr::PRISMA {
            continue;
        }
        let p = rect(r.x + 52.0, y, r.w - 68.0, PRISMA_H - 12.0);
        let (anti_cheat, steam) = game_facts(&g.path);
        // Pela Steam o jogo com anti-cheat abre pelo lançador dele (start_protected_game), que o Prisma recusa.
        let steam = steam.filter(|_| !(anti_cheat && g.ac_off));
        if anti_cheat {
            // Marcado, a caixa fica no lugar da explicação de como abrir.
            let at = if g.ac_off {
                p.y
            } else {
                ui.c.text_wrap(rect(p.x, p.y, p.w, 40.0), "Bloqueado: este jogo tem anti-cheat, e o HDR do Prisma precisa entrar no jogo. Escolha Auto HDR ou RTX HDR, ou marque abaixo se você abre o jogo sem o anti-cheat.", 13.0, WARN, Weight::Regular);
                p.y + 44.0
            };
            let mut off = g.ac_off;
            if ui.check(&format!("hdr-ac{i}"), rect(p.x, at, p.w, 36.0), "Abro sem o anti-cheat (offline, mods); com ele rodando, o Prisma não entra", &mut off) {
                ac = Some((i, off));
            }
            if !g.ac_off {
                y += PRISMA_H;
                continue;
            }
        } else {
            let how = if steam.is_some() {
                "Jogo da Steam: cole a linha abaixo em Propriedades > Opções de inicialização. Depois é só abrir pela Steam."
            } else {
                "Abra o jogo pelo botão Abrir com HDR, ou use a linha abaixo nas opções de inicialização do launcher."
            };
            ui.c.text_wrap(rect(p.x, p.y, p.w, 36.0), how, 13.0, TEXT2, Weight::Regular);
        }
        // Na última abertura outro mod trocou o formato da imagem e o gancho saiu do caminho (`HdrLive::other_format`).
        let other = app.hdr_other_format.eq_ignore_ascii_case(&g.exe);
        const OTHER: &str = "Sem o HDR do Prisma na última abertura: outro mod trocou o formato da imagem (no ERR, o ForceHDR do ERSS). Desligue o HDR dele para usar o do Prisma.";
        if !g.mods.is_empty() {
            // O carregador de mods abre o gancho com o jogo (me3 do ERR); a linha e o Abrir ficam de fora.
            if other {
                ui.c.text_wrap(rect(p.x, p.y + 40.0, p.w - 160.0, 34.0), OTHER, 12.0, WARN, Weight::Regular);
            } else {
                ui.c.text(rect(p.x, p.y + 40.0, p.w - 160.0, 16.0), "Gancho na pasta de mods: abra o jogo pelo lançador dos mods, com o Prisma aberto.", 12.0, TEXT2, Align::Left, Weight::Regular);
                ui.c.text(rect(p.x, p.y + 58.0, p.w - 160.0, 16.0), &g.mods, 12.0, MUTED, Align::Left, Weight::Regular);
            }
            if ui.button(&format!("hdr-gx{i}"), rect(p.right() - 150.0, p.y + 38.0, 150.0, 34.0), "Tirar da pasta", false) {
                if crate::sys::inject::take_native(&g.mods) {
                    mods_off = Some(i);
                } else {
                    app.toast.info("Feche o jogo antes de tirar o gancho.");
                }
            }
        } else {
            let line = autohdr::launch_option();
            if other {
                ui.c.text_wrap(rect(p.x, p.y + 40.0, p.w - 440.0, 34.0), OTHER, 12.0, WARN, Weight::Regular);
            } else {
                ui.c.text(rect(p.x, p.y + 40.0, p.w - 440.0, 32.0), &line, 12.0, MUTED, Align::Left, Weight::Regular);
            }
            let at = rect(p.right() - 430.0, p.y + 38.0, 140.0, 34.0);
            if ui.button(&format!("hdr-gm{i}"), at, "Pasta de mods", false) {
                app.pick_mods(i);
            }
            ui.tip(at, "Para jogo aberto por um lançador de mods (me3 do ELDEN RING Reforged): escolha a pasta de DLLs dele (dll\\offline) e o gancho entra com o jogo, sem injeção.");
            if ui.button(&format!("hdr-gc{i}"), rect(p.right() - 280.0, p.y + 38.0, 120.0, 34.0), "Copiar linha", false) {
                let ok = crate::capture::copy_text(app.paint_window.0, &line);
                app.toast.info(if ok { "Linha copiada." } else { "Não consegui copiar." });
            }
            if ui.button(&format!("hdr-go{i}"), rect(p.right() - 150.0, p.y + 38.0, 150.0, 34.0), if steam.is_some() { "Abrir pela Steam" } else { "Abrir com HDR" }, true) {
                launch = Some((g.path.clone(), steam));
            }
        }
        // Automático: a calibração da tela e a análise, com o ajuste fino guardado como diferença.
        let auto = g.prisma.is_none();
        let base = autohdr::auto_base(calibrated, g);
        let mut h = autohdr::resolve(calibrated, g);
        let ways = ["Automático", "Manual"].map(String::from);
        let mut w = usize::from(!auto);
        if ui.select(&format!("hdr-way{i}"), rect(p.x, p.y + 86.0, 200.0, 36.0), &ways, &mut w) {
            way = Some((i, w == 0, h));
        }
        let said = g.scan.map_or_else(|| "Sem análise ainda: use Analisar no menu do HDR dentro do jogo.".to_string(), |s| scan_text(&s));
        ui.c.text_wrap(rect(p.x + 216.0, p.y + 86.0, p.w - 216.0, 36.0), &said, 12.0, TEXT2, Weight::Regular);
        let moved = knobs(ui, &format!("hdr-k{i}"), rect(p.x, p.y + 132.0, p.w, 192.0), 2, &mut h, auto.then_some(base));
        if ui.button(&format!("hdr-gr{i}"), rect(p.x, p.y + 330.0, 200.0, 34.0), "Voltar ao automático", false) {
            reset = Some(i);
        }
        // O mesmo ajuste com o jogo na frente: o atalho abre o menu por cima dele.
        ui.c.text(rect(p.x + 224.0, p.y + 332.0, 170.0, 30.0), "Menu dentro do jogo", 13.0, TEXT2, Align::Left, Weight::Regular);
        if ui.hotkey("hk-hdr-menu", rect(p.x + 394.0, p.y + 330.0, 220.0, 34.0), &mut app.cfg.hk_hdr_menu) {
            hotkey_changed = true;
        }
        let g = &mut app.cfg.hdr_games[i];
        if moved && auto {
            let t = autohdr::tune_of(base, h);
            tuned |= g.tune != t;
            g.tune = t;
        } else if moved && g.prisma != Some(h) {
            g.prisma = Some(h);
            tuned = true;
        }
        y += PRISMA_H;
    }
    // Ao vivo: quem abriu o jogo relê o config quando ele muda. No arraste, grava a cada 250 ms, e o
    // último valor grava ao soltar mesmo que o arraste tenha parado antes disso.
    thread_local!(static SAVED: std::cell::Cell<(Option<std::time::Instant>, bool)> = const { std::cell::Cell::new((None, false)) });
    let (last, pending) = SAVED.get();
    if (tuned || pending) && last.is_none_or(|t| t.elapsed().as_millis() >= 250 || !ui.s.down) {
        SAVED.set((Some(std::time::Instant::now()), false));
        app.cfg.save();
    } else if tuned {
        SAVED.set((last, true));
    }
    if let Some((i, to_auto, h)) = way {
        // Manual começa do que o Automático dava.
        app.cfg.hdr_games[i].prisma = (!to_auto).then_some(h);
        app.cfg.save();
    }
    if let Some(i) = reset {
        let g = &mut app.cfg.hdr_games[i];
        (g.prisma, g.tune) = (None, [0.0; 7]);
        app.cfg.save();
    }
    if let Some((i, n)) = mark {
        app.cfg.hdr_games[i].native = n;
        app.cfg.save();
    }
    if let Some((i, off)) = ac {
        app.cfg.hdr_games[i].ac_off = off;
        app.cfg.save();
    }
    if let Some(i) = mods_off {
        app.cfg.hdr_games[i].mods.clear();
        app.cfg.save();
    }
    if hotkey_changed {
        app.cfg.save();
        app.register_hotkeys();
    }
    if let Some((path, steam)) = launch {
        let ok = match steam {
            Some(id) => crate::sys::quick::open_as_user(&format!("steam://rungameid/{id}")),
            None => autohdr::open(&path, &[]),
        };
        if !ok {
            app.toast.info("Não consegui abrir o jogo.");
        }
    }
    if let Some((i, mode)) = change {
        let g = &mut app.cfg.hdr_games[i];
        if autohdr::apply(&g.exe, mode) {
            g.mode = mode;
        } else if mode == autohdr::RTX && !crate::startup::is_elevated() {
            app.toast.info("O driver NVIDIA só aceita o RTX HDR com o Prisma aberto como administrador.");
        } else {
            app.toast.info("Não consegui gravar a escolha.");
        }
        app.cfg.save();
    }
    if let Some(i) = remove {
        let g = app.cfg.hdr_games.remove(i);
        autohdr::apply(&g.exe, autohdr::DEFAULT);
        app.cfg.save();
    }
}

fn cal_height(phase: &Phase) -> f32 {
    match phase {
        Phase::Review(_) => 270.0,
        _ => 170.0,
    }
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header(ui, a, "HDR", "O que a tela, o perfil e o Windows estão informando, e a calibração própria.");
    let (info, asked) = {
        let s = app.hdr.lock().unwrap();
        (s.info.clone(), s.reading())
    };
    if ui.button("hdr-read", rect(a.right() - 148.0, 30.0, 120.0, 36.0), if asked { "Lendo…" } else { "Ler de novo" }, false) {
        app.hdr.lock().unwrap().asked = true;
        app.load_hdr();
    }
    let Some(info) = info else {
        ui.c.text(rect(a.x + 28.0, 100.0, 400.0, 24.0), "Lendo a tela e o perfil…", 14.0, MUTED, Align::Left, Weight::Regular);
        return;
    };
    if info.screens.is_empty() {
        ui.c.text(rect(a.x + 28.0, 100.0, 400.0, 24.0), "Não consegui ler as telas.", 14.0, MUTED, Align::Left, Weight::Regular);
        return;
    }
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let mut top = 88.0;
    if info.screens.len() > 1 {
        let names: Vec<String> = info.screens.iter().map(|s| s.name.clone()).collect();
        let mut sel = app.hdr_screen.min(names.len() - 1);
        if ui.select("hdr-screen", rect(x, top, 320.0, 36.0), &names, &mut sel) {
            app.hdr_screen = sel;
        }
        top += 48.0;
    }
    let s = &info.screens[app.hdr_screen.min(info.screens.len() - 1)];
    let notes = alerts(&info, s);
    let cw = (w - 12.0) / 2.0;
    let note_h = if notes.is_empty() { 0.0 } else { 54.0 + notes.len() as f32 * 44.0 };
    let tests = app.sync.lock().unwrap().results.len();
    let (h1, h2, h3) = (190.0, 186.0, 200.0 + if tests > 0 { 12.0 + tests as f32 * ROW } else { 0.0 });
    let (phase, previous) = {
        let st = app.cal.lock().unwrap();
        (st.phase.clone(), st.previous.clone())
    };
    if let Phase::Saved(spec, live) = &phase {
        app.cfg.hdr_profiles.retain(|p| p.screen != s.name || p.mode != MODES[app.hdr_mode].0);
        let l = spec.light;
        app.cfg.hdr_profiles.push(HdrProfile { screen: s.name.clone(), mode: MODES[app.hdr_mode].0.to_string(), file: spec.file.clone(), peak: l.peak, full: l.full, black: l.black });
        app.cfg.save();
        app.cal.lock().unwrap().phase = Phase::Idle;
        let text = if *live { "Perfil salvo e ativado." } else { "Perfil ativado. O Windows ainda informa os valores antigos aos jogos." };
        app.toast.info(text);
        app.load_hdr();
    }
    let cal_h = cal_height(&phase);
    let saved: Vec<HdrProfile> = app.cfg.hdr_profiles.iter().filter(|p| p.screen == s.name).cloned().collect();
    let shown = previous.is_empty() || s.profile.as_ref().is_some_and(|p| p.name.eq_ignore_ascii_case(&previous));
    let saved_h = if saved.is_empty() && shown { 0.0 } else { 54.0 + (saved.len() + !shown as usize) as f32 * 38.0 + 12.0 };
    let games_h = games_height(app);
    let total = cal_h + 12.0 + saved_h + games_h + 12.0 + note_h + h1 + h2 + h3 + 24.0;
    let view = rect(x, top, w, a.h - top - 20.0);
    ui.begin_scroll("hdr", view, total);
    let mut y = top;
    let r = rect(x, y, w, cal_h);
    match phase {
        Phase::Review(m) => review_card(ui, app, r, s, m),
        Phase::Measuring => {
            ui.card(r);
            ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Calibração própria");
            ui.c.text(rect(r.x + 16.0, r.y + 42.0, r.w - 32.0, 24.0), "Os padrões estão abertos na tela.", 13.0, TEXT2, Align::Left, Weight::Regular);
        }
        Phase::Saving => {
            ui.card(r);
            ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Calibração própria");
            ui.c.text(rect(r.x + 16.0, r.y + 42.0, r.w - 32.0, 24.0), "Gravando e conferindo o perfil…", 13.0, TEXT2, Align::Left, Weight::Regular);
        }
        Phase::Failed(e) => {
            start_card(ui, app, r, s);
            ui.c.text(rect(r.x + 170.0, r.y + 12.0, r.w - 186.0, 22.0), e, 13.0, WARN, Align::Right, Weight::Regular);
        }
        _ => start_card(ui, app, r, s),
    }
    y += cal_h + 12.0;
    if saved_h > 0.0 {
        saved_card(ui, app, rect(x, y, w, saved_h - 12.0), s, &saved, &previous);
        y += saved_h;
    }
    games_card(ui, app, rect(x, y, w, games_h), s);
    y += games_h + 12.0;
    if !notes.is_empty() {
        let r = rect(x, y, w, note_h - 12.0);
        ui.card(r);
        ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Atenção");
        for (i, n) in notes.iter().enumerate() {
            ui.c.text_wrap(rect(r.x + 16.0, r.y + 42.0 + i as f32 * 44.0, r.w - 32.0, 40.0), n, 13.0, WARN, Weight::Regular);
        }
        y += note_h;
    }
    screen_card(ui, rect(x, y, cw, h1), s);
    profile_card(ui, rect(x + cw + 12.0, y, cw, h1), s);
    y += h1 + 12.0;
    light_card(ui, rect(x, y, w, h2), s);
    y += h2 + 12.0;
    dv_card(ui, rect(x, y, cw, h3), &info, s);
    sound_card(ui, app, rect(x + cw + 12.0, y, cw, h3), &info, s);
    ui.end_scroll();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_follow_native_hdr() {
        let ids = |native, cur| mode_options(native, cur).iter().map(|m| m.0).collect::<Vec<_>>();
        // Com HDR próprio: só deixar nativo ou melhorar com o Prisma.
        assert_eq!(ids(autohdr::NATIVE_YES, autohdr::DEFAULT), [autohdr::DEFAULT, autohdr::PRISMA]);
        // A escolha de antes continua à vista, mesmo fora da lista.
        assert_eq!(ids(autohdr::NATIVE_YES, autohdr::AUTO)[0], autohdr::AUTO);
        // Sem HDR próprio, nunca "Deixar nativo".
        assert!(!ids(autohdr::NATIVE_NO, autohdr::PRISMA).contains(&autohdr::DEFAULT));
    }
}
