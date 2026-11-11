//! Opções do compartilhamento, escolhidas na página Compartilhar tela.
//!
//! - As do host ficam em `opcoes.txt` na pasta do compartilhamento, que o serviço também lê.
//! - As de quem assiste ficam em `cliente.txt` na pasta deste usuário.
//!
//! As variáveis de ambiente de teste (`PRISMA_CODEC`, `PRISMA_MBPS` e as outras) ainda valem por cima.

use super::*;
use crate::config::{MOD_ALT, MOD_CTRL, MOD_SHIFT};

/// Um atalho: modificadores (`MOD_*` do config) e a tecla virtual.
pub type Keys = (u32, u32);

const VK_END: u32 = 0x23;
const VK_SCROLL: u32 = 0x91;

/// As três posições das telas; cada uma com `eu`, um id em hexadecimal ou nada. Sem `eu`,
/// este PC fica no meio (ou na primeira livre).
fn screens_parse(v: &str) -> [String; 3] {
    let mut out: [String; 3] = Default::default();
    for (k, item) in v.split(',').take(3).enumerate() {
        let t = item.trim().to_ascii_lowercase();
        if (t == "eu" || (t.len() >= 6 && t.chars().all(|c| c.is_ascii_hexdigit()))) && !out.contains(&t) {
            out[k] = t;
        }
    }
    if !out.iter().any(|o| o == "eu") {
        if let Some(k) = [1, 0, 2].into_iter().find(|&k| out[k].is_empty()) {
            out[k] = "eu".into();
        }
    }
    out
}

pub fn keys_text(k: Keys) -> String {
    format!("{},{}", k.0, k.1)
}

fn keys_parse(v: &str, def: Keys) -> Keys {
    let mut it = v.split(',').map(|x| x.trim().parse::<u32>());
    match (it.next(), it.next()) {
        (Some(Ok(m)), Some(Ok(k))) if k != 0 => (m, k),
        _ => def,
    }
}

/// Os modificadores apertados agora, no formato dos atalhos.
pub fn mods_now(ctrl: bool, alt: bool, shift: bool) -> u32 {
    (ctrl as u32 * MOD_CTRL) | (alt as u32 * MOD_ALT) | (shift as u32 * MOD_SHIFT)
}

/// Codecs na ordem da lista da página.
pub const CODECS: [&str; 4] = ["auto", "av1", "hevc", "h264"];

#[derive(Clone, PartialEq, Debug)]
pub struct Opts {
    /// HDR do host numa tela sem HDR: falso converte, verdadeiro desliga o HDR do Windows enquanto alguém assiste.
    pub hdr_off: bool,
    /// Quantas pessoas assistem juntas.
    pub guests: usize,
    /// Ponte dos controles: auto, vigem ou viiper.
    pub pads: String,
    /// Permissões do convite que a página cria.
    pub invite: u8,
    /// Nome que os outros veem; vazio usa o nome do PC no Windows.
    pub name: String,
    /// Altura máxima do vídeo; 0 segue a tela de quem assiste.
    pub res: u32,
    /// Quadros por segundo; 0 é o menor entre os Hz dos dois lados.
    pub fps: u32,
    /// Banda em Mbit/s; 0 calcula pelo tamanho e pelos quadros.
    pub mbps: u32,
    /// auto, av1, hevc ou h264.
    pub codec: String,
    /// 0 menor atraso, 1 equilíbrio, 2 qualidade.
    pub priority: u8,
    /// HDR10 (10 bits) quando as duas telas estão em HDR.
    pub ten_bit: bool,
    /// 4:4:4 quando quem assiste pede.
    pub allow444: bool,
    /// 0 é o monitor principal; 1, 2... a ordem do Windows.
    pub monitor: u32,
    /// Repete o último quadro para manter o ritmo, mesmo com a tela parada.
    pub constant: bool,
    /// Programa cujo som vai sozinho (nome do exe); vazio manda o som do PC.
    pub sound_app: String,
    /// Tira do som o primeiro programa de chamada aberto (Discord, Teams, Zoom...).
    pub no_calls: bool,
    /// Um visitante por vez no mouse e no teclado.
    pub one_at_a_time: bool,
    /// Mexeu no mouse ou no teclado daqui: os visitantes param por 1 segundo.
    pub host_first: bool,
    /// Recusa conexões enquanto este PC assiste a outro.
    pub no_chain: bool,
    /// Porta UDP fixa; 0 deixa o Windows escolher.
    pub port: u16,
    /// Pede ao roteador para abrir a porta (UPnP, NAT-PMP, PCP).
    pub upnp: bool,
    /// Desconecta todo mundo.
    pub key_panic: Keys,
    /// Placa de vídeo que codifica (nome do Windows); vazio usa a do monitor transmitido.
    pub gpu: String,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            hdr_off: false,
            guests: 3,
            pads: "auto".into(),
            invite: SOUND,
            name: String::new(),
            res: 0,
            fps: 0,
            mbps: 0,
            codec: "auto".into(),
            priority: 1,
            ten_bit: true,
            allow444: true,
            monitor: 0,
            constant: false,
            sound_app: String::new(),
            no_calls: true,
            one_at_a_time: true,
            host_first: true,
            no_chain: true,
            port: 0,
            upnp: true,
            key_panic: (MOD_CTRL | MOD_ALT | MOD_SHIFT, b'Q' as u32),
            gpu: String::new(),
        }
    }
}

fn clean(v: &str, max: usize) -> String {
    v.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_string()
}

impl Opts {
    pub fn path() -> std::path::PathBuf {
        dir().join("opcoes.txt")
    }

    pub fn parse(text: &str) -> Self {
        let mut o = Opts::default();
        let flag = |v: &str| v == "1";
        for line in text.lines() {
            let Some((k, v)) = line.trim().split_once('=') else { continue };
            match k {
                "hdr_desligar" => o.hdr_off = flag(v),
                "pessoas" => o.guests = v.parse().unwrap_or(3usize).clamp(1, 8),
                "controles" if matches!(v, "auto" | "vigem" | "viiper") => o.pads = v.into(),
                "convite" => o.invite = parse_perms(v).unwrap_or(SOUND),
                "nome" => o.name = clean(v, 40),
                "resolucao" => o.res = v.parse().unwrap_or(0),
                "quadros" => o.fps = v.parse::<u32>().unwrap_or(0).min(480),
                "banda" => o.mbps = v.parse::<u32>().unwrap_or(0).min(500),
                "codec" if CODECS.contains(&v) => o.codec = v.into(),
                "prioridade" => o.priority = v.parse::<u8>().unwrap_or(1).min(2),
                "dez_bits" => o.ten_bit = flag(v),
                "permitir_444" => o.allow444 = flag(v),
                "monitor" => o.monitor = v.parse().unwrap_or(0),
                "ritmo_constante" => o.constant = flag(v),
                "som_programa" => o.sound_app = clean(v, 80),
                "sem_chamadas" => o.no_calls = flag(v),
                "um_por_vez" => o.one_at_a_time = flag(v),
                "host_primeiro" => o.host_first = flag(v),
                "sem_cadeia" => o.no_chain = flag(v),
                "porta" => o.port = v.parse().unwrap_or(0),
                "upnp" => o.upnp = flag(v),
                "atalho_emergencia" => o.key_panic = keys_parse(v, o.key_panic),
                "gpu" => o.gpu = clean(v, 128),
                _ => {}
            }
        }
        o
    }

    #[cfg_attr(not(feature = "stream"), allow(dead_code))]
    pub fn text(&self) -> String {
        let b = |x: bool| x as u8;
        format!(
            "hdr_desligar={}\npessoas={}\ncontroles={}\nconvite={}\nnome={}\nresolucao={}\nquadros={}\nbanda={}\ncodec={}\nprioridade={}\n\
dez_bits={}\npermitir_444={}\nmonitor={}\nritmo_constante={}\nsom_programa={}\nsem_chamadas={}\num_por_vez={}\nhost_primeiro={}\n\
sem_cadeia={}\nporta={}\nupnp={}\natalho_emergencia={}\ngpu={}\n",
            b(self.hdr_off),
            self.guests,
            self.pads,
            perms_key(self.invite),
            self.name,
            self.res,
            self.fps,
            self.mbps,
            self.codec,
            self.priority,
            b(self.ten_bit),
            b(self.allow444),
            self.monitor,
            b(self.constant),
            self.sound_app,
            b(self.no_calls),
            b(self.one_at_a_time),
            b(self.host_first),
            b(self.no_chain),
            self.port,
            b(self.upnp),
            keys_text(self.key_panic),
            self.gpu,
        )
    }

    pub fn load() -> Self {
        Self::parse(&std::fs::read_to_string(Self::path()).unwrap_or_default())
    }

    /// Mudou algo que o vídeo já aberto usa: a transmissão recomeça para valer. O resto (ritmo,
    /// som, mouse e teclado, pessoas) muda sem recomeçar.
    pub fn video_differs(&self, o: &Opts) -> bool {
        (self.res, self.fps, self.mbps, &self.codec, self.priority, self.ten_bit, self.allow444, self.monitor, &self.gpu)
            != (o.res, o.fps, o.mbps, &o.codec, o.priority, o.ten_bit, o.allow444, o.monitor, &o.gpu)
    }

    #[cfg_attr(not(feature = "stream"), allow(dead_code))]
    pub fn save(&self) -> std::io::Result<()> {
        let p = Self::path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(p, self.text())
    }
}

/// Opções de quem assiste, em `cliente.txt`.
#[derive(Clone, PartialEq, Debug)]
pub struct ClientOpts {
    /// Abre em tela cheia sem borda.
    pub fullscreen: bool,
    /// Espera a atualização da tela para mostrar (sem rasgos, um pouco mais de atraso).
    pub vsync: bool,
    /// Bit 1 prende o mouse na janela em tela cheia; bit 2 manda Alt+Tab e a tecla Windows ao host.
    pub immersive: u8,
    /// 0 desligado, 1 simples, 2 detalhado.
    pub meter: u8,
    /// Avisa quando a conexão piora.
    pub warn: bool,
    /// Não toca o som do host.
    pub mute: bool,
    /// Pede 4:4:4 (texto nítido), se a placa daqui decodifica.
    pub ask444: bool,
    /// O controle continua valendo com a janela em segundo plano.
    pub pad_background: bool,
    /// O teclado vira controle (WASD, setas, J, K, U, I...).
    pub pad_keys: bool,
    /// Quadros por segundo pedidos ao host; 0 é o automático (os Hz desta tela).
    pub fps: u32,
    /// Placa de vídeo que decodifica e mostra (nome do Windows); vazio usa a do Windows.
    pub gpu: String,
    /// Mouse e teclado entre PCs: quem fica na esquerda, no meio e na direita. `eu` é este PC;
    /// os outros, pelo id; vazio é posição livre.
    pub screens: [String; 3],
    pub key_menu: Keys,
    pub key_full: Keys,
    pub key_cad: Keys,
    pub key_disc: Keys,
    pub key_imm: Keys,
    pub key_meter: Keys,
    pub key_sound: Keys,
    /// Trava o mouse na tela em que ele está (Mouse e teclado).
    pub key_lock: Keys,
}

/// Quadros por segundo que dá para pedir pelo menu e pela aba Cliente (0: automático).
pub const CLIENT_FPS: [u32; 6] = [0, 30, 60, 90, 120, 144];

impl Default for ClientOpts {
    fn default() -> Self {
        ClientOpts {
            fullscreen: true,
            vsync: false,
            immersive: 3,
            meter: 0,
            warn: true,
            mute: false,
            ask444: false,
            pad_background: false,
            pad_keys: false,
            key_menu: (MOD_CTRL | MOD_ALT | MOD_SHIFT, b'P' as u32),
            key_full: (MOD_CTRL | MOD_ALT | MOD_SHIFT, b'F' as u32),
            fps: 0,
            key_cad: (MOD_CTRL | MOD_ALT, VK_END),
            key_disc: (MOD_CTRL | MOD_ALT | MOD_SHIFT, b'D' as u32),
            key_imm: (MOD_CTRL | MOD_ALT | MOD_SHIFT, b'I' as u32),
            key_meter: (MOD_CTRL | MOD_ALT | MOD_SHIFT, b'M' as u32),
            key_sound: (MOD_CTRL | MOD_ALT | MOD_SHIFT, b'S' as u32),
            gpu: String::new(),
            screens: [String::new(), "eu".into(), String::new()],
            key_lock: (0, VK_SCROLL),
        }
    }
}

impl ClientOpts {
    pub fn path() -> std::path::PathBuf {
        user_dir().join("cliente.txt")
    }

    pub fn parse(text: &str) -> Self {
        let mut o = ClientOpts::default();
        let flag = |v: &str| v == "1";
        for line in text.lines() {
            let Some((k, v)) = line.trim().split_once('=') else { continue };
            match k {
                "tela_cheia" => o.fullscreen = flag(v),
                "vsync" => o.vsync = flag(v),
                "imersivo" => o.immersive = v.parse::<u8>().unwrap_or(3).min(3),
                "medidor" => o.meter = v.parse::<u8>().unwrap_or(0).min(2),
                "aviso" => o.warn = flag(v),
                "mudo" => o.mute = flag(v),
                "pedir_444" => o.ask444 = flag(v),
                "controle_fundo" => o.pad_background = flag(v),
                "teclado_controle" => o.pad_keys = flag(v),
                "atalho_menu" => o.key_menu = keys_parse(v, o.key_menu),
                "atalho_tela_cheia" => o.key_full = keys_parse(v, o.key_full),
                "atalho_ctrl_alt_del" => o.key_cad = keys_parse(v, o.key_cad),
                "atalho_desconectar" => o.key_disc = keys_parse(v, o.key_disc),
                "atalho_imersivo" => o.key_imm = keys_parse(v, o.key_imm),
                "atalho_medidor" => o.key_meter = keys_parse(v, o.key_meter),
                "atalho_som" => o.key_sound = keys_parse(v, o.key_sound),
                "quadros" => o.fps = v.parse::<u32>().ok().filter(|f| CLIENT_FPS.contains(f)).unwrap_or(0),
                "gpu" => o.gpu = clean(v, 128),
                "telas" => o.screens = screens_parse(v),
                "atalho_travar_mouse" => o.key_lock = keys_parse(v, o.key_lock),
                _ => {}
            }
        }
        o
    }

    #[cfg_attr(not(feature = "stream"), allow(dead_code))]
    pub fn text(&self) -> String {
        let b = |x: bool| x as u8;
        format!(
            "tela_cheia={}\nvsync={}\nimersivo={}\nmedidor={}\naviso={}\nmudo={}\npedir_444={}\ncontrole_fundo={}\nteclado_controle={}\n\
quadros={}\natalho_menu={}\natalho_tela_cheia={}\natalho_ctrl_alt_del={}\natalho_desconectar={}\natalho_imersivo={}\natalho_medidor={}\natalho_som={}\ngpu={}\ntelas={}\natalho_travar_mouse={}\n",
            b(self.fullscreen),
            b(self.vsync),
            self.immersive,
            self.meter,
            b(self.warn),
            b(self.mute),
            b(self.ask444),
            b(self.pad_background),
            b(self.pad_keys),
            self.fps,
            keys_text(self.key_menu),
            keys_text(self.key_full),
            keys_text(self.key_cad),
            keys_text(self.key_disc),
            keys_text(self.key_imm),
            keys_text(self.key_meter),
            keys_text(self.key_sound),
            self.gpu,
            self.screens.join(","),
            keys_text(self.key_lock),
        )
    }

    pub fn load() -> Self {
        Self::parse(&std::fs::read_to_string(Self::path()).unwrap_or_default())
    }

    #[cfg_attr(not(feature = "stream"), allow(dead_code))]
    pub fn save(&self) -> std::io::Result<()> {
        let p = Self::path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(p, self.text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opts_round_trip() {
        let o = Opts {
            hdr_off: true,
            guests: 2,
            pads: "vigem".into(),
            invite: SOUND | COPY,
            name: "Sala".into(),
            res: 1080,
            fps: 60,
            mbps: 40,
            codec: "hevc".into(),
            priority: 0,
            ten_bit: false,
            allow444: false,
            monitor: 2,
            constant: true,
            sound_app: "game.exe".into(),
            no_calls: false,
            one_at_a_time: false,
            host_first: false,
            no_chain: false,
            port: 47610,
            upnp: false,
            key_panic: (MOD_CTRL, b'K' as u32),
            gpu: "NVIDIA GeForce RTX 4060 Laptop GPU".into(),
        };
        assert_eq!(Opts::parse(&o.text()), o);
        assert_eq!(Opts::parse("pessoas=99\ncontroles=outro\ncodec=mp3\nnome=a\tb"), Opts { guests: 8, name: "ab".into(), ..Opts::default() });
        // Arquivo da 0.11.0: o resto fica no padrão.
        assert_eq!(Opts::parse("hdr_desligar=1\npessoas=2\ncontroles=auto\nconvite=total\n"), Opts { hdr_off: true, guests: 2, invite: TOTAL, ..Opts::default() });
        let c = ClientOpts { fullscreen: false, vsync: true, immersive: 1, meter: 2, warn: false, mute: true, ask444: true, pad_background: true, pad_keys: true, fps: 60, gpu: "AMD Radeon(TM) Graphics".into(), key_menu: (MOD_ALT, 0x70), key_sound: (MOD_CTRL, b'M' as u32), ..ClientOpts::default() };
        assert_eq!(ClientOpts::parse(&c.text()), c);
        let c = ClientOpts { screens: ["eu".into(), "".into(), "31ded57656ab".into()], key_lock: (MOD_CTRL, b'L' as u32), ..ClientOpts::default() };
        assert_eq!(ClientOpts::parse(&c.text()), c);
        // Lixo e repetidos saem, e este PC sempre tem lugar.
        assert_eq!(ClientOpts::parse("telas=abcdef12,abcdef12,x;y").screens, ["abcdef12".to_string(), "eu".into(), "".into()]);
        assert_eq!(ClientOpts::parse("atalho_menu=2,0\nmedidor=9\nquadros=75"), ClientOpts { meter: 2, ..ClientOpts::default() });
    }

    #[test]
    fn what_restarts_the_video() {
        let o = Opts::default();
        // Ritmo, som, mouse e teclado mudam sem recomeçar.
        assert!(!o.video_differs(&Opts { constant: true, no_calls: false, sound_app: "game.exe".into(), one_at_a_time: false, host_first: false, guests: 2, ..Opts::default() }));
        for v in [Opts { fps: 30, ..Opts::default() }, Opts { mbps: 80, ..Opts::default() }, Opts { priority: 2, ..Opts::default() }, Opts { gpu: "Outra".into(), ..Opts::default() }, Opts { monitor: 1, ..Opts::default() }] {
            assert!(o.video_differs(&v), "{v:?}");
        }
    }
}
