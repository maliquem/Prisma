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
    #[serde(other)]
    Unknown,
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
    /// Pedido recusado; o motivo vem no idioma do Prisma.
    Failed {
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
            Reply::Failed { reason: "x".into() },
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
        ];
        for r in replies {
            let json = serde_json::to_string(&r).unwrap();
            assert_eq!(serde_json::from_str::<Reply>(&json).unwrap(), r, "{json}");
        }
        assert_eq!(serde_json::to_string(&Reply::Done).unwrap(), r#"{"type":"done"}"#);
    }

    #[test]
    fn new_pages_parse() {
        for (name, page) in [("energy", Page::Energy), ("fullscreen", Page::Fullscreen), ("sound", Page::Sound), ("hdr", Page::Hdr), ("nvidia", Page::Nvidia), ("capture", Page::Capture)] {
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
