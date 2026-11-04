//! Ferramentas do Windows, com salvaguarda antes e veredito depois.

use super::*;
use crate::sys::disks::Level;
use crate::sys::tools::{self, cmd, native, neutral, simple, Enc, Step};
use crate::sys::{events, reg};

#[derive(Clone, Copy, PartialEq)]
pub enum Id {
    Repair,
    Crashes,
    Copy,
    Chkdsk,
    Protect,
    Unprotect,
    Retrim,
    WuReset,
    Flush,
    NetReset,
    Dns,
    Clean,
    Winget,
    WingetAll,
    Store,
    StoreClean,
    Wake,
    Drivers,
    Support,
    Memory,
    Memresult,
    Hiber,
    Recovery,
    Advanced,
    Firmware,
    Panels,
}

struct Item {
    id: Id,
    group: &'static str,
    title: &'static str,
    desc: &'static str,
    /// Muda algo no sistema: pergunta antes de rodar.
    writes: bool,
}

const LIST: &[Item] = &[
    Item { id: Id::Repair, group: "Reparar", title: "Reparar o Windows", desc: "Roda o DISM, que baixa o que estiver danificado, e depois o SFC, nessa ordem, como manda a Microsoft. Pode levar de 10 a 30 minutos e usa o disco do sistema.", writes: true },
    Item { id: Id::Crashes, group: "Reparar", title: "Histórico de falhas", desc: "Reinícios inesperados, telas azuis e erros de hardware dos últimos 90 dias. Só leitura.", writes: false },
    Item { id: Id::WuReset, group: "Reparar", title: "Consertar o Windows Update", desc: "Para os serviços, renomeia as pastas de cache sem apagá-las e religa tudo. Para quando as atualizações travam.", writes: true },
    Item { id: Id::Copy, group: "Disco", title: "Copiar dados de um disco com defeito", desc: "Primeiro passo em disco suspeito. Lê uma vez, pula o que estiver ruim e registra o que falhou. Só escreve no destino.", writes: false },
    Item { id: Id::Chkdsk, group: "Disco", title: "Verificar disco", desc: "O modo sem reparo só lê. Os modos que corrigem escrevem no disco e ficam bloqueados em disco suspeito.", writes: true },
    Item { id: Id::Protect, group: "Disco", title: "Impedir verificação automática", desc: "Evita que o Windows rode a verificação sozinho ao ligar num disco suspeito, até você copiar os dados.", writes: true },
    Item { id: Id::Unprotect, group: "Disco", title: "Voltar a verificação automática", desc: "Desfaz o item anterior, em todos os discos.", writes: true },
    Item { id: Id::Retrim, group: "Disco", title: "Otimizar SSD (TRIM)", desc: "Avisa o SSD sobre o espaço livre. Só para SSD saudável.", writes: true },
    Item { id: Id::Flush, group: "Rede", title: "Limpar cache de DNS", desc: "Primeiro degrau quando sites não abrem. Não tem efeito colateral.", writes: false },
    Item { id: Id::NetReset, group: "Rede", title: "Refazer a pilha de rede", desc: "Segundo degrau: Winsock e TCP/IP. Salva o estado atual antes. Pede reinício.", writes: true },
    Item { id: Id::Dns, group: "Rede", title: "Trocar DNS", desc: "Muda o servidor de nomes do adaptador. DNS criptografado se ativa em Configurações, Rede.", writes: true },
    Item { id: Id::Clean, group: "Limpeza", title: "Limpar arquivos do sistema", desc: "Temporários, restos de atualização e relatórios. Não toca em Windows.old, Lixeira, Downloads nem shaders.", writes: true },
    Item { id: Id::Winget, group: "Programas", title: "Ver programas desatualizados", desc: "Lista o que tem versão nova pelo winget.", writes: false },
    Item { id: Id::WingetAll, group: "Programas", title: "Atualizar todos os programas", desc: "Atualiza pelo winget, um por um, sem janelas.", writes: true },
    Item { id: Id::Store, group: "Avançado", title: "Analisar cache de atualizações", desc: "Mostra quanto a pasta WinSxS ocupa de verdade e se vale limpar.", writes: false },
    Item { id: Id::StoreClean, group: "Avançado", title: "Limpar cache de atualizações", desc: "Roda a tarefa do próprio Windows, que preserva 30 dias de atualizações.", writes: true },
    Item { id: Id::Wake, group: "Diagnóstico", title: "O que acorda o PC", desc: "Último despertar, estados de energia disponíveis e o que impede o sono.", writes: false },
    Item { id: Id::Drivers, group: "Diagnóstico", title: "Drivers com problema", desc: "Lista dispositivos com erro. Só leitura.", writes: false },
    Item { id: Id::Support, group: "Diagnóstico", title: "Gerar pacote de suporte", desc: "Salva relatório de vídeo, informações do sistema e histórico de falhas numa pasta.", writes: false },
    Item { id: Id::Memory, group: "Diagnóstico", title: "Testar memória", desc: "Abre o Diagnóstico de Memória do Windows, que reinicia o PC e testa antes de ligar.", writes: true },
    Item { id: Id::Memresult, group: "Diagnóstico", title: "Ver resultado do teste de memória", desc: "Lê o último resultado. Sem registro não quer dizer aprovado.", writes: false },
    Item { id: Id::Hiber, group: "Sistema", title: "Ligar ou desligar hibernação", desc: "Desligar libera o hiberfil.sys, mas pode afetar a Inicialização Rápida.", writes: true },
    Item { id: Id::Recovery, group: "Sistema", title: "Estado da recuperação", desc: "Mostra se o ambiente de recuperação do Windows está ativo.", writes: false },
    Item { id: Id::Advanced, group: "Sistema", title: "Reiniciar em modo avançado", desc: "Abre as opções de recuperação, inclusive Modo de Segurança. Reinicia agora.", writes: true },
    Item { id: Id::Firmware, group: "Sistema", title: "Reiniciar na BIOS", desc: "Reinicia direto na configuração da placa-mãe.", writes: true },
    Item { id: Id::Panels, group: "Sistema", title: "Painéis do Windows", desc: "Atalhos para ferramentas escondidas.", writes: false },
];

const PANELS: &[(&str, &str, &str)] = &[
    ("Confiabilidade", "perfmon", "/rel"),
    ("Eventos", "eventvwr.msc", ""),
    ("Dispositivos", "devmgmt.msc", ""),
    ("Disco", "diskmgmt.msc", ""),
    ("Inicialização", "msconfig", ""),
    ("Recursos", "resmon", ""),
    ("Todas as opções", "explorer", "shell:::{ED7BA470-8E54-465E-825C-99712043E01C}"),
];

const DNS: &[(&str, &str, &str)] = &[
    ("Automático do roteador", "", ""),
    ("Cloudflare", "1.1.1.1", "1.0.0.1"),
    ("Google", "8.8.8.8", "8.8.4.4"),
    ("Quad9", "9.9.9.9", "149.112.112.112"),
];

const CHK_MODES: [&str; 4] = ["Só verificar, sem reparar", "Reparar online (/scan)", "Corrigir (/f)", "Corrigir e ler a superfície (/r)"];

fn elevate_needed(id: Id) -> bool {
    !matches!(id, Id::Crashes | Id::Flush | Id::Winget | Id::Wake | Id::Drivers | Id::Memresult | Id::Panels | Id::Copy)
}

fn plan(id: Id, app: &mut App, letter: char) -> Option<(Vec<Step>, tools::Verdict)> {
    let d = format!("{letter}:");
    Some(match id {
        Id::Repair => {
            let since = tools::now_stamp();
            (
                vec![
                    cmd("DISM: conferir e reparar a imagem", "dism", &["/Online", "/Cleanup-Image", "/RestoreHealth", "/English"], Enc::Oem),
                    cmd("SFC: conferir os arquivos do sistema", "sfc", &["/scannow"], Enc::Utf16),
                ],
                Box::new(move |done: &[tools::Done]| {
                    if done.first().is_some_and(|s| !s.ok) {
                        return (Level::Risk, "O DISM não conseguiu reparar a imagem. Veja a saída acima e o arquivo dism.log.".to_string());
                    }
                    tools::sfc_verdict(&since, &tools::read_cbs())
                }),
            )
        }
        Id::Crashes => (
            vec![native("Lendo o log do sistema", |l| {
                let list = events::crashes(90);
                if list.is_empty() {
                    l.push("Nenhuma queda ou erro grave em 90 dias.".to_string());
                }
                for e in list {
                    let what = match e.id {
                        41 => "Reinício sem desligar corretamente",
                        1001 => "Tela azul",
                        6008 => "Desligamento inesperado",
                        _ => "Erro de hardware (WHEA)",
                    };
                    l.push(format!("{}  {what}", e.time.replace('T', " ")));
                }
                true
            })],
            neutral("Horários em UTC. Vários itens no mesmo horário costumam ser a mesma queda."),
        ),
        Id::WuReset => {
            let stamp = tools::now_stamp().replace([' ', ':', '-'], "");
            let win = std::env::var("windir").unwrap_or_else(|_| "C:\\Windows".into());
            let services = ["wuauserv", "bits", "cryptsvc"];
            let mut s: Vec<Step> = services.iter().map(|n| cmd(&format!("Parar {n}"), "net", &["stop", n, "/y"], Enc::Oem)).collect();
            s.push(native("Renomear as pastas de cache", move |l| {
                let mut ok = true;
                for (dir, name) in [("SoftwareDistribution", "SoftwareDistribution"), ("System32\\catroot2", "catroot2")] {
                    let from = format!("{win}\\{dir}");
                    let to = format!("{from}.prisma-{stamp}");
                    match std::fs::rename(&from, &to) {
                        Ok(_) => l.push(format!("{name} guardada em {to}")),
                        Err(e) => {
                            l.push(format!("{name}: {e}"));
                            ok = false;
                        }
                    }
                }
                ok
            }));
            for n in services {
                s.push(cmd(&format!("Religar {n}"), "net", &["start", n], Enc::Oem));
            }
            app.log("resetou o cache do Windows Update");
            (s, simple("Pronto. Procure atualizações de novo em Configurações.", "Algo não terminou"))
        }
        Id::Copy => {
            if app.tool_src.is_empty() || app.tool_dst.is_empty() {
                return None;
            }
            let (a, b) = (app.tool_src.clone(), app.tool_dst.clone());
            (
                vec![Step {
                    label: "Copiando".to_string(),
                    work: tools::Work::Cmd {
                        exe: "robocopy".to_string(),
                        args: vec![a, b, "/E".into(), "/COPY:DAT".into(), "/DCOPY:T".into(), "/R:1".into(), "/W:1".into(), "/XJ".into(), "/NP".into(), "/NDL".into(), "/NFL".into()],
                        enc: Enc::Oem,
                    },
                }],
                Box::new(|done: &[tools::Done]| {
                    // No robocopy, códigos abaixo de 8 indicam sucesso.
                    match done.first().and_then(|s| s.code) {
                        Some(c) if c < 8 => (Level::Good, "Cópia concluída. Confira os arquivos no destino.".to_string()),
                        Some(_) => (Level::Warn, "Parte dos arquivos não foi copiada: pode ser área danificada. Veja a saída acima.".to_string()),
                        None => (Level::Risk, "A cópia não começou.".to_string()),
                    }
                }),
            )
        }
        Id::Chkdsk => {
            let args: Vec<String> = match app.tool_mode {
                1 => vec![d.clone(), "/scan".into()],
                2 => vec![d.clone(), "/f".into()],
                3 => vec![d.clone(), "/r".into()],
                _ => vec![d.clone()],
            };
            app.log(&format!("chkdsk {}", args.join(" ")));
            (
                vec![Step { label: format!("Verificando {d}"), work: tools::Work::Cmd { exe: "chkdsk".into(), args, enc: Enc::Ansi } }],
                Box::new(|done: &[tools::Done]| match done.first().and_then(|s| s.code) {
                    Some(0) => (Level::Good, "Sem problemas no sistema de arquivos.".to_string()),
                    Some(1) => (Level::Warn, "Erros encontrados e corrigidos.".to_string()),
                    Some(2) => (Level::Warn, "O disco está em uso: a verificação pede reinício. Em disco suspeito, copie os dados antes.".to_string()),
                    Some(_) => (Level::Risk, "A verificação achou problemas que não corrigiu. Veja a saída acima.".to_string()),
                    None => (Level::Risk, "A verificação não começou.".to_string()),
                }),
            )
        }
        Id::Protect => {
            app.log(&format!("chkntfs /x {d}"));
            (vec![cmd("Excluindo do chkdsk automático", "chkntfs", &["/x", &d], Enc::Oem)], simple("Pronto. Lembre de desfazer depois de trocar o disco.", "Não funcionou"))
        }
        Id::Unprotect => (vec![cmd("Voltando ao padrão", "chkntfs", &["/d"], Enc::Oem)], simple("Verificação automática de volta ao padrão.", "Não funcionou")),
        Id::Retrim => (vec![cmd("TRIM", "defrag", &[&d, "/L", "/U"], Enc::Oem)], neutral("Confira a saída: o defrag devolve sucesso até quando falha.")),
        Id::Flush => (vec![cmd("Limpando", "ipconfig", &["/flushdns"], Enc::Oem)], simple("Cache de DNS limpo.", "Não funcionou")),
        Id::NetReset => {
            let dir = crate::config::data_dir();
            app.log("refez a pilha de rede");
            (
                vec![
                    native("Salvando a configuração atual", move |l| {
                        let o = std::process::Command::new("ipconfig").arg("/all").output();
                        let _ = std::fs::create_dir_all(&dir);
                        let p = dir.join("rede_antes.txt");
                        let ok = o.is_ok_and(|o| std::fs::write(&p, o.stdout).is_ok());
                        l.push(format!("Guardada em {}", p.display()));
                        ok
                    }),
                    cmd("Winsock", "netsh", &["winsock", "reset"], Enc::Utf8),
                    cmd("TCP/IP", "netsh", &["int", "ip", "reset"], Enc::Utf8),
                ],
                simple("Pronto. Reinicie para valer.", "Não funcionou por inteiro"),
            )
        }
        Id::Dns => {
            let name = app.nets.iter().filter(|n| n.up).nth(app.tool_net).map(|n| n.name.clone())?;
            let (_, a, b) = DNS[app.tool_dns];
            app.log(&format!("trocou DNS de {name} para {}", if a.is_empty() { "automático" } else { a }));
            let n = format!("name={name}");
            let mut s = vec![];
            if a.is_empty() {
                s.push(cmd("Voltando ao automático", "netsh", &["interface", "ipv4", "set", "dnsservers", &n, "source=dhcp"], Enc::Utf8));
            } else {
                s.push(cmd("Servidor principal", "netsh", &["interface", "ipv4", "set", "dnsservers", &n, "static", a, "primary"], Enc::Utf8));
                s.push(cmd("Servidor secundário", "netsh", &["interface", "ipv4", "add", "dnsservers", &n, b, "index=2"], Enc::Utf8));
            }
            s.push(cmd("Conferindo", "netsh", &["interface", "ipv4", "show", "dnsservers", &n], Enc::Utf8));
            (s, simple("DNS alterado. Confira acima.", "Não funcionou"))
        }
        Id::Clean => {
            const KEY: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Explorer\\VolumeCaches";
            let free = || {
                let (mut f, mut t) = (0u64, 0u64);
                unsafe {
                    let _ = windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(windows::core::w!("C:\\"), None, Some(&mut t), Some(&mut f));
                }
                f
            };
            let before = std::sync::Arc::new(std::sync::Mutex::new(0u64));
            let (b1, b2) = (before.clone(), before.clone());
            (
                vec![
                    native("Escolhendo o que limpar", move |l| {
                        *b1.lock().unwrap() = free();
                        for c in tools::CLEAN {
                            reg::write_dword(reg::Hive::Machine, &format!("{KEY}\\{c}"), "StateFlags0099", 2);
                            l.push(format!("  Marcada: {c}"));
                        }
                        true
                    }),
                    cmd("Limpando", "cleanmgr", &["/sagerun:99"], Enc::Oem),
                    native("Conferindo", move |l| {
                        for c in tools::CLEAN {
                            reg::delete(reg::Hive::Machine, &format!("{KEY}\\{c}"), "StateFlags0099");
                        }
                        l.push("  Marcas removidas do registro".to_string());
                        let got = free().saturating_sub(*b2.lock().unwrap());
                        l.push(format!("Liberados {}", fmt_mem(got)));
                        true
                    }),
                ],
                neutral("Limpeza concluída. Windows.old e Lixeira não foram tocados."),
            )
        }
        Id::Winget => (vec![cmd("Procurando", "winget", &["upgrade", "--accept-source-agreements", "--disable-interactivity"], Enc::Utf8)], neutral("Lista acima.")),
        Id::WingetAll => (
            vec![cmd("Atualizando", "winget", &["upgrade", "--all", "--silent", "--accept-package-agreements", "--accept-source-agreements", "--disable-interactivity"], Enc::Utf8)],
            simple("Atualizações concluídas.", "Nem tudo atualizou; veja acima"),
        ),
        Id::Store => (vec![cmd("Analisando", "dism", &["/Online", "/Cleanup-Image", "/AnalyzeComponentStore", "/English"], Enc::Oem)], neutral("Veja a recomendação no fim da saída.")),
        Id::StoreClean => (
            vec![cmd("Pedindo a limpeza", "schtasks", &["/Run", "/TN", "\\Microsoft\\Windows\\Servicing\\StartComponentCleanup"], Enc::Oem)],
            simple("Tarefa iniciada: ela segue em segundo plano por alguns minutos.", "Não iniciou"),
        ),
        Id::Wake => (
            vec![
                cmd("Último despertar", "powercfg", &["/lastwake"], Enc::Oem),
                cmd("Estados disponíveis", "powercfg", &["/a"], Enc::Oem),
                cmd("O que impede o sono", "powercfg", &["/requests"], Enc::Oem),
                cmd("Timers de despertar", "powercfg", &["/waketimers"], Enc::Oem),
            ],
            neutral("Sem administrador, as duas últimas podem falhar."),
        ),
        Id::Drivers => (vec![cmd("Procurando", "pnputil", &["/enum-devices", "/problem"], Enc::Ansi)], neutral("Lista acima.")),
        Id::Support => {
            let dir = crate::config::data_dir().join(format!("suporte_{}", tools::now_stamp().replace([' ', ':'], "-")));
            let _ = std::fs::create_dir_all(&dir);
            let (dx, ms, ev) = (dir.join("dxdiag.txt"), dir.join("sistema.txt"), dir.join("falhas.txt"));
            let shown = dir.display().to_string();
            (
                vec![
                    cmd("Vídeo", "dxdiag", &["/t", &dx.display().to_string()], Enc::Oem),
                    cmd("Sistema", "msinfo32", &["/report", &ms.display().to_string()], Enc::Oem),
                    native("Falhas", move |l| {
                        let t: Vec<String> = events::crashes(90).into_iter().map(|e| format!("{} {} {}", e.time, e.id, e.provider)).collect();
                        l.push(format!("Pasta: {shown}"));
                        std::fs::write(ev, t.join("\n")).is_ok()
                    }),
                ],
                neutral("Pacote salvo na pasta indicada acima."),
            )
        }
        Id::Memory => {
            let _ = std::process::Command::new("mdsched.exe").spawn();
            return None;
        }
        Id::Memresult => (
            vec![native("Lendo", |l| {
                match events::memory_result() {
                    Some(e) => l.push(format!("{} evento {}: {}", e.time.replace('T', " "), e.id, if e.id == 1201 { "sem erros" } else { "veja o Visualizador de Eventos" })),
                    None => l.push("Nenhum resultado registrado. Isso não quer dizer aprovado.".to_string()),
                }
                true
            })],
            neutral("Três estados possíveis: passou, com erros ou sem resultado."),
        ),
        Id::Hiber => {
            let on = reg::dword(reg::Hive::Machine, "SYSTEM\\CurrentControlSet\\Control\\Power", "HibernateEnabled") == Some(1);
            app.log(if on { "desligou a hibernação" } else { "ligou a hibernação" });
            (vec![cmd("Alterando", "powercfg", &["/h", if on { "off" } else { "on" }], Enc::Oem)], simple(if on { "Hibernação desligada." } else { "Hibernação ligada." }, "Não funcionou"))
        }
        Id::Recovery => (vec![cmd("Lendo", "reagentc", &["/info"], Enc::Oem)], neutral("Veja o estado acima.")),
        Id::Advanced => (vec![cmd("Reiniciando", "shutdown", &["/r", "/o", "/t", "0"], Enc::Oem)], neutral("Reiniciando.")),
        Id::Firmware => (vec![cmd("Reiniciando", "shutdown", &["/r", "/fw", "/t", "0"], Enc::Oem)], neutral("Reiniciando.")),
        Id::Panels => return None,
    })
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header(ui, a, "Ferramentas", "O que o Windows esconde em linha de comando, com resultado em português.");
    let x = a.x + 28.0;
    let top = 88.0;
    let h = a.h - top - 20.0;
    let lw = 290.0;
    let l = rect(x, top, lw, h);
    ui.card(l);
    let list = l.inset(8.0);
    let mut rows: Vec<(Option<&str>, usize)> = Vec::new();
    let mut last = "";
    for (i, t) in LIST.iter().enumerate() {
        if t.group != last {
            rows.push((Some(t.group), i));
            last = t.group;
        }
        rows.push((None, i));
    }
    ui.begin_scroll("tl", list, rows.len() as f32 * 34.0);
    for (row, (g, i)) in rows.iter().enumerate() {
        let y = list.y + row as f32 * 34.0;
        match g {
            Some(g) => ui.c.text(rect(list.x + 10.0, y + 8.0, list.w - 20.0, 24.0), g, 12.0, MUTED, Align::Left, Weight::Semibold),
            None => {
                let r = rect(list.x, y, list.w, 32.0);
                if app.tool_sel == *i {
                    ui.c.round(r, 6.0, CARD2);
                } else if ui.is_hover(r) {
                    ui.c.round(r, 6.0, CARD2.alpha(0.5));
                }
                ui.c.text(r.inset_xy(10.0, 0.0), LIST[*i].title, 14.0, TEXT, Align::Left, Weight::Regular);
                if ui.hit(&format!("t{i}"), r) {
                    app.tool_sel = *i;
                    app.tool_ack = false;
                    app.tool_mode = 0;
                }
            }
        }
    }
    ui.end_scroll();

    let it = &LIST[app.tool_sel.min(LIST.len() - 1)];
    let r = rect(l.right() + 12.0, top, a.w - 56.0 - lw - 12.0, h);
    ui.card(r);
    let px = r.x + 16.0;
    let pw = r.w - 32.0;
    ui.title(rect(px, r.y + 12.0, pw, 22.0), it.title);
    ui.c.text_wrap(rect(px, r.y + 38.0, pw, 42.0), it.desc, 13.0, TEXT2, Weight::Regular);
    let mut y = r.y + 88.0;

    let letters: Vec<char> = crate::sys::mft::drives();
    let needs_drive = matches!(it.id, Id::Chkdsk | Id::Protect | Id::Retrim);
    let mut blocked: Option<String> = None;
    let mut select_at: Option<Rect> = None;
    let letter = letters.get(app.tool_drive).copied().unwrap_or('C');
    let suspect = app.suspect_letters().contains(&letter);
    let c_suspect = app.suspect_letters().contains(&'C');

    if needs_drive {
        select_at = Some(rect(px, y, 200.0, 44.0));
        if it.id == Id::Chkdsk {
            y += 0.0;
        }
        y += 54.0;
        if suspect {
            ui.c.text_wrap(rect(px, y, pw, 40.0), "Este disco está suspeito: copie os dados antes de qualquer reparo. Modos que escrevem podem piorar a perda.", 13.0, WARN, Weight::Semibold);
            y += 44.0;
        }
    }
    if it.id == Id::Repair && c_suspect {
        ui.c.text(rect(px, y, pw, 22.0), "O disco do sistema está suspeito: resolva isso antes de reparar o Windows.", 13.0, WARN, Align::Left, Weight::Semibold);
        y += 28.0;
    }
    if it.id == Id::Retrim && !app.disks.iter().any(|d| d.ssd && d.volumes.iter().any(|v| v.letter == letter)) {
        blocked = Some("TRIM é só para SSD.".to_string());
    }
    if it.id == Id::Retrim && suspect {
        blocked = Some("Bloqueado em disco suspeito.".to_string());
    }
    if it.id == Id::Chkdsk && suspect && app.tool_mode >= 1 {
        let mut ack = app.tool_ack;
        ui.check("ack", rect(px, y, pw, 40.0), "Já copiei os dados e aceito o risco", &mut ack);
        app.tool_ack = ack;
        y += 44.0;
        if app.tool_mode == 3 {
            blocked = Some("O modo /r lê toda a superfície por muitas horas: bloqueado em disco suspeito.".to_string());
        } else if !ack {
            blocked = Some("Confirme acima para liberar.".to_string());
        }
    }
    if it.id == Id::Copy {
        ui.label(rect(px, y, 300.0, 18.0), "De onde", TEXT2);
        let s = rect(px, y + 20.0, pw, 40.0);
        ui.c.round(s, 8.0, SIDE);
        ui.c.round_line(s, 8.0, BORDER, 1.0);
        let shown = if app.tool_src.is_empty() { "Escolher pasta" } else { app.tool_src.as_str() };
        ui.c.text(s.inset_xy(10.0, 0.0), shown, 14.0, TEXT, Align::Left, Weight::Regular);
        if ui.hit("csrc", s) {
            app.pick_tool_folder(2);
        }
        y += 68.0;
        ui.label(rect(px, y, 300.0, 18.0), "Para onde (outro disco)", TEXT2);
        let d = rect(px, y + 20.0, pw, 40.0);
        ui.c.round(d, 8.0, SIDE);
        ui.c.round_line(d, 8.0, BORDER, 1.0);
        let shown = if app.tool_dst.is_empty() { "Escolher pasta" } else { app.tool_dst.as_str() };
        ui.c.text(d.inset_xy(10.0, 0.0), shown, 14.0, TEXT, Align::Left, Weight::Regular);
        if ui.hit("cdst", d) {
            app.pick_tool_folder(3);
        }
        y += 72.0;
        if app.tool_src.is_empty() || app.tool_dst.is_empty() {
            blocked = Some("Escolha origem e destino.".to_string());
        } else if app.tool_src.chars().next().map(|c| c.to_ascii_uppercase()) == app.tool_dst.chars().next().map(|c| c.to_ascii_uppercase()) {
            blocked = Some("O destino precisa estar em outro disco.".to_string());
        }
    }
    if it.id == Id::Panels {
        let bw = 150.0;
        for (i, (name, exe, arg)) in PANELS.iter().enumerate() {
            let c = (i % 3) as f32;
            let row = (i / 3) as f32;
            if ui.button(name, rect(px + c * (bw + 8.0), y + row * 52.0, bw, 44.0), name, false) {
                let mut cmd = std::process::Command::new(if exe.ends_with(".msc") { "mmc" } else { exe });
                if exe.ends_with(".msc") {
                    cmd.arg(exe);
                }
                if !arg.is_empty() {
                    cmd.arg(arg);
                }
                let _ = cmd.spawn();
            }
        }
        return;
    }
    let mut dns_at = None;
    if it.id == Id::Dns {
        dns_at = Some(rect(px, y, 240.0, 44.0));
    }

    let sel = app.tool_sel.min(LIST.len() - 1);
    let shared = app.tool_log(sel);
    let run = shared.lock().unwrap().clone();
    let busy = app.tool_busy();
    let need_admin = elevate_needed(it.id) && !app.elevated;
    let bw = 210.0;
    let by = y + if it.id == Id::Dns { 54.0 } else { 0.0 } + if it.id == Id::Chkdsk { 0.0 } else { 0.0 };
    let label = if run.running {
        "Rodando…".to_string()
    } else if need_admin {
        "Pede administrador".to_string()
    } else {
        "Executar".to_string()
    };
    if busy && !run.running && blocked.is_none() {
        blocked = Some("Outra ferramenta está rodando. Espere terminar.".to_string());
    }
    let ok = !busy && !need_admin && blocked.is_none();
    let mut go = false;
    if ui.button_ex("go", rect(px, by, bw, 44.0), &label, true, ok) {
        if it.writes {
            ui.ask("tool-go", "Executar esta ferramenta?", &format!("{}. {}", it.title, it.desc));
        } else {
            go = true;
        }
    }
    go |= ui.answered("tool-go");
    if go {
        if let Some((steps, verdict)) = plan(it.id, app, letter) {
            tools::start(steps, verdict, shared.clone());
        }
    }
    if let Some(b) = &blocked {
        ui.c.text(rect(px + bw + 12.0, by, pw - bw - 12.0, 44.0), b, 13.0, WARN, Align::Left, Weight::Regular);
    }

    let oy = by + 56.0;
    let out = rect(px, oy, pw, r.bottom() - oy - 14.0);
    ui.c.round(out, 8.0, SIDE);
    let mut body = out.inset(10.0);
    // O resultado fica no fim: os textos dele apontam para a saída acima.
    if let Some((lv, msg)) = &run.verdict {
        let color = match lv {
            Level::Good => ACCENT,
            Level::Warn => WARN,
            Level::Risk => REC,
            Level::Unknown => TEXT2,
        };
        ui.c.text_wrap(rect(body.x, body.bottom() - 40.0, body.w, 40.0), msg, 14.0, color, Weight::Semibold);
        body = rect(body.x, body.y, body.w, body.h - 46.0);
    } else if run.running {
        ui.c.text(rect(body.x, body.bottom() - 22.0, body.w, 22.0), &run.step, 14.0, ACCENT, Align::Left, Weight::Semibold);
        body = rect(body.x, body.y, body.w, body.h - 28.0);
    }
    if run.lines.is_empty() {
        ui.c.text(rect(body.x, body.y, body.w, 22.0), "O passo a passo aparece aqui.", 13.0, MUTED, Align::Left, Weight::Regular);
    } else {
        log_view(ui, &format!("tool-log{sel}"), body, &run.lines);
    }

    // Listas por último, para abrirem por cima do resto.
    if let Some(s) = dns_at {
        let opts: Vec<String> = DNS.iter().map(|d| d.0.to_string()).collect();
        let mut v = app.tool_dns;
        if ui.select("dns", s, &opts, &mut v) {
            app.tool_dns = v;
        }
        let nets: Vec<String> = app.nets.iter().filter(|n| n.up).map(|n| n.name.clone()).collect();
        let mut v = app.tool_net.min(nets.len().saturating_sub(1));
        if ui.select("dnet", rect(s.right() + 8.0, s.y, 240.0, 44.0), &nets, &mut v) {
            app.tool_net = v;
        }
    }
    if let Some(s) = select_at {
        if it.id == Id::Chkdsk {
            let opts: Vec<String> = CHK_MODES.iter().map(|m| m.to_string()).collect();
            let mut v = app.tool_mode;
            if ui.select("mode", rect(s.right() + 8.0, s.y, 300.0, 44.0), &opts, &mut v) {
                app.tool_mode = v;
                app.tool_ack = false;
            }
        }
        let opts: Vec<String> = letters.iter().map(|d| format!("Disco {d}:")).collect();
        let mut v = app.tool_drive;
        if ui.select("drv", s, &opts, &mut v) {
            app.tool_drive = v;
            app.tool_ack = false;
        }
    }
}
