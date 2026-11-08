//! Espaço em disco: pastas, tipos de arquivo e mapa visual.

use std::sync::atomic::Ordering;

use super::*;
use crate::sys::mft::{self, Scan};

const PALETTE: [u32; 8] = [0x4a7fb5, 0x5c9e8f, 0xb58a4a, 0x8f6fb0, 0xb56a6a, 0x6f8fb0, 0x8fa35c, 0xb07a9e];

fn tint(s: &str) -> Color {
    let h = s.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    rgb(PALETTE[(h % 8) as usize])
}

fn fmt_size(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1} GB", b as f64 / (1u64 << 30) as f64).replace('.', crate::i18n::comma())
    } else if b >= 1 << 20 {
        format!("{} MB", b >> 20)
    } else {
        format!("{} KB", b >> 10)
    }
}

/// Mapa em retângulos: fatias alternadas pelo lado maior, com proporção boa.
fn squarify(items: &[(u32, f64)], r: Rect, out: &mut Vec<(u32, Rect)>) {
    let total: f64 = items.iter().map(|i| i.1).sum();
    if items.is_empty() || total <= 0.0 || r.w < 2.0 || r.h < 2.0 {
        return;
    }
    let area = r.w as f64 * r.h as f64;
    let scale = area / total;
    let side = r.w.min(r.h) as f64;
    let mut n = 0;
    let mut sum = 0.0;
    let mut best = f64::MAX;
    while n < items.len() {
        let v = items[n].1 * scale;
        let s = sum + v;
        let (hi, lo) = (items[0].1 * scale, v.max(1e-9));
        let worst = (side * side * hi / (s * s)).max(s * s / (side * side * lo));
        if worst > best && n > 0 {
            break;
        }
        best = worst;
        sum = s;
        n += 1;
    }
    let thick = (sum / side) as f32;
    let mut pos = 0.0f32;
    for (id, v) in &items[..n] {
        let len = (*v * scale / thick as f64) as f32;
        let c = if r.w >= r.h { rect(r.x, r.y + pos, thick, len) } else { rect(r.x + pos, r.y, len, thick) };
        out.push((*id, c));
        pos += len;
    }
    let rest = if r.w >= r.h {
        rect(r.x + thick, r.y, r.w - thick, r.h)
    } else {
        rect(r.x, r.y + thick, r.w, r.h - thick)
    };
    squarify(&items[n..], rest, out);
}

fn open_in_explorer(path: &str, select: bool) {
    let arg = if select { format!("/select,\"{path}\"") } else { format!("\"{path}\"") };
    let _ = std::process::Command::new(crate::sys::system_tool("explorer")).raw_arg(arg).spawn();
}

trait RawArg {
    fn raw_arg(&mut self, a: String) -> &mut Self;
}

impl RawArg for std::process::Command {
    fn raw_arg(&mut self, a: String) -> &mut Self {
        std::os::windows::process::CommandExt::raw_arg(self, a)
    }
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    let sub = match app.scan.lock().unwrap().as_ref() {
        Some(s) => {
            let how = if s.used_mft { "lendo a tabela do NTFS" } else { "percorrendo as pastas" };
            format!("{} arquivos em {:.1} s, {how}", s.files, s.elapsed.as_secs_f32()).replace('.', crate::i18n::comma())
        }
        None => "Mostra o que ocupa espaço em cada disco.".to_string(),
    };
    header(ui, a, "Espaço em disco", &sub);

    let drives = mft::drives();
    let opts: Vec<String> = drives.iter().map(|d| format!("Disco {d}:")).collect();
    let sel = rect(a.right() - 300.0, 28.0, 150.0, 40.0);
    let running = app.scan_progress.running.load(Ordering::Relaxed);
    let label = if running { "Analisando…" } else { "Analisar" };
    if ui.button_ex("scan", rect(a.right() - 142.0, 28.0, 114.0, 40.0), label, true, !running) {
        if let Some(d) = drives.get(app.scan_drive) {
            *app.scan.lock().unwrap() = None;
            app.scan_at = None;
            app.scan_sel = None;
            mft::start(*d, app.scan_progress.clone(), app.scan.clone());
        }
    }

    let x = a.x + 28.0;
    let top = 88.0;
    let h = a.h - top - 20.0;
    let guard = app.scan.clone();
    let lock = guard.lock().unwrap();
    match lock.as_ref() {
        None => {
            let r = rect(x, top, a.w - 56.0, h);
            ui.card(r);
            let p = &app.scan_progress;
            let (done, total) = (p.done.load(Ordering::Relaxed), p.total.load(Ordering::Relaxed));
            let msg = if running {
                if total > 0 { format!("Lendo {done} de {total} registros") } else { format!("{done} itens lidos") }
            } else {
                "Escolha um disco e clique em Analisar.".to_string()
            };
            ui.c.text(rect(r.x, r.y + r.h / 2.0 - 34.0, r.w, 24.0), &msg, 15.0, TEXT2, Align::Center, Weight::Regular);
            if running && total > 0 {
                let b = rect(r.x + r.w / 2.0 - 180.0, r.y + r.h / 2.0, 360.0, 6.0);
                ui.c.round(b, 3.0, SIDE);
                ui.c.round(rect(b.x, b.y, b.w * done as f32 / total as f32, 6.0), 3.0, ACCENT);
            }
        }
        Some(s) => draw_result(ui, app, s, rect(x, top, a.w - 56.0, h)),
    }
    drop(lock);
    if let Some(i) = app.scan_delete.take() {
        let mut g = guard.lock().unwrap();
        if let Some(s) = g.as_mut() {
            let path = s.path(i);
            if !mft::protected(&path) && crate::sys::uninstall::to_recycle(&path) {
                s.forget(i);
                drop(g);
                app.log(&format!("apagou para a Lixeira {path}"));
                app.toast.info("Enviado para a Lixeira");
                app.scan_sel = None;
            } else {
                drop(g);
                app.toast.info("Não consegui apagar");
            }
        }
    }

    let mut d = app.scan_drive;
    if ui.select("drv", sel, &opts, &mut d) {
        app.scan_drive = d;
    }
}

fn draw_result(ui: &mut Ui, app: &mut App, s: &Scan, area: Rect) {
    let cur = match app.scan_at {
        Some(c) if (c as usize) < s.nodes.len() && s.exists(c) => c,
        _ => s.root,
    };
    let lw = (area.w * 0.46).floor();
    let l = rect(area.x, area.y, lw, area.h);
    ui.card(l);
    let mut next = None;
    let path = s.path(cur);
    for (i, name) in ["Pastas", "Maiores arquivos"].iter().enumerate() {
        let w = if i == 0 { 84.0 } else { 150.0 };
        let x = if i == 0 { l.x + 12.0 } else { l.x + 102.0 };
        if ui.button(name, rect(x, l.y + 10.0, w, 36.0), name, app.scan_tab == i as u8) {
            app.scan_tab = i as u8;
            ui.reset_scroll("tree");
        }
    }
    let foot = 104.0;
    let list = rect(l.x + 8.0, l.y + 94.0, l.w - 16.0, l.h - 102.0 - foot);
    let mut sel = app.scan_sel.filter(|i| (*i as usize) < s.nodes.len() && s.exists(*i));

    if app.scan_tab == 0 {
        if ui.button_ex("up", rect(l.x + 12.0, l.y + 52.0, 44.0, 34.0), "←", false, cur != s.root) {
            next = Some(s.nodes[cur as usize].parent);
        }
        ui.c.text(rect(l.x + 64.0, l.y + 52.0, l.w - 76.0, 34.0), &path, 13.0, TEXT2, Align::Left, Weight::Regular);
    } else {
        let t = format!("Os {} maiores arquivos do disco", s.largest.len());
        ui.c.text(rect(l.x + 16.0, l.y + 52.0, l.w - 32.0, 34.0), &t, 13.0, TEXT2, Align::Left, Weight::Regular);
    }

    let total = s.nodes[cur as usize].size.max(1);
    let big = s.largest.first().map_or(1, |f| s.nodes[*f as usize].size).max(1);
    let kids: Vec<u32> = if app.scan_tab == 0 {
        s.children(cur).iter().copied().filter(|k| s.exists(*k)).collect()
    } else {
        s.largest.clone()
    };
    ui.begin_scroll("tree", list, kids.len() as f32 * 40.0);
    let first = (ui.scroll_y() / 40.0) as usize;
    let last = first + (list.h / 40.0) as usize + 2;
    for (row, &k) in kids.iter().enumerate().take(last).skip(first) {
        let n = &s.nodes[k as usize];
        let y = list.y + row as f32 * 40.0;
        let r = rect(list.x, y, list.w, 38.0);
        if sel == Some(k) {
            ui.c.round(r, 6.0, CARD2);
        } else if ui.is_hover(r) {
            ui.c.round(r, 6.0, CARD2.alpha(0.5));
        }
        let go = rect(r.right() - 34.0, y + 4.0, 28.0, 28.0);
        let mut entered = false;
        if n.is_dir() {
            ui.c.icon(go.x + 5.0, go.y + 5.0, 18.0, &[&[(9., 5.), (16., 12.), (9., 19.)]], if ui.is_hover(go) { ACCENT } else { MUTED });
            if ui.hit(&format!("g{k}"), go) {
                next = Some(k);
                entered = true;
            }
        }
        if !entered && ui.hit(&format!("n{k}"), rect(r.x, r.y, r.w - 40.0, r.h)) {
            sel = Some(k);
        }
        let name = s.name(k);
        let color = if n.is_dir() { ACCENT } else { TEXT };
        ui.c.text(rect(r.x + 10.0, y + 2.0, r.w - 150.0, 22.0), name, 14.0, color, Align::Left, Weight::Regular);
        ui.c.text(rect(r.right() - 138.0, y + 2.0, 96.0, 22.0), &fmt_size(n.size), 13.0, TEXT, Align::Right, Weight::Regular);
        let frac = n.size as f32 / if app.scan_tab == 0 { total as f32 } else { big as f32 };
        let bar = r.w - 60.0;
        ui.c.round(rect(r.x + 10.0, y + 28.0, bar, 3.0), 1.5, SIDE);
        ui.c.round(rect(r.x + 10.0, y + 28.0, bar * frac, 3.0), 1.5, ACCENT);
    }
    ui.end_scroll();
    app.scan_sel = sel;

    // Ações sobre o item escolhido; sem escolha, valem para a pasta aberta.
    let f = rect(l.x + 12.0, l.bottom() - foot, l.w - 24.0, foot - 10.0);
    let target = sel.unwrap_or(cur);
    let tp = s.path(target);
    ui.c.line(f.x, f.y, f.right(), f.y, LINE, 1.0);
    ui.c.text(rect(f.x, f.y + 6.0, f.w, 34.0), &tp, 12.0, TEXT2, Align::Left, Weight::Regular);
    let bw = (f.w - 8.0) / 2.0;
    if ui.button("exp", rect(f.x, f.y + 42.0, bw, 44.0), "Abrir no Explorador", false) {
        open_in_explorer(&tp, target != s.root);
    }
    let can = app.elevated && sel.is_some() && !mft::protected(&tp);
    let label = if app.elevated { "Apagar" } else { "Apagar pede admin" };
    if ui.button_ex("del", rect(f.x + bw + 8.0, f.y + 42.0, bw, 44.0), label, false, can) {
        ui.ask("scan-del", "Enviar para a Lixeira?", &tp);
    }
    if ui.answered("scan-del") {
        app.scan_delete = sel;
    }

    let rx = l.right() + 12.0;
    let rw = area.right() - rx;
    let mh = (area.h * 0.58).floor();
    let m = rect(rx, area.y, rw, mh);
    ui.card(m);
    let shown = s.children(cur);
    let items: Vec<(u32, f64)> = shown.iter().filter(|k| s.exists(**k)).take(200).map(|k| (*k, s.nodes[*k as usize].size as f64)).filter(|i| i.1 > 0.0).collect();
    let mut cells = Vec::with_capacity(items.len());
    squarify(&items, m.inset(10.0), &mut cells);
    for (id, c) in &cells {
        let name = s.name(*id);
        let color = tint(name);
        let hover = ui.is_hover(*c);
        let r = c.inset(1.0);
        ui.c.round(r, 3.0, if hover { color } else { color.alpha(0.82) });
        if r.w > 54.0 && r.h > 22.0 {
            ui.c.text(rect(r.x + 6.0, r.y + 3.0, r.w - 10.0, 18.0), name, 12.0, WHITE, Align::Left, Weight::Regular);
            if r.h > 42.0 {
                ui.c.text(rect(r.x + 6.0, r.y + 21.0, r.w - 10.0, 16.0), &fmt_size(s.nodes[*id as usize].size), 11.0, WHITE.alpha(0.85), Align::Left, Weight::Regular);
            }
        }
        if s.nodes[*id as usize].is_dir() && ui.hit(&format!("c{id}"), *c) {
            next = Some(*id);
        }
    }

    let t = rect(rx, m.bottom() + 12.0, rw, area.h - mh - 12.0);
    ui.card(t);
    ui.title(rect(t.x + 16.0, t.y + 10.0, 200.0, 22.0), "Tipos de arquivo");
    let list = rect(t.x + 8.0, t.y + 38.0, t.w - 16.0, t.h - 44.0);
    let vol = s.nodes[s.root as usize].size.max(1);
    ui.begin_scroll("types", list, s.types.len() as f32 * 30.0);
    for (i, ty) in s.types.iter().enumerate() {
        let y = list.y + i as f32 * 30.0;
        let name = if ty.ext.is_empty() { "Sem extensão".to_string() } else { format!(".{}", ty.ext) };
        ui.c.round(rect(list.x + 8.0, y + 9.0, 12.0, 12.0), 3.0, tint(&ty.ext));
        ui.c.text(rect(list.x + 28.0, y, 130.0, 30.0), &name, 13.0, TEXT, Align::Left, Weight::Regular);
        let info = format!("{} arquivos", ty.files);
        ui.c.text(rect(list.x + 150.0, y, list.w - 330.0, 30.0), &info, 12.0, MUTED, Align::Left, Weight::Regular);
        let pct = format!("{:.1}%", ty.size as f32 / vol as f32 * 100.0).replace('.', crate::i18n::comma());
        ui.c.text(rect(list.right() - 176.0, y, 74.0, 30.0), &fmt_size(ty.size), 13.0, TEXT, Align::Right, Weight::Regular);
        ui.c.text(rect(list.right() - 92.0, y, 84.0, 30.0), &pct, 12.0, MUTED, Align::Right, Weight::Regular);
    }
    ui.end_scroll();

    if let Some(n) = next {
        app.scan_at = Some(n);
        app.scan_sel = None;
        ui.reset_scroll("tree");
        ui.s.needs_redraw = true;
    }
}
