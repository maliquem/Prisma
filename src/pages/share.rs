//! Compartilhar tela: este PC como host, os aparelhos pareados e o serviço do Windows. O host, quem
//! assiste e o pareamento rodam em processos do próprio Prisma, presos a esta janela: se o Prisma
//! fechar, eles fecham junto. O que cada um diz vai para um arquivo, que a página mostra.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use super::*;
use crate::stream::{self, Opts, COPY, KEYBOARD, MOUSE, PAD, PRESETS, SOUND, SYSTEM};

const GAP: f32 = 12.0;
/// Nomes curtos das permissões, para caberem nas caixas.
const PERMS: [(&str, u8); 6] = [("Som", SOUND), ("Controle", PAD), ("Teclado", KEYBOARD), ("Mouse", MOUSE), ("Copiar e colar", COPY), ("Tela segura", SYSTEM)];
const NOTICE: &str = "Pela internet, a conexão usa o iroh (n0, licença MIT ou Apache-2.0). Sem caminho direto nem VPN, os pacotes passam pelos relés públicos da n0, sempre criptografados de ponta a ponta.";

struct Proc {
    child: Child,
    log: PathBuf,
}

impl Proc {
    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

#[derive(Default)]
struct State {
    host: Option<Proc>,
    view: Option<Proc>,
    pair: Option<Proc>,
    invite: Option<Proc>,
    field: String,
    job: isize,
    opts: Option<Opts>,
    /// Trabalho do serviço (instalar ou remover) rodando.
    service_busy: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Um trabalho preso à janela: o Windows fecha os processos do trabalho quando o Prisma fecha.
fn job(st: &mut State) -> windows::Win32::Foundation::HANDLE {
    use windows::Win32::System::JobObjects::*;
    if st.job == 0 {
        unsafe {
            if let Ok(j) = CreateJobObjectW(None, None) {
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let _ = SetInformationJobObject(j, JobObjectExtendedLimitInformation, &info as *const _ as *const _, std::mem::size_of_val(&info) as u32);
                st.job = j.0 as isize;
            }
        }
    }
    windows::Win32::Foundation::HANDLE(st.job as *mut _)
}

fn spawn(st: &mut State, args: &[String], log: &str) -> Option<Proc> {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    let dir = stream::user_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let log = dir.join(log);
    let out = std::fs::File::create(&log).ok()?;
    let err = out.try_clone().ok()?;
    // Sem janela de console: o que o processo diz vai para o arquivo.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let child = Command::new(std::env::current_exe().ok()?).args(args).stdin(Stdio::null()).stdout(out).stderr(err).creation_flags(CREATE_NO_WINDOW).spawn().ok()?;
    unsafe {
        let _ = windows::Win32::System::JobObjects::AssignProcessToJobObject(job(st), windows::Win32::Foundation::HANDLE(child.as_raw_handle()));
    }
    Some(Proc { child, log })
}

/// Fim do arquivo de um processo, em linhas.
fn tail(p: &Option<Proc>) -> Vec<String> {
    let Some(p) = p else { return Vec::new() };
    let Ok(b) = std::fs::read(&p.log) else { return Vec::new() };
    let start = b.len().saturating_sub(16 * 1024);
    String::from_utf8_lossy(&b[start..]).lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect()
}

fn invite_of(lines: &[String]) -> Option<String> {
    lines.iter().rev().map(|l| l.trim()).find(|l| l.starts_with("PRISMA-")).map(str::to_string)
}

fn stop(p: &mut Option<Proc>) {
    if let Some(mut x) = p.take() {
        // Só o processo que esta página abriu, pelo identificador dele.
        let _ = x.child.kill();
        let _ = x.child.wait();
    }
}

fn service_job(arg: &'static str, elevated: bool) {
    std::thread::spawn(move || {
        if elevated {
            let _ = Command::new(std::env::current_exe().unwrap_or_default()).arg(arg).status();
        } else {
            let _ = crate::startup::run_elevated_wait(arg);
        }
        if let Some(st) = STATE.lock().unwrap().as_mut() {
            st.service_busy = false;
        }
    });
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    let mut guard = STATE.lock().unwrap();
    let st = guard.get_or_insert_with(State::default);
    let opts = st.opts.get_or_insert_with(Opts::load).clone();
    let mut new_opts = opts.clone();
    header(ui, a, "Compartilhar tela", "Acesso remoto e co-op pela internet, sem conta. Em teste.");
    let service = stream::service::installed();

    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let top = 88.0;
    let lw = ((w - GAP) / 2.0).floor();
    let h = a.h - top - 20.0;

    // ---------- Este PC ----------
    let l = rect(x, top, lw, h);
    ui.card(l);
    let ix = l.x + 16.0;
    let iw = l.w - 32.0;
    ui.title(rect(ix, l.y + 12.0, iw, 22.0), "Este PC");
    let mut y = l.y + 40.0;
    let me = stream::me().map(|(n, id)| format!("«{n}», id {id}")).unwrap_or_default();
    ui.c.text_raw(rect(ix, y, iw, 20.0), &me, 13.0, TEXT2, Align::Left, Weight::Regular);
    y += 24.0;
    let hosting = st.host.as_mut().is_some_and(Proc::alive);
    let state = if service {
        "O serviço do Windows compartilha este PC, inclusive a tela de bloqueio."
    } else if hosting {
        "Compartilhando. Os aparelhos pareados já podem conectar."
    } else {
        "Desligado."
    };
    // Quem está conectado: o host grava a lista a cada mudança.
    let room: Vec<String> = if service || hosting {
        std::fs::read_to_string(stream::host::room_path()).unwrap_or_default().lines().filter(|l| !l.is_empty()).map(str::to_string).collect()
    } else {
        Vec::new()
    };
    let state = if room.is_empty() {
        crate::i18n::tr(state).into_owned()
    } else {
        format!("{} {}", crate::i18n::tr("Conectados agora:"), room.join(", "))
    };
    ui.c.text_raw(rect(ix, y, iw, 20.0), &state, 13.0, if !room.is_empty() { WARN } else if service || hosting { ACCENT } else { MUTED }, Align::Left, Weight::Regular);
    y += 26.0;
    ui.label(rect(ix, y, iw, 18.0), "Quem parear agora pode", TEXT2);
    let sel_y = y + 20.0;
    y += 70.0;
    let bw = ((iw - GAP) / 2.0).floor();
    if service {
        if ui.button("sh-inv", rect(ix, y, bw, 40.0), "Criar convite", true) {
            stop(&mut st.invite);
            st.invite = spawn(st, &[format!("--stream-invite={}", stream::perms_key(opts.invite))], "convite.log");
        }
    } else if hosting {
        if ui.button("sh-stop", rect(ix, y, bw, 40.0), "Parar", true) {
            stop(&mut st.host);
            // O host fechado à força pode ter deixado o HDR desligado.
            std::thread::spawn(stream::host::restore_hdr);
        }
    } else if ui.button("sh-go", rect(ix, y, bw, 40.0), "Compartilhar este PC", true) {
        st.host = spawn(st, &[format!("--stream-net={}", stream::perms_key(opts.invite))], "host.log");
    }
    let host_lines = if service { tail(&st.invite) } else { tail(&st.host) };
    let invite = invite_of(&host_lines);
    if ui.button("sh-copy", rect(ix + bw + GAP, y, bw, 40.0), "Copiar convite", false) {
        match &invite {
            Some(t) => {
                crate::capture::copy_text(app.hub, t);
                app.toast.info("Convite copiado. Vale 10 minutos e uma vez.");
            }
            None => app.toast.info("Nenhum convite ainda"),
        }
    }
    y += 48.0;
    let note = match &invite {
        Some(_) => "Convite pronto: mande para o outro PC. Ele vale 10 minutos e uma vez, e a confirmação aparece aqui.",
        None if service || hosting => "Ctrl+Alt+Shift+Q, neste PC, desconecta todo mundo na hora.",
        None => "O convite aparece quando o compartilhamento liga.",
    };
    ui.c.text_wrap(rect(ix, y, iw, 36.0), note, 13.0, MUTED, Weight::Regular);
    y += 40.0;

    ui.check("sh-hdr", rect(ix, y, iw, 36.0), "Desligar o HDR quando quem assiste não tem HDR", &mut new_opts.hdr_off);
    y += 38.0;
    let mut guests = new_opts.guests as u32;
    ui.label(rect(ix, y, iw / 2.0, 36.0), "Pessoas ao mesmo tempo", TEXT2);
    if ui.stepper("sh-guests", rect(ix + iw - 150.0, y, 150.0, 36.0), &mut guests, (1, 4), 1, "") {
        new_opts.guests = guests as usize;
    }
    y += 42.0;
    ui.label(rect(ix, y, iw / 2.0, 36.0), "Controles virtuais", TEXT2);
    let pads_y = y;
    y += 44.0;

    // Serviço do Windows.
    let srv = if service { "Serviço do Windows instalado" } else { "Serviço do Windows" };
    ui.c.text(rect(ix, y, iw - 130.0, 20.0), srv, 14.0, TEXT, Align::Left, Weight::Semibold);
    ui.c.text_wrap(rect(ix, y + 22.0, iw - 130.0, 36.0), "Para usar a tela de bloqueio, o UAC e o Ctrl+Alt+Del de longe. Pede administrador.", 12.0, MUTED, Weight::Regular);
    let label = if st.service_busy {
        "Aguarde"
    } else if service {
        "Remover"
    } else {
        "Instalar"
    };
    if ui.button("sh-srv", rect(ix + iw - 120.0, y + 6.0, 120.0, 40.0), label, false) && !st.service_busy {
        if service {
            ui.ask("sh-srv-off", "Remover o serviço?", "O compartilhamento da tela de bloqueio para, e os aparelhos pareados voltam para este usuário.");
        } else {
            ui.ask(
                "sh-srv-on",
                "Instalar o serviço?",
                "O Prisma vai pedir administrador e mostrar o que muda no Windows antes de instalar. O serviço liga com o PC, mesmo antes do login.",
            );
        }
    }
    if ui.answered("sh-srv-on") {
        stop(&mut st.host);
        st.service_busy = true;
        service_job("--stream-service-install", app.elevated);
    }
    if ui.answered("sh-srv-off") {
        st.service_busy = true;
        service_job("--stream-service-remove", app.elevated);
    }
    y += 60.0;
    let log_r = rect(ix, y, iw, l.bottom() - y - 12.0);
    if host_lines.is_empty() {
        ui.c.text_wrap(rect(ix, y, iw, 60.0), NOTICE, 12.0, MUTED, Weight::Regular);
    } else if log_r.h > 40.0 {
        log_view(ui, "sh-host-log", log_r, &host_lines);
    }

    // ---------- Aparelhos ----------
    let r = rect(l.right() + GAP, top, w - lw - GAP, h);
    ui.card(r);
    let rx = r.x + 16.0;
    let rw = r.w - 32.0;
    ui.title(rect(rx, r.y + 12.0, rw, 22.0), "Aparelhos pareados");
    let mut y = r.y + 40.0;
    ui.label(rect(rx, y, rw, 18.0), "Parear com outro PC", TEXT2);
    y += 20.0;
    let fw = rw - 2.0 * (84.0 + 8.0);
    ui.search("sh-field", rect(rx, y, fw, 40.0), &mut st.field, "Convite do outro PC");
    if ui.button("sh-paste", rect(rx + fw + 8.0, y, 84.0, 40.0), "Colar", false) {
        if let Some(t) = crate::capture::clipboard_text(app.hub) {
            st.field = t.trim().to_string();
        }
    }
    if ui.button("sh-pair", rect(rx + fw + 100.0, y, 84.0, 40.0), "Parear", true) {
        let text = st.field.trim().to_string();
        if text.starts_with("PRISMA-") || text.starts_with("prisma-") {
            stop(&mut st.pair);
            st.pair = spawn(st, &[format!("--stream-pair={text}")], "parear.log");
            st.field.clear();
        } else {
            app.toast.info("Isso não parece um convite do Prisma");
        }
    }
    y += 48.0;
    // O que o pareamento e quem assiste disseram por último.
    let side: Vec<String> = tail(&st.pair).into_iter().chain(tail(&st.view)).filter(|l| !l.starts_with("Conexão pela internet")).collect();
    if let Some(last) = side.last() {
        ui.c.text_wrap(rect(rx, y, rw, 36.0), last, 12.0, TEXT2, Weight::Regular);
    }
    y += 40.0;

    let rows = stream::device_rows();
    const ROW_V: f32 = 132.0;
    const ROW_H: f32 = 64.0;
    let list = rect(rx, y, rw, r.bottom() - y - 12.0);
    let content: f32 = rows.iter().map(|d| if d.viewer { ROW_V } else { ROW_H }).sum::<f32>().max(1.0);
    if rows.is_empty() {
        ui.c.text_wrap(rect(rx, y, rw, 60.0), "Nenhum aparelho pareado. Ligue o compartilhamento neste PC e cole o convite no outro, ou o contrário.", 13.0, MUTED, Weight::Regular);
    }
    let mut changes: Vec<(String, u8, bool)> = Vec::new();
    let mut forget: Option<(String, String)> = None;
    let mut watch: Option<String> = None;
    ui.begin_scroll("sh-list", list, content);
    let mut yy = list.y;
    for (i, d) in rows.iter().enumerate() {
        let rh = if d.viewer { ROW_V } else { ROW_H };
        let row = rect(rx, yy, rw, rh - 8.0);
        ui.c.round(row, 8.0, CARD2);
        let px = row.x + 12.0;
        let pw = row.w - 24.0;
        ui.c.text_raw(rect(px, row.y + 8.0, pw - 100.0, 20.0), &d.name, 14.0, TEXT, Align::Left, Weight::Semibold);
        let role = if d.viewer { "Assiste a este PC" } else { "Este PC assiste a ele" };
        ui.c.text(rect(px, row.y + 28.0, pw - 100.0, 18.0), role, 12.0, MUTED, Align::Left, Weight::Regular);
        if ui.button(&format!("sh-forget{i}"), rect(row.right() - 96.0, row.y + 10.0, 84.0, 32.0), "Esquecer", false) {
            forget = Some((d.id.clone(), d.name.clone()));
        }
        if d.viewer {
            let cw = (pw / 3.0).floor();
            let mut perms = d.perms;
            for (k, (name, bit)) in PERMS.iter().enumerate() {
                let cx = px + (k % 3) as f32 * cw;
                let cy = row.y + 50.0 + (k / 3) as f32 * 26.0;
                let mut on = perms & bit != 0;
                if ui.check(&format!("sh-p{i}-{k}"), rect(cx, cy, cw - 4.0, 26.0), name, &mut on) {
                    perms = if on { perms | bit } else { perms & !bit };
                }
            }
            let mut ask = d.ask;
            let changed_ask = ui.check(&format!("sh-ask{i}"), rect(px, row.y + 100.0, pw, 24.0), "Perguntar a cada conexão", &mut ask);
            if perms != d.perms || changed_ask {
                changes.push((d.id.clone(), perms, ask));
            }
        } else if ui.button(&format!("sh-watch{i}"), rect(row.right() - 188.0, row.y + 10.0, 84.0, 32.0), "Assistir", true) {
            watch = Some(d.name.clone());
        }
        yy += rh;
    }
    ui.end_scroll();
    for (id, perms, ask) in changes {
        if stream::set_device(&id, perms, ask).is_err() {
            app.toast.info("Não deu para salvar. Com o serviço, abra o Prisma como administrador.");
        }
    }
    if let Some((id, name)) = forget {
        ui.ask(&format!("sh-forget-{id}"), "Esquecer este aparelho?", &format!("«{name}» vai precisar de um convite novo para conectar."));
    }
    for d in &rows {
        if ui.answered(&format!("sh-forget-{}", d.id)) && stream::forget_device(d.viewer, &d.id).is_err() {
            app.toast.info("Não deu para salvar. Com o serviço, abra o Prisma como administrador.");
        }
    }
    if let Some(name) = watch {
        stop(&mut st.view);
        st.view = spawn(st, &[format!("--stream-connect={name}")], "assistir.log");
    }

    // Listas por último, para abrirem por cima do resto.
    let opt = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let names: Vec<&str> = PRESETS.iter().rev().map(|p| p.1).collect();
    let mut s = PRESETS.iter().rev().position(|p| p.2 == opts.invite).unwrap_or(0);
    if ui.select("sh-preset", rect(ix, sel_y, iw, 40.0), &opt(&names), &mut s) {
        new_opts.invite = PRESETS.iter().rev().nth(s).map_or(SOUND, |p| p.2);
    }
    let pads = ["auto", "vigem", "viiper"];
    let mut s = pads.iter().position(|p| *p == opts.pads).unwrap_or(0);
    if ui.select("sh-pads", rect(ix + iw - 220.0, pads_y, 220.0, 36.0), &opt(&["Automático", "ViGEmBus", "VIIPER"]), &mut s) {
        new_opts.pads = pads[s].to_string();
    }
    if new_opts != opts {
        if new_opts.save().is_err() {
            app.toast.info("Não deu para salvar. Com o serviço, abra o Prisma como administrador.");
        }
        st.opts = Some(new_opts);
    }
}

/// Fecha o que a página abriu (o trabalho também fecha tudo quando o Prisma sai).
pub fn shutdown() {
    if let Some(st) = STATE.lock().unwrap().as_mut() {
        let had_host = st.host.is_some();
        stop(&mut st.host);
        stop(&mut st.view);
        stop(&mut st.pair);
        stop(&mut st.invite);
        if had_host {
            stream::host::restore_hdr();
        }
    }
}
