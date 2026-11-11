use super::picker::{self, Target};
use super::*;
use crate::config::Defaults;
use crate::meter;

const ROW: f32 = 56.0;
const ROW_NARROW: f32 = 112.0;
/// Largura dos controles de cada jogo e o mínimo que sobra para o nome ao lado deles.
const CONTROLS_W: f32 = 760.0;
const NAME_MIN: f32 = 250.0;
/// Minutos do registro sendo digitados.
static LOG_FIELD: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

const ITEMS: [(&str, u32); 16] = [
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
    ("Registro", meter::ITEM_LOG),
    ("Plano de energia", meter::ITEM_POWER),
];

fn opts(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn general(ui: &mut Ui, app: &mut App, r: Rect) {
    ui.card(r);
    let (x, w) = (r.x + 16.0, (r.w - 56.0) / 3.0);
    let mut changed = false;
    ui.label(rect(x, r.y + 12.0, w, 18.0), "HDR automático", TEXT2);
    let hdr_r = rect(x, r.y + 34.0, w, 40.0);
    ui.c.text_wrap(
        rect(x, r.y + 82.0, w, 104.0),
        "Liga o HDR do Windows quando o jogo abre e desliga quando ele fecha. Se o HDR já estava ligado, ele continua ligado. Em Todos, vale para todo jogo que o modo jogo do Energia reconhece; Não ligar, na lista, tira um jogo. O medidor só aparece pelo atalho.",
        12.0,
        MUTED,
        Weight::Regular,
    );

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
    ui.label(rect(x, r.y + 196.0, w, 18.0), "Posição do compacto", TEXT2);
    let mut s = app.cfg.meter_pos_compact as usize;
    if ui.select("mt-pc", rect(x, r.y + 218.0, w, 40.0), &opts(&["Superior esquerdo", "Superior centro", "Superior direito"]), &mut s) {
        app.cfg.meter_pos_compact = s as u8;
        changed = true;
    }
    ui.label(rect(kx, r.y + 196.0, w, 18.0), "Posição do completo", TEXT2);
    let mut s = app.cfg.meter_pos_full as usize;
    if ui.select("mt-pf", rect(kx, r.y + 218.0, w, 40.0), &opts(&["Lado direito", "Lado esquerdo"]), &mut s) {
        app.cfg.meter_pos_full = s as u8;
        changed = true;
    }
    // Lista por último, para abrir por cima do resto.
    let mut s = app.cfg.meter_hdr as usize;
    if ui.select("mt-hdr", hdr_r, &opts(&["Desligado", "Só na lista", "Todos"]), &mut s) {
        app.cfg.meter_hdr = s as u8;
        changed = true;
    }
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

fn limiter(ui: &mut Ui, app: &mut App, r: Rect) {
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Limitador durante o jogo");
    ui.label(rect(r.x + 16.0, r.y + 36.0, r.w - 32.0, 18.0), "Com o jogo aberto, estes atalhos mudam o limite dele, e o medidor completo mostra o resultado.", MUTED);
    let (x, w) = (r.x + 16.0, (r.w - 68.0) / 4.0);
    let keys: [(&str, &str, u8); 4] = [("mt-k8", "Controles no medidor", 3), ("mt-k4", "Ligar ou desligar", 0), ("mt-k5", "Aumentar o limite", 1), ("mt-k6", "Diminuir o limite", 2)];
    let mut hk = false;
    for (i, (key, name, which)) in keys.iter().enumerate() {
        let kx = x + i as f32 * (w + 12.0);
        ui.label(rect(kx, r.y + 62.0, w, 18.0), name, TEXT2);
        let target = match which {
            0 => &mut app.cfg.hk_limit_toggle,
            1 => &mut app.cfg.hk_limit_up,
            2 => &mut app.cfg.hk_limit_down,
            _ => &mut app.cfg.hk_meter_keys,
        };
        hk |= ui.hotkey(key, rect(kx, r.y + 82.0, w, 36.0), target);
    }
    ui.label(rect(r.x + 16.0, r.y + 126.0, r.w - 32.0, 18.0), "A injeção muda o limite na hora. Se falhar ou houver anti-cheat, ele vai para o driver e vale ao reabrir o jogo.", MUTED);
    if hk {
        app.cfg.save();
        app.register_hotkeys();
    }
}

fn log(ui: &mut Ui, app: &mut App, r: Rect) {
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Registro de dados");
    ui.label(rect(r.x + 16.0, r.y + 36.0, r.w - 32.0, 18.0), "Grava cada quadro, cada segundo e a CPU de cada thread do jogo, para estudar desempenho. Só roda se você ligar.", MUTED);
    let (x, w) = (r.x + 16.0, (r.w - 56.0) / 3.0);
    let mut changed = false;
    ui.label(rect(x, r.y + 62.0, w, 18.0), "Minutos de jogo gravados (0 é sem limite)", TEXT2);
    {
        let mut guard = LOG_FIELD.lock().unwrap();
        let field = guard.get_or_insert_with(|| app.cfg.meter_log_min.to_string());
        field.retain(|c| c.is_ascii_digit());
        field.truncate(4);
        ui.search("mt-ltime", rect(x, r.y + 82.0, w, 36.0), field, "5");
        let min = if field.is_empty() { 5 } else { field.parse().unwrap_or(5) };
        if min != app.cfg.meter_log_min {
            app.cfg.meter_log_min = min;
            changed = true;
        }
    }
    let kx = x + w + 12.0;
    ui.label(rect(kx, r.y + 62.0, w, 18.0), "Ligar e desligar", TEXT2);
    let hk = ui.hotkey("mt-k7", rect(kx, r.y + 82.0, w, 36.0), &mut app.cfg.hk_log);
    let bx = kx + w + 12.0;
    let bw = (w - 8.0) / 2.0;
    let recording = app.meter.log.is_some();
    let game = !app.meter.session.exe.is_empty();
    if ui.button_ex("mt-lgo", rect(bx, r.y + 82.0, bw, 36.0), if recording { "Parar" } else { "Iniciar" }, true, game || recording) {
        app.meter_log();
    }
    if ui.button("mt-lopen", rect(bx + bw + 8.0, r.y + 82.0, bw, 36.0), "Abrir pasta", false) {
        let dir = crate::sys::gamelog::root();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::process::Command::new(crate::sys::system_tool("explorer")).arg(&dir).spawn();
    }
    changed |= ui.check("mt-lauto", rect(x, r.y + 126.0, w * 2.0, 32.0), "Começar sozinho quando um jogo abrir", &mut app.cfg.meter_log_auto);
    let status = if recording { "Gravando" } else if game { "Pronto para gravar" } else { "Abra um jogo para iniciar" };
    ui.c.text(rect(bx, r.y + 130.0, w, 22.0), status, 12.0, if recording { ACCENT } else { MUTED }, Align::Left, Weight::Regular);
    if changed || hk {
        app.cfg.save();
    }
    if hk {
        app.register_hotkeys();
    }
}

/// Em janela estreita, o nome vai para cima dos controles.
fn is_narrow(w: f32) -> bool {
    w - CONTROLS_W < NAME_MIN
}

fn games(ui: &mut Ui, app: &mut App, r: Rect) {
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 150.0, 22.0), "Jogos");
    picker::add_button(ui, app, rect(r.right() - 124.0, r.y + 8.0, 108.0, 32.0), Target::MeterGame);
    let (view_x, limit_x, fps_x, how_x, unit_x, hdr_x, preset_x) = (r.right() - CONTROLS_W, r.right() - 660.0, r.right() - 546.0, r.right() - 440.0, r.right() - 336.0, r.right() - 226.0, r.right() - 136.0);
    let y = r.y + 46.0;
    let narrow = is_narrow(r.w);
    let row_h = if narrow { ROW_NARROW } else { ROW };
    let heads = |ui: &mut Ui, y: f32| {
        for (x, name) in [(view_x, "Auto HDR"), (limit_x, "Limitador"), (fps_x, "FPS"), (how_x, "Método"), (unit_x, "Limitando"), (hdr_x, "HDR"), (preset_x, "Perfil DLSS")] {
            ui.c.text(rect(x, y, 104.0, 18.0), name, 12.0, MUTED, Align::Left, Weight::Regular);
        }
    };
    if !narrow {
        heads(ui, y);
    }
    let cap = crate::meter::first_limit(app.refresh_hz());
    let (mut remove, mut limit, mut preset, mut changed, mut blocked, mut auto_hdr) = (None, None, None, false, false, None);
    let count = app.cfg.meter_games.len();
    if count == 0 {
        ui.c.text(rect(r.x + 16.0, y + 24.0, r.w - 32.0, 22.0), "Nenhum jogo ainda. Use Adicionar.", 13.0, MUTED, Align::Left, Weight::Regular);
    }
    for i in 0..count {
        let top = y + 24.0 + i as f32 * row_h;
        let g = &mut app.cfg.meter_games[i];
        ui.file_icon(&g.path, &g.exe, rect(r.x + 16.0, top + 13.0, 26.0, 26.0));
        let room = if narrow { r.w - 68.0 } else { view_x - r.x - 64.0 };
        ui.c.text(rect(r.x + 52.0, top + 6.0, room, 22.0), app_name(&g.exe), 14.0, TEXT, Align::Left, Weight::Regular);
        let protected = crate::meter::anti_cheat(&g.path);
        let note = if protected {
            "Anti-cheat detectado"
        } else if g.inject_off {
            "Limite pelo driver: a injeção falhou"
        } else {
            "Nenhum anti-cheat detectado"
        };
        ui.c.text(rect(r.x + 52.0, top + 28.0, room, 18.0), note, 12.0, MUTED, Align::Left, Weight::Regular);
        if narrow {
            heads(ui, top + 50.0);
        }
        let controls = if narrow { top + 68.0 } else { top + 8.0 };
        let row = |x: f32, w: f32| rect(x, controls, w, 36.0);
        // A mesma escolha da aba HDR: mudar aqui muda lá.
        let mut s = app.cfg.hdr_games.iter().find(|h| h.exe.eq_ignore_ascii_case(&g.exe)).map_or(0, |h| h.mode as usize);
        if ui.select(&format!("mt-v{i}"), row(view_x, 96.0), &opts(&["Padrão", "Forçar", "Desligar"]), &mut s) {
            auto_hdr = Some((g.exe.clone(), g.path.clone(), s as u8));
        }
        let g = &mut app.cfg.meter_games[i];
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
        let mut s = crate::meter::uses_injection(g) as usize;
        if ui.select(&format!("mt-m{i}"), row(how_x, 96.0), &opts(&["Driver", "Injeção"]), &mut s) {
            if s == 0 {
                g.inject = false;
                g.inject_off = false;
                limit = Some(i);
                changed = true;
            } else if protected {
                blocked = true;
            } else {
                g.inject = true;
                g.inject_off = false;
                limit = Some(i);
                changed = true;
            }
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
        // O perfil é do DLSS: sem ele no jogo, não há o que escolher.
        match crate::meter::has_dlss(&g.path) {
            Some(true) => {
                let mut s = g.preset as usize;
                if ui.select(&format!("mt-p{i}"), row(preset_x, 92.0), &opts(&["Do jogo", "Recomendado", "K", "L", "M"]), &mut s) {
                    g.preset = s as u8;
                    preset = Some(i);
                    changed = true;
                }
            }
            Some(false) => ui.c.text(row(preset_x, 92.0), "Sem DLSS", 12.0, MUTED, Align::Left, Weight::Regular),
            None => ui.c.text(row(preset_x, 92.0), "…", 12.0, MUTED, Align::Left, Weight::Regular),
        }
        let b = rect(r.right() - 38.0, controls + 6.0, 24.0, 24.0);
        if ui.is_hover(b) {
            ui.c.round(b, 6.0, BORDER);
        }
        ui.c.icon(b.x + 4.0, b.y + 4.0, 16.0, &[&[(6., 6.), (18., 18.)], &[(18., 6.), (6., 18.)]], MUTED);
        if ui.hit(&format!("mt-x{i}"), b) {
            remove = Some(i);
        }
    }
    if blocked {
        app.toast.info("Injeção bloqueada: anti-cheat detectado.");
    }
    if let Some((exe, path, mode)) = auto_hdr {
        if crate::sys::autohdr::apply(&exe, mode) {
            match app.cfg.hdr_games.iter_mut().find(|h| h.exe.eq_ignore_ascii_case(&exe)) {
                Some(h) => h.mode = mode,
                None => app.cfg.hdr_games.push(crate::config::HdrGame { exe, path, mode }),
            }
            changed = true;
        } else {
            app.toast.info("Não consegui gravar a escolha.");
        }
    }
    if let Some(i) = limit {
        let g = app.cfg.meter_games[i].clone();
        if !crate::meter::write_limit(&g) {
            app.toast.info("Não consegui gravar no driver NVIDIA.");
        } else if crate::meter::uses_injection(&g) {
            app.toast.info("Com injeção, o limite muda na hora.");
        } else {
            app.toast.info("Limite gravado. Vale na próxima abertura do jogo.");
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
    let (h1, h2, hl, hr) = (272.0, 212.0, 154.0, 172.0);
    let h3 = 88.0 + app.cfg.meter_games.len().max(1) as f32 * if is_narrow(w) { ROW_NARROW } else { ROW };
    ui.begin_scroll("meter", view, h1 + h2 + hl + hr + h3 + 48.0);
    // Listas por último, para abrirem por cima do resto.
    games(ui, app, rect(x, top + h1 + h2 + hl + hr + 48.0, w, h3));
    limiter(ui, app, rect(x, top + h1 + h2 + 24.0, w, hl));
    log(ui, app, rect(x, top + h1 + h2 + hl + 36.0, w, hr));
    items(ui, app, rect(x, top + h1 + 12.0, w, h2));
    general(ui, app, rect(x, top, w, h1));
    ui.end_scroll();
}
