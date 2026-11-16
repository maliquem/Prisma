//! Cartão "Esconder do jogo" da aba Controles (Fase 8): o HidHide esconde o controle físico
//! enquanto a ponte estiver ligada. O cartão Drivers instala, atualiza e remove o HidHide e o
//! ViGEmBus da fonte oficial e avisa quando o filtro do HidHide falta no Windows.

use super::*;
use crate::sys::hidhide;
use crate::sys::nefarius::{self, Kit};
use crate::sys::pads::Pad;

pub fn card(ui: &mut Ui, app: &mut App, list: &[Pad], at: Rect) -> f32 {
    if let Some(m) = hidhide::take_message() {
        app.toast.info(m);
    }
    let busy = hidhide::busy();
    if !hidhide::installed() {
        let note = crate::i18n::tr("O HidHide é um driver gratuito da Nefarius que esconde o controle físico do jogo, para ele ver só o controle da ponte. Ele se instala no cartão Drivers, logo abaixo.");
        let nh = crate::ui::gfx::measure(&note, 12.0, Weight::Regular, Some(at.w - 32.0)).1.ceil();
        let r = rect(at.x, at.y, at.w, 56.0 + nh);
        ui.card(r);
        ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Esconder do jogo");
        ui.c.text_wrap(rect(r.x + 16.0, r.y + 40.0, r.w - 32.0, nh), &note, 12.0, TEXT2, Weight::Regular);
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

/// Cartão "Drivers": versão instalada e a mais nova no GitHub de cada driver, com instalar,
/// atualizar e remover. Tudo roda fora da janela, um de cada vez, pela fila do HidHide.
pub fn drivers(ui: &mut Ui, app: &mut App, at: Rect) -> f32 {
    let busy = hidhide::busy();
    let w = at.w - 32.0;
    let bw = 120.0_f32.min((w - 8.0) / 2.0);
    let kits: [(&'static Kit, &str); 2] = [
        (&nefarius::HIDHIDE, "Esconde o controle físico do jogo."),
        (&nefarius::VIGEMBUS, "Cria o controle de Xbox virtual da ponte."),
    ];
    let warn = nefarius::filter_missing();
    let warn_text = crate::i18n::tr("O HidHide está instalado, mas o filtro dele não está ativo no Windows, então esconder o controle não tem efeito. Reinstalar a versão mais nova costuma resolver; o Windows pode pedir para reiniciar.");
    // Cada driver: instalado, versão instalada, a do GitHub e o texto já medido.
    let rows: Vec<(bool, String, Option<Option<String>>, String, f32)> = kits.iter().map(|(k, what)| {
        let installed = k.installed();
        let mine = k.version().unwrap_or_default();
        let latest = nefarius::latest(k);
        let gh = match &latest {
            None => crate::i18n::tr("procurando…").into_owned(),
            Some(None) => crate::i18n::tr("não deu para ver").into_owned(),
            Some(Some(v)) => v.clone(),
        };
        let state = match (installed, mine.is_empty()) {
            (true, false) => crate::i18n::tr(&format!("Instalado: {mine}")).into_owned(),
            (true, true) => crate::i18n::tr("Instalado").into_owned(),
            (false, _) => crate::i18n::tr("Não instalado").into_owned(),
        };
        let t = format!("{}\n{}\n{}\n{}", k.name, state, crate::i18n::tr(&format!("No GitHub: {gh}")), crate::i18n::tr(what));
        let h = crate::ui::gfx::measure(&t, 12.0, Weight::Regular, Some(w - bw - 8.0)).1.ceil().max(76.0);
        (installed, mine, latest, t, h)
    }).collect();
    let wh = if warn { crate::ui::gfx::measure(&warn_text, 12.0, Weight::Regular, Some(w)).1.ceil() + 54.0 } else { 0.0 };
    let r = rect(at.x, at.y, at.w, 52.0 + rows.iter().map(|row| row.4 + 12.0).sum::<f32>() + wh);
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, w, 22.0), "Drivers");
    let mut y = r.y + 44.0;
    for ((k, _), (installed, mine, latest, t, h)) in kits.iter().zip(rows) {
        let k: &'static Kit = k;
        ui.c.text_wrap(rect(r.x + 16.0, y, w - bw - 8.0, h), &t, 12.0, TEXT2, Weight::Regular);
        let bx = r.right() - 16.0 - bw;
        let id = k.name.to_ascii_lowercase();
        let update = matches!(&latest, Some(Some(v)) if !mine.is_empty() && nefarius::newer(v, &mine));
        let action = if !installed { Some("Instalar") } else if update { Some("Atualizar") } else { None };
        if let Some(a) = action {
            if ui.button_ex(&format!("drv-inst-{id}"), rect(bx, y, bw, 34.0), a, true, !busy) {
                let body = if k.name == nefarius::HIDHIDE.name {
                    "O Prisma baixa o instalador oficial do GitHub da Nefarius, confere a assinatura e abre o instalador. O Windows pede permissão de administrador e pode pedir para reiniciar. Se foi o Prisma que instalou o HidHide, ele sai junto na desinstalação do Prisma."
                } else {
                    "O Prisma baixa o instalador oficial do GitHub da Nefarius, confere a assinatura e abre o instalador. O Windows pede permissão de administrador e pode pedir para reiniciar."
                };
                let title = if installed { format!("Atualizar o {}?", k.name) } else { format!("Instalar o {}?", k.name) };
                ui.ask(&format!("drv-inst-{id}"), &title, body);
            }
        }
        if ui.answered(&format!("drv-inst-{id}")) {
            run_install(k);
        }
        if installed && ui.button_ex(&format!("drv-rm-{id}"), rect(bx, y + 42.0, bw, 34.0), "Desinstalar", false, !busy) {
            ui.ask(&format!("drv-rm-{id}"), &format!("Desinstalar o {}?", k.name), "O Prisma abre o desinstalador oficial do driver. O Windows pede permissão de administrador e pode pedir para reiniciar. Sem o HidHide, o jogo volta a ver os controles físicos; sem o ViGEmBus, a ponte e os controles de quem assiste param de funcionar.");
        }
        if ui.answered(&format!("drv-rm-{id}")) {
            let hh = k.name == nefarius::HIDHIDE.name;
            if hh {
                app.cfg.pad_hide.clear();
                app.cfg.save();
            }
            hidhide::background(move || {
                let r = if hh { hidhide::remove() } else { k.uninstall() };
                nefarius::forget();
                Some(match r {
                    Ok(false) => "Driver removido",
                    Ok(true) => "Driver removido. O Windows pede para reiniciar o PC para terminar.",
                    Err(e) => e,
                })
            });
        }
        y += h + 12.0;
    }
    if warn {
        let th = wh - 54.0;
        ui.c.text_wrap(rect(r.x + 16.0, y, w, th), &warn_text, 12.0, WARN, Weight::Regular);
        if ui.button_ex("drv-fix-hidhide", rect(r.x + 16.0, y + th + 8.0, w, 34.0), "Reparar o HidHide", true, !busy) {
            ui.ask("drv-fix-hidhide", "Reparar o HidHide?", "O Prisma baixa a versão mais nova do instalador oficial e abre o instalador, que tira a versão atual e instala de novo. O Windows pede permissão de administrador e pode pedir para reiniciar; depois do reinício, abra o instalador de novo se ele pedir.");
        }
        if ui.answered("drv-fix-hidhide") {
            run_install(&nefarius::HIDHIDE);
        }
    }
    r.bottom() + 12.0
}

fn run_install(k: &'static Kit) {
    let hh = k.name == nefarius::HIDHIDE.name;
    hidhide::background(move || {
        let r = if hh { hidhide::install() } else { k.install() };
        nefarius::forget();
        Some(match r {
            Ok(false) => "Driver instalado",
            Ok(true) => "Driver instalado. O Windows pede para reiniciar o PC para terminar.",
            Err(e) => e,
        })
    });
}
