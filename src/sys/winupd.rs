//! Windows Update pela API do próprio Windows (WUA): listar as pendentes roda como usuário comum;
//! esconder, mostrar, instalar e a política "avisar antes de baixar" rodam numa cópia de
//! administrador (`--wu=`), com o pedido do UAC a cada ação.

use super::reg::{delete, dword, write_dword, Hive};
use serde::{Deserialize, Serialize};
use windows::core::BSTR;
use windows::Win32::Foundation::VARIANT_BOOL;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::System::UpdateAgent::*;

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
pub struct WinUpd {
    pub id: String,
    pub title: String,
    pub kb: String,
    pub category: String,
    /// "Critical", "Important"... (vazio fora das de segurança).
    pub severity: String,
    pub hidden: bool,
    /// Definições do Defender: saem várias vezes por dia e não contam como novidade.
    pub definition: bool,
}

const DEFINITIONS: &str = "e0789628-ce08-4437-be74-2495b842f43b";
const ALL: &str = "IsInstalled=0 and IsHidden=0 or IsInstalled=0 and IsHidden=1";

fn session() -> windows::core::Result<IUpdateSession> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let s: IUpdateSession = CoCreateInstance(&UpdateSession, None, CLSCTX_INPROC_SERVER)?;
        let _ = s.SetClientApplicationID(&BSTR::from("Prisma"));
        Ok(s)
    }
}

fn search(s: &IUpdateSession) -> windows::core::Result<IUpdateCollection> {
    unsafe { s.CreateUpdateSearcher()?.Search(&BSTR::from(ALL))?.Updates() }
}

fn info(u: &IUpdate) -> windows::core::Result<WinUpd> {
    unsafe {
        let kbs = u.KBArticleIDs()?;
        let kb = if kbs.Count()? > 0 { format!("KB{}", kbs.get_Item(0)?) } else { String::new() };
        let cats = u.Categories()?;
        let (mut category, mut definition) = (String::new(), false);
        for i in 0..cats.Count()? {
            let c = cats.get_Item(i)?;
            if category.is_empty() {
                category = c.Name()?.to_string();
            }
            definition |= c.CategoryID()?.to_string().eq_ignore_ascii_case(DEFINITIONS);
        }
        Ok(WinUpd {
            id: u.Identity()?.UpdateID()?.to_string(),
            title: u.Title()?.to_string(),
            kb,
            category,
            severity: u.MsrcSeverity().map(|s| s.to_string()).unwrap_or_default(),
            hidden: u.IsHidden()?.as_bool(),
            definition,
        })
    }
}

/// As atualizações que faltam instalar, as escondidas também. Demora (o Windows consulta o servidor).
pub fn pending() -> Result<Vec<WinUpd>, String> {
    let list = (|| -> windows::core::Result<Vec<WinUpd>> {
        let all = search(&session()?)?;
        let mut out = Vec::new();
        for i in 0..unsafe { all.Count()? } {
            out.push(info(&unsafe { all.get_Item(i)? })?);
        }
        Ok(out)
    })();
    list.map_err(|e| format!("{:#010x}", e.code().0))
}

/// As atualizações de `ids` na busca de agora.
fn pick(s: &IUpdateSession, ids: &[&str]) -> windows::core::Result<Vec<IUpdate>> {
    let all = search(s)?;
    let mut out = Vec::new();
    for i in 0..unsafe { all.Count()? } {
        let u = unsafe { all.get_Item(i)? };
        let id = unsafe { u.Identity()?.UpdateID()? }.to_string();
        if ids.iter().any(|x| x.eq_ignore_ascii_case(&id)) {
            out.push(u);
        }
    }
    Ok(out)
}

fn set_hidden(ids: &[&str], hide: bool) -> windows::core::Result<u32> {
    for u in pick(&session()?, ids)? {
        unsafe { u.SetIsHidden(VARIANT_BOOL::from(hide))? };
    }
    Ok(0)
}

/// Baixa e instala. 0 instalou, 2 instalou e pede reiniciar, 3 falhou.
fn install(ids: &[&str]) -> windows::core::Result<u32> {
    let s = session()?;
    let ups = pick(&s, ids)?;
    if ups.is_empty() {
        return Ok(3);
    }
    unsafe {
        let col: IUpdateCollection = CoCreateInstance(&UpdateCollection, None, CLSCTX_INPROC_SERVER)?;
        for u in &ups {
            if !u.EulaAccepted()?.as_bool() {
                u.AcceptEula()?;
            }
            col.Add(u)?;
        }
        let d = s.CreateUpdateDownloader()?;
        d.SetUpdates(&col)?;
        d.Download()?;
        let inst = s.CreateUpdateInstaller()?;
        inst.SetUpdates(&col)?;
        let r = inst.Install()?;
        let ok = matches!(r.ResultCode()?, x if x == orcSucceeded || x == orcSucceededWithErrors);
        Ok(match (ok, r.RebootRequired()?.as_bool()) {
            (false, _) => 3,
            (true, true) => 2,
            (true, false) => 0,
        })
    }
}

const WU: &str = r"SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate";
const AU: &str = r"SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU";

/// "Avisar antes de baixar" ligado, com os drivers fora das atualizações de qualidade.
pub fn policy_on() -> bool {
    dword(Hive::Machine, AU, "AUOptions") == Some(2) && dword(Hive::Machine, WU, "ExcludeWUDriversInQualityUpdate") == Some(1)
}

/// Nunca mexe no serviço `wuauserv`: o Windows o religa e os Ajustes acusam como problema.
fn set_policy(on: bool) -> bool {
    if on {
        write_dword(Hive::Machine, AU, "NoAutoUpdate", 0) && write_dword(Hive::Machine, AU, "AUOptions", 2) && write_dword(Hive::Machine, WU, "ExcludeWUDriversInQualityUpdate", 1)
    } else {
        // delete devolve false quando o valor já não existe: o que importa é ele não estar lá.
        for (k, n) in [(AU, "NoAutoUpdate"), (AU, "AUOptions"), (WU, "ExcludeWUDriversInQualityUpdate")] {
            delete(Hive::Machine, k, n);
        }
        !policy_on()
    }
}

/// Abre a cópia de administrador e espera. `op`: `hide:<ids>`, `show:<ids>`, `install:<ids>` ou `policy:1|0`.
/// None quando o pedido do UAC foi recusado; 259 quando passou de 10 minutos.
pub fn run_elevated(op: &str) -> Option<u32> {
    crate::startup::run_elevated_wait(&format!("--wu={op}"))
}

/// Cópia de administrador: lê `--wu=` e sai.
pub fn run_arg() -> Option<i32> {
    let op = std::env::args().find_map(|a| a.strip_prefix("--wu=").map(str::to_string))?;
    let (cmd, rest) = op.split_once(':').unwrap_or((&op, ""));
    let ids: Vec<&str> = rest.split(',').filter(|s| !s.is_empty()).collect();
    let r = match cmd {
        "hide" => set_hidden(&ids, true),
        "show" => set_hidden(&ids, false),
        "install" => install(&ids),
        "policy" => Ok(if set_policy(rest == "1") { 0 } else { 3 }),
        _ => Ok(3),
    };
    let code = r.unwrap_or_else(|e| {
        crate::config::log(&format!("Windows Update ({cmd}): falhou {:#010x}", e.code().0));
        3
    });
    crate::config::log(&format!("Windows Update ({cmd}, {} itens): {code}", ids.len()));
    Some(code as i32)
}
