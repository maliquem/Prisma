//! Driver NVIDIA: as opções do perfil global e do perfil de cada jogo, como no NVIDIA Profile Inspector.
//! Só aparece com placa NVIDIA.

use super::picker::{self, Target};
use super::*;
use crate::sys::drs::{self, Profile, Snapshot, Value};
use crate::sys::nvset::{self, Kind, Setting};
use std::path::PathBuf;
use std::sync::Mutex;

const ROW: f32 = 48.0;
const CTL_W: f32 = 320.0;
const DEFAULT_W: f32 = 100.0;

#[derive(Default)]
struct State {
    /// 0 é o global; depois, os jogos na ordem de `games`.
    who: usize,
    tab: usize,
    /// Perfil e ids lidos, com o que o driver disse de cada um.
    snap: Option<(Profile, Vec<u32>, Snapshot)>,
    backups: Vec<PathBuf>,
    restore: Option<PathBuf>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

pub fn available() -> bool {
    drs::driver_version().is_some()
}

/// Jogos da tela: os escolhidos aqui e os do Medidor, sem repetir.
fn games(app: &App) -> Vec<String> {
    let mut v = app.cfg.nv_games.clone();
    for g in &app.cfg.meter_games {
        if !v.iter().any(|x| x.eq_ignore_ascii_case(&g.exe)) {
            v.push(g.exe.clone());
        }
    }
    v
}

pub fn add_game(app: &mut App, exe: &str) {
    if !app.cfg.nv_games.iter().any(|g| g.eq_ignore_ascii_case(exe)) {
        app.cfg.nv_games.push(exe.to_string());
    }
    let who = games(app).iter().position(|g| g.eq_ignore_ascii_case(exe)).map_or(0, |i| i + 1);
    let mut st = STATE.lock().unwrap();
    let st = st.get_or_insert_with(State::default);
    st.who = who;
    st.snap = None;
}

/// Opções que este driver tem e que fazem sentido neste perfil.
fn shown<'a>(who: &'a Profile, group: &'static [Setting]) -> impl Iterator<Item = &'static Setting> + 'a {
    let ver = drs::driver_version().unwrap_or(0);
    group.iter().filter(move |s| s.min_driver <= ver && !(s.global_only && *who != Profile::Global))
}

fn tabs(ui: &mut Ui, x: f32, y: f32, sel: &mut usize) {
    let mut cx = x;
    for (i, (name, _)) in nvset::GROUPS.iter().enumerate() {
        let name = crate::i18n::tr(name);
        let w = crate::ui::gfx::measure(&name, 14.0, Weight::Semibold, None).0 + 28.0;
        let r = rect(cx, y, w, 34.0);
        let on = *sel == i;
        if on {
            ui.c.round(r, 8.0, CARD2);
        }
        if ui.hit(&format!("nv-tab{i}"), r) {
            *sel = i;
        }
        ui.c.text_raw(r, &name, 14.0, if on { TEXT } else { MUTED }, Align::Center, if on { Weight::Semibold } else { Weight::Regular });
        cx += w + 4.0;
    }
}

/// "Padrão (valor)": o que vale sem nada gravado aqui, quando o driver diz.
fn default_label(s: &Setting, v: Option<Value>) -> String {
    let base = match v {
        Some(v) if !v.mine => Some(v.now),
        Some(v) => v.predefined,
        None => None,
    };
    match base {
        Some(b) => format!("Padrão ({})", nvset::value_name(s, b)),
        None => "Padrão".into(),
    }
}

/// Uma linha: nome com a explicação ao passar o mouse e o controle. Devolve a mudança pedida.
fn row(ui: &mut Ui, s: &Setting, v: Option<Value>, r: Rect) -> Option<Option<u32>> {
    let mine = v.is_some_and(|v| v.mine);
    if mine {
        ui.c.round(rect(r.x + 14.0, r.y + r.h / 2.0 - 4.0, 8.0, 8.0), 4.0, ACCENT);
    }
    let name = rect(r.x + 30.0, r.y, r.w - CTL_W - 56.0, r.h);
    ui.c.text_fit(name, s.name, 14.0, TEXT, Align::Left, Weight::Regular);
    ui.tip(name, s.tip);
    let ctl = rect(r.right() - CTL_W - 14.0, r.y + 6.0, CTL_W, r.h - 12.0);
    let key = format!("nv-{:08x}", s.id);
    match &s.kind {
        Kind::List(list) => {
            let mut opts = vec![default_label(s, v)];
            opts.extend(list.iter().map(|x| x.1.to_string()));
            let mut sel = 0;
            if let Some(v) = v.filter(|v| v.mine) {
                match list.iter().position(|x| x.0 == v.now) {
                    Some(i) => sel = i + 1,
                    None => {
                        // Valor que não está na lista, gravado por outro programa: aparece como está.
                        opts.push(nvset::value_name(s, v.now));
                        sel = opts.len() - 1;
                    }
                }
            }
            let before = sel;
            if ui.select(&key, ctl, &opts, &mut sel) && sel != before {
                return match sel {
                    0 => Some(None),
                    i if i <= list.len() => Some(Some(list[i - 1].0)),
                    _ => None,
                };
            }
        }
        Kind::Number { min, max, step, unit, start } => {
            let side = rect(ctl.right() - DEFAULT_W, ctl.y, DEFAULT_W, ctl.h);
            if let Some(v) = v.filter(|v| v.mine) {
                let mut n = v.now.clamp(*min, *max);
                if ui.stepper(&key, rect(ctl.x, ctl.y, ctl.w - DEFAULT_W - 8.0, ctl.h), &mut n, (*min, *max), *step, unit) {
                    return Some(Some(n));
                }
                if ui.button(&format!("{key}-d"), side, "Padrão", false) {
                    return Some(None);
                }
            } else {
                ui.c.text_fit(rect(ctl.x + 10.0, ctl.y, ctl.w - DEFAULT_W - 18.0, ctl.h), &default_label(s, v), 14.0, MUTED, Align::Left, Weight::Regular);
                if ui.button(&format!("{key}-m"), side, "Mudar", false) {
                    let from = v.map(|v| v.now).filter(|n| (*min..=*max).contains(n)).unwrap_or(*start);
                    return Some(Some(from));
                }
            }
        }
    }
    None
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    let mut guard = STATE.lock().unwrap();
    let mut st = guard.take().unwrap_or_else(|| State { tab: test_tab(), ..State::default() });
    drop(guard);
    draw_with(ui, app, a, &mut st);
    *STATE.lock().unwrap() = Some(st);
}

fn draw_with(ui: &mut Ui, app: &mut App, a: Rect, st: &mut State) {
    let ver = drs::driver_version().unwrap_or(0);
    let ver = format!("{}{}{:02}", ver / 100, crate::i18n::comma(), ver % 100);
    header_room(ui, a, "Driver NVIDIA", &format!("Driver {ver}. As mudanças valem na próxima vez que o jogo abrir."), 200.0);
    if ui.button("nv-bk", rect(a.right() - 28.0 - 160.0, 30.0, 160.0, 36.0), "Cópias", false) {
        st.backups = drs::backups();
        if st.backups.is_empty() {
            app.toast.info("Ainda não há cópias. O Prisma guarda uma antes da primeira mudança.");
        } else {
            let names: Vec<String> = st.backups.iter().map(|p| backup_name(p)).collect();
            let items: Vec<(&str, bool)> = names.iter().map(|n| (n.as_str(), true)).collect();
            ui.menu("nv-bk-menu", &items);
        }
    }
    if let Some(i) = ui.menu_pick("nv-bk-menu") {
        if let Some(p) = st.backups.get(i) {
            st.restore = Some(p.clone());
            ui.ask(
                "nv-restore",
                "Voltar a esta cópia?",
                &format!("Os perfis do driver voltam como estavam em {}. O que mudou depois, aqui ou em outro programa, se perde.", backup_name(p)),
            );
        }
    }
    if ui.answered("nv-restore") {
        if let Some(p) = st.restore.take() {
            let ok = drs::restore(&p);
            crate::config::log(&format!("driver NVIDIA: voltou a cópia {} ({ok})", backup_name(&p)));
            app.toast.info(if ok { "Perfis do driver restaurados" } else { "O driver não aceitou a cópia" });
            st.snap = None;
        }
    }

    let list = games(app);
    if st.who > list.len() {
        st.who = 0;
    }
    let who = if st.who == 0 { Profile::Global } else { Profile::Game(list[st.who - 1].clone()) };
    let x = a.x + 28.0;
    let mut names = vec!["Todos os jogos (global)".to_string()];
    names.extend(list.iter().map(|g| app_name(g).to_string()));
    let mut sel = st.who;
    picker::add_button(ui, app, rect(x + 332.0, 92.0, 120.0, 36.0), Target::NvGame);
    let own = st.who > 0 && app.cfg.nv_games.iter().any(|g| g.eq_ignore_ascii_case(&list[st.who - 1]));
    if own && ui.button("nv-rm", rect(x + 460.0, 92.0, 150.0, 36.0), "Tirar da lista", false) {
        ui.ask("nv-rm", "Tirar da lista?", "O jogo sai só desta lista. O que foi gravado no perfil dele continua no driver.");
    }
    if ui.answered("nv-rm") && st.who > 0 {
        let exe = list[st.who - 1].clone();
        app.cfg.nv_games.retain(|g| !g.eq_ignore_ascii_case(&exe));
        app.cfg.save();
        st.who = 0;
        st.snap = None;
        return;
    }

    let ids: Vec<u32> = nvset::GROUPS.iter().flat_map(|g| shown(&who, g.1).map(|s| s.id)).collect();
    if st.snap.as_ref().is_none_or(|s| s.0 != who || s.1 != ids) {
        st.snap = drs::snapshot(&who, &ids).map(|s| (who.clone(), ids.clone(), s));
    }
    let about = match (&who, &st.snap) {
        (Profile::Global, _) => "Vale para todos os jogos, menos o que o perfil de cada jogo troca.".to_string(),
        (_, Some((_, _, s))) if !s.name.is_empty() => format!("Perfil no driver: {}", s.name),
        _ => "Este jogo ainda não tem perfil no driver. O Prisma cria um na primeira mudança.".to_string(),
    };
    ui.c.text_fit(rect(x, 136.0, a.w - 56.0, 20.0), &about, 13.0, MUTED, Align::Left, Weight::Regular);
    tabs(ui, x, 164.0, &mut st.tab);

    let card = rect(x, 208.0, a.w - 56.0, a.h - 228.0);
    ui.card(card);
    let group = nvset::GROUPS[st.tab.min(nvset::GROUPS.len() - 1)].1;
    let rows: Vec<&Setting> = shown(&who, group).collect();
    let mut change = None;
    let inner = rect(card.x, card.y + 6.0, card.w, card.h - 12.0);
    ui.begin_scroll("nv-list", inner, rows.len() as f32 * ROW);
    for (i, s) in rows.iter().enumerate() {
        let v = st.snap.as_ref().and_then(|(_, ids, snap)| ids.iter().position(|id| *id == s.id).and_then(|k| snap.values[k]));
        let r = rect(inner.x, inner.y + i as f32 * ROW, inner.w, ROW);
        if let Some(c) = row(ui, s, v, r) {
            change = Some((s.id, c));
        }
    }
    ui.end_scroll();
    // A lista de perfis por último, para abrir por cima do resto.
    if ui.select("nv-who", rect(x, 92.0, 320.0, 36.0), &names, &mut sel) && sel != st.who {
        st.who = sel;
        st.snap = None;
        ui.s.needs_redraw = true;
    }

    if let Some((id, value)) = change {
        let ok = drs::write(&who, &[(id, value)]);
        let target = match &who {
            Profile::Global => "global".to_string(),
            Profile::Game(exe) => exe.clone(),
        };
        crate::config::log(&format!("driver NVIDIA: {target} 0x{id:08X} = {} ({ok})", value.map_or("padrão".into(), |v| format!("0x{v:08X}"))));
        if !ok {
            app.toast.info("O driver não aceitou a mudança");
        }
        st.snap = None;
        ui.s.needs_redraw = true;
    }
}

/// Fotos de teste: `--test-tab=N` abre numa aba.
fn test_tab() -> usize {
    if !cfg!(debug_assertions) {
        return 0;
    }
    std::env::args().find_map(|a| a.strip_prefix("--test-tab=").and_then(|n| n.parse().ok())).unwrap_or(0)
}

/// "perfis-2026-11-11-14-03-22.nvdrs" vira "2026-11-11 14:03:22".
fn backup_name(p: &std::path::Path) -> String {
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let d = stem.strip_prefix("perfis-").unwrap_or(&stem);
    match (d.get(..10), d.get(11..)) {
        (Some(day), Some(time)) => format!("{day} {}", time.replace('-', ":")),
        _ => d.to_string(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn backup_names_read_as_dates() {
        assert_eq!(super::backup_name(std::path::Path::new("x/perfis-2026-11-11-14-03-22.nvdrs")), "2026-11-11 14:03:22");
    }
}
