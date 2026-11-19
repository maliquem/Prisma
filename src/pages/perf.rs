//! Desempenho: GPU por engine, CPU por thread, composição da RAM e VRAM por programa. As leituras
//! extras (`sys::perf`) só rodam com a página aberta, ou sempre se a opção pedir.

use super::*;
use crate::i18n::tr;
use crate::sys::monitor::{Group, Snapshot};
use crate::sys::perf::ENGINES;
use std::collections::VecDeque;

const TABS: [&str; 4] = ["GPU", "CPU", "RAM", "VRAM"];
const ENGINE_NAMES: [&str; ENGINES] = ["3D", "Copy", "Encode", "Decode", "Compute"];
/// Cores das fatias por programa na barra da VRAM; o resto fica cinza.
const SLICES: [Color; 5] = [ACCENT, WARN, rgb(0x8fd694), rgb(0xc59bff), rgb(0xff8fb1)];
const ROW: f32 = 34.0;
const GAP: f32 = 12.0;

fn dec(v: f32, n: usize) -> String {
    format!("{v:.n$}").replace('.', crate::i18n::comma())
}

fn gb(b: u64) -> String {
    dec(b as f32 / (1u64 << 30) as f32, 1)
}

fn perf_info() -> windows::Win32::System::ProcessStatus::PERFORMANCE_INFORMATION {
    let mut pi = windows::Win32::System::ProcessStatus::PERFORMANCE_INFORMATION::default();
    let _ = unsafe { windows::Win32::System::ProcessStatus::GetPerformanceInfo(&mut pi, std::mem::size_of_val(&pi) as u32) };
    pi
}

/// Cartão de número: título, valor grande com a unidade e uma linha embaixo.
fn stat(ui: &mut Ui, r: Rect, title: &str, val: &str, unit: &str, sub: &str) {
    ui.card(r);
    ui.c.text(rect(r.x + 14.0, r.y + 10.0, r.w - 28.0, 18.0), title, 12.0, MUTED, Align::Left, Weight::Regular);
    let size = if unit.is_empty() && val.chars().count() > 6 { 16.0 } else { 24.0 };
    ui.c.text(rect(r.x + 14.0, r.y + 28.0, r.w - 28.0, 32.0), val, size, TEXT, Align::Left, Weight::Semibold);
    let vw = measure(val, size, Weight::Semibold, None).0;
    ui.c.text(rect(r.x + 18.0 + vw, r.y + 35.0, (r.w - 32.0 - vw).max(0.0), 24.0), unit, 13.0, MUTED, Align::Left, Weight::Regular);
    ui.c.text(rect(r.x + 14.0, r.y + 62.0, r.w - 28.0, 18.0), sub, 12.0, MUTED, Align::Left, Weight::Regular);
}

/// Uma fileira de cartões de número com a mesma largura.
fn stats(ui: &mut Ui, x: f32, y: f32, w: f32, items: &[(&str, String, &str, String)]) {
    let cw = (w - GAP * (items.len() as f32 - 1.0)) / items.len() as f32;
    for (i, (t, v, u, s)) in items.iter().enumerate() {
        stat(ui, rect(x + i as f32 * (cw + GAP), y, cw, 86.0), t, v, u, s);
    }
}

/// Gráfico de 60 s, de 0 a 100, com linhas de grade a cada 25%.
fn chart(ui: &mut Ui, r: Rect, title: &str, right: &str, vals: &VecDeque<f32>, c: Color) {
    ui.card(r);
    ui.c.text(rect(r.x + 14.0, r.y + 8.0, r.w - 120.0, 20.0), title, 13.0, MUTED, Align::Left, Weight::Regular);
    ui.c.text(rect(r.right() - 120.0, r.y + 8.0, 106.0, 20.0), right, 13.0, TEXT, Align::Right, Weight::Semibold);
    let g = rect(r.x + 14.0, r.y + 34.0, r.w - 28.0, r.h - 46.0);
    ui.c.round(g, 4.0, SIDE);
    for k in 1..4 {
        let y = g.y + g.h * k as f32 / 4.0;
        ui.c.line(g.x, y, g.right(), y, LINE, 1.0);
    }
    let v: Vec<f32> = vals.iter().copied().collect();
    ui.c.sparkline(g, &v, c, 1.5);
}

/// Barra empilhada com legenda embaixo: (nome, bytes, cor).
fn stack(ui: &mut Ui, r: Rect, title: &str, right: &str, parts: &[(String, u64, Color)], total: u64) {
    ui.card(r);
    ui.c.text(rect(r.x + 14.0, r.y + 8.0, r.w - 240.0, 20.0), title, 13.0, MUTED, Align::Left, Weight::Regular);
    ui.c.text(rect(r.right() - 254.0, r.y + 8.0, 240.0, 20.0), right, 13.0, TEXT, Align::Right, Weight::Semibold);
    let bar = rect(r.x + 14.0, r.y + 34.0, r.w - 28.0, 16.0);
    ui.c.round(bar, 4.0, SIDE);
    let mut x = bar.x;
    for (_, b, c) in parts {
        let w = bar.w * *b as f32 / total.max(1) as f32;
        ui.c.fill(rect(x, bar.y, w.min(bar.right() - x), bar.h), *c);
        x += w;
    }
    let mut lx = bar.x;
    for (name, b, c) in parts {
        let t = format!("{name} {} GB", gb(*b));
        let tw = measure(&t, 12.0, Weight::Regular, None).0 + 30.0;
        if lx + tw > r.right() - 14.0 {
            break;
        }
        ui.c.round(rect(lx, bar.bottom() + 12.0, 10.0, 10.0), 2.0, *c);
        ui.c.text_raw(rect(lx + 14.0, bar.bottom() + 6.0, tw, 22.0), &t, 12.0, TEXT2, Align::Left, Weight::Regular);
        lx += tw;
    }
}

/// Linha da tabela: o programa e o texto de cada coluna.
struct Row<'a> {
    g: &'a Group,
    vals: Vec<String>,
}

/// Tabela de programas; um clique abre o detalhe ao lado, outro fecha.
fn table(ui: &mut Ui, app: &mut App, r: Rect, key: &str, title: &str, cols: &[(&str, f32)], rows: &[Row]) {
    ui.card(r);
    ui.c.text(rect(r.x + 14.0, r.y + 8.0, r.w - 28.0, 20.0), title, 13.0, MUTED, Align::Left, Weight::Regular);
    ui.c.text(rect(r.x + 14.0, r.y + 8.0, r.w - 28.0, 20.0), "Toque numa linha para abrir", 12.0, MUTED, Align::Right, Weight::Regular);
    let total: f32 = cols.iter().map(|c| c.1 + 6.0).sum();
    let hy = r.y + 32.0;
    ui.c.text(rect(r.x + 24.0, hy, 160.0, 22.0), "Programa", 12.0, MUTED, Align::Left, Weight::Regular);
    let mut cx = r.right() - 22.0 - total;
    for (name, w) in cols {
        ui.c.text(rect(cx, hy, *w, 22.0), name, 12.0, MUTED, Align::Right, Weight::Regular);
        cx += w + 6.0;
    }
    let list = rect(r.x + 8.0, hy + 24.0, r.w - 16.0, r.bottom() - hy - 32.0);
    ui.begin_scroll(key, list, rows.len() as f32 * ROW);
    let first = (ui.scroll_y() / ROW) as usize;
    let last = first + (list.h / ROW) as usize + 2;
    for (i, row) in rows.iter().enumerate().take(last).skip(first) {
        let y = list.y + i as f32 * ROW;
        let lr = rect(list.x, y, list.w, ROW);
        let pid = row.g.pids.first().copied().unwrap_or(0);
        if app.perf_sel.as_deref() == Some(row.g.exe.as_str()) {
            ui.c.round(lr, 6.0, CARD2);
        } else if ui.is_hover(lr) {
            ui.c.round(lr, 6.0, CARD2.alpha(0.5));
        }
        match app.path_of(&row.g.exe, pid) {
            Some(path) => ui.file_icon(&path, &row.g.exe, rect(lr.x + 10.0, y + 8.0, 18.0, 18.0)),
            None => ui.c.round(rect(lr.x + 10.0, y + 8.0, 18.0, 18.0), 5.0, LINE),
        }
        let base = app.name_of(&row.g.exe, pid);
        let name = if row.g.pids.len() > 1 { format!("{base} ({})", row.g.pids.len()) } else { base };
        ui.c.text_raw(rect(lr.x + 36.0, y, list.w - total - 46.0, ROW), &name, 14.0, TEXT, Align::Left, Weight::Regular);
        let mut cx = r.right() - 22.0 - total;
        for (j, v) in row.vals.iter().enumerate() {
            ui.c.text_raw(rect(cx, y, cols[j].1, ROW), v, 13.0, TEXT, Align::Right, Weight::Regular);
            cx += cols[j].1 + 6.0;
        }
        if ui.hit(&format!("{key}-{}", row.g.exe), lr) {
            app.perf_sel = if app.perf_sel.as_deref() == Some(row.g.exe.as_str()) { None } else { Some(row.g.exe.clone()) };
        }
    }
    ui.end_scroll();
}

/// Detalhe do programa aberto: tudo o que se mede dele, e os botões do Medidor e de encerrar.
fn detail(ui: &mut Ui, app: &mut App, r: Rect, s: &Snapshot) {
    let Some(g) = app.perf_sel.as_ref().and_then(|e| s.groups.iter().find(|g| &g.exe == e)) else {
        app.perf_sel = None;
        return;
    };
    ui.card(r);
    let pid = g.pids.first().copied().unwrap_or(0);
    let path = app.path_of(&g.exe, pid);
    if let Some(p) = &path {
        ui.file_icon(p, &g.exe, rect(r.x + 14.0, r.y + 14.0, 22.0, 22.0));
    }
    let name = app.name_of(&g.exe, pid);
    ui.c.text_raw(rect(r.x + 44.0, r.y + 10.0, r.w - 58.0, 30.0), &name, 15.0, TEXT, Align::Left, Weight::Semibold);
    ui.c.text_raw(rect(r.x + 14.0, r.y + 38.0, r.w - 28.0, 18.0), &g.exe, 12.0, MUTED, Align::Left, Weight::Regular);
    let mut lines = vec![
        (tr("CPU").into_owned(), fmt_pct(g.cpu)),
        (tr("Threads").into_owned(), g.threads.to_string()),
        (tr("Processos").into_owned(), g.pids.len().to_string()),
        (tr("RAM em uso").into_owned(), fmt_mem(g.ram)),
        (tr("VRAM dedicada").into_owned(), fmt_mem(g.vram)),
        (tr("VRAM compartilhada").into_owned(), fmt_mem(g.shared)),
    ];
    for (i, n) in ENGINE_NAMES.iter().enumerate() {
        lines.push((format!("GPU {n}"), fmt_pct(g.eng[i].min(100.0))));
    }
    let mut y = r.y + 64.0;
    for (k, v) in &lines {
        if y + 22.0 > r.bottom() - 100.0 {
            break;
        }
        ui.c.text_raw(rect(r.x + 14.0, y, r.w - 28.0, 20.0), k, 13.0, MUTED, Align::Left, Weight::Regular);
        ui.c.text_raw(rect(r.x + 14.0, y, r.w - 28.0, 20.0), v, 13.0, TEXT, Align::Right, Weight::Regular);
        y += 22.0;
    }
    let exe = g.exe.clone();
    let pids = g.pids.clone();
    if ui.button_ex("perf-meter", rect(r.x + 14.0, r.bottom() - 92.0, r.w - 28.0, 36.0), "Abrir no Medidor", true, path.is_some()) {
        if let Some(p) = &path {
            app.add_meter_game(&exe, p);
            app.page = Page::Meter;
            app.refresh_page();
            ui.s.needs_redraw = true;
        }
    }
    if ui.button_ex("perf-end", rect(r.x + 14.0, r.bottom() - 48.0, r.w - 28.0, 36.0), "Finalizar tarefa", false, !crate::optimizer::is_system(&exe)) {
        app.mon_target = Some((exe.clone(), pids));
        ui.ask("perf-end", "Finalizar tarefa?", &format!("{} será fechado à força. O que não foi salvo nesse programa será perdido.", app_name(&exe)));
    }
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    let s = app.snap.clone();
    let ram = crate::sys::perf::ram();
    let ram_kind = if ram.kind.is_empty() { String::new() } else { format!(" {}", ram.kind) };
    let sub = format!(
        "{} · {} · {}{ram_kind}",
        crate::sys::perf::cpu_name(),
        if s.gpu_name.is_empty() { "GPU" } else { &s.gpu_name },
        fmt_gb(s.ram_total)
    );
    header_room(ui, a, "Desempenho", &sub, 320.0);
    if ui.check("perf-always", rect(a.right() - 316.0, 32.0, 288.0, 24.0), "Medir com a janela fechada", &mut app.cfg.perf_always) {
        app.cfg.save();
        app.refresh_page();
    }
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let mut tab = app.perf_tab as usize;
    if ui.segment("perf-tab", x, 88.0, 32.0, &TABS, &mut tab) {
        app.perf_tab = tab as u8;
    }
    let mut y = 132.0;
    // Com um programa aberto, a tabela divide a largura com o detalhe.
    let side = 300.0;
    let rows_w = if app.perf_sel.is_some() { w - side - GAP } else { w };
    let dash = || "—".to_string();
    let nv = s.gpu.mhz > 0;
    match app.perf_tab {
        1 => {
            let pi = perf_info();
            let hot = s.threads.iter().copied().fold(0.0f32, f32::max);
            let base = s.base_mhz / 1000.0;
            stats(
                ui,
                x,
                y,
                w,
                &[
                    ("Uso total", format!("{:.0}", s.cpu), "%", format!("{} {:.0}%", tr("thread mais cheia:"), hot)),
                    ("Clock", if s.ghz > 0.0 { dec(s.ghz, 2) } else { dash() }, "GHz", if base > 0.0 { format!("{} {} GHz", tr("base"), dec(base, 1)) } else { String::new() }),
                    ("Temperatura", s.cpu_temp.map_or_else(dash, |t| format!("{t:.0}")), if s.cpu_temp.is_some() { "°C" } else { "" }, tr("pelo PawnIO").into_owned()),
                    ("Processos", pi.ProcessCount.to_string(), "", format!("{} {}", pi.ThreadCount, tr("threads"))),
                    ("Ligado há", crate::overlays::uptime(unsafe { windows::Win32::System::SystemInformation::GetTickCount64() } / 1000), "", String::new()),
                ],
            );
            y += 86.0 + GAP;
            if s.cpu_temp.is_none() && !meter::pawnio_installed() {
                meter::pawnio_row(ui, rect(x, y, w, 40.0));
                y += 40.0 + GAP;
            }
            let half = (w - GAP) / 2.0;
            chart(ui, rect(x, y, half, 170.0), "Uso total · 60 s", &format!("{:.0}%", s.cpu), &s.hist_cpu, ACCENT);
            threads(ui, rect(x + half + GAP, y, half, 170.0), &s);
            y += 170.0 + GAP;
            let mut gs: Vec<&Group> = s.groups.iter().collect();
            gs.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
            let rows: Vec<Row> = gs.into_iter().map(|g| Row { g, vals: vec![fmt_pct(g.cpu), g.threads.to_string(), g.pids.len().to_string()] }).collect();
            table(ui, app, rect(x, y, rows_w, a.h - y - 20.0), "perf-cpu", "Programas", &[("CPU", 60.0), ("Threads", 70.0), ("Processos", 80.0)], &rows);
        }
        2 => {
            let pi = perf_info();
            let page = pi.PageSize as u64;
            let p = &s.perf;
            // Sem a leitura das listas (primeiro segundo), mostra só o em uso do Windows.
            let in_use = if p.free + p.standby > 0 { s.ram_total.saturating_sub(p.modified + p.standby + p.free) } else { s.ram_used };
            let parts = [
                (tr("Em uso").into_owned(), in_use, ACCENT),
                (tr("Modificada").into_owned(), p.modified, WARN),
                (tr("Em espera (cache)").into_owned(), p.standby, rgb(0x8fd694)),
                (tr("Livre").into_owned(), p.free, BORDER2),
            ];
            let right = format!("{} {} {} GB {}", gb(in_use), tr("de"), gb(s.ram_total), tr("em uso"));
            stack(ui, rect(x, y, w, 84.0), "Composição", &right, &parts, s.ram_total);
            y += 84.0 + GAP;
            let half = (w - GAP) / 2.0;
            chart(ui, rect(x, y, half, 190.0), "Em uso · 60 s", &format!("{:.0}%", s.ram_pct()), &s.hist_ram, ACCENT);
            let d = rect(x + half + GAP, y, half, 190.0);
            ui.card(d);
            ui.c.text(rect(d.x + 14.0, d.y + 8.0, d.w - 28.0, 20.0), "Detalhes", 13.0, MUTED, Align::Left, Weight::Regular);
            let speed = if ram.speed > 0 { format!("{}-{} · {} {} {} {}", ram.kind, ram.speed, ram.used, tr("de"), ram.slots, tr("slots")) } else { dash() };
            let kv = [
                ("Comprometida", format!("{} {} {}", fmt_mem(pi.CommitTotal as u64 * page), tr("de"), fmt_mem(pi.CommitLimit as u64 * page))),
                ("Cache", fmt_mem(pi.SystemCache as u64 * page)),
                ("Pool paginado", fmt_mem(pi.KernelPaged as u64 * page)),
                ("Pool não paginado", fmt_mem(pi.KernelNonpaged as u64 * page)),
                ("Disponível", fmt_mem(s.ram_total.saturating_sub(s.ram_used))),
                ("Pentes", speed),
            ];
            for (i, (k, v)) in kv.iter().enumerate() {
                let ly = d.y + 34.0 + i as f32 * 24.0;
                ui.c.text(rect(d.x + 14.0, ly, d.w - 28.0, 22.0), k, 13.0, MUTED, Align::Left, Weight::Regular);
                ui.c.text_raw(rect(d.x + 14.0, ly, d.w - 28.0, 22.0), v, 13.0, TEXT, Align::Right, Weight::Regular);
            }
            y += 190.0 + GAP;
            let mut gs: Vec<&Group> = s.groups.iter().collect();
            gs.sort_by(|a, b| b.ram.cmp(&a.ram));
            let rows: Vec<Row> = gs.into_iter().map(|g| Row { g, vals: vec![fmt_mem(g.ram), g.pids.len().to_string()] }).collect();
            table(ui, app, rect(x, y, rows_w, a.h - y - 20.0), "perf-ram", "Programas", &[("Em uso", 80.0), ("Processos", 80.0)], &rows);
        }
        3 => {
            let mut gs: Vec<&Group> = s.groups.iter().filter(|g| g.vram > 0 || g.shared > 0).collect();
            gs.sort_by(|a, b| b.vram.cmp(&a.vram));
            // shortcut: o Windows deixa a GPU usar até metade da RAM; ler o limite exato do adaptador se precisar.
            let shared_max = s.ram_total / 2;
            let pct = |a: u64, b: u64| if b > 0 { a as f32 / b as f32 * 100.0 } else { 0.0 };
            stats(
                ui,
                x,
                y,
                w,
                &[
                    ("Dedicada", gb(s.gpu.used), "GB", format!("{} {} GB · {:.0}%", tr("de"), gb(s.gpu.total), s.vram_pct())),
                    ("Compartilhada", gb(s.perf.shared), "GB", format!("{} {} GB · {}", tr("de"), gb(shared_max), tr("sai da RAM"))),
                    ("Programas usando", gs.len().to_string(), "", gs.first().map_or_else(String::new, |g| format!("{} {}", tr("maior:"), app_name(&g.exe)))),
                ],
            );
            y += 86.0 + GAP;
            let mut parts: Vec<(String, u64, Color)> = gs.iter().zip(SLICES).map(|(g, c)| (app_name(&g.exe).to_string(), g.vram, c)).collect();
            let sum: u64 = parts.iter().map(|p| p.1).sum();
            let rest = s.gpu.used.saturating_sub(sum);
            if rest > 0 {
                parts.push((tr("Outros").into_owned(), rest, BORDER2));
            }
            // O contador por programa conta memória que a placa não tem residente e pode somar mais que o
            // usado; aí a barra encolhe na proporção para terminar no uso real.
            let scale = if s.gpu.used > 0 && sum > s.gpu.used { sum as f64 / s.gpu.used as f64 } else { 1.0 };
            stack(ui, rect(x, y, w, 84.0), "Dedicada por programa", &format!("{:.0}%", pct(s.gpu.used, s.gpu.total)), &parts, (s.gpu.total as f64 * scale) as u64);
            y += 84.0 + GAP;
            chart(ui, rect(x, y, w, 150.0), "Dedicada · 60 s", &format!("{:.0}%", s.vram_pct()), &s.hist_vram, ACCENT);
            y += 150.0 + GAP;
            let rows: Vec<Row> = gs.into_iter().map(|g| Row { g, vals: vec![fmt_mem(g.vram), fmt_mem(g.shared)] }).collect();
            table(ui, app, rect(x, y, rows_w, a.h - y - 20.0), "perf-vram", "Programas", &[("Dedicada", 90.0), ("Compartilhada", 110.0)], &rows);
        }
        _ => {
            let top = (0..ENGINES).max_by(|&i, &j| s.perf.eng[i].total_cmp(&s.perf.eng[j])).filter(|&i| s.perf.eng[i] > 0.0);
            let reason = crate::sys::nvml::REASONS.iter().find(|r| s.gpu.reasons & r.0 != 0).map_or("Nenhum", |r| match r.1 {
                "power_cap" | "power_brake" => "Limite de energia",
                "sw_thermal" | "hw_thermal" => "Temperatura",
                "hw_slowdown" => "Lentidão do hardware",
                _ => "Ociosa",
            });
            let only_nv = || tr("só na NVIDIA").into_owned();
            let g = &s.gpu;
            stats(
                ui,
                x,
                y,
                w,
                &[
                    ("Uso total", format!("{:.0}", g.util), "%", top.map_or_else(String::new, |i| format!("{} {}", tr("maior engine:"), ENGINE_NAMES[i]))),
                    ("Clock do núcleo", if nv { g.mhz.to_string() } else { dash() }, if nv { "MHz" } else { "" }, if nv { format!("{} {} MHz", tr("máx."), g.max_mhz) } else { only_nv() }),
                    ("Clock da memória", if nv { g.mem_mhz.to_string() } else { dash() }, if nv { "MHz" } else { "" }, if nv { format!("{} {}%", tr("controlador"), g.mem_util) } else { only_nv() }),
                    ("Temperatura", if g.temp > 0 { g.temp.to_string() } else { dash() }, if g.temp > 0 { "°C" } else { "" }, if nv { format!("{} {}%", tr("ventoinha"), g.fan) } else { String::new() }),
                    ("Consumo", if nv { format!("{:.0}", g.watts) } else { dash() }, if nv { "W" } else { "" }, if nv { format!("{} {:.0} W", tr("limite"), g.power_limit) } else { only_nv() }),
                    ("Limite do clock", if nv { tr(reason).into_owned() } else { dash() }, "", if nv { String::new() } else { only_nv() }),
                ],
            );
            y += 86.0 + GAP;
            let ew = (w - GAP * (ENGINES as f32 - 1.0)) / ENGINES as f32;
            for i in 0..ENGINES {
                let r = rect(x + i as f32 * (ew + GAP), y, ew, 84.0);
                ui.card(r);
                ui.c.text_raw(rect(r.x + 14.0, r.y + 8.0, r.w - 28.0, 20.0), ENGINE_NAMES[i], 13.0, MUTED, Align::Left, Weight::Regular);
                ui.c.text_raw(rect(r.x + 14.0, r.y + 8.0, r.w - 28.0, 20.0), &format!("{:.0}%", s.perf.eng[i]), 13.0, TEXT, Align::Right, Weight::Semibold);
                let v: Vec<f32> = s.hist_eng[i].iter().copied().collect();
                ui.c.sparkline(rect(r.x + 14.0, r.y + 36.0, r.w - 28.0, 38.0), &v, if s.perf.eng[i] >= 80.0 { WARN } else { ACCENT }, 1.5);
            }
            y += 84.0 + GAP;
            chart(ui, rect(x, y, w, 150.0), "Uso total · 60 s", &format!("{:.0}%", g.util), &s.hist_gpu, ACCENT);
            y += 150.0 + GAP;
            let mut gs: Vec<&Group> = s.groups.iter().filter(|g| g.eng.iter().any(|v| *v > 0.0) || g.vram > 0).collect();
            gs.sort_by(|a, b| b.eng.iter().sum::<f32>().total_cmp(&a.eng.iter().sum::<f32>()).then(b.vram.cmp(&a.vram)));
            let rows: Vec<Row> = gs.into_iter().map(|g| Row { g, vals: g.eng.iter().map(|v| fmt_pct(v.min(100.0))).chain([fmt_mem(g.vram)]).collect() }).collect();
            let cols: Vec<(&str, f32)> = ENGINE_NAMES.iter().map(|n| (*n, 60.0)).chain([("VRAM", 76.0)]).collect();
            table(ui, app, rect(x, y, rows_w, a.h - y - 20.0), "perf-gpu", "Programas por engine", &cols, &rows);
        }
    }
    if app.perf_sel.is_some() {
        detail(ui, app, rect(x + rows_w + GAP, y, side, a.h - y - 20.0), &s);
    }
    if ui.answered("perf-end") {
        if let Some((_, pids)) = app.mon_target.take() {
            crate::optimizer::force(&pids);
            app.perf_sel = None;
        }
    }
}

/// Cada thread: uso em barra e o clock efetivo embaixo.
fn threads(ui: &mut Ui, r: Rect, s: &Snapshot) {
    ui.card(r);
    ui.c.text(rect(r.x + 14.0, r.y + 8.0, r.w - 28.0, 20.0), "Por thread (uso e clock)", 13.0, MUTED, Align::Left, Weight::Regular);
    let n = s.threads.len().max(1);
    let g = rect(r.x + 14.0, r.y + 34.0, r.w - 28.0, r.h - 46.0);
    let cols = ((g.w / 58.0) as usize).clamp(1, n);
    let lines = n.div_ceil(cols);
    let (cw, ch) = (g.w / cols as f32, g.h / lines as f32);
    for (i, v) in s.threads.iter().enumerate() {
        let c = rect(g.x + (i % cols) as f32 * cw, g.y + (i / cols) as f32 * ch, cw - 4.0, ch - 4.0);
        ui.c.round(c, 3.0, SIDE);
        let h = c.h * v.clamp(0.0, 100.0) / 100.0;
        ui.c.round(rect(c.x, c.bottom() - h, c.w, h), 3.0, if *v >= 70.0 { WARN.alpha(0.6) } else { ACCENT.alpha(0.45) });
        if ch >= 30.0 {
            let mhz = s.perf.thread_perf.get(i).map(|p| p / 100.0 * s.base_mhz / 1000.0).filter(|g| *g > 0.0);
            ui.c.text_raw(rect(c.x, c.y + 1.0, c.w, 14.0), &format!("{v:.0}%"), 11.0, TEXT, Align::Center, Weight::Regular);
            if let Some(ghz) = mhz {
                ui.c.text_raw(rect(c.x, c.bottom() - 15.0, c.w, 14.0), &dec(ghz, 2), 11.0, TEXT2, Align::Center, Weight::Regular);
            }
        }
    }
}
