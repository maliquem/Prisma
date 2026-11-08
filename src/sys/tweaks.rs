//! Catálogo curado de ajustes do Windows e diário de reversão.
//! Regra central: capturar o valor real antes de escrever, inclusive "não existia",
//! e só desfazer se ninguém mexeu depois. Nunca toca em atualização, antivírus, UAC,
//! Integridade da Memória ou plataforma de virtualização.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use windows::core::*;
use windows::Win32::System::Services::*;

use super::reg::{self, Hive};

const CDM: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\ContentDeliveryManager";
const ADV: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Act {
    /// Valor DWORD no registro.
    Dw(Hive, &'static str, &'static str, u32),
    /// Tipo de início de serviço: 3 manual, 4 desativado.
    Svc(&'static str, u32),
    /// Pacote Appx, para este usuário e para contas novas.
    App(&'static str),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Class {
    /// Reversível na hora.
    R1,
    /// Reversível com reinício.
    R2,
    /// Reinstalável pela Loja; pode falhar.
    R3,
}

pub struct Tweak {
    pub id: &'static str,
    pub title: &'static str,
    pub note: &'static str,
    pub class: Class,
    /// Perfis em que vem marcado: J jogo, D desenvolvimento, P privacidade.
    pub base: &'static str,
    /// Perfis em que aparece desmarcado.
    pub opt: &'static str,
    pub subjective: bool,
    pub acts: &'static [Act],
}

use Act::*;
use Class::*;
use Hive::{Machine as M, User as U};

pub const CATALOG: &[Tweak] = &[
    Tweak {
        id: "diag",
        title: "Dados de diagnóstico no mínimo do Pro",
        note: "Fica em Necessários. Desligar por completo não existe no Pro.",
        class: R1, base: "JDP", opt: "", subjective: false,
        acts: &[Dw(M, "SOFTWARE\\Policies\\Microsoft\\Windows\\DataCollection", "AllowTelemetry", 1)],
    },
    Tweak {
        id: "ads",
        title: "Sem ID de publicidade e experiências personalizadas",
        note: "Consenso entre as seis ferramentas analisadas.",
        class: R1, base: "JDP", opt: "", subjective: false,
        acts: &[
            Dw(M, "SOFTWARE\\Policies\\Microsoft\\Windows\\AdvertisingInfo", "DisabledByGroupPolicy", 1),
            Dw(U, "Software\\Microsoft\\Windows\\CurrentVersion\\AdvertisingInfo", "Enabled", 0),
            Dw(U, "Software\\Microsoft\\Windows\\CurrentVersion\\Privacy", "TailoredExperiencesWithDiagnosticDataEnabled", 0),
        ],
    },
    Tweak {
        id: "suggest",
        title: "Sem conteúdo sugerido e instalação silenciosa de apps",
        note: "Só para este usuário: as políticas equivalentes não valem no Pro.",
        class: R1, base: "JDP", opt: "", subjective: false,
        acts: &[
            Dw(U, CDM, "SilentInstalledAppsEnabled", 0),
            Dw(U, CDM, "SystemPaneSuggestionsEnabled", 0),
            Dw(U, CDM, "SoftLandingEnabled", 0),
            Dw(U, CDM, "SubscribedContent-338388Enabled", 0),
            Dw(U, CDM, "SubscribedContent-338389Enabled", 0),
            Dw(U, CDM, "SubscribedContent-338393Enabled", 0),
            Dw(U, CDM, "SubscribedContent-353694Enabled", 0),
            Dw(U, CDM, "SubscribedContent-353696Enabled", 0),
        ],
    },
    Tweak {
        id: "search",
        title: "Pesquisa sem destaques, nuvem e localização",
        note: "Não desliga o Bing por inteiro: essa opção não existe no Pro.",
        class: R1, base: "JDP", opt: "", subjective: false,
        acts: &[
            Dw(M, "SOFTWARE\\Policies\\Microsoft\\Windows\\Windows Search", "AllowCloudSearch", 0),
            Dw(M, "SOFTWARE\\Policies\\Microsoft\\Windows\\Windows Search", "AllowSearchToUseLocation", 0),
            Dw(M, "SOFTWARE\\Policies\\Microsoft\\Windows\\Windows Search", "EnableDynamicContentInWSB", 0),
        ],
    },
    Tweak {
        id: "widgets",
        title: "Widgets desligados",
        note: "Tira o painel de notícias.",
        class: R1, base: "JDP", opt: "", subjective: false,
        acts: &[Dw(M, "SOFTWARE\\Policies\\Microsoft\\Dsh", "AllowNewsAndInterests", 0)],
    },
    Tweak {
        id: "peers",
        title: "Atualizações sem compartilhar com outros PCs",
        note: "O Windows continua baixando normalmente da Microsoft.",
        class: R1, base: "JDP", opt: "", subjective: false,
        acts: &[Dw(M, "SOFTWARE\\Policies\\Microsoft\\Windows\\DeliveryOptimization", "DODownloadMode", 99)],
    },
    Tweak {
        id: "feedback",
        title: "Sem pedidos de feedback e com menos relatórios",
        note: "Baixo impacto.",
        class: R1, base: "JDP", opt: "", subjective: false,
        acts: &[
            Dw(M, "SOFTWARE\\Policies\\Microsoft\\Windows\\DataCollection", "DoNotShowFeedbackNotifications", 1),
            Dw(M, "SOFTWARE\\Policies\\Microsoft\\Windows\\DataCollection", "LimitDumpCollection", 1),
            Dw(M, "SOFTWARE\\Policies\\Microsoft\\Windows\\DataCollection", "LimitDiagnosticLogCollection", 1),
        ],
    },
    Tweak {
        id: "maps",
        title: "Serviço de mapas abre só quando precisar",
        note: "O único serviço automático que vale mudar.",
        class: R1, base: "JDP", opt: "", subjective: false,
        acts: &[Svc("MapsBroker", 3)],
    },
    Tweak {
        id: "activity",
        title: "Sem publicar histórico de atividades",
        note: "Mantém o histórico da área de transferência funcionando.",
        class: R1, base: "P", opt: "JD", subjective: false,
        acts: &[
            Dw(M, "SOFTWARE\\Policies\\Microsoft\\Windows\\System", "PublishUserActivities", 0),
            Dw(M, "SOFTWARE\\Policies\\Microsoft\\Windows\\System", "UploadUserActivities", 0),
        ],
    },
    Tweak {
        id: "clipboard",
        title: "Área de transferência sem nuvem",
        note: "Copiar e colar entre aparelhos para de funcionar.",
        class: R1, base: "P", opt: "", subjective: false,
        acts: &[Dw(M, "SOFTWARE\\Policies\\Microsoft\\Windows\\System", "AllowCrossDeviceClipboard", 0)],
    },
    Tweak {
        id: "startrec",
        title: "Menu Iniciar sem recomendações",
        note: "Questão de gosto.",
        class: R1, base: "P", opt: "JD", subjective: true,
        acts: &[Dw(U, ADV, "Start_TrackDocs", 0), Dw(U, ADV, "Start_IrisRecommendations", 0)],
    },
    Tweak {
        id: "ai",
        title: "IA do Windows desligada onde a política vale",
        note: "Paint e análise de dados. O Recall só roda em PCs com NPU.",
        class: R1, base: "P", opt: "JD", subjective: true,
        acts: &[
            Dw(U, "Software\\Policies\\Microsoft\\Windows\\WindowsAI", "DisableAIDataAnalysis", 1),
            Dw(M, "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\Paint", "DisableImageCreator", 1),
        ],
    },
    Tweak {
        id: "diagtrack",
        title: "Parar o serviço de telemetria",
        note: "Opcional e com aviso: há relatos de efeito em conquistas do Xbox.",
        class: R1, base: "", opt: "P", subjective: false,
        acts: &[Svc("DiagTrack", 3)],
    },
    Tweak {
        id: "dvr",
        title: "Sem gravação em segundo plano do Game Bar",
        note: "Sem ganho medido. Só evita que o Windows grave os últimos segundos.",
        class: R1, base: "", opt: "J", subjective: true,
        acts: &[Dw(U, "Software\\Microsoft\\Windows\\CurrentVersion\\GameDVR", "HistoricalCaptureEnabled", 0)],
    },
    Tweak {
        id: "longpaths",
        title: "Caminhos de arquivo longos",
        note: "Evita erros em projetos grandes. Pode pedir reinício.",
        class: R2, base: "D", opt: "", subjective: false,
        acts: &[Dw(M, "SYSTEM\\CurrentControlSet\\Control\\FileSystem", "LongPathsEnabled", 1)],
    },
    Tweak {
        id: "devmode",
        title: "Modo Desenvolvedor",
        note: "Permite instalar pacotes de teste e criar links sem administrador.",
        class: R1, base: "", opt: "D", subjective: false,
        acts: &[Dw(M, "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\AppModelUnlock", "AllowDevelopmentWithoutDevLicense", 1)],
    },
    Tweak {
        id: "ext",
        title: "Mostrar extensões de arquivo",
        note: "Conveniência, e ajuda a notar arquivos disfarçados.",
        class: R1, base: "D", opt: "JP", subjective: true,
        acts: &[Dw(U, ADV, "HideFileExt", 0)],
    },
    Tweak {
        id: "endtask",
        title: "Finalizar tarefa na barra de tarefas",
        note: "Conveniência.",
        class: R1, base: "", opt: "JDP", subjective: true,
        acts: &[Dw(U, "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced\\TaskbarDeveloperSettings", "TaskbarEndTask", 1)],
    },
    Tweak {
        id: "apps",
        title: "Remover apps que quase ninguém usa",
        note: "Notícias, Clima, Solitaire, Clipchamp, Feedback, Office Hub, To Do e Power Automate.",
        class: R3, base: "P", opt: "JD", subjective: false,
        acts: &[
            App("Microsoft.BingNews"),
            App("Microsoft.BingWeather"),
            App("Microsoft.MicrosoftSolitaireCollection"),
            App("Clipchamp.Clipchamp"),
            App("Microsoft.WindowsFeedbackHub"),
            App("Microsoft.MicrosoftOfficeHub"),
            App("Microsoft.Todos"),
            App("Microsoft.PowerAutomateDesktop"),
        ],
    },
    Tweak {
        id: "apps2",
        title: "Remover Notas, Alarmes e Gravador",
        note: "Consenso entre ferramentas, mas muita gente usa.",
        class: R3, base: "", opt: "JDP", subjective: true,
        acts: &[
            App("Microsoft.MicrosoftStickyNotes"),
            App("Microsoft.WindowsAlarms"),
            App("Microsoft.WindowsSoundRecorder"),
        ],
    },
    Tweak {
        id: "copilot",
        title: "Remover o app Copilot",
        note: "Só o app. A Microsoft não oferece política clara para o Pro.",
        class: R3, base: "", opt: "JDP", subjective: true,
        acts: &[App("Microsoft.Copilot")],
    },
];

/// Pacotes que o Prisma recusa remover, seja qual for o pedido.
const NEVER_REMOVE: &[&str] = &[
    "microsoft.windowsstore",
    "microsoft.storepurchaseapp",
    "microsoft.desktopappinstaller",
    "microsoft.sechealthui",
    "microsoft.gamingservices",
    "microsoft.xboxidentityprovider",
    "microsoft.xbox.tcui",
    "microsoft.xboxgamingoverlay",
    "microsoft.gamingapp",
    "microsoft.windowshdrcalibration",
    "microsoft.vclibs",
    "microsoft.net.native",
    "microsoft.ui.xaml",
    "microsoft.windows.photos",
    "microsoft.paint",
    "microsoft.windowsnotepad",
    "microsoft.windowscalculator",
    "microsoft.windowsterminal",
];

pub fn may_remove(name: &str) -> bool {
    let low = name.to_ascii_lowercase();
    !NEVER_REMOVE.iter().any(|n| low.starts_with(n))
}

/// O que ficou de fora, e por quê. Faz parte do produto: recusar também é informar.
pub const LEFT_OUT: &[(&str, &str, &str)] = &[
    ("Integridade da Memória desligada", "6% de FPS em 720p e cerca de 2% em 4K, mas tira uma proteção real do kernel. A escolha fica na tela oficial do Windows.", "Medido"),
    ("Ferramentas de timer", "Desde 2020 o Windows não muda a resolução de outros programas.", "Documentado"),
    ("Limpadores de RAM", "Sem ganho medido; a purga repetida causou travadas com 32 GB.", "Relatos"),
    ("Planos Ultimate e Bitsum", "Sem teste independente em Ryzen 5000.", "Sem dados"),
    ("Nagle, TcpAckFrequency", "Jogos modernos usam UDP nos dados críticos.", "Relatos"),
    ("NetworkThrottlingIndex", "Não melhora, e a latência de rede subiu ao desligar.", "Medido"),
    ("SystemResponsiveness, prioridade Games", "Nenhum jogo foi visto usando; o valor 100 impediu o serviço de iniciar.", "Medido"),
    ("Modo MSI, isolar núcleos, afinidade", "Pouco ou nada, e um jogo ficou injogável.", "Relatos"),
    ("Listas de serviços em massa", "Quebraram Bluetooth; as ferramentas sérias recuaram disso.", "Documentado"),
    ("Limiar de svchost", "Travou serviços do Xbox; dois projetos removeram.", "Documentado"),
    ("AllowExperimentation", "Bloqueia correções automáticas de problemas da Microsoft.", "Documentado"),
    ("Pagefile desligado", "Um PC de 32 GB chegou a usar 39 GB de memória reservada.", "Documentado"),
    ("Bloquear atualizações", "A Microsoft recomenda contra, e há relatos de quebra.", "Documentado"),
    ("Defender, UAC, SmartScreen", "Reduzem segurança, sem ganho.", "Documentado"),
    ("Virtualização desligada", "Derruba o WSL e outras máquinas virtuais.", "Documentado"),
    ("Remover Edge ou Loja", "Sem reversão confiável.", "Documentado"),
];

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub tweak: String,
    /// Texto que identifica o alvo: `reg|HKLM|chave|nome`, `svc|nome` ou `app|nome`.
    pub target: String,
    /// Valor antes, em texto; `-` quando não existia.
    pub before: String,
    pub after: String,
}

fn journal_path() -> PathBuf {
    crate::config::data_dir().join("diario.txt")
}

pub fn load_journal() -> Vec<Entry> {
    let Ok(t) = std::fs::read_to_string(journal_path()) else { return Vec::new() };
    t.lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split('\t').collect();
            (p.len() == 4).then(|| Entry { tweak: p[0].into(), target: p[1].into(), before: p[2].into(), after: p[3].into() })
        })
        .collect()
}

fn save_journal(list: &[Entry]) {
    let mut s = String::new();
    for e in list {
        s.push_str(&format!("{}\t{}\t{}\t{}\n", e.tweak, e.target, e.before, e.after));
    }
    let _ = std::fs::create_dir_all(crate::config::data_dir());
    let _ = std::fs::write(journal_path(), s);
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|v| format!("{v:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).filter_map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()).collect()
}

fn reg_state(h: Hive, key: &str, name: &str) -> String {
    match reg::read(h, key, name) {
        Some((t, b)) => format!("{t}:{}", hex(&b)),
        None => "-".to_string(),
    }
}

fn svc_key(name: &str) -> String {
    format!("SYSTEM\\CurrentControlSet\\Services\\{name}")
}

fn svc_state(name: &str) -> String {
    match reg::dword(Hive::Machine, &svc_key(name), "Start") {
        Some(v) => {
            let d = reg::dword(Hive::Machine, &svc_key(name), "DelayedAutostart").unwrap_or(0);
            format!("{v}:{d}")
        }
        None => "-".to_string(),
    }
}

fn set_service(name: &str, start: u32, delayed: bool) -> bool {
    unsafe {
        let Ok(scm) = OpenSCManagerW(None, None, SC_MANAGER_CONNECT) else { return false };
        let mut ok = false;
        if let Ok(h) = OpenServiceW(scm, &HSTRING::from(name), SERVICE_CHANGE_CONFIG) {
            ok = ChangeServiceConfigW(
                h,
                ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
                SERVICE_START_TYPE(start),
                SERVICE_ERROR(SERVICE_NO_CHANGE),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .is_ok();
            if ok && start == 2 {
                let info = SERVICE_DELAYED_AUTO_START_INFO { fDelayedAutostart: delayed.into() };
                let _ = ChangeServiceConfig2W(h, SERVICE_CONFIG_DELAYED_AUTO_START_INFO, Some(&info as *const _ as *const _));
            }
            let _ = CloseServiceHandle(h);
        }
        let _ = CloseServiceHandle(scm);
        ok
    }
}

fn powershell(cmd: &str) -> Option<String> {
    use std::os::windows::process::CommandExt;
    let o = std::process::Command::new(crate::sys::system_tool("powershell"))
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", cmd])
        .creation_flags(0x0800_0000)
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&o.stdout).to_string())
}

/// Nomes dos pacotes instalados para este usuário.
pub fn installed_apps() -> Vec<String> {
    powershell("(Get-AppxPackage).Name").unwrap_or_default().lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

fn remove_app(name: &str) -> bool {
    if !may_remove(name) || name.contains(['\'', '"', ';', ' ']) {
        return false;
    }
    let cmd = format!(
        "Get-AppxPackage -Name '{name}' | Remove-AppxPackage -ErrorAction SilentlyContinue; \
         Get-AppxProvisionedPackage -Online | Where-Object DisplayName -eq '{name}' | Remove-AppxProvisionedPackage -Online -ErrorAction SilentlyContinue | Out-Null; \
         if (Get-AppxPackage -Name '{name}') {{ 'ficou' }} else {{ 'saiu' }}"
    );
    powershell(&cmd).is_some_and(|o| o.contains("saiu"))
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Status {
    /// Está como o Windows estava.
    Off,
    /// Aplicado e ainda em vigor.
    On,
    /// O Prisma aplicou, mas algo mudou depois (uma atualização, por exemplo).
    Drifted,
    /// Já estava assim, sem o Prisma.
    Already,
}

fn act_matches(a: &Act, apps: &[String]) -> bool {
    match a {
        Dw(h, k, n, v) => reg::dword(*h, k, n) == Some(*v),
        Svc(n, s) => reg::dword(Hive::Machine, &svc_key(n), "Start") == Some(*s),
        App(n) => !apps.is_empty() && !apps.iter().any(|x| x.eq_ignore_ascii_case(n)),
    }
}

pub fn status(t: &Tweak, journal: &[Entry], apps: &[String]) -> Status {
    let all = t.acts.iter().all(|a| act_matches(a, apps));
    let ours = journal.iter().any(|e| e.tweak == t.id);
    match (ours, all) {
        (true, true) => Status::On,
        (true, false) => Status::Drifted,
        (false, true) => Status::Already,
        (false, false) => Status::Off,
    }
}

/// Aplica um ajuste e registra o estado anterior. Devolve quantas ações falharam.
pub fn apply(t: &Tweak, journal: &mut Vec<Entry>, log: &mut dyn FnMut(String)) -> usize {
    apply_acts(t.id, t.acts, journal, log)
}

/// Valor do registro como aparece no passo a passo.
fn show_reg(s: &str) -> String {
    match s.split_once(':') {
        Some(("4", b)) => {
            let v = unhex(b);
            v.get(..4).map_or(s.to_string(), |d| u32::from_le_bytes([d[0], d[1], d[2], d[3]]).to_string())
        }
        Some((_, _)) => "outro valor".to_string(),
        None => "não existia".to_string(),
    }
}

fn show_svc(s: &str) -> String {
    let (start, delayed) = s.split_once(':').unwrap_or((s, "0"));
    match (start, delayed) {
        ("2", "1") => "Automático com atraso".to_string(),
        ("2", _) => "Automático".to_string(),
        ("3", _) => "Manual".to_string(),
        ("4", _) => "Desativado".to_string(),
        _ => format!("tipo {start}"),
    }
}

pub fn apply_acts(id: &str, acts: &[Act], journal: &mut Vec<Entry>, log: &mut dyn FnMut(String)) -> usize {
    let mut failed = 0;
    for a in acts {
        match a {
            Dw(h, k, n, v) => {
                let target = format!("reg|{}|{k}|{n}", h.tag());
                let name = format!("{}\\{k}\\{n}", h.tag());
                let before = reg_state(*h, k, n);
                if reg::write_dword(*h, k, n, *v) {
                    log(format!("  ✓ Registro {name}: {} → {v}", show_reg(&before)));
                    journal.push(Entry { tweak: id.into(), target, before, after: reg_state(*h, k, n) });
                    save_journal(journal);
                } else {
                    log(format!("  ✗ Registro {name}: o Windows recusou a escrita"));
                    failed += 1;
                }
            }
            Svc(n, s) => {
                let before = svc_state(n);
                if before == "-" {
                    log(format!("  Serviço {n}: não existe neste PC, nada a fazer"));
                    continue;
                }
                if set_service(n, *s, false) {
                    let after = svc_state(n);
                    log(format!("  ✓ Serviço {n}: início {} → {}", show_svc(&before), show_svc(&after)));
                    journal.push(Entry { tweak: id.into(), target: format!("svc|{n}"), before, after });
                    save_journal(journal);
                } else {
                    log(format!("  ✗ Serviço {n}: o Windows recusou a mudança"));
                    failed += 1;
                }
            }
            App(n) => {
                log(format!("  Removendo o app {n}…"));
                if remove_app(n) {
                    log(format!("  ✓ App {n}: removido"));
                    journal.push(Entry { tweak: id.into(), target: format!("app|{n}"), before: "instalado".into(), after: "removido".into() });
                    save_journal(journal);
                } else {
                    log(format!("  ✗ App {n}: continua instalado"));
                    failed += 1;
                }
            }
        }
    }
    failed
}

#[derive(Default, Debug, Clone)]
pub struct UndoReport {
    pub restored: usize,
    /// Alguém mudou depois do Prisma: não mexe e avisa.
    pub skipped: Vec<String>,
    /// Apps removidos voltam pela Loja, não aqui.
    pub apps: Vec<String>,
    pub lines: Vec<String>,
}

/// Desfaz um ajuste, ou todos com `id` vazio, do mais novo para o mais velho.
pub fn undo(id: &str, journal: &mut Vec<Entry>) -> UndoReport {
    let mut rep = UndoReport::default();
    let mut keep = Vec::new();
    for e in journal.drain(..).rev() {
        if !id.is_empty() && e.tweak != id {
            keep.push(e);
            continue;
        }
        let p: Vec<&str> = e.target.splitn(4, '|').collect();
        match p[0] {
            "reg" if p.len() == 4 => {
                let h = Hive::parse(p[1]);
                let name = format!("{}\\{}\\{}", p[1], p[2], p[3]);
                if reg_state(h, p[2], p[3]) != e.after {
                    rep.lines.push(format!("  ✗ Registro {name}: mudou depois do Prisma, ficou como está"));
                    rep.skipped.push(format!("{}\\{}", p[2], p[3]));
                    continue;
                }
                let ok = if e.before == "-" {
                    reg::delete(h, p[2], p[3])
                } else {
                    let (t, b) = e.before.split_once(':').unwrap_or(("4", ""));
                    reg::write(h, p[2], p[3], t.parse().unwrap_or(4), &unhex(b))
                };
                if ok {
                    rep.lines.push(if e.before == "-" {
                        format!("  ✓ Registro {name}: removido, como era antes")
                    } else {
                        format!("  ✓ Registro {name}: voltou para {}", show_reg(&e.before))
                    });
                    rep.restored += 1;
                } else {
                    rep.lines.push(format!("  ✗ Registro {name}: não consegui restaurar"));
                    keep.push(e);
                }
            }
            "svc" if p.len() == 2 => {
                if svc_state(p[1]) != e.after {
                    rep.lines.push(format!("  ✗ Serviço {}: mudou depois do Prisma, ficou como está", p[1]));
                    rep.skipped.push(p[1].to_string());
                    continue;
                }
                let (s, d) = e.before.split_once(':').unwrap_or(("3", "0"));
                if set_service(p[1], s.parse().unwrap_or(3), d == "1") {
                    rep.lines.push(format!("  ✓ Serviço {}: início voltou para {}", p[1], show_svc(&e.before)));
                    rep.restored += 1;
                } else {
                    rep.lines.push(format!("  ✗ Serviço {}: não consegui restaurar", p[1]));
                    keep.push(e);
                }
            }
            "app" if p.len() == 2 => {
                rep.lines.push(format!("  App {}: volta pela Loja, não por aqui", p[1]));
                rep.apps.push(p[1].to_string());
            }
            _ => {}
        }
    }
    keep.reverse();
    *journal = keep;
    save_journal(journal);
    rep
}

#[derive(Default, Clone)]
pub struct Run {
    pub running: bool,
    pub current: String,
    pub done: usize,
    pub total: usize,
    pub failed: Vec<String>,
    pub restore: Option<bool>,
    pub checks: Vec<(bool, String)>,
    pub lines: Vec<String>,
}

pub type Shared = Arc<Mutex<Run>>;

fn say(s: &Shared, line: String) {
    s.lock().unwrap().lines.push(line);
}

/// Confere que o essencial segue de pé depois de aplicar.
fn verify() -> Vec<(bool, String)> {
    let start = |n: &str| reg::dword(Hive::Machine, &svc_key(n), "Start").unwrap_or(0);
    let store = powershell("(Get-AppxPackage -Name Microsoft.WindowsStore).Name").is_some_and(|o| o.contains("WindowsStore"));
    vec![
        (start("wuauserv") != 4, "Windows Update não está desativado".to_string()),
        (start("BITS") != 4, "Transferência em segundo plano não está desativada".to_string()),
        (start("wscsvc") != 4, "Central de Segurança ativa".to_string()),
        (store, "Loja do Windows presente".to_string()),
    ]
}

use super::tools::clock;

pub fn start(ids: Vec<&'static str>, restore: bool, shared: Shared) {
    *shared.lock().unwrap() = Run { running: true, total: ids.len(), ..Default::default() };
    let _ = std::thread::Builder::new().name("ajustes".into()).spawn(move || {
        let begin = std::time::Instant::now();
        if restore {
            say(&shared, format!("▸ {}  Ponto de restauração", clock()));
            let ok = super::uninstall::restore_point("Prisma: antes de ajustar o Windows");
            shared.lock().unwrap().restore = Some(ok);
            say(&shared, if ok { "  ✓ Criado".to_string() } else { "  ✗ O Windows recusou".to_string() });
        }
        let mut j = load_journal();
        for id in ids {
            let Some(t) = CATALOG.iter().find(|t| t.id == id) else { continue };
            shared.lock().unwrap().current = t.title.to_string();
            say(&shared, format!("▸ {}  {}", clock(), t.title));
            let log = shared.clone();
            let bad = apply(t, &mut j, &mut |l| say(&log, l));
            let mut s = shared.lock().unwrap();
            s.done += 1;
            if bad > 0 {
                s.failed.push(t.title.to_string());
            }
        }
        say(&shared, format!("▸ {}  Conferindo o essencial", clock()));
        shared.lock().unwrap().current = "Conferindo o essencial".to_string();
        let c = verify();
        for (ok, text) in &c {
            say(&shared, format!("  {} {text}", if *ok { "✓" } else { "✗" }));
        }
        say(&shared, format!("▸ {}  Fim, em {}", clock(), super::tools::elapsed(begin.elapsed())));
        let mut s = shared.lock().unwrap();
        s.checks = c;
        s.current.clear();
        s.running = false;
    });
}
