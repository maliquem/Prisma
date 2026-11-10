//! Controles virtuais do co-op, no host: cada controle de quem assiste vira um controle tipo Xbox
//! aqui, pela ponte que ela escolher.
//!
//! - **ViGEmBus** (Nefarius, programa de terceiros já instalado no PC): cliente `vigem-client`, MIT.
//! - **VIIPER** (servidor separado, GPL, que cria o controle pelo USB/IP): cliente `viiper-client`,
//!   MIT, que só conversa com ele pela rede local. Nenhum código do servidor entra no Prisma.
//!
//! `PRISMA_PADS=vigem`, `viiper` ou `auto` (padrão: ViGEmBus e, sem ele, VIIPER).
//! `PRISMA_VIIPER=endereço:porta` muda o endereço do VIIPER (padrão 127.0.0.1:3242).

use super::PadState;

/// Um controle virtual ligado no host. Sai do Windows quando é solto (drop).
pub trait VirtualPad {
    /// Manda o estado novo. Falso: o controle caiu.
    fn update(&mut self, s: &PadState) -> bool;
    /// Vibração pedida pelo jogo agora: motor grave e motor agudo, de 0 a 255.
    fn rumble(&self) -> (u8, u8);
    /// Vaga do XInput (0 a 3), quando se sabe.
    fn slot(&self) -> Option<u8>;
}

#[cfg(feature = "stream")]
/// Vagas do XInput ocupadas agora, um bit por vaga.
pub fn xinput_mask() -> u8 {
    use windows::Win32::UI::Input::XboxController::{XInputGetState, XINPUT_STATE};
    let mut mask = 0;
    for i in 0..4u32 {
        let mut st = XINPUT_STATE::default();
        if unsafe { XInputGetState(i, &mut st) } == 0 {
            mask |= 1 << i;
        }
    }
    mask
}

pub struct Bridge {
    inner: Inner,
    pub name: &'static str,
}

enum Inner {
    #[cfg(feature = "stream")]
    Vigem(std::sync::Arc<vigem_client::Client>),
    #[cfg(feature = "stream")]
    Viiper { addr: std::net::SocketAddr, bus: u32 },
    #[cfg(not(feature = "stream"))]
    #[allow(dead_code)]
    None,
    /// Para os testes: controles de mentira, que anotam o que recebem.
    #[cfg(test)]
    Fake(FakeLog),
}

/// O que os controles de mentira receberam: (vaga, estado).
#[cfg(test)]
pub type FakeLog = std::sync::Arc<std::sync::Mutex<Vec<(u8, PadState)>>>;

#[cfg(test)]
struct FakePad {
    slot: u8,
    log: FakeLog,
}

#[cfg(test)]
impl VirtualPad for FakePad {
    fn update(&mut self, s: &PadState) -> bool {
        self.log.lock().unwrap().push((self.slot, *s));
        true
    }
    fn rumble(&self) -> (u8, u8) {
        (0, 0)
    }
    fn slot(&self) -> Option<u8> {
        Some(self.slot)
    }
}

impl Bridge {
    /// Abre a ponte escolhida. O erro diz o que falta, em português.
    #[cfg(feature = "stream")]
    pub fn open() -> Result<Bridge, String> {
        let want = std::env::var("PRISMA_PADS").unwrap_or_default().to_lowercase();
        let vigem = || match vigem_client::Client::connect() {
            Ok(c) => Ok(Bridge { inner: Inner::Vigem(std::sync::Arc::new(c)), name: "ViGEmBus" }),
            Err(vigem_client::Error::BusNotFound) => Err("o ViGEmBus não está instalado".to_string()),
            Err(e) => Err(format!("o ViGEmBus não abriu: {e:?}")),
        };
        match want.as_str() {
            "vigem" => vigem(),
            "viiper" => viiper::open(),
            _ => vigem().or_else(|a| viiper::open().map_err(|b| format!("{a}; {b}"))),
        }
    }

    #[cfg(not(feature = "stream"))]
    pub fn open() -> Result<Bridge, String> {
        Err("os controles só existem compilando com --features stream".to_string())
    }

    /// Ponte de mentira para os testes: as vagas vão em ordem, a partir de 0.
    #[cfg(test)]
    pub fn fake(log: FakeLog) -> Bridge {
        Bridge { inner: Inner::Fake(log), name: "teste" }
    }

    /// Liga um controle novo.
    pub fn plug(&self) -> Result<Box<dyn VirtualPad>, String> {
        match &self.inner {
            #[cfg(test)]
            Inner::Fake(log) => {
                // A vaga é quantos já foram ligados (cada ligação fica anotada com todos os botões).
                let n = log.lock().unwrap().iter().filter(|(_, s)| s.buttons == 0xFFFF).count() as u8;
                log.lock().unwrap().push((n, PadState { buttons: 0xFFFF, ..Default::default() }));
                Ok(Box::new(FakePad { slot: n, log: log.clone() }))
            }
            #[cfg(feature = "stream")]
            Inner::Vigem(c) => vigem::plug(c),
            #[cfg(feature = "stream")]
            Inner::Viiper { addr, bus } => viiper::plug(*addr, *bus),
            #[cfg(not(feature = "stream"))]
            Inner::None => Err(String::new()),
        }
    }
}

/// Espera o controle novo aparecer numa vaga do XInput que estava livre.
#[cfg(feature = "stream")]
fn new_slot(before: u8, wait: std::time::Duration) -> Option<u8> {
    let t = std::time::Instant::now();
    loop {
        let fresh = xinput_mask() & !before;
        if fresh != 0 {
            return Some(fresh.trailing_zeros() as u8);
        }
        if t.elapsed() > wait {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[cfg(feature = "stream")]
fn pack(low: u8, high: u8) -> u16 {
    (low as u16) << 8 | high as u16
}

#[cfg(feature = "stream")]
fn unpack(v: u16) -> (u8, u8) {
    ((v >> 8) as u8, v as u8)
}

#[cfg(feature = "stream")]
mod vigem {
    use std::sync::atomic::{AtomicU16, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use vigem_client::{Client, TargetId, XButtons, XGamepad, Xbox360Wired};

    use super::*;

    struct Pad {
        t: Xbox360Wired<Arc<Client>>,
        rumble: Arc<AtomicU16>,
        slot: Option<u8>,
    }

    pub fn plug(c: &Arc<Client>) -> Result<Box<dyn VirtualPad>, String> {
        let before = xinput_mask();
        let mut t = Xbox360Wired::new(c.clone(), TargetId::XBOX360_WIRED);
        t.plugin().map_err(|e| format!("o ViGEmBus não ligou o controle: {e:?}"))?;
        let _ = t.wait_ready();
        // O índice do ViGEm pode chegar antes do XInput dar a vaga (e repetir a de outro controle):
        // só vale se for uma vaga que estava livre.
        let told = t.get_user_index().ok().filter(|&i| i < 4).map(|i| i as u8);
        let slot = new_slot(before, Duration::from_millis(1000)).or(told.filter(|&i| before & (1 << i) == 0));
        if std::env::var("PRISMA_PADS_DEBUG").is_ok() {
            eprintln!("vigem: índice {told:?}, vagas antes {before:04b}, depois {:04b}, vaga {slot:?}", xinput_mask());
        }
        let rumble = Arc::new(AtomicU16::new(0));
        // A vibração chega por uma linha própria, que acaba sozinha quando o controle sai.
        if let Ok(n) = t.request_notification() {
            let r = rumble.clone();
            n.spawn_thread(move |_, x| r.store(pack(x.large_motor, x.small_motor), Ordering::Relaxed));
        }
        Ok(Box::new(Pad { t, rumble, slot }))
    }

    impl VirtualPad for Pad {
        fn update(&mut self, s: &PadState) -> bool {
            let g = XGamepad { buttons: XButtons { raw: s.buttons }, left_trigger: s.lt, right_trigger: s.rt, thumb_lx: s.lx, thumb_ly: s.ly, thumb_rx: s.rx, thumb_ry: s.ry };
            self.t.update(&g).is_ok()
        }
        fn rumble(&self) -> (u8, u8) {
            unpack(self.rumble.load(Ordering::Relaxed))
        }
        fn slot(&self) -> Option<u8> {
            self.slot
        }
    }
}

#[cfg(feature = "stream")]
mod viiper {
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicU16, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use viiper_client::devices::xbox360::Xbox360Input;
    use viiper_client::{DeviceCreateRequest, DeviceStream, ViiperClient};

    use super::*;

    const ADDR: &str = "127.0.0.1:3242";

    pub fn open() -> Result<Bridge, String> {
        let text = std::env::var("PRISMA_VIIPER").unwrap_or_else(|_| ADDR.to_string());
        let addr: SocketAddr = text.parse().map_err(|_| format!("endereço do VIIPER inválido: {text}"))?;
        // Sem servidor, a conexão recusa na hora; só espera de verdade num endereço de outra máquina.
        std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|_| format!("o VIIPER não está rodando em {addr}"))?;
        let c = ViiperClient::new(addr);
        let bus = match c.bus_list().map_err(|e| format!("o VIIPER não respondeu: {e}"))?.buses.first() {
            Some(&b) => b,
            None => c.bus_create(None).map_err(|e| format!("o VIIPER não criou o barramento: {e}"))?.bus_id,
        };
        Ok(Bridge { inner: Inner::Viiper { addr, bus }, name: "VIIPER" })
    }

    struct Pad {
        stream: Option<DeviceStream>,
        addr: SocketAddr,
        bus: u32,
        dev: String,
        rumble: Arc<AtomicU16>,
        slot: Option<u8>,
    }

    pub fn plug(addr: SocketAddr, bus: u32) -> Result<Box<dyn VirtualPad>, String> {
        let before = xinput_mask();
        let c = ViiperClient::new(addr);
        let req = DeviceCreateRequest { r#type: Some("xbox360".into()), id_vendor: None, id_product: None, device_specific: None };
        let dev = c.bus_device_add(bus, &req).map_err(|e| format!("o VIIPER não criou o controle: {e}"))?.dev_id;
        // O VIIPER tira o controle se a conexão dele não abrir em 5 s.
        let mut stream = match c.connect_device(bus, &dev) {
            Ok(s) => s,
            Err(e) => {
                let _ = c.bus_device_remove(bus, Some(&dev));
                return Err(format!("o VIIPER não abriu o controle: {e}"));
            }
        };
        let rumble = Arc::new(AtomicU16::new(0));
        let r = rumble.clone();
        let _ = stream.on_output(move |rd| {
            let mut b = [0u8; 2];
            rd.read_exact(&mut b)?;
            r.store(pack(b[0], b[1]), Ordering::Relaxed);
            Ok(())
        });
        let slot = new_slot(before, Duration::from_millis(1500));
        Ok(Box::new(Pad { stream: Some(stream), addr, bus, dev, rumble, slot }))
    }

    impl VirtualPad for Pad {
        fn update(&mut self, s: &PadState) -> bool {
            let Some(st) = &mut self.stream else { return false };
            let i = Xbox360Input { buttons: s.buttons as u32, lt: s.lt, rt: s.rt, lx: s.lx, ly: s.ly, rx: s.rx, ry: s.ry, reserved: [0; 6] };
            st.send(&i).is_ok()
        }
        fn rumble(&self) -> (u8, u8) {
            unpack(self.rumble.load(Ordering::Relaxed))
        }
        fn slot(&self) -> Option<u8> {
            self.slot
        }
    }

    impl Drop for Pad {
        fn drop(&mut self) {
            self.stream = None;
            let _ = ViiperClient::new(self.addr).bus_device_remove(self.bus, Some(&self.dev));
        }
    }
}

/// Bancada da ponte: liga dois controles, confere no XInput o que chegou, a vibração e o tempo até
/// o Windows ver a mudança, e solta. `--test-pads=saida.txt`.
#[cfg(feature = "stream")]
pub fn bench(out: &str) {
    use std::fmt::Write as _;
    use std::time::{Duration, Instant};
    use windows::Win32::UI::Input::XboxController::*;
    let mut r = String::from("Ponte de controles (fase 5)\n");
    let report = |r: &str| {
        let _ = std::fs::write(out, r);
    };
    let _ = writeln!(r, "vagas do XInput ocupadas antes: {:04b}", xinput_mask());
    let b = match Bridge::open() {
        Ok(b) => b,
        Err(e) => {
            let _ = writeln!(r, "ponte: {e}");
            return report(&r);
        }
    };
    let _ = writeln!(r, "ponte: {}", b.name);
    let mut pads = Vec::new();
    for n in 0..2 {
        let t = Instant::now();
        match b.plug() {
            Ok(p) => {
                let _ = writeln!(r, "controle {}: vaga {:?}, ligado em {:.0} ms", n + 1, p.slot(), t.elapsed().as_secs_f64() * 1000.0);
                pads.push(p);
            }
            Err(e) => {
                let _ = writeln!(r, "controle {}: {e}", n + 1);
            }
        }
    }
    let _ = writeln!(r, "vagas ocupadas com os virtuais: {:04b}", xinput_mask());
    std::thread::sleep(Duration::from_secs(1));
    let _ = writeln!(r, "um segundo depois: {:04b}", xinput_mask());
    let read = |slot: u8| {
        let mut st = XINPUT_STATE::default();
        (unsafe { XInputGetState(slot as u32, &mut st) } == 0).then_some(st.Gamepad)
    };
    if let Some(p) = pads.first_mut() {
        if let Some(slot) = p.slot() {
            // O que vai é o que o Windows lê.
            let s = PadState { buttons: 0x1000 | 0x0001, lt: 200, rt: 10, lx: -12345, ly: 32767, rx: 7, ry: -32768 };
            p.update(&s);
            std::thread::sleep(Duration::from_millis(50));
            match read(slot) {
                Some(g) => {
                    let back = PadState { buttons: g.wButtons.0, lt: g.bLeftTrigger, rt: g.bRightTrigger, lx: g.sThumbLX, ly: g.sThumbLY, rx: g.sThumbRX, ry: g.sThumbRY };
                    let _ = writeln!(r, "estado lido de volta: {}", if back == s { "igual ao enviado" } else { "diferente" });
                    if back != s {
                        let _ = writeln!(r, "  enviado {s:?}\n  lido    {back:?}");
                    }
                }
                None => {
                    let _ = writeln!(r, "estado: a vaga {slot} não respondeu");
                }
            }
            // Tempo da troca de botão até o XInput ver.
            let mut times = Vec::new();
            for i in 0..200u32 {
                let want = if i % 2 == 0 { 0x2000 } else { 0x4000 };
                let s = PadState { buttons: want, ..Default::default() };
                let t = Instant::now();
                p.update(&s);
                while t.elapsed() < Duration::from_millis(100) {
                    if read(slot).is_some_and(|g| g.wButtons.0 == want) {
                        times.push(t.elapsed().as_secs_f64() * 1000.0);
                        break;
                    }
                }
            }
            times.sort_by(|a, b| a.total_cmp(b));
            if !times.is_empty() {
                let at = |q: f64| times[((times.len() - 1) as f64 * q).round() as usize];
                let _ = writeln!(r, "até o XInput ver (200 trocas, {} vistas): mediana {:.3} ms, p95 {:.3} ms, máx {:.3} ms", times.len(), at(0.5), at(0.95), at(1.0));
            }
            // Vibração: pedida pelo XInput como um jogo faria, volta pela ponte.
            let v = XINPUT_VIBRATION { wLeftMotorSpeed: 200 * 257, wRightMotorSpeed: 30 * 257 };
            unsafe {
                XInputSetState(slot as u32, &v);
            }
            let t = Instant::now();
            while p.rumble() == (0, 0) && t.elapsed() < Duration::from_millis(500) {
                std::thread::sleep(Duration::from_millis(1));
            }
            let _ = writeln!(r, "vibração pedida (200, 30), chegou {:?} em {:.1} ms", p.rumble(), t.elapsed().as_secs_f64() * 1000.0);
            let off = XINPUT_VIBRATION::default();
            unsafe {
                XInputSetState(slot as u32, &off);
            }
        }
    }
    drop(pads);
    std::thread::sleep(Duration::from_millis(300));
    let _ = writeln!(r, "vagas ocupadas depois de soltar: {:04b}", xinput_mask());
    report(&r);
}

#[cfg(all(test, feature = "stream"))]
mod tests {
    use super::*;

    #[test]
    fn rumble_packing() {
        assert_eq!(unpack(pack(200, 7)), (200, 7));
        assert_eq!(unpack(pack(0, 255)), (0, 255));
    }
}
