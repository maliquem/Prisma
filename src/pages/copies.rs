//! Cópias: o histórico do que foi copiado de onde para onde, e as listas de cópia com agenda.
//! A cópia em si roda na janela de cópia (`paste.rs`), num prisma.exe à parte; a agenda roda aqui,
//! com o Prisma aberto (`tick`), sem nada registrado no Windows.

use std::path::PathBuf;

use super::*;
use crate::i18n::tr;
use crate::sys::copy;
use crate::sys::copylist::{self, Job, Record, When};

const ICON_FOLDER: Icon = &[&[(3., 6.), (9., 6.), (11., 8.), (21., 8.), (21., 19.), (3., 19.), (3., 6.)]];
const WHEN: [&str; 4] = ["Só quando eu mandar", "Uma vez", "Todo dia", "Dias da semana"];
const DAYS: [&str; 7] = ["Dom", "Seg", "Ter", "Qua", "Qui", "Sex", "Sáb"];
const ROW: f32 = 56.0;

/// O que a página guarda entre um quadro e outro.
#[derive(Default)]
pub struct Data {
    pub tab: usize,
    pub history: Vec<Record>,
    stamp: Option<std::time::SystemTime>,
    /// O que o Shift+Del do Prisma apagou, só para ler.
    pub erased: Vec<crate::sys::erase::Erased>,
    erased_stamp: Option<std::time::SystemTime>,
    pub search: String,
    pub jobs: Vec<Job>,
    loaded: bool,
    pub sel: Option<usize>,
    pub edit: Option<Edit>,
    /// Minuto da última olhada na agenda.
    checked: i64,
}

/// A lista aberta no editor, com o horário em campos soltos até salvar.
pub struct Edit {
    pub job: Job,
    pub kind: usize,
    pub day: u32,
    pub month: u32,
    pub hour: u32,
    pub minute: u32,
    pub days: u8,
}

impl Edit {
    fn new(job: Job) -> Edit {
        let (_, month, day, h, m) = copylist::civil(copylist::now());
        let mut e = Edit { job, kind: 0, day, month, hour: (h + 1) % 24, minute: m, days: 0b0111110 };
        match e.job.when {
            When::Manual => {}
            When::Once(t) => {
                let (_, mo, d, h, mi) = copylist::civil(t);
                (e.kind, e.month, e.day, e.hour, e.minute) = (1, mo, d, h, mi);
            }
            When::Daily(mi) => (e.kind, e.hour, e.minute) = (2, mi as u32 / 60, mi as u32 % 60),
            When::Weekly(d, mi) => (e.kind, e.days, e.hour, e.minute) = (3, d, mi as u32 / 60, mi as u32 % 60),
        }
        e
    }

    fn when(&self, now: i64) -> When {
        let min = (self.hour * 60 + self.minute) as u16;
        match self.kind {
            1 => When::Once(copylist::once_at(now, self.month, self.day, min)),
            2 => When::Daily(min),
            3 => When::Weekly(self.days, min),
            _ => When::Manual,
        }
    }

    /// A lista como vai para o arquivo, ou o motivo de ainda não dar.
    fn job(&self, now: i64) -> Result<Job, &'static str> {
        if self.job.items.is_empty() || self.job.dest.as_os_str().is_empty() {
            return Err("Escolha o que copiar e para onde");
        }
        if self.kind == 3 && self.days == 0 {
            return Err("Escolha pelo menos um dia");
        }
        Ok(Job { when: self.when(now), ..self.job.clone() })
    }
}

fn blank() -> Job {
    Job { id: 0, name: String::new(), items: Vec::new(), dest: PathBuf::new(), when: When::Manual, policy: copy::SKIP, verify: false, mv: false, last: 0 }
}

/// Data e hora de um horário em minutos locais.
fn date(t: i64) -> String {
    let (y, m, d, h, mi) = copylist::civil(t);
    if crate::i18n::is_english() {
        format!("{y}-{m:02}-{d:02} {h:02}:{mi:02}")
    } else {
        format!("{d:02}/{m:02}/{y} {h:02}:{mi:02}")
    }
}

/// Abre a janela de cópia com a lista, num prisma.exe à parte. `quiet`: agendada, sem roubar o foco.
pub fn launch(job: &Job, quiet: bool) -> bool {
    let Ok(spec) = copylist::spec_file(job) else { return false };
    let mut c = std::process::Command::new(std::env::current_exe().unwrap_or_default());
    c.arg(format!("--copy-file={}", spec.display()));
    if quiet {
        c.arg("--quiet");
    }
    // Deixa a janela nova vir para a frente (a página está em foco; a agendada não rouba o foco).
    let Ok(ch) = c.spawn() else {
        let _ = std::fs::remove_file(&spec);
        return false;
    };
    if !quiet {
        let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(ch.id()) };
    }
    true
}

fn load_history(d: &mut Data) {
    let path = copylist::history_path();
    let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    if stamp != d.stamp || d.history.is_empty() {
        d.stamp = stamp;
        d.history = copylist::load_history(&path);
    }
    // Enquanto uma exclusão grava, o arquivo está trancado: fica a lista de antes e tenta de novo.
    let path = crate::sys::erase::history_path();
    let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    if stamp != d.erased_stamp {
        if let Some(v) = crate::sys::erase::load(&path) {
            d.erased = v;
            d.erased_stamp = stamp;
        }
    }
}

/// Foto de teste (`--test-page=copies-erased`): a aba Excluídos com três exclusões de exemplo.
#[cfg(debug_assertions)]
pub fn test_erased(d: &mut Data) {
    use crate::sys::erase::Erased;
    let path = crate::sys::erase::history_path();
    d.erased_stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    d.tab = 2;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |t| t.as_secs() as i64);
    let item = |s: &str| s.to_string();
    d.erased = vec![
        Erased { at: now - 60, files: 12_344, bytes: 3_650_000_000, pending: 1, errors: 0, plan_b: true, items: vec![item(r"D:\Programas\WindowsApps antigo")] },
        Erased { at: now - 3_600, files: 3, bytes: 52_000_000, pending: 0, errors: 2, plan_b: false, items: vec![item(r"C:\Users\a\Downloads\video.mkv"), item(r"C:\Users\a\Downloads\b.zip"), item(r"C:\Users\a\Downloads\c.iso")] },
        Erased { at: now - 90_000, files: 45_210, bytes: 18_400_000_000, pending: 0, errors: 0, plan_b: false, items: vec![item(r"E:\Jogos\Jogo velho com nome bem comprido para ver o corte")] },
    ];
}

/// Sem conseguir ler o arquivo, `loaded` fica falso: tenta de novo no próximo segundo e nada grava
/// por cima das listas que estão lá.
fn load_jobs(d: &mut Data) {
    if !d.loaded {
        if let Ok(jobs) = copylist::load_jobs(&copylist::jobs_path()) {
            d.jobs = jobs;
            d.loaded = true;
        }
    }
}

/// Ao entrar na página.
pub fn enter(app: &mut App) {
    load_jobs(&mut app.cp);
    load_history(&mut app.cp);
}

/// A cada segundo: roda as listas cuja hora chegou e, com a página aberta, relê o histórico.
pub fn tick(app: &mut App) {
    load_jobs(&mut app.cp);
    let now = copylist::now();
    if now != app.cp.checked {
        app.cp.checked = now;
        let (mut ran, mut failed) = (Vec::new(), Vec::new());
        // Só marca como feita a lista que abriu: a que falhou tenta de novo no próximo minuto.
        for j in app.cp.jobs.iter_mut().filter(|j| copylist::due(j, now)) {
            if launch(j, true) {
                j.last = now;
                ran.push(j.name.clone());
            } else {
                failed.push(j.name.clone());
            }
        }
        if !failed.is_empty() {
            app.log(&format!("cópia agendada não abriu: {}", failed.join(", ")));
        }
        if !ran.is_empty() {
            let _ = copylist::save_jobs(&copylist::jobs_path(), &app.cp.jobs);
            app.log(&format!("cópia agendada: {}", ran.join(", ")));
        }
    }
    if app.page == Page::Copies {
        let before = (app.cp.stamp, app.cp.erased_stamp);
        load_history(&mut app.cp);
        if (app.cp.stamp, app.cp.erased_stamp) != before {
            app.invalidate();
        }
    }
}

/// O que veio da janela de escolher: 0 arquivos, 1 pastas (o que copiar), 2 destino.
pub fn picked(app: &mut App, kind: usize, paths: Vec<String>) {
    let Some(e) = app.cp.edit.as_mut() else { return };
    if kind == 2 {
        if let Some(p) = paths.into_iter().next() {
            e.job.dest = p.into();
        }
        return;
    }
    for p in paths.into_iter().map(PathBuf::from) {
        if !e.job.items.contains(&p) {
            e.job.items.push(p);
        }
    }
}

/// "Nova cópia": uma lista nova, já com os arquivos copiados no Explorer, se houver.
fn new_list(app: &mut App) {
    let mut job = blank();
    if let Some((items, cut)) = copy::clipboard() {
        job.items = items;
        job.mv = cut;
    }
    app.cp.tab = 1;
    app.cp.sel = None;
    app.cp.edit = Some(Edit::new(job));
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header(ui, a, "Cópias", "O que foi copiado de onde para onde, e listas de cópia para rodar quando quiser ou na hora marcada.");
    if ui.button("cp-new", rect(a.right() - 168.0, 24.0, 140.0, 44.0), "Nova cópia", true) {
        new_list(app);
    }
    let x = a.x + 28.0;
    tabs(ui, x, 88.0, &mut app.cp.tab);
    let body = rect(x, 136.0, a.w - 56.0, a.h - 136.0 - 20.0 - 44.0);
    match app.cp.tab {
        0 => history(ui, app, body),
        2 => erased(ui, app, body),
        _ => lists(ui, app, body),
    }
    let half = body.w / 2.0;
    super::adjust::paste_key_check(ui, app, rect(x, body.bottom() + 8.0, half - 8.0, 32.0));
    super::adjust::del_key_check(ui, app, rect(x + half, body.bottom() + 8.0, half, 32.0));
}

fn tabs(ui: &mut Ui, x: f32, y: f32, sel: &mut usize) {
    ui.tabs("cp-tab", x, y, &["Histórico", "Listas de cópia", "Excluídos"].map(tr), sel, true, None);
}

/// O que o Shift+Del do Prisma apagou: só o registro, sem desfazer (não tem volta).
fn erased(ui: &mut Ui, app: &mut App, b: Rect) {
    let l = rect(b.x, b.y, b.w, b.h - 52.0);
    ui.card(l);
    if app.cp.erased.is_empty() {
        ui.c.text_wrap(rect(l.x + 20.0, l.y + 20.0, l.w - 40.0, 60.0), "Nada apagado pelo Shift+Del do Prisma ainda.", 14.0, MUTED, Weight::Regular);
    }
    let list = l.inset(8.0);
    ui.begin_scroll("cp-erased", list, app.cp.erased.len() as f32 * ROW);
    let wide = list.w - 150.0 - 120.0 - 150.0;
    for (n, rec) in app.cp.erased.iter().enumerate() {
        let y = list.y + n as f32 * ROW;
        if y - ui.scroll_y() + ROW < list.y || y - ui.scroll_y() > list.bottom() {
            continue;
        }
        let r = rect(list.x, y, list.w, ROW - 2.0);
        if ui.is_hover(r) {
            ui.c.round(r, 8.0, CARD2);
        }
        ui.c.text_raw(rect(r.x + 12.0, y + 8.0, 128.0, 20.0), &date(rec.at), 13.0, TEXT2, Align::Left, Weight::Regular);
        ui.c.text(rect(r.x + 12.0, y + 28.0, 128.0, 18.0), "Apagado para sempre", 12.0, MUTED, Align::Left, Weight::Regular);
        let first = rec.items.first().map(std::path::Path::new);
        let name = first.and_then(|p| p.file_name()).map_or(String::new(), |n| n.to_string_lossy().into_owned());
        let name = if rec.items.len() > 1 { tr(&format!("{} e mais {}", name, rec.items.len() - 1)).into_owned() } else { name };
        let parent = first.and_then(|p| p.parent()).map_or(String::new(), |p| p.display().to_string());
        let fx = r.x + 150.0;
        ui.c.text_raw(rect(fx, y + 8.0, wide, 20.0), &name, 14.0, TEXT, Align::Left, Weight::Regular);
        ui.c.text_raw(rect(fx, y + 28.0, wide, 18.0), &parent, 12.0, MUTED, Align::Left, Weight::Regular);
        let sx = fx + wide + 10.0;
        ui.c.text_raw(rect(sx, y + 8.0, 110.0, 20.0), &crate::paste::size_text(rec.bytes), 13.0, TEXT2, Align::Right, Weight::Regular);
        ui.c.text_raw(rect(sx, y + 28.0, 110.0, 18.0), &tr(&format!("{} arquivos", rec.files)), 12.0, MUTED, Align::Right, Weight::Regular);
        let (label, col) = if rec.pending > 0 {
            (tr(&format!("{} no reinício", rec.pending)).into_owned(), WARN)
        } else if rec.errors == 1 {
            (tr("1 erro").into_owned(), WARN)
        } else if rec.errors > 1 {
            (tr(&format!("{} erros", rec.errors)).into_owned(), WARN)
        } else {
            (tr("Apagado").into_owned(), ACCENT)
        };
        let cw = (measure(&label, 12.0, Weight::Semibold, None).0 + 20.0).min(140.0);
        let chip = rect(sx + 120.0 + (140.0 - cw) / 2.0, y + 15.0, cw, 24.0);
        ui.c.round(chip, 12.0, col.alpha(0.16));
        ui.c.text_fit(chip.inset_xy(6.0, 0.0), &label, 12.0, col, Align::Center, Weight::Semibold);
        // Na dica: o que foi escolhido, o que ficou para o reinício e se precisou do plano B.
        let mut tip: Vec<String> = rec.items.iter().take(8).cloned().collect();
        if rec.items.len() > 8 {
            tip.push(tr(&format!("e mais {}", rec.items.len() - 8)).into_owned());
        }
        if rec.pending > 0 {
            tip.push(tr(&format!("{} itens ficaram para o próximo reinício", rec.pending)).into_owned());
        }
        if rec.plan_b {
            tip.push(tr("Usou o plano B (takeown e icacls)").into_owned());
        }
        ui.tip(r, &tip.join("\n"));
    }
    ui.end_scroll();
    ui.c.text(rect(b.x + 4.0, l.bottom() + 12.0, b.w, 40.0), "Só o registro do que o Shift+Del do Prisma apagou: não passa pela Lixeira e não tem volta. Guarda as últimas 500.", 13.0, MUTED, Align::Left, Weight::Regular);
}

fn result_chip(r: &Record) -> (String, Color) {
    match r.result {
        copylist::VERIFIED => (tr("Conferido").into_owned(), ACCENT),
        copylist::ERRORS if r.errors.len() == 1 => (tr("1 erro").into_owned(), WARN),
        copylist::ERRORS => (tr(&format!("{} erros", r.errors.len())).into_owned(), WARN),
        copylist::CANCELED => (tr("Cancelado").into_owned(), MUTED),
        copylist::NO_SPACE => (tr("Sem espaço").into_owned(), WARN),
        _ => (tr("Pronto").into_owned(), ACCENT),
    }
}

fn history(ui: &mut Ui, app: &mut App, b: Rect) {
    ui.search("cp-search", rect(b.right() - 280.0, 88.0, 280.0, 34.0), &mut app.cp.search, "Procurar no histórico");
    let q = app.cp.search.to_lowercase();
    let shown: Vec<usize> = (0..app.cp.history.len())
        .filter(|&i| {
            let r = &app.cp.history[i];
            q.is_empty() || r.name.to_lowercase().contains(&q) || r.dest.to_lowercase().contains(&q) || r.items.iter().any(|s| s.to_lowercase().contains(&q))
        })
        .collect();

    let l = rect(b.x, b.y, b.w, b.h - 52.0);
    ui.card(l);
    if app.cp.history.is_empty() {
        ui.c.text_wrap(rect(l.x + 20.0, l.y + 20.0, l.w - 40.0, 60.0), "Nenhuma cópia ainda. As cópias feitas pela janela do Prisma aparecem aqui.", 14.0, MUTED, Weight::Regular);
    }
    let list = l.inset(8.0);
    ui.begin_scroll("cp-hist", list, shown.len() as f32 * ROW);
    let (mut open, mut again) = (None, None);
    let wide = list.w - 140.0 - 120.0 - 120.0 - 84.0;
    for (n, &i) in shown.iter().enumerate() {
        let y = list.y + n as f32 * ROW;
        if y - ui.scroll_y() + ROW < list.y || y - ui.scroll_y() > list.bottom() {
            continue;
        }
        let rec = &app.cp.history[i];
        let r = rect(list.x, y, list.w, ROW - 2.0);
        if ui.is_hover(r) {
            ui.c.round(r, 8.0, CARD2);
        }
        ui.c.text_raw(rect(r.x + 12.0, y + 8.0, 128.0, 20.0), &date(rec.at), 13.0, TEXT2, Align::Left, Weight::Regular);
        ui.c.text(rect(r.x + 12.0, y + 28.0, 128.0, 18.0), if rec.mv { "Movido" } else { "Copiado" }, 12.0, MUTED, Align::Left, Weight::Regular);
        // De → Para: o nome da lista, ou o primeiro item e quantos mais.
        let from = if !rec.name.is_empty() {
            rec.name.clone()
        } else {
            let first = rec.items.first().map_or(String::new(), |s| std::path::Path::new(s).file_name().map_or(s.clone(), |n| n.to_string_lossy().into_owned()));
            if rec.items.len() > 1 { tr(&format!("{} e mais {}", first, rec.items.len() - 1)).into_owned() } else { first }
        };
        let fx = r.x + 150.0;
        ui.c.text_raw(rect(fx, y + 8.0, wide, 20.0), &from, 14.0, TEXT, Align::Left, Weight::Regular);
        ui.c.text_raw(rect(fx, y + 28.0, wide, 18.0), &format!("→ {}", rec.dest), 12.0, MUTED, Align::Left, Weight::Regular);
        let sx = fx + wide + 10.0;
        ui.c.text_raw(rect(sx, y + 8.0, 110.0, 20.0), &crate::paste::size_text(rec.bytes), 13.0, TEXT2, Align::Right, Weight::Regular);
        ui.c.text_raw(rect(sx, y + 28.0, 110.0, 18.0), &tr(&format!("{} arquivos", rec.files)), 12.0, MUTED, Align::Right, Weight::Regular);
        let (label, col) = result_chip(rec);
        let cw = measure(&label, 12.0, Weight::Semibold, None).0 + 20.0;
        let chip = rect(sx + 120.0 + (110.0 - cw.min(110.0)) / 2.0, y + 15.0, cw.min(110.0), 24.0);
        ui.c.round(chip, 12.0, col.alpha(0.16));
        ui.c.text_fit(chip.inset_xy(6.0, 0.0), &label, 12.0, col, Align::Center, Weight::Semibold);
        if !rec.errors.is_empty() {
            let why: Vec<String> = rec.errors.iter().take(5).map(|(p, w)| format!("{} — {}", std::path::Path::new(p).file_name().map_or(p.clone(), |n| n.to_string_lossy().into_owned()), tr(w))).collect();
            ui.tip(chip, &why.join("\n"));
        }
        let ib = rect(r.right() - 80.0, y + 11.0, 32.0, 32.0);
        let ia = rect(r.right() - 42.0, y + 11.0, 32.0, 32.0);
        for (k, (s, icon, tip)) in [(ib, ICON_FOLDER, "Abrir o destino"), (ia, ICON_AGAIN, "Copiar de novo")].into_iter().enumerate() {
            let hov = ui.is_hover(s);
            if hov {
                ui.c.round(s, 8.0, LINE);
            }
            ui.c.icon(s.x + 7.0, s.y + 7.0, 18.0, icon, if hov { TEXT } else { TEXT2 });
            ui.tip(s, tip);
            if ui.hit(&format!("cp-h{k}-{i}"), s) {
                if k == 0 { open = Some(i) } else { again = Some(i) }
            }
        }
    }
    ui.end_scroll();
    if let Some(i) = open {
        let dest = app.cp.history[i].dest.clone();
        let _ = std::process::Command::new(crate::sys::system_tool("explorer")).arg(dest).spawn();
    }
    if let Some(i) = again {
        let rec = &app.cp.history[i];
        let job = Job { name: rec.name.clone(), items: rec.items.iter().map(PathBuf::from).collect(), dest: rec.dest.clone().into(), mv: rec.mv, ..blank() };
        launch(&job, false);
    }

    let fy = l.bottom() + 12.0;
    ui.c.text(rect(b.x + 4.0, fy, b.w - 220.0, 40.0), "Guarda as últimas 500 cópias: caminhos, tamanhos e resultado, na pasta de dados do Prisma.", 13.0, MUTED, Align::Left, Weight::Regular);
    if ui.button_ex("cp-clear", rect(b.right() - 200.0, fy, 200.0, 40.0), "Limpar histórico", false, !app.cp.history.is_empty()) {
        ui.ask("cp-clear-ask", "Limpar o histórico?", "As cópias somem desta lista. Os arquivos copiados não mudam.");
    }
    if ui.answered("cp-clear-ask") {
        let _ = copylist::clear_history(&copylist::history_path());
        app.cp.history.clear();
        app.cp.stamp = None;
    }
}

/// Quando a lista roda, em poucas palavras.
fn when_text(j: &Job) -> String {
    match copylist::next_slot(j.when, copylist::now()) {
        _ if j.when == When::Manual => tr("Só quando eu mandar").into_owned(),
        Some(t) => tr(&format!("Próxima: {}", date(t))).into_owned(),
        None => tr("Já rodou").into_owned(),
    }
}

fn lists(ui: &mut Ui, app: &mut App, b: Rect) {
    let lw = 300.0;
    let l = rect(b.x, b.y, lw, b.h);
    ui.card(l);
    let list = rect(l.x + 8.0, l.y + 8.0, l.w - 16.0, l.h - 68.0);
    if app.cp.jobs.is_empty() {
        ui.c.text_wrap(rect(l.x + 16.0, l.y + 16.0, l.w - 32.0, 80.0), "Nenhuma lista ainda. Uma lista junta arquivos e pastas de vários lugares para copiar juntos.", 13.0, MUTED, Weight::Regular);
    }
    ui.begin_scroll("cp-jobs", list, app.cp.jobs.len() as f32 * 54.0);
    let mut pick = None;
    for (i, j) in app.cp.jobs.iter().enumerate() {
        let y = list.y + i as f32 * 54.0;
        let r = rect(list.x, y, list.w, 52.0);
        if app.cp.sel == Some(i) {
            ui.c.round(r, 8.0, CARD2);
        }
        if ui.hit(&format!("cp-j{i}"), r) {
            pick = Some(i);
        }
        let name = if j.name.is_empty() { tr("Lista sem nome").into_owned() } else { j.name.clone() };
        ui.c.text_raw(rect(r.x + 12.0, y + 7.0, r.w - 24.0, 20.0), &name, 14.0, TEXT, Align::Left, Weight::Regular);
        ui.c.text_raw(rect(r.x + 12.0, y + 27.0, r.w - 24.0, 18.0), &when_text(j), 12.0, MUTED, Align::Left, Weight::Regular);
    }
    ui.end_scroll();
    if let Some(i) = pick {
        app.cp.sel = Some(i);
        app.cp.edit = Some(Edit::new(app.cp.jobs[i].clone()));
    }
    if ui.button("cp-add", rect(l.x + 12.0, l.bottom() - 54.0, l.w - 24.0, 40.0), "Nova lista", false) {
        app.cp.sel = None;
        app.cp.edit = Some(Edit::new(blank()));
    }

    let r = rect(l.right() + 12.0, b.y, b.w - lw - 12.0, b.h);
    ui.card(r);
    if app.cp.edit.is_none() {
        ui.c.text_wrap(rect(r.x + 20.0, r.y + 20.0, r.w - 40.0, 60.0), "Escolha uma lista para ver e mudar, ou crie uma nova.", 14.0, MUTED, Weight::Regular);
        return;
    }
    editor(ui, app, r);
}

fn editor(ui: &mut Ui, app: &mut App, r: Rect) {
    let host = app.hub;
    let Some(e) = app.cp.edit.as_mut() else { return };
    let x = r.x + 20.0;
    let w = r.w - 40.0;
    let area = rect(r.x, r.y + 8.0, r.w, r.h - 76.0);
    let content = 560.0 + e.job.items.len().max(1) as f32 * 30.0;
    ui.begin_scroll("cp-edit", area, content);
    let mut y = area.y + 8.0;

    ui.label(rect(x, y, w, 18.0), "Nome", MUTED);
    ui.search("cp-name", rect(x, y + 22.0, w.min(420.0), 36.0), &mut e.job.name, "Ex.: Backup das fotos");
    y += 72.0;

    ui.label(rect(x, y, w, 18.0), "O que copiar", MUTED);
    y += 24.0;
    let mut drop = None;
    if e.job.items.is_empty() {
        ui.c.text(rect(x, y, w, 30.0), "Nada ainda.", 13.0, MUTED, Align::Left, Weight::Regular);
        y += 30.0;
    }
    for (i, p) in e.job.items.iter().enumerate() {
        let row = rect(x, y, w, 28.0);
        ui.c.text_raw(rect(x + 8.0, y, w - 48.0, 28.0), &p.display().to_string(), 13.0, TEXT2, Align::Left, Weight::Regular);
        let xr = rect(row.right() - 30.0, y + 2.0, 24.0, 24.0);
        let hov = ui.is_hover(xr);
        ui.c.icon(xr.x + 4.0, xr.y + 4.0, 16.0, ICON_X, if hov { WARN } else { MUTED });
        ui.tip(xr, "Tirar da lista");
        if ui.hit(&format!("cp-x{i}"), xr) {
            drop = Some(i);
        }
        y += 30.0;
    }
    if let Some(i) = drop {
        e.job.items.remove(i);
    }
    let post = |kind: usize| unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(Some(host), crate::app::WM_PICK_COPY, windows::Win32::Foundation::WPARAM(kind), windows::Win32::Foundation::LPARAM(0));
    };
    if ui.button("cp-files", rect(x, y + 4.0, 190.0, 36.0), "Adicionar arquivos", false) {
        post(0);
    }
    if ui.button("cp-folder", rect(x + 198.0, y + 4.0, 190.0, 36.0), "Adicionar pasta", false) {
        post(1);
    }
    y += 56.0;

    ui.label(rect(x, y, w, 18.0), "Para onde", MUTED);
    let dr = rect(x, y + 22.0, w - 150.0, 36.0);
    ui.c.round(dr, 8.0, SIDE);
    ui.c.round_line(dr, 8.0, BORDER, 1.0);
    let dest = e.job.dest.display().to_string();
    if dest.is_empty() {
        ui.c.text(dr.inset_xy(10.0, 0.0), "Nenhuma pasta escolhida", 14.0, MUTED, Align::Left, Weight::Regular);
    } else {
        ui.c.text_raw(dr.inset_xy(10.0, 0.0), &dest, 14.0, TEXT, Align::Left, Weight::Regular);
    }
    if ui.button("cp-dest", rect(dr.right() + 10.0, dr.y, 140.0, 36.0), "Escolher", false) {
        post(2);
    }
    y += 76.0;

    ui.label(rect(x, y, w, 18.0), "Quando", MUTED);
    y += 24.0;
    let mut bx = x;
    for (i, t) in WHEN.iter().enumerate() {
        let tw = measure(&tr(t), 14.0, Weight::Semibold, None).0 + 28.0;
        if ui.button_ex(&format!("cp-w{i}"), rect(bx, y, tw, 36.0), t, e.kind == i, true) {
            e.kind = i;
        }
        bx += tw + 6.0;
    }
    y += 46.0;
    if e.kind == 3 {
        for (i, d) in DAYS.iter().enumerate() {
            let on = e.days & (1 << i) != 0;
            if ui.button_ex(&format!("cp-d{i}"), rect(x + i as f32 * 58.0, y, 52.0, 34.0), d, on, true) {
                e.days ^= 1 << i;
            }
        }
        y += 44.0;
    }
    if e.kind > 0 {
        let mut sx = x;
        let mut field = |ui: &mut Ui, key: &str, label: &str, v: &mut u32, range: (u32, u32)| {
            ui.label(rect(sx, y, 120.0, 18.0), label, MUTED);
            ui.stepper(key, rect(sx, y + 20.0, 120.0, 34.0), v, range, 1, "");
            sx += 130.0;
        };
        if e.kind == 1 {
            field(ui, "cp-day", "Dia", &mut e.day, (1, 31));
            field(ui, "cp-month", "Mês", &mut e.month, (1, 12));
        }
        field(ui, "cp-hour", "Hora", &mut e.hour, (0, 23));
        field(ui, "cp-min", "Minuto", &mut e.minute, (0, 59));
        y += 66.0;
    }

    ui.label(rect(x, y, w, 18.0), "Se o arquivo já existir no destino", MUTED);
    y += 24.0;
    let sw = ((w.min(560.0) - 12.0) / 3.0).floor();
    for (i, (t, p)) in [("Pular", copy::SKIP), ("Substituir", copy::REPLACE), ("Manter os dois", copy::KEEP_BOTH)].into_iter().enumerate() {
        if ui.button_ex(&format!("cp-p{i}"), rect(x + i as f32 * (sw + 6.0), y, sw, 36.0), t, e.job.policy == p, true) {
            e.job.policy = p;
        }
    }
    y += 48.0;
    ui.check("cp-verify", rect(x, y, w, 30.0), "Conferir com hash (SHA-256)", &mut e.job.verify);
    ui.check("cp-mv", rect(x, y + 32.0, w, 30.0), "Mover em vez de copiar", &mut e.job.mv);
    y += 74.0;
    ui.c.text_wrap(rect(x, y, w, 40.0), "A cópia agendada roda com o Prisma aberto na bandeja. Se o PC estiver desligado na hora, ela roda assim que o Prisma abrir.", 13.0, MUTED, Weight::Regular);
    ui.end_scroll();

    // Botões fixos embaixo.
    let by = r.bottom() - 56.0;
    let saved = app.cp.sel.is_some();
    if ui.button_ex("cp-del", rect(x, by, 150.0, 40.0), "Apagar lista", false, saved) {
        ui.ask("cp-del-ask", "Apagar esta lista?", "A lista e a agenda dela somem. Os arquivos não mudam.");
    }
    let now = copylist::now();
    if ui.button("cp-run", rect(r.right() - 20.0 - 2.0 * 150.0 - 10.0, by, 150.0, 40.0), "Copiar agora", false) {
        match e.job(now) {
            Ok(j) => {
                if !launch(&j, false) {
                    app.toast.info("Não deu para abrir a janela de cópia");
                }
            }
            Err(why) => app.toast.info(why),
        }
    }
    if ui.button("cp-save", rect(r.right() - 20.0 - 150.0, by, 150.0, 40.0), "Salvar", true) {
        match e.job(now) {
            Ok(_) if !app.cp.loaded => app.toast.info("Não deu para salvar a lista"),
            Ok(mut j) => {
                // Salvar conta como a última vez: um horário que já passou hoje não roda na hora.
                j.last = now;
                let d = &mut app.cp;
                match d.sel {
                    Some(i) if i < d.jobs.len() => d.jobs[i] = j.clone(),
                    _ => {
                        j.id = d.jobs.iter().map(|o| o.id).max().unwrap_or(0) + 1;
                        d.jobs.push(j.clone());
                        d.sel = Some(d.jobs.len() - 1);
                    }
                }
                d.edit = Some(Edit::new(j));
                if copylist::save_jobs(&copylist::jobs_path(), &d.jobs).is_ok() {
                    app.toast.info("Lista salva");
                } else {
                    app.toast.info("Não deu para salvar a lista");
                }
            }
            Err(why) => app.toast.info(why),
        }
    }
    if ui.answered("cp-del-ask") {
        let d = &mut app.cp;
        if let Some(i) = d.sel.filter(|&i| i < d.jobs.len()) {
            d.jobs.remove(i);
            let _ = copylist::save_jobs(&copylist::jobs_path(), &d.jobs);
        }
        d.sel = None;
        d.edit = None;
    }
}
