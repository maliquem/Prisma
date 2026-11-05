use super::*;
use crate::sys::hdr::{Edid, Info, Light, Profile, Screen};

const ROW: f32 = 28.0;

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

fn sound_card(ui: &Ui, r: Rect, info: &Info, s: &Screen) {
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Som");
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

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header(ui, a, "HDR", "O que a tela, o perfil e o Windows estão informando. Nada aqui muda o PC.");
    let (info, busy) = {
        let s = app.hdr.lock().unwrap();
        (s.info.clone(), s.busy)
    };
    if ui.button("hdr-read", rect(a.right() - 148.0, 30.0, 120.0, 36.0), if busy { "Lendo…" } else { "Ler de novo" }, false) {
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
    let (h1, h2, h3) = (190.0, 186.0, 200.0);
    let total = note_h + h1 + h2 + h3 + 24.0;
    let view = rect(x, top, w, a.h - top - 20.0);
    ui.begin_scroll("hdr", view, total);
    let mut y = top;
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
    sound_card(ui, rect(x + cw + 12.0, y, cw, h3), &info, s);
    ui.end_scroll();
}
