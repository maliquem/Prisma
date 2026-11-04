use std::time::Instant;

use super::*;
use crate::sys::power::{self, Mode};

fn plan_select(ui: &mut Ui, key: &str, r: Rect, app_plans: &[String], ids: &[String], cur: &mut String) -> bool {
    let mut sel = ids.iter().position(|i| i == cur).unwrap_or(0);
    if ui.select(key, r, app_plans, &mut sel) {
        *cur = ids[sel].clone();
        return true;
    }
    false
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    let sub = match app.detector.mode {
        Mode::Rest => "Em repouso".to_string(),
        Mode::Load => format!("Carga alta há {} min", app.detector.since.elapsed().as_secs() / 60),
        Mode::Game => format!(
            "Modo jogo há {} min · {} detectado",
            app.detector.since.elapsed().as_secs() / 60,
            app_name(&app.detector.game_exe)
        ),
    };
    header(ui, a, "Energia", &sub);
    let mut auto = app.cfg.power_auto;
    if ui.check("auto", rect(a.right() - 200.0, 30.0, 172.0, 34.0), "Troca automática", &mut auto) {
        app.cfg.power_auto = auto;
        app.cfg.save();
    }

    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let names: Vec<String> = app.plans.iter().map(|p| p.name.clone()).collect();
    let ids: Vec<String> = app.plans.iter().map(|p| p.id.clone()).collect();
    let cw = (w - 24.0) / 3.0;
    let top = 88.0;
    let r0 = rect(x, top, cw, 92.0);
    let r1 = rect(x + cw + 12.0, top, cw, 92.0);
    let r2 = rect(x + 2.0 * (cw + 12.0), top, cw, 92.0);
    ui.card_on(r0);
    ui.card(r1);
    ui.card(r2);
    ui.label(rect(r0.x + 16.0, r0.y + 10.0, r0.w - 32.0, 18.0), "Com jogo ou carga pesada", TEXT2);
    ui.label(rect(r1.x + 16.0, r1.y + 10.0, r1.w - 32.0, 18.0), "No resto do tempo", TEXT2);
    ui.label(rect(r2.x + 16.0, r2.y + 10.0, r2.w - 32.0, 18.0), "Ao entrar em modo jogo", TEXT2);

    let tl_y = top + 104.0;
    let tl = rect(x, tl_y, w, 84.0);
    ui.card(tl);
    ui.title(rect(tl.x + 16.0, tl.y + 10.0, 300.0, 22.0), "Últimas 2 horas");
    let bar = rect(tl.x + 16.0, tl.y + 42.0, tl.w - 32.0, 28.0);
    ui.c.round(bar, 6.0, SIDE);
    let now = Instant::now();
    let span = 2.0 * 3600.0;
    for s in &app.detector.timeline {
        let ago0 = (now - s.start).as_secs_f32().min(span);
        let ago1 = s.end.map_or(0.0, |e| (now - e).as_secs_f32());
        let x0 = bar.right() - ago0 / span * bar.w;
        let x1 = bar.right() - ago1 / span * bar.w;
        let r = rect(x0, bar.y, (x1 - x0).max(1.0), bar.h);
        let (bg, fg) = if s.mode == Mode::Rest { (LINE, TEXT) } else { (ACCENT, ON_ACCENT) };
        ui.c.fill(r, bg);
        if r.w > 60.0 {
            ui.c.text(r.inset_xy(8.0, 0.0), &s.label, 12.0, fg, Align::Left, Weight::Regular);
        }
    }

    let by = tl_y + 96.0;
    let bh = a.h - by - 20.0;
    let rules_w = 300.0;
    let t = rect(x, by, w - rules_w - 12.0, bh);
    ui.card(t);
    ui.title(rect(t.x + 16.0, t.y + 10.0, 300.0, 22.0), "Seus planos na tomada");
    let has_prisma = app.plans.iter().any(|p| p.name == power::NAME_REST);
    if !has_prisma && ui.button("mk", rect(t.right() - 186.0, t.y + 8.0, 170.0, 36.0), "Criar planos Prisma", true) {
        let (rest, game) = power::ensure_prisma_plans();
        if let Some(r) = rest {
            app.cfg.plan_rest = r;
        }
        let _ = game;
        app.cfg.save();
        app.refresh_plans();
    }
    let cols = [("Mínimo", 70.0), ("EPP", 50.0), ("Boost", 104.0), ("Núcleos", 70.0)];
    let total: f32 = cols.iter().map(|c| c.1 + 8.0).sum();
    let hy = t.y + 44.0;
    ui.label(rect(t.x + 24.0, hy, 200.0, 20.0), "Plano", MUTED);
    let mut cx = t.right() - 16.0 - total;
    for (n, cw) in &cols {
        ui.c.text(rect(cx, hy, *cw, 20.0), n, 12.0, MUTED, Align::Left, Weight::Regular);
        cx += cw + 8.0;
    }
    let mut warn = false;
    for (i, v) in app.plan_values.iter().enumerate() {
        let y = hy + 24.0 + i as f32 * 32.0;
        if y + 32.0 > t.bottom() - 28.0 {
            break;
        }
        let r = rect(t.x + 8.0, y, t.w - 16.0, 32.0);
        let current = app.plans[i].id == app.active_plan;
        if current {
            ui.c.round(r, 6.0, CARD2);
        }
        ui.c.text(rect(r.x + 16.0, y, r.w - total - 28.0, 32.0), &v.name, 14.0, TEXT, Align::Left, Weight::Regular);
        let is_saver = app.plans[i].id.starts_with("a1841308");
        let bad = is_saver && v.min >= 100;
        warn |= bad;
        let cells = [format!("{}%", v.min), v.epp.to_string(), power::boost_name(v.boost).to_string(), format!("{}%", v.park)];
        let mut cx = t.right() - 16.0 - total;
        for (j, c) in cells.iter().enumerate() {
            let color = if bad && j == 0 { WARN } else { TEXT };
            ui.c.text(rect(cx, y, cols[j].1, 32.0), c, 14.0, color, Align::Left, Weight::Regular);
            cx += cols[j].1 + 8.0;
        }
    }
    if warn {
        let msg = "Sua Economia de energia mantém a CPU em 100% mínimo, então quase não economiza.";
        ui.c.text(rect(t.x + 16.0, t.bottom() - 28.0, t.w - 32.0, 20.0), msg, 13.0, WARN, Align::Left, Weight::Regular);
    }

    let rr = rect(t.right() + 12.0, by, rules_w, bh);
    ui.card(rr);
    ui.title(rect(rr.x + 16.0, rr.y + 10.0, rr.w - 32.0, 22.0), "O que conta como carga");
    let c = &mut app.cfg;
    let mut changed = false;
    let rules: [(&str, &mut bool); 4] = [
        ("Jogo em tela cheia ou sem bordas", &mut c.rule_fullscreen),
        ("GPU 3D acima de 40% por 5 s", &mut c.rule_gpu),
        ("CPU acima de 35% por 20 s", &mut c.rule_cpu),
        ("Steam, Epic, GOG e Xbox", &mut c.rule_library),
    ];
    for (i, (label, v)) in rules.into_iter().enumerate() {
        let y = rr.y + 40.0 + i as f32 * 44.0;
        changed |= ui.check(label, rect(rr.x + 16.0, y, rr.w - 32.0, 44.0), label, v);
    }
    ui.label(rect(rr.x + 16.0, rr.y + 222.0, rr.w - 32.0, 20.0), "Volta ao repouso após 60 s sem carga.", MUTED);

    // Por último, porque a lista aberta fica por cima do resto.
    let mut on = app.cfg.on_game as usize;
    let opts = ["Perguntar se quero otimizar".to_string(), "Otimizar sem perguntar".to_string(), "Só trocar o plano".to_string()];
    if ui.select("ongame", rect(r2.x + 16.0, r2.y + 34.0, r2.w - 32.0, 44.0), &opts, &mut on) {
        app.cfg.on_game = on as u8;
        changed = true;
    }
    changed |= plan_select(ui, "pgame", rect(r0.x + 16.0, r0.y + 34.0, r0.w - 32.0, 44.0), &names, &ids, &mut app.cfg.plan_game);
    changed |= plan_select(ui, "prest", rect(r1.x + 16.0, r1.y + 34.0, r1.w - 32.0, 44.0), &names, &ids, &mut app.cfg.plan_rest);
    if changed {
        app.cfg.save();
    }
}
