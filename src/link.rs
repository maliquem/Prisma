//! Ponte com o Prisma Launcher: um pipe nomeado atende o launcher numa thread própria.
//! A thread do pipe nunca toca no `App`: cada pedido entra numa fila, a janela de controle
//! recebe `WM_LINK` e a thread principal responde.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use prisma_link::{cap, fail as code, MeterView, NvidiaKind, NvidiaSetting, NvidiaValue, Page as LinkPage, PowerPlan, Reply, Request, ServerInfo, VolumeChange};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::app::App;
use crate::pages::Page;
use crate::sys::drs::{self, Profile};
use crate::sys::nvset::{self, Kind, Setting};
use crate::sys::{audio, power, procs};

/// Avisa a janela de controle que há pedidos do launcher na fila.
pub const WM_LINK: u32 = WM_APP + 92;

/// Quanto a thread do pipe espera a thread principal antes de seguir sem responder.
const WAIT: Duration = Duration::from_secs(5);

/// Quanto a thread do pipe espera o código aparecer no registro do convite. Com `WAIT`, fica
/// abaixo dos 30 s em que o launcher desiste.
#[cfg(feature = "stream")]
const INVITE_WAIT: Duration = Duration::from_secs(20);

pub enum Outcome {
    Done,
    /// O convite está sendo criado pelo processo `pid`; o código aparece neste registro.
    #[cfg_attr(not(feature = "stream"), allow(dead_code))]
    InviteLog { log: PathBuf, pid: u32 },
    /// Código de `prisma_link::fail` e o motivo já no idioma do Prisma.
    Failed(&'static str, String),
    /// A pasta de capturas (`cfg.cap_dir`), para as fotos e as gravações.
    CaptureDirs(String),
    /// Resposta pronta.
    Reply(Reply),
    /// Trabalho demorado que não toca no `App` (o HDR espera a tela trocar): roda na thread do
    /// pipe, sem travar a janela.
    Later(Box<dyn FnOnce() -> Reply + Send>),
}

impl std::fmt::Debug for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Outcome::Done => write!(f, "Done"),
            Outcome::InviteLog { log, pid } => write!(f, "InviteLog({log:?}, {pid})"),
            Outcome::Failed(c, r) => write!(f, "Failed({c}, {r:?})"),
            Outcome::CaptureDirs(d) => write!(f, "CaptureDirs({d:?})"),
            Outcome::Reply(r) => write!(f, "Reply({r:?})"),
            Outcome::Later(_) => write!(f, "Later"),
        }
    }
}

/// `Later` nunca é igual a nada: só serve aos testes, que comparam o resto.
impl PartialEq for Outcome {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Outcome::Done, Outcome::Done) => true,
            (Outcome::InviteLog { log: a, pid: p }, Outcome::InviteLog { log: b, pid: q }) => a == b && p == q,
            (Outcome::Failed(c, a), Outcome::Failed(d, b)) => c == d && a == b,
            (Outcome::CaptureDirs(a), Outcome::CaptureDirs(b)) => a == b,
            (Outcome::Reply(a), Outcome::Reply(b)) => a == b,
            _ => false,
        }
    }
}

/// Falha com o código e o motivo no idioma do Prisma.
fn fail(code: &'static str, reason: &str) -> Outcome {
    Outcome::Failed(code, crate::i18n::tr(reason).into_owned())
}

/// O que o cliente espera de cada pedido.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Wait {
    /// Avisos e pedidos sem resposta: o cliente não espera.
    Nothing,
    /// O convite responde código ou `ShareInviteFailed`, sempre.
    Invite,
    /// As pastas de captura: sem resposta a tempo, o launcher desiste sozinho.
    Dirs,
    /// Os pedidos novos respondem sempre: a resposta própria, `Done` ou `Failed`.
    Answer,
}

fn wait_of(req: &Request) -> Wait {
    match req {
        Request::Hello { .. } | Request::Unknown | Request::GameStarted { .. } | Request::GameStopped { .. } | Request::OpenPage { .. } | Request::SaveReplay => Wait::Nothing,
        Request::ShareInvite { .. } => Wait::Invite,
        Request::CaptureDirs => Wait::Dirs,
        _ => Wait::Answer,
    }
}

/// Cada pedido leva a hora em que entrou: pedido mais velho que `WAIT` já perdeu o cliente.
type Entry = (Instant, Request, SyncSender<Outcome>);

static QUEUE: Mutex<VecDeque<Entry>> = Mutex::new(VecDeque::new());

/// O que este Prisma sabe fazer pelo launcher. Sem `page.game_profile`: ainda não há uma
/// página de perfil por jogo.
pub fn caps() -> Vec<String> {
    caps_with(crate::pages::nvidia::available())
}

/// `nvidia`: há placa NVIDIA com driver; sem ela, nem a página nem as opções do driver.
fn caps_with(nvidia: bool) -> Vec<String> {
    let mut caps = vec![
        cap::CAPTURE_DIRS,
        cap::GAME_EVENTS,
        cap::PAGE_METER,
        cap::PAGE_SHARE,
        cap::PAGE_ENERGY,
        cap::PAGE_FULLSCREEN,
        cap::PAGE_SOUND,
        cap::PAGE_HDR,
        cap::PAGE_CAPTURE,
        cap::SAVE_REPLAY,
        cap::METER_VIEW,
        cap::POWER_PLAN,
        cap::FULLSCREEN,
        cap::SOUND_VOLUME,
        cap::HDR,
        cap::WINDOW_SHOT,
        cap::RECORD_WINDOW,
    ];
    if cfg!(feature = "stream") {
        caps.push(cap::SHARE_INVITE);
        caps.push(cap::SHARE_INVITE_MINUTES);
    }
    if nvidia {
        caps.push(cap::NVIDIA);
        caps.push(cap::PAGE_NVIDIA);
    }
    caps.into_iter().map(String::from).collect()
}

/// Abre a thread do pipe. Se o pipe não puder ser criado (outro Prisma já o tem), registra e
/// a thread termina; o resto do Prisma segue normal.
pub fn start(hub: HWND) {
    let hub = hub.0 as isize;
    std::thread::spawn(move || {
        let name = match prisma_link::default_pipe_name() {
            Ok(n) => n,
            Err(e) => {
                crate::config::log(&format!("ponte com o launcher: sem nome do pipe ({e})"));
                return;
            }
        };
        let info = ServerInfo {
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            caps: caps(),
        };
        let res = prisma_link::serve(&name, info, |req| {
            let wait = wait_of(&req);
            let (tx, rx) = sync_channel(1);
            push(req, tx);
            unsafe {
                let _ = PostMessageW(Some(HWND(hub as *mut _)), WM_LINK, WPARAM(0), LPARAM(0));
            }
            if wait == Wait::Nothing {
                return None;
            }
            match rx.recv_timeout(WAIT) {
                Ok(outcome) => reply_for(wait, outcome),
                Err(_) => timeout_reply(wait),
            }
        });
        if let Err(e) = res {
            crate::config::log(&format!("ponte com o launcher: pipe indisponível ({e})"));
        }
    });
}

/// A janela não atendeu a tempo (ocupada, menu aberto).
fn timeout_reply(wait: Wait) -> Option<Reply> {
    let reason = || crate::i18n::tr("O Prisma não respondeu").into_owned();
    match wait {
        // O launcher sempre recebe resposta do convite e dos pedidos novos.
        Wait::Invite => Some(Reply::ShareInviteFailed { reason: reason() }),
        Wait::Answer => Some(Reply::Failed { code: code::BUSY.into(), reason: reason() }),
        // Sem resposta das pastas: o launcher desiste sozinho.
        Wait::Dirs | Wait::Nothing => None,
    }
}

/// Resposta ao cliente. Roda na thread do pipe: esperar o código no registro ou o HDR da tela
/// não trava a janela.
fn reply_for(wait: Wait, outcome: Outcome) -> Option<Reply> {
    let invite = wait == Wait::Invite;
    let answer = wait == Wait::Answer;
    match outcome {
        Outcome::Reply(r) => Some(r),
        Outcome::Later(work) => Some(work()),
        Outcome::Done if answer => Some(Reply::Done),
        Outcome::Failed(_, reason) if invite => Some(Reply::ShareInviteFailed { reason }),
        Outcome::Failed(c, reason) if answer => Some(Reply::Failed { code: c.into(), reason }),
        Outcome::CaptureDirs(dir) => Some(Reply::CaptureDirs { shots: dir.clone(), replays: dir }),
        #[cfg(feature = "stream")]
        Outcome::InviteLog { log, pid } if invite => Some(wait_invite(&log, INVITE_WAIT, || crate::sys::procs::is_alive(pid))),
        #[cfg(not(feature = "stream"))]
        Outcome::InviteLog { .. } if invite => Some(Reply::ShareInviteFailed {
            reason: "indisponível".into(),
        }),
        _ => None,
    }
}

/// Lê o registro do convite a cada 250 ms. Com o código na mão, espera o resultado da
/// publicação nos relés: publicado responde o código, não publicado responde a falha. Se o prazo
/// acabar ou o processo do convite sair, uma última leitura decide: código sem resultado ainda
/// vale (o convite longo funciona), sem código é falha.
#[cfg(feature = "stream")]
fn wait_invite(log: &std::path::Path, limit: Duration, alive: impl Fn() -> bool) -> Reply {
    use crate::stream::{weblink::web_link, CODE_FAIL};
    const NOT_PUBLISHED: &str = "o código não foi publicado; use o convite longo na página Compartilhar tela";
    let failed = |reason: &str| Reply::ShareInviteFailed { reason: reason.into() };
    let start = Instant::now();
    loop {
        // O processo é visto antes da leitura: se já tinha saído, esta leitura é a última.
        let late = start.elapsed() >= limit;
        let over = late || !alive();
        let lines: Vec<String> = std::fs::read(log)
            .map(|b| String::from_utf8_lossy(&b).lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect())
            .unwrap_or_default();
        match crate::pages::share::invite_of(&lines) {
            Some(s) if s.published == Some(false) => return failed(NOT_PUBLISHED),
            Some(s) if s.published == Some(true) || over => {
                let link = web_link(&s.code);
                return Reply::ShareInviteCode { code: s.code, link };
            }
            None if lines.iter().any(|l| l.starts_with(CODE_FAIL)) => return failed(NOT_PUBLISHED),
            None if late => return failed("o convite não ficou pronto a tempo"),
            None if over => return failed("o convite não foi criado"),
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(250).min(limit.saturating_sub(start.elapsed())));
    }
}

fn push(req: Request, tx: SyncSender<Outcome>) {
    push_at(Instant::now(), req, tx);
}

fn push_at(at: Instant, req: Request, tx: SyncSender<Outcome>) {
    QUEUE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back((at, req, tx));
}

/// Na thread principal, no `WM_LINK` e a cada `WM_TICK` (o `WM_LINK` se perde se chega com a
/// janela num laço modal, como o menu da bandeja): atende tudo o que está na fila.
pub fn drain(app: &mut App) {
    drain_with(|req| app.on_link(req));
}

fn drain_with(mut f: impl FnMut(Request) -> Outcome) {
    loop {
        // A trava sai antes de atender: o pedido pode demorar e a thread do pipe empilha.
        let next = QUEUE.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
        let Some((at, req, tx)) = next else { break };
        // Pedido parado na fila (janela ocupada, menu aberto) não roda atrasado: o cliente já desistiu.
        // Os avisos de jogo ficam de fora: perder um deixaria a dica errada para sempre.
        let notice = matches!(req, Request::GameStarted { .. } | Request::GameStopped { .. });
        if !notice && at.elapsed() > WAIT {
            continue;
        }
        let _ = tx.send(f(req));
    }
}

impl App {
    pub(crate) fn on_link(&mut self, req: Request) -> Outcome {
        match req {
            Request::OpenPage { page } => {
                let page = match page {
                    LinkPage::Meter => Page::Meter,
                    LinkPage::Share => Page::Share,
                    LinkPage::Energy => Page::Energy,
                    LinkPage::Fullscreen => Page::Fullscreen,
                    LinkPage::Sound => Page::Volume,
                    LinkPage::Hdr => Page::Hdr,
                    LinkPage::Nvidia if crate::pages::nvidia::available() => Page::Nvidia,
                    LinkPage::Capture => Page::Capture,
                    _ => return Outcome::Done,
                };
                self.go(page);
                self.show_main();
                Outcome::Done
            }
            Request::SaveReplay => {
                self.save_replay();
                Outcome::Done
            }
            #[cfg(feature = "stream")]
            Request::ShareInvite { minutes } => match crate::pages::share::create_invite(self, minutes) {
                Some((log, pid)) => {
                    // Convite criado por outro programa não passa em silêncio.
                    self.toast.info("Convite do Compartilhar tela criado pelo Prisma Launcher");
                    Outcome::InviteLog { log, pid }
                }
                None => Outcome::Failed(code::FAILED, "não deu para criar o convite".into()),
            },
            #[cfg(not(feature = "stream"))]
            Request::ShareInvite { .. } => Outcome::Failed(code::NOT_SUPPORTED, "indisponível".into()),
            Request::CaptureDirs => Outcome::CaptureDirs(self.cfg.cap_dir.clone()),
            Request::GameStarted { pid, exe, .. } => {
                self.detector.hint(pid, &exe);
                Outcome::Done
            }
            Request::GameStopped { pid } => {
                self.detector.unhint(pid);
                Outcome::Done
            }
            Request::MeterView { view } => {
                self.set_meter_view(match view {
                    MeterView::Off => crate::meter::VIEW_OFF,
                    MeterView::Compact => crate::meter::VIEW_COMPACT,
                    MeterView::Full => crate::meter::VIEW_FULL,
                });
                Outcome::Done
            }
            Request::PowerPlans => Outcome::Reply(Reply::PowerPlans {
                plans: power::list().into_iter().map(|p| PowerPlan { id: p.id, name: p.name }).collect(),
                active: power::active().unwrap_or_default(),
                fixed: self.cfg.plan_fixed.clone(),
                auto: self.cfg.power_auto,
            }),
            Request::SetPowerPlan { id } => self.link_power(&id),
            Request::Fullscreen { pid } => {
                let Some((h, exe)) = self.link_window(pid) else { return fail(code::NO_WINDOW, "Janela do jogo não encontrada") };
                let before = self.full.is_on(h);
                self.toggle_window(h, &exe);
                let on = self.full.is_on(h);
                if on == before {
                    return fail(code::FAILED, "Essa janela não pode ficar sem bordas");
                }
                Outcome::Reply(Reply::Toggled { on })
            }
            Request::Volume { pid, change } => {
                let pid = if pid == 0 { self.fg_info().map_or(0, |f| f.1) } else { pid };
                match audio::change_focused(pid, audio_change(change)) {
                    Some((vol, muted)) => Outcome::Reply(Reply::Volume { level: (vol * 100.0).round() as u32, muted }),
                    None => fail(code::NO_AUDIO, "O jogo ainda não tem som aberto"),
                }
            }
            Request::Hdr { on } => Outcome::Later(Box::new(move || hdr_reply(on))),
            Request::NvidiaCatalog => {
                if !crate::pages::nvidia::available() {
                    return fail(code::NOT_SUPPORTED, "Sem placa NVIDIA");
                }
                let ver = drs::driver_version().unwrap_or(0);
                Outcome::Reply(Reply::NvidiaCatalog { settings: nv_shown(ver, true).map(|(g, s)| nv_setting(g, s)).collect() })
            }
            Request::NvidiaGet { exe } => {
                if !crate::pages::nvidia::available() {
                    return fail(code::NOT_SUPPORTED, "Sem placa NVIDIA");
                }
                let who = match nv_profile(&exe) {
                    Ok(w) => w,
                    Err(e) => return fail(code::INVALID, e),
                };
                let ver = drs::driver_version().unwrap_or(0);
                let ids: Vec<u32> = nv_shown(ver, who == Profile::Global).map(|(_, s)| s.id).collect();
                match drs::snapshot(&who, &ids) {
                    Some(snap) => Outcome::Reply(Reply::NvidiaValues {
                        values: ids
                            .iter()
                            .zip(snap.values)
                            .map(|(id, v)| NvidiaValue { id: *id, value: v.filter(|v| v.mine).map(|v| v.now), now: v.map(|v| v.now) })
                            .collect(),
                    }),
                    None => fail(code::FAILED, "O driver NVIDIA não respondeu"),
                }
            }
            Request::NvidiaSet { exe, id, value } => self.link_nvidia_set(&exe, id, value),
            Request::WindowShot { pid } => {
                let Some((h, _)) = self.link_window(pid) else { return fail(code::NO_WINDOW, "Janela do jogo não encontrada") };
                match self.save_window_shot(h) {
                    Some(path) => Outcome::Reply(Reply::Shot { path }),
                    None => fail(code::FAILED, "Não consegui capturar a janela"),
                }
            }
            Request::RecordWindow { pid } => {
                // Com gravação em andamento, para, como o atalho.
                if self.is_recording() {
                    self.toggle_record_of(true, None);
                    return Outcome::Reply(Reply::Toggled { on: false });
                }
                let Some((h, _)) = self.link_window(pid) else { return fail(code::NO_WINDOW, "Janela do jogo não encontrada") };
                self.toggle_record_of(true, Some(h));
                Outcome::Reply(Reply::Toggled { on: self.is_recording() })
            }
            Request::Hello { .. } | Request::Unknown => Outcome::Done,
        }
    }

    /// A janela do jogo que o launcher abriu; `pid` 0 é a janela em primeiro plano, como os atalhos.
    fn link_window(&self, pid: u32) -> Option<(HWND, String)> {
        if pid == 0 {
            return self.fg_info().map(|(h, _, exe)| (h, exe));
        }
        let h = procs::main_window(pid)?;
        let exe = procs::image_path(procs::window_pid(h)).map(|p| procs::exe_name(&p)).unwrap_or_default();
        Some((h, exe))
    }

    /// Plano fixo, como a página Energia com a troca automática desligada; vazio volta à troca.
    fn link_power(&mut self, id: &str) -> Outcome {
        let plans = power::list();
        let fixed = match fixed_plan(id, &plans) {
            Ok(f) => f,
            Err(e) => return fail(code::INVALID, e),
        };
        match &fixed {
            None => self.cfg.power_auto = true,
            Some(f) => {
                self.cfg.power_auto = false;
                self.cfg.plan_fixed = f.clone();
            }
        }
        self.cfg.save();
        self.apply_power_choice();
        self.invalidate();
        match fixed {
            Some(f) if !self.active_plan.eq_ignore_ascii_case(&f) => fail(code::FAILED, "O Windows não trocou o plano"),
            _ => Outcome::Done,
        }
    }

    fn link_nvidia_set(&mut self, exe: &str, id: u32, value: Option<u32>) -> Outcome {
        if !crate::pages::nvidia::available() {
            return fail(code::NOT_SUPPORTED, "Sem placa NVIDIA");
        }
        let who = match nv_profile(exe).and_then(|w| nv_check(drs::driver_version().unwrap_or(0), &w, id, value).map(|_| w)) {
            Ok(w) => w,
            Err(e) => return fail(code::INVALID, e),
        };
        let ok = drs::write(&who, &[(id, value)]);
        crate::config::log(&format!(
            "driver NVIDIA pelo launcher: {} 0x{id:08X} = {} ({ok})",
            if exe.is_empty() { "global" } else { exe },
            value.map_or("padrão".into(), |v| format!("0x{v:08X}"))
        ));
        // O jogo entra na lista da página, como quando a pessoa escolhe por lá.
        if !exe.is_empty() && !self.cfg.nv_games.iter().any(|g| g.eq_ignore_ascii_case(exe)) {
            self.cfg.nv_games.push(exe.to_string());
            self.cfg.save();
        }
        crate::pages::nvidia::stale();
        self.invalidate();
        if ok {
            Outcome::Done
        } else {
            fail(code::FAILED, "O driver não aceitou a mudança")
        }
    }
}

fn audio_change(c: VolumeChange) -> audio::Change {
    match c {
        VolumeChange::Step(n) => audio::Change::Step(n),
        VolumeChange::Set(v) => audio::Change::Set(v.min(100) as f32 / 100.0),
        VolumeChange::ToggleMute => audio::Change::ToggleMute,
    }
}

/// O plano pedido, com o id como o Windows escreve; `None` é a troca automática.
fn fixed_plan(id: &str, plans: &[power::Plan]) -> Result<Option<String>, &'static str> {
    if id.is_empty() {
        return Ok(None);
    }
    plans.iter().find(|p| p.id.eq_ignore_ascii_case(id)).map(|p| Some(p.id.clone())).ok_or("Esse plano não existe")
}

/// Liga, desliga ou alterna o HDR da tela. Espera a tela trocar (até 10 s).
fn hdr_reply(on: Option<bool>) -> Reply {
    let failed = |c: &str, r: &str| Reply::Failed { code: c.into(), reason: crate::i18n::tr(r).into_owned() };
    let Some((adapter, output, now)) = crate::sys::hdr::hdr_output() else { return failed(code::NOT_SUPPORTED, "Nenhuma tela aceita HDR") };
    let want = on.unwrap_or(!now);
    if want == now || crate::sys::hdr::set_hdr(adapter, output, want) {
        Reply::Toggled { on: want }
    } else {
        failed(code::FAILED, "O Windows não trocou o HDR")
    }
}

/// Opções que este driver tem, com o grupo; sem as da placa toda fora do perfil global.
fn nv_shown(ver: u32, global: bool) -> impl Iterator<Item = (&'static str, &'static Setting)> {
    nvset::GROUPS
        .iter()
        .flat_map(|(g, list)| list.iter().map(move |s| (*g, s)))
        .filter(move |(_, s)| s.min_driver <= ver && (global || !s.global_only))
}

fn nv_setting(group: &str, s: &Setting) -> NvidiaSetting {
    let tr = |t: &str| crate::i18n::tr(t).into_owned();
    NvidiaSetting {
        id: s.id,
        name: tr(s.name),
        group: tr(group),
        global_only: s.global_only,
        kind: match &s.kind {
            Kind::List(l) => NvidiaKind::List(l.iter().map(|(v, n)| (*v, tr(n))).collect()),
            Kind::Number { min, max, step, unit, .. } => NvidiaKind::Number { min: *min, max: *max, step: *step, unit: unit.to_string() },
        },
    }
}

/// Perfil do pedido: vazio é o global; o jogo vem pelo nome do executável, sem pasta.
fn nv_profile(exe: &str) -> Result<Profile, &'static str> {
    if exe.is_empty() {
        return Ok(Profile::Global);
    }
    let ok = exe.len() > 4 && exe.to_ascii_lowercase().ends_with(".exe") && !exe.contains(['\\', '/', ':']);
    if ok {
        Ok(Profile::Game(exe.to_string()))
    } else {
        Err("Nome de executável inválido")
    }
}

/// O valor pedido precisa ser um que a página aceitaria.
fn nv_check(ver: u32, who: &Profile, id: u32, value: Option<u32>) -> Result<(), &'static str> {
    let Some((_, s)) = nv_shown(ver, *who == Profile::Global).find(|(_, s)| s.id == id) else { return Err("O Prisma não conhece essa opção") };
    let Some(v) = value else { return Ok(()) };
    let fits = match &s.kind {
        Kind::List(l) => l.iter().any(|x| x.0 == v),
        Kind::Number { min, max, .. } => (*min..=*max).contains(&v),
    };
    if fits {
        Ok(())
    } else {
        Err("Valor fora das opções")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;
    use std::time::Duration;

    #[test]
    fn caps_follow_features() {
        for nvidia in [false, true] {
            let caps = caps_with(nvidia);
            let has = |c: &str| caps.iter().any(|x| x == c);
            for c in [
                cap::PAGE_METER, cap::PAGE_SHARE, cap::SAVE_REPLAY, cap::GAME_EVENTS, cap::CAPTURE_DIRS,
                cap::METER_VIEW, cap::POWER_PLAN, cap::FULLSCREEN, cap::SOUND_VOLUME, cap::HDR,
                cap::WINDOW_SHOT, cap::RECORD_WINDOW, cap::PAGE_ENERGY, cap::PAGE_FULLSCREEN,
                cap::PAGE_SOUND, cap::PAGE_HDR, cap::PAGE_CAPTURE,
            ] {
                assert!(has(c), "{c}");
            }
            assert_eq!(has(cap::SHARE_INVITE), cfg!(feature = "stream"));
            assert_eq!(has(cap::SHARE_INVITE_MINUTES), cfg!(feature = "stream"));
            assert_eq!(has(cap::NVIDIA), nvidia);
            assert_eq!(has(cap::PAGE_NVIDIA), nvidia);
            assert!(!has(cap::PAGE_GAME_PROFILE));
            // Reservada até a tela Controles existir.
            assert!(!has(cap::PAGE_CONTROLLERS));
        }
    }

    #[test]
    fn every_new_request_gets_an_answer() {
        let answer = [
            Request::MeterView { view: MeterView::Off },
            Request::PowerPlans,
            Request::SetPowerPlan { id: String::new() },
            Request::Fullscreen { pid: 1 },
            Request::Volume { pid: 1, change: VolumeChange::ToggleMute },
            Request::Hdr { on: None },
            Request::NvidiaCatalog,
            Request::NvidiaGet { exe: String::new() },
            Request::NvidiaSet { exe: String::new(), id: 1, value: None },
            Request::WindowShot { pid: 1 },
            Request::RecordWindow { pid: 1 },
        ];
        for r in &answer {
            assert_eq!(wait_of(r), Wait::Answer, "{r:?}");
            assert!(matches!(timeout_reply(Wait::Answer), Some(Reply::Failed { code, .. }) if code == "busy"));
        }
        assert_eq!(wait_of(&Request::ShareInvite { minutes: Some(5) }), Wait::Invite);
        assert_eq!(wait_of(&Request::CaptureDirs), Wait::Dirs);
        assert_eq!(wait_of(&Request::SaveReplay), Wait::Nothing);
        assert_eq!(wait_of(&Request::GameStopped { pid: 1 }), Wait::Nothing);
        assert!(timeout_reply(Wait::Dirs).is_none());
        assert!(matches!(timeout_reply(Wait::Invite), Some(Reply::ShareInviteFailed { .. })));
    }

    #[test]
    fn replies_by_kind() {
        assert_eq!(reply_for(Wait::Answer, Outcome::Done), Some(Reply::Done));
        assert_eq!(reply_for(Wait::Nothing, Outcome::Done), None);
        assert_eq!(reply_for(Wait::Answer, Outcome::Failed(code::NO_WINDOW, "x".into())), Some(Reply::Failed { code: "no_window".into(), reason: "x".into() }));
        assert_eq!(reply_for(Wait::Invite, Outcome::Failed(code::FAILED, "x".into())), Some(Reply::ShareInviteFailed { reason: "x".into() }));
        assert_eq!(reply_for(Wait::Answer, Outcome::Reply(Reply::Toggled { on: true })), Some(Reply::Toggled { on: true }));
        // O trabalho demorado roda na thread do pipe e vira a resposta.
        let later = Outcome::Later(Box::new(|| Reply::Toggled { on: false }));
        assert_eq!(reply_for(Wait::Answer, later), Some(Reply::Toggled { on: false }));
    }

    #[test]
    fn volume_changes_map() {
        assert!(matches!(audio_change(VolumeChange::Step(-5)), audio::Change::Step(-5)));
        assert!(matches!(audio_change(VolumeChange::Set(40)), audio::Change::Set(v) if (v - 0.4).abs() < 1e-6));
        assert!(matches!(audio_change(VolumeChange::Set(250)), audio::Change::Set(v) if v == 1.0));
        assert!(matches!(audio_change(VolumeChange::ToggleMute), audio::Change::ToggleMute));
    }

    #[test]
    fn fixed_plan_checks_the_id() {
        let plans = vec![power::Plan { guid: Default::default(), id: "381b4222-f694-41f0-9685-ff5bb260df2e".into(), name: "Equilibrado".into() }];
        assert_eq!(fixed_plan("", &plans), Ok(None));
        assert_eq!(fixed_plan("381B4222-F694-41F0-9685-FF5BB260DF2E", &plans), Ok(Some(plans[0].id.clone())));
        assert!(fixed_plan("nada", &plans).is_err());
    }

    #[test]
    fn nvidia_catalog_fits_one_frame() {
        let settings: Vec<NvidiaSetting> = nv_shown(u32::MAX, true).map(|(g, s)| nv_setting(g, s)).collect();
        assert_eq!(settings.len(), nvset::ids().len());
        let mut frame = Vec::new();
        prisma_link::write_frame(&mut frame, &Reply::NvidiaCatalog { settings }).unwrap();
        assert!(frame.len() < prisma_link::MAX_FRAME as usize / 2, "{} bytes", frame.len());
        // Fora do global, sem as opções da placa toda; driver velho, sem as novas.
        assert!(nv_shown(u32::MAX, false).all(|(_, s)| !s.global_only));
        assert!(nv_shown(0, true).all(|(_, s)| s.min_driver == 0));
    }

    #[test]
    fn nvidia_requests_are_checked() {
        assert_eq!(nv_profile(""), Ok(Profile::Global));
        assert_eq!(nv_profile("Jogo.EXE"), Ok(Profile::Game("Jogo.EXE".into())));
        for bad in ["jogo", ".exe", r"C:\jogo.exe", "../jogo.exe", "a:b.exe"] {
            assert!(nv_profile(bad).is_err(), "{bad}");
        }
        let game = Profile::Game("g.exe".into());
        // Limite de FPS: número de 10 a 1000.
        assert_eq!(nv_check(u32::MAX, &game, 0x1083_5002, Some(60)), Ok(()));
        assert_eq!(nv_check(u32::MAX, &game, 0x1083_5002, None), Ok(()));
        assert!(nv_check(u32::MAX, &game, 0x1083_5002, Some(5)).is_err());
        // Modo de baixa latência: lista 0, 1, 2.
        assert!(nv_check(u32::MAX, &game, 0x0005_F543, Some(3)).is_err());
        // G-SYNC só no global.
        assert!(nv_check(u32::MAX, &game, 0x1094_F157, Some(1)).is_err());
        assert_eq!(nv_check(u32::MAX, &Profile::Global, 0x1094_F157, Some(1)), Ok(()));
        assert!(nv_check(u32::MAX, &game, 0xDEAD_BEEF, None).is_err());
    }

    #[test]
    fn no_window_for_missing_process() {
        assert_eq!(procs::main_window(0), None);
        // O processo dos testes não tem janela.
        assert_eq!(procs::main_window(std::process::id()), None);
    }

    /// A fila é global: os testes que a usam rodam um de cada vez.
    static QUEUE_TEST: Mutex<()> = Mutex::new(());

    #[test]
    fn queue_round_trip() {
        let _g = QUEUE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let (tx, rx) = sync_channel(1);
        push(Request::SaveReplay, tx);
        drain_with(|req| {
            assert_eq!(req, Request::SaveReplay);
            Outcome::Done
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)), Ok(Outcome::Done));
    }

    #[test]
    fn capture_dirs_reply_uses_config_dir() {
        let _g = QUEUE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let (tx, rx) = sync_channel(1);
        push(Request::CaptureDirs, tx);
        drain_with(|req| {
            assert_eq!(req, Request::CaptureDirs);
            Outcome::CaptureDirs(r"C:\Videos\Prisma".into())
        });
        let outcome = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            reply_for(Wait::Dirs, outcome),
            Some(Reply::CaptureDirs { shots: r"C:\Videos\Prisma".into(), replays: r"C:\Videos\Prisma".into() })
        );
    }

    #[test]
    fn stale_entries_are_dropped() {
        let _g = QUEUE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let old = Instant::now()
            .checked_sub(WAIT + Duration::from_secs(1))
            .expect("relógio com tempo suficiente");
        let (tx_old, rx_old) = sync_channel(1);
        push_at(old, Request::SaveReplay, tx_old);
        let (tx_new, rx_new) = sync_channel(1);
        push(Request::SaveReplay, tx_new);
        let mut calls = 0;
        drain_with(|_| {
            calls += 1;
            Outcome::Done
        });
        assert_eq!(calls, 1);
        assert!(rx_old.recv_timeout(Duration::from_millis(100)).is_err());
        assert_eq!(rx_new.recv_timeout(Duration::from_secs(1)), Ok(Outcome::Done));
    }

    #[test]
    fn stale_game_notices_still_run() {
        let _g = QUEUE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let old = Instant::now()
            .checked_sub(WAIT + Duration::from_secs(1))
            .expect("relógio com tempo suficiente");
        let (tx, rx) = sync_channel(1);
        drop(rx); // o cliente já foi embora
        push_at(old, Request::GameStopped { pid: 7 }, tx);
        let mut calls = 0;
        drain_with(|_| {
            calls += 1;
            Outcome::Done
        });
        assert_eq!(calls, 1);
    }

    /// Arquivo de registro temporário, um por teste.
    #[cfg(feature = "stream")]
    fn temp_log(name: &str, text: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("prisma-link-{}-{name}.log", std::process::id()));
        std::fs::write(&p, text).unwrap();
        p
    }

    #[cfg(feature = "stream")]
    fn code_reply() -> Reply {
        let link = crate::stream::weblink::web_link("K7Q-2PX");
        Reply::ShareInviteCode { code: "K7Q-2PX".into(), link }
    }

    #[cfg(feature = "stream")]
    fn code_log(after: &str) -> String {
        format!("Convite novo ...\n{}K7Q-2PX\nPRISMA-LONGO\n{after}", crate::stream::CODE_LINE)
    }

    #[cfg(feature = "stream")]
    #[test]
    fn invite_found_in_log() {
        let p = temp_log("found", &code_log(&format!("{} (2 de 3 relés)\n", crate::stream::CODE_OK)));
        let t = Instant::now();
        let r = wait_invite(&p, Duration::from_secs(5), || true);
        let _ = std::fs::remove_file(&p);
        assert_eq!(r, code_reply());
        assert!(t.elapsed() < Duration::from_secs(1));
    }

    #[cfg(feature = "stream")]
    #[test]
    fn invite_not_published() {
        let p = temp_log("fail", &code_log(&format!("{}: mande o convite longo\n", crate::stream::CODE_FAIL)));
        let r = wait_invite(&p, Duration::from_secs(5), || true);
        let _ = std::fs::remove_file(&p);
        assert!(matches!(r, Reply::ShareInviteFailed { .. }), "{r:?}");
    }

    #[cfg(feature = "stream")]
    #[test]
    fn invite_publishing_at_deadline_still_gives_code() {
        let p = temp_log("pending", &code_log(""));
        let t = Instant::now();
        let r = wait_invite(&p, Duration::from_millis(500), || true);
        let _ = std::fs::remove_file(&p);
        assert_eq!(r, code_reply());
        assert!(t.elapsed() >= Duration::from_millis(500));
    }

    #[cfg(feature = "stream")]
    #[test]
    fn invite_written_late() {
        let p = temp_log("late", "");
        let q = p.clone();
        let w = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            std::fs::write(&q, code_log("")).unwrap();
            std::thread::sleep(Duration::from_millis(300));
            std::fs::write(&q, code_log(&format!("{} (3 de 3 relés)\n", crate::stream::CODE_OK))).unwrap();
        });
        let r = wait_invite(&p, Duration::from_secs(5), || true);
        w.join().unwrap();
        let _ = std::fs::remove_file(&p);
        assert_eq!(r, code_reply());
    }

    #[cfg(feature = "stream")]
    #[test]
    fn invite_times_out() {
        let p = temp_log("timeout", "Convite novo ...\n");
        let t = Instant::now();
        let r = wait_invite(&p, Duration::from_millis(500), || true);
        let _ = std::fs::remove_file(&p);
        assert!(matches!(r, Reply::ShareInviteFailed { .. }), "{r:?}");
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[cfg(feature = "stream")]
    #[test]
    fn invite_process_gone_answers_at_once() {
        let p = temp_log("gone", "erro: sem identidade\n");
        let t = Instant::now();
        let r = wait_invite(&p, Duration::from_secs(20), || false);
        let _ = std::fs::remove_file(&p);
        assert!(matches!(r, Reply::ShareInviteFailed { .. }), "{r:?}");
        assert!(t.elapsed() < Duration::from_secs(1));
    }

    #[cfg(feature = "stream")]
    #[test]
    fn invite_process_gone_after_code() {
        let p = temp_log("gone-code", &code_log(""));
        let r = wait_invite(&p, Duration::from_secs(20), || false);
        let _ = std::fs::remove_file(&p);
        assert_eq!(r, code_reply());
    }
}
