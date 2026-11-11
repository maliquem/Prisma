//! Ponte com o Prisma Launcher: um pipe nomeado atende o launcher numa thread própria.
//! A thread do pipe nunca toca no `App`: cada pedido entra numa fila, a janela de controle
//! recebe `WM_LINK` e a thread principal responde.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use prisma_link::{cap, Page as LinkPage, Reply, Request, ServerInfo};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::app::App;
use crate::pages::Page;

/// Avisa a janela de controle que há pedidos do launcher na fila.
pub const WM_LINK: u32 = WM_APP + 92;

/// Quanto a thread do pipe espera a thread principal antes de seguir sem responder.
const WAIT: Duration = Duration::from_secs(5);

/// Quanto a thread do pipe espera o código aparecer no registro do convite. Com `WAIT`, fica
/// abaixo dos 30 s em que o launcher desiste.
#[cfg(feature = "stream")]
const INVITE_WAIT: Duration = Duration::from_secs(20);

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Done,
    /// O convite está sendo criado pelo processo `pid`; o código aparece neste registro.
    #[cfg_attr(not(feature = "stream"), allow(dead_code))]
    InviteLog { log: PathBuf, pid: u32 },
    Failed(String),
}

/// Cada pedido leva a hora em que entrou: pedido mais velho que `WAIT` já perdeu o cliente.
type Entry = (Instant, Request, SyncSender<Outcome>);

static QUEUE: Mutex<VecDeque<Entry>> = Mutex::new(VecDeque::new());

/// O que este Prisma sabe fazer pelo launcher. Sem `page.game_profile`: ainda não há uma
/// página de perfil por jogo.
pub fn caps() -> Vec<String> {
    let mut caps = vec![
        cap::GAME_EVENTS,
        cap::PAGE_METER,
        cap::PAGE_SHARE,
        cap::SAVE_REPLAY,
    ];
    if cfg!(feature = "stream") {
        caps.push(cap::SHARE_INVITE);
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
            let invite = matches!(req, Request::ShareInvite);
            let (tx, rx) = sync_channel(1);
            push(req, tx);
            unsafe {
                let _ = PostMessageW(Some(HWND(hub as *mut _)), WM_LINK, WPARAM(0), LPARAM(0));
            }
            match rx.recv_timeout(WAIT) {
                Ok(outcome) => reply_for(invite, outcome),
                // O launcher sempre recebe resposta do convite, mesmo com a janela ocupada.
                Err(_) if invite => Some(Reply::ShareInviteFailed {
                    reason: "o Prisma não respondeu".into(),
                }),
                Err(_) => None,
            }
        });
        if let Err(e) = res {
            crate::config::log(&format!("ponte com o launcher: pipe indisponível ({e})"));
        }
    });
}

/// Resposta ao cliente: só o convite responde algo; o resto não tem resposta. Roda na thread
/// do pipe: esperar o código no registro não trava a janela.
fn reply_for(invite: bool, outcome: Outcome) -> Option<Reply> {
    match outcome {
        Outcome::Failed(reason) if invite => Some(Reply::ShareInviteFailed { reason }),
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

/// Na thread principal, no `WM_LINK`: atende tudo o que está na fila.
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
            Request::ShareInvite => match crate::pages::share::create_invite(self) {
                Some((log, pid)) => Outcome::InviteLog { log, pid },
                None => Outcome::Failed("não deu para criar o convite".into()),
            },
            #[cfg(not(feature = "stream"))]
            Request::ShareInvite => Outcome::Failed("indisponível".into()),
            Request::GameStarted { pid, exe, .. } => {
                self.detector.hint(pid, &exe);
                Outcome::Done
            }
            Request::GameStopped { pid } => {
                self.detector.unhint(pid);
                Outcome::Done
            }
            Request::Hello { .. } | Request::Unknown => Outcome::Done,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;
    use std::time::Duration;

    #[test]
    fn caps_follow_features() {
        let caps = caps();
        let has = |c: &str| caps.iter().any(|x| x == c);
        assert!(has(cap::PAGE_METER));
        assert!(has(cap::PAGE_SHARE));
        assert!(has(cap::SAVE_REPLAY));
        assert!(has(cap::GAME_EVENTS));
        assert_eq!(has(cap::SHARE_INVITE), cfg!(feature = "stream"));
        assert!(!has(cap::PAGE_GAME_PROFILE));
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
