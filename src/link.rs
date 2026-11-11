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

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Done,
    /// O convite está sendo criado; o código aparece neste registro.
    #[allow(dead_code)]
    InviteLog(PathBuf),
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
            reply_for(invite, rx.recv_timeout(WAIT).ok()?)
        });
        if let Err(e) = res {
            crate::config::log(&format!("ponte com o launcher: pipe indisponível ({e})"));
        }
    });
}

/// Resposta ao cliente: só o convite responde algo; o resto não tem resposta.
fn reply_for(invite: bool, outcome: Outcome) -> Option<Reply> {
    match outcome {
        Outcome::Failed(reason) if invite => Some(Reply::ShareInviteFailed { reason }),
        Outcome::InviteLog(_) if invite => Some(Reply::ShareInviteFailed {
            reason: "indisponível".into(),
        }),
        _ => None,
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
}
