//! Discos e redes: tudo que está ligado no PC, com saúde dos discos e as ferramentas do Windows
//! (CHKDSK, formatar, partições e letra).

use super::*;
use crate::sys::disks::{self, Health, Job, Level, Volume, FILE_SYSTEMS};
use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Repair,
    Format,
    Letter,
    Resize,
    Delete,
    Create,
}

/// Janela de confirmação aberta: o pedido e o que a pessoa preenche nela.
struct Sheet {
    kind: Kind,
    vol: Volume,
    disk: u32,
    disk_name: String,
    fs: usize,
    label: String,
    quick: bool,
    sectors: bool,
    /// GB digitados (Redimensionar e Criar partição).
    size: String,
    /// Letras livres para Trocar letra; a opção depois delas é "Sem letra".
    letters: Vec<char>,
    to: usize,
    /// A letra digitada para confirmar formatar e apagar.
    typed: String,
}

impl Sheet {
    /// O pedido do que está preenchido; None com tamanho que não é número.
    fn job(&self) -> Option<Job> {
        let letter = self.vol.letter;
        let size = self.size.trim();
        Some(match self.kind {
            Kind::Repair => Job::Repair { letter, sectors: self.sectors },
            Kind::Format => Job::Format { letter, fs: self.fs, label: self.label.trim().to_string(), quick: self.quick },
            Kind::Letter => Job::Letter { letter, to: self.letters.get(self.to).copied() },
            Kind::Resize => Job::Resize { letter, gb: size.parse().ok()? },
            Kind::Delete => Job::Delete(letter),
            Kind::Create => Job::Create { disk: self.disk, gb: if size.is_empty() { None } else { Some(size.parse().ok()?) }, fs: self.fs, label: self.label.trim().to_string() },
        })
    }

    /// Em português: a tradução vem ao desenhar.
    fn title(&self) -> String {
        let v = vol_name(&self.vol);
        match self.kind {
            Kind::Repair => format!("Reparar {v}"),
            Kind::Format => format!("Formatar {v}"),
            Kind::Letter => format!("Trocar a letra de {v}"),
            Kind::Resize => format!("Redimensionar {v}"),
            Kind::Delete => format!("Apagar a partição {v}"),
            Kind::Create => format!("Criar partição em {}", self.disk_name),
        }
    }
}

/// Um pedido rodando (ou terminado) no PowerShell como administrador.
struct Running {
    title: String,
    job: Job,
    log: PathBuf,
    p: disks::Progress,
    at: Instant,
    read: Instant,
}

#[derive(Default)]
struct St {
    /// 0 Saúde, 1 Ferramentas.
    view: usize,
    /// Volume escolhido no disco.
    vol: usize,
    sheet: Option<Sheet>,
    run: Option<Running>,
}

thread_local! {
    static ST: RefCell<St> = RefCell::new(St::default());
}

fn vol_name(v: &Volume) -> String {
    if v.label.is_empty() {
        format!("{}:", v.letter)
    } else {
        format!("{}: ({})", v.letter, v.label)
    }
}

/// A letra do Windows e as letras do disco onde ele está.
fn system_letters(list: &[disks::Disk]) -> (char, Vec<char>) {
    let win = crate::sys::windows_dir().to_string_lossy().chars().next().unwrap_or('C').to_ascii_uppercase();
    let sys = list.iter().find(|d| d.volumes.iter().any(|v| v.letter == win)).map_or(vec![win], |d| d.volumes.iter().map(|v| v.letter).collect());
    (win, sys)
}

/// O Prisma (o .exe ou a pasta de dados) está no volume.
fn prisma_on(letter: char) -> bool {
    [std::env::current_exe().unwrap_or_default(), crate::config::data_dir()].iter().any(|p| p.to_string_lossy().chars().next().is_some_and(|c| c.eq_ignore_ascii_case(&letter)))
}

fn level_color(l: Level) -> Color {
    match l {
        Level::Good => ACCENT,
        Level::Warn => WARN,
        Level::Risk => REC,
        Level::Unknown => MUTED,
    }
}

fn level_name(l: Level) -> &'static str {
    match l {
        Level::Good => "Bom",
        Level::Warn => "Atenção",
        Level::Risk => "Risco",
        Level::Unknown => "Sem dados",
    }
}

fn fmt_size(b: u64) -> String {
    if b >= 1_000_000_000_000 {
        format!("{:.1} TB", b as f64 / 1e12).replace('.', crate::i18n::comma())
    } else {
        format!("{} GB", b / 1_000_000_000)
    }
}

fn fmt_hours(h: u64) -> String {
    if h >= 24 * 365 {
        format!("{h} h ({:.1} anos)", h as f64 / (24.0 * 365.0)).replace('.', crate::i18n::comma())
    } else {
        format!("{h} h")
    }
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    if ST.with(|s| s.borrow().sheet.is_some()) {
        ui.block();
    }
    header(ui, a, "Discos e redes", "Tudo que está ligado no PC, em tempo real.");
    const TABS: [&str; 2] = ["Discos", "Redes"];
    let mut tab = app.store_tab as usize;
    if ui.segment("st-tab", a.right() - 28.0 - Ui::segment_width(&TABS), 30.0, 36.0, &TABS, &mut tab) {
        app.store_tab = tab as u8;
    }
    if app.store_tab == 0 {
        draw_disks(ui, app, a);
    } else {
        draw_nets(ui, app, a);
    }
}

fn draw_disks(ui: &mut Ui, app: &mut App, a: Rect) {
    let x = a.x + 28.0;
    let top = 88.0;
    let h = a.h - top - 20.0;
    let rw = 320.0;
    let lw = a.w - 56.0 - rw - 12.0;
    let health = app.health.lock().unwrap().clone();

    let list = rect(x, top, lw, h);
    let row_h = 112.0;
    ui.begin_scroll("disks", list, app.disks.len() as f32 * (row_h + 10.0));
    let mut pick = None;
    for (i, d) in app.disks.iter().enumerate() {
        let r = rect(list.x, list.y + i as f32 * (row_h + 10.0), list.w, row_h);
        let sel = app.disk_sel == i;
        if sel {
            ui.card_on(r);
        } else {
            ui.card(r);
        }
        if ui.hit(&format!("d{i}"), r) {
            pick = Some(i);
        }
        let hl = health.get(&d.number);
        let lvl = hl.map_or(Level::Unknown, |h| h.level);
        let tx = r.x + 16.0;
        let name = if d.name.is_empty() { format!("Disco {}", d.number) } else { d.name.clone() };
        let side = 196.0;
        ui.c.text(rect(tx, r.y + 10.0, r.w - side - 32.0, 22.0), &name, 15.0, TEXT, Align::Left, Weight::Semibold);
        let letters: Vec<String> = d.volumes.iter().map(|v| format!("{}:", v.letter)).collect();
        let sub = format!("{} {} · {} · {}", d.kind(), d.bus_name(), fmt_size(d.size), letters.join(" "));
        ui.c.text(rect(tx, r.y + 33.0, r.w - side - 32.0, 18.0), &sub, 13.0, MUTED, Align::Left, Weight::Regular);

        // Espaço usado somando os volumes do disco.
        let (total, free) = d.volumes.iter().fold((0, 0), |s, v| (s.0 + v.total, s.1 + v.free));
        if total > 0 {
            let used = 1.0 - free as f32 / total as f32;
            let bar = rect(tx, r.y + 62.0, r.w - side - 32.0, 6.0);
            ui.c.round(bar, 3.0, SIDE);
            ui.c.round(rect(bar.x, bar.y, bar.w * used, bar.h), 3.0, if used > 0.95 { WARN } else { ACCENT });
            let t = format!("{} livres de {}", fmt_size(free), fmt_size(total));
            ui.c.text(rect(tx, r.y + 74.0, bar.w, 18.0), &t, 12.0, MUTED, Align::Left, Weight::Regular);
        }

        let sx = r.right() - side;
        let color = level_color(lvl);
        ui.c.circle(sx + 6.0, r.y + 21.0, 5.0, color);
        ui.c.text(rect(sx + 18.0, r.y + 10.0, 80.0, 22.0), level_name(lvl), 14.0, color, Align::Left, Weight::Semibold);
        if let Some(t) = hl.and_then(|h| h.temp) {
            ui.c.text(rect(sx + 90.0, r.y + 10.0, 90.0, 22.0), &format!("{t} °C"), 13.0, TEXT2, Align::Right, Weight::Regular);
        }
        let line = format!("Ler {} · Gravar {}", fmt_rate(d.read), fmt_rate(d.write));
        ui.c.text(rect(sx, r.y + 34.0, side - 16.0, 18.0), &line, 12.0, TEXT2, Align::Left, Weight::Regular);
        ui.c.text(rect(sx, r.y + 52.0, side - 16.0, 18.0), &format!("Ativo {:.0}%", d.active), 12.0, MUTED, Align::Left, Weight::Regular);
        ui.c.sparkline(rect(sx, r.y + 74.0, side - 16.0, 26.0), &d.hist, ACCENT, 1.5);
    }
    ui.end_scroll();
    if let Some(i) = pick {
        app.disk_sel = i;
    }

    let r = rect(list.right() + 12.0, top, rw, h);
    ui.card(r);
    ST.with(|s| {
        let mut s = s.borrow_mut();
        if pick.is_some() {
            s.vol = 0;
        }
        poll(app, &mut s);
        let mut view = s.view;
        if ui.segment("st-view", r.x + 10.0, r.y + 8.0, 32.0, &["Saúde", "Ferramentas"], &mut view) {
            s.view = view;
        }
        if s.view == 1 {
            draw_tools(ui, app, &mut s, r);
        } else {
            draw_health(ui, app, r, &health);
        }
        if s.sheet.is_some() {
            draw_sheet(ui, app, &mut s);
        }
    });
}

fn draw_health(ui: &mut Ui, app: &mut App, r: Rect, health: &std::collections::HashMap<u32, Health>) {
    let Some(d) = app.disks.get(app.disk_sel) else { return };
    let empty = Health::default();
    let hl = health.get(&d.number);
    let lvl = hl.map_or(Level::Unknown, |h| h.level);
    let v = rect(r.x + 16.0, r.y + 40.0, r.w - 32.0, 40.0);
    ui.c.text(v, level_name(lvl), 26.0, level_color(lvl), Align::Left, Weight::Semibold);
    let summary = hl.map_or("Lendo o disco…", |h| h.summary.as_str());
    ui.c.text_wrap(rect(r.x + 16.0, r.y + 84.0, r.w - 32.0, 44.0), summary, 13.0, TEXT2, Weight::Regular);
    let h0 = hl.unwrap_or(&empty);
    let mut rows: Vec<(String, String)> = Vec::new();
    if let Some(t) = h0.temp {
        rows.push(("Temperatura".into(), format!("{t} °C")));
    }
    if let Some(v) = h0.hours {
        rows.push(("Tempo ligado".into(), fmt_hours(v)));
    }
    rows.extend(h0.details.iter().cloned());
    rows.push(("Número de série".into(), d.serial.clone()));
    let mut y = r.y + 132.0;
    for (k, v) in &rows {
        ui.c.line(r.x + 16.0, y, r.right() - 16.0, y, LINE, 1.0);
        ui.label(rect(r.x + 16.0, y + 5.0, 150.0, 20.0), k, MUTED);
        ui.c.text(rect(r.x + 130.0, y + 5.0, r.w - 146.0, 20.0), v, 13.0, TEXT, Align::Right, Weight::Regular);
        y += 30.0;
        if y + 30.0 > r.bottom() - 52.0 {
            break;
        }
    }
    if ui.button("hl", rect(r.x + 16.0, r.bottom() - 52.0, r.w - 32.0, 40.0), "Ler saúde de novo", false) {
        app.health.lock().unwrap().clear();
        disks::spawn_health(&app.disks, app.health.clone());
    }
}

/// Lê o log do pedido rodando; ao terminar uma mudança de partição, lê os discos de novo.
fn poll(app: &mut App, s: &mut St) {
    let Some(run) = s.run.as_mut() else { return };
    if run.p.done.is_some() || run.read.elapsed() < Duration::from_millis(500) {
        return;
    }
    run.read = Instant::now();
    let Ok(bytes) = std::fs::read(&run.log) else { return };
    run.p = disks::parse_log(&String::from_utf8_lossy(&bytes));
    if run.p.done.is_some() && !matches!(run.job, Job::Scan(_) | Job::Repair { .. }) {
        app.disks = disks::list();
        app.disk_sel = app.disk_sel.min(app.disks.len().saturating_sub(1));
        s.vol = 0;
    }
}

/// Monta o comando e pede o administrador ao Windows. O log fica na pasta temporária.
fn start(app: &mut App, s: &mut St, job: Job, title: String) {
    let (win, sys) = system_letters(&app.disks);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let log = std::env::temp_dir().join(format!("prisma-disco-{}-{stamp}.log", std::process::id()));
    let script = match disks::script(&job, win, &sys, &log.to_string_lossy()) {
        Ok(script) => script,
        Err(e) => return app.toast.info(e),
    };
    if !disks::run_job(&script) {
        return app.toast.info("O Windows não deu a permissão de administrador.");
    }
    if let Some(old) = s.run.take() {
        let _ = std::fs::remove_file(old.log);
    }
    s.run = Some(Running { title, job, log, p: Default::default(), at: Instant::now(), read: Instant::now() });
    s.sheet = None;
}

fn new_sheet(kind: Kind, vol: &Volume, d: &disks::Disk) -> Sheet {
    Sheet {
        kind,
        vol: vol.clone(),
        disk: d.number,
        disk_name: if d.name.is_empty() { format!("Disco {}", d.number) } else { d.name.clone() },
        fs: 0,
        label: if kind == Kind::Format { vol.label.clone() } else { String::new() },
        quick: true,
        sectors: false,
        size: if kind == Kind::Resize { (vol.total / 1_000_000_000).to_string() } else { String::new() },
        letters: disks::free_letters(),
        to: 0,
        typed: String::new(),
    }
}

fn draw_tools(ui: &mut Ui, app: &mut App, s: &mut St, r: Rect) {
    if s.run.is_some() {
        return draw_run(ui, app, s, r);
    }
    let Some(d) = app.disks.get(app.disk_sel).cloned() else { return };
    let (win, sys) = system_letters(&app.disks);
    let x = r.x + 16.0;
    let w = r.w - 32.0;
    let bw = (w - 8.0) / 2.0;
    let mut y = r.y + 54.0;
    let mut open = None;
    let mut vol = Volume::default();
    if d.volumes.is_empty() {
        ui.c.text_wrap(rect(x, y, w, 40.0), "Este disco não tem volume com letra.", 13.0, MUTED, Weight::Regular);
        y += 48.0;
    } else {
        let names: Vec<String> = d.volumes.iter().map(vol_name).collect();
        s.vol = s.vol.min(names.len() - 1);
        ui.select("st-vol", rect(x, y, w, 34.0), &names, &mut s.vol);
        y += 46.0;
        vol = d.volumes[s.vol].clone();
        let l = vol.letter;
        ui.c.text(rect(x, y, w, 20.0), "Verificar e reparar (CHKDSK)", 13.0, TEXT2, Align::Left, Weight::Semibold);
        y += 24.0;
        if ui.button("st-scan", rect(x, y, bw, 34.0), "Analisar", false) {
            start(app, s, Job::Scan(l), format!("Analisar {}", vol_name(&vol)));
        }
        if ui.button("st-repair", rect(x + bw + 8.0, y, bw, 34.0), "Reparar…", false) {
            open = Some(Kind::Repair);
        }
        y += 40.0;
        ui.c.text_wrap(rect(x, y, w, 34.0), "Analisar só lê o disco. Reparar mexe nele: leia os riscos antes.", 12.0, MUTED, Weight::Regular);
        y += 40.0;
        ui.c.text(rect(x, y, w, 20.0), "Volume", 13.0, TEXT2, Align::Left, Weight::Semibold);
        y += 24.0;
        let locked = l == win || sys.contains(&l);
        if ui.button_ex("st-fmt", rect(x, y, bw, 34.0), "Formatar…", false, !locked) {
            open = Some(Kind::Format);
        }
        if ui.button_ex("st-let", rect(x + bw + 8.0, y, bw, 34.0), "Trocar letra…", false, l != win) {
            open = Some(Kind::Letter);
        }
        y += 40.0;
        if ui.button("st-size", rect(x, y, bw, 34.0), "Redimensionar…", false) {
            open = Some(Kind::Resize);
        }
        if ui.button_ex("st-del", rect(x + bw + 8.0, y, bw, 34.0), "Apagar partição…", false, !locked) {
            open = Some(Kind::Delete);
        }
        y += 40.0;
        if locked {
            ui.c.text_wrap(rect(x, y, w, 34.0), "Disco do Windows: formatar e apagar ficam bloqueados.", 12.0, MUTED, Weight::Regular);
            y += 40.0;
        }
    }
    ui.c.text(rect(x, y, w, 20.0), "Disco", 13.0, TEXT2, Align::Left, Weight::Semibold);
    y += 24.0;
    if ui.button("st-new", rect(x, y, w, 34.0), "Criar partição…", false) {
        open = Some(Kind::Create);
    }
    if let Some(kind) = open {
        s.sheet = Some(new_sheet(kind, &vol, &d));
    }
}

/// O que o código de saída quer dizer. CHKDSK: 0 sem erro, 1 corrigiu, 2 limpeza, 3 erro sem correção.
fn outcome(job: &Job, code: i32, win: char) -> (&'static str, Color) {
    match (job, code) {
        (_, -1) => ("O Windows não conseguiu. O motivo está abaixo.", REC),
        (Job::Repair { letter, .. }, _) if *letter == win => ("No volume do Windows, o reparo roda na próxima reinicialização. Veja abaixo o que o CHKDSK disse.", WARN),
        (Job::Scan(_) | Job::Repair { .. }, 0) => ("Nenhum problema encontrado.", ACCENT),
        (Job::Scan(_) | Job::Repair { .. }, 1) => ("Erros encontrados e corrigidos.", ACCENT),
        (Job::Scan(_) | Job::Repair { .. }, 2) => ("Limpeza feita, sem erros.", ACCENT),
        (Job::Scan(_), _) => ("O CHKDSK achou problemas ou não conseguiu ler o volume. Veja abaixo; Reparar corrige.", WARN),
        (Job::Repair { .. }, _) => ("O reparo não terminou agora. Veja abaixo o que o CHKDSK disse.", WARN),
        _ => ("Pronto.", ACCENT),
    }
}

fn draw_run(ui: &mut Ui, app: &mut App, s: &mut St, r: Rect) {
    let (win, _) = system_letters(&app.disks);
    let Some(run) = s.run.as_ref() else { return };
    let x = r.x + 16.0;
    let w = r.w - 32.0;
    let mut y = r.y + 54.0;
    ui.c.text_fit(rect(x, y, w, 22.0), &run.title, 15.0, TEXT, Align::Left, Weight::Semibold);
    y += 28.0;
    let (status, color) = match run.p.done {
        None => {
            let t = run.at.elapsed().as_secs();
            (format!("Rodando há {}. Num HD grande pode levar horas; não desligue o PC no meio.", format!("{}:{:02}", t / 60, t % 60)), TEXT2)
        }
        Some(code) => {
            let (t, c) = outcome(&run.job, code, win);
            (t.to_string(), c)
        }
    };
    ui.c.text_wrap(rect(x, y, w, 54.0), &status, 13.0, color, Weight::Regular);
    y += 58.0;
    if let Some((lo, hi)) = run.p.range {
        ui.c.text_wrap(rect(x, y, w, 36.0), &format!("O Windows aceita de {} a {}.", fmt_size(lo), fmt_size(hi)), 13.0, TEXT2, Weight::Regular);
        y += 40.0;
    }
    // As últimas linhas do log que cabem, de baixo para cima.
    let area = rect(x, y, w, r.bottom() - 60.0 - y);
    let mut shown = String::new();
    for l in run.p.lines.iter().rev() {
        let t = if shown.is_empty() { l.clone() } else { format!("{l}\n{shown}") };
        if crate::ui::gfx::measure(&t, 12.0, Weight::Regular, Some(area.w)).1 > area.h {
            break;
        }
        shown = t;
    }
    ui.c.text_wrap(area, &shown, 12.0, MUTED, Weight::Regular);
    if ui.button("st-close", rect(x, r.bottom() - 52.0, w, 40.0), "Fechar", false) {
        if let Some(run) = s.run.take() {
            // Rodando, o PowerShell ainda grava no log: ele fica para a limpeza da pasta temporária.
            if run.p.done.is_some() {
                let _ = std::fs::remove_file(run.log);
            }
        }
    }
}

fn draw_sheet(ui: &mut Ui, app: &mut App, s: &mut St) {
    let (win, sys) = system_letters(&app.disks);
    let Some(sh) = s.sheet.as_mut() else { return };
    let h = match sh.kind {
        Kind::Repair => 420.0,
        Kind::Format => 540.0,
        Kind::Create => 440.0,
        Kind::Delete => 380.0,
        Kind::Letter | Kind::Resize => 360.0,
    };
    let r = ui.sheet(560.0, h);
    let x = r.x + 24.0;
    let w = r.w - 48.0;
    let mut y = r.y + 20.0;
    ui.c.text_fit(rect(x, y, w, 26.0), &sh.title(), 17.0, TEXT, Align::Left, Weight::Semibold);
    y += 40.0;

    let l = sh.vol.letter;
    let used = sh.vol.total.saturating_sub(sh.vol.free);
    let destroy = matches!(sh.kind, Kind::Format | Kind::Delete);
    let mut paras: Vec<(String, Color)> = match sh.kind {
        Kind::Repair => vec![
            ("Riscos do reparo:".into(), WARN),
            ("• Arquivos em partes corrompidas do disco podem ser apagados ou ir para a pasta FOUND.000.".into(), TEXT2),
            ("• O volume é desmontado: programas com arquivos abertos nele perdem o que não foi salvo.".into(), TEXT2),
            ("• No volume do Windows, o reparo só roda na próxima reinicialização.".into(), TEXT2),
            ("• Num HD grande pode levar horas. Não desligue o PC no meio.".into(), TEXT2),
        ],
        Kind::Format | Kind::Delete => vec![
            (format!("Tudo em {} será apagado: {} usados. Isso não tem volta.", vol_name(&sh.vol), fmt_size(used)), WARN),
            ("Programas com arquivos abertos nesse volume perdem o que não foi salvo.".into(), TEXT2),
        ],
        Kind::Letter => vec![(format!("Programas instalados, atalhos e bibliotecas de jogos que usam {l}: param de achar os arquivos até a letra voltar."), WARN)],
        Kind::Resize => vec![(
            "Diminuir libera espaço para outra partição; aumentar usa o espaço livre logo depois dela. O Windows não diminui além do que está em uso. Faça uma cópia do que importa antes.".into(),
            TEXT2,
        )],
        Kind::Create => vec![("Usa o espaço livre do disco, sem mexer nas partições que já existem. Disco novo, sem partições, é preparado como GPT.".into(), TEXT2)],
    };
    if sh.kind != Kind::Create && sh.kind != Kind::Repair && prisma_on(l) {
        paras.push(("O Prisma está nesse volume: ele e as configurações dele podem parar de funcionar.".into(), REC));
    }
    for (t, c) in &paras {
        let ph = crate::ui::gfx::measure(t, 13.0, Weight::Regular, Some(w)).1.ceil();
        ui.c.text_wrap(rect(x, y, w, ph + 2.0), t, 13.0, *c, Weight::Regular);
        y += ph + 8.0;
    }
    y += 8.0;

    let lw = 180.0;
    let fw = w - lw;
    let fs_names: Vec<String> = FILE_SYSTEMS.iter().map(|f| f.to_string()).collect();
    match sh.kind {
        Kind::Repair => {
            ui.check("st-sec", rect(x, y, w, 30.0), "Procurar setores ruins (/r): bem mais lento", &mut sh.sectors);
            y += 40.0;
        }
        Kind::Format | Kind::Create => {
            if sh.kind == Kind::Create {
                ui.label(rect(x, y, lw, 34.0), "Tamanho (GB)", TEXT2);
                ui.search("st-gb", rect(x + lw, y, fw, 34.0), &mut sh.size, "Todo o espaço livre");
                y += 44.0;
            }
            ui.label(rect(x, y, lw, 34.0), "Sistema de arquivos", TEXT2);
            ui.select("st-fs", rect(x + lw, y, fw, 34.0), &fs_names, &mut sh.fs);
            y += 44.0;
            ui.label(rect(x, y, lw, 34.0), "Rótulo", TEXT2);
            ui.search("st-lbl", rect(x + lw, y, fw, 34.0), &mut sh.label, "Sem rótulo");
            y += 44.0;
            if sh.kind == Kind::Format {
                ui.check("st-quick", rect(x, y, w, 30.0), "Formatação rápida (sem ela, o Windows lê o disco todo e demora)", &mut sh.quick);
                y += 40.0;
            }
        }
        Kind::Letter => {
            let mut names: Vec<String> = sh.letters.iter().map(|c| format!("{c}:")).collect();
            names.push("Sem letra".into());
            ui.label(rect(x, y, lw, 34.0), "Nova letra", TEXT2);
            ui.select("st-to", rect(x + lw, y, fw, 34.0), &names, &mut sh.to);
            y += 44.0;
        }
        Kind::Resize => {
            ui.label(rect(x, y, lw, 34.0), "Novo tamanho (GB)", TEXT2);
            ui.search("st-gb", rect(x + lw, y, fw, 34.0), &mut sh.size, "Em GB");
            y += 44.0;
        }
        Kind::Delete => {}
    }
    if destroy {
        ui.label(rect(x, y, lw, 34.0), "Para confirmar, digite", TEXT2);
        ui.search("st-typed", rect(x + lw, y, fw, 34.0), &mut sh.typed, &format!("A letra {l}"));
        y += 44.0;
    }

    let job = sh.job();
    let err = match &job {
        None => Some("Digite o tamanho em GB, só com números."),
        Some(j) => disks::refuse(j, win, &sys).err(),
    };
    if let Some(e) = err {
        ui.c.text_wrap(rect(x, y, w, 36.0), e, 13.0, REC, Weight::Regular);
    }
    let typed = !destroy || sh.typed.trim().eq_ignore_ascii_case(&l.to_string());
    let action = match sh.kind {
        Kind::Repair => "Reparar",
        Kind::Format => "Formatar",
        Kind::Letter => "Trocar",
        Kind::Resize => "Redimensionar",
        Kind::Delete => "Apagar",
        Kind::Create => "Criar",
    };
    let by = r.bottom() - 56.0;
    let go = ui.button_ex("st-go", rect(r.right() - 24.0 - 160.0, by, 160.0, 36.0), action, true, err.is_none() && typed);
    let cancel = ui.button("st-cancel", rect(r.right() - 24.0 - 160.0 - 12.0 - 120.0, by, 120.0, 36.0), "Cancelar", false);
    let title = sh.title();
    if cancel {
        s.sheet = None;
    } else if go {
        if let Some(job) = job {
            start(app, s, job, title);
        }
    }
}

fn draw_nets(ui: &mut Ui, app: &mut App, a: Rect) {
    let x = a.x + 28.0;
    let top = 88.0;
    let list = rect(x, top, a.w - 56.0, a.h - top - 20.0);
    let row_h = 104.0;
    ui.begin_scroll("nets", list, app.nets.len() as f32 * (row_h + 10.0));
    for (i, n) in app.nets.iter().enumerate() {
        let r = rect(list.x, list.y + i as f32 * (row_h + 10.0), list.w, row_h);
        if n.up {
            ui.card_on(r);
        } else {
            ui.card(r);
        }
        let tx = r.x + 16.0;
        let tw = r.w * 0.42;
        ui.c.text(rect(tx, r.y + 10.0, tw, 22.0), &n.name, 15.0, TEXT, Align::Left, Weight::Semibold);
        ui.c.text(rect(tx, r.y + 33.0, tw, 18.0), &n.desc, 13.0, MUTED, Align::Left, Weight::Regular);
        let state = if n.up {
            let speed = if n.speed >= 1_000_000_000 {
                format!("{} Gbps", n.speed / 1_000_000_000)
            } else {
                format!("{} Mbps", n.speed / 1_000_000)
            };
            if n.ip.is_empty() { format!("Conectado · {speed}") } else { format!("Conectado · {speed} · {}", n.ip) }
        } else {
            "Desconectado".to_string()
        };
        let color = if n.up { ACCENT } else { MUTED };
        ui.c.text(rect(tx, r.y + 56.0, tw, 18.0), &state, 13.0, color, Align::Left, Weight::Regular);
        let sx = r.x + tw + 32.0;
        let sw = 190.0;
        ui.c.text(rect(sx, r.y + 12.0, sw, 20.0), &format!("Baixando {}", fmt_rate(n.down_rate)), 13.0, TEXT, Align::Left, Weight::Regular);
        ui.c.text(rect(sx, r.y + 34.0, sw, 20.0), &format!("Enviando {}", fmt_rate(n.up_rate)), 13.0, TEXT, Align::Left, Weight::Regular);
        let t = format!("Total: {} recebidos, {} enviados", fmt_mem(n.total_down), fmt_mem(n.total_up));
        ui.c.text(rect(sx, r.y + 58.0, r.right() - sx - 16.0, 18.0), &t, 12.0, MUTED, Align::Left, Weight::Regular);
        let g = rect(sx + sw + 8.0, r.y + 12.0, r.right() - sx - sw - 24.0, 40.0);
        ui.c.sparkline(g, &n.hist, ACCENT, 1.5);
    }
    ui.end_scroll();
}
