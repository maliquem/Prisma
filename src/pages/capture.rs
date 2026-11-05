use super::*;
use crate::app::{native_fps, ACT_RECORD, ACT_RECORD_FULL, ACT_SHOT};
use crate::capture;
use crate::capture::engine::bitrate;
use crate::capture::mux::Codec;

const GAP: f32 = 12.0;

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header(ui, a, "Captura", "Tudo fica no seu PC. Nada é enviado.");
    if ui.button("shot", rect(a.right() - 428.0, 24.0, 112.0, 44.0), "Tirar foto", false) {
        app.post_action(ACT_SHOT);
    }
    defaults_button(ui, app, a.right() - 532.0, crate::config::Defaults::Capture, "Volta atalhos, pasta, formatos, som e replay ao padrão.");
    if app.is_recording() {
        if ui.button("rec", rect(a.right() - 308.0, 24.0, 280.0, 44.0), "Parar", true) {
            app.post_action(ACT_RECORD);
        }
    } else {
        if ui.button("rec", rect(a.right() - 308.0, 24.0, 144.0, 44.0), "Gravar Janela", true) {
            app.post_action(ACT_RECORD);
        }
        if ui.button("rec-full", rect(a.right() - 156.0, 24.0, 128.0, 44.0), "Gravar Tela", true) {
            app.post_action(ACT_RECORD_FULL);
        }
    }

    let mon = capture::cursor_monitor();
    let fps = native_fps(mon);
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let top = 88.0;
    let rw = 300.0;
    let lw = w - rw - GAP;
    let h = a.h - top - 20.0;

    let l = rect(x, top, lw, h);
    ui.card(l);
    ui.title(rect(l.x + 16.0, l.y + 12.0, 200.0, 22.0), "Vídeo");
    let fw = (l.w - 44.0) / 2.0;
    let col = |i: usize| l.x + 16.0 + i as f32 * (fw + 12.0);
    let y0 = l.y + 42.0;
    let y1 = y0 + 76.0;
    ui.label(rect(col(0), y0, fw, 18.0), "O replay grava", TEXT2);
    ui.label(rect(col(1), y0, fw, 18.0), "Resolução e FPS", TEXT2);
    ui.label(rect(col(0), y1, fw, 18.0), "Cor com HDR ligado na tela", TEXT2);
    ui.label(rect(col(1), y1, fw, 18.0), "Codec", TEXT2);

    let mut y = y1 + 76.0;
    let mut changed = false;
    let c = &mut app.cfg;
    changed |= ui.check("c-sys", rect(col(0), y, 170.0, 44.0), "Som do sistema", &mut c.cap_sys_audio);
    changed |= ui.check("c-mic", rect(col(0) + 180.0, y, 130.0, 44.0), "Microfone", &mut c.cap_mic);
    changed |= ui.check("c-cur", rect(col(0) + 320.0, y, 170.0, 44.0), "Mostrar o cursor", &mut c.cap_cursor);
    y += 44.0;
    let sound = if c.cap_sys_audio {
        "Som do sistema ligado: grava todos os sons do PC."
    } else {
        "Som do sistema desligado: Gravar Janela grava só o som da janela, e Gravar Tela não grava o som do PC."
    };
    ui.c.text_wrap(rect(col(0), y, l.w - 32.0, 22.0), sound, 13.0, TEXT2, Weight::Regular);
    y += 30.0;

    let (mw, mh, mf) = match c.cap_mode {
        1 => (3840, 2160, 60),
        2 => (2560, 1440, 60),
        3 => (1920, 1080, 60),
        _ => (3840, 2160, fps),
    };
    let codec = match c.cap_codec {
        1 => Codec::Hevc,
        2 => Codec::H264,
        _ => Codec::Av1,
    };
    let mb = bitrate(mw, mh, mf, codec) as u64 * 60 / 8 / 1_000_000;
    let e = rect(col(0), y, l.w - 32.0, 44.0);
    ui.c.round(e, 10.0, CARD2);
    ui.c.text(e.inset_xy(14.0, 0.0), &format!("Cerca de {mb} MB por minuto"), 14.0, TEXT, Align::Left, Weight::Regular);
    ui.c.text(e.inset_xy(14.0, 0.0), "Estimativa, varia com a cena", 13.0, TEXT2, Align::Right, Weight::Regular);
    y += 60.0;

    ui.title(rect(col(0), y, 300.0, 22.0), "Replay");
    y += 26.0;
    let txt = "Em modo jogo, guarda os últimos segundos na memória. O atalho salva sem interromper.";
    ui.c.text_wrap(rect(col(0), y, l.w - 32.0, 40.0), txt, 13.0, MUTED, Weight::Regular);
    y += 42.0;
    let mut on = c.replay_on;
    let replay_toggled = ui.check("c-rep", rect(col(0), y, 180.0, 44.0), "Ligar replay", &mut on);
    if replay_toggled {
        c.replay_on = on;
        changed = true;
    }
    let ry = y;

    let r = rect(l.right() + GAP, top, rw, h);
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Atalhos e pasta");
    let items: [(&str, &str, u8); 6] = [
        ("hk1", "Foto da área", 0),
        ("hk2", "Foto da tela inteira", 1),
        ("hk5", "Foto da janela em foco", 4),
        ("hk3", "Gravar Janela e parar", 2),
        ("hk6", "Gravar Tela e parar", 5),
        ("hk4", "Salvar replay", 3),
    ];
    const STEP: f32 = 66.0;
    let mut hk_changed = false;
    for (i, (key, name, which)) in items.iter().enumerate() {
        let y = r.y + 40.0 + i as f32 * STEP;
        ui.label(rect(r.x + 16.0, y, r.w - 32.0, 18.0), name, TEXT2);
        let hk = match which {
            0 => &mut app.cfg.hk_shot,
            1 => &mut app.cfg.hk_shot_full,
            2 => &mut app.cfg.hk_record,
            4 => &mut app.cfg.hk_shot_win,
            5 => &mut app.cfg.hk_record_full,
            _ => &mut app.cfg.hk_replay,
        };
        hk_changed |= ui.hotkey(key, rect(r.x + 16.0, y + 20.0, r.w - 32.0, 40.0), hk);
    }
    let py = r.y + 40.0 + 6.0 * STEP;
    ui.label(rect(r.x + 16.0, py, r.w - 32.0, 18.0), "Pasta", TEXT2);
    let pr = rect(r.x + 16.0, py + 20.0, r.w - 32.0, 40.0);
    ui.c.round(pr, 8.0, SIDE);
    ui.c.round_line(pr, 8.0, if ui.is_hover(pr) { BORDER2 } else { BORDER }, 1.0);
    ui.c.text(pr.inset_xy(10.0, 0.0), &app.cfg.cap_dir, 14.0, TEXT, Align::Left, Weight::Regular);
    if ui.hit("dir", pr) {
        app.pick_folder();
    }
    let fy = py + STEP;
    ui.label(rect(r.x + 16.0, fy, r.w - 32.0, 18.0), "Formato de foto", TEXT2);

    // Listas por último, para abrirem por cima do resto.
    let opt = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let c = &mut app.cfg;
    let mut s = c.shot_hdr as usize;
    if ui.select("fmt", rect(r.x + 16.0, fy + 20.0, r.w - 32.0, 40.0), &opt(&["PNG em SDR", "PNG e JPEG XR em HDR"]), &mut s) {
        c.shot_hdr = s == 1;
        changed = true;
    }
    let secs = [15u32, 30, 60, 120];
    let mut s = secs.iter().position(|v| *v == c.replay_secs).unwrap_or(1);
    if ui.select("secs", rect(col(0) + 190.0, ry, 180.0, 44.0), &opt(&["15 segundos", "30 segundos", "1 minuto", "2 minutos"]), &mut s) {
        c.replay_secs = secs[s];
        changed = true;
    }
    let mut s = c.cap_hdr as usize;
    if ui.select("hdr", rect(col(0), y1 + 22.0, fw, 44.0), &opt(&["Gravar em SDR", "Gravar em HDR10"]), &mut s) {
        c.cap_hdr = s == 1;
        changed = true;
    }
    let mut s = c.cap_codec as usize;
    if ui.select("cod", rect(col(1), y1 + 22.0, fw, 44.0), &opt(&["AV1 pela GPU", "HEVC pela GPU", "H.264 (mais compatível)"]), &mut s) {
        c.cap_codec = s as u8;
        changed = true;
    }
    let mut s = c.cap_target as usize;
    if ui.select("tgt", rect(col(0), y0 + 22.0, fw, 44.0), &opt(&["Monitor em foco", "Janela em foco"]), &mut s) {
        c.cap_target = s as u8;
        changed = true;
    }
    let native = format!("Nativo: {fps} FPS");
    let mut s = c.cap_mode as usize;
    let modes = [native.as_str(), "4K a 60 FPS", "1440p a 60 FPS", "1080p a 60 FPS"];
    if ui.select("mode", rect(col(1), y0 + 22.0, fw, 44.0), &opt(&modes), &mut s) {
        c.cap_mode = s as u8;
        changed = true;
    }
    if changed || hk_changed {
        app.cfg.save();
    }
    if hk_changed {
        app.register_hotkeys();
    }
    if replay_toggled {
        app.replay_changed();
    }
}
