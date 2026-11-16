//! Troca do perfil ao vivo: uma thread por fluxo (fora do tempo real) olha o arquivo do perfil,
//! monta a corrente nova e entrega; o tempo real pega com `try_lock` e mistura a velha com a nova
//! por 20 ms. Nada é alocado nem liberado no tempo real: a corrente velha volta para a thread.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use prisma_eq::{Chain, Profile, channel_names};

use crate::log;

/// Tempo da mistura entre o perfil velho e o novo.
const FADE_MS: f32 = 20.0;
// shortcut: olha o arquivo a cada 250 ms em vez de ReadDirectoryChangesW; trocar se muitos fluxos
// abertos ao mesmo tempo pesarem.
const POLL: Duration = Duration::from_millis(250);

#[derive(Clone)]
pub struct Params {
    pub id: u32,
    pub dir: PathBuf,
    /// GUID do aparelho (vazio: só o perfil geral).
    pub device: String,
    pub mask: u32,
    pub ch: u32,
    pub rate: f32,
}

/// Perfil do aparelho, senão o geral.
fn profile_path(p: &Params) -> Option<PathBuf> {
    let candidates = [(!p.device.is_empty()).then(|| p.dir.join(format!("perfil-{}.txt", p.device))), Some(p.dir.join("perfil.txt"))];
    candidates.into_iter().flatten().find(|f| f.exists())
}

/// Arquivo, data e tamanho: muda quando o perfil é salvo, trocado ou apagado.
type Stamp = Option<(PathBuf, Option<SystemTime>, u64)>;

fn stamp(p: &Params) -> Stamp {
    let path = profile_path(p)?;
    let meta = std::fs::metadata(&path).ok();
    Some((path, meta.as_ref().and_then(|m| m.modified().ok()), meta.map_or(0, |m| m.len())))
}

/// Corrente do perfil; `None` = passar o som sem mexer (sem perfil, perfil plano ou com erro).
fn build(p: &Params, path: Option<&Path>) -> Option<Chain> {
    let Some(path) = path else {
        log(&format!("instância {} sem perfil: passa o som sem mexer", p.id));
        return None;
    };
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            log(&format!("instância {} não li {}: {e}", p.id, path.display()));
            return None;
        }
    };
    match Profile::parse(&text) {
        Ok(profile) => {
            log(&format!("instância {} perfil {} ({} filtros, canais {:?})", p.id, path.display(), profile.filters.len(), channel_names(p.mask, p.ch as usize)));
            Some(Chain::new(&profile, p.mask, p.ch as usize, p.rate as f64)).filter(|c| !c.is_flat())
        }
        Err(e) => {
            log(&format!("instância {} perfil {} com erro, passa o som sem mexer: {e}", p.id, path.display()));
            None
        }
    }
}

/// Entre a thread e o tempo real. `fresh`: corrente nova para usar; senão, `chain` é a velha que
/// o tempo real devolveu, para a thread liberar.
#[derive(Default)]
struct Slot {
    fresh: bool,
    chain: Option<Chain>,
}

#[derive(Default)]
struct Rt {
    cur: Option<Chain>,
    old: Option<Chain>,
    fading: bool,
    fade: usize,
    fade_len: usize,
    ch: usize,
    scratch: Vec<f32>,
}

#[derive(Default)]
pub struct Live {
    rt: Mutex<Rt>,
    slot: Arc<Mutex<Slot>>,
    watcher: Mutex<Option<(Arc<AtomicBool>, JoinHandle<()>)>>,
}

impl Live {
    /// `LockForProcess`: monta o perfil atual e liga a thread que olha o arquivo.
    pub fn start(&self, p: Params, max_frames: u32) {
        self.stop();
        let first = stamp(&p);
        let chain = build(&p, first.as_ref().map(|s| s.0.as_path()));
        if let Ok(mut rt) = self.rt.lock() {
            *rt = Rt {
                cur: chain,
                ch: p.ch as usize,
                fade_len: ((FADE_MS / 1000.0 * p.rate) as usize).max(1),
                scratch: vec![0.0; max_frames as usize * p.ch as usize],
                ..Rt::default()
            };
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (slot, quit) = (self.slot.clone(), stop.clone());
        let thread = std::thread::Builder::new().name("prisma-eq".into()).spawn(move || {
            let mut last = first;
            while !quit.load(Relaxed) {
                std::thread::park_timeout(POLL);
                if quit.load(Relaxed) {
                    break;
                }
                let now = stamp(&p);
                let retired = if now != last {
                    last = now;
                    let chain = build(&p, last.as_ref().map(|s| s.0.as_path()));
                    slot.lock().ok().map(|mut s| std::mem::replace(&mut *s, Slot { fresh: true, chain }))
                } else {
                    // Libera aqui a corrente velha que o tempo real devolveu.
                    slot.lock().ok().filter(|s| !s.fresh && s.chain.is_some()).map(|mut s| std::mem::take(&mut *s))
                };
                drop(retired);
            }
        });
        if let (Ok(t), Ok(mut w)) = (thread, self.watcher.lock()) {
            *w = Some((stop, t));
        }
    }

    /// `UnlockForProcess` e fim da instância: para a thread e esvazia tudo.
    pub fn stop(&self) {
        if let Some((stop, t)) = self.watcher.lock().ok().and_then(|mut w| w.take()) {
            stop.store(true, Relaxed);
            t.thread().unpark();
            let _ = t.join();
        }
        if let Ok(mut rt) = self.rt.lock() {
            *rt = Rt::default();
        }
        if let Ok(mut s) = self.slot.lock() {
            *s = Slot::default();
        }
    }

    /// Tempo real: quadros intercalados, no lugar. Trava ocupada: passa o som sem mexer.
    pub fn process(&self, data: &mut [f32]) {
        let Ok(mut guard) = self.rt.try_lock() else { return };
        let Rt { cur, old, fading, fade, fade_len, ch, scratch } = &mut *guard;
        if *ch == 0 {
            return;
        }
        // Corrente nova pronta: começa a mistura. Uma velha que ainda não voltou vai no lugar dela.
        if !*fading
            && let Ok(mut s) = self.slot.try_lock()
            && s.fresh
        {
            let new = s.chain.take();
            s.chain = old.take();
            s.fresh = false;
            *old = std::mem::replace(cur, new);
            (*fading, *fade) = (true, 0);
        }
        if *fading && data.len() <= scratch.len() {
            let dry = &mut scratch[..data.len()];
            dry.copy_from_slice(data);
            if let Some(o) = old.as_mut() {
                o.process(dry);
            }
            if let Some(c) = cur.as_mut() {
                c.process(data);
            }
            for (i, (frame, prev)) in data.chunks_exact_mut(*ch).zip(dry.chunks_exact(*ch)).enumerate() {
                let t = ((*fade + i) as f32 / *fade_len as f32).min(1.0);
                for (v, p) in frame.iter_mut().zip(prev) {
                    *v = p + (*v - p) * t;
                }
            }
            *fade += data.len() / *ch;
            *fading = *fade < *fade_len;
        } else {
            // Bloco maior que o previsto no LockForProcess: pula a mistura.
            *fading = false;
            if let Some(c) = cur.as_mut() {
                c.process(data);
            }
        }
        // Mistura acabou: devolve a velha para a thread liberar.
        if !*fading
            && old.is_some()
            && let Ok(mut s) = self.slot.try_lock()
            && !s.fresh
            && s.chain.is_none()
        {
            s.chain = old.take();
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.stop();
    }
}
