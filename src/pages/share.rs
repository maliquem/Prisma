//! Compartilhar tela: este PC como host, os aparelhos pareados, as opções e o serviço do Windows. O
//! host, quem assiste e o pareamento rodam em processos do próprio Prisma, presos a esta janela: se
//! o Prisma fechar, eles fecham junto. O que cada um diz vai para um arquivo, que a página mostra.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Instant;

use super::*;
use crate::config::Hotkey;
use crate::stream::{self, code, weblink, ClientOpts, Opts, COPY, KEYBOARD, MIC, MOUSE, PAD, PRESETS, SOUND, SYSTEM};
use crate::sys::xbridge::Stage;

const GAP: f32 = 12.0;
/// Nomes curtos das permissões, para caberem nas caixas.
const PERMS: [(&str, u8); 7] = [("Som", SOUND), ("Controle", PAD), ("Teclado", KEYBOARD), ("Mouse", MOUSE), ("Copiar e colar", COPY), ("Tela segura", SYSTEM), ("Microfone", MIC)];
const NOTICE: &str = "Pela internet, a conexão usa o iroh (n0, licença MIT ou Apache-2.0). Sem caminho direto nem VPN, os pacotes passam pelos relés públicos da n0, sempre criptografados de ponta a ponta.";
const TABS: [&str; 6] = ["Compartilhar", "Host", "Cliente", "Rede", "Atalhos", "Controles"];

struct Proc {
    child: Child,
    log: PathBuf,
    /// O fim do log já lido e o (tamanho, data) do arquivo naquela leitura.
    seen: std::cell::RefCell<Seen>,
}

#[derive(Default)]
struct Seen {
    key: Option<(u64, std::time::SystemTime)>,
    lines: Vec<String>,
}

impl Proc {
    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

/// O convite que a janela mostra: código, convite longo e se os relés aceitaram.
#[derive(Clone, Default)]
pub(crate) struct Shown {
    pub(crate) code: String,
    long: Option<String>,
    /// None publicando, Some(true) publicado, Some(false) nenhum relé aceitou.
    pub(crate) published: Option<bool>,
    /// Até quando vale (segundos desde 1970).
    until: u64,
}

impl Shown {
    /// Segundos que faltam; 0 vencido.
    fn left(&self) -> u64 {
        self.until.saturating_sub(stream::unix_now())
    }
}

/// A linha de latência vem do registro em português: as palavras dela no idioma do Prisma.
fn lat_words(l: &str) -> String {
    let t = |s: &str| crate::i18n::tr(s).into_owned();
    l.replace(" rede ", &format!(" {} ", t("rede")))
        .replace("(mín ", &format!("({} ", t("mín")))
        .replace(", máx ", &format!(", {} ", t("máx")))
        .replace(" ponta a ponta ", &format!(" {} ", t("ponta a ponta")))
        .replace(" som ", &format!(" {} ", t("som")))
}

/// Tempo que falta, como 9:58 ou 23:59:58.
fn clock(s: u64) -> String {
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Nomes da lista de validade do convite.
fn minutes_name(m: u32) -> String {
    if m == 60 {
        crate::i18n::tr("1 hora").into_owned()
    } else {
        crate::i18n::tr("{} minutos").replace("{}", &m.to_string())
    }
}

#[derive(Default)]
struct State {
    host: Option<Proc>,
    view: Option<Proc>,
    /// O PC que a janela de assistir mostra.
    view_name: String,
    /// Mouse e teclado neste PC pareado (sem vídeo), e o nome dele.
    /// Mouse e teclado ligado, e com quais PCs (pelo id).
    kvm: Option<(Proc, Vec<String>)>,
    /// Só o som ligado, e de qual PC.
    ear: Option<(Proc, String)>,
    pair: Option<Proc>,
    invite: Option<Proc>,
    field: String,
    job: isize,
    opts: Option<Opts>,
    client: Option<ClientOpts>,
    /// Quando `cliente.txt` mudou por último: o menu da conexão também grava nele.
    client_at: Option<std::time::SystemTime>,
    /// Trabalho do serviço (instalar ou remover) rodando.
    service_busy: bool,
    tab: usize,
    /// A janela do convite aberta.
    popup: bool,
    /// O último código que a janela já mostrou sozinha.
    seen: String,
    /// O código revogado antes de vencer.
    revoked: String,
    /// Código que chegou por link, esperando a resposta da pergunta.
    link_code: Option<String>,
    /// Pedido para abrir a janela Compartilhar do Windows (fora do desenho).
    share_req: Option<(String, String, String)>,
    name_field: Option<String>,
    port_field: Option<String>,
    sound_apps: (Vec<String>, Option<Instant>),
    /// Janelas que dá para compartilhar, lidas a cada 2 s.
    share_wins: (Vec<stream::host::Shareable>, Option<Instant>),
    /// Fotos de teste: um convite de exemplo.
    test_invite: Option<Shown>,
    /// Fotos de teste: pessoas conectadas de exemplo.
    test_room: Option<Vec<stream::host::Present>>,
    /// Aparelhos pareados: 0 são os PCs que este assiste, 1 os que assistem a este.
    dev_tab: usize,
    /// Aparelho sendo renomeado: id e o texto do campo.
    renaming: Option<(String, String)>,
    /// Aba Host: as opções de qual aparelho que assiste (None: as de todos).
    host_for: Option<String>,
    /// Aba Rede: de quem são as caixas de rede. Vazio: quem assiste a este PC, todos;
    /// `v:<id>`: um aparelho que assiste; `h:<id>`: este PC assistindo a outro.
    net_for: String,
    /// Saídas e entradas de som, lidas a cada 5 s.
    sound_devs: (Vec<crate::sys::audio::Device>, Vec<crate::sys::audio::Device>, Option<Instant>),
}

/// Fotos de teste: `--test-tab=N` abre numa aba e `--test-invite` mostra a janela do convite.
fn test_setup() -> State {
    let mut s = State::default();
    if cfg!(debug_assertions) {
        for a in std::env::args() {
            if let Some(n) = a.strip_prefix("--test-tab=").and_then(|n| n.parse().ok()) {
                s.tab = n;
            }
            if let Some(n) = a.strip_prefix("--test-devtab=").and_then(|n| n.parse().ok()) {
                s.dev_tab = n;
            }
            if a == "--test-room" {
                use stream::host::Present;
                s.test_room = Some(vec![
                    Present { key: "a1b2".into(), perms: SOUND | PAD, route: "direto pela internet, 203.0.113.7:4500, 14 ms".into(), name: "Notebook da sala".into() },
                    Present { key: "c3d4".into(), perms: crate::stream::TOTAL, route: "pelo relé da n0 (https://use1-1.relay.n0.iroh.link./), 48 ms".into(), name: "PC do escritório".into() },
                    Present { key: "e5f6".into(), perms: SOUND | KEYBOARD, route: "direto na rede local, 192.168.0.20:51000, 2 ms".into(), name: "Tablet".into() },
                ]);
            }
            if a == "--test-invite" {
                s.test_invite = Some(Shown { code: "K7M2-QX9P".into(), long: Some("PRISMA-EXEMPLO".into()), published: Some(true), until: stream::unix_now() + 598 });
                s.seen = "K7M2-QX9P".into();
                s.popup = true;
            }
        }
    }
    s
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// As opções mudaram fora da página (pelo Prisma Launcher): a página lê de novo do arquivo.
pub fn reload_opts() {
    if let Some(st) = STATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        st.opts = None;
    }
}

/// Um trabalho preso à janela: o Windows fecha os processos do trabalho quando o Prisma fecha.
fn job(st: &mut State) -> windows::Win32::Foundation::HANDLE {
    use windows::Win32::System::JobObjects::*;
    if st.job == 0 {
        unsafe {
            if let Ok(j) = CreateJobObjectW(None, None) {
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                // BREAKAWAY_OK: a cópia vigia da tela virtual (`vdd::watch`) sai do trabalho e
                // sobrevive ao Prisma, para devolver as telas daqui.
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
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
    Some(Proc { child, log, seen: Default::default() })
}

/// Fim do arquivo de um processo, em linhas.
fn tail(p: &Option<Proc>) -> Vec<String> {
    p.as_ref().map(tail_of).unwrap_or_default()
}

fn tail_of(p: &Proc) -> Vec<String> {
    read_tail(&p.log, &mut p.seen.borrow_mut())
}

/// Roda a cada quadro para até 6 logs: só relê quando tamanho ou data mudam, e só os últimos 16 KB,
/// porque o log cresce a sessão inteira.
fn read_tail(path: &std::path::Path, seen: &mut Seen) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    const TAIL: u64 = 16 * 1024;
    let Ok(mut f) = std::fs::File::open(path) else { return Vec::new() };
    let Ok(meta) = f.metadata() else { return Vec::new() };
    let key = meta.modified().ok().map(|t| (meta.len(), t));
    if key.is_some() && key == seen.key {
        return seen.lines.clone();
    }
    let skip = meta.len().saturating_sub(TAIL);
    let mut b = Vec::new();
    if f.seek(SeekFrom::Start(skip)).and_then(|_| f.read_to_end(&mut b)).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&b);
    // Cortado no meio, a primeira linha vem pela metade.
    let whole = if skip > 0 { text.split_once('\n').map_or("", |(_, r)| r) } else { &text };
    seen.lines = whole.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect();
    seen.key = key;
    seen.lines.clone()
}

/// O convite mais novo que o processo do convite escreveu.
pub(crate) fn invite_of(lines: &[String]) -> Option<Shown> {
    let at = lines.iter().rposition(|l| l.starts_with(stream::CODE_LINE))?;
    let rest = &lines[at..];
    let code = rest[0].trim_start_matches(stream::CODE_LINE).trim().to_string();
    let long = rest.iter().map(|l| l.trim()).find(|l| l.starts_with("PRISMA-")).map(str::to_string);
    // A validade vem logo antes do código.
    let until = lines[..at].iter().rev().take(2).find_map(|l| l.strip_prefix(stream::EXPIRES_LINE)).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    let published = if rest.iter().any(|l| l.starts_with(stream::CODE_OK)) {
        Some(true)
    } else if rest.iter().any(|l| l.starts_with(stream::CODE_FAIL)) {
        Some(false)
    } else {
        None
    };
    Some(Shown { code, long, published, until })
}

/// Cria um convite como o botão "Criar convite": liga o host se nem o serviço nem o host
/// estiverem ligados, para o convite anterior e abre o novo. Devolve o registro do convite e o
/// processo que o escreve. `minutes` troca a validade da página só neste convite (o
/// `--stream-invite` limita de 1 a 60).
pub fn create_invite(_app: &mut App, minutes: Option<u32>) -> Option<(PathBuf, u32)> {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let st = guard.get_or_insert_with(test_setup);
    let mut opts = st.opts.get_or_insert_with(Opts::load).clone();
    if let Some(m) = minutes {
        opts.invite_min = m;
    }
    invite_in(st, &opts)
}

fn invite_in(st: &mut State, opts: &Opts) -> Option<(PathBuf, u32)> {
    if !stream::service::installed() && !st.host.as_mut().is_some_and(Proc::alive) {
        st.host = spawn(st, &["--stream-share".to_string()], "host.log");
    }
    stop(&mut st.invite);
    st.invite = spawn(st, &[format!("--stream-invite={},{}", stream::perms_key(opts.invite), opts.invite_min)], "convite.log");
    st.revoked.clear();
    st.invite.as_ref().map(|p| (p.log.clone(), p.child.id()))
}

fn stop(p: &mut Option<Proc>) {
    if let Some(mut x) = p.take() {
        // Só o processo que esta página abriu, pelo identificador dele.
        let _ = x.child.kill();
        let _ = x.child.wait();
    }
}

fn stop_kvm(st: &mut State) {
    let mut p = st.kvm.take().map(|k| k.0);
    stop(&mut p);
    // Morto à força, ele não devolve o cursor que escondeu: devolve aqui, antes de o Prisma sair.
    stream::cursor::restore_orphan();
}

/// Põe um PC no arranjo das telas, se ele ainda não está: ao lado deste PC, primeiro à
/// direita. Falso quando não sobra posição.
fn place(screens: &mut [String; 3], id: &str) -> bool {
    if screens.iter().any(|s| s == id) {
        return true;
    }
    let me = screens.iter().position(|s| s == "eu").unwrap_or(1) as i32;
    let free = [me + 1, me - 1, me + 2, me - 2].into_iter().filter(|k| (0..3).contains(k)).map(|k| k as usize).find(|&k| screens[k].is_empty());
    match free {
        Some(k) => {
            screens[k] = id.to_string();
            true
        }
        None => false,
    }
}

fn stop_ear(st: &mut State) {
    let mut p = st.ear.take().map(|k| k.0);
    stop(&mut p);
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

/// Linha de abas sob o título. Com alguém conectado, aparece a aba Conectados, com quantos são.
fn tabs(ui: &mut Ui, x: f32, y: f32, sel: &mut usize, present: usize) {
    let mut names: Vec<String> = TABS.iter().map(|n| crate::i18n::tr(n).into_owned()).collect();
    if present > 0 {
        names.push(format!("{} ({present})", crate::i18n::tr("Conectados")));
    }
    if *sel >= names.len() {
        *sel = 0;
    }
    ui.tabs("sh-tab", x, y, &names, sel, true, Some(TABS.len()));
}

/// Aba Conectados: cada pessoa na sala, com o acesso que muda na hora, desconectar e banir.
#[allow(clippy::too_many_arguments)]
fn room_tab(ui: &mut Ui, app: &mut App, present: &[stream::host::Present], x: f32, top: f32, w: f32, h: f32) {
    let c = rect(x, top, w, h);
    ui.card(c);
    let ix = c.x + 16.0;
    let iw = c.w - 32.0;
    ui.title(rect(ix, c.y + 12.0, iw, 22.0), "Conectados agora");
    let mut y = c.y + 44.0;
    let devices = stream::device_rows();
    const ROW: f32 = 64.0;
    let note_h = 52.0;
    let content = (present.len() as f32 * ROW).max(1.0);
    let list = rect(ix, y, iw, content.min((c.bottom() - y - note_h - 12.0).max(ROW)));
    let names: Vec<String> = PRESETS.iter().map(|p| crate::i18n::tr(p.1).into_owned()).chain([crate::i18n::tr("Personalizado").into_owned()]).collect();
    let btn = 112.0;
    let sel_w = 260.0f32.min(iw * 0.34);
    let mut picks: Vec<(usize, Rect)> = Vec::new();
    let mut kick: Option<usize> = None;
    let mut ban: Option<usize> = None;
    ui.begin_scroll("sh-room", list, content);
    for (i, p) in present.iter().enumerate() {
        let row = rect(ix, y, iw, ROW - 8.0);
        ui.c.round(row, 8.0, CARD2);
        let px = row.x + 12.0;
        let right = row.right() - 12.0;
        let ban_r = rect(right - btn, row.y + 12.0, btn, 32.0);
        let kick_r = rect(ban_r.x - 8.0 - btn, row.y + 12.0, btn, 32.0);
        let sel_r = rect(kick_r.x - 12.0 - sel_w, row.y + 10.0, sel_w, 36.0);
        let tw = (sel_r.x - 12.0 - px).max(40.0);
        let shown = app.cfg.share_names.iter().find(|n| n.0 == p.key).map_or(p.name.as_str(), |n| n.1.as_str());
        ui.c.text_raw(rect(px, row.y + 8.0, tw, 20.0), shown, 14.0, TEXT, Align::Left, Weight::Semibold);
        ui.c.text_raw(rect(px, row.y + 30.0, tw, 18.0), &stream::route_label(&p.route), 12.0, MUTED, Align::Left, Weight::Regular);
        if ui.button(&format!("sh-kick{i}"), kick_r, "Desconectar", false) {
            kick = Some(i);
        }
        let paired = devices.iter().any(|d| d.viewer && d.id == p.key);
        if ui.button_ex(&format!("sh-ban{i}"), ban_r, "Banir", false, paired) {
            ban = Some(i);
        }
        picks.push((i, sel_r));
        y += ROW;
    }
    ui.end_scroll();
    ui.c.text_wrap(
        rect(ix, list.bottom() + 4.0, iw, note_h),
        "O acesso muda na hora e fica salvo para as próximas conexões. Desconectar derruba só esta conexão: o aparelho continua pareado. Banir derruba e esquece o aparelho, que só volta com um convite novo.",
        12.0,
        MUTED,
        Weight::Regular,
    );
    // Listas por último, para abrirem por cima das linhas de baixo.
    for (i, r) in picks {
        let p = &present[i];
        let mut s = PRESETS.iter().position(|q| q.2 == p.perms).unwrap_or(PRESETS.len());
        if ui.select(&format!("sh-acc{i}"), r, &names, &mut s) && s < PRESETS.len() {
            let perms = PRESETS[s].2;
            let mut ok = stream::host::ask_room(&format!("acesso {} {perms}", p.key)).is_ok();
            if let Some(d) = devices.iter().find(|d| d.viewer && d.id == p.key) {
                ok &= stream::set_device(&d.id, perms, d.ask).is_ok();
            }
            if !ok {
                app.toast.info("Não deu para salvar. Com o serviço, abra o Prisma como administrador.");
            }
        }
    }
    if let Some(i) = kick {
        if stream::host::ask_room(&format!("desconectar {}", present[i].key)).is_err() {
            app.toast.info("Não deu para salvar. Com o serviço, abra o Prisma como administrador.");
        }
    }
    if let Some(i) = ban {
        let p = &present[i];
        ui.ask(&format!("sh-ban-{}", p.key), "Banir este aparelho?", &format!("«{}» sai agora e vai precisar de um convite novo para conectar.", p.name));
    }
    for p in present {
        if ui.answered(&format!("sh-ban-{}", p.key)) {
            let a = stream::host::ask_room(&format!("desconectar {}", p.key));
            let b = stream::forget_device(true, &p.key);
            if a.is_err() || b.is_err() {
                app.toast.info("Não deu para salvar. Com o serviço, abra o Prisma como administrador.");
            }
        }
    }
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    let mut guard = STATE.lock().unwrap();
    let st = guard.get_or_insert_with(test_setup);
    let opts = st.opts.get_or_insert_with(Opts::load).clone();
    // O menu da conexão gravou outra coisa: a aba mostra o mesmo que ele.
    let at = std::fs::metadata(ClientOpts::path()).and_then(|m| m.modified()).ok();
    if at != st.client_at {
        st.client_at = at;
        st.client = None;
    }
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
    // Quem está conectado: o host grava a lista a cada mudança.
    let service = stream::service::installed();
    let hosting = st.host.as_mut().is_some_and(Proc::alive);
    let present = match &st.test_room {
        Some(r) => r.clone(),
        None if service || hosting => stream::host::room(),
        None => Vec::new(),
    };
    tabs(ui, x, 84.0, &mut st.tab, present.len());
    let top = 128.0;
    let h = a.h - top - 20.0;
    match st.tab {
        6 => room_tab(ui, app, &present, x, top, w, h),
        1 => host_tab(ui, app, st, &mut new_opts, x, top, w, h),
        2 => client_tab(ui, app, &mut new_client, x, top, w, h),
        3 => net_tab(ui, app, st, &mut new_opts, x, top, w, h),
        4 => keys_tab(ui, app, &mut new_opts, &mut new_client, x, top, w, h),
        5 => pads_tab(ui, &mut new_opts, &mut new_client, x, top, w, h),
        _ => main_tab(ui, app, st, &opts, &mut new_opts, &mut new_client, &shown, x, top, w, h),
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
    let left = s.left();
    let revoked = st.revoked == s.code;
    let dead = revoked || left == 0;
    let (status, color) = if revoked {
        (crate::i18n::tr("Revogado: o código não vale mais.").into_owned(), WARN)
    } else if left == 0 {
        (crate::i18n::tr("Vencido. Crie um convite novo.").into_owned(), WARN)
    } else {
        match s.published {
            None => (crate::i18n::tr("Publicando o código nos relés...").into_owned(), MUTED),
            Some(true) => (crate::i18n::tr("Pronto. Vale uma vez e vence em {}.").replace("{}", &clock(left)), ACCENT),
            Some(false) => (crate::i18n::tr("Os relés não responderam: mande o convite longo.").into_owned(), WARN),
        }
    };
    ui.c.text_raw(rect(ix, b.y + 112.0, iw, 20.0), &status, 13.0, color, Align::Center, Weight::Regular);
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
    let ready = s.published != Some(false) && !dead;
    let note = crate::i18n::tr("Vale uma vez e vence em {}.").replace("{}", &clock(left));
    if ui.button_ex("sh-c-code", rect(ix, y, bw, 40.0), "Copiar código", false, ready) {
        crate::capture::copy_text(app.hub, &s.code);
        app.toast.info(&format!("{} {note}", crate::i18n::tr("Código copiado.")));
    }
    if ui.button_ex("sh-c-link", rect(ix + bw + GAP, y, bw, 40.0), "Copiar link", false, ready) {
        crate::capture::copy_text(app.hub, &link);
        app.toast.info(&format!("{} {note}", crate::i18n::tr("Link copiado.")));
    }
    if ui.button_ex("sh-c-share", rect(ix + 2.0 * (bw + GAP), y, bw, 40.0), "Compartilhar", true, ready) {
        let text = format!("{} {}", crate::i18n::tr("Convite do Prisma para ver este PC:"), s.code);
        st.share_req = Some((crate::i18n::tr("Convite do Prisma").into_owned(), format!("{text}\n{link}"), link.clone()));
    }
    let y2 = b.bottom() - 64.0;
    let close_w = 112.0;
    let revoke_w = (crate::ui::gfx::measure("Revogar", 14.0, Weight::Semibold, None).0 + 32.0).max(96.0);
    if let Some(long) = &s.long {
        let long_w = (iw - close_w - revoke_w - 2.0 * GAP).min(220.0);
        if ui.button_ex("sh-c-long", rect(ix, y2, long_w, 40.0), "Copiar convite longo", false, !dead) {
            crate::capture::copy_text(app.hub, long);
            app.toast.info(&format!("{} {note}", crate::i18n::tr("Convite copiado.")));
        }
    }
    // Revogar: o host passa a recusar o código na hora, antes de vencer.
    if ui.button_ex("sh-c-revoke", rect(b.right() - 24.0 - close_w - GAP - revoke_w, y2, revoke_w, 40.0), "Revogar", false, !dead && st.test_invite.is_none()) {
        match stream::revoke_invite() {
            Ok(()) => {
                stop(&mut st.invite);
                st.revoked = s.code.clone();
                app.toast.info("Convite revogado: o código não vale mais.");
            }
            Err(_) => app.toast.info("Não deu para revogar. Com o serviço, abra o Prisma como administrador."),
        }
    }
    if ui.button("sh-c-close", rect(b.right() - 24.0 - close_w, y2, close_w, 40.0), "Fechar", false) {
        st.popup = false;
    }
}

#[allow(clippy::too_many_arguments)]
fn main_tab(ui: &mut Ui, app: &mut App, st: &mut State, opts: &Opts, new_opts: &mut Opts, client: &mut ClientOpts, shown: &Option<Shown>, x: f32, top: f32, w: f32, h: f32) {
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
    let room: Vec<String> = match &st.test_room {
        Some(r) => r.iter().map(|p| p.name.clone()).collect(),
        None if service || hosting => stream::host::room().into_iter().map(|p| p.name).collect(),
        None => Vec::new(),
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
    // A tela inteira (o monitor da aba Host) ou só uma janela; a lista abre no fim.
    ui.label(rect(ix, y, iw, 18.0), "O que as pessoas veem", TEXT2);
    let src_y = y + 20.0;
    y += 70.0;
    // Validade do convite ao lado das permissões.
    let min_w = 150.0;
    ui.label(rect(ix, y, iw - min_w - GAP, 18.0), "Quem parear agora pode", TEXT2);
    ui.label(rect(ix + iw - min_w, y, min_w, 18.0), "Convite vale", TEXT2);
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
        invite_in(st, opts);
    }
    y += 48.0;
    match shown {
        Some(s) => {
            let when = if st.revoked == s.code {
                crate::i18n::tr("revogado").into_owned()
            } else if s.left() == 0 {
                crate::i18n::tr("vencido").into_owned()
            } else {
                crate::i18n::tr("vence em {}").replace("{}", &clock(s.left()))
            };
            let t = format!("{} {} ({when})", crate::i18n::tr("Último convite:"), s.code);
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
    let kvm_log = st.kvm.as_ref().map(|k| tail_of(&k.0)).unwrap_or_default();
    let ear_log = st.ear.as_ref().map(|k| tail_of(&k.0)).unwrap_or_default();
    let view_log = tail(&st.view);
    let side: Vec<String> = tail(&st.pair).into_iter().chain(view_log.iter().cloned()).chain(kvm_log.iter().cloned()).chain(ear_log.iter().cloned()).filter(|l| !l.starts_with("Conexão pela internet")).collect();
    if let Some(last) = side.last() {
        ui.c.text_wrap(rect(rx, y, rw, 36.0), last, 12.0, TEXT2, Weight::Regular);
    }
    y += 40.0;

    // Latência de cada modo ligado: a última linha de cada um, por PC, e as últimas de todos.
    let watching = st.view.as_mut().is_some_and(Proc::alive).then(|| st.view_name.clone());
    let lat_of = |lines: &[String]| -> Vec<String> { lines.iter().filter_map(|l| l.strip_prefix(stream::LAT_LINE)).map(lat_words).collect() };
    let view_lat = if watching.is_some() { lat_of(&view_log) } else { Vec::new() };
    let kvm_lat = lat_of(&kvm_log);
    let ear_lat = lat_of(&ear_log);
    let mut lat_log: Vec<String> = Vec::new();
    for (mode, lines) in [("Assistir", &view_lat), ("Mouse e teclado", &kvm_lat), ("Só o som", &ear_lat)] {
        lat_log.extend(lines.iter().map(|l| format!("{} · {}", crate::i18n::tr(mode), l)));
    }
    // As linhas começam pela hora: em ordem, as últimas 40.
    lat_log.sort_by(|a, b| a.split(" · ").nth(1).cmp(&b.split(" · ").nth(1)));
    let skip = lat_log.len().saturating_sub(40);
    lat_log.drain(..skip);
    // Tira a hora e o nome do PC da linha, para mostrar ao lado do nome.
    let short = |l: &String| {
        let rest = l.split_once(' ').map_or(l.as_str(), |x| x.1);
        rest.rsplit("» ").next().unwrap_or(rest).to_string()
    };

    let all = stream::device_rows();
    // Duas abas: os PCs que este assiste e os que assistem a este.
    let mut tx = rx;
    for (k, name) in ["Este PC assiste", "Assistem a este PC"].iter().enumerate() {
        let label = format!("{} ({})", crate::i18n::tr(name), all.iter().filter(|d| d.viewer == (k == 1)).count());
        let tw = crate::ui::gfx::measure(&label, 13.0, Weight::Semibold, None).0 + 24.0;
        let tr_ = rect(tx, y, tw, 30.0);
        let on = st.dev_tab == k;
        if on {
            ui.c.round(tr_, 8.0, CARD2);
        }
        if ui.hit(&format!("sh-dtab{k}"), tr_) {
            st.dev_tab = k;
            st.renaming = None;
        }
        ui.c.text_raw(tr_, &label, 13.0, if on { TEXT } else { MUTED }, Align::Center, if on { Weight::Semibold } else { Weight::Regular });
        tx += tw + 4.0;
    }
    y += 38.0;
    let rows: Vec<_> = all.iter().filter(|d| d.viewer == (st.dev_tab == 1)).collect();
    // Mouse e teclado ligados (o processo ainda roda) em qual PC.
    let kvm_on: Vec<String> = st.kvm.as_mut().and_then(|k| matches!(k.0.child.try_wait(), Ok(None)).then(|| k.1.clone())).unwrap_or_default();
    let ear_on = st.ear.as_mut().and_then(|k| matches!(k.0.child.try_wait(), Ok(None)).then(|| k.1.clone()));
    // Botões da direita: Esquecer, Renomear e, nos PCs que este assiste, Assistir, Mouse e teclado
    // e Só o som. A largura segue o texto, que em outros idiomas é mais comprido; sem espaço para
    // o nome ao lado, os botões descem para uma linha própria.
    let bw = |s: &str| (crate::ui::gfx::measure(s, 14.0, Weight::Semibold, None).0 + 28.0).max(84.0);
    let (forget_w, watch_bw, kvm_bw, ear_bw) = (bw("Esquecer"), bw("Assistir").max(bw("Ligar")), bw("Mouse e teclado").max(bw("Parar mouse e teclado")), bw("Só o som").max(bw("Parar o som")));
    let rn_max = bw("Renomear").max(bw("Salvar"));
    let extra_bw = kvm_bw + ear_bw + 8.0;
    let all_buttons = forget_w + rn_max + if st.dev_tab == 0 { watch_bw + extra_bw + 16.0 } else { 0.0 } + 16.0;
    let stack = rw - 24.0 - all_buttons - 8.0 < 160.0;
    let drop = if stack { 44.0 } else { 0.0 };
    // Nem os botões sozinhos cabem numa linha: Mouse e teclado e Só o som descem para outra.
    let kvm_down = rw - 24.0 < all_buttons;
    let row_v = 158.0 + drop;
    let row_h = 64.0 + drop + if kvm_down { 44.0 } else { 0.0 };
    // A latência de um PC com modo ligado ganha uma linha no cartão dele.
    let lat_line = |d: &stream::DeviceRow| -> Option<String> {
        if d.viewer {
            return None;
        }
        let mut parts: Vec<String> = Vec::new();
        if watching.as_deref() == Some(d.name.as_str()) {
            if let Some(l) = view_lat.last() {
                parts.push(format!("{} {}", crate::i18n::tr("Assistindo:"), short(l)));
            }
        }
        if kvm_on.contains(&d.id) {
            let mine: Vec<&String> = kvm_lat.iter().filter(|l| l.contains(&format!("«{}»", d.name))).collect();
            if let Some(l) = mine.last() {
                parts.push(format!("{} {}", crate::i18n::tr("Mouse e teclado:"), short(l)));
            }
        }
        if ear_on.as_deref() == Some(d.name.as_str()) {
            if let Some(l) = ear_lat.last() {
                parts.push(format!("{} {}", crate::i18n::tr("Som:"), short(l)));
            }
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    };
    let height_of = |d: &stream::DeviceRow| if d.viewer { row_v } else { row_h + if lat_line(d).is_some() { 20.0 } else { 0.0 } };
    // O registro de latência fica embaixo da lista, quando algum modo está ligado.
    let log_h = if lat_log.is_empty() { 0.0 } else { 132.0 };
    let list = rect(rx, y, rw, r.bottom() - y - 12.0 - log_h);
    let content: f32 = rows.iter().map(|d| height_of(d)).sum::<f32>().max(1.0);
    if all.is_empty() {
        ui.c.text_wrap(rect(rx, y, rw, 60.0), "Nenhum aparelho pareado. Crie um convite neste PC e digite o código no outro, ou o contrário.", 13.0, MUTED, Weight::Regular);
    } else if rows.is_empty() {
        let empty = if st.dev_tab == 0 { "Nenhum PC para assistir. Digite aqui o código de um convite feito no outro PC." } else { "Nenhum PC assiste a este. Crie um convite e digite o código no outro PC." };
        ui.c.text_wrap(rect(rx, y, rw, 60.0), empty, 13.0, MUTED, Weight::Regular);
    }
    let mut renamed: Option<(String, String)> = None;
    let mut changes: Vec<(String, u8, bool)> = Vec::new();
    let mut forget: Option<(String, String)> = None;
    let mut watch: Option<String> = None;
    let mut kvm: Option<(String, bool)> = None;
    let mut ear: Option<(String, bool)> = None;
    let mut wake: Option<(String, String)> = None;
    ui.begin_scroll("sh-list", list, content);
    let mut yy = list.y;
    for (i, d) in rows.iter().enumerate() {
        let rh = height_of(d);
        let row = rect(rx, yy, rw, rh - 8.0);
        if let Some(l) = lat_line(d) {
            ui.c.text_raw(rect(row.x + 12.0, row.bottom() - 26.0, row.w - 24.0, 18.0), &l, 12.0, ACCENT, Align::Left, Weight::Regular);
        }
        ui.c.round(row, 8.0, CARD2);
        let px = row.x + 12.0;
        let pw = row.w - 24.0;
        let editing = st.renaming.as_ref().is_some_and(|e| e.0 == d.id);
        let rn_w = bw(if editing { "Salvar" } else { "Renomear" });
        let watch_w = if d.viewer {
            0.0
        } else if kvm_down {
            watch_bw + 8.0
        } else {
            watch_bw + extra_bw + 16.0
        };
        let name_w = if stack { pw } else { (pw - forget_w - rn_w - watch_w - 24.0).max(60.0) };
        let by = row.y + 10.0 + drop;
        let alias = app.cfg.share_names.iter().find(|n| n.0 == d.id).map(|n| n.1.clone());
        if editing {
            let field = &mut st.renaming.as_mut().unwrap().1;
            ui.search(&format!("sh-rn{i}"), rect(px, row.y + 6.0, name_w, 36.0), field, &d.name);
        } else {
            ui.c.text_raw(rect(px, row.y + 8.0, name_w, 20.0), alias.as_deref().unwrap_or(&d.name), 14.0, TEXT, Align::Left, Weight::Semibold);
            // Com outro nome, o original fica embaixo, menor e mais claro.
            if alias.is_some() {
                ui.c.text_raw(rect(px, row.y + 29.0, name_w, 16.0), &d.name, 11.0, MUTED, Align::Left, Weight::Regular);
            }
            // Ligado ou não, na linha de baixo (depois do nome original, se houver outro nome).
            if !d.viewer {
                let (label, color) = match stream::presence_of(&d.id) {
                    stream::Presence::Online => ("Online", ACCENT),
                    stream::Presence::Offline => ("Offline", MUTED),
                    stream::Presence::Checking => ("Verificando", MUTED),
                };
                let sx = px + if alias.is_some() { crate::ui::gfx::measure(&d.name, 11.0, Weight::Regular, None).0 + 10.0 } else { 0.0 };
                if sx + 40.0 < px + name_w {
                    ui.c.round(rect(sx, row.y + 34.0, 7.0, 7.0), 3.5, color);
                    ui.c.text_raw(rect(sx + 12.0, row.y + 29.0, px + name_w - sx - 12.0, 16.0), &crate::i18n::tr(label), 11.0, color, Align::Left, Weight::Regular);
                }
            }
        }
        if ui.button(&format!("sh-forget{i}"), rect(row.right() - 12.0 - forget_w, by, forget_w, 32.0), "Esquecer", false) {
            forget = Some((d.id.clone(), d.name.clone()));
        }
        let rn_r = rect(row.right() - 20.0 - forget_w - rn_w, by, rn_w, 32.0);
        if editing {
            if ui.button(&format!("sh-rn-ok{i}"), rn_r, "Salvar", true) {
                renamed = st.renaming.take();
            }
        } else if ui.button(&format!("sh-rn-go{i}"), rn_r, "Renomear", false) {
            st.renaming = Some((d.id.clone(), alias.clone().unwrap_or_default()));
        }
        if d.viewer {
            let cw = (pw / 3.0).floor();
            let mut perms = d.perms;
            for (k, (name, bit)) in PERMS.iter().enumerate() {
                let cx = px + (k % 3) as f32 * cw;
                let cy = row.y + 50.0 + drop + (k / 3) as f32 * 26.0;
                let mut on = perms & bit != 0;
                if ui.check(&format!("sh-p{i}-{k}"), rect(cx, cy, cw - 4.0, 26.0), name, &mut on) {
                    perms = if on { perms | bit } else { perms & !bit };
                }
            }
            let mut ask = d.ask;
            let changed_ask = ui.check(&format!("sh-ask{i}"), rect(px, row.y + 126.0 + drop, pw, 24.0), "Perguntar a cada conexão", &mut ask);
            if perms != d.perms || changed_ask {
                changes.push((d.id.clone(), perms, ask));
            }
        } else {
            // Desligado e com o MAC guardado: no lugar de Assistir, Ligar.
            let watch_r = rect(rn_r.x - watch_bw - 8.0, by, watch_bw, 32.0);
            if d.wake && stream::presence_of(&d.id) == stream::Presence::Offline {
                if ui.button(&format!("sh-wake{i}"), watch_r, "Ligar", true) {
                    wake = Some((d.id.clone(), alias.clone().unwrap_or_else(|| d.name.clone())));
                }
            } else if ui.button(&format!("sh-watch{i}"), watch_r, "Assistir", true) {
                watch = Some(d.name.clone());
            }
            let on = kvm_on.contains(&d.id);
            let (kvm_r, ear_r) = if kvm_down {
                let ear_r = rect(row.right() - 12.0 - ear_bw, by + 44.0, ear_bw, 32.0);
                (rect(ear_r.x - 8.0 - kvm_bw, by + 44.0, kvm_bw, 32.0), ear_r)
            } else {
                let kvm_r = rect(rn_r.x - watch_w, by, kvm_bw, 32.0);
                (kvm_r, rect(kvm_r.right() + 8.0, by, ear_bw, 32.0))
            };
            // Ligado: o botão fica na cor de destaque e diz o que para.
            if ui.button(&format!("sh-kvm{i}"), kvm_r, if on { "Parar mouse e teclado" } else { "Mouse e teclado" }, on) {
                kvm = Some((d.id.clone(), !on));
            }
            let hearing = ear_on.as_deref() == Some(d.name.as_str());
            if ui.button(&format!("sh-ear{i}"), ear_r, if hearing { "Parar o som" } else { "Só o som" }, hearing) {
                ear = Some((d.name.clone(), !hearing));
            }
        }
        yy += rh;
    }
    ui.end_scroll();
    if !lat_log.is_empty() {
        let ly = list.bottom() + 8.0;
        ui.label(rect(rx, ly, rw, 18.0), "Latência", TEXT2);
        log_view(ui, "sh-lat-log", rect(rx, ly + 22.0, rw, log_h - 30.0), &lat_log);
    }
    // Nome vazio volta ao original.
    if let Some((id, name)) = renamed {
        let name: String = name.chars().filter(|c| !c.is_control() && *c != '|').take(40).collect();
        app.cfg.share_names.retain(|n| n.0 != id);
        if !name.trim().is_empty() {
            app.cfg.share_names.push((id, name.trim().to_string()));
        }
        app.cfg.save();
    }
    for (id, perms, ask) in changes {
        if stream::set_device(&id, perms, ask).is_err() {
            app.toast.info("Não deu para salvar. Com o serviço, abra o Prisma como administrador.");
        }
        // Conectado agora: vale na hora, sem reconectar.
        if stream::host::room().iter().any(|p| p.key == id) && stream::host::ask_room(&format!("acesso {id} {perms}")).is_err() {
            app.toast.info("Não deu para mudar na hora. Vale na próxima conexão.");
        }
    }
    if let Some((id, name)) = forget {
        ui.ask(&format!("sh-forget-{id}"), "Esquecer este aparelho?", &format!("«{name}» vai precisar de um convite novo para conectar."));
    }
    for d in &rows {
        if ui.answered(&format!("sh-forget-{}", d.id)) {
            // Se ele está assistindo agora, sai da sala também.
            if d.viewer && stream::host::room().iter().any(|p| p.key == d.id) {
                let _ = stream::host::ask_room(&format!("desconectar {}", d.id));
            }
            if stream::forget_device(d.viewer, &d.id).is_err() {
                app.toast.info("Não deu para salvar. Com o serviço, abra o Prisma como administrador.");
            } else if !all.iter().any(|o| o.id == d.id && o.viewer != d.viewer) {
                // O nome dado sai junto, a menos que o mesmo aparelho continue na outra aba.
                app.cfg.share_names.retain(|n| n.0 != d.id);
                app.cfg.save();
            }
        }
    }
    if let Some((id, name)) = wake {
        ui.ask(
            &format!("sh-wake-{id}"),
            "Ligar este PC?",
            &format!("O Prisma manda o pacote de Wake on LAN para «{name}» pela rede. A placa-mãe e o adaptador de rede dele precisam estar com o Wake on LAN ligado (na BIOS e nas propriedades do adaptador)."),
        );
    }
    for d in &rows {
        if ui.answered(&format!("sh-wake-{}", d.id)) {
            stream::wake_device(&d.id);
        }
    }
    if let Some(name) = watch {
        stop(&mut st.view);
        st.view = spawn(st, &[format!("--stream-connect={name}")], "assistir.log");
        st.view_name = name;
    }
    // Um PC entra ou sai do Mouse e teclado: a conexão recomeça com os PCs que ficam.
    if let Some((id, start)) = kvm {
        let mut ids = kvm_on.clone();
        ids.retain(|i| *i != id);
        let mut ok = true;
        if start {
            ok = place(&mut client.screens, &id);
            if ok {
                ids.push(id);
            } else {
                app.toast.info("As três posições já têm tela. Mude em Cliente > Telas lado a lado.");
            }
        }
        if ok {
            stop_kvm(st);
            if !ids.is_empty() {
                let mut arg: Vec<String> = Vec::new();
                for (k, who) in client.screens.iter().enumerate() {
                    if who == "eu" || ids.contains(who) {
                        arg.push(format!("{who}|{k}"));
                    }
                }
                st.kvm = spawn(st, &[format!("--stream-mouse={}", arg.join(","))], "mouse-teclado.log").map(|p| (p, ids));
            }
        }
    }
    if let Some((name, start)) = ear {
        stop_ear(st);
        if start {
            st.ear = spawn(st, &[format!("--stream-sound={name}")], "so-o-som.log").map(|p| (p, name));
        }
    }

    // Listas por último, para abrirem por cima do resto.
    let names: Vec<&str> = PRESETS.iter().rev().map(|p| p.1).collect();
    let mut s = PRESETS.iter().rev().position(|p| p.2 == opts.invite).unwrap_or(0);
    if ui.select("sh-preset", rect(ix, sel_y, iw - min_w - GAP, 40.0), &opt(&names), &mut s) {
        new_opts.invite = PRESETS.iter().rev().nth(s).map_or(SOUND, |p| p.2);
    }
    let mins: Vec<String> = stream::INVITE_MINUTES.iter().map(|m| minutes_name(*m)).collect();
    let mut m = stream::INVITE_MINUTES.iter().position(|x| *x == opts.invite_min).unwrap_or(1);
    if ui.select("sh-inv-min", rect(ix + iw - min_w, sel_y, min_w, 40.0), &mins, &mut m) {
        new_opts.invite_min = stream::INVITE_MINUTES[m];
    }

    // Tela inteira ou uma janela aberta. Uma escolha de janela que já fechou volta para a tela inteira.
    if st.share_wins.1.is_none_or(|t| t.elapsed().as_secs() >= 2) {
        st.share_wins = (stream::host::share_windows(), Some(Instant::now()));
    }
    let chosen = stream::host::chosen_window(opts.window);
    if opts.window.0 != 0 && chosen.is_none() {
        new_opts.window = (0, 0);
    }
    let short = |t: &str| if t.chars().count() > 60 { format!("{}…", t.chars().take(59).collect::<String>()) } else { t.to_string() };
    let only = crate::i18n::tr("Só a janela:").into_owned();
    let mut vals = vec![(0u64, 0u32)];
    let mut names = vec![crate::i18n::tr("Tela inteira").into_owned()];
    for w in &st.share_wins.0 {
        vals.push((w.hwnd, w.pid));
        names.push(format!("{only} {}", short(&w.title)));
    }
    if let Some(h) = chosen.filter(|_| !vals.contains(&opts.window)) {
        vals.push(opts.window);
        names.push(format!("{only} {}", short(&crate::sys::procs::window_title(h))));
    }
    let mut s = vals.iter().position(|v| *v == new_opts.window).unwrap_or(0);
    if ui.select("sh-src", rect(ix, src_y, iw, 40.0), &names, &mut s) {
        new_opts.window = vals[s];
    }
}

/// Valores das listas do host, com o 0 como automático.
const RES: [(u32, &str); 5] = [(0, "Automático"), (720, "720p"), (1080, "1080p"), (1440, "1440p"), (2160, "2160p (4K)")];
const FPS: [u32; 8] = [0, 30, 60, 90, 120, 144, 165, 240];
/// Acima de [`stream::NV_MAX_MBPS`], só com o PyroWave.
const MBPS: [u32; 13] = [0, 5, 10, 20, 30, 50, 80, 120, 150, 200, 300, 500, 700];

/// As placas de vídeo deste PC, lidas de novo a cada 10 s (uma placa externa pode chegar).
fn gpu_names() -> Vec<String> {
    static CACHE: std::sync::Mutex<(Vec<String>, Option<Instant>)> = std::sync::Mutex::new((Vec::new(), None));
    let mut c = CACHE.lock().unwrap();
    if c.1.is_none_or(|t| t.elapsed().as_secs() >= 10) {
        *c = (crate::capture::adapters().into_iter().map(|(n, _)| n).collect(), Some(Instant::now()));
    }
    c.0.clone()
}

/// A lista da placa de vídeo: automática e as placas, com a escolhida mesmo se sumiu.
fn gpu_pick(ui: &mut Ui, key: &str, r: Rect, auto: &str, cur: &mut String) {
    let mut vals = vec![String::new()];
    vals.extend(gpu_names());
    if !vals.contains(cur) {
        vals.push(cur.clone());
    }
    let mut names = vec![crate::i18n::tr(auto).into_owned()];
    names.extend(vals[1..].iter().cloned());
    let mut s = vals.iter().position(|v| v == cur).unwrap_or(0);
    if ui.select(key, r, &names, &mut s) {
        *cur = vals[s].clone();
    }
}

fn pick<T: PartialEq + Copy>(ui: &mut Ui, key: &str, r: Rect, values: &[T], names: Vec<String>, cur: &mut T) {
    let mut s = values.iter().position(|v| v == cur).unwrap_or(0);
    if ui.select(key, r, &names, &mut s) {
        *cur = values[s];
    }
}

/// A aba Host: as opções de todos, ou as de um aparelho que assiste. Mexer nas opções de um
/// aparelho cria a configuração própria dele; desmarcar volta às de todos.
#[allow(clippy::too_many_arguments)]
fn host_tab(ui: &mut Ui, app: &mut App, st: &mut State, all: &mut Opts, x: f32, top: f32, w: f32, h: f32) {
    // Os aparelhos que assistem a este PC, com o nome que a pessoa deu.
    let viewers: Vec<(String, String)> = stream::device_rows()
        .into_iter()
        .filter(|d| d.viewer)
        .map(|d| {
            let shown = app.cfg.share_names.iter().find(|n| n.0 == d.id).map_or(d.name.clone(), |n| n.1.clone());
            (d.id, shown)
        })
        .collect();
    if st.host_for.as_ref().is_some_and(|id| !viewers.iter().any(|v| v.0 == *id)) {
        st.host_for = None;
    }
    let target = st.host_for.clone();
    let own_before = target.as_deref().is_some_and(Opts::has_own);
    let before = match &target {
        Some(id) => Opts::load_for(id),
        None => all.clone(),
    };
    let mut edit = before.clone();
    let mut own = own_before;
    let o = &mut edit;

    let lw = ((w - GAP) / 2.0).floor();
    let l = rect(x, top, lw, h);
    ui.card(l);
    let ix = l.x + 16.0;
    let iw = l.w - 32.0;
    // As listas abrem por cima: guardo onde ficam e desenho no fim.
    let ctl = 240.0f32.min(iw * 0.55);
    ui.title(rect(ix, l.y + 12.0, iw - ctl - 8.0, 22.0), "Imagem");
    let for_r = rect(ix + iw - ctl, l.y + 6.0, ctl, 34.0);
    let mut y = l.y + 48.0;
    match &target {
        None => {
            let name_r = row(ui, ix, y, iw, "Nome deste PC", ctl);
            let field = st.name_field.get_or_insert_with(|| all.name.clone());
            ui.search("sh-name", name_r, field, &std::env::var("COMPUTERNAME").unwrap_or_default());
            let clean: String = field.chars().filter(|c| !c.is_control()).take(40).collect();
            if clean.trim() != all.name {
                all.name = clean.trim().to_string();
            }
        }
        Some(_) => {
            ui.check("sh-own", rect(ix, y + 3.0, iw, 30.0), "Configuração própria para este aparelho", &mut own);
        }
    }
    y += 44.0;
    let mon_r = row(ui, ix, y, iw, "Monitor", ctl);
    y += 44.0;
    let gpu_r = row(ui, ix, y, iw, "Placa de vídeo", ctl);
    y += 44.0;
    let res_r = row(ui, ix, y, iw, "Resolução máxima", ctl);
    y += 44.0;
    let fps_r = row(ui, ix, y, iw, "Quadros por segundo", ctl);
    y += 44.0;
    let prio_r = row(ui, ix, y, iw, "Prioridade", ctl);
    y += 44.0;
    let codec_r = row(ui, ix, y, iw, "Codec", ctl);
    y += 44.0;
    let mbps_r = row(ui, ix, y, iw, "Limite de banda", ctl);
    y += 46.0;
    ui.check("sh-10", rect(ix, y, iw, 30.0), "10 bits: HDR10 com as duas telas em HDR, e degradês sem faixas para quem pede", &mut o.ten_bit);
    y += 32.0;
    ui.check("sh-444", rect(ix, y, iw, 30.0), "Aceitar 4:4:4 quando quem assiste pede", &mut o.allow444);
    y += 32.0;
    ui.check("sh-hdr", rect(ix, y, iw, 30.0), "Desligar o HDR quando quem assiste não tem HDR", &mut o.hdr_off);
    y += 36.0;
    let foot = rect(ix, y, iw, (l.bottom() - y - 12.0).max(0.0));
    if o.codec == "pyrowave" {
        // O PyroWave troca compressão por atraso: só cabe em rede rápida, e o relé nunca leva.
        ui.c.text_wrap(foot, "PyroWave: uns 200 Mbit/s em 1080p60 e mais em 4K. Só na rede local ou pela VPN (ZeroTier, Tailscale), nunca pelo relé; fora delas, e com quem assiste numa versão sem ele, vale o automático.", 12.0, WARN, Weight::Regular);
    } else {
        ui.c.text_wrap(foot, "Os automáticos seguem quem assiste: a altura da tela, o menor entre os Hz dos dois lados e o melhor codec que os dois têm. O 4:4:4 e o HDR10 caem sozinhos se o outro lado não tiver.", 12.0, MUTED, Weight::Regular);
    }

    let r = rect(l.right() + GAP, top, w - lw - GAP, h);
    ui.card(r);
    let rx = r.x + 16.0;
    let rw = r.w - 32.0;
    ui.title(rect(rx, r.y + 12.0, rw, 22.0), "Som, mouse e teclado");
    let mut y = r.y + 44.0;
    let rctl = 240.0f32.min(rw * 0.55);
    let snd_r = row(ui, rx, y, rw, "Som que vai", rctl);
    y += 44.0;
    let out_r = row(ui, rx, y, rw, "Saída capturada", rctl);
    y += 44.0;
    let in_r = row(ui, rx, y, rw, "Entrada junto", rctl);
    y += 44.0;
    ui.check("sh-calls", rect(rx, y, rw, 30.0), "Tirar do som o programa de chamada", &mut o.no_calls);
    y += 40.0;
    // Pessoas e cadeia valem para todos os aparelhos.
    if target.is_none() {
        let mut guests = all.guests as u32;
        ui.label(rect(rx, y, rw - 160.0, 36.0), "Pessoas ao mesmo tempo", TEXT2);
        if ui.stepper("sh-guests", rect(rx + rw - 150.0, y, 150.0, 36.0), &mut guests, (1, 4), 1, "") {
            all.guests = guests as usize;
        }
        y += 46.0;
    }
    ui.check("sh-one", rect(rx, y, rw, 30.0), "Uma pessoa por vez no mouse e no teclado", &mut o.one_at_a_time);
    y += 32.0;
    ui.check("sh-first", rect(rx, y, rw, 30.0), "O mouse e o teclado deste PC têm prioridade", &mut o.host_first);
    y += 32.0;
    if target.is_none() {
        ui.check("sh-chain", rect(rx, y, rw, 30.0), "Recusar conexões enquanto este PC assiste a outro", &mut all.no_chain);
        y += 32.0;
        // A NVML só trava o clock com o serviço (SYSTEM) ou elevado: sem eles, a caixa já diz.
        let can = stream::service_dir().join("servico.tsv").exists() || crate::startup::is_elevated();
        let label = if crate::pages::nvidia::available() && can {
            "GPU em desempenho máximo enquanto compartilha"
        } else if crate::pages::nvidia::available() {
            "GPU em desempenho máximo enquanto compartilha (sem efeito: precisa do serviço do Prisma ou do Prisma como administrador)"
        } else {
            "GPU em desempenho máximo enquanto compartilha (sem efeito: só placas NVIDIA)"
        };
        ui.check("sh-gpumax", rect(rx, y, rw, 30.0), label, &mut all.gpu_max);
        y += 32.0;
    }
    y += 6.0;
    let note = if target.is_some() {
        "Configuração própria: vale quando este aparelho conecta. Com mais de uma pessoa ao mesmo tempo, vale a de quem conectou primeiro. Nome, pessoas, porta, controles e atalhos são os de todos."
    } else {
        "Com prioridade, quem assiste para por 1 segundo quando alguém mexe no mouse ou no teclado daqui. Uma pessoa por vez: a vez passa depois de 2 segundos sem mexer. O Windows só deixa tirar um programa de chamada (Discord, Teams, Zoom, WhatsApp, Skype, Telegram, Slack, Signal), e só da saída padrão: sai o que estiver tocando. Uma saída escolhida manda o que toca nela. GPU em desempenho máximo: enquanto alguém assiste, a placa NVIDIA fica no clock máximo da GPU e da memória, e fica livre de novo no fim; precisa do serviço ou do Prisma como administrador, e se a placa não obedecer aparece um aviso. As mudanças valem na hora; as da imagem recomeçam o vídeo por um instante."
    };
    ui.c.text_wrap(rect(rx, y, rw, (r.bottom() - y - 12.0).max(0.0)), note, 12.0, MUTED, Weight::Regular);

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
    // A tela virtual entra na lista quando o driver está instalado (cartão na aba Rede).
    if crate::sys::vdd::stage() == crate::sys::vdd::Stage::Ready || o.monitor >= crate::sys::vdd::VIRTUAL {
        mon_vals.extend([crate::sys::vdd::VIRTUAL, crate::sys::vdd::PRIVATE]);
        mon_names.extend(["Tela virtual", "Tela virtual, telas daqui apagadas"].map(|t| crate::i18n::tr(t).into_owned()));
    }
    if !mon_vals.contains(&o.monitor) {
        mon_vals.push(o.monitor);
        mon_names.push(format!("{} {}", crate::i18n::tr("Monitor"), o.monitor));
    }
    pick(ui, "sh-mon", mon_r, &mon_vals, mon_names, &mut o.monitor);
    gpu_pick(ui, "sh-gpu", gpu_r, "A do monitor", &mut o.gpu);
    let rv: Vec<u32> = RES.iter().map(|r| r.0).collect();
    pick(ui, "sh-res", res_r, &rv, opt(&RES.map(|r| r.1)), &mut o.res);
    let fps_names: Vec<String> = FPS.iter().map(|&f| if f == 0 { crate::i18n::tr("Automático").into_owned() } else { f.to_string() }).collect();
    pick(ui, "sh-fps", fps_r, &FPS, fps_names, &mut o.fps);
    pick(ui, "sh-prio", prio_r, &[0u8, 1, 2], opt(&["Menor atraso", "Equilíbrio", "Qualidade"]), &mut o.priority);
    let codec_vals: Vec<&str> = stream::CODECS.to_vec();
    let mut cur = o.codec.as_str();
    pick(ui, "sh-codec", codec_r, &codec_vals, opt(&["Automático", "AV1", "HEVC (H.265)", "H.264", "PyroWave"]), &mut cur);
    o.codec = cur.to_string();
    // Os limites acima do NVENC só aparecem com o PyroWave; trocar de codec traz o limite para baixo.
    let cap = if o.codec == "pyrowave" { stream::MAX_MBPS } else { stream::NV_MAX_MBPS };
    o.mbps = o.mbps.min(cap);
    let mbps_vals: Vec<u32> = MBPS.iter().copied().filter(|&m| m <= cap).collect();
    let mbps_names: Vec<String> = mbps_vals.iter().map(|&m| if m == 0 { crate::i18n::tr("Automático").into_owned() } else { format!("{m} Mbit/s") }).collect();
    pick(ui, "sh-mbps", mbps_r, &mbps_vals, mbps_names, &mut o.mbps);

    // Programas com som agora, lidos a cada 5 s.
    if st.sound_apps.1.is_none_or(|t| t.elapsed().as_secs() >= 5) {
        let mut v: Vec<String> = crate::sys::audio::sessions().into_iter().map(|s| s.exe).filter(|e| !e.is_empty()).collect();
        v.sort();
        v.dedup();
        st.sound_apps = (v, Some(Instant::now()));
    }
    // Com os programas de chamada fora do som, eles também saem desta lista.
    if o.no_calls && stream::host::is_call_app(&o.sound_app) {
        o.sound_app.clear();
    }
    let mut vals: Vec<String> = vec![String::new()];
    vals.extend(st.sound_apps.0.iter().filter(|e| !(o.no_calls && stream::host::is_call_app(e))).cloned());
    if !o.sound_app.is_empty() && !vals.contains(&o.sound_app) {
        vals.push(o.sound_app.clone());
    }
    let mut names = vec![crate::i18n::tr("Todo o PC").into_owned()];
    names.extend(vals[1..].iter().map(|e| format!("{} {e}", crate::i18n::tr("Só"))));
    let mut s = vals.iter().position(|v| *v == o.sound_app).unwrap_or(0);
    if ui.select("sh-snd", snd_r, &names, &mut s) {
        o.sound_app = vals[s].clone();
    }

    // Saídas e entradas de som, lidas a cada 5 s (um fone USB pode chegar).
    if st.sound_devs.2.is_none_or(|t| t.elapsed().as_secs() >= 5) {
        st.sound_devs = (crate::sys::audio::devices(false), crate::sys::audio::devices(true), Some(Instant::now()));
    }
    let mut vals: Vec<String> = vec![String::new()];
    let mut names = vec![crate::i18n::tr("A padrão do Windows").into_owned()];
    for d in &st.sound_devs.0 {
        vals.push(d.id.clone());
        names.push(d.name.clone());
    }
    if !vals.contains(&o.sound_out) {
        vals.push(o.sound_out.clone());
        names.push(crate::i18n::tr("Desligada agora").into_owned());
    }
    let mut s = vals.iter().position(|v| *v == o.sound_out).unwrap_or(0);
    if ui.select("sh-out", out_r, &names, &mut s) {
        o.sound_out = vals[s].clone();
    }
    let mut vals: Vec<String> = vec![String::new(), stream::MIC_DEFAULT.to_string()];
    let mut names = vec![crate::i18n::tr("Nenhuma").into_owned(), crate::i18n::tr("A padrão do Windows").into_owned()];
    for d in &st.sound_devs.1 {
        vals.push(d.id.clone());
        names.push(d.name.clone());
    }
    if !vals.contains(&o.sound_in) {
        vals.push(o.sound_in.clone());
        names.push(crate::i18n::tr("Desligada agora").into_owned());
    }
    let mut s = vals.iter().position(|v| *v == o.sound_in).unwrap_or(0);
    if ui.select("sh-in", in_r, &names, &mut s) {
        o.sound_in = vals[s].clone();
    }

    // Para quem valem as opções.
    let mut for_names = vec![crate::i18n::tr("Todos os aparelhos").into_owned()];
    for_names.extend(viewers.iter().map(|v| {
        let mark = if Opts::has_own(&v.0) { " *" } else { "" };
        format!("{}{mark}", v.1)
    }));
    let mut s = target.as_ref().and_then(|id| viewers.iter().position(|v| v.0 == *id)).map_or(0, |i| i + 1);
    let picked = ui.select("sh-for", for_r, &for_names, &mut s);

    // Grava o que mudou: nas de todos, ou na configuração própria do aparelho.
    match &target {
        None => {
            if edit != before {
                let name = all.name.clone();
                let (guests, chain, gpu_max) = (all.guests, all.no_chain, all.gpu_max);
                *all = edit;
                all.name = name;
                all.guests = guests;
                all.no_chain = chain;
                all.gpu_max = gpu_max;
            }
        }
        Some(id) => {
            let result = if own_before && !own {
                Opts::drop_own(id)
            } else if own && (!own_before || edit != before) {
                edit.save_for(id, all)
            } else if !own && edit != before {
                // Mexeu numa opção sem configuração própria: ela nasce agora.
                edit.save_for(id, all)
            } else {
                Ok(())
            };
            if result.is_err() {
                app.toast.info("Não deu para salvar. Com o serviço, abra o Prisma como administrador.");
            }
        }
    }
    if picked {
        st.host_for = (s > 0).then(|| viewers[s - 1].0.clone());
    }
}

#[allow(clippy::too_many_arguments)]
fn client_tab(ui: &mut Ui, app: &App, c: &mut ClientOpts, x: f32, top: f32, w: f32, h: f32) {
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
    y += 34.0;
    ui.check("sh-vrr", rect(ix, y, iw, 30.0), "Tela G-SYNC ou FreeSync: cada quadro na hora, sem rasgar", &mut c.vrr);
    y += 34.0;
    ui.check("sh-upscale", rect(ix, y, iw, 30.0), "Super-resolução da placa (NVIDIA RTX ou Intel)", &mut c.upscale);
    y += 40.0;
    let ctl = 240.0f32.min(iw * 0.55);
    let imm_r = row(ui, ix, y, iw, "Modo imersivo", ctl);
    y += 46.0;
    ui.check("sh-ask444", rect(ix, y, iw, 30.0), "Pedir 4:4:4 (texto mais nítido; HEVC só em NVIDIA ou Intel)", &mut c.ask444);
    y += 34.0;
    ui.check("sh-ask10", rect(ix, y, iw, 30.0), "Pedir 10 bits (degradês sem faixas; host com NVIDIA)", &mut c.ask10);
    y += 34.0;
    ui.check("sh-mute", rect(ix, y, iw, 30.0), "Começar sem o som do outro PC", &mut c.mute);
    y += 34.0;
    ui.check("sh-mic", rect(ix, y, iw, 30.0), "Mandar meu microfone ao outro PC (se ele permitir)", &mut c.mic);
    ui.tip(rect(ix, y, iw, 30.0), "Lá, os programas ouvem a sua voz no microfone CABLE Output. O outro PC precisa do VB-CABLE e da permissão Microfone para este aparelho.");
    y += 38.0;
    ui.label(rect(ix, y, iw, 18.0), "Telas lado a lado: esquerda, meio e direita", TEXT2);
    y += 22.0;
    let sw = ((iw - 16.0) / 3.0).floor();
    let screen_r: Vec<Rect> = (0..3).map(|k| rect(ix + k as f32 * (sw + 8.0), y, sw, 40.0)).collect();
    y += 52.0;
    ui.c.text_wrap(
        rect(ix, y, iw, (l.bottom() - y - 12.0).max(0.0)),
        "No modo imersivo, em tela cheia, o mouse fica preso na janela, e a tecla Windows e o Alt+Tab vão para o outro PC. No Tudo, vai qualquer tecla, mesmo as que um programa deste PC pegaria. Os atalhos desta janela sempre funcionam aqui. Numa tela com G-SYNC ou FreeSync, deixe o VSync desligado: a imagem não rasga. Telas lado a lado valem para Mouse e teclado, na lista de PCs: o mouse passa pela borda de uma tela para a vizinha, sem vídeo. Um PC sem posição vai para a primeira livre ao lado deste.",
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
    y += 44.0;
    let cfps_r = row(ui, rx, y, rw, "Quadros por segundo", rctl);
    y += 44.0;
    let pace_r = row(ui, rx, y, rw, "Ritmo dos quadros", rctl);
    y += 44.0;
    let cgpu_r = row(ui, rx, y, rw, "Placa de vídeo", rctl);
    y += 46.0;
    ui.check("sh-warn", rect(rx, y, rw, 30.0), "Avisar quando a conexão piorar", &mut c.warn);
    y += 40.0;
    ui.c.text_wrap(
        rect(rx, y, rw, (r.bottom() - y - 12.0).max(0.0)),
        "O simples mostra o atraso de ponta a ponta e uma bolinha verde, amarela ou vermelha. O detalhado mostra também a rede, o vídeo, os quadros por segundo, a banda, a decodificação, os quadros perdidos e o caminho. O aviso aparece quando a rede perde mais de 2% dos quadros, quando a latência varia mais de 10 ms ou sobe por 3 segundos seguidos. Os quadros por segundo pedidos não passam dos Hz desta tela; o automático usa os Hz dela. Tudo isso muda também pelo menu da conexão. A placa de vídeo vale na próxima conexão.",
        12.0,
        MUTED,
        Weight::Regular,
    );

    // Cada posição: nenhum, este PC ou um PC que este assiste.
    let mut who: Vec<(String, String)> = vec![(String::new(), crate::i18n::tr("Nenhum").into_owned()), ("eu".into(), crate::i18n::tr("Este PC").into_owned())];
    for d in stream::device_rows().into_iter().filter(|d| !d.viewer) {
        let shown = app.cfg.share_names.iter().find(|n| n.0 == d.id).map_or(d.name.clone(), |n| n.1.clone());
        who.push((d.id, shown));
    }
    for (k, r) in screen_r.iter().enumerate() {
        let mut s = who.iter().position(|w| w.0 == c.screens[k]).unwrap_or(0);
        let names: Vec<String> = who.iter().map(|w| w.1.clone()).collect();
        if ui.select(&format!("sh-screen{k}"), *r, &names, &mut s) {
            let pick = who[s].0.clone();
            // Quem estava em outra posição sai de lá; este PC nunca fica sem lugar.
            let old = std::mem::replace(&mut c.screens[k], pick.clone());
            if !pick.is_empty() {
                for (j, o) in c.screens.iter_mut().enumerate() {
                    if j != k && *o == pick {
                        *o = old.clone();
                    }
                }
            }
            if !c.screens.iter().any(|o| o == "eu") {
                c.screens[k] = "eu".into();
                if let Some(j) = (0..3).find(|&j| j != k && c.screens[j].is_empty()) {
                    c.screens[j] = pick;
                }
            }
        }
    }
    pick(ui, "sh-imm", imm_r, &stream::IMMERSIVE, opt(&["Desligado", "Só o mouse", "Só o teclado", "Mouse e teclado", "Tudo"]), &mut c.immersive);
    pick(ui, "sh-meter", meter_r, &[0u8, 1, 2], opt(&["Desligado", "Simples", "Detalhado"]), &mut c.meter);
    let cfps = crate::stream::CLIENT_FPS;
    let cfps_names: Vec<String> = cfps.iter().map(|&f| if f == 0 { crate::i18n::tr("Automático").into_owned() } else { f.to_string() }).collect();
    pick(ui, "sh-cfps", cfps_r, &cfps, cfps_names, &mut c.fps);
    pick(ui, "sh-pace", pace_r, &[false, true], opt(&["Só quando a tela muda", "Constante"]), &mut c.constant);
    gpu_pick(ui, "sh-cgpu", cgpu_r, "A do Windows", &mut c.gpu);
}

#[allow(clippy::too_many_arguments)]
fn net_tab(ui: &mut Ui, app: &mut App, st: &mut State, o: &mut Opts, x: f32, top: f32, w: f32, h: f32) {
    let lw = ((w - GAP) / 2.0).floor();
    let vh = virtual_card(ui, x, top + h, lw);
    let l = rect(x, top, lw, h - vh - GAP);
    ui.card(l);
    let ix = l.x + 16.0;
    let iw = l.w - 32.0;
    ui.title(rect(ix, l.y + 12.0, iw, 22.0), "Portas");
    let mut y = l.y + 44.0;
    let port_r = row(ui, ix, y, iw, "Porta fixa (UDP)", 160.0);
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
    // O `rede.txt` fica no disco depois que o host para: só vale com o host no ar.
    let live = stream::service::installed() || st.host.as_mut().is_some_and(Proc::alive);
    if let Some(seen) = live.then(|| std::fs::read_to_string(stream::net_path()).unwrap_or_default()).and_then(|t| net_seen(&t, o.upnp)) {
        ui.c.text_wrap(rect(ix, y, iw, 36.0), &seen, 12.0, ACCENT, Weight::Regular);
        y += 40.0;
    }
    ui.c.text_wrap(
        rect(ix, y, iw, (l.bottom() - y - 12.0).max(0.0)),
        "Automática serve para quase todo mundo: o Prisma fura o NAT sem porta fixa. Se a conexão sem VPN passar pelo relé, escolha uma porta fixa e abra essa porta UDP no roteador para este PC (ou deixe o pedido ao roteador abrir). Com CGNAT (a operadora divide o IP com outras casas) abrir a porta não adianta: use VPN ou IPv6. A porta vale na próxima vez que o compartilhamento ligar; se estiver ocupada, o Prisma usa uma aleatória. O pedido ao roteador some sozinho quando a conexão acaba.",
        12.0,
        MUTED,
        Weight::Regular,
    );

    let ch = cable_card(ui, l.right() + GAP, top + h, w - lw - GAP);
    let r = rect(l.right() + GAP, top, w - lw - GAP, h - ch - GAP);
    ui.card(r);
    let rx = r.x + 16.0;
    let rw = r.w - 32.0;
    ui.title(rect(rx, r.y + 12.0, rw, 22.0), "Como a conexão passa");
    let y = net_boxes(ui, app, st, o, rx, r.y + 44.0, rw);
    ui.c.text_wrap(rect(rx, y, rw, (r.bottom() - y - 12.0).max(0.0)), NOTICE, 13.0, TEXT2, Weight::Regular);
    let lines = tail(&st.view);
    if let Some(route) = lines.iter().rev().find_map(|l| l.strip_prefix("conectado: ")) {
        let t = format!("{} {}", crate::i18n::tr("Última conexão:"), stream::route_label(route));
        ui.c.text_raw(rect(rx, r.bottom() - 40.0, rw, 24.0), &t, 13.0, ACCENT, Align::Left, Weight::Regular);
        if route.starts_with("pelo relé") {
            ui.c.text(rect(rx, r.bottom() - 64.0, rw, 24.0), stream::RELAY_TIP, 12.0, WARN, Align::Left, Weight::Regular);
        }
    }
}

/// As caixas de rede da aba Rede, por aparelho: por onde a conexão pode passar (direto, cada VPN
/// deste PC, o relé). Quem assiste a este PC fica nas opções do host (todos ou um aparelho); este
/// PC assistindo a outro fica em `cliente-redes.txt`. Devolve onde o resto do cartão começa.
#[allow(clippy::too_many_arguments)]
fn net_boxes(ui: &mut Ui, app: &mut App, st: &mut State, o: &mut Opts, x: f32, y: f32, w: f32) -> f32 {
    use crate::i18n::tr;
    use stream::route;
    let devs: Vec<(String, String)> = stream::device_rows()
        .into_iter()
        .map(|d| {
            let shown = app.cfg.share_names.iter().find(|n| n.0 == d.id).map_or(d.name.clone(), |n| n.1.clone());
            (format!("{}:{}", if d.viewer { "v" } else { "h" }, d.id), shown)
        })
        .collect();
    let mut names = vec![tr("Quem assiste a este PC: todos os aparelhos").into_owned()];
    for (k, n) in &devs {
        let lead = if k.starts_with('v') { "Quem assiste a este PC:" } else { "Este PC assistindo a" };
        names.push(format!("{} «{n}»", tr(lead)));
    }
    let mut s = devs.iter().position(|d| d.0 == st.net_for).map_or(0, |i| i + 1);
    if s == 0 {
        st.net_for.clear();
    }
    let sel_r = rect(x, y, w, 34.0);
    let mut y = y + 44.0;
    let (kind, id) = st.net_for.split_once(':').unwrap_or(("", ""));
    let deny = match kind {
        "v" => Opts::load_for(id).nets_off,
        "h" => route::client_deny(id),
        _ => o.nets_off.clone(),
    };
    let mut new = deny.clone();
    for (i, key) in route::networks().into_iter().enumerate() {
        let mut on = route::allowed(&route::parse_deny(&new), key);
        if ui.check(&format!("sh-net{i}"), rect(x, y, w, 30.0), &route::network_name(key), &mut on) {
            new = route::set_allowed(&new, key, on);
        }
        y += 34.0;
    }
    if new != deny {
        let saved = match kind {
            "v" => {
                let mut e = Opts::load_for(id);
                e.nets_off = new;
                e.save_for(id, o)
            }
            "h" => route::set_client_deny(id, &new),
            _ => {
                o.nets_off = new;
                Ok(())
            }
        };
        if saved.is_err() {
            app.toast.info("Não deu para salvar. Com o serviço, abra o Prisma como administrador.");
        }
    }
    let note = "Com várias marcadas, o Prisma usa a melhor; o relé só quando nenhuma direta chega. Desmarcada, a rede não é usada: se só ela chegar, a conexão fecha. Para quem assiste a este PC vale na hora; para este PC assistindo a outro, na próxima conexão.";
    let nh = crate::ui::gfx::measure(&tr(note), 12.0, Weight::Regular, Some(w)).1.ceil();
    ui.c.text_wrap(rect(x, y + 2.0, w, nh), note, 12.0, MUTED, Weight::Regular);
    // A lista por último: aberta, ela fica por cima das caixas.
    if ui.select("sh-netfor", sel_r, &names, &mut s) {
        st.net_for = if s == 0 { String::new() } else { devs[s - 1].0.clone() };
    }
    y + nh + 14.0
}

/// A linha "o host vê" da aba Rede, a partir do `rede.txt` que o host grava.
fn net_seen(text: &str, upnp: bool) -> Option<String> {
    use crate::i18n::tr;
    let get = |k: &str| text.lines().find_map(|l| l.strip_prefix(k)).filter(|v| !v.is_empty());
    let port = get("porta=")?;
    let mut parts = vec![format!("{} {port}", tr("O host usa a porta"))];
    if let Some(ip) = get("ip4=") {
        parts.push(format!("{} {ip}", tr("IP público")));
    }
    if let Some(ip) = get("ip6=") {
        parts.push(format!("IPv6 {ip}"));
    }
    if upnp {
        parts.push(tr(if get("upnp=") == Some("1") { "o roteador abriu a porta" } else { "o roteador não abriu a porta sozinho" }).into_owned());
    }
    Some(parts.join(" · "))
}

/// Aviso antes de baixar e instalar o driver da tela virtual: o que entra no PC e como desfazer.
const VDD_CONSENT: &str = "O Prisma vai baixar o Virtual Display Driver (VirtualDrivers, programa livre de terceiros, assinado) e o NefCon, que instala o driver, das páginas oficiais deles no GitHub, e conferir os dois. O Windows pede permissão de administrador; o certificado do driver entra em Editores Confiáveis. A tela virtual só liga quando alguém conecta com ela escolhida em Monitor, e desliga ao sair. Para desfazer: este cartão, Remover.";

/// O que o cartão de um driver de terceiros pediu.
enum DriverAct {
    Install,
    Check,
    Remove,
}

/// Cartão de um driver de terceiros que o Prisma só instala com consentimento: a linha do estado,
/// Instalar, Verificar de novo e Remover (só se foi o Prisma que instalou). Fica no pé da coluna, a
/// partir de `bottom`. Devolve a altura e o que foi pedido.
#[allow(clippy::too_many_arguments)]
fn driver_card(ui: &mut Ui, id: &str, title: &str, x: f32, bottom: f32, w: f32, stage: &Stage, line: &str, ours: bool, outside: &str, install: (&str, &str), remove: (&str, &str)) -> (f32, Option<DriverAct>) {
    let mut note = crate::i18n::tr(line).into_owned();
    if let Stage::Failed(e) = stage {
        note = format!("{note} ({})", crate::i18n::tr(e));
    }
    // Remover só tira o que o Prisma pôs; o que veio de outro instalador fica com ele.
    if !ours && *stage != Stage::Absent && !stage.busy() {
        note = format!("{note} {}", crate::i18n::tr(outside));
    }
    let nh = crate::ui::gfx::measure(&note, 12.0, Weight::Regular, Some(w - 32.0)).1.ceil();
    let r = rect(x, bottom - (100.0 + nh), w, 100.0 + nh);
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), title);
    ui.c.text_wrap(rect(r.x + 16.0, r.y + 40.0, r.w - 32.0, nh), &note, 12.0, TEXT2, Weight::Regular);
    let bw = ((r.w - 40.0) / 2.0).floor();
    let b1 = rect(r.x + 16.0, r.y + 50.0 + nh, bw, 34.0);
    let b2 = rect(r.x + 24.0 + bw, r.y + 50.0 + nh, bw, 34.0);
    let busy = stage.busy();
    let (ik, rk) = (format!("{id}-install"), format!("{id}-remove"));
    let mut act = None;
    if *stage == Stage::Absent {
        if ui.button(&ik, b1, "Instalar…", true) && !busy {
            ui.ask(&ik, install.0, install.1);
        }
    } else {
        if ui.button(&format!("{id}-check"), b1, "Verificar de novo", false) && !busy {
            act = Some(DriverAct::Check);
        }
        if ours && ui.button(&rk, b2, "Remover…", false) && !busy {
            ui.ask(&rk, remove.0, remove.1);
        }
    }
    if ui.answered(&ik) {
        act = Some(DriverAct::Install);
    }
    if ui.answered(&rk) {
        act = Some(DriverAct::Remove);
    }
    (r.h, act)
}

/// Tela virtual (VDD) no pé da coluna esquerda da aba Rede. Devolve a altura que ocupou.
fn virtual_card(ui: &mut Ui, x: f32, bottom: f32, w: f32) -> f32 {
    use crate::sys::vdd;
    let stage = vdd::stage();
    let line = match &stage {
        Stage::Absent => "Não instalada. Com ela, quem assiste ganha uma tela só sua, no tamanho e nos Hz dele, e dá para apagar as telas daqui.",
        Stage::Downloading => "Baixando o driver da página oficial.",
        Stage::Installing => "Instalando. Aceite a permissão de administrador do Windows.",
        Stage::Reboot => "Reinicie o PC para terminar.",
        Stage::Removing => "Removendo a tela virtual.",
        Stage::Failed(_) => "A instalação não deu certo.",
        Stage::Ready => "Pronta. Escolha Tela virtual em Monitor, na aba Host ou na configuração de um aparelho.",
    };
    let (h, act) = driver_card(
        ui,
        "vdd",
        "Tela virtual",
        x,
        bottom,
        w,
        &stage,
        line,
        vdd::ours(),
        "Instalada fora do Prisma; para remover, use o instalador dela.",
        ("Instalar a tela virtual?", VDD_CONSENT),
        ("Remover a tela virtual?", "Sai só o que o Prisma pôs: o driver, o certificado dele em Editores Confiáveis e a pasta C:\\VirtualDisplayDriver. O que já estava no PC antes fica. O Windows pede permissão de administrador."),
    );
    match act {
        Some(DriverAct::Install) => vdd::start_install(),
        Some(DriverAct::Check) => vdd::recheck(),
        Some(DriverAct::Remove) => vdd::start_remove(),
        None => {}
    }
    h
}

/// Aviso antes de baixar e abrir o instalador do VB-CABLE: de quem é, a licença e como desfazer.
const CABLE_CONSENT: &str = "O VB-CABLE é da VB-Audio, donationware de terceiros: o uso pessoal é livre, com doação opcional, e o uso profissional ou em empresa pede licenças pagas na loja da VB-Audio. O Prisma vai baixar o pacote da página oficial (vb-audio.com), conferir a soma e abrir o instalador da VB-Audio, que pede permissão de administrador. Nele, clique em Install Driver; o PC pode pedir para reiniciar. Para desfazer: este cartão, Remover.";

/// VB-CABLE no pé da coluna direita da aba Rede: o microfone de quem assiste toca nele.
fn cable_card(ui: &mut Ui, x: f32, bottom: f32, w: f32) -> f32 {
    use crate::sys::vbcable;
    let stage = vbcable::stage();
    let line = match &stage {
        Stage::Absent => "Não instalado. Com ele, quem assiste e tem a permissão Microfone fala nos programas daqui, que ouvem o microfone CABLE Output.",
        Stage::Downloading => "Baixando o VB-CABLE da página oficial.",
        Stage::Installing => "No instalador da VB-Audio, clique em Install Driver.",
        Stage::Reboot => "Se o instalador terminou, reinicie o PC para o cabo aparecer.",
        Stage::Removing => "No instalador da VB-Audio, clique em Remove Driver.",
        Stage::Failed(_) => "Não deu certo.",
        Stage::Ready => "Pronto. Nos programas daqui, escolha CABLE Output como microfone, e dê a permissão Microfone ao aparelho de quem assiste.",
    };
    let (h, act) = driver_card(
        ui,
        "vbc",
        "Microfone de quem assiste (VB-CABLE)",
        x,
        bottom,
        w,
        &stage,
        line,
        vbcable::ours(),
        "Instalado fora do Prisma; para remover, use o instalador dele.",
        ("Instalar o VB-CABLE?", CABLE_CONSENT),
        ("Remover o VB-CABLE?", "Abre o instalador da VB-Audio: clique em Remove Driver. O Windows pede permissão de administrador, e o PC pode pedir para reiniciar."),
    );
    match act {
        Some(DriverAct::Install) => vbcable::start_install(),
        Some(DriverAct::Check) => vbcable::recheck(),
        Some(DriverAct::Remove) => vbcable::start_remove(),
        None => {}
    }
    h
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
    hotkey_row(ui, "sh-k-imm", ix, y, iw, "Ligar e desligar o modo imersivo", &mut c.key_imm);
    y += 44.0;
    hotkey_row(ui, "sh-k-meter", ix, y, iw, "Trocar o medidor de latência", &mut c.key_meter);
    y += 44.0;
    hotkey_row(ui, "sh-k-sound", ix, y, iw, "Ligar e desligar o som", &mut c.key_sound);
    y += 44.0;
    hotkey_row(ui, "sh-k-cad", ix, y, iw, "Ctrl+Alt+Del no outro PC", &mut c.key_cad);
    y += 44.0;
    hotkey_row(ui, "sh-k-disc", ix, y, iw, "Desconectar", &mut c.key_disc);
    y += 52.0;
    ui.label(rect(ix, y, iw, 18.0), "Mouse e teclado entre PCs", MUTED);
    y += 24.0;
    hotkey_row(ui, "sh-k-lock", ix, y, iw, "Travar o mouse na tela atual", &mut c.key_lock);
    y += 52.0;
    ui.label(rect(ix, y, iw, 18.0), "No PC que compartilha", MUTED);
    y += 24.0;
    hotkey_row(ui, "sh-k-panic", ix, y, iw, "Desconectar todo mundo", &mut o.key_panic);
    y += 52.0;
    let keys = [c.key_menu, c.key_full, c.key_imm, c.key_meter, c.key_sound, c.key_cad, c.key_disc];
    if keys.iter().enumerate().any(|(i, k)| keys.iter().skip(i + 1).any(|j| j == k)) {
        ui.c.text(rect(ix, y, iw, 20.0), "Dois atalhos iguais: só o primeiro da lista funciona.", 13.0, WARN, Align::Left, Weight::Regular);
        y += 26.0;
    }
    ui.c.text_wrap(
        rect(ix, y, iw, (card.bottom() - y - 12.0).max(0.0)),
        "Clique num atalho e aperte a combinação nova; Esc cancela. Os atalhos de quem assiste ficam com a janela e não vão para o outro PC. Use combinações com Ctrl, Alt ou Shift, para não roubar teclas do jogo. Os novos valem na próxima conexão. O atalho do modo imersivo liga e desliga só na conexão aberta; o modo que fica é o da aba Cliente.",
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
        "No host, cada controle de quem assiste vira um controle de Xbox virtual, pelo ViGEmBus ou pelo VIIPER (o ViGEmBus se instala no cartão Drivers da aba Controles; o VIIPER, não). Eles saem sozinhos quando o jogo da frente tem anti-cheat. Teclado como controle: WASD é o analógico esquerdo, as setas são o direcional, J é A, K é B, U é X, I é Y, Q e E são LB e RB, Z e C os gatilhos, Enter é Start e Backspace é Back.",
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
    // Ligado ou não dos PCs pareados: só com a página aberta e a janela à vista. Sem esta chamada,
    // a verificação para sozinha em 2 s.
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible};
    if app.main.as_ref().is_some_and(|m| unsafe { IsWindowVisible(m.hwnd).as_bool() && !IsIconic(m.hwnd).as_bool() }) {
        stream::presence_keep();
    }
    if let Some((text, name)) = stream::wake_note() {
        // O aviso traduz o texto pronto pelo modelo com {}.
        app.toast.info(&text.replace("{}", &name));
    }
    let req = STATE.lock().unwrap().as_mut().and_then(|s| s.share_req.take());
    let Some((title, text, link)) = req else { return };
    let Some(hwnd) = app.main.as_ref().map(|m| m.hwnd) else { return };
    if !weblink::share_sheet(hwnd, &title, &text, &link) {
        crate::capture::copy_text(app.hub, &text);
        app.toast.info("O Windows não abriu a janela de compartilhar: o convite foi copiado.");
    }
}

/// O co-op está transmitindo: os controles dele podem estar no VIIPER.
pub fn hosting() -> bool {
    STATE.lock().unwrap().as_ref().is_some_and(|s| s.host.is_some())
}

/// Fecha o que a página abriu (o trabalho também fecha tudo quando o Prisma sai).
pub fn shutdown() {
    if let Some(st) = STATE.lock().unwrap().as_mut() {
        let had_host = st.host.is_some();
        stop(&mut st.host);
        stop(&mut st.view);
        stop_kvm(st);
        stop_ear(st);
        stop(&mut st.pair);
        stop(&mut st.invite);
        if had_host {
            stream::host::restore_hdr();
        }
    }
}

#[cfg(test)]
mod net_seen_tests {
    #[test]
    fn reads_rede_txt() {
        let opened = crate::i18n::tr("o roteador abriu a porta").into_owned();
        let t = super::net_seen("ip4=200.1.2.3\nporta=50000\nupnp=1\n", true).unwrap();
        assert!(t.contains("50000") && t.contains("200.1.2.3") && t.contains(&opened), "{t}");
        assert_eq!(super::net_seen("porta=50000\nupnp=0\n", false).unwrap().split(" · ").count(), 1);
        assert!(super::net_seen("", true).is_none());
    }
}

#[cfg(test)]
mod tail_tests {
    use super::{read_tail, Seen};

    #[test]
    fn arquivo_pequeno_vem_inteiro() {
        let f = std::env::temp_dir().join(format!("prisma-tail-curto-{}.txt", std::process::id()));
        std::fs::write(&f, "um\n\ndois\n").unwrap();
        let mut seen = Seen::default();
        assert_eq!(read_tail(&f, &mut seen), ["um", "dois"]);
        // Sem mudar o arquivo, vem do que já foi lido.
        seen.lines.push("guardado".into());
        assert_eq!(read_tail(&f, &mut seen).last().map(String::as_str), Some("guardado"));
        std::fs::write(&f, "um\ndois\ntres\n").unwrap();
        assert_eq!(read_tail(&f, &mut seen), ["um", "dois", "tres"], "tamanho mudou, relê");
        let _ = std::fs::remove_file(&f);
        assert!(read_tail(&f, &mut Seen::default()).is_empty());
    }

    #[test]
    fn arquivo_grande_vem_so_o_fim() {
        let f = std::env::temp_dir().join(format!("prisma-tail-longo-{}.txt", std::process::id()));
        let text: String = (0..3000).map(|i| format!("linha {i}\n")).collect();
        assert!(text.len() > 16 * 1024);
        std::fs::write(&f, &text).unwrap();
        let lines = read_tail(&f, &mut Seen::default());
        let _ = std::fs::remove_file(&f);
        assert_eq!(lines.last().map(String::as_str), Some("linha 2999"));
        assert!(!lines.iter().any(|l| l == "linha 0"), "o começo fica de fora");
        let first: usize = lines[0].trim_start_matches("linha ").parse().expect("a primeira linha vem inteira");
        assert_eq!(lines.len(), 3000 - first);
    }
}
