//! Pesquisar: acha arquivos e pastas pelo nome em todos os discos, enquanto você digita. A mesma
//! pesquisa abre numa janelinha pelo atalho, por cima de qualquer programa.

use std::os::windows::process::CommandExt;
use std::time::Instant;

use super::*;
use crate::sys::index::{self, Item, Searcher, Sort};

pub const SEARCH: &str = "find-q";
pub const POP_SEARCH: &str = "pop-q";
const ROW: f32 = 34.0;
const MENU: &str = "find-menu";

/// Janelinha do atalho: largura, altura da caixa, de cada linha e do rodapé.
pub const POP_W: f32 = 720.0;
pub const POP_TOP: f32 = 64.0;
pub const POP_ROW: f32 = 50.0;
pub const POP_FOOT: f32 = 36.0;
pub const POP_ROWS: usize = 8;

/// Uma pesquisa e a lista dela: a da página e a da janelinha são separadas.
#[derive(Default)]
pub struct Find {
    pub q: String,
    pub sort: Sort,
    pub desc: bool,
    pub sel: usize,
    sel_key: Option<u64>,
    res: Searcher,
    sent: Option<(String, Sort, bool)>,
    sent_ver: u64,
    pub hits: Vec<u64>,
    hits_query: String,
    pub total: usize,
    ms: u32,
    /// A seleção mudou pelo teclado: a lista rola até ela.
    follow: bool,
    /// Pesquisa nova: a lista volta ao topo.
    reset: bool,
    click: Option<(u64, Instant)>,
    menu_key: Option<u64>,
}

impl Find {
    /// Pega o resultado que chegou. Na mesma pesquisa, a seleção continua no mesmo arquivo.
    pub fn take(&mut self) -> bool {
        let Some(r) = self.res.out.lock().unwrap().take() else { return false };
        let same = r.query == self.hits_query;
        self.hits = r.hits;
        self.total = r.total;
        self.ms = r.ms;
        self.hits_query = r.query;
        if same {
            if let Some(p) = self.sel_key.and_then(|k| self.hits.iter().position(|&h| h == k)) {
                self.sel = p;
            }
        } else {
            self.sel = 0;
            self.reset = true;
        }
        self.sel = self.sel.min(self.hits.len().saturating_sub(1));
        self.sel_key = self.hits.get(self.sel).copied();
        true
    }

    pub fn send(&mut self, notify: isize, tag: usize) {
        let want = (self.q.clone(), self.sort, self.desc);
        if self.sent.as_ref() == Some(&want) {
            return;
        }
        self.sent = Some(want);
        if self.q.trim().is_empty() {
            self.res.clear();
            self.hits.clear();
            self.hits_query.clear();
            self.total = 0;
            self.sel = 0;
            self.sel_key = None;
            return;
        }
        self.sent_ver = index::version();
        self.res.run(&self.q, self.sort, self.desc, notify, crate::app::WM_FIND_DONE, tag);
    }

    /// O índice mudou desde a última pesquisa: roda de novo, sem perder a seleção.
    pub fn stale(&mut self, notify: isize, tag: usize) {
        if !self.q.trim().is_empty() && self.sent.is_some() && index::version() != self.sent_ver {
            self.sent = None;
            self.send(notify, tag);
        }
    }

    pub fn clear(&mut self) {
        self.q.clear();
        self.sent = None;
        self.res.clear();
        self.hits.clear();
        self.hits_query.clear();
        self.total = 0;
        self.sel = 0;
        self.sel_key = None;
    }

    fn select(&mut self, i: isize) {
        if self.hits.is_empty() {
            return;
        }
        self.sel = i.clamp(0, self.hits.len() as isize - 1) as usize;
        self.sel_key = Some(self.hits[self.sel]);
        self.follow = true;
    }

    fn selected(&self) -> Option<Item> {
        let k = *self.hits.get(self.sel)?;
        index::items(&[k]).pop().flatten()
    }
}

/// Abre como o Explorador abriria: com o programa padrão e sem herdar o administrador.
pub fn open(path: &str) {
    let _ = std::process::Command::new(crate::sys::system_tool("explorer")).raw_arg(format!("\"{path}\"")).spawn();
}

fn copy(app: &App, path: &str) {
    crate::capture::copy_text(app.hub, path);
}

fn act(app: &mut App, it: &Item, what: u8) {
    let p = it.path();
    match what {
        0 => open(&p),
        1 => crate::sys::procs::reveal(&p),
        _ => {
            copy(app, &p);
            app.toast.info("Caminho copiado");
        }
    }
}

fn group(n: u64) -> String {
    let s = n.to_string();
    let sep = if crate::i18n::is_english() { ',' } else { '.' };
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(sep);
        }
        out.push(c);
    }
    out
}

fn fmt_size(b: u64) -> String {
    let one = |v: f64, u: &str| format!("{v:.1} {u}").replace('.', crate::i18n::comma());
    if b < 1 << 20 {
        format!("{} KB", b.div_ceil(1024))
    } else if b < 1 << 30 {
        one(b as f64 / (1u64 << 20) as f64, "MB")
    } else {
        one(b as f64 / (1u64 << 30) as f64, "GB")
    }
}

/// Deslocamento do horário local agora, em segundos.
fn local_offset() -> i64 {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::Storage::FileSystem::FileTimeToLocalFileTime;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let ft = (now + 11_644_473_600) * 10_000_000;
    let utc = FILETIME { dwLowDateTime: ft as u32, dwHighDateTime: (ft >> 32) as u32 };
    let mut loc = FILETIME::default();
    if unsafe { FileTimeToLocalFileTime(&utc, &mut loc) }.is_err() {
        return 0;
    }
    let l = ((loc.dwHighDateTime as u64) << 32) | loc.dwLowDateTime as u64;
    (l as i64 - ft as i64) / 10_000_000
}

fn fmt_date(t: u32, off: i64) -> String {
    if t == 0 {
        return String::new();
    }
    let s = t as i64 + off;
    let (days, rem) = (s.div_euclid(86_400), s.rem_euclid(86_400));
    // Dias desde 1970 em ano, mês e dia (calendário gregoriano).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    let (h, mi) = (rem / 3600, rem % 3600 / 60);
    if crate::i18n::is_english() {
        format!("{y}-{m:02}-{d:02} {h:02}:{mi:02}")
    } else {
        format!("{d:02}/{m:02}/{y} {h:02}:{mi:02}")
    }
}

/// Andamento do índice numa linha.
fn status_line(app: &App) -> (String, bool) {
    let st = index::status();
    if let Some(d) = st.iter().find(|d| d.state == index::State::Reading) {
        let t = if d.total > 0 {
            format!("Lendo o disco {}: {}%", d.letter, (d.done * 100 / d.total.max(1)).min(100))
        } else {
            format!("Lendo o disco {}: {} itens", d.letter, group(d.done))
        };
        return (t, false);
    }
    let ready: Vec<&index::Drive> = st.iter().filter(|d| d.state == index::State::Ready).collect();
    if ready.is_empty() {
        let t = if st.is_empty() && app.cfg.search_drives.is_empty() { "Conferindo a saúde dos discos…" } else { "Preparando o índice…" };
        return (t.to_string(), false);
    }
    let files: u64 = ready.iter().map(|d| d.files as u64).sum();
    let letters: Vec<String> = ready.iter().map(|d| format!("{}:", d.letter)).collect();
    let live = if ready.iter().all(|d| d.live) { "atualiza na hora" } else { "atualiza a cada 15 minutos" };
    (format!("{} itens em {} · {} de memória · {live}", group(files), letters.join(" "), fmt_mem(index::memory() as u64)), true)
}

fn intro(ui: &mut Ui, app: &mut App, x: f32, w: f32) {
    let body = "O Prisma guarda os nomes de todos os arquivos dos seus discos num índice, e a pesquisa responde enquanto você digita. O índice usa memória: cerca de 70 MB a cada milhão de arquivos.";
    let admin = if app.elevated {
        "Com o Prisma como administrador, o índice fica pronto em segundos e se atualiza na hora em que um arquivo muda."
    } else {
        "Sem administrador, o Prisma percorre as pastas, o que demora mais, e atualiza o índice a cada 15 minutos."
    };
    let disks = "Discos com problema de saúde ficam de fora, para não forçar o disco. Dá para incluir em Opções.";
    let key = format!("Atalho: {} abre a pesquisa por cima de qualquer programa.", app.cfg.hk_find.label());
    let tw = w - 48.0;
    let hs: Vec<f32> = [body, admin, disks, key.as_str()].iter().map(|t| measure(t, 14.0, Weight::Regular, Some(tw)).1).collect();
    let h = 64.0 + hs.iter().sum::<f32>() + 12.0 * 3.0 + 72.0;
    let r = rect(x, 96.0, w, h);
    ui.card(r);
    ui.title(rect(r.x + 24.0, r.y + 20.0, tw, 26.0), "Ligar a pesquisa");
    let mut y = r.y + 58.0;
    for (t, th) in [body, admin, disks, key.as_str()].iter().zip(&hs) {
        ui.c.text_wrap(rect(r.x + 24.0, y, tw, th + 4.0), t, 14.0, TEXT2, Weight::Regular);
        y += th + 12.0;
    }
    if ui.button_ex("find-on", rect(r.x + 24.0, r.bottom() - 60.0, 120.0, 40.0), "Ligar", true, true) {
        app.set_search(true);
    }
}

fn options(ui: &mut Ui, app: &mut App, r: Rect) {
    ui.card(r);
    if app.find_drives.is_empty() {
        app.find_drives = crate::sys::mft::drives();
    }
    let x = r.x + 20.0;
    ui.c.text(rect(x, r.y + 14.0, 200.0, 20.0), "Discos", 13.0, MUTED, Align::Left, Weight::Regular);
    let suspect = app.suspect_letters();
    let chosen = app.search_letters();
    let mut cx = x;
    let mut toggle = None;
    for &l in &app.find_drives.clone() {
        let on = chosen.contains(&l);
        let label = format!("{l}:");
        let c = rect(cx, r.y + 38.0, 56.0, 34.0);
        if ui.button_ex(&format!("find-d{l}"), c, &label, on, true) {
            toggle = Some(l);
        }
        if suspect.contains(&l) {
            ui.tip(c, "Disco com problema de saúde: ler ele inteiro força o disco");
        }
        cx += 64.0;
        if cx + 56.0 > r.right() - 20.0 {
            break;
        }
    }
    if let Some(l) = toggle {
        let mut set = chosen;
        if let Some(i) = set.iter().position(|&c| c == l) {
            set.remove(i);
        } else {
            set.push(l);
            set.sort();
        }
        app.cfg.search_drives = set.into_iter().collect();
        // Nenhum disco escolhido ainda guarda a escolha: sem ela, voltaria a ser todos.
        if app.cfg.search_drives.is_empty() {
            app.cfg.search_drives = "-".into();
        }
        app.cfg.save();
    }
    let ky = r.y + 86.0;
    ui.c.text(rect(x, ky, 200.0, 20.0), "Atalho da janelinha", 13.0, MUTED, Align::Left, Weight::Regular);
    if ui.hotkey("hk-find", rect(x, ky + 24.0, 230.0, 40.0), &mut app.cfg.hk_find) {
        app.cfg.save();
        app.register_hotkeys();
    }
    let bx = r.right() - 20.0;
    if ui.button("find-off", rect(bx - 120.0, ky + 24.0, 120.0, 40.0), "Desligar", false) {
        ui.ask("find-off-ask", "Desligar a pesquisa?", "O índice sai da memória e do disco. Ao ligar de novo, os discos são lidos outra vez.");
    }
    if ui.answered("find-off-ask") {
        app.set_search(false);
    }
    if ui.button("find-again", rect(bx - 120.0 - 12.0 - 170.0, ky + 24.0, 170.0, 40.0), "Ler os discos de novo", false) {
        index::refresh();
    }
}

/// Teclas da lista. Devolve true se alguma mudou algo.
fn keys(ui: &mut Ui, app: &mut App, pop: bool, page: usize) -> bool {
    let nav = std::mem::take(&mut ui.s.nav);
    let mut any = false;
    for (vk, ctrl) in nav {
        let st = if pop { &mut app.pop } else { &mut app.find };
        let s = st.sel as isize;
        match vk {
            0x26 => st.select(s - 1),
            0x28 => st.select(s + 1),
            0x21 => st.select(s - page as isize),
            0x22 => st.select(s + page as isize),
            0x0D => {
                if let Some(it) = st.selected() {
                    act(app, &it, if ctrl { 1 } else { 0 });
                    if pop {
                        app.close_find();
                    }
                }
            }
            0x43 if ctrl => {
                if let Some(it) = st.selected() {
                    act(app, &it, 2);
                }
            }
            0x1B if !pop => st.clear(),
            _ => continue,
        }
        any = true;
    }
    any
}

fn menu(ui: &mut Ui, app: &mut App, pop: bool) {
    if let Some(i) = ui.menu_pick(MENU) {
        let st = if pop { &mut app.pop } else { &mut app.find };
        if let Some(it) = st.menu_key.take().and_then(|k| index::items(&[k]).pop().flatten()) {
            act(app, &it, i as u8);
            if pop && i < 2 {
                app.close_find();
            }
        }
    }
}

fn open_menu(ui: &mut Ui, st: &mut Find, i: usize, k: u64) {
    st.sel = i;
    st.sel_key = Some(k);
    st.menu_key = Some(k);
    ui.menu(MENU, &[("Abrir", true), ("Mostrar na pasta", true), ("Copiar caminho", true)]);
}

fn empty_hint(ui: &Ui, r: Rect, app: &App, st: &Find) {
    let t = if st.q.trim().is_empty() {
        "Digite parte do nome. Dicas: ext:pdf só PDFs · pasta: só pastas · *.mp4 com curinga · !palavra tira · docs\\nota procura no caminho"
    } else if index::status().iter().all(|d| d.state != index::State::Ready) {
        "O índice ainda está sendo preparado."
    } else if st.hits_query != st.q {
        ""
    } else {
        "Nada encontrado."
    };
    let _ = app;
    ui.c.text_wrap(r, t, 14.0, MUTED, Weight::Regular);
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header_room(ui, a, "Pesquisar arquivos", "Acha arquivos e pastas pelo nome em todos os discos, enquanto você digita.", 160.0);
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    if !app.cfg.search_on {
        intro(ui, app, x, w);
        return;
    }
    if ui.button_ex("find-opts", rect(a.right() - 28.0 - 120.0, 30.0, 120.0, 36.0), "Opções", app.find_opts, true) {
        app.find_opts = !app.find_opts;
    }
    app.find.take();
    let top = 96.0;
    let mut y = top + 40.0 + 10.0;
    let opts_h = 168.0;
    let list_y = y + 26.0 + if app.find_opts { opts_h + 12.0 } else { 0.0 } + 32.0;
    let list = rect(x, list_y, w, a.bottom() - 16.0 - list_y);
    let page = ((list.h / ROW) as usize).max(1);
    if keys(ui, app, false, page) {
        ui.s.needs_redraw = true;
    }
    let mut q = std::mem::take(&mut app.find.q);
    ui.search(SEARCH, rect(x, top, w, 40.0), &mut q, "Nome do arquivo ou da pasta");
    app.find.q = q;
    if ui.s.capturing == 0 && !ui.s.covered() {
        crate::ui::focus_on(ui.s, SEARCH);
    }
    app.find.send(app.hub.0 as isize, 0);

    let (status, _) = status_line(app);
    let count = if app.find.q.trim().is_empty() || app.find.hits_query != app.find.q {
        String::new()
    } else {
        let shown = if app.find.total > app.find.hits.len() { format!(", mostrando {}", group(app.find.hits.len() as u64)) } else { String::new() };
        format!("{} encontrados{shown} · {} ms", group(app.find.total as u64), app.find.ms)
    };
    let cw = if count.is_empty() { 0.0 } else { measure(&count, 13.0, Weight::Regular, None).0 + 8.0 };
    ui.c.text(rect(x + 2.0, y, w - cw - 12.0, 20.0), &status, 13.0, MUTED, Align::Left, Weight::Regular);
    ui.c.text(rect(x + w - cw, y, cw, 20.0), &count, 13.0, MUTED, Align::Right, Weight::Regular);
    y += 26.0;
    if app.find_opts {
        options(ui, app, rect(x, y, w, opts_h));
        y += opts_h + 12.0;
    }

    // Cabeçalho das colunas: clicar ordena, de novo inverte, a terceira volta à relevância.
    let size_w = 92.0;
    let date_w = 136.0;
    let name_w = ((w - size_w - date_w - 40.0) * 0.42).max(140.0);
    let cols = [
        (Sort::Name, "Nome", rect(x + 10.0, y, name_w, 28.0), false),
        (Sort::Folder, "Pasta", rect(x + 18.0 + name_w, y, w - name_w - size_w - date_w - 40.0, 28.0), false),
        (Sort::Size, "Tamanho", rect(x + w - date_w - size_w - 18.0, y, size_w, 28.0), true),
        (Sort::Date, "Modificado", rect(x + w - date_w - 8.0, y, date_w, 28.0), true),
    ];
    for (sort, label, r, right) in cols {
        let on = app.find.sort == sort;
        let text = if on { format!("{label} {}", if app.find.desc { "↓" } else { "↑" }) } else { label.to_string() };
        let hover = ui.is_hover(r);
        ui.c.text(r, &text, 13.0, if on || hover { TEXT } else { MUTED }, if right { Align::Right } else { Align::Left }, Weight::Semibold);
        if ui.hit(&format!("find-col-{label}"), r) {
            let first_desc = matches!(sort, Sort::Size | Sort::Date);
            if !on {
                app.find.sort = sort;
                app.find.desc = first_desc;
            } else if app.find.desc == first_desc {
                app.find.desc = !first_desc;
            } else {
                app.find.sort = Sort::Best;
                app.find.desc = false;
            }
            ui.s.needs_redraw = true;
        }
    }
    ui.c.fill(rect(x, list.y - 3.0, w, 1.0), LINE);

    let n = app.find.hits.len();
    if n == 0 {
        empty_hint(ui, rect(x + 10.0, list.y + 12.0, w - 20.0, 60.0), app, &app.find);
        menu(ui, app, false);
        return;
    }
    if std::mem::take(&mut app.find.reset) {
        ui.set_scroll("find-list", 0.0);
    }
    if std::mem::take(&mut app.find.follow) {
        let sy = ui.scroll_y_of("find-list");
        let ry = app.find.sel as f32 * ROW;
        if ry < sy {
            ui.set_scroll("find-list", ry);
        } else if ry + ROW > sy + list.h {
            ui.set_scroll("find-list", ry + ROW - list.h);
        }
    }
    ui.begin_scroll("find-list", list, n as f32 * ROW);
    let first = (ui.scroll_y() / ROW) as usize;
    let last = (first + page + 2).min(n);
    let keys_shown: Vec<u64> = app.find.hits[first..last].to_vec();
    let items = index::items(&keys_shown);
    let off = local_offset();
    let dbl = std::time::Duration::from_millis(unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() } as u64);
    for (j, it) in items.iter().enumerate() {
        let i = first + j;
        let k = keys_shown[j];
        let r = rect(list.x, list.y + i as f32 * ROW, list.w, ROW - 2.0);
        let sel = i == app.find.sel;
        if sel {
            ui.c.round(r, 6.0, CARD2);
        } else if ui.is_hover(r) {
            ui.c.round(r, 6.0, CARD);
        }
        let Some(it) = it else {
            ui.c.text(rect(r.x + 38.0, r.y, r.w - 48.0, r.h), "Apagado ou movido", 13.0, MUTED, Align::Left, Weight::Regular);
            continue;
        };
        let path = it.path();
        ui.kind_icon(&path, &it.name, it.dir, rect(r.x + 10.0, r.y + (r.h - 20.0) / 2.0, 20.0, 20.0));
        let [c0, c1, c2, c3] = cols.map(|c| c.2);
        ui.c.text_raw(rect(c0.x + 28.0, r.y, c0.w - 28.0, r.h), &it.name, 14.0, TEXT, Align::Left, Weight::Regular);
        ui.c.text_raw(rect(c1.x, r.y, c1.w, r.h), &it.folder, 13.0, TEXT2, Align::Left, Weight::Regular);
        if !it.dir {
            ui.c.text_raw(rect(c2.x, r.y, c2.w, r.h), &fmt_size(it.size), 13.0, MUTED, Align::Right, Weight::Regular);
        }
        ui.c.text_raw(rect(c3.x, r.y, c3.w, r.h), &fmt_date(it.mtime, off), 13.0, MUTED, Align::Right, Weight::Regular);
        if ui.hit(&format!("find-r{i}"), r) {
            let again = app.find.click.is_some_and(|(ck, t)| ck == k && t.elapsed() <= dbl);
            if again {
                app.find.click = None;
                open(&path);
            } else {
                app.find.sel = i;
                app.find.sel_key = Some(k);
                app.find.click = Some((k, Instant::now()));
            }
            ui.s.needs_redraw = true;
        }
        if ui.right_hit(r) {
            open_menu(ui, &mut app.find, i, k);
        }
    }
    ui.end_scroll();
    menu(ui, app, false);
}

/// Altura da janelinha para o que ela mostra agora.
pub fn pop_height(app: &App) -> f32 {
    let rows = if app.pop.q.trim().is_empty() {
        0.0
    } else if app.pop.hits.is_empty() {
        1.0
    } else {
        app.pop.hits.len().min(POP_ROWS) as f32
    };
    POP_TOP + rows * POP_ROW + POP_FOOT + if rows > 0.0 { 6.0 } else { 0.0 }
}

pub fn draw_popup(ui: &mut Ui, app: &mut App, t: Rect) {
    app.pop.take();
    if keys(ui, app, true, POP_ROWS) {
        ui.s.needs_redraw = true;
    }
    ui.c.fill(t, BG);
    let mut q = std::mem::take(&mut app.pop.q);
    ui.search(POP_SEARCH, rect(t.x + 14.0, t.y + 12.0, t.w - 28.0, 40.0), &mut q, "Pesquisar arquivos e pastas");
    app.pop.q = q;
    if !ui.s.covered() {
        crate::ui::focus_on(ui.s, POP_SEARCH);
    }
    app.pop.send(app.hub.0 as isize, 1);
    if ui.s.wheel != 0.0 {
        let s = app.pop.sel as isize - ui.s.wheel.signum() as isize;
        app.pop.select(s);
    }
    let foot = rect(t.x + 16.0, t.bottom() - POP_FOOT, t.w - 32.0, POP_FOOT - 6.0);
    let n = app.pop.hits.len();
    let busy = index::status().iter().find(|d| d.state != index::State::Ready).is_some() || index::status().is_empty();
    if !app.pop.q.trim().is_empty() && n == 0 {
        let msg = if busy && index::status().iter().all(|d| d.state != index::State::Ready) {
            status_line(app).0
        } else if app.pop.hits_query != app.pop.q {
            String::new()
        } else {
            "Nada encontrado.".to_string()
        };
        ui.c.text(rect(t.x + 24.0, t.y + POP_TOP, t.w - 48.0, POP_ROW), &msg, 14.0, MUTED, Align::Left, Weight::Regular);
    }
    // A janela mostra até oito linhas e rola junto com a seleção.
    let first = app.pop.sel.saturating_sub(POP_ROWS - 1).min(n.saturating_sub(POP_ROWS));
    let last = (first + POP_ROWS).min(n);
    let keys_shown: Vec<u64> = app.pop.hits.get(first..last).unwrap_or(&[]).to_vec();
    let items = index::items(&keys_shown);
    let off = local_offset();
    let mut pick = None;
    for (j, it) in items.iter().enumerate() {
        let i = first + j;
        let k = keys_shown[j];
        let r = rect(t.x + 8.0, t.y + POP_TOP + j as f32 * POP_ROW, t.w - 16.0, POP_ROW - 2.0);
        if i == app.pop.sel {
            ui.c.round(r, 8.0, CARD2);
            ui.c.round(rect(r.x, r.y + 12.0, 3.0, r.h - 24.0), 1.5, ACCENT);
        } else if ui.is_hover(r) {
            ui.c.round(r, 8.0, CARD);
        }
        let Some(it) = it else {
            ui.c.text(rect(r.x + 56.0, r.y, r.w - 72.0, r.h), "Apagado ou movido", 13.0, MUTED, Align::Left, Weight::Regular);
            continue;
        };
        let path = it.path();
        ui.kind_icon(&path, &it.name, it.dir, rect(r.x + 14.0, r.y + (r.h - 28.0) / 2.0, 28.0, 28.0));
        let side = 150.0;
        ui.c.text_raw(rect(r.x + 56.0, r.y + 5.0, r.w - 56.0 - side - 12.0, 22.0), &it.name, 15.0, TEXT, Align::Left, Weight::Semibold);
        ui.c.text_raw(rect(r.x + 56.0, r.y + 26.0, r.w - 56.0 - side - 12.0, 18.0), &it.folder, 12.0, MUTED, Align::Left, Weight::Regular);
        let info = if it.dir { "Pasta".to_string() } else { fmt_size(it.size) };
        ui.c.text(rect(r.right() - side - 12.0, r.y + 5.0, side, 22.0), &info, 12.0, TEXT2, Align::Right, Weight::Regular);
        ui.c.text_raw(rect(r.right() - side - 12.0, r.y + 26.0, side, 18.0), &fmt_date(it.mtime, off), 12.0, MUTED, Align::Right, Weight::Regular);
        if ui.hit(&format!("pop-r{i}"), r) {
            pick = Some(path.clone());
        }
        if ui.right_hit(r) {
            open_menu(ui, &mut app.pop, i, k);
        }
    }
    ui.c.fill(rect(t.x, foot.y - 4.0, t.w, 1.0), LINE);
    let hint = if n > 0 {
        "Enter abre · Ctrl+Enter mostra na pasta · Ctrl+C copia o caminho"
    } else {
        "ext:pdf só PDFs · pasta: só pastas · *.mp4 curinga · !palavra tira"
    };
    let count = if n > 0 { format!("{} / {}", group(app.pop.sel as u64 + 1), group(app.pop.total as u64)) } else if busy { status_line(app).0 } else { String::new() };
    let cw = if count.is_empty() { 0.0 } else { (measure(&count, 12.0, Weight::Regular, None).0 + 8.0).min(foot.w * 0.5) };
    ui.c.text(rect(foot.x, foot.y, foot.w - cw - 12.0, foot.h), hint, 12.0, MUTED, Align::Left, Weight::Regular);
    ui.c.text(rect(foot.right() - cw, foot.y, cw, foot.h), &count, 12.0, MUTED, Align::Right, Weight::Regular);
    menu(ui, app, true);
    if let Some(p) = pick {
        open(&p);
        app.close_find();
    }
}
