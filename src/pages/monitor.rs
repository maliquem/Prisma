use super::*;

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    let s = app.snap.clone();
    let sub = format!(
        "{} threads · {} · {}",
        s.threads.len(),
        if s.gpu_name.is_empty() { "GPU" } else { &s.gpu_name },
        fmt_gb(s.ram_total)
    );
    header(ui, a, "Monitor", &sub);
    if ui.button("opt-now", rect(a.right() - 168.0, 24.0, 140.0, 44.0), "Otimizar agora", true) {
        app.page = Page::Optimize;
        app.refresh_page();
        ui.s.needs_redraw = true;
    }

    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let top = 88.0;
    let cw = (w - 36.0) / 4.0;
    let cards = [
        ("CPU", format!("{:.0}", s.cpu), "%", if s.ghz > 0.0 { format!("{:.2} GHz", s.ghz).replace('.', ",") } else { String::new() }, &s.hist_cpu),
        ("GPU", format!("{:.0}", s.gpu.util), "%", format!("{} °C", s.gpu.temp), &s.hist_gpu),
        ("RAM", format!("{:.1}", s.ram_used as f64 / (1u64 << 30) as f64).replace('.', ","), "GB", format!("de {}", fmt_gb(s.ram_total)), &s.hist_ram),
        ("VRAM", format!("{:.1}", s.gpu.used as f64 / (1u64 << 30) as f64).replace('.', ","), "GB", format!("de {}", fmt_gb(s.gpu.total)), &s.hist_vram),
    ];
    for (i, (name, val, unit, right, hist)) in cards.iter().enumerate() {
        let r = rect(x + i as f32 * (cw + 12.0), top, cw, 118.0);
        ui.card(r);
        ui.c.text(rect(r.x + 16.0, r.y + 10.0, 80.0, 20.0), name, 13.0, MUTED, Align::Left, Weight::Regular);
        ui.c.text(rect(r.x + 60.0, r.y + 10.0, r.w - 76.0, 20.0), right, 13.0, MUTED, Align::Right, Weight::Regular);
        ui.c.text(rect(r.x + 16.0, r.y + 30.0, r.w - 32.0, 42.0), val, 32.0, TEXT, Align::Left, Weight::Semibold);
        let vw = measure(val, 32.0, Weight::Semibold, None).0;
        ui.c.text(rect(r.x + 22.0 + vw, r.y + 42.0, 40.0, 28.0), unit, 14.0, MUTED, Align::Left, Weight::Regular);
        let vals: Vec<f32> = hist.iter().copied().collect();
        let hot = vals.last().copied().unwrap_or(0.0) >= 80.0;
        ui.c.sparkline(rect(r.x + 16.0, r.y + 76.0, r.w - 32.0, 32.0), &vals, if hot { WARN } else { ACCENT }, 1.5);
    }

    let by = top + 130.0;
    let bh = a.h - by - 20.0;
    let core_w = 290.0;
    let tw = w - core_w - 12.0;
    let t = rect(x, by, tw, bh);
    ui.card(t);
    ui.title(rect(t.x + 16.0, t.y + 12.0, 200.0, 22.0), "Programas");
    let cols = [("CPU", 64.0), ("RAM", 80.0), ("GPU", 64.0), ("VRAM", 80.0)];
    let total: f32 = cols.iter().map(|c| c.1 + 8.0).sum();
    let hy = t.y + 38.0;
    let mut cx = t.right() - 16.0 - total;
    ui.label(rect(t.x + 24.0, hy, 200.0, 22.0), "Programa", MUTED);
    for (i, (name, cw)) in cols.iter().enumerate() {
        let r = rect(cx, hy, *cw, 22.0);
        let sel = app.sort == i as u8;
        ui.c.text(r, name, 12.0, if sel { ACCENT } else { MUTED }, Align::Right, if sel { Weight::Semibold } else { Weight::Regular });
        if ui.hit(name, r) {
            app.sort = i as u8;
        }
        cx += cw + 8.0;
    }

    let mut rows = s.groups.clone();
    match app.sort {
        1 => rows.sort_by(|a, b| b.ram.cmp(&a.ram)),
        2 => rows.sort_by(|a, b| b.gpu.partial_cmp(&a.gpu).unwrap()),
        3 => rows.sort_by(|a, b| b.vram.cmp(&a.vram)),
        _ => rows.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap()),
    }
    let list = rect(t.x + 8.0, hy + 24.0, t.w - 16.0, t.bottom() - hy - 32.0);
    rows.truncate(60);
    ui.begin_scroll("procs", list, rows.len() as f32 * 34.0);
    for (i, p) in rows.iter().enumerate() {
        let y = list.y + i as f32 * 34.0;
        let r = rect(list.x, y, list.w, 34.0);
        if i == 0 {
            ui.c.round(r, 6.0, CARD2);
        }
        let name = if p.pids.len() > 1 {
            format!("{} ({} processos)", app_name(&p.exe), p.pids.len())
        } else {
            app_name(&p.exe).to_string()
        };
        let nw = list.w - total - 28.0;
        ui.c.text(rect(r.x + 16.0, y, nw, 34.0), &name, 14.0, TEXT, Align::Left, Weight::Regular);
        let vals = [fmt_pct(p.cpu), fmt_mem(p.ram), fmt_pct(p.gpu.min(100.0)), fmt_mem(p.vram)];
        let mut cx = t.right() - 16.0 - total;
        for (j, v) in vals.iter().enumerate() {
            let cw = cols[j].1;
            ui.c.text(rect(cx, y, cw, 34.0), v, 14.0, TEXT, Align::Right, Weight::Regular);
            cx += cw + 8.0;
        }
    }
    ui.end_scroll();

    let c = rect(t.right() + 12.0, by, core_w, bh);
    ui.card(c);
    ui.title(rect(c.x + 16.0, c.y + 12.0, 200.0, 22.0), "Núcleos");
    ui.label(rect(c.x + 16.0, c.y + 34.0, c.w - 32.0, 20.0), "Cada coluna é um núcleo com 2 threads", MUTED);
    let n = (s.threads.len() / 2).max(1);
    let gap = 8.0;
    let col = (c.w - 32.0 - gap * (n as f32 - 1.0)) / n as f32;
    let bar_h = (bh - 180.0).clamp(60.0, 160.0);
    let by2 = c.y + 62.0;
    let mut best = (0usize, -1.0f32);
    let mut least = (0usize, 101.0f32);
    for i in 0..n {
        let cx = c.x + 16.0 + i as f32 * (col + gap);
        let a0 = s.threads.get(i * 2).copied().unwrap_or(0.0);
        let b0 = s.threads.get(i * 2 + 1).copied().unwrap_or(0.0);
        let load = a0.max(b0);
        if load > best.1 {
            best = (i, load);
        }
        if load < least.1 {
            least = (i, load);
        }
        for (k, v) in [a0, b0].iter().enumerate() {
            let bw = (col - 2.0) / 2.0;
            let bx = cx + k as f32 * (bw + 2.0);
            ui.c.round(rect(bx, by2, bw, bar_h), 3.0, SIDE);
            let h = bar_h * v / 100.0;
            ui.c.round(rect(bx, by2 + bar_h - h, bw, h), 3.0, if *v >= 70.0 { WARN } else { ACCENT });
        }
        ui.c.text(rect(cx, by2 + bar_h + 2.0, col, 18.0), &i.to_string(), 12.0, MUTED, Align::Center, Weight::Regular);
    }
    let dy = by2 + bar_h + 28.0;
    let lines = [
        ("Mais usado", format!("Núcleo {} · {:.0}%", best.0, best.1)),
        ("Menos usado", format!("Núcleo {} · {:.0}%", least.0, least.1)),
        ("Frequência", if s.ghz > 0.0 { format!("{:.2} GHz", s.ghz).replace('.', ",") } else { "—".into() }),
    ];
    for (i, (k, v)) in lines.iter().enumerate() {
        let y = dy + i as f32 * 22.0;
        ui.label(rect(c.x + 16.0, y, 110.0, 20.0), k, MUTED);
        ui.c.text(rect(c.x + 100.0, y, c.w - 116.0, 20.0), v, 13.0, TEXT, Align::Right, Weight::Regular);
    }
}
