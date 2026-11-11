#![cfg(windows)]

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::windows::io::FromRawHandle;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use prisma_link::{
    Client, PROTOCOL, Page, Reply, Request, ServerInfo, current_user_sid, pipe_name, read_frame,
    serve, write_frame,
};

const WAIT: Duration = Duration::from_secs(2);

fn unique_name() -> String {
    static N: AtomicU32 = AtomicU32::new(0);
    format!(
        r"\\.\pipe\prisma-link-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

fn info() -> ServerInfo {
    ServerInfo {
        app_version: "9.9.9".into(),
        caps: vec!["page.meter".into()],
    }
}

/// Sobe `serve` numa thread solta; o handler manda cada Request para o canal
/// e responde com `reply(&req)`.
fn spawn_server(
    reply: impl Fn(&Request) -> Option<Reply> + Send + 'static,
) -> (String, mpsc::Receiver<Request>) {
    let name = unique_name();
    let (tx, rx) = mpsc::channel();
    let n = name.clone();
    thread::spawn(move || {
        let _ = serve(&n, info(), move |req| {
            let r = reply(&req);
            let _ = tx.send(req);
            r
        });
    });
    (name, rx)
}

fn retry<T>(mut f: impl FnMut() -> io::Result<T>) -> T {
    let start = Instant::now();
    loop {
        match f() {
            Ok(v) => return v,
            Err(e) if start.elapsed() > WAIT => panic!("não conectou: {e}"),
            Err(_) => thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn connect(name: &str) -> Client {
    retry(|| Client::connect(name, "0.1.0"))
}

fn raw_connect(name: &str) -> File {
    retry(|| OpenOptions::new().read(true).write(true).open(name))
}

#[test]
fn pipe_name_uses_sid() {
    assert_eq!(pipe_name("S-1-5-21-1"), r"\\.\pipe\prisma-link-S-1-5-21-1");
}

#[test]
fn current_sid_looks_like_a_sid() {
    let sid = current_user_sid().unwrap();
    assert!(sid.starts_with("S-1-"), "{sid}");
}

#[test]
fn handshake_returns_server_caps() {
    let (name, _rx) = spawn_server(|_| None);
    let c = connect(&name);
    let caps = c.capabilities();
    assert_eq!(caps.protocol, 1);
    assert_eq!(caps.protocol, PROTOCOL);
    assert_eq!(caps.app_version, "9.9.9");
    assert_eq!(caps.caps, vec!["page.meter".to_string()]);
}

/// Servidor falso cru: cria o pipe direto, lê o Hello e responde o que quiser.
fn fake_server(name: &str, reply: Reply, close_after: bool) -> mpsc::Receiver<()> {
    use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows::core::HSTRING;

    let h = unsafe {
        CreateNamedPipeW(
            &HSTRING::from(name),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            1,
            65_536,
            65_536,
            0,
            None,
        )
    };
    assert!(!h.is_invalid());
    let raw = h.0 as usize;
    let (done_tx, done_rx) = mpsc::channel();
    thread::spawn(move || {
        let h = windows::Win32::Foundation::HANDLE(raw as *mut _);
        let _ = unsafe { ConnectNamedPipe(h, None) };
        let mut f = unsafe { File::from_raw_handle(raw as *mut _) };
        let hello: Option<Request> = read_frame(&mut f).unwrap();
        assert!(matches!(hello, Some(Request::Hello { .. })));
        write_frame(&mut f, &reply).unwrap();
        if close_after {
            drop(f);
            let _ = done_tx.send(());
        } else {
            let _ = done_tx.send(());
            // segura o handle até o processo acabar
            std::mem::forget(f);
        }
    });
    done_rx
}

#[test]
fn newer_server_protocol_is_accepted() {
    let name = unique_name();
    let _done = fake_server(
        &name,
        Reply::Capabilities {
            protocol: 2,
            app_version: "2.0.0".into(),
            caps: vec!["page.meter".into(), "teleport".into()],
        },
        false,
    );
    let c = connect(&name);
    assert_eq!(c.capabilities().protocol, 2);
    assert!(c.capabilities().has("teleport"));
    assert!(c.capabilities().has("page.meter"));
    assert!(c.alive());
}

#[test]
fn requests_reach_handler_in_order() {
    let (name, rx) = spawn_server(|_| None);
    let mut c = connect(&name);
    let sent = [
        Request::GameStarted {
            pid: 42,
            exe: r"C:\g.exe".into(),
            name: "G".into(),
        },
        Request::OpenPage { page: Page::Meter },
        Request::SaveReplay,
    ];
    for r in &sent {
        c.send(r).unwrap();
    }
    for r in &sent {
        assert_eq!(&rx.recv_timeout(WAIT).unwrap(), r);
    }
}

#[test]
fn reply_comes_back() {
    let (name, _rx) = spawn_server(|req| match req {
        Request::ShareInvite { .. } => Some(Reply::ShareInviteCode {
            code: "ABC-123".into(),
            link: "https://x/#ABC-123".into(),
        }),
        _ => None,
    });
    let mut c = connect(&name);
    c.send(&Request::ShareInvite { minutes: None }).unwrap();
    assert_eq!(
        c.recv().unwrap(),
        Reply::ShareInviteCode {
            code: "ABC-123".into(),
            link: "https://x/#ABC-123".into(),
        }
    );
}

#[test]
fn unknown_message_is_skipped() {
    let (name, rx) = spawn_server(|_| None);
    let mut f = raw_connect(&name);
    write_frame(
        &mut f,
        &Request::Hello {
            protocol: 1,
            app_version: "x".into(),
        },
    )
    .unwrap();
    let caps: Option<Reply> = read_frame(&mut f).unwrap();
    assert!(matches!(caps, Some(Reply::Capabilities { .. })));
    write_frame(&mut f, &serde_json::json!({"type": "future"})).unwrap();
    write_frame(&mut f, &Request::SaveReplay).unwrap();
    assert_eq!(rx.recv_timeout(WAIT).unwrap(), Request::SaveReplay);
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
}

/// Depois que o servidor derruba o cliente, a leitura acaba: fim limpo ou erro
/// de pipe desconectado/quebrado, nunca um quadro.
fn assert_dropped(f: &mut File) {
    match read_frame::<_, Reply>(f) {
        Ok(None) => {}
        Ok(Some(r)) => panic!("esperava fim, veio {r:?}"),
        Err(e) => assert!(
            matches!(
                e.raw_os_error(),
                Some(109 /* BROKEN_PIPE */) | Some(233 /* PIPE_NOT_CONNECTED */)
            ),
            "{e:?}"
        ),
    }
}

#[test]
fn first_frame_must_be_hello() {
    let (name, rx) = spawn_server(|_| None);
    let mut f = raw_connect(&name);
    write_frame(&mut f, &Request::SaveReplay).unwrap();
    assert_dropped(&mut f);
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
}

#[test]
fn bad_client_does_not_kill_server() {
    let (name, _rx) = spawn_server(|_| None);
    let mut f = raw_connect(&name);
    write_frame(
        &mut f,
        &Request::Hello {
            protocol: 1,
            app_version: "x".into(),
        },
    )
    .unwrap();
    let _caps: Option<Reply> = read_frame(&mut f).unwrap();
    f.write_all(&1_000_000u32.to_le_bytes()).unwrap();
    f.flush().unwrap();
    assert_dropped(&mut f);
    drop(f);
    let c = connect(&name);
    assert_eq!(c.capabilities().app_version, "9.9.9");
}

#[test]
fn alive_turns_false_when_server_drops() {
    let name = unique_name();
    let done = fake_server(
        &name,
        Reply::Capabilities {
            protocol: 1,
            app_version: "1".into(),
            caps: vec![],
        },
        true,
    );
    let c = connect(&name);
    done.recv_timeout(WAIT).unwrap();
    let start = Instant::now();
    while c.alive() {
        assert!(start.elapsed() < WAIT, "alive() continuou true");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn second_server_on_same_name_fails() {
    let (name, _rx) = spawn_server(|_| None);
    // garante que o primeiro já criou o pipe
    let _c = connect(&name);
    let r = serve(&name, info(), |_| None);
    assert!(r.is_err());
}

/// Servidor cru que aceita, lê o Hello e nunca responde (segura o handle).
fn silent_server(name: &str) {
    use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows::core::HSTRING;

    let h = unsafe {
        CreateNamedPipeW(
            &HSTRING::from(name),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            1,
            65_536,
            65_536,
            0,
            None,
        )
    };
    assert!(!h.is_invalid());
    let raw = h.0 as usize;
    thread::spawn(move || {
        let h = windows::Win32::Foundation::HANDLE(raw as *mut _);
        let _ = unsafe { ConnectNamedPipe(h, None) };
        let mut f = unsafe { File::from_raw_handle(raw as *mut _) };
        let _hello: Option<Request> = read_frame(&mut f).unwrap();
        std::mem::forget(f);
    });
}

#[test]
fn connect_gives_up_when_hello_is_never_answered() {
    let name = unique_name();
    silent_server(&name);
    let start = Instant::now();
    let err = loop {
        match Client::connect(&name, "0.1.0") {
            Ok(_) => panic!("conectou sem Capabilities"),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                assert!(start.elapsed() < WAIT, "pipe não apareceu");
                thread::sleep(Duration::from_millis(10));
            }
            Err(e) => break e,
        }
    };
    assert_eq!(err.kind(), io::ErrorKind::TimedOut, "{err:?}");
    let took = start.elapsed();
    assert!(
        took >= prisma_link::CONNECT_TIMEOUT && took < prisma_link::CONNECT_TIMEOUT + WAIT,
        "{took:?}"
    );
}

#[test]
fn recv_timeout_gives_up_and_still_reads_replies() {
    let (name, _rx) = spawn_server(|req| match req {
        Request::ShareInvite { .. } => Some(Reply::ShareInviteFailed { reason: "x".into() }),
        _ => None,
    });
    let mut c = connect(&name);
    let start = Instant::now();
    let err = c.recv_timeout(Duration::from_millis(150)).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    assert!(start.elapsed() >= Duration::from_millis(150));
    assert!(start.elapsed() < WAIT);
    c.send(&Request::ShareInvite { minutes: None }).unwrap();
    assert_eq!(
        c.recv_timeout(WAIT).unwrap(),
        Reply::ShareInviteFailed { reason: "x".into() }
    );
}

#[test]
fn recv_timeout_reports_dropped_server() {
    let name = unique_name();
    let done = fake_server(
        &name,
        Reply::Capabilities {
            protocol: 1,
            app_version: "1".into(),
            caps: vec![],
        },
        true,
    );
    let mut c = connect(&name);
    done.recv_timeout(WAIT).unwrap();
    let start = Instant::now();
    let err = c.recv_timeout(WAIT).unwrap_err();
    assert_ne!(err.kind(), io::ErrorKind::TimedOut, "{err:?}");
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn capture_dirs_reply_comes_back() {
    let (name, rx) = spawn_server(|req| match req {
        Request::CaptureDirs => Some(Reply::CaptureDirs {
            shots: r"C:\V\Prisma".into(),
            replays: r"C:\V\Prisma".into(),
        }),
        _ => None,
    });
    let mut c = connect(&name);
    c.send(&Request::CaptureDirs).unwrap();
    assert_eq!(rx.recv_timeout(WAIT).unwrap(), Request::CaptureDirs);
    assert_eq!(
        c.recv_timeout(WAIT).unwrap(),
        Reply::CaptureDirs {
            shots: r"C:\V\Prisma".into(),
            replays: r"C:\V\Prisma".into(),
        }
    );
}
