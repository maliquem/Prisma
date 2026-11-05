use super::picker::{self, Target};
use super::*;
use crate::config::Defaults;
use crate::meter;

const ROW: f32 = 56.0;
const ITEMS: [(&str, u32); 14] = [
    ("FPS e médias", meter::ITEM_FPS),
    ("Frame time", meter::ITEM_FRAME),
    ("Latência do PC", meter::ITEM_LATENCY),
    ("Limitador", meter::ITEM_LIMIT),
    ("Upscaling", meter::ITEM_UPSCALE),
    ("Geração de quadros", meter::ITEM_GEN),
    ("Reflex", meter::ITEM_REFLEX),
    ("HDR", meter::ITEM_HDR),
    ("CPU por núcleo", meter::ITEM_CPU),
    ("GPU", meter::ITEM_GPU),
    ("VRAM e RAM", meter::ITEM_MEM),
    ("Disco e rede", meter::ITEM_IO),
    ("Modo de exibição", meter::ITEM_MODE),
    ("Tempo de jogo", meter::ITEM_TIME),
];

fn opts(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn general(ui: &mut Ui, app: &mut App, r: Rect) {
    ui.card(r);
    let (x, w) = (r.x + 16.0, (r.w - 56.0) / 3.0);
    let mut changed = false;
    ui.label(rect(x, r.y + 12.0, w, 18.0), "Quando o medidor liga", TEXT2);
    let mut s = app.cfg.meter_rule as usize;
    if ui.select("mt-rule", rect(x, r.y + 34.0, w, 40.0), &opts(&["Só nos jogos da lista", "Em todos, menos os da lista"]), &mut s) {
        app.cfg.meter_rule = s as u8;
        changed = true;
    }
    ui.label(rect(x, r.y + 86.0, w, 18.0), "HDR automático", TEXT2);
    let mut s = app.cfg.meter_hdr as usize;
    if ui.select("mt-hdr", rect(x, r.y + 108.0, w, 40.0), &opts(&["Desligado", "Só na lista", "Todos, menos a lista"]), &mut s) {
        app.cfg.meter_hdr = s as u8;
        changed = true;
    }
    ui.c.text_wrap(rect(x, r.y + 152.0, w, 36.0), "Liga o HDR do Windows quando o jogo abrir e volta quando fechar. É separado do medidor.", 12.0, MUTED, Weight::Regular);

    let kx = x + w + 12.0;
    let keys: [(&str, &str, u8); 3] = [("mt-k1", "Mostrar compacto", 0), ("mt-k2", "Mostrar completo", 1), ("mt-k3", "Esconder tudo", 2)];
    let mut hk = false;
    for (i, (key, name, which)) in keys.iter().enumerate() {
        let y = r.y + 12.0 + i as f32 * 60.0;
        ui.label(rect(kx, y, w, 18.0), name, TEXT2);
        let target = match which {
            0 => &mut app.cfg.hk_meter_compact,
            1 => &mut app.cfg.hk_meter_full,
            _ => &mut app.cfg.hk_meter_off,
        };
        hk |= ui.hotkey(key, rect(kx, y + 20.0, w, 36.0), target);
    }

    let ox = kx + w + 12.0;
    ui.label(rect(ox, r.y + 12.0, w, 18.0), "Opacidade do compacto", TEXT2);
    changed |= ui.stepper("mt-o1", rect(ox, r.y + 34.0, w, 36.0), &mut app.cfg.meter_op_compact, (20, 100), 4, "%");
    ui.label(rect(ox, r.y + 86.0, w, 18.0), "Opacidade do completo", TEXT2);
    changed |= ui.stepper("mt-o2", rect(ox, r.y + 108.0, w, 36.0), &mut app.cfg.meter_op_full, (20, 100), 4, "%");
    if changed || hk {
        app.cfg.save();
    }
    if hk {
        app.register_hotkeys();
    }
}

fn items(ui: &mut Ui, app: &mut App, r: Rect) {
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "O que aparece no completo");
    ui.label(rect(r.x + 16.0, r.y + 36.0, r.w - 32.0, 18.0), "Fica em uma coluna. A segunda só abre se não couber na tela.", MUTED);
    let w = (r.w - 32.0) / 4.0;
    let mut changed = false;
    for (i, (name, bit)) in ITEMS.iter().enumerate() {
        let on_rect = rect(r.x + 16.0 + (i % 4) as f32 * w, r.y + 58.0 + (i / 4) as f32 * 38.0, w - 8.0, 36.0);
        let mut on = app.cfg.meter_items & bit != 0;
        if ui.check(&format!("mt-i{i}"), on_rect, name, &mut on) {
            app.cfg.meter_items = if on { app.cfg.meter_items | bit } else { app.cfg.meter_items & !bit };
            changed = true;
        }
    }
    if changed {
        app.cfg.save();
    }
}

fn games(ui: &mut Ui, app: &mut App, r: Rect) {
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 150.0, 22.0), "Jogos");
    picker::add_button(ui, app, rect(r.right() - 124.0, r.y + 8.0, 108.0, 32.0), Target::MeterGame);
    let (view_x, limit_x, fps_x, unit_x, hdr_x, preset_x) = (r.right() - 656.0, r.right() - 556.0, r.right() - 442.0, r.right() - 336.0, r.right() - 226.0, r.right() - 136.0);
    let y = r.y + 46.0;
    for (x, name) in [(view_x, "Abrir em"), (limit_x, "Limitador"), (fps_x, "FPS"), (unit_x, "Limitando"), (hdr_x, "HDR"), (preset_x, "Perfil DLSS")] {
        ui.c.text(rect(x, y, 104.0, 18.0), name, 12.0, MUTED, Align::Left, Weight::Regular);
    }
    let cap = crate::meter::first_limit(app.refresh_hz());
    let (mut remove, mut limit, mut preset, mut changed) = (None, None, None, false);
    let count = app.cfg.meter_games.len();
    if count == 0 {
        ui.c.text(rect(r.x + 16.0, y + 24.0, r.w - 32.0, 22.0), "Nenhum jogo ainda. Use Adicionar.", 13.0, MUTED, Align::Left, Weight::Regular);
    }
    for i in 0..count {
        let top = y + 24.0 + i as f32 * ROW;
        let g = &mut app.cfg.meter_games[i];
        ui.file_icon(&g.path, &g.exe, rect(r.x + 16.0, top + 13.0, 26.0, 26.0));
        let room = view_x - r.x - 64.0;
        ui.c.text(rect(r.x + 52.0, top + 6.0, room, 22.0), app_name(&g.exe), 14.0, TEXT, Align::Left, Weight::Regular);
        let note = if crate::meter::anti_cheat(&g.path) { "Anti-cheat detectado" } else { "Nenhum anti-cheat detectado" };
        ui.c.text(rect(r.x + 52.0, top + 28.0, room, 18.0), note, 12.0, MUTED, Align::Left, Weight::Regular);
        let row = |x: f32, w: f32| rect(x, top + 8.0, w, 36.0);
        let mut s = g.view as usize;
        if ui.select(&format!("mt-v{i}"), row(view_x, 96.0), &opts(&["Compacto", "Completo", "Nenhum"]), &mut s) {
            g.view = s as u8;
            changed = true;
        }
        let mut s = g.limit_mode as usize;
        if ui.select(&format!("mt-l{i}"), row(limit_x, 110.0), &opts(&["Desligado", "Automático", "Manual"]), &mut s) {
            g.limit_mode = s as u8;
            if s == 1 {
                g.limit_fps = cap;
                g.measured = false;
            }
            limit = Some(i);
            changed = true;
        }
        if g.limit_mode == 2 {
            if ui.stepper(&format!("mt-f{i}"), row(fps_x, 102.0), &mut g.limit_fps, (20, 1023), 1, "") {
                limit = Some(i);
                changed = true;
            }
        } else if g.limit_mode == 1 {
            ui.c.text(row(fps_x, 102.0), &g.limit_fps.to_string(), 13.0, TEXT, Align::Left, Weight::Regular);
        }
        let mut s = g.limit_real as usize;
        if ui.select(&format!("mt-u{i}"), row(unit_x, 106.0), &opts(&["Com geração", "FPS real"]), &mut s) {
            g.limit_real = s == 1;
            limit = Some(i);
            changed = true;
        }
        let mut s = g.hdr as usize;
        if ui.select(&format!("mt-h{i}"), row(hdr_x, 86.0), &opts(&["Padrão", "Ligar", "Não ligar"]), &mut s) {
            g.hdr = s as u8;
            changed = true;
        }
        let mut s = g.preset as usize;
        if ui.select(&format!("mt-p{i}"), row(preset_x, 92.0), &opts(&["Do jogo", "Recomendado", "K", "L", "M"]), &mut s) {
            g.preset = s as u8;
            preset = Some(i);
            changed = true;
        }
        let b = rect(r.right() - 38.0, top + 14.0, 24.0, 24.0);
        if ui.is_hover(b) {
            ui.c.round(b, 6.0, BORDER);
        }
        ui.c.icon(b.x + 4.0, b.y + 4.0, 16.0, &[&[(6., 6.), (18., 18.)], &[(18., 6.), (6., 18.)]], MUTED);
        if ui.hit(&format!("mt-x{i}"), b) {
            remove = Some(i);
        }
    }
    if let Some(i) = limit {
        let g = app.cfg.meter_games[i].clone();
        if crate::meter::write_limit(&g) {
            app.toast.info("Limite gravado. Vale na próxima abertura do jogo.");
        } else {
            app.toast.info("Não consegui gravar no driver NVIDIA.");
        }
    }
    if let Some(i) = preset {
        let g = app.cfg.meter_games[i].clone();
        if !crate::meter::write_preset(&g) {
            app.toast.info("Não consegui gravar no driver NVIDIA.");
        }
    }
    if let Some(i) = remove {
        let g = app.cfg.meter_games.remove(i);
        crate::sys::drs::forget(&g.exe);
        changed = true;
    }
    if changed {
        app.cfg.save();
    }
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header(ui, a, "Medidor", "Leitor de performance, limitador de FPS e HDR automático por jogo.");
    defaults_button(ui, app, a.right() - 124.0, Defaults::Meter, "Volta atalhos, opacidade, itens e regras ao padrão. A lista de jogos continua salva.");
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let top = 88.0;
    let view = rect(x, top, w, a.h - top - 20.0);
    if app.pick_for == Some(Target::MeterGame) {
        // A lista tem rolagem própria e não pode ficar dentro da rolagem da página.
        ui.card(view);
        picker::draw(ui, app, view);
        return;
    }
    let (h1, h2) = (200.0, 212.0);
    let h3 = 88.0 + app.cfg.meter_games.len().max(1) as f32 * ROW;
    ui.begin_scroll("meter", view, h1 + h2 + h3 + 24.0);
    // Listas por último, para abrirem por cima do resto.
    games(ui, app, rect(x, top + h1 + h2 + 24.0, w, h3));
    items(ui, app, rect(x, top + h1 + 12.0, w, h2));
    general(ui, app, rect(x, top, w, h1));
    ui.end_scroll();
}
