use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Hello {
        protocol: u32,
        app_version: String,
    },
    GameStarted {
        pid: u32,
        exe: String,
        name: String,
    },
    GameStopped {
        pid: u32,
    },
    OpenPage {
        page: Page,
    },
    /// `minutes` só com a capacidade `share_invite.minutes`; sem ele, a validade da página.
    ShareInvite {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        minutes: Option<u32>,
    },
    SaveReplay,
    CaptureDirs,
    /// Mostra ou esconde o medidor (`meter.view`).
    MeterView {
        view: MeterView,
    },
    /// Planos de energia do Windows e a escolha do Prisma (`power.plan`).
    PowerPlans,
    /// Plano fixo; vazio volta à troca automática (`power.plan`).
    SetPowerPlan {
        id: String,
    },
    /// Alterna a janela do processo entre sem bordas e normal (`fullscreen`). `pid` 0: a janela em
    /// primeiro plano, como o atalho.
    Fullscreen {
        pid: u32,
    },
    /// Volume do processo (`sound.volume`).
    Volume {
        pid: u32,
        change: VolumeChange,
    },
    /// HDR do Windows na tela principal; `None` alterna (`hdr`).
    Hdr {
        #[serde(default)]
        on: Option<bool>,
    },
    /// Opções do driver que o Prisma conhece (`nvidia`).
    NvidiaCatalog,
    /// Valores no perfil do jogo; `exe` vazio é o perfil global (`nvidia`).
    NvidiaGet {
        exe: String,
    },
    /// `value` `None` volta ao padrão do driver (`nvidia`).
    NvidiaSet {
        exe: String,
        id: u32,
        #[serde(default)]
        value: Option<u32>,
    },
    /// Foto da janela do processo, salva direto na pasta de capturas (`capture.window_shot`).
    WindowShot {
        pid: u32,
    },
    /// Liga ou para a gravação da janela do processo (`capture.record_window`).
    RecordWindow {
        pid: u32,
    },
    /// Liga ou para a gravação da tela inteira, a do monitor em foco (`capture.record_screen`).
    /// Responde `Toggled`.
    RecordScreen,
    /// Controles ligados agora (`controllers`).
    Controllers,
    /// Perfis do controle como teclado e mouse e qual está em uso (`controllers.mapping`).
    PadProfiles,
    /// Troca para o perfil `name` até o programa em foco mudar (`controllers.mapping`).
    SetPadProfile {
        name: String,
    },
    /// Para ou volta o mapeamento, como a combinação de parada no controle; `None` alterna
    /// (`controllers.mapping`).
    PadStop {
        #[serde(default)]
        on: Option<bool>,
    },
    /// Liga ou desliga o controle como teclado e mouse, como a caixa da aba Controles; `None`
    /// alterna (`controllers.mapping`).
    PadMap {
        #[serde(default)]
        on: Option<bool>,
    },
    /// Liga ou desliga a ponte do controle Xbox (Fase 8), como o botão Virar controle Xbox; `None`
    /// alterna (`controllers.bridge`). Sem a ponte instalada, `Failed` com `not_supported`: a
    /// instalação só acontece na tela, com o consentimento.
    PadBridge {
        #[serde(default)]
        on: Option<bool>,
    },
    /// Abre ou fecha o teclado do Prisma para controle; `None` alterna (`controllers.keyboard`).
    Keyboard {
        #[serde(default)]
        on: Option<bool>,
    },
    /// Alguém pede para entrar no compartilhamento de tela (`share.ask`): o Prisma pergunta e
    /// responde `ShareAnswer` quando ela escolher, sem prazo. `perms` são os bits das permissões;
    /// `code` vem só no pareamento, para ela conferir com o outro PC.
    ShareAsk {
        name: String,
        perms: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
    /// Estado das opções da barra do launcher para o jogo (`options`). `exe` é o caminho ou o nome
    /// do .exe (o que é por jogo); `pid`, quando o jogo está aberto, dá o volume dele.
    Options {
        #[serde(default)]
        exe: String,
        #[serde(default)]
        pid: u32,
    },
    /// Muda uma opção do jogo; responde `Done` ou `Failed`. Cada opção tem a sua capacidade (veja
    /// `OptionValue`).
    SetOption {
        #[serde(default)]
        exe: String,
        #[serde(default)]
        pid: u32,
        option: OptionValue,
    },
    /// Arma um convite do Compartilhar tela só da janela do jogo `exe` (o nome do .exe): quando o
    /// jogo abrir, o Prisma cria o convite. Responde `Done` já; o código aparece em
    /// `Reply::Options::invite` (`share.invite_game`).
    ShareInviteForGame {
        exe: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        minutes: Option<u32>,
        /// Reservado: amigo que vai receber o link (chat). Hoje é ignorado.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
    },
    /// Desarma o convite do jogo `exe`; vazio desarma qualquer um. Responde `Done`. Um convite já
    /// criado continua valendo (a página Compartilhar tela o revoga).
    ShareInviteCancel {
        #[serde(default)]
        exe: String,
    },
    #[serde(other)]
    Unknown,
}

/// Limite de FPS do jogo, como a tela Medidor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FpsLimit {
    Off,
    /// O Prisma mede o jogo e escolhe.
    Auto,
    /// FPS de 20 a 1023.
    Manual(u32),
}

/// O que o Compartilhar tela transmite, ou o que o replay grava.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareMode {
    #[default]
    Screen,
    /// Só a janela do jogo (a do `pid`).
    Game,
}

/// Resolução do replay e das gravações, como a tela Captura.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplayResolution {
    #[serde(rename = "native")]
    Native,
    #[serde(rename = "1080p")]
    P1080,
    #[serde(rename = "1440p")]
    P1440,
    #[serde(rename = "2160p")]
    P2160,
}

/// Replay da tela Captura: guarda os últimos `secs` segundos (15, 30, 60 ou 120) em modo jogo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayOptions {
    pub on: bool,
    pub secs: u32,
    /// A tela inteira ou só a janela do jogo em primeiro plano.
    #[serde(default)]
    pub target: ShareMode,
    pub resolution: ReplayResolution,
    /// HDR10; falso é SDR.
    pub hdr: bool,
}

/// Em que pé está o convite armado com `ShareInviteForGame`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InviteState {
    /// Esperando o jogo abrir.
    Armed,
    /// Jogo aberto, criando o convite.
    Creating,
    /// `code` e `link` prontos.
    Ready,
    /// Não deu; o motivo vem em `reason`.
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameInvite {
    pub exe: String,
    pub state: InviteState,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub link: String,
    #[serde(default)]
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    /// O padrão do Windows agora.
    pub default: bool,
}

/// Uma opção de `SetOption`, com a capacidade que a anuncia.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionValue {
    /// `meter.fps_limit`.
    FpsLimit(FpsLimit),
    /// Auto HDR do Windows para o jogo; `None` deixa o Windows decidir (`hdr.auto`). Vale na próxima
    /// abertura.
    AutoHdr(Option<bool>),
    /// Aparelho de saída do jogo; vazio é o padrão do Windows (`sound.devices`).
    AudioOut(String),
    /// Aparelho de entrada do jogo; vazio é o padrão do Windows (`sound.devices`).
    AudioIn(String),
    /// `share.mode`. `Game` precisa do `pid` com a janela aberta.
    ShareMode(ShareMode),
    /// `replay.options`.
    Replay(ReplayOptions),
    /// Sem bordas sozinho sempre que o jogo abrir (`fullscreenizer`).
    Fullscreenizer(bool),
    /// Otimizar ao abrir o jogo; falso pula este jogo (`optimize`). A regra geral da tela Energia
    /// ("Não perguntar") vale por cima.
    Optimize(bool),
    /// Medidor que abre sozinho quando o jogo abre; `Off` não abre (`meter.on_launch`).
    Meter(MeterView),
    /// Liga o HDR do Windows quando o jogo abre e devolve como estava quando fecha (`hdr.on_launch`).
    HdrOnLaunch(bool),
    /// Volume do jogo, de 0 a 100, posto quando ele abre (a lista do Som, programa fixo); `None`
    /// tira o volume guardado (`sound.game_volume`).
    Volume(Option<u32>),
    /// Perfil de equalizador que roda enquanto o jogo está aberto, um de `eq_profiles`; vazio é o
    /// padrão do aparelho (`sound.eq`).
    EqProfile(String),
    /// Opção que este lado não conhece, com o que veio nela.
    #[serde(untagged)]
    Unknown(serde_json::Value),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeterView {
    Off,
    Compact,
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeChange {
    /// Soma pontos de 0 a 100 (negativo abaixa).
    Step(i32),
    /// Volume de 0 a 100.
    Set(u32),
    ToggleMute,
}

/// Um controle ligado, como a tela Controles mostra.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Controller {
    pub name: String,
    /// `usb` ou `bluetooth`.
    pub link: String,
    /// Bateria em %, quando o controle informa. Os que só dão níveis (Xbox) vêm em 0, 33, 67 ou 100.
    pub battery: Option<u32>,
    pub charging: bool,
    /// No cabo, sem bateria a informar.
    pub wired: bool,
    /// Relatórios por segundo; `None` enquanto não foi medida. No Xbox sai do contador do XInput
    /// com o controle em movimento, e então não há `p99_us`.
    pub rate_hz: Option<u32>,
    /// p99 do intervalo entre relatórios, em microssegundos.
    pub p99_us: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerPlan {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NvidiaSetting {
    pub id: u32,
    pub name: String,
    pub group: String,
    /// Só vale no perfil global.
    pub global_only: bool,
    pub kind: NvidiaKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NvidiaKind {
    /// Valor no driver e nome na tela.
    List(Vec<(u32, String)>),
    Number {
        min: u32,
        max: u32,
        step: u32,
        unit: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NvidiaValue {
    pub id: u32,
    /// Gravado neste perfil; `None`: o padrão.
    pub value: Option<u32>,
    /// O que vale agora, venha de onde vier (perfil, global ou driver), quando o driver diz.
    pub now: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Page {
    Meter,
    Share,
    GameProfile,
    Energy,
    Fullscreen,
    Sound,
    Hdr,
    Nvidia,
    Capture,
    /// Tela Controles (`page.controllers`).
    Controllers,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Capabilities {
        protocol: u32,
        app_version: String,
        caps: Vec<String>,
    },
    ShareInviteCode {
        code: String,
        link: String,
    },
    ShareInviteFailed {
        reason: String,
    },
    CaptureDirs {
        shots: String,
        replays: String,
    },
    /// Pedido atendido, sem mais nada a dizer.
    Done,
    /// Pedido recusado. O cliente decide pelo `code` (um de `crate::fail`); o `reason`, no idioma
    /// do Prisma, é só para mostrar.
    Failed {
        #[serde(default)]
        code: String,
        reason: String,
    },
    PowerPlans {
        plans: Vec<PowerPlan>,
        /// Plano ativo agora.
        active: String,
        /// Plano fixo; vazio com a troca automática.
        fixed: String,
        auto: bool,
    },
    /// Estado depois de alternar (Fullscreenizer, HDR, gravação).
    Toggled {
        on: bool,
    },
    Volume {
        level: u32,
        muted: bool,
    },
    NvidiaCatalog {
        settings: Vec<NvidiaSetting>,
    },
    NvidiaValues {
        values: Vec<NvidiaValue>,
    },
    Shot {
        path: String,
    },
    Controllers {
        pads: Vec<Controller>,
    },
    PadProfiles {
        profiles: Vec<String>,
        /// Perfil em uso; vazio com o mapeamento desligado.
        active: String,
        /// O controle como teclado e mouse está ligado na tela Controles.
        mapping: bool,
        /// Parado pela combinação no controle ou pelo `PadStop`.
        stopped: bool,
    },
    /// Resposta ao `ShareAsk`. `trust`: no pareamento, confiar no aparelho e não perguntar mais.
    ShareAnswer {
        allow: bool,
        #[serde(default)]
        trust: bool,
    },
    /// Resposta ao `Options`.
    Options {
        /// Medidor que abre com o jogo (`OptionValue::Meter`).
        meter: MeterView,
        fps_limit: FpsLimit,
        /// HDR do Windows na tela principal; `None` sem tela com HDR.
        hdr: Option<bool>,
        /// Escolha do jogo; `None` deixa o Windows decidir.
        auto_hdr: Option<bool>,
        /// A chave geral do Auto HDR nas Configurações do Windows.
        auto_hdr_windows: bool,
        /// O HDR do Windows liga quando o jogo abre (a escolha do jogo ou a regra geral do Medidor).
        #[serde(default)]
        hdr_on_launch: bool,
        audio_out: Vec<AudioDevice>,
        audio_in: Vec<AudioDevice>,
        /// Aparelhos do jogo; vazio é o padrão do Windows.
        audio_out_id: String,
        audio_in_id: String,
        /// Volume guardado do jogo, de 0 a 100, posto quando ele abre; `None` fora da lista do Som.
        volume: Option<u32>,
        /// O jogo aberto (`pid`) está mudo agora.
        muted: bool,
        share_mode: ShareMode,
        replay: ReplayOptions,
        fullscreenizer: bool,
        optimize: bool,
        /// Sobe a cada mudança no Prisma, venha de onde vier: igual ao da última leitura, nada mudou.
        #[serde(default)]
        rev: u64,
        /// O convite armado com `ShareInviteForGame`, se houver.
        #[serde(default)]
        invite: Option<GameInvite>,
        /// Perfis de equalizador do aparelho de saída do jogo (o padrão do Windows agora, se o jogo
        /// usa o padrão). Vazio: nenhum criado no Prisma.
        #[serde(default)]
        eq_profiles: Vec<String>,
        /// Perfil escolhido para o jogo; vazio é o padrão do aparelho.
        #[serde(default)]
        eq_profile: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub protocol: u32,
    pub app_version: String,
    pub caps: Vec<String>,
}

impl Capabilities {
    pub fn has(&self, cap: &str) -> bool {
        self.caps.iter().any(|c| c == cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_shape_is_stable() {
        assert_eq!(
            serde_json::to_string(&Request::OpenPage {
                page: Page::GameProfile
            })
            .unwrap(),
            r#"{"type":"open_page","page":"game_profile"}"#
        );
        assert_eq!(
            serde_json::to_string(&Request::Hello {
                protocol: 1,
                app_version: "0.1.0".into()
            })
            .unwrap(),
            r#"{"type":"hello","protocol":1,"app_version":"0.1.0"}"#
        );
    }

    #[test]
    fn unknown_type_reads_as_unknown() {
        let r: Request = serde_json::from_str(r#"{"type":"future_thing","x":1}"#).unwrap();
        assert_eq!(r, Request::Unknown);
        let r: Reply = serde_json::from_str(r#"{"type":"future_thing","x":1}"#).unwrap();
        assert_eq!(r, Reply::Unknown);
    }

    #[test]
    fn pad_map_without_on_toggles() {
        assert_eq!(serde_json::from_str::<Request>(r#"{"type":"pad_map"}"#).unwrap(), Request::PadMap { on: None });
        assert_eq!(serde_json::from_str::<Request>(r#"{"type":"pad_map","on":false}"#).unwrap(), Request::PadMap { on: Some(false) });
        assert_eq!(serde_json::from_str::<Request>(r#"{"type":"keyboard"}"#).unwrap(), Request::Keyboard { on: None });
        assert_eq!(serde_json::from_str::<Request>(r#"{"type":"pad_bridge","on":true}"#).unwrap(), Request::PadBridge { on: Some(true) });
    }

    #[test]
    fn capture_dirs_round_trips() {
        let req = serde_json::to_string(&Request::CaptureDirs).unwrap();
        assert_eq!(req, r#"{"type":"capture_dirs"}"#);
        assert_eq!(
            serde_json::from_str::<Request>(&req).unwrap(),
            Request::CaptureDirs
        );
        let reply = Reply::CaptureDirs {
            shots: r"C:\V\Prisma".into(),
            replays: r"C:\V\Prisma".into(),
        };
        let json = serde_json::to_string(&reply).unwrap();
        assert_eq!(
            json,
            r#"{"type":"capture_dirs","shots":"C:\\V\\Prisma","replays":"C:\\V\\Prisma"}"#
        );
        assert_eq!(serde_json::from_str::<Reply>(&json).unwrap(), reply);
    }

    #[test]
    fn old_peer_reads_capture_dirs_as_unknown() {
        // Um par antigo não conhece `capture_dirs`: o `#[serde(other)]` o lê como Unknown.
        // Simulado com um tipo idêntico ao protocolo anterior.
        #[derive(Debug, PartialEq, Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum OldRequest {
            SaveReplay,
            #[serde(other)]
            Unknown,
        }
        let r: OldRequest = serde_json::from_str(r#"{"type":"capture_dirs"}"#).unwrap();
        assert_eq!(r, OldRequest::Unknown);
    }

    #[test]
    fn old_share_invite_still_parses() {
        let r: Request = serde_json::from_str(r#"{"type":"share_invite"}"#).unwrap();
        assert_eq!(r, Request::ShareInvite { minutes: None });
        assert_eq!(
            serde_json::to_string(&Request::ShareInvite { minutes: None }).unwrap(),
            r#"{"type":"share_invite"}"#
        );
        let r: Request = serde_json::from_str(r#"{"type":"share_invite","minutes":30}"#).unwrap();
        assert_eq!(r, Request::ShareInvite { minutes: Some(30) });
    }

    #[test]
    fn old_peer_ignores_invite_minutes() {
        // O Prisma antigo tinha `ShareInvite` sem campos: os minutos são ignorados.
        #[derive(Debug, PartialEq, Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum OldRequest {
            ShareInvite,
            #[serde(other)]
            Unknown,
        }
        let r: OldRequest =
            serde_json::from_str(r#"{"type":"share_invite","minutes":30}"#).unwrap();
        assert_eq!(r, OldRequest::ShareInvite);
    }

    #[test]
    fn new_requests_json_shape() {
        let cases: Vec<(Request, &str)> = vec![
            (Request::MeterView { view: MeterView::Compact }, r#"{"type":"meter_view","view":"compact"}"#),
            (Request::PowerPlans, r#"{"type":"power_plans"}"#),
            (Request::SetPowerPlan { id: String::new() }, r#"{"type":"set_power_plan","id":""}"#),
            (Request::Fullscreen { pid: 7 }, r#"{"type":"fullscreen","pid":7}"#),
            (Request::Volume { pid: 7, change: VolumeChange::Step(-5) }, r#"{"type":"volume","pid":7,"change":{"step":-5}}"#),
            (Request::Volume { pid: 7, change: VolumeChange::Set(40) }, r#"{"type":"volume","pid":7,"change":{"set":40}}"#),
            (Request::Volume { pid: 7, change: VolumeChange::ToggleMute }, r#"{"type":"volume","pid":7,"change":"toggle_mute"}"#),
            (Request::Hdr { on: None }, r#"{"type":"hdr","on":null}"#),
            (Request::Hdr { on: Some(true) }, r#"{"type":"hdr","on":true}"#),
            (Request::NvidiaCatalog, r#"{"type":"nvidia_catalog"}"#),
            (Request::NvidiaGet { exe: "g.exe".into() }, r#"{"type":"nvidia_get","exe":"g.exe"}"#),
            (Request::NvidiaSet { exe: "g.exe".into(), id: 5, value: None }, r#"{"type":"nvidia_set","exe":"g.exe","id":5,"value":null}"#),
            (Request::WindowShot { pid: 7 }, r#"{"type":"window_shot","pid":7}"#),
            (Request::RecordWindow { pid: 7 }, r#"{"type":"record_window","pid":7}"#),
            (Request::RecordScreen, r#"{"type":"record_screen"}"#),
            (Request::ShareAsk { name: "PC".into(), perms: 3, code: None }, r#"{"type":"share_ask","name":"PC","perms":3}"#),
            (Request::ShareAsk { name: "PC".into(), perms: 3, code: Some("12 34".into()) }, r#"{"type":"share_ask","name":"PC","perms":3,"code":"12 34"}"#),
        ];
        for (req, json) in cases {
            assert_eq!(serde_json::to_string(&req).unwrap(), json);
            assert_eq!(serde_json::from_str::<Request>(json).unwrap(), req);
        }
        // `on` e `value` podem faltar.
        assert_eq!(serde_json::from_str::<Request>(r#"{"type":"hdr"}"#).unwrap(), Request::Hdr { on: None });
        assert_eq!(
            serde_json::from_str::<Request>(r#"{"type":"nvidia_set","exe":"","id":5}"#).unwrap(),
            Request::NvidiaSet { exe: String::new(), id: 5, value: None }
        );
    }

    #[test]
    fn new_replies_round_trip() {
        let replies = vec![
            Reply::Done,
            Reply::Failed { code: "no_window".into(), reason: "x".into() },
            Reply::PowerPlans {
                plans: vec![PowerPlan { id: "a".into(), name: "Equilibrado".into() }],
                active: "a".into(),
                fixed: String::new(),
                auto: true,
            },
            Reply::Toggled { on: true },
            Reply::Volume { level: 40, muted: false },
            Reply::NvidiaCatalog {
                settings: vec![
                    NvidiaSetting { id: 1, name: "VSync".into(), group: "Sincronia".into(), global_only: false, kind: NvidiaKind::List(vec![(0, "Desligado".into())]) },
                    NvidiaSetting { id: 2, name: "FPS".into(), group: "Sincronia".into(), global_only: true, kind: NvidiaKind::Number { min: 10, max: 1000, step: 1, unit: " FPS".into() } },
                ],
            },
            Reply::NvidiaValues { values: vec![NvidiaValue { id: 1, value: None, now: Some(0) }, NvidiaValue { id: 2, value: Some(60), now: Some(60) }] },
            Reply::Shot { path: r"C:\V\a.png".into() },
            Reply::Controllers {
                pads: vec![Controller { name: "DualSense".into(), link: "usb".into(), battery: Some(15), charging: true, wired: false, rate_hz: Some(250), p99_us: Some(4100) }],
            },
            Reply::PadProfiles { profiles: vec!["Desktop".into(), "Base".into()], active: "Desktop".into(), mapping: true, stopped: false },
            Reply::ShareAnswer { allow: true, trust: false },
        ];
        for r in replies {
            let json = serde_json::to_string(&r).unwrap();
            assert_eq!(serde_json::from_str::<Reply>(&json).unwrap(), r, "{json}");
        }
        assert_eq!(serde_json::to_string(&Reply::Done).unwrap(), r#"{"type":"done"}"#);
        // Sem `trust`, a resposta não confia no aparelho.
        assert_eq!(serde_json::from_str::<Reply>(r#"{"type":"share_answer","allow":true}"#).unwrap(), Reply::ShareAnswer { allow: true, trust: false });
        assert_eq!(
            serde_json::to_string(&Reply::Failed { code: "busy".into(), reason: "r".into() }).unwrap(),
            r#"{"type":"failed","code":"busy","reason":"r"}"#
        );
    }

    #[test]
    fn options_requests_round_trip() {
        let replay = ReplayOptions { on: true, secs: 30, target: ShareMode::Game, resolution: ReplayResolution::P1440, hdr: false };
        let cases: Vec<(Request, &str)> = vec![
            (Request::Options { exe: r"C:\G\g.exe".into(), pid: 7 }, r#"{"type":"options","exe":"C:\\G\\g.exe","pid":7}"#),
            (Request::SetOption { exe: "g.exe".into(), pid: 0, option: OptionValue::FpsLimit(FpsLimit::Off) }, r#"{"type":"set_option","exe":"g.exe","pid":0,"option":{"fps_limit":"off"}}"#),
            (Request::SetOption { exe: "g.exe".into(), pid: 0, option: OptionValue::FpsLimit(FpsLimit::Auto) }, r#"{"type":"set_option","exe":"g.exe","pid":0,"option":{"fps_limit":"auto"}}"#),
            (Request::SetOption { exe: "g.exe".into(), pid: 0, option: OptionValue::FpsLimit(FpsLimit::Manual(120)) }, r#"{"type":"set_option","exe":"g.exe","pid":0,"option":{"fps_limit":{"manual":120}}}"#),
            (Request::SetOption { exe: "g.exe".into(), pid: 0, option: OptionValue::AutoHdr(None) }, r#"{"type":"set_option","exe":"g.exe","pid":0,"option":{"auto_hdr":null}}"#),
            (Request::SetOption { exe: "g.exe".into(), pid: 0, option: OptionValue::AutoHdr(Some(true)) }, r#"{"type":"set_option","exe":"g.exe","pid":0,"option":{"auto_hdr":true}}"#),
            (Request::SetOption { exe: "g.exe".into(), pid: 0, option: OptionValue::AudioOut("{0.0.0}.{a}".into()) }, r#"{"type":"set_option","exe":"g.exe","pid":0,"option":{"audio_out":"{0.0.0}.{a}"}}"#),
            (Request::SetOption { exe: "g.exe".into(), pid: 0, option: OptionValue::AudioIn(String::new()) }, r#"{"type":"set_option","exe":"g.exe","pid":0,"option":{"audio_in":""}}"#),
            (Request::SetOption { exe: "g.exe".into(), pid: 0, option: OptionValue::EqProfile("FPS".into()) }, r#"{"type":"set_option","exe":"g.exe","pid":0,"option":{"eq_profile":"FPS"}}"#),
            (Request::SetOption { exe: "g.exe".into(), pid: 7, option: OptionValue::ShareMode(ShareMode::Game) }, r#"{"type":"set_option","exe":"g.exe","pid":7,"option":{"share_mode":"game"}}"#),
            (Request::SetOption { exe: "g.exe".into(), pid: 0, option: OptionValue::Replay(replay) }, r#"{"type":"set_option","exe":"g.exe","pid":0,"option":{"replay":{"on":true,"secs":30,"target":"game","resolution":"1440p","hdr":false}}}"#),
            (Request::ShareInviteForGame { exe: "g.exe".into(), minutes: None, to: None }, r#"{"type":"share_invite_for_game","exe":"g.exe"}"#),
            (Request::ShareInviteForGame { exe: "g.exe".into(), minutes: Some(30), to: Some("ana".into()) }, r#"{"type":"share_invite_for_game","exe":"g.exe","minutes":30,"to":"ana"}"#),
            (Request::ShareInviteCancel { exe: "g.exe".into() }, r#"{"type":"share_invite_cancel","exe":"g.exe"}"#),
            (Request::SetOption { exe: "g.exe".into(), pid: 0, option: OptionValue::Fullscreenizer(true) }, r#"{"type":"set_option","exe":"g.exe","pid":0,"option":{"fullscreenizer":true}}"#),
            (Request::SetOption { exe: "g.exe".into(), pid: 0, option: OptionValue::Optimize(false) }, r#"{"type":"set_option","exe":"g.exe","pid":0,"option":{"optimize":false}}"#),
            (Request::SetOption { exe: "g.exe".into(), pid: 0, option: OptionValue::Meter(MeterView::Full) }, r#"{"type":"set_option","exe":"g.exe","pid":0,"option":{"meter":"full"}}"#),
            (Request::SetOption { exe: "g.exe".into(), pid: 0, option: OptionValue::HdrOnLaunch(true) }, r#"{"type":"set_option","exe":"g.exe","pid":0,"option":{"hdr_on_launch":true}}"#),
            (Request::SetOption { exe: "g.exe".into(), pid: 0, option: OptionValue::Volume(Some(35)) }, r#"{"type":"set_option","exe":"g.exe","pid":0,"option":{"volume":35}}"#),
            (Request::SetOption { exe: "g.exe".into(), pid: 0, option: OptionValue::Volume(None) }, r#"{"type":"set_option","exe":"g.exe","pid":0,"option":{"volume":null}}"#),
        ];
        for (req, json) in cases {
            assert_eq!(serde_json::to_string(&req).unwrap(), json);
            assert_eq!(serde_json::from_str::<Request>(json).unwrap(), req);
        }
        // `exe` e `pid` podem faltar; uma opção que este lado não conhece vira Unknown.
        assert_eq!(serde_json::from_str::<Request>(r#"{"type":"options"}"#).unwrap(), Request::Options { exe: String::new(), pid: 0 });
        assert_eq!(
            serde_json::from_str::<Request>(r#"{"type":"set_option","option":{"future":1}}"#).unwrap(),
            Request::SetOption { exe: String::new(), pid: 0, option: OptionValue::Unknown(serde_json::json!({"future": 1})) }
        );
        // Sem `target`, o replay antigo grava a tela inteira.
        assert_eq!(
            serde_json::from_str::<ReplayOptions>(r#"{"on":true,"secs":30,"resolution":"native","hdr":false}"#).unwrap().target,
            ShareMode::Screen
        );
        for (res, name) in [(ReplayResolution::Native, "native"), (ReplayResolution::P1080, "1080p"), (ReplayResolution::P2160, "2160p")] {
            assert_eq!(serde_json::to_string(&res).unwrap(), format!("\"{name}\""));
        }
    }

    #[test]
    fn options_reply_round_trips() {
        let dev = |id: &str, default| AudioDevice { id: id.into(), name: format!("Alto-falante {id}"), default };
        let replies = vec![
            Reply::Options {
                meter: MeterView::Compact,
                fps_limit: FpsLimit::Manual(90),
                hdr: Some(true),
                auto_hdr: None,
                auto_hdr_windows: true,
                hdr_on_launch: true,
                audio_out: vec![dev("a", true), dev("b", false)],
                audio_in: vec![],
                audio_out_id: "b".into(),
                audio_in_id: String::new(),
                volume: Some(40),
                muted: false,
                share_mode: ShareMode::Screen,
                replay: ReplayOptions { on: false, secs: 120, target: ShareMode::Screen, resolution: ReplayResolution::Native, hdr: true },
                fullscreenizer: true,
                optimize: false,
                rev: 0,
                invite: None,
                eq_profiles: vec!["Jogos".into(), "FPS".into()],
                eq_profile: "FPS".into(),
            },
            Reply::Options {
                meter: MeterView::Off,
                fps_limit: FpsLimit::Auto,
                hdr: None,
                auto_hdr: Some(false),
                auto_hdr_windows: false,
                hdr_on_launch: false,
                audio_out: vec![],
                audio_in: vec![dev("m", true)],
                audio_out_id: String::new(),
                audio_in_id: "m".into(),
                volume: None,
                muted: true,
                share_mode: ShareMode::Game,
                replay: ReplayOptions { on: true, secs: 15, target: ShareMode::Game, resolution: ReplayResolution::P1080, hdr: false },
                fullscreenizer: false,
                optimize: true,
                rev: 42,
                invite: Some(GameInvite { exe: "g.exe".into(), state: InviteState::Ready, code: "ABCD".into(), link: "https://x/ABCD".into(), reason: String::new() }),
                eq_profiles: vec![],
                eq_profile: String::new(),
            },
        ];
        for r in replies {
            let json = serde_json::to_string(&r).unwrap();
            assert_eq!(serde_json::from_str::<Reply>(&json).unwrap(), r, "{json}");
        }
    }

    #[test]
    fn old_peer_reads_options_as_unknown() {
        // O Prisma 1.0 não conhece `options` nem `set_option`.
        #[derive(Debug, PartialEq, Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum OldRequest {
            SaveReplay,
            #[serde(other)]
            Unknown,
        }
        for json in [
            r#"{"type":"options","exe":"g.exe"}"#,
            r#"{"type":"set_option","option":{"optimize":true}}"#,
            r#"{"type":"share_invite_for_game","exe":"g.exe"}"#,
            r#"{"type":"share_invite_cancel"}"#,
        ] {
            assert_eq!(serde_json::from_str::<OldRequest>(json).unwrap(), OldRequest::Unknown);
        }
    }

    #[test]
    fn new_pages_parse() {
        for (name, page) in [("energy", Page::Energy), ("fullscreen", Page::Fullscreen), ("sound", Page::Sound), ("hdr", Page::Hdr), ("nvidia", Page::Nvidia), ("capture", Page::Capture), ("controllers", Page::Controllers)] {
            let r: Request = serde_json::from_str(&format!(r#"{{"type":"open_page","page":"{name}"}}"#)).unwrap();
            assert_eq!(r, Request::OpenPage { page });
        }
    }

    #[test]
    fn unknown_page_reads_as_unknown() {
        let r: Request = serde_json::from_str(r#"{"type":"open_page","page":"stats"}"#).unwrap();
        assert_eq!(
            r,
            Request::OpenPage {
                page: Page::Unknown
            }
        );
    }

    #[test]
    fn extra_fields_are_ignored() {
        let r: Request =
            serde_json::from_str(r#"{"type":"game_stopped","pid":5,"extra":true}"#).unwrap();
        assert_eq!(r, Request::GameStopped { pid: 5 });
    }

    #[test]
    fn capabilities_has() {
        let c = Capabilities {
            protocol: 1,
            app_version: "x".into(),
            caps: vec!["page.meter".into(), "foo".into()],
        };
        assert!(c.has("page.meter"));
        assert!(!c.has("save_replay"));
    }
}
