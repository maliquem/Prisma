//! Cartão "Esconder do jogo" da aba Controles (Fase 8): o HidHide esconde o controle físico
//! enquanto a ponte estiver ligada. Sem o driver, explica e oferece instalar da fonte oficial.

use super::*;
use crate::sys::hidhide;
use crate::sys::pads::Pad;

pub fn card(ui: &mut Ui, app: &mut App, list: &[Pad], at: Rect) -> f32 {
    if let Some(m) = hidhide::take_message() {
        app.toast.info(m);
    }
    let busy = hidhide::busy();
    if !hidhide::installed() {
        let note = crate::i18n::tr("O HidHide é um driver gratuito da Nefarius que esconde o controle físico do jogo, para ele ver só o controle da ponte.");
        let nh = crate::ui::gfx::measure(&note, 12.0, Weight::Regular, Some(at.w - 32.0)).1.ceil();
        let r = rect(at.x, at.y, at.w, 98.0 + nh);
        ui.card(r);
        ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Esconder do jogo");
        ui.c.text_wrap(rect(r.x + 16.0, r.y + 40.0, r.w - 32.0, nh), &note, 12.0, TEXT2, Weight::Regular);
        let label = if busy { "Instalando…" } else { "Instalar o HidHide" };
        if ui.button_ex("pad-hh-install", rect(r.x + 16.0, r.y + 50.0 + nh, r.w - 32.0, 34.0), label, false, !busy) {
            ui.ask("pad-hh-install", "Instalar o HidHide?", "O Prisma baixa o instalador oficial do GitHub da Nefarius, confere a assinatura e abre o instalador. O Windows pede permissão de administrador e pode pedir para reiniciar. Na desinstalação do Prisma, o HidHide sai junto.");
        }
        if ui.answered("pad-hh-install") {
            hidhide::background(|| Some(match hidhide::install() {
                Ok(false) => "HidHide instalado",
                Ok(true) => "HidHide instalado. O Windows pede para reiniciar o PC para terminar.",
                Err(e) => e,
            }));
        }
        return r.bottom() + 12.0;
    }
    // Só controles lidos pelo HID: o Xbox lido pelo XInput fica de fora.
    let pads: Vec<&Pad> = list.iter().filter(|p| !p.key.starts_with("xinput:")).collect();
    let lg = hidhide::ledger();
    let note = crate::i18n::tr("Enquanto a ponte estiver ligada, o jogo não vê os controles marcados. O Prisma continua lendo. Sem a ponte, eles voltam a aparecer.");
    let nh = crate::ui::gfx::measure(&note, 12.0, Weight::Regular, Some(at.w - 32.0)).1.ceil();
    let rows = pads.len().max(1) as f32;
    let r = rect(at.x, at.y, at.w, 98.0 + nh + rows * 42.0);
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Esconder do jogo");
    ui.c.text_wrap(rect(r.x + 16.0, r.y + 40.0, r.w - 32.0, nh), &note, 12.0, TEXT2, Weight::Regular);
    let mut y = r.y + 48.0 + nh;
    if pads.is_empty() {
        ui.c.text_fit(rect(r.x + 16.0, y, r.w - 32.0, 34.0), "Nenhum controle que dê para esconder", 12.0, MUTED, Align::Left, Weight::Regular);
    }
    let bw = 150.0_f32.min((r.w - 32.0) / 2.0);
    for p in pads {
        let hidden = !lg.devs.is_empty() && hidhide::hidden(&p.key);
        let cw = if hidden { r.w - 28.0 - bw - 8.0 } else { r.w - 20.0 };
        let mut on = app.cfg.pad_hide.contains(&p.key);
        if ui.check(&format!("pad-hh-{}", p.key), rect(r.x + 10.0, y, cw, 34.0), &crate::i18n::tr(&p.name), &mut on) && !busy {
            app.cfg.pad_hide.retain(|k| *k != p.key);
            if on {
                app.cfg.pad_hide.push(p.key.clone());
            }
            app.cfg.save();
            let key = p.key.clone();
            // Ligar só esconde com a ponte ligada para este controle; desligar mostra na hora.
            hidhide::background(move || {
                let ok = if on { !hidhide::bridged(&key) || hidhide::set_hidden(&key, true) } else { hidhide::set_hidden(&key, false) };
                (!ok).then_some("O HidHide não aceitou a mudança")
            });
        }
        // Sempre à vista enquanto o controle estiver escondido.
        if hidden && ui.button_ex(&format!("pad-hh-show-{}", p.key), rect(r.right() - 16.0 - bw, y, bw, 34.0), "Mostrar o controle", false, !busy) {
            app.cfg.pad_hide.retain(|k| *k != p.key);
            app.cfg.save();
            let key = p.key.clone();
            hidhide::background(move || (!hidhide::set_hidden(&key, false)).then_some("O HidHide não aceitou a mudança"));
        }
        y += 42.0;
    }
    if ui.button_ex("pad-hh-undo", rect(r.x + 16.0, y + 8.0, r.w - 32.0, 34.0), "Desfazer tudo", false, !lg.clean() && !busy) {
        app.cfg.pad_hide.clear();
        app.cfg.save();
        hidhide::background(|| Some(if hidhide::limpar() { "As listas do HidHide voltaram a ser como antes" } else { "O HidHide não aceitou a mudança" }));
    }
    ui.tip(rect(r.x + 16.0, y + 8.0, r.w - 32.0, 34.0), "Tira das listas do HidHide só o que o Prisma acrescentou. O que já estava lá fica.");
    r.bottom() + 12.0
}
