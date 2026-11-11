//! Só o som: o som de um PC pareado toca neste, sem vídeo, pela mesma conexão e pelo mesmo
//! pareamento do Compartilhar tela. Quem ouve pede com [`SOUND_ONLY`] no Hello; o host responde
//! com um Info sem quadros por segundo e manda só os blocos de som, como manda a quem assiste.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::link::{Link, Poll};
use super::view::{play_until, Speaker};
use super::*;
use crate::capture::mux::Codec;

const SILENCE: Duration = Duration::from_secs(5);

// ---------- Quem ouve ----------

/// `--stream-sound=nome`: toca aqui o som do PC pareado `nome`.
pub fn listen(target: &str) {
    super::net::with_host(target, run);
}

fn run(link: Arc<dyn Link>, name: &str) {
    let mut out = Vec::with_capacity(64);
    let bye = |out: &mut Vec<u8>| {
        write(out, &Msg::Bye);
        link.send(out);
        link.send(out);
    };
    // Um host de versão anterior trata isto como quem assiste e se revela mandando vídeo.
    let mut hello = Vec::new();
    write(&mut hello, &Msg::Hello { w: 1280, h: 720, hz: 60, codecs: codec_bit(Codec::H264), pid: std::process::id(), flags: SOUND_ONLY });
    let mut buf = vec![0u8; 2048];
    let start = Instant::now();
    let mut last_hello = start - Duration::from_secs(2);
    loop {
        if start.elapsed() > Duration::from_secs(15) {
            eprintln!("«{name}» não respondeu ao pedido de som");
            return bye(&mut out);
        }
        if last_hello.elapsed() > Duration::from_secs(1) {
            link.send(&hello);
            last_hello = Instant::now();
        }
        match link.recv(&mut buf, Duration::from_millis(100)) {
            Poll::Got(n) => match read(&buf[..n]) {
                Some(Msg::Info { fps, .. }) if fps != 0 => {
                    eprintln!("«{name}» tem uma versão do Prisma sem o Só o som: atualize o Prisma lá");
                    return bye(&mut out);
                }
                Some(Msg::Info { .. }) => break,
                Some(Msg::Bye) => return eprintln!("«{name}» recusou: este PC não tem a permissão de som lá"),
                _ => {}
            },
            Poll::Closed => return eprintln!("a conexão com «{name}» fechou"),
            _ => {}
        }
    }
    eprintln!("pronto: o som de «{name}» toca neste PC");
    let spk = Arc::new(Speaker::new(false));
    let quit = Arc::new(AtomicBool::new(false));
    let player = {
        let (spk, quit) = (spk.clone(), quit.clone());
        std::thread::Builder::new().name("so-o-som".into()).spawn(move || play_until(&spk, &quit)).ok()
    };
    let mut last_ping = Instant::now() - Duration::from_secs(1);
    let mut heard = Instant::now();
    let (mut next, mut lost, mut blocks) = (None::<u32>, 0u64, 0u64);
    loop {
        if last_ping.elapsed() > Duration::from_millis(500) {
            write(&mut out, &Msg::Ping { t: now() });
            link.send(&out);
            last_ping = Instant::now();
        }
        match link.recv(&mut buf, Duration::from_millis(20)) {
            Poll::Got(n) => {
                heard = Instant::now();
                match read(&buf[..n]) {
                    Some(Msg::Audio { seq, data, .. }) => {
                        blocks += 1;
                        if let Some(n) = next {
                            let gap = seq.wrapping_sub(n);
                            if gap > 0 && gap < 1000 {
                                lost += gap as u64;
                            }
                        }
                        next = Some(seq.wrapping_add(1));
                        spk.audio.lock().unwrap().extend(data.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
                    }
                    Some(Msg::Info { perms, .. }) if perms & SOUND == 0 => {
                        eprintln!("«{name}» tirou a permissão de som deste PC");
                        break;
                    }
                    Some(Msg::Bye) => {
                        eprintln!("«{name}» encerrou a conexão");
                        break;
                    }
                    _ => {}
                }
            }
            Poll::Closed => {
                eprintln!("a conexão com «{name}» fechou");
                break;
            }
            _ => {}
        }
        if heard.elapsed() > SILENCE {
            eprintln!("«{name}» parou de responder");
            break;
        }
    }
    quit.store(true, Ordering::Relaxed);
    if let Some(t) = player {
        let _ = t.join();
    }
    eprintln!("som: {blocks} blocos de 5 ms recebidos, {lost} perdidos, {} vezes acabou antes do próximo", spk.underruns.load(Ordering::Relaxed));
    bye(&mut out);
}

// ---------- Quem manda o som ----------

/// Um PC pareado ouve o som daqui, sem vídeo. A origem do som segue as opções do Host (um
/// programa só, sem os programas de chamada), como no Compartilhar tela.
pub(super) fn serve(link: Arc<dyn Link>, perms: u8, name: String, pid: u32) {
    let mut out = Vec::with_capacity(64);
    if perms & SOUND == 0 {
        eprintln!("«{name}» pediu o som, mas não tem a permissão de som aqui");
        write(&mut out, &Msg::Bye);
        link.send(&out);
        link.send(&out);
        return;
    }
    let mut info = Vec::new();
    write(&mut info, &Msg::Info { codec: VideoCodec::Std(Codec::H264), w: 0, h: 0, fps: 0, perms, recover: false, hdr10: false, yuv444: false, priority: 255 });
    link.send(&info);
    eprintln!("«{name}» ouve o som daqui");
    crate::config::log(&format!("compartilhar: «{name}» ouve o som deste PC"));
    // Quem ouve no mesmo PC: o som que ele toca voltaria em eco.
    let here = (link.same_pc() && pid != 0).then_some(pid);
    let stop = Arc::new(AtomicBool::new(false));
    let sender = {
        let (links, stop) = (Arc::new(Mutex::new(vec![link.clone()])), stop.clone());
        std::thread::Builder::new().name("so-o-som-host".into()).spawn(move || super::host::audio(links, here, stop)).ok()
    };
    let mut buf = vec![0u8; 2048];
    let mut heard = Instant::now();
    loop {
        match link.recv(&mut buf, Duration::from_millis(50)) {
            Poll::Got(n) => {
                heard = Instant::now();
                match read(&buf[..n]) {
                    // Repetido contra perda: a resposta vai de novo.
                    Some(Msg::Hello { .. }) => link.send(&info),
                    Some(Msg::Ping { t }) => {
                        write(&mut out, &Msg::Pong { t, host: now() });
                        link.send(&out);
                    }
                    Some(Msg::Bye) => break,
                    _ => {}
                }
            }
            Poll::Closed => break,
            _ => {}
        }
        if heard.elapsed() > SILENCE {
            break;
        }
    }
    stop.store(true, Ordering::Relaxed);
    if let Some(t) = sender {
        let _ = t.join();
    }
    eprintln!("«{name}» parou de ouvir o som daqui");
    crate::config::log(&format!("compartilhar: «{name}» parou de ouvir o som deste PC"));
}
