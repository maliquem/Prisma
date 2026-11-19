//! Aba Equalizador da página Som: perfil de cada aparelho de saída, curva com bandas editáveis,
//! pré-amplificação com o limitador, canais de 5.1 e 7.1 e o motor (APO). Os perfis ficam em
//! `sys::eq`; o motor lê o perfil do aparelho e troca ao vivo, com mistura.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use prisma_eq::{Filter, Group, Kind, Profile};

use super::*;
use crate::i18n::tr;
use crate::sys::{audio, eq, eqapo};

const MAX_BANDS: usize = 10;
/// Altura do desenho: ±15 dB.
const DB: f64 = 15.0;
const KINDS: [(Kind, &str); 5] =
    [(Kind::Peak, "Pico"), (Kind::LowShelf, "Grave (prateleira)"), (Kind::HighShelf, "Agudo (prateleira)"), (Kind::LowPass, "Passa-baixa"), (Kind::HighPass, "Passa-alta")];
const GRAPHIC: [usize; 3] = [10, 15, 31];

#[derive(Default)]
pub struct State {
    /// Aba da página Som: 0 Programas, 1 Equalizador.
    pub tab: usize,
    /// Aparelho aberto (id do Windows) e o formato dele, lido de vez em quando.
    dev: String,
    out: Option<(Instant, audio::Output)>,
    /// Perfis do aparelho, o aberto ("" é o plano) e o conteúdo.
    list: Vec<String>,
    name: String,
    prof: Profile,
    loaded: Option<String>,
    /// Mudou: grava um pouco depois (o motor troca o som a cada gravação).
    save_at: Option<Instant>,
    graphic: bool,
    sel: usize,
    last_press: Option<Instant>,
    /// Canal desenhado: 0 todos, depois os nomes do aparelho.
    channel: usize,
    /// Dando nome: 0 renomeia, 1 cria vazio, 2 duplica, 3 o importado.
    naming: Option<(u8, String)>,
    imported: Option<Profile>,
    bufs: HashMap<String, String>,
    compare: bool,
    /// Estado do motor: quando leu, registrado no Windows, ligado neste aparelho.
    apo: Option<(Instant, bool, bool)>,
    busy: bool,
    pending: String,
    body_h: f32,
}

/// Perfil aberto para exportar: nome e conteúdo.
pub fn current(app: &App) -> Option<(String, Profile)> {
    (!app.eq.name.is_empty()).then(|| (app.eq.name.clone(), app.eq.prof.clone()))
}

/// Arquivo escolhido em Importar: pede o nome do perfil novo.
pub fn imported(app: &mut App, file: &str, p: Profile, skipped: usize) {
    let st = &mut app.eq;
    st.imported = Some(p);
    st.naming = Some((3, eq::clean_name(file)));
    if skipped > 0 {
        app.toast.info(&tr("{} linhas do arquivo ficaram de fora").replace("{}", &skipped.to_string()));
    }
}

/// A cópia de administrador terminou.
pub fn job_done(app: &mut App, ok: bool) {
    app.eq.busy = false;
    app.eq.apo = None;
    app.toast.info(if ok { "Pronto: o som do Windows reiniciou com o equalizador" } else { "O equalizador não mudou (permissão recusada ou erro; veja eq\\instalar.log)" });
}

/// Liga o perfil `name` no aparelho `g`, como escolher na página (para os atalhos de aparelho).
pub fn set_profile(app: &mut App, g: &str, name: &str) -> Result<(), String> {
    let p = eq::load(g, name).ok_or_else(|| format!("perfil não encontrado: {name}"))?;
    app.cfg.eq_devs.retain(|(d, _)| d != g);
    app.cfg.eq_devs.push((g.to_string(), name.to_string()));
    app.cfg.save();
    app.eq.loaded = None; // a página lê de novo
    eq::set_device(g, Some(&p))
}

fn guid(st: &State) -> String {
    eq::guid_of(&st.dev)
}

/// Lê de novo a lista e o perfil do aparelho escolhido.
fn load(st: &mut State, app_devs: &[(String, String)]) {
    let g = guid(st);
    st.list = eq::profiles(&g);
    st.name = app_devs.iter().find(|(d, _)| *d == g).map(|(_, n)| n.clone()).filter(|n| st.list.contains(n)).unwrap_or_default();
    st.prof = if st.name.is_empty() { Profile::default() } else { eq::load(&g, &st.name).unwrap_or_default() };
    st.graphic = !st.prof.graphic.is_empty();
    st.sel = 0;
    st.save_at = None;
    st.bufs.clear();
    st.loaded = Some(st.dev.clone());
}

/// Troca o perfil do aparelho e avisa o motor.
fn choose(st: &mut State, app: &mut App, name: &str) {
    let g = guid(st);
    app.cfg.eq_devs.retain(|(d, _)| *d != g);
    if !name.is_empty() {
        app.cfg.eq_devs.push((g.clone(), name.to_string()));
    }
    app.cfg.save();
    load(st, &app.cfg.eq_devs);
    st.compare = false;
    if let Err(e) = eq::set_device(&g, (!st.name.is_empty()).then_some(&st.prof)) {
        app.log(&format!("equalizador: {e}"));
    }
}

fn flush(st: &mut State, app: &mut App) {
    st.save_at = None;
    if st.name.is_empty() {
        return;
    }
    let g = guid(st);
    let mut r = eq::save(&g, &st.name, &st.prof);
    if !st.compare {
        r = r.and(eq::set_device(&g, Some(&st.prof)));
    }
    if let Err(e) = r {
        app.log(&format!("equalizador: {e}"));
        app.toast.info("Não consegui gravar o perfil do equalizador");
    }
}

/// Campo de número: o texto fica do jeito que se digita e vale quando é um número.
fn num(ui: &mut Ui, bufs: &mut HashMap<String, String>, key: &str, r: Rect, val: &mut f64, lo: f64, hi: f64, dec: usize) -> bool {
    let shown = format!("{:.*}", dec, *val);
    let buf = bufs.entry(key.to_string()).or_insert_with(|| shown.clone());
    if !ui.focused(key) {
        *buf = shown;
    }
    if ui.search(key, r, buf, "") {
        if let Ok(v) = buf.trim().replace(',', ".").parse::<f64>() {
            if v.is_finite() {
                *val = v.clamp(lo, hi);
                return true;
            }
        }
    }
    false
}

fn fx(g: Rect, f: f64) -> f32 {
    g.x + g.w * ((f / 20.0).ln() / 1000f64.ln()) as f32
}
fn freq_at(g: Rect, x: f32) -> f64 {
    (20.0 * 1000f64.powf(((x - g.x) / g.w) as f64)).clamp(20.0, 20_000.0)
}
fn dy(g: Rect, db: f64) -> f32 {
    g.y + g.h * (0.5 - (db / (2.0 * DB)) as f32)
}
fn db_at(g: Rect, y: f32) -> f64 {
    (((0.5 - (y - g.y) / g.h) as f64) * 2.0 * DB).clamp(-DB, DB)
}

fn has_gain(k: Kind) -> bool {
    !matches!(k, Kind::LowPass | Kind::HighPass)
}

/// Grupo de um canal só: onde ficam o ganho e o atraso dele.
fn group_of<'a>(p: &'a mut Profile, ch: &str) -> &'a mut Group {
    let i = match p.groups.iter().position(|g| g.channels.len() == 1 && g.channels[0] == ch) {
        Some(i) => i,
        None => {
            p.groups.push(Group { channels: vec![ch.to_string()], ..Default::default() });
            p.groups.len() - 1
        }
    };
    &mut p.groups[i]
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    let mut st = std::mem::take(&mut app.eq);
    draw_in(ui, app, &mut st, a);
    app.eq = st;
}

fn draw_in(ui: &mut Ui, app: &mut App, st: &mut State, a: Rect) {
    let outputs = audio::devices(false);
    if outputs.is_empty() {
        ui.card(rect(a.x, a.y, a.w, 80.0));
        ui.label(rect(a.x + 16.0, a.y + 30.0, a.w - 32.0, 20.0), "Nenhum aparelho de saída ligado.", MUTED);
        return;
    }
    if !outputs.iter().any(|d| d.id == st.dev) {
        st.dev = audio::default_id(false).filter(|id| outputs.iter().any(|d| d.id == *id)).unwrap_or_else(|| outputs[0].id.clone());
    }
    if st.loaded.as_deref() != Some(st.dev.as_str()) {
        if st.save_at.is_some() {
            flush(st, app);
        }
        load(st, &app.cfg.eq_devs);
        st.out = None;
        st.apo = None;
        st.channel = 0;
    }
    let now = Instant::now();
    if st.out.as_ref().is_none_or(|(t, _)| now.duration_since(*t) > Duration::from_secs(3)) {
        st.out = Some((now, audio::output_of(&st.dev).unwrap_or_default()));
    }
    let g = guid(st);
    if st.apo.is_none_or(|(t, _, _)| now.duration_since(t) > Duration::from_secs(3)) && !st.busy {
        st.apo = Some((now, eqapo::registered(), eqapo::active_on(&g)));
    }
    let out = st.out.as_ref().map(|o| o.1.clone()).unwrap_or_default();
    let (_, registered, active) = st.apo.unwrap_or((now, false, false));
    let rate = if out.rate > 0 { out.rate as f64 } else { 48_000.0 };
    let names: Vec<&'static str> = prisma_eq::channel_names(out.mask, out.channels.max(2) as usize).into_iter().filter(|n| !n.is_empty()).collect();

    let prev_scroll = ui.scroll_y_of("eq-scroll");
    let mut wheel_used = false;
    ui.begin_scroll("eq-scroll", a, st.body_h);
    let mut y = a.y;

    // Aparelho e perfil.
    let c = rect(a.x, y, a.w, 96.0);
    ui.card(c);
    let half = (c.w - 48.0) / 2.0;
    ui.label(rect(c.x + 16.0, c.y + 10.0, half, 18.0), "Aparelho de saída", TEXT2);
    let dev_names: Vec<String> = outputs.iter().map(|d| d.name.clone()).collect();
    let mut di = outputs.iter().position(|d| d.id == st.dev).unwrap_or(0);
    let dev_r = rect(c.x + 16.0, c.y + 32.0, half, 36.0);
    let fmt = {
        let ch = match out.channels {
            1 => tr("mono").into_owned(),
            2 => tr("estéreo").into_owned(),
            6 => "5.1".into(),
            8 => "7.1".into(),
            n => tr("{} canais").replace("{}", &n.to_string()),
        };
        let state = if active { tr("EQ ativo neste aparelho") } else { tr("EQ desligado neste aparelho") };
        format!("{state} · {} kHz · {ch}", out.rate / 1000)
    };
    ui.c.text_fit(rect(c.x + 16.0, c.y + 72.0, half, 18.0), &fmt, 12.0, if active { ACCENT } else { MUTED }, Align::Left, Weight::Regular);

    let px = c.x + 32.0 + half;
    ui.label(rect(px, c.y + 10.0, half, 18.0), "Perfil do aparelho", TEXT2);
    let more_r = rect(c.right() - 16.0 - 96.0, c.y + 32.0, 96.0, 36.0);
    let prof_r = rect(px, c.y + 32.0, more_r.x - px - 8.0, 36.0);
    let mut new_naming = None;
    if let Some((mode, mut text)) = st.naming.take() {
        ui.search("eq-name", rect(px, c.y + 32.0, half - 200.0, 36.0), &mut text, "Nome do perfil");
        let name = eq::clean_name(&text);
        let taken = st.list.iter().any(|n| n.eq_ignore_ascii_case(&name) && !(mode == 0 && *n == st.name));
        if taken {
            ui.c.text_fit(rect(px, c.y + 72.0, half, 18.0), &tr("Esse nome já existe"), 12.0, WARN, Align::Left, Weight::Regular);
        }
        let ok_r = rect(c.right() - 16.0 - 196.0, c.y + 32.0, 96.0, 36.0);
        let mut done = ui.button("eq-name-no", rect(c.right() - 16.0 - 96.0, c.y + 32.0, 96.0, 36.0), "Cancelar", false);
        if ui.button_ex("eq-name-ok", ok_r, if mode == 0 { "Renomear" } else { "Criar" }, true, !name.is_empty() && !taken) {
            let r = match mode {
                0 => eq::rename(&g, &st.name, &name).map(|_| {
                    for e in app.cfg.eq_apps.iter_mut().filter(|e| e.1 == st.name) {
                        e.1 = name.clone();
                    }
                }),
                1 => eq::save(&g, &name, &Profile::default()),
                2 => eq::save(&g, &name, &st.prof),
                _ => eq::save(&g, &name, &st.imported.take().unwrap_or_default()),
            };
            match r {
                Ok(()) => choose(st, app, &name),
                Err(e) => {
                    app.log(&format!("equalizador: {e}"));
                    app.toast.info("Não consegui gravar o perfil do equalizador");
                }
            }
            done = true;
        }
        if !done {
            new_naming = Some((mode, text));
        }
    } else {
        let mut opts = st.list.clone();
        opts.push(tr("Plano (desligado)").into_owned());
        opts.push(tr("+ Novo perfil…").into_owned());
        let mut pi = st.list.iter().position(|n| *n == st.name).unwrap_or(st.list.len());
        if ui.select("eq-prof", prof_r, &opts, &mut pi) {
            if st.save_at.is_some() {
                flush(st, app);
            }
            if pi == st.list.len() + 1 {
                new_naming = Some((1, String::new()));
            } else {
                let n = st.list.get(pi).cloned().unwrap_or_default();
                choose(st, app, &n);
            }
        }
        let has = !st.name.is_empty();
        if ui.button("eq-more", more_r, "Mais ▾", false) {
            let t: Vec<String> = ["Renomear", "Duplicar", "Importar…", "Exportar…", "Apagar"].iter().map(|s| tr(s).into_owned()).collect();
            let on = [has, has, true, has, has];
            let items: Vec<(&str, bool)> = t.iter().map(String::as_str).zip(on).collect();
            ui.menu("eq-more-menu", &items);
        }
        match ui.menu_pick("eq-more-menu") {
            Some(0) => new_naming = Some((0, st.name.clone())),
            Some(1) => new_naming = Some((2, format!("{} 2", st.name))),
            Some(2) => app.eq_file(false),
            Some(3) => {
                flush(st, app);
                app.eq_file(true);
            }
            Some(4) => ui.ask("eq-del", &tr("Apagar o perfil {}?").replace("{}", &st.name), &tr("O aparelho volta ao plano. O arquivo do perfil sai do disco.")),
            _ => {}
        }
        if ui.answered("eq-del") {
            let _ = eq::remove(&g, &st.name);
            app.cfg.eq_apps.retain(|e| e.1 != st.name);
            choose(st, app, "");
        }
    }
    st.naming = new_naming;
    // Por último, para a lista abrir por cima do resto.
    if ui.select("eq-dev", dev_r, &dev_names, &mut di) {
        if st.save_at.is_some() {
            flush(st, app);
        }
        st.dev = outputs[di].id.clone();
    }
    y = c.bottom() + 12.0;

    // Motor.
    let m = rect(a.x, y, a.w, 84.0);
    ui.card(m);
    ui.title(rect(m.x + 16.0, m.y + 10.0, m.w - 32.0, 22.0), "Motor do equalizador");
    let spatial = !out.spatial.is_empty();
    let (text, color) = if st.busy {
        (tr("Esperando o Windows…").into_owned(), MUTED)
    } else if spatial {
        (tr("Com {} ligado, o Windows não passa o som deste aparelho pelo equalizador. Desligue o som espacial dele para usar.").replace("{}", &out.spatial), WARN)
    } else if active {
        (tr("Ligado neste aparelho. O som de todos os programas passa pelo perfil do aparelho.").into_owned(), TEXT2)
    } else if registered {
        (tr("Instalado, mas desligado neste aparelho.").into_owned(), TEXT2)
    } else {
        (tr("Não instalado. Os perfis podem ser montados agora; o som só muda com o motor ligado.").into_owned(), TEXT2)
    };
    let bw = 180.0;
    ui.c.text_wrap(rect(m.x + 16.0, m.y + 38.0, m.w - 48.0 - 2.0 * (bw + 8.0), 40.0), &text, 12.0, color, Weight::Regular);
    let b1 = rect(m.right() - 16.0 - bw, m.y + 24.0, bw, 36.0);
    let b0 = rect(b1.x - 8.0 - bw, b1.y, bw, 36.0);
    if active {
        if ui.button_ex("eq-apo-off", b1, "Desligar aqui", false, !st.busy) {
            st.pending = format!("remove:{g}");
            ui.ask("eq-apo", &tr("Desligar o equalizador neste aparelho?"), &tr("O Windows vai pedir administrador. A configuração original do aparelho volta e o som do Windows reinicia (alguns segundos de silêncio)."));
        }
    } else if ui.button_ex("eq-apo-on", b1, if registered { "Ligar aqui" } else { "Ligar o equalizador…" }, true, !st.busy && !spatial) {
        st.pending = format!("install:{g}");
        ui.ask(
            "eq-apo",
            &tr("Ligar o equalizador neste aparelho?"),
            &tr("O Windows vai pedir administrador. O Prisma copia o motor para Arquivos de Programas, coloca no aparelho e reinicia o som do Windows (alguns segundos de silêncio). A configuração original do aparelho fica guardada para voltar igual ao remover. Nada é baixado.\n\nO motor ainda não é assinado: para o Windows carregar, a proteção do motor de áudio fica desligada (o Equalizer APO faz o mesmo). Conteúdo protegido, como alguns filmes em streaming, pode tocar em qualidade menor ou não tocar."),
        );
    }
    if registered && ui.button_ex("eq-apo-rm", b0, "Remover de todos", false, !st.busy) {
        st.pending = "remove".into();
        ui.ask("eq-apo", &tr("Remover o equalizador de todos os aparelhos?"), &tr("O Windows vai pedir administrador. Cada aparelho volta à configuração original, o motor sai do Windows e o som reinicia. Os perfis ficam guardados."));
    }
    if ui.answered("eq-apo") {
        st.busy = true;
        let op = std::mem::take(&mut st.pending);
        let hub = app.hub.0 as isize;
        let _ = std::thread::Builder::new().name("equalizador".into()).spawn(move || {
            let ok = eqapo::run_elevated(&op);
            crate::app::post_eq_done(hub, ok);
        });
    }
    y = m.bottom() + 12.0;

    // Curva.
    let editable = !st.name.is_empty();
    let table_h = if st.graphic { 52.0 } else { 40.0 + 40.0 * st.prof.filters.len() as f32 + 48.0 };
    let k = rect(a.x, y, a.w, 48.0 + 260.0 + 12.0 + 96.0 + table_h);
    ui.card(k);
    ui.title(rect(k.x + 16.0, k.y + 12.0, 140.0, 22.0), "Equalizador");
    let mut changed = false;
    let seg = |i: f32| rect(k.x + 140.0 + i * 112.0, k.y + 8.0, 108.0, 32.0);
    if ui.button("eq-mode-p", seg(0.0), "Paramétrico", !st.graphic) {
        st.graphic = false;
    }
    if ui.button("eq-mode-g", seg(1.0), "Gráfico", st.graphic) {
        st.graphic = true;
        if editable && st.prof.graphic.is_empty() {
            st.prof.graphic = vec![0.0; 10];
            changed = true;
        }
    }
    let cmp_r = rect(k.right() - 16.0 - 150.0, k.y + 8.0, 150.0, 32.0);
    if ui.button_ex("eq-cmp", cmp_r, "Comparar sem EQ", st.compare, editable) {
        st.compare = !st.compare;
        let _ = eq::set_device(&g, (!st.compare).then_some(&st.prof));
    }
    let mut ch_opts = vec![tr("Canal: todos").into_owned()];
    ch_opts.extend(names.iter().map(|n| tr("Canal: {}").replace("{}", n)));
    st.channel = st.channel.min(ch_opts.len() - 1);
    let ch_r = rect(cmp_r.x - 8.0 - 150.0, k.y + 8.0, 150.0, 32.0);
    let channel = if st.channel == 0 { "" } else { names[st.channel - 1] };

    let gr = rect(k.x + 56.0, k.y + 56.0, k.w - 80.0, 240.0);
    ui.c.round(rect(gr.x - 8.0, gr.y - 8.0, gr.w + 16.0, gr.h + 16.0), 8.0, CARD2);
    for f in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0] {
        let x = fx(gr, f);
        ui.c.line(x, gr.y, x, gr.bottom(), BORDER, 1.0);
        let t = if f >= 1000.0 { format!("{}k", f / 1000.0) } else { format!("{f}") };
        ui.c.text_raw(rect(x - 20.0, gr.bottom() + 4.0, 40.0, 14.0), &t, 11.0, MUTED, Align::Center, Weight::Regular);
    }
    for d in [-12.0, -6.0, 0.0, 6.0, 12.0] {
        let yy = dy(gr, d);
        ui.c.line(gr.x, yy, gr.right(), yy, if d == 0.0 { MUTED } else { BORDER }, 1.0);
        ui.c.text_raw(rect(gr.x - 48.0, yy - 7.0, 36.0, 14.0), &format!("{d:+}"), 11.0, MUTED, Align::Right, Weight::Regular);
    }
    // Curva do canal escolhido (com a pré-amplificação e o ganho do canal).
    let steps = 160;
    let mut last = None;
    for i in 0..=steps {
        let x = gr.x + gr.w * i as f32 / steps as f32;
        let d = st.prof.response_db(channel, freq_at(gr, x), rate).clamp(-DB, DB);
        let p = (x, dy(gr, d));
        if let Some((x0, y0)) = last {
            ui.c.line(x0, y0, p.0, p.1, if st.compare { MUTED } else { ACCENT }, 2.0);
        }
        last = Some(p);
    }
    if !editable {
        ui.c.text_wrap_center(rect(gr.x, gr.y + gr.h / 2.0 - 30.0, gr.w, 24.0), &tr("Plano: escolha ou crie um perfil para editar a curva."), 13.0, TEXT2, Weight::Regular);
    }

    // Pontos: arrastar muda frequência e ganho, a roda muda o Q, clique duplo cria uma banda.
    let mut on_point = false;
    if editable && st.graphic {
        if let Some((freqs, _)) = prisma_eq::graphic_freqs(st.prof.graphic.len()) {
            for (i, &f) in freqs.iter().enumerate() {
                let (x, yy) = (fx(gr, f), dy(gr, st.prof.graphic[i]));
                let key = format!("eq-g{i}");
                if ui.drag(&key, rect(x - 8.0, gr.y, 16.0, gr.h)) {
                    st.prof.graphic[i] = (db_at(gr, ui.s.my) * 2.0).round() / 2.0;
                    changed = true;
                }
                ui.c.line(x, dy(gr, 0.0), x, yy, ACCENT, 3.0);
                ui.c.circle(x, yy, 5.0, WHITE);
            }
        }
    } else if editable {
        for i in 0..st.prof.filters.len() {
            let f = &st.prof.filters[i];
            let (x, yy) = (fx(gr, f.freq), dy(gr, if has_gain(f.kind) { f.gain_db } else { 0.0 }));
            let pr = rect(x - 10.0, yy - 10.0, 20.0, 20.0);
            let hover = ui.is_hover(pr);
            on_point |= hover;
            if hover && ui.s.wheel != 0.0 {
                let q = &mut st.prof.filters[i].q;
                *q = (*q * if ui.s.wheel > 0.0 { 1.12 } else { 1.0 / 1.12 }).clamp(0.1, 20.0);
                ui.s.wheel = 0.0;
                wheel_used = true;
                changed = true;
            }
            if ui.drag(&format!("eq-p{i}"), pr) {
                st.sel = i;
                on_point = true;
                let (mx, my) = (ui.s.mx, ui.s.my);
                let f = &mut st.prof.filters[i];
                f.freq = freq_at(gr, mx).round();
                if has_gain(f.kind) {
                    f.gain_db = (db_at(gr, my) * 10.0).round() / 10.0;
                }
                changed = true;
            }
            let f = &st.prof.filters[i];
            let col = if !f.on { MUTED } else if i == st.sel { WHITE } else { ACCENT };
            ui.c.circle(x, yy, if i == st.sel { 7.0 } else { 5.0 }, col);
            ui.c.text_raw(rect(x - 10.0, yy - 24.0, 20.0, 14.0), &(i + 1).to_string(), 11.0, TEXT2, Align::Center, Weight::Regular);
        }
        if ui.s.pressed && ui.is_hover(gr) && !on_point {
            let double = st.last_press.is_some_and(|t| now.duration_since(t) < Duration::from_millis(400));
            st.last_press = Some(now);
            if double && st.prof.filters.len() < MAX_BANDS {
                st.prof.filters.push(Filter { on: true, kind: Kind::Peak, freq: freq_at(gr, ui.s.mx).round(), gain_db: (db_at(gr, ui.s.my) * 10.0).round() / 10.0, q: 1.0 });
                st.sel = st.prof.filters.len() - 1;
                st.last_press = None;
                changed = true;
            }
        }
    }

    // Ganho geral, proteção e limitador.
    let gy = gr.bottom() + 28.0;
    ui.label(rect(k.x + 16.0, gy, 160.0, 20.0), "Pré-amplificação (dB)", TEXT2);
    if editable && num(ui, &mut st.bufs, "eq-pre", rect(k.x + 176.0, gy - 8.0, 80.0, 34.0), &mut st.prof.preamp_db, -DB, DB, 1) {
        changed = true;
    }
    let mut v = ((st.prof.preamp_db + DB) / (2.0 * DB)) as f32;
    if editable && ui.slider("eq-pre-s", rect(k.x + 272.0, gy - 6.0, 220.0, 30.0), &mut v) {
        st.prof.preamp_db = ((v as f64 * 2.0 - 1.0) * DB * 2.0).round() / 2.0;
        changed = true;
    }
    let mut lim = st.prof.limiter;
    if editable && ui.check("eq-lim", rect(k.x + 512.0, gy - 8.0, 220.0, 34.0), "Limitador", &mut lim) {
        st.prof.limiter = lim;
        changed = true;
    }
    let mut prot = st.prof.protect;
    if editable && ui.check("eq-prot", rect(k.x + 16.0, gy + 34.0, 420.0, 34.0), "Proteção: corta abaixo de 20 Hz e acima de 20 kHz", &mut prot) {
        st.prof.protect = prot;
        changed = true;
    }
    // Ganho acima de 0 dB sem o limitador estoura no volume cheio.
    let peak = (0..=60).map(|i| st.prof.response_db(channel, 20.0 * 1000f64.powf(i as f64 / 60.0), rate)).fold(f64::MIN, f64::max);
    if editable && peak > 0.5 {
        let warn = if st.prof.limiter { tr("Acima de 0 dB: o limitador segura os picos no volume cheio.") } else { tr("Pode distorcer: baixe a pré-amplificação ou ligue o limitador.") };
        ui.c.text_fit(rect(k.x + 512.0, gy + 34.0, k.w - 528.0, 34.0), &warn, 12.0, if st.prof.limiter { MUTED } else { WARN }, Align::Left, Weight::Regular);
    }

    // Bandas.
    let ty = gy + 84.0;
    if st.graphic {
        ui.label(rect(k.x + 16.0, ty + 8.0, 70.0, 20.0), "Bandas", TEXT2);
        let mut bi = GRAPHIC.iter().position(|n| *n == st.prof.graphic.len()).unwrap_or(0);
        let opts: Vec<String> = GRAPHIC.iter().map(|n| n.to_string()).collect();
        if ui.button_ex("eq-g-zero", rect(k.x + 196.0, ty, 96.0, 36.0), "Zerar", false, editable) {
            st.prof.graphic.iter_mut().for_each(|v| *v = 0.0);
            changed = true;
        }
        if editable && ui.select("eq-g-n", rect(k.x + 88.0, ty, 96.0, 36.0), &opts, &mut bi) {
            // Cada banda nova pega o ganho da antiga mais perto.
            let old = prisma_eq::graphic_freqs(st.prof.graphic.len()).map(|(f, _)| f.to_vec()).unwrap_or_default();
            let (new, _) = prisma_eq::graphic_freqs(GRAPHIC[bi]).unwrap_or((&[], 1.0));
            st.prof.graphic = new
                .iter()
                .map(|f| old.iter().zip(&st.prof.graphic).min_by(|a, b| (a.0 / f).ln().abs().total_cmp(&(b.0 / f).ln().abs())).map_or(0.0, |(_, g)| *g))
                .collect();
            changed = true;
        }
    } else {
        let cols = [("Liga", 56.0), ("Tipo", 200.0), ("Frequência (Hz)", 140.0), ("Ganho (dB)", 120.0), ("Q", 100.0)];
        let mut cx = k.x + 16.0;
        for (t, w) in cols {
            ui.label(rect(cx, ty, w, 18.0), t, MUTED);
            cx += w + 8.0;
        }
        let mut drop = None;
        let mut kind_sel = Vec::new();
        for i in 0..st.prof.filters.len() {
            let ry = ty + 24.0 + i as f32 * 40.0;
            if i == st.sel {
                ui.c.round(rect(k.x + 8.0, ry - 2.0, k.w - 16.0, 40.0), 6.0, CARD2);
            }
            let f = &mut st.prof.filters[i];
            let mut x = k.x + 16.0;
            if ui.check(&format!("eq-on{i}"), rect(x, ry, 48.0, 36.0), "", &mut f.on) {
                changed = true;
            }
            x += 64.0;
            kind_sel.push((i, rect(x, ry, 200.0, 36.0)));
            x += 208.0;
            changed |= num(ui, &mut st.bufs, &format!("eq-f{i}"), rect(x, ry, 140.0, 36.0), &mut f.freq, 20.0, 20_000.0, 0);
            x += 148.0;
            if has_gain(f.kind) {
                changed |= num(ui, &mut st.bufs, &format!("eq-gn{i}"), rect(x, ry, 120.0, 36.0), &mut f.gain_db, -DB, DB, 1);
            }
            x += 128.0;
            changed |= num(ui, &mut st.bufs, &format!("eq-q{i}"), rect(x, ry, 100.0, 36.0), &mut f.q, 0.1, 20.0, 2);
            let b = rect(k.right() - 44.0, ry + 6.0, 24.0, 24.0);
            if ui.is_hover(b) {
                ui.c.round(b, 6.0, BORDER);
            }
            ui.c.icon(b.x + 4.0, b.y + 4.0, 16.0, &[&[(6., 6.), (18., 18.)], &[(18., 6.), (6., 18.)]], MUTED);
            if ui.hit(&format!("eq-x{i}"), b) {
                drop = Some(i);
            }
        }
        let ay = ty + 24.0 + st.prof.filters.len() as f32 * 40.0 + 6.0;
        let can_add = editable && st.prof.filters.len() < MAX_BANDS;
        if ui.button_ex("eq-add", rect(k.x + 16.0, ay, 150.0, 34.0), "Adicionar banda", false, can_add) {
            let f = st.prof.filters.last().map_or(1000.0, |f| (f.freq * 2.0).min(16_000.0));
            st.prof.filters.push(Filter { on: true, kind: Kind::Peak, freq: f, gain_db: 0.0, q: 1.0 });
            st.sel = st.prof.filters.len() - 1;
            changed = true;
        }
        ui.c.text_fit(rect(k.x + 180.0, ay, k.w - 200.0, 34.0), &tr("Até 10 bandas. Clique duplo na curva também cria uma; a roda do mouse num ponto muda o Q."), 12.0, MUTED, Align::Left, Weight::Regular);
        let kinds: Vec<String> = KINDS.iter().map(|(_, n)| tr(n).into_owned()).collect();
        for (i, r) in kind_sel {
            let mut ki = KINDS.iter().position(|(kk, _)| *kk == st.prof.filters[i].kind).unwrap_or(0);
            if ui.select(&format!("eq-k{i}"), r, &kinds, &mut ki) {
                st.prof.filters[i].kind = KINDS[ki].0;
                changed = true;
            }
        }
        if let Some(i) = drop {
            st.prof.filters.remove(i);
            st.sel = st.sel.min(st.prof.filters.len().saturating_sub(1));
            st.bufs.clear();
            changed = true;
        }
    }
    let _ = ui.select("eq-ch", ch_r, &ch_opts, &mut st.channel);
    y = k.bottom() + 12.0;

    // Canais: ganho e atraso de cada caixa, quando o aparelho tem mais de duas.
    if names.len() > 2 {
        let rows = names.len().div_ceil(2);
        let n = rect(a.x, y, a.w, 56.0 + rows as f32 * 44.0 + 44.0);
        ui.card(n);
        ui.title(rect(n.x + 16.0, n.y + 12.0, 200.0, 22.0), "Canais");
        ui.c.text_fit(rect(n.x + 140.0, n.y + 12.0, n.w - 160.0, 22.0), &tr("Ganho em dB e atraso em ms (1 ms ≈ 34 cm), para alinhar as caixas da sala."), 12.0, MUTED, Align::Left, Weight::Regular);
        let cw = (n.w - 48.0) / 2.0;
        for (i, ch) in names.iter().enumerate() {
            let cx = n.x + 16.0 + (i % 2) as f32 * (cw + 16.0);
            let ry = n.y + 48.0 + (i / 2) as f32 * 44.0;
            ui.c.text_raw(rect(cx, ry, 60.0, 36.0), ch, 14.0, TEXT, Align::Left, Weight::Semibold);
            let (mut gain, mut delay) = st.prof.groups.iter().find(|gr| gr.channels.len() == 1 && gr.channels[0] == *ch).map_or((0.0, 0.0), |gr| (gr.gain_db, gr.delay_ms));
            let gk = format!("eq-cg{ch}");
            let dk = format!("eq-cd{ch}");
            if editable {
                ui.c.text(rect(cx + 64.0, ry, 50.0, 36.0), "Ganho", 12.0, MUTED, Align::Left, Weight::Regular);
                let g1 = num(ui, &mut st.bufs, &gk, rect(cx + 116.0, ry, 90.0, 36.0), &mut gain, -DB, DB, 1);
                ui.c.text(rect(cx + 220.0, ry, 50.0, 36.0), "Atraso", 12.0, MUTED, Align::Left, Weight::Regular);
                let d1 = num(ui, &mut st.bufs, &dk, rect(cx + 272.0, ry, 90.0, 36.0), &mut delay, 0.0, prisma_eq::MAX_DELAY_MS, 1);
                if g1 || d1 {
                    let gp = group_of(&mut st.prof, ch);
                    gp.gain_db = gain;
                    gp.delay_ms = delay;
                    // Grupo zerado e sem filtros não precisa ficar no arquivo.
                    st.prof.groups.retain(|gr| gr.gain_db != 0.0 || gr.delay_ms != 0.0 || !gr.filters.is_empty());
                    changed = true;
                }
            }
        }
        if ui.button("eq-spk", rect(n.x + 16.0, n.bottom() - 48.0, 320.0, 34.0), "Abrir configuração de alto-falantes do Windows", false) {
            let _ = std::process::Command::new(crate::sys::system_tool("control")).arg("mmsys.cpl").spawn();
        }
        y = n.bottom() + 12.0;
    }

    st.body_h = y - a.y + 8.0;
    ui.end_scroll();
    if wheel_used {
        ui.set_scroll("eq-scroll", prev_scroll);
    }

    if changed && editable {
        st.save_at.get_or_insert(now);
    }
    if let Some(t) = st.save_at {
        if now.duration_since(t) > Duration::from_millis(250) {
            flush(st, app);
        } else {
            ui.s.needs_redraw = true;
        }
    }
}
