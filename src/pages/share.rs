//! Compartilhar tela: este PC como host, os aparelhos pareados, as opções e o serviço do Windows. O
//! host, quem assiste e o pareamento rodam em processos do próprio Prisma, presos a esta janela: se
//! o Prisma fechar, eles fecham junto. O que cada um diz vai para um arquivo, que a página mostra.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Instant;

use super::*;
use crate::config::Hotkey;
use crate::stream::{self, code, weblink, ClientOpts, Opts, COPY, KEYBOARD, MOUSE, PAD, PRESETS, SOUND, SYSTEM};

const GAP: f32 = 12.0;
/// Nomes curtos das permissões, para caberem nas caixas.
const PERMS: [(&str, u8); 6] = [("Som", SOUND), ("Controle", PAD), ("Teclado", KEYBOARD), ("Mouse", MOUSE), ("Copiar e colar", COPY), ("Tela segura", SYSTEM)];
const NOTICE: &str = "Pela internet, a conexão usa o iroh (n0, licença MIT ou Apache-2.0). Sem caminho direto nem VPN, os pacotes passam pelos relés públicos da n0, sempre criptografados de ponta a ponta.";
const TABS: [&str; 6] = ["Compartilhar", "Host", "Cliente", "Rede", "Atalhos", "Controles"];

struct Proc {
    child: Child,
    log: PathBuf,
}

impl Proc {
    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

/// O convite que a janela mostra: código, convite longo e se os relés aceitaram.
#[derive(Clone, Default)]
struct Shown {
    code: String,
    long: Option<String>,
    /// None publicando, Some(true) publicado, Some(false) nenhum relé aceitou.
    published: Option<bool>,
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
    client: Option<ClientOpts>,
    /// Trabalho do serviço (instalar ou remover) rodando.
    service_busy: bool,
    tab: usize,
    /// A janela do convite aberta.
    popup: bool,
    /// O último código que a janela já mostrou sozinha.
    seen: String,
    /// Código que chegou por link, esperando a resposta da pergunta.
    link_code: Option<String>,
    /// Pedido para abrir a janela Compartilhar do Windows (fora do desenho).
    share_req: Option<(String, String, String)>,
    name_field: Option<String>,
    port_field: Option<String>,
    sound_apps: (Vec<String>, Option<Instant>),
    /// Fotos de teste: um convite de exemplo.
    test_invite: Option<Shown>,
}

/// Fotos de teste: `--test-tab=N` abre numa aba e `--test-invite` mostra a janela do convite.
fn test_setup() -> State {
    let mut s = State::default();
    if cfg!(debug_assertions) {
        for a in std::env::args() {
            if let Some(n) = a.strip_prefix("--test-tab=").and_then(|n| n.parse().ok()) {
                s.tab = n;
            }
            if a == "--test-invite" {
                s.test_invite = Some(Shown { code: "K7M2-QX9P".into(), long: Some("PRISMA-EXEMPLO".into()), published: Some(true) });
                s.seen = "K7M2-QX9P".into();
                s.popup = true;
            }
        }
    }
    s
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

/// O convite mais novo que o processo do convite escreveu.
fn invite_of(lines: &[String]) -> Option<Shown> {
    let at = lines.iter().rposition(|l| l.starts_with(stream::CODE_LINE))?;
    let rest = &lines[at..];
    let code = rest[0].trim_start_matches(stream::CODE_LINE).trim().to_string();
    let long = rest.iter().map(|l| l.trim()).find(|l| l.starts_with("PRISMA-")).map(str::to_string);
    let published = if rest.iter().any(|l| l.starts_with(stream::CODE_OK)) {
        Some(true)
    } else if rest.iter().any(|l| l.starts_with(stream::CODE_FAIL)) {
        Some(false)
    } else {
        None
    };
    Some(Shown { code, long, published })
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

fn opt(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Linha de opção: o nome à esquerda, o controle à direita (devolve a área dele).
fn row(ui: &mut Ui, x: f32, y: f32, w: f32, label: &str, ctl_w: f32) -> Rect {
    ui.label(rect(x, y, w - ctl_w - 12.0, 36.0), label, TEXT2);
    rect(x + w - ctl_w, y, ctl_w, 36.0)
}

/// Linha de abas sob o título.
fn tabs(ui: &mut Ui, x: f32, y: f32, sel: &mut usize) {
    let mut cx = x;
    for (i, name) in TABS.iter().enumerate() {
        let w = crate::ui::gfx::measure(&crate::i18n::tr(name), 14.0, Weight::Semibold, None).0 + 28.0;
        let r = rect(cx, y, w, 34.0);
        let on = *sel == i;
        if on {
            ui.c.round(r, 8.0, CARD2);
        }
        if ui.hit(&format!("sh-tab{i}"), r) {
            *sel = i;
        }
        ui.c.text(r, name, 14.0, if on { TEXT } else { MUTED }, Align::Center, if on { Weight::Semibold } else { Weight::Regular });
        cx += w + 4.0;
    }
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    let mut guard = STATE.lock().unwrap();
    let st = guard.get_or_insert_with(test_setup);
    let opts = st.opts.get_or_insert_with(Opts::load).clone();
    let client = st.client.get_or_insert_with(ClientOpts::load).clone();
    let mut new_opts = opts.clone();
    let mut new_client = client.clone();

    // Um link de convite chegou: pergunta antes de parear.
    if let Some(c) = weblink::take_pending() {
        st.tab = 0;
        ui.ask(
            "sh-link",
            "Parear com este convite?",
            &format!("Chegou o convite {c}. Os dois PCs vão mostrar um código de confirmação de seis dígitos: confira antes de aceitar no outro PC."),
        );
        st.link_code = Some(c);
    }
    if ui.answered("sh-link") {
        if let Some(c) = st.link_code.take() {
            stop(&mut st.pair);
            st.pair = spawn(st, &[format!("--stream-pair={c}")], "parear.log");
        }
    }

    let invite_lines = tail(&st.invite);
    let shown = invite_of(&invite_lines).or_else(|| st.test_invite.clone());
    if let Some(s) = &shown {
        if s.code != st.seen {
            st.seen = s.code.clone();
            st.popup = true;
        }
    }
    if st.popup {
        ui.block();
    }

    header(ui, a, "Compartilhar tela", "Acesso remoto e co-op pela internet, sem conta. Em teste.");
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    tabs(ui, x, 84.0, &mut st.tab);
    let top = 128.0;
    let h = a.h - top - 20.0;
    match st.tab {
        1 => host_tab(ui, st, &mut new_opts, x, top, w, h),
        2 => client_tab(ui, &mut new_client, x, top, w, h),
        3 => net_tab(ui, app, st, &mut new_opts, x, top, w, h),
        4 => keys_tab(ui, app, &mut new_opts, &mut new_client, x, top, w, h),
        5 => pads_tab(ui, &mut new_opts, &mut new_client, x, top, w, h),
        _ => main_tab(ui, app, st, &opts, &mut new_opts, &shown, x, top, w, h),
    }

    if st.popup {
        if let Some(s) = &shown {
            invite_sheet(ui, app, st, s);
        } else {
            st.popup = false;
        }
    }

    if new_opts != opts {
        if new_opts.save().is_err() {
            app.toast.info("Não deu para salvar. Com o serviço, abra o Prisma como administrador.");
        }
        st.opts = Some(new_opts);
    }
    if new_client != client {
        if new_client.save().is_err() {
            app.toast.info("Não deu para salvar as opções de quem assiste.");
        }
        st.client = Some(new_client);
    }
}

/// A janela do convite: o código grande, copiar, link e compartilhar.
fn invite_sheet(ui: &mut Ui, app: &mut App, st: &mut State, s: &Shown) {
    let b = ui.sheet(540.0, 330.0);
    let ix = b.x + 24.0;
    let iw = b.w - 48.0;
    ui.c.text(rect(ix, b.y + 20.0, iw, 28.0), "Convite criado", 17.0, TEXT, Align::Left, Weight::Semibold);
    ui.c.text_raw(rect(ix, b.y + 58.0, iw, 52.0), &s.code, 38.0, ACCENT, Align::Center, Weight::Semibold);
    let (status, color) = match s.published {
        None => ("Publicando o código nos relés...", MUTED),
        Some(true) => ("Pronto. Vale 10 minutos e uma vez.", ACCENT),
        Some(false) => ("Os relés não responderam: mande o convite longo.", WARN),
    };
    ui.c.text(rect(ix, b.y + 112.0, iw, 20.0), status, 13.0, color, Align::Center, Weight::Regular);
    ui.c.text_wrap(
        rect(ix, b.y + 142.0, iw, 40.0),
        "No outro PC, digite o código em Parear com outro PC, ou mande o link: ele abre o Prisma e pareia depois de perguntar.",
        13.0,
        TEXT2,
        Weight::Regular,
    );
    let bw = ((iw - 2.0 * GAP) / 3.0).floor();
    let y = b.y + 196.0;
    let link = weblink::web_link(&s.code);
    let ready = s.published != Some(false);
    if ui.button_ex("sh-c-code", rect(ix, y, bw, 40.0), "Copiar código", false, ready) {
        crate::capture::copy_text(app.hub, &s.code);
        app.toast.info("Código copiado. Vale 10 minutos e uma vez.");
    }
    if ui.button_ex("sh-c-link", rect(ix + bw + GAP, y, bw, 40.0), "Copiar link", false, ready) {
        crate::capture::copy_text(app.hub, &link);
        app.toast.info("Link copiado. Vale 10 minutos e uma vez.");
    }
    if ui.button_ex("sh-c-share", rect(ix + 2.0 * (bw + GAP), y, bw, 40.0), "Compartilhar", true, ready) {
        let text = format!("{} {}", crate::i18n::tr("Convite do Prisma para ver este PC:"), s.code);
        st.share_req = Some((crate::i18n::tr("Convite do Prisma").into_owned(), format!("{text}\n{link}"), link.clone()));
    }
    let y2 = b.bottom() - 64.0;
    if let Some(long) = &s.long {
        if ui.button("sh-c-long", rect(ix, y2, 200.0, 40.0), "Copiar convite longo", false) {
            crate::capture::copy_text(app.hub, long);
            app.toast.info("Convite copiado. Vale 10 minutos e uma vez.");
        }
    }
    if ui.button("sh-c-close", rect(b.right() - 24.0 - 112.0, y2, 112.0, 40.0), "Fechar", false) {
        st.popup = false;
    }
}

#[allow(clippy::too_many_arguments)]
fn main_tab(ui: &mut Ui, app: &mut App, st: &mut State, opts: &Opts, new_opts: &mut Opts, shown: &Option<Shown>, x: f32, top: f32, w: f32, h: f32) {
    let service = stream::service::installed();
    let lw = ((w - GAP) / 2.0).floor();

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
    let failing = service && stream::service::failing();
    let state = if failing && room.is_empty() {
        crate::i18n::tr("O serviço está caindo: reabra o Prisma como administrador.").into_owned()
    } else if room.is_empty() {
        crate::i18n::tr(state).into_owned()
    } else {
        format!("{} {}", crate::i18n::tr("Conectados agora:"), room.join(", "))
    };
    ui.c.text_raw(rect(ix, y, iw, 20.0), &state, 13.0, if !room.is_empty() || failing { WARN } else if service || hosting { ACCENT } else { MUTED }, Align::Left, Weight::Regular);
    y += 26.0;
    ui.label(rect(ix, y, iw, 18.0), "Quem parear agora pode", TEXT2);
    let sel_y = y + 20.0;
    y += 70.0;
    let bw = ((iw - GAP) / 2.0).floor();
    if service {
        ui.c.text_wrap(rect(ix, y, bw, 40.0), "Ligado pelo serviço do Windows.", 13.0, MUTED, Weight::Regular);
    } else if hosting {
        if ui.button("sh-stop", rect(ix, y, bw, 40.0), "Parar", false) {
            stop(&mut st.host);
            // O host fechado à força pode ter deixado o HDR desligado.
            std::thread::spawn(stream::host::restore_hdr);
        }
    } else if ui.button("sh-go", rect(ix, y, bw, 40.0), "Compartilhar este PC", false) {
        st.host = spawn(st, &["--stream-share".to_string()], "host.log");
    }
    // O convite vale para o host que estiver ligado; sem host, liga junto.
    if ui.button("sh-inv", rect(ix + bw + GAP, y, bw, 40.0), "Criar convite", true) {
        if !service && !hosting {
            st.host = spawn(st, &["--stream-share".to_string()], "host.log");
        }
        stop(&mut st.invite);
        st.invite = spawn(st, &[format!("--stream-invite={}", stream::perms_key(opts.invite))], "convite.log");
    }
    y += 48.0;
    match shown {
        Some(s) => {
            let t = format!("{} {}", crate::i18n::tr("Último convite:"), s.code);
            ui.c.text_raw(rect(ix, y, iw - 100.0, 32.0), &t, 13.0, TEXT2, Align::Left, Weight::Regular);
            if ui.button("sh-show", rect(ix + iw - 92.0, y, 92.0, 32.0), "Mostrar", false) {
                st.popup = true;
            }
        }
        None => {
            let note = if service || hosting { "Ctrl+Alt+Shift+Q, neste PC, desconecta todo mundo na hora." } else { "Os aparelhos pareados conectam quando o compartilhamento está ligado." };
            ui.c.text_wrap(rect(ix, y, iw, 36.0), note, 13.0, MUTED, Weight::Regular);
        }
    }
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
    let host_lines = tail(&st.host);
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
    ui.search("sh-field", rect(rx, y, fw, 40.0), &mut st.field, "Código ou convite do outro PC");
    if ui.button("sh-paste", rect(rx + fw + 8.0, y, 84.0, 40.0), "Colar", false) {
        if let Some(t) = crate::capture::clipboard_text(app.hub) {
            st.field = t.trim().to_string();
        }
    }
    if ui.button("sh-pair", rect(rx + fw + 100.0, y, 84.0, 40.0), "Parear", true) {
        let text = st.field.trim().to_string();
        if code::normalize(&text).is_some() || text.to_ascii_uppercase().starts_with("PRISMA-") {
            stop(&mut st.pair);
            st.pair = spawn(st, &[format!("--stream-pair={text}")], "parear.log");
            st.field.clear();
        } else {
            app.toast.info("Isso não parece um código do Prisma");
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
        ui.c.text_wrap(rect(rx, y, rw, 60.0), "Nenhum aparelho pareado. Crie um convite neste PC e digite o código no outro, ou o contrário.", 13.0, MUTED, Weight::Regular);
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
    let names: Vec<&str> = PRESETS.iter().rev().map(|p| p.1).collect();
    let mut s = PRESETS.iter().rev().position(|p| p.2 == opts.invite).unwrap_or(0);
    if ui.select("sh-preset", rect(ix, sel_y, iw, 40.0), &opt(&names), &mut s) {
        new_opts.invite = PRESETS.iter().rev().nth(s).map_or(SOUND, |p| p.2);
    }
}

/// Valores das listas do host, com o 0 como automático.
const RES: [(u32, &str); 5] = [(0, "Automático"), (720, "720p"), (1080, "1080p"), (1440, "1440p"), (2160, "2160p (4K)")];
const FPS: [u32; 8] = [0, 30, 60, 90, 120, 144, 165, 240];
const MBPS: [u32; 9] = [0, 5, 10, 20, 30, 50, 80, 120, 150];

fn pick<T: PartialEq + Copy>(ui: &mut Ui, key: &str, r: Rect, values: &[T], names: Vec<String>, cur: &mut T) {
    let mut s = values.iter().position(|v| v == cur).unwrap_or(0);
    if ui.select(key, r, &names, &mut s) {
        *cur = values[s];
    }
}

#[allow(clippy::too_many_arguments)]
fn host_tab(ui: &mut Ui, st: &mut State, o: &mut Opts, x: f32, top: f32, w: f32, h: f32) {
    let lw = ((w - GAP) / 2.0).floor();
    let l = rect(x, top, lw, h);
    ui.card(l);
    let ix = l.x + 16.0;
    let iw = l.w - 32.0;
    ui.title(rect(ix, l.y + 12.0, iw, 22.0), "Imagem");
    let mut y = l.y + 44.0;
    // As listas abrem por cima: guardo onde ficam e desenho no fim.
    let ctl = 240.0f32.min(iw * 0.55);
    let name_r = row(ui, ix, y, iw, "Nome deste PC", ctl);
    let field = st.name_field.get_or_insert_with(|| o.name.clone());
    ui.search("sh-name", name_r, field, &std::env::var("COMPUTERNAME").unwrap_or_default());
    let clean: String = field.chars().filter(|c| !c.is_control()).take(40).collect();
    if clean.trim() != o.name {
        o.name = clean.trim().to_string();
    }
    y += 44.0;
    let mon_r = row(ui, ix, y, iw, "Monitor", ctl);
    y += 44.0;
    let res_r = row(ui, ix, y, iw, "Resolução máxima", ctl);
    y += 44.0;
    let fps_r = row(ui, ix, y, iw, "Quadros por segundo", ctl);
    y += 44.0;
    let pace_r = row(ui, ix, y, iw, "Ritmo dos quadros", ctl);
    y += 44.0;
    let prio_r = row(ui, ix, y, iw, "Prioridade", ctl);
    y += 44.0;
    let codec_r = row(ui, ix, y, iw, "Codec", ctl);
    y += 44.0;
    let mbps_r = row(ui, ix, y, iw, "Limite de banda", ctl);
    y += 46.0;
    ui.check("sh-10", rect(ix, y, iw, 30.0), "HDR10 de 10 bits quando as duas telas estão em HDR", &mut o.ten_bit);
    y += 32.0;
    ui.check("sh-444", rect(ix, y, iw, 30.0), "Aceitar 4:4:4 quando quem assiste pede", &mut o.allow444);
    y += 32.0;
    ui.check("sh-hdr", rect(ix, y, iw, 30.0), "Desligar o HDR quando quem assiste não tem HDR", &mut o.hdr_off);
    y += 36.0;
    ui.c.text_wrap(rect(ix, y, iw, (l.bottom() - y - 12.0).max(0.0)), "Os automáticos seguem quem assiste: a altura da tela, o menor entre os Hz dos dois lados e o melhor codec que os dois têm. O 4:4:4 e o HDR10 caem sozinhos se o outro lado não tiver.", 12.0, MUTED, Weight::Regular);

    let r = rect(l.right() + GAP, top, w - lw - GAP, h);
    ui.card(r);
    let rx = r.x + 16.0;
    let rw = r.w - 32.0;
    ui.title(rect(rx, r.y + 12.0, rw, 22.0), "Som, mouse e teclado");
    let mut y = r.y + 44.0;
    let rctl = 240.0f32.min(rw * 0.55);
    let snd_r = row(ui, rx, y, rw, "Som que vai", rctl);
    y += 44.0;
    ui.check("sh-calls", rect(rx, y, rw, 30.0), "Tirar do som o programa de chamada", &mut o.no_calls);
    y += 40.0;
    let mut guests = o.guests as u32;
    ui.label(rect(rx, y, rw - 160.0, 36.0), "Pessoas ao mesmo tempo", TEXT2);
    if ui.stepper("sh-guests", rect(rx + rw - 150.0, y, 150.0, 36.0), &mut guests, (1, 4), 1, "") {
        o.guests = guests as usize;
    }
    y += 46.0;
    ui.check("sh-one", rect(rx, y, rw, 30.0), "Uma pessoa por vez no mouse e no teclado", &mut o.one_at_a_time);
    y += 32.0;
    ui.check("sh-first", rect(rx, y, rw, 30.0), "O mouse e o teclado deste PC têm prioridade", &mut o.host_first);
    y += 32.0;
    ui.check("sh-chain", rect(rx, y, rw, 30.0), "Recusar conexões enquanto este PC assiste a outro", &mut o.no_chain);
    y += 38.0;
    ui.c.text_wrap(
        rect(rx, y, rw, (r.bottom() - y - 12.0).max(0.0)),
        "Com prioridade, quem assiste para por 1 segundo sempre que alguém mexe no mouse ou no teclado daqui. Uma pessoa por vez: a vez passa depois de 2 segundos sem mexer. Programas de chamada: Discord, Teams, Zoom, WhatsApp, Skype, Telegram, Slack e Signal; sai do som o primeiro que estiver aberto.",
        12.0,
        MUTED,
        Weight::Regular,
    );

    // Listas.
    let mons = stream::host::monitors();
    let mut mon_vals: Vec<u32> = vec![0];
    let mut mon_names = vec![crate::i18n::tr("Principal").into_owned()];
    for (i, m) in mons.iter().enumerate() {
        let mut mi = windows::Win32::Graphics::Gdi::MONITORINFO { cbSize: std::mem::size_of::<windows::Win32::Graphics::Gdi::MONITORINFO>() as u32, ..Default::default() };
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::GetMonitorInfoW(*m, &mut mi);
        }
        let rc = mi.rcMonitor;
        mon_vals.push(i as u32 + 1);
        mon_names.push(format!("{} {} ({}x{})", crate::i18n::tr("Monitor"), i + 1, rc.right - rc.left, rc.bottom - rc.top));
    }
    if !mon_vals.contains(&o.monitor) {
        mon_vals.push(o.monitor);
        mon_names.push(format!("{} {}", crate::i18n::tr("Monitor"), o.monitor));
    }
    pick(ui, "sh-mon", mon_r, &mon_vals, mon_names, &mut o.monitor);
    let rv: Vec<u32> = RES.iter().map(|r| r.0).collect();
    pick(ui, "sh-res", res_r, &rv, opt(&RES.map(|r| r.1)), &mut o.res);
    let fps_names: Vec<String> = FPS.iter().map(|&f| if f == 0 { crate::i18n::tr("Automático").into_owned() } else { f.to_string() }).collect();
    pick(ui, "sh-fps", fps_r, &FPS, fps_names, &mut o.fps);
    pick(ui, "sh-pace", pace_r, &[false, true], opt(&["Só quando a tela muda", "Constante"]), &mut o.constant);
    pick(ui, "sh-prio", prio_r, &[0u8, 1, 2], opt(&["Menor atraso", "Equilíbrio", "Qualidade"]), &mut o.priority);
    let codec_vals: Vec<&str> = stream::CODECS.to_vec();
    let mut cur = o.codec.as_str();
    pick(ui, "sh-codec", codec_r, &codec_vals, opt(&["Automático", "AV1", "HEVC (H.265)", "H.264"]), &mut cur);
    o.codec = cur.to_string();
    let mbps_names: Vec<String> = MBPS.iter().map(|&m| if m == 0 { crate::i18n::tr("Automático").into_owned() } else { format!("{m} Mbit/s") }).collect();
    pick(ui, "sh-mbps", mbps_r, &MBPS, mbps_names, &mut o.mbps);

    // Programas com som agora, lidos a cada 5 s.
    if st.sound_apps.1.is_none_or(|t| t.elapsed().as_secs() >= 5) {
        let mut v: Vec<String> = crate::sys::audio::sessions().into_iter().map(|s| s.exe).filter(|e| !e.is_empty()).collect();
        v.sort();
        v.dedup();
        st.sound_apps = (v, Some(Instant::now()));
    }
    let mut vals: Vec<String> = vec![String::new()];
    vals.extend(st.sound_apps.0.iter().cloned());
    if !o.sound_app.is_empty() && !vals.contains(&o.sound_app) {
        vals.push(o.sound_app.clone());
    }
    let mut names = vec![crate::i18n::tr("Todo o PC").into_owned()];
    names.extend(vals[1..].iter().map(|e| format!("{} {e}", crate::i18n::tr("Só"))));
    let mut s = vals.iter().position(|v| *v == o.sound_app).unwrap_or(0);
    if ui.select("sh-snd", snd_r, &names, &mut s) {
        o.sound_app = vals[s].clone();
    }
}

fn client_tab(ui: &mut Ui, c: &mut ClientOpts, x: f32, top: f32, w: f32, h: f32) {
    let lw = ((w - GAP) / 2.0).floor();
    let l = rect(x, top, lw, h);
    ui.card(l);
    let ix = l.x + 16.0;
    let iw = l.w - 32.0;
    ui.title(rect(ix, l.y + 12.0, iw, 22.0), "Janela de quem assiste");
    let mut y = l.y + 44.0;
    ui.check("sh-full", rect(ix, y, iw, 30.0), "Abrir em tela cheia sem borda", &mut c.fullscreen);
    y += 34.0;
    ui.check("sh-vsync", rect(ix, y, iw, 30.0), "VSync: sem rasgos na imagem, com um pouco mais de atraso", &mut c.vsync);
    y += 40.0;
    let ctl = 240.0f32.min(iw * 0.55);
    let imm_r = row(ui, ix, y, iw, "Modo imersivo", ctl);
    y += 46.0;
    ui.check("sh-ask444", rect(ix, y, iw, 30.0), "Pedir 4:4:4 (texto mais nítido; precisa de NVIDIA ou Intel)", &mut c.ask444);
    y += 34.0;
    ui.check("sh-mute", rect(ix, y, iw, 30.0), "Começar sem o som do outro PC", &mut c.mute);
    y += 40.0;
    ui.c.text_wrap(
        rect(ix, y, iw, (l.bottom() - y - 12.0).max(0.0)),
        "No modo imersivo, em tela cheia, o mouse fica preso na janela, e a tecla Windows e o Alt+Tab vão para o outro PC. O atalho do menu sempre funciona aqui. Numa tela com G-SYNC ou FreeSync, deixe o VSync desligado: a imagem não rasga.",
        12.0,
        MUTED,
        Weight::Regular,
    );

    let r = rect(l.right() + GAP, top, w - lw - GAP, h);
    ui.card(r);
    let rx = r.x + 16.0;
    let rw = r.w - 32.0;
    ui.title(rect(rx, r.y + 12.0, rw, 22.0), "Medidor e avisos");
    let mut y = r.y + 44.0;
    let rctl = 240.0f32.min(rw * 0.55);
    let meter_r = row(ui, rx, y, rw, "Medidor de latência", rctl);
    y += 46.0;
    ui.check("sh-warn", rect(rx, y, rw, 30.0), "Avisar quando a conexão piorar", &mut c.warn);
    y += 40.0;
    ui.c.text_wrap(
        rect(rx, y, rw, (r.bottom() - y - 12.0).max(0.0)),
        "O simples mostra o atraso de ponta a ponta e uma bolinha verde, amarela ou vermelha. O detalhado mostra também a rede, o vídeo, os quadros por segundo, a banda, a decodificação, os quadros perdidos e o caminho. O aviso aparece quando a rede perde mais de 2% dos quadros, quando a latência varia mais de 10 ms ou sobe por 3 segundos seguidos. Os dois mudam também pelo menu da conexão.",
        12.0,
        MUTED,
        Weight::Regular,
    );

    pick(ui, "sh-imm", imm_r, &[0u8, 1, 2, 3], opt(&["Desligado", "Só o mouse", "Só o teclado", "Mouse e teclado"]), &mut c.immersive);
    pick(ui, "sh-meter", meter_r, &[0u8, 1, 2], opt(&["Desligado", "Simples", "Detalhado"]), &mut c.meter);
}

#[allow(clippy::too_many_arguments)]
fn net_tab(ui: &mut Ui, app: &mut App, st: &mut State, o: &mut Opts, x: f32, top: f32, w: f32, h: f32) {
    let lw = ((w - GAP) / 2.0).floor();
    let l = rect(x, top, lw, h);
    ui.card(l);
    let ix = l.x + 16.0;
    let iw = l.w - 32.0;
    ui.title(rect(ix, l.y + 12.0, iw, 22.0), "Portas");
    let mut y = l.y + 44.0;
    let port_r = row(ui, ix, y, iw, "Porta UDP do host", 160.0);
    let field = st.port_field.get_or_insert_with(|| if o.port == 0 { String::new() } else { o.port.to_string() });
    field.retain(|c| c.is_ascii_digit());
    field.truncate(5);
    ui.search("sh-port", port_r, field, &crate::i18n::tr("Automática"));
    match field.parse::<u32>() {
        Ok(p) if (1024..=65535).contains(&p) => o.port = p as u16,
        _ if field.is_empty() => o.port = 0,
        _ => {}
    }
    y += 44.0;
    if !field.is_empty() && o.port.to_string() != *field {
        ui.c.text(rect(ix, y, iw, 18.0), "Use um número de 1024 a 65535.", 12.0, WARN, Align::Left, Weight::Regular);
    }
    y += 24.0;
    ui.check("sh-upnp", rect(ix, y, iw, 30.0), "Pedir ao roteador para abrir a porta (UPnP, NAT-PMP, PCP)", &mut o.upnp);
    y += 40.0;
    ui.c.text_wrap(
        rect(ix, y, iw, (l.bottom() - y - 12.0).max(0.0)),
        "Automática serve para quase todo mundo: o Prisma fura o NAT sem porta fixa. A porta fixa é para quem redireciona a porta no roteador. Ela vale na próxima vez que o compartilhamento ligar. O pedido ao roteador some sozinho quando a conexão acaba.",
        12.0,
        MUTED,
        Weight::Regular,
    );
    let _ = app;

    let r = rect(l.right() + GAP, top, w - lw - GAP, h);
    ui.card(r);
    let rx = r.x + 16.0;
    let rw = r.w - 32.0;
    ui.title(rect(rx, r.y + 12.0, rw, 22.0), "Como a conexão passa");
    let y = r.y + 44.0;
    ui.c.text_wrap(rect(rx, y, rw, (r.bottom() - y - 12.0).max(0.0)), NOTICE, 13.0, TEXT2, Weight::Regular);
    let lines = tail(&st.view);
    if let Some(route) = lines.iter().rev().find_map(|l| l.strip_prefix("conectado: ")) {
        let t = format!("{} {route}", crate::i18n::tr("Última conexão:"));
        ui.c.text_raw(rect(rx, r.bottom() - 40.0, rw, 24.0), &t, 13.0, ACCENT, Align::Left, Weight::Regular);
    }
}

fn hotkey_row(ui: &mut Ui, key: &str, x: f32, y: f32, w: f32, label: &str, k: &mut stream::Keys) {
    let r = row(ui, x, y, w, label, 220.0);
    let mut hk = Hotkey::new(k.0, k.1);
    if ui.hotkey(key, r, &mut hk) && hk.vk != 0 {
        *k = (hk.mods, hk.vk);
    }
}

#[allow(clippy::too_many_arguments)]
fn keys_tab(ui: &mut Ui, app: &mut App, o: &mut Opts, c: &mut ClientOpts, x: f32, top: f32, w: f32, h: f32) {
    let card = rect(x, top, w, h);
    ui.card(card);
    let ix = card.x + 16.0;
    let iw = (card.w - 32.0).min(720.0);
    ui.title(rect(ix, card.y + 12.0, iw, 22.0), "Atalhos da conexão");
    let mut y = card.y + 44.0;
    ui.label(rect(ix, y, iw, 18.0), "Na janela de quem assiste", MUTED);
    y += 24.0;
    hotkey_row(ui, "sh-k-menu", ix, y, iw, "Menu da conexão", &mut c.key_menu);
    y += 44.0;
    hotkey_row(ui, "sh-k-full", ix, y, iw, "Tela cheia", &mut c.key_full);
    y += 44.0;
    hotkey_row(ui, "sh-k-cad", ix, y, iw, "Ctrl+Alt+Del no outro PC", &mut c.key_cad);
    y += 52.0;
    ui.label(rect(ix, y, iw, 18.0), "No PC que compartilha", MUTED);
    y += 24.0;
    hotkey_row(ui, "sh-k-panic", ix, y, iw, "Desconectar todo mundo", &mut o.key_panic);
    y += 52.0;
    let keys = [c.key_menu, c.key_full, c.key_cad];
    if keys.iter().enumerate().any(|(i, k)| keys.iter().skip(i + 1).any(|j| j == k)) {
        ui.c.text(rect(ix, y, iw, 20.0), "Dois atalhos iguais: só o primeiro da lista funciona.", 13.0, WARN, Align::Left, Weight::Regular);
        y += 26.0;
    }
    ui.c.text_wrap(
        rect(ix, y, iw, (card.bottom() - y - 12.0).max(0.0)),
        "Clique num atalho e aperte a combinação nova; Esc cancela. Os atalhos de quem assiste ficam com a janela e não vão para o outro PC. Use combinações com Ctrl, Alt ou Shift, para não roubar teclas do jogo. Os novos valem na próxima conexão.",
        12.0,
        MUTED,
        Weight::Regular,
    );
    let _ = app;
}

/// Os controles deste PC: vaga do XInput e a bateria, quando o Windows sabe.
fn local_pads() -> Vec<(u32, String)> {
    use windows::Win32::UI::Input::XboxController::*;
    let mut out = Vec::new();
    for i in 0..4u32 {
        let mut st = XINPUT_STATE::default();
        if unsafe { XInputGetState(i, &mut st) } != 0 {
            continue;
        }
        let mut b = XINPUT_BATTERY_INFORMATION::default();
        let bat = if unsafe { XInputGetBatteryInformation(i, BATTERY_DEVTYPE_GAMEPAD, &mut b) } == 0 {
            match (b.BatteryType, b.BatteryLevel) {
                (t, _) if t == BATTERY_TYPE_WIRED => "com fio",
                (t, _) if t == BATTERY_TYPE_DISCONNECTED || t == BATTERY_TYPE_UNKNOWN => "",
                (_, l) if l == BATTERY_LEVEL_FULL => "bateria cheia",
                (_, l) if l == BATTERY_LEVEL_MEDIUM => "bateria média",
                (_, l) if l == BATTERY_LEVEL_LOW => "bateria baixa",
                _ => "bateria acabando",
            }
        } else {
            ""
        };
        out.push((i + 1, bat.to_string()));
    }
    out
}

fn pads_tab(ui: &mut Ui, o: &mut Opts, c: &mut ClientOpts, x: f32, top: f32, w: f32, h: f32) {
    let lw = ((w - GAP) / 2.0).floor();
    let l = rect(x, top, lw, h);
    ui.card(l);
    let ix = l.x + 16.0;
    let iw = l.w - 32.0;
    ui.title(rect(ix, l.y + 12.0, iw, 22.0), "Controles");
    let mut y = l.y + 44.0;
    let bridge_r = row(ui, ix, y, iw, "Controles virtuais no host", 200.0);
    y += 46.0;
    ui.check("sh-pbg", rect(ix, y, iw, 30.0), "Mandar o controle com a janela em segundo plano", &mut c.pad_background);
    y += 34.0;
    ui.check("sh-pkeys", rect(ix, y, iw, 30.0), "Usar o teclado como controle", &mut c.pad_keys);
    y += 40.0;
    ui.c.text_wrap(
        rect(ix, y, iw, (l.bottom() - y - 12.0).max(0.0)),
        "No host, cada controle de quem assiste vira um controle de Xbox virtual, pelo ViGEmBus ou pelo VIIPER (o Prisma não instala nenhum dos dois). Eles saem sozinhos quando o jogo da frente tem anti-cheat. Teclado como controle: WASD é o analógico esquerdo, as setas são o direcional, J é A, K é B, U é X, I é Y, Q e E são LB e RB, Z e C os gatilhos, Enter é Start e Backspace é Back.",
        12.0,
        MUTED,
        Weight::Regular,
    );

    let r = rect(l.right() + GAP, top, w - lw - GAP, h);
    ui.card(r);
    let rx = r.x + 16.0;
    let rw = r.w - 32.0;
    ui.title(rect(rx, r.y + 12.0, rw, 22.0), "Ligados agora");
    let mut y = r.y + 44.0;
    ui.label(rect(rx, y, rw, 18.0), "Neste PC", MUTED);
    y += 24.0;
    let mine = local_pads();
    if mine.is_empty() {
        ui.c.text(rect(rx, y, rw, 22.0), "Nenhum controle ligado.", 13.0, TEXT2, Align::Left, Weight::Regular);
        y += 26.0;
    }
    for (slot, bat) in mine {
        let t = if bat.is_empty() {
            format!("{} {slot}", crate::i18n::tr("Controle"))
        } else {
            format!("{} {slot} · {}", crate::i18n::tr("Controle"), crate::i18n::tr(&bat))
        };
        ui.c.text_raw(rect(rx, y, rw, 22.0), &t, 13.0, TEXT, Align::Left, Weight::Regular);
        y += 26.0;
    }
    y += 12.0;
    ui.label(rect(rx, y, rw, 18.0), "Virtuais de quem assiste a este PC", MUTED);
    y += 24.0;
    let list = std::fs::read_to_string(stream::host::pads_path()).unwrap_or_default();
    let mut any = false;
    for line in list.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if let [who, pad, player, idle] = f[..] {
            any = true;
            let mut t = format!("{who}, {} {pad}: {} {player}", crate::i18n::tr("controle"), crate::i18n::tr("jogador"));
            if idle == "1" {
                t += &format!(" ({})", crate::i18n::tr("parado"));
            }
            ui.c.text_raw(rect(rx, y, rw, 22.0), &t, 13.0, TEXT, Align::Left, Weight::Regular);
            y += 26.0;
        }
    }
    if !any {
        ui.c.text(rect(rx, y, rw, 22.0), "Nenhum agora.", 13.0, TEXT2, Align::Left, Weight::Regular);
    }

    let pads = ["auto", "vigem", "viiper"];
    let mut cur = pads.iter().position(|p| *p == o.pads).unwrap_or(0);
    if ui.select("sh-pads", bridge_r, &opt(&["Automático", "ViGEmBus", "VIIPER"]), &mut cur) {
        o.pads = pads[cur].to_string();
    }
}

/// Pelo relógio da página, fora do desenho: a janela Compartilhar do Windows.
pub fn tick(app: &mut App) {
    let req = STATE.lock().unwrap().as_mut().and_then(|s| s.share_req.take());
    let Some((title, text, link)) = req else { return };
    let Some(hwnd) = app.main.as_ref().map(|m| m.hwnd) else { return };
    if !weblink::share_sheet(hwnd, &title, &text, &link) {
        crate::capture::copy_text(app.hub, &text);
        app.toast.info("O Windows não abriu a janela de compartilhar: o convite foi copiado.");
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
