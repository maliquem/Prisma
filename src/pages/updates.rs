//! Atualizações seguras: avisa quando sai driver da NVIDIA ou atualização do Windows e, depois de
//! esperar os relatos, diz se vale instalar. A conferência roda aqui, com o Prisma aberto ou na
//! bandeja (`tick`), sem tarefa no Windows; a lógica e as fontes ficam em `sys::safeupd`.

use std::sync::Mutex;

use super::*;
use crate::i18n::tr;
use crate::sys::safeupd::{self, Notice, Sev, State, Watch};
use crate::sys::winupd;

/// Dias entre as conferências (0: desligado) e horas de espera pelos relatos.
const EVERY: [u32; 5] = [0, 1, 2, 3, 7];
const EVERY_NAMES: [&str; 5] = ["Desligado", "Todo dia", "A cada 2 dias", "A cada 3 dias", "Toda semana"];
const WAIT: [u32; 3] = [24, 72, 96];
const WAIT_NAMES: [&str; 3] = ["24 horas", "3 dias", "4 dias"];
const KEY_PAGE: &str = "https://aistudio.google.com/apikey";

#[derive(Default)]
pub struct Data {
    pub st: State,
    loaded: bool,
    running: bool,
    /// Ação de administrador em andamento.
    op: Option<&'static str>,
    /// Atualizações do Windows marcadas (UpdateID).
    sel: Vec<String>,
    policy: bool,
    /// Altura do conteúdo e de cada cartão no quadro anterior: o fundo vem antes do que ele mede.
    h: f32,
    cards: [f32; 3],
}

/// Resultado da conferência (o estado novo, os avisos e se foi pedida no botão).
static DONE: Mutex<Option<(State, Vec<Notice>, bool)>> = Mutex::new(None);
/// Resultado da cópia de administrador: a ação e o código de saída (None: UAC recusado).
static OP: Mutex<Option<(&'static str, Option<u32>)>> = Mutex::new(None);

fn load(app: &mut App) {
    if !app.su.loaded {
        app.su.st = safeupd::load();
        app.su.loaded = true;
    }
}

pub fn enter(app: &mut App) {
    load(app);
    app.su.policy = winupd::policy_on();
}

/// Confere numa thread: é rede e COM, e o Windows Update demora.
fn start(app: &mut App, force: bool, manual: bool) {
    if app.su.running {
        return;
    }
    app.su.running = true;
    let (st, key, wait) = (app.su.st.clone(), app.cfg.safe_key.clone(), app.cfg.safe_wait);
    std::thread::spawn(move || {
        // Sempre devolve um resultado: sem ele a página ficaria em "Procurando..." e a agenda pararia.
        let old = st.clone();
        let (st, notes) = std::panic::catch_unwind(|| safeupd::run(st, &key, wait, force)).unwrap_or_else(|_| {
            crate::config::log("Atualizações seguras: a conferência falhou");
            (safeupd::State { checked: safeupd::now(), ..old }, Vec::new())
        });
        *DONE.lock().unwrap_or_else(|e| e.into_inner()) = Some((st, notes, manual));
    });
}

fn elevated(app: &mut App, name: &'static str, arg: String) {
    if app.su.op.is_some() {
        return;
    }
    app.su.op = Some(name);
    std::thread::spawn(move || {
        let code = winupd::run_elevated(&arg);
        *OP.lock().unwrap() = Some((name, code));
    });
}

/// A cada segundo: recolhe o que as threads terminaram e começa a conferência quando chega a hora.
pub fn tick(app: &mut App) {
    load(app);
    let done = DONE.lock().unwrap().take();
    if let Some((st, notes, manual)) = done {
        app.su.running = false;
        safeupd::save(&st);
        app.su.st = st;
        let ids: Vec<String> = app.su.st.updates.iter().map(|u| u.id.clone()).collect();
        app.su.sel.retain(|id| ids.contains(id));
        let msgs: Vec<String> = notes
            .iter()
            .map(|n| match n {
                Notice::NewDriver(v) => tr("Saiu o driver {} da NVIDIA. Aguardando relatos antes do veredito.").replace("{}", v),
                Notice::NewWindows(n) => tr("{} atualizações novas do Windows. Aguardando relatos antes do veredito.").replace("{}", &n.to_string()),
                Notice::Verdict(name) => tr("Veredito pronto: {}").replace("{}", name),
            })
            .collect();
        if !msgs.is_empty() {
            app.toast.info(&format!("{}\n{}", tr("Atualizações seguras"), msgs.join("\n")));
        } else if manual {
            app.toast.info(&tr("Nada novo"));
        }
        app.invalidate();
    }
    let op = OP.lock().unwrap().take();
    if let Some((name, code)) = op {
        app.su.op = None;
        let msg = match code {
            None => "O Windows não deu a permissão de administrador",
            Some(0) => "Pronto",
            Some(2) => "Instalado. Reinicie o PC para terminar",
            Some(259) => "Ainda rodando depois de 10 minutos; confira no Windows Update",
            _ => "Não deu certo; veja o registro de ações",
        };
        app.toast.info(&tr(msg));
        if name == "policy" {
            app.su.policy = winupd::policy_on();
        } else if code.is_some() {
            start(app, false, false);
        }
        app.invalidate();
    }
    if safeupd::due(&app.su.st, app.cfg.safe_every, app.cfg.safe_wait, safeupd::now()) {
        start(app, false, false);
    }
}

/// Data e hora locais de um horário Unix.
fn when(t: u64) -> String {
    let off = crate::sys::copylist::now() - safeupd::now() as i64 / 60;
    let (y, m, d, h, mi) = crate::sys::copylist::civil(t as i64 / 60 + off);
    if crate::i18n::is_english() {
        format!("{y}-{m:02}-{d:02} {h:02}:{mi:02}")
    } else {
        format!("{d:02}/{m:02}/{y} {h:02}:{mi:02}")
    }
}

/// Corta com reticências o que não cabe em `w`.
fn clip(s: &str, size: f32, w: f32) -> String {
    if measure_raw(s, size, Weight::Regular) <= w {
        return s.to_string();
    }
    let mut out: String = s.to_string();
    while !out.is_empty() && measure_raw(&format!("{out}…"), size, Weight::Regular) > w {
        out.pop();
    }
    format!("{out}…")
}

fn wrap_h(s: &str, size: f32, w: f32) -> f32 {
    measure(s, size, Weight::Regular, Some(w)).1
}

/// Texto quebrado em linhas; devolve a altura usada.
fn para(ui: &Ui, x: f32, y: f32, w: f32, s: &str, size: f32, c: Color) -> f32 {
    let h = wrap_h(&tr(s), size, w);
    ui.c.text_wrap(rect(x, y, w, h + 2.0), s, size, c, Weight::Regular);
    h
}

/// Só página da web: `open_url` é o ShellExecute, que também abre arquivo e programa, e os links vêm
/// da IA (que lê páginas de qualquer um) e do site da NVIDIA.
fn web(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

fn host(url: &str) -> &str {
    let s = url.split("://").nth(1).unwrap_or(url);
    s.split('/').next().unwrap_or(s).trim_start_matches("www.")
}

fn error_text(e: &str) -> String {
    let why = match e.split('@').next().unwrap_or("") {
        "chave" => "a chave do Gemini foi recusada",
        "limite" => "acabou a cota grátis do Gemini por hoje",
        "rede" => "sem resposta da internet",
        "modelo" => "o modelo do Gemini não está disponível",
        _ => "a resposta veio num formato inesperado",
    };
    tr("A pesquisa falhou ({}). Tenta de novo em uma hora.").replace("{}", &tr(why))
}

/// Uma novidade: a espera, os problemas oficiais e o veredito. Devolve a altura.
fn watch(ui: &mut Ui, app: &mut App, x: f32, y0: f32, w: f32, wt: &Watch, company: &str, key: &str) -> f32 {
    let mut y = y0;
    let wait = app.cfg.safe_wait;
    if wt.judged == 0 {
        let s = tr("Visto em {a}. Veredito depois de {b}, quando já houver relatos.").replace("{a}", &when(wt.seen)).replace("{b}", &when(safeupd::verdict_at(wt, wait)));
        y += para(ui, x, y, w - 190.0, &s, 13.0, TEXT2) + 6.0;
    }
    // Também depois de um veredito sem chave: a chave colada depois vale para esta novidade.
    if wt.verdict.is_none() && (wt.judged == 0 || !app.cfg.safe_key.trim().is_empty()) {
        if ui.button_ex(&format!("su-now-{key}"), rect(x + w - 180.0, y0 - 4.0, 180.0, 34.0), "Pesquisar agora", false, !app.su.running) {
            start(app, true, true);
        }
        y = y.max(y0 + 36.0);
    }
    if wt.judged == 0 && !wt.error.is_empty() {
        y += para(ui, x, y, w, &error_text(&wt.error), 13.0, WARN) + 6.0;
    }
    let title = tr("Problemas que a {} admite").replace("{}", company);
    ui.c.text_raw(rect(x, y, w, 20.0), &title, 13.0, MUTED, Align::Left, Weight::Semibold);
    y += 22.0;
    if wt.official.is_empty() {
        y += para(ui, x, y, w, "Nenhum listado.", 13.0, TEXT2) + 6.0;
    }
    for o in &wt.official {
        y += para(ui, x + 12.0, y, w - 12.0, &format!("• {o}"), 13.0, TEXT2) + 4.0;
    }
    y += 6.0;
    let Some(v) = &wt.verdict else {
        if wt.judged != 0 {
            y += para(ui, x, y, w, "Sem a chave do Gemini, o veredito usa só as fontes oficiais acima.", 13.0, MUTED) + 6.0;
        }
        return y - y0;
    };
    let (label, col) = if v.recommend { ("Recomendado instalar", ACCENT) } else { ("Melhor esperar", WARN) };
    ui.c.text(rect(x, y, w * 0.5, 26.0), label, 17.0, col, Align::Left, Weight::Semibold);
    let pct = tr("{}% sem problemas (estimativa)").replace("{}", &v.ok_pct.to_string());
    ui.c.text_raw(rect(x + w * 0.5, y, w * 0.5, 26.0), &pct, 14.0, TEXT, Align::Right, Weight::Regular);
    y += 32.0;
    if v.bugs.is_empty() {
        y += para(ui, x, y, w, "Nenhum problema relatado.", 13.0, TEXT2) + 6.0;
    }
    for b in &v.bugs {
        let (s, c) = match b.sev {
            Sev::High => ("Grave", REC),
            Sev::Medium => ("Médio", WARN),
            Sev::Low => ("Baixo", MUTED),
        };
        let chip = rect(x, y, 64.0, 22.0);
        ui.c.round(chip, 11.0, c.alpha(0.16));
        ui.c.text_fit(chip.inset_xy(4.0, 0.0), s, 12.0, c, Align::Center, Weight::Semibold);
        y += para(ui, x + 76.0, y + 2.0, w - 76.0, &b.text, 13.0, TEXT2).max(22.0) + 6.0;
    }
    if !v.sources.is_empty() {
        ui.c.text(rect(x, y, 70.0, 22.0), "Fontes:", 13.0, MUTED, Align::Left, Weight::Regular);
        let mut sx = x + 70.0;
        for (i, s) in v.sources.iter().enumerate() {
            let name = host(s);
            let tw = measure_raw(name, 13.0, Weight::Regular) + 8.0;
            if sx + tw > x + w {
                break;
            }
            let r = rect(sx, y, tw, 22.0);
            ui.c.text_raw(r, name, 13.0, if ui.is_hover(r) { TEXT } else { ACCENT }, Align::Left, Weight::Regular);
            ui.tip(r, s);
            if ui.hit(&format!("su-src-{key}-{i}"), r) && web(s) {
                crate::report::open_url(s);
            }
            sx += tw + 10.0;
        }
        y += 28.0;
    }
    y - y0
}

fn card_title(ui: &Ui, x: f32, y: f32, w: f32, s: &str) {
    ui.c.text(rect(x, y, w, 24.0), s, 16.0, TEXT, Align::Left, Weight::Semibold);
}

/// Botão que abre um link; devolve a posição do próximo.
fn link(ui: &mut Ui, key: &str, x: f32, y: f32, label: &str, url: &str) -> f32 {
    let w = measure(&tr(label), 14.0, Weight::Semibold, None).0 + 32.0;
    if ui.button_ex(key, rect(x, y, w, 36.0), label, false, web(url)) {
        crate::report::open_url(url);
    }
    x + w + 8.0
}

fn nvidia(ui: &mut Ui, app: &mut App, x: f32, y0: f32, w: f32) -> f32 {
    let st = app.su.st.clone();
    let mut y = y0;
    card_title(ui, x, y, w, "Driver NVIDIA");
    y += 32.0;
    if st.installed.is_empty() {
        let s = if st.checked == 0 { "Ainda não conferido." } else { "Nenhuma placa NVIDIA neste PC." };
        return y + para(ui, x, y, w, s, 13.0, MUTED) - y0;
    }
    let newest = st.driver.as_ref().map_or("—".to_string(), |d| format!("{} ({})", d.version, d.date));
    let line = tr("Instalado: {a}    Mais novo: {b}").replace("{a}", &st.installed).replace("{b}", &newest);
    ui.c.text_raw(rect(x, y, w, 22.0), &line, 14.0, TEXT, Align::Left, Weight::Regular);
    y += 26.0;
    if st.nv_error {
        y += para(ui, x, y, w, "Não consegui consultar o site da NVIDIA. Tento de novo na próxima conferência.", 13.0, WARN) + 6.0;
    } else if st.nv.is_none() && st.driver.is_some() {
        y += para(ui, x, y, w, "Você já está no driver mais novo.", 13.0, TEXT2) + 6.0;
    }
    let d = st.driver.clone().unwrap_or_default();
    let version = st.nv.as_ref().map_or(st.installed.clone(), |w| w.name.clone());
    y += 4.0;
    let mut bx = link(ui, "su-nv-get", x, y, "Baixar", &d.url);
    bx = link(ui, "su-nv-notes", bx, y, "Notas da versão", &d.notes);
    link(ui, "su-nv-reddit", bx, y, "Discussão no Reddit", &safeupd::reddit_nv(&version));
    y += 48.0;
    if let Some(wt) = &st.nv {
        y += watch(ui, app, x, y, w, wt, "NVIDIA", "nv");
    }
    y - y0
}

fn windows(ui: &mut Ui, app: &mut App, x: f32, y0: f32, w: f32) -> f32 {
    let st = app.su.st.clone();
    let mut y = y0;
    card_title(ui, x, y, w, "Windows Update");
    y += 32.0;
    if !st.win_error.is_empty() {
        let s = tr("Não consegui consultar o Windows Update ({}).").replace("{}", &st.win_error);
        y += para(ui, x, y, w, &s, 13.0, WARN) + 6.0;
    } else if st.updates.is_empty() {
        let s = if st.checked == 0 { "Ainda não conferido." } else { "Nenhuma atualização pendente." };
        y += para(ui, x, y, w, s, 13.0, TEXT2) + 6.0;
    }
    for (i, u) in st.updates.iter().enumerate() {
        let mut on = app.su.sel.contains(&u.id);
        if ui.check(&format!("su-u{i}"), rect(x, y + 2.0, 24.0, 40.0), "", &mut on) {
            app.su.sel.retain(|s| s != &u.id);
            if on {
                app.su.sel.push(u.id.clone());
            }
        }
        let tx = x + 32.0;
        ui.c.text_raw(rect(tx, y + 2.0, w - 32.0, 20.0), &clip(&u.title, 14.0, w - 32.0), 14.0, if u.hidden { MUTED } else { TEXT }, Align::Left, Weight::Regular);
        let mut info: Vec<String> = [&u.kb, &u.category, &u.severity].into_iter().filter(|s| !s.is_empty()).cloned().collect();
        if u.hidden {
            info.push(tr("Bloqueada").into_owned());
        }
        ui.c.text_raw(rect(tx, y + 22.0, w - 32.0, 18.0), &info.join(" · "), 12.0, MUTED, Align::Left, Weight::Regular);
        y += 46.0;
    }
    y += 4.0;
    let ids = app.su.sel.join(",");
    let any = !ids.is_empty() && app.su.op.is_none();
    let mut bx = x;
    for (k, label, op, primary) in [("su-hide", "Bloquear", "hide", false), ("su-show", "Desbloquear", "show", false), ("su-inst", "Instalar", "install", true)] {
        let bw = measure(&tr(label), 14.0, Weight::Semibold, None).0 + 32.0;
        if ui.button_ex(k, rect(bx, y, bw, 36.0), label, primary, any) {
            elevated(app, op, format!("{op}:{ids}"));
        }
        bx += bw + 8.0;
    }
    link(ui, "su-win-reddit", bx, y, "Discussão no Reddit", &safeupd::reddit_win());
    y += 42.0;
    let note = match app.su.op {
        Some("install") => "Baixando e instalando... o Windows pede a permissão de administrador.",
        Some(_) => "Aguardando a permissão de administrador...",
        None => "Bloquear esconde a atualização no Windows Update até você desbloquear. Cada ação pede a permissão de administrador.",
    };
    y += para(ui, x, y, w, note, 12.0, MUTED) + 10.0;
    if let Some(wt) = &st.win {
        y += watch(ui, app, x, y, w, wt, "Microsoft", "win");
    }
    y - y0
}

fn options(ui: &mut Ui, app: &mut App, x: f32, y0: f32, w: f32) -> f32 {
    let mut y = y0;
    card_title(ui, x, y, w, "Opções");
    y += 34.0;
    let lw = 190.0;
    ui.c.text(rect(x, y, lw, 36.0), "Conferir", 14.0, TEXT2, Align::Left, Weight::Regular);
    let mut sel = EVERY.iter().position(|&d| d == app.cfg.safe_every).unwrap_or(0);
    if ui.segment("su-every", x + lw, y, 36.0, &EVERY_NAMES.map(|s| tr(s).into_owned()).iter().map(String::as_str).collect::<Vec<_>>(), &mut sel) {
        app.cfg.safe_every = EVERY[sel];
        app.cfg.save();
    }
    y += 44.0;
    ui.c.text(rect(x, y, lw, 36.0), "Esperar relatos por", 14.0, TEXT2, Align::Left, Weight::Regular);
    let mut sel = WAIT.iter().position(|&h| h == app.cfg.safe_wait).unwrap_or(1);
    if ui.segment("su-wait", x + lw, y, 36.0, &WAIT_NAMES.map(|s| tr(s).into_owned()).iter().map(String::as_str).collect::<Vec<_>>(), &mut sel) {
        app.cfg.safe_wait = WAIT[sel];
        app.cfg.save();
    }
    y += 50.0;
    ui.c.text(rect(x, y, lw, 36.0), "Chave do Gemini", 14.0, TEXT2, Align::Left, Weight::Regular);
    let has = !app.cfg.safe_key.trim().is_empty();
    let mut bx = x + lw;
    let bw = measure(&tr("Colar chave"), 14.0, Weight::Semibold, None).0 + 32.0;
    if ui.button("su-key", rect(bx, y, bw, 36.0), "Colar chave", false) {
        match crate::capture::clipboard_text(app.hub).map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && !s.contains(char::is_whitespace)) {
            Some(k) => {
                app.cfg.safe_key = k;
                app.cfg.save();
                app.toast.info(&tr("Chave do Gemini guardada"));
            }
            None => app.toast.info(&tr("Copie a chave antes de colar")),
        }
    }
    bx += bw + 8.0;
    let bw = measure(&tr("Apagar chave"), 14.0, Weight::Semibold, None).0 + 32.0;
    if ui.button_ex("su-key-del", rect(bx, y, bw, 36.0), "Apagar chave", false, has) {
        app.cfg.safe_key.clear();
        app.cfg.save();
    }
    bx += bw + 8.0;
    link(ui, "su-key-get", bx, y, "Pegar uma chave grátis", KEY_PAGE);
    y += 42.0;
    let s = if has {
        "Chave guardada. O Gemini pesquisa os relatos com a busca do Google (megathreads do r/nvidia e do r/Windows11, fórum da NVIDIA, Guru3D e sites de notícias) e dá o veredito. A cota grátis passa de 500 pesquisas por dia."
    } else {
        "Sem chave, o veredito usa só as fontes oficiais: os problemas conhecidos da Microsoft e os da NVIDIA nas notas da versão."
    };
    y += para(ui, x + lw, y, w - lw, s, 12.0, MUTED) + 14.0;
    let mut on = app.su.policy;
    let label = "Avisar antes de baixar atualizações do Windows e não trocar o driver de vídeo pelo Windows Update";
    if ui.check("su-policy", rect(x, y, w, 36.0), label, &mut on) && app.su.op.is_none() {
        elevated(app, "policy", format!("policy:{}", on as u8));
    }
    y += 38.0;
    y += para(ui, x + 28.0, y, w - 28.0, "Usa a política do próprio Windows (pede administrador) e desfaz ao desmarcar. O serviço do Windows Update continua ligado.", 12.0, MUTED) + 14.0;
    y += para(ui, x, y, w, "A % sem problemas é uma estimativa da IA a partir das reclamações nos fóruns: não existe essa estatística oficial.", 12.0, MUTED);
    y + 4.0 - y0
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    load(app);
    header_room(ui, a, "Atualizações seguras", "Avisa quando sai driver da NVIDIA ou atualização do Windows e diz se vale instalar, pelos relatos de quem já instalou.", 220.0);
    let label = if app.su.running { "Procurando..." } else { "Procurar agora" };
    if ui.button_ex("su-check", rect(a.right() - 196.0, 24.0, 168.0, 44.0), label, true, !app.su.running) {
        start(app, false, true);
    }
    let x = a.x + 28.0;
    let view = rect(x, 92.0, a.w - 56.0, a.h - 92.0 - 16.0);
    ui.begin_scroll("su-scroll", view, app.su.h);
    let w = view.w - 16.0;
    let mut y = view.y;
    let last = if app.su.st.checked == 0 { tr("Nunca conferido").into_owned() } else { tr("Última conferência: {}").replace("{}", &when(app.su.st.checked)) };
    ui.c.text_raw(rect(x, y, w, 20.0), &last, 13.0, MUTED, Align::Left, Weight::Regular);
    y += 28.0;
    for part in 0..3 {
        let h = app.su.cards[part];
        let card = rect(x, y, w, h + 36.0);
        ui.card(card);
        let inner = card.inset_xy(18.0, 18.0);
        let used = match part {
            0 => nvidia(ui, app, inner.x, inner.y, inner.w),
            1 => windows(ui, app, inner.x, inner.y, inner.w),
            _ => options(ui, app, inner.x, inner.y, inner.w),
        };
        if (used - h).abs() > 0.5 {
            app.su.cards[part] = used;
            app.invalidate();
        }
        y += used + 36.0 + 14.0;
    }
    ui.end_scroll();
    let total = y - view.y;
    if (total - app.su.h).abs() > 0.5 {
        app.su.h = total;
        app.invalidate();
    }
}
