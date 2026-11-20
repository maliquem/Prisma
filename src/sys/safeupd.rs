//! Atualizações seguras: o que saiu do driver da NVIDIA e do Windows Update, e se vale instalar.
//!
//! Fontes: o endpoint de busca do site da NVIDIA, as "Open Issues" das notas em PDF, os "known
//! issues" da Microsoft (release health) e, com a chave da pessoa, o Gemini com a busca do Google,
//! que lê os relatos (megathreads do r/nvidia e do r/Windows11, fórum da NVIDIA, Guru3D...).
//! Uma novidade é avisada logo ("saiu, aguardando relatos") e só ganha o veredito depois da espera.

use super::winupd::WinUpd;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
pub struct Driver {
    pub version: String,
    pub date: String,
    pub url: String,
    /// Notas da versão em PDF (com as "Open Issues").
    pub notes: String,
    pub details: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Default, Debug, PartialEq)]
pub enum Sev {
    High,
    #[default]
    Medium,
    Low,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
pub struct Bug {
    pub text: String,
    pub sev: Sev,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
pub struct Verdict {
    pub bugs: Vec<Bug>,
    /// Estimativa da IA, a partir das reclamações: não existe essa estatística pronta.
    pub ok_pct: u8,
    pub recommend: bool,
    pub sources: Vec<String>,
}

/// Uma novidade acompanhada: aparece, espera os relatos e ganha o veredito.
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct Watch {
    /// Versão do driver, ou os KBs pendentes do Windows separados por vírgula.
    pub name: String,
    /// Quando o Prisma viu a novidade (segundos Unix).
    pub seen: u64,
    /// Problemas que a própria empresa admite.
    pub official: Vec<String>,
    pub verdict: Option<Verdict>,
    /// Quando o veredito rodou; 0 enquanto espera (ou quando a pesquisa falhou e vai tentar de novo).
    pub judged: u64,
    pub error: String,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct State {
    pub checked: u64,
    /// Driver instalado, "621.24"; vazio sem placa NVIDIA.
    pub installed: String,
    pub driver: Option<Driver>,
    pub nv_error: bool,
    pub nv: Option<Watch>,
    pub updates: Vec<WinUpd>,
    pub win_error: String,
    pub win: Option<Watch>,
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn path() -> std::path::PathBuf {
    crate::config::data_dir().join("atualizacoes.json")
}

pub fn load() -> State {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save(st: &State) {
    let _ = std::fs::create_dir_all(crate::config::data_dir());
    if let Ok(b) = serde_json::to_vec(st) {
        let _ = std::fs::write(path(), b);
    }
}

/// Há conferência a fazer: passou o intervalo, ou acabou a espera de uma novidade sem veredito.
/// `every_days` 0 é desligado.
pub fn due(st: &State, every_days: u32, wait_h: u32, now: u64) -> bool {
    if every_days == 0 {
        return false;
    }
    now >= st.checked + every_days as u64 * 86400 || [&st.nv, &st.win].into_iter().flatten().any(|w| waiting_done(w, wait_h, now))
}

/// A espera acabou e o veredito ainda não saiu. Uma pesquisa que falhou espera uma hora antes de tentar de novo.
fn waiting_done(w: &Watch, wait_h: u32, now: u64) -> bool {
    w.judged == 0 && now >= w.seen + wait_h as u64 * 3600 && (w.error.is_empty() || now >= w.seen.max(retry_after(w)))
}

fn retry_after(w: &Watch) -> u64 {
    w.error.split_once('@').and_then(|(_, t)| t.parse::<u64>().ok()).map_or(0, |t| t + 3600)
}

/// Quando sai o veredito, para a tela.
pub fn verdict_at(w: &Watch, wait_h: u32) -> u64 {
    w.seen + wait_h as u64 * 3600
}

/// "621.24" a partir dos centésimos da NvAPI.
pub fn version_text(v: u32) -> String {
    format!("{}.{:02}", v / 100, v % 100)
}

/// `a` é mais nova que `b` ("621.24" > "581.97").
pub fn newer(a: &str, b: &str) -> bool {
    let n = |s: &str| -> Vec<u32> { s.split('.').map(|p| p.trim().parse().unwrap_or(0)).collect() };
    n(a) > n(b)
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn field(v: &Value, k: &str) -> String {
    match &v[k] {
        Value::String(s) => url_decode(s),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// O driver Game Ready mais novo da resposta da NVIDIA (a lista mistura com o Studio, `IsCRD` 1).
pub fn parse_nv(json: &str) -> Option<Driver> {
    let v: Value = serde_json::from_str(json).ok()?;
    let ids = v["IDS"].as_array()?;
    let infos: Vec<&Value> = ids.iter().map(|i| &i["downloadInfo"]).filter(|d| !field(d, "Version").is_empty()).collect();
    let d = infos.iter().find(|d| field(d, "IsCRD") != "1").or(infos.first())?;
    let other = field(d, "OtherNotes");
    let notes = other
        .split("href=\"")
        .skip(1)
        .filter_map(|s| s.split('"').next())
        .find(|h| h.ends_with("release-notes.pdf"))
        .unwrap_or_default()
        .to_string();
    Some(Driver { version: field(d, "Version"), date: field(d, "ReleaseDateTime"), url: field(d, "DownloadURL"), notes, details: field(d, "DetailsURL") })
}

/// O código da placa na busca da NVIDIA, pelo nome do DXGI ("NVIDIA GeForce RTX 4070").
pub fn pfid(gpus_json: &str, name: &str) -> Option<String> {
    let v: Value = serde_json::from_str(gpus_json).ok()?;
    let want = name.trim().strip_prefix("NVIDIA ").unwrap_or(name.trim()).to_lowercase();
    ["desktop", "notebook"].iter().filter_map(|k| v[*k].as_object()).flatten().find(|(k, _)| k.to_lowercase() == want).and_then(|(_, id)| id.as_str().map(str::to_string))
}

/// Windows 11 a partir do build 22000; os códigos vêm do os-data.json do nvidia-data.
/// shortcut: só Windows 10 e 11 de 64 bits, que são os que o Prisma roda.
fn nv_os() -> &'static str {
    if win_build() >= 22000 { "135" } else { "57" }
}

fn win_build() -> u32 {
    use super::reg::{text, Hive};
    text(Hive::Machine, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "CurrentBuildNumber").and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

fn http_text(url: &str) -> Option<String> {
    super::update::download(url).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Lista das placas com o pfid, guardada para não baixar sempre. Volta a baixar quando a placa não está nela.
fn gpus(name: &str) -> Option<String> {
    let cache = crate::config::data_dir().join("nvidia-gpus.json");
    if let Some(id) = std::fs::read_to_string(&cache).ok().and_then(|j| pfid(&j, name)) {
        return Some(id);
    }
    let j = http_text("https://raw.githubusercontent.com/ZenitH-AT/nvidia-data/main/gpu-data.json")?;
    let id = pfid(&j, name);
    if id.is_some() {
        let _ = std::fs::write(&cache, &j);
    }
    id
}

fn nvidia_name() -> Option<String> {
    crate::capture::adapters().into_iter().map(|(n, _)| n).find(|n| n.to_uppercase().contains("NVIDIA"))
}

fn nv_latest() -> Option<Driver> {
    let id = gpus(&nvidia_name()?)?;
    let url = format!(
        "https://gfwsl.geforce.com/services_toolkit/services/com/nvidia/services/AjaxDriverService.php?func=DriverManualLookup&pfid={id}&osID={}&dch=1&numberOfResults=10",
        nv_os()
    );
    parse_nv(&http_text(&url)?)
}

/// Texto de um PDF simples (as notas da NVIDIA): os blocos comprimidos com texto, linha por linha.
pub fn pdf_text(pdf: &[u8]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut pos = 0;
    while let Some(i) = find(&pdf[pos..], b"stream") {
        let mut s = pos + i + 6;
        pos = s;
        if pdf.get(s) == Some(&b'\r') {
            s += 1;
        }
        if pdf.get(s) != Some(&b'\n') {
            continue;
        }
        s += 1;
        let Some(e) = find(&pdf[s..], b"endstream") else { break };
        pos = s + e + 9;
        let Ok(t) = miniz_oxide::inflate::decompress_to_vec_zlib(&pdf[s..s + e]) else { continue };
        if find(&t, b"BT").is_some() {
            text_ops(&t, &mut out);
        }
    }
    out.join("\n")
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

/// Cp1252 das notas: aspas e travessões curvos viram os simples, o resto é latin1.
fn pdf_char(b: u8) -> char {
    match b {
        0x91 | 0x92 => '\'',
        0x93 | 0x94 => '"',
        0x96 | 0x97 => '-',
        _ => b as char,
    }
}

/// Junta as strings dos operadores de texto. Td/TD com deslocamento vertical, Tm em outra altura,
/// T* e ET quebram a linha; Td na mesma linha vira espaço.
fn text_ops(t: &[u8], out: &mut Vec<String>) {
    let mut line = String::new();
    let mut nums: Vec<f32> = Vec::new();
    let mut y: Option<f32> = None;
    let mut i = 0;
    let push = |line: &mut String, out: &mut Vec<String>| {
        if !line.trim().is_empty() {
            out.push(line.trim().to_string());
        }
        line.clear();
    };
    while i < t.len() {
        let c = t[i];
        if c == b'(' {
            i += 1;
            let mut depth = 0;
            while i < t.len() {
                match t[i] {
                    b'\\' if i + 1 < t.len() => {
                        i += 1;
                        let e = t[i];
                        if e.is_ascii_digit() {
                            let mut v = 0u32;
                            let mut k = 0;
                            while k < 3 && i < t.len() && (b'0'..=b'7').contains(&t[i]) {
                                v = v * 8 + (t[i] - b'0') as u32;
                                i += 1;
                                k += 1;
                            }
                            line.push(pdf_char(v as u8));
                            continue;
                        }
                        line.push(match e {
                            b'n' | b'r' | b't' => ' ',
                            x => pdf_char(x),
                        });
                    }
                    b'(' => {
                        depth += 1;
                        line.push('(');
                    }
                    b')' if depth > 0 => {
                        depth -= 1;
                        line.push(')');
                    }
                    b')' => break,
                    x => line.push(pdf_char(x)),
                }
                i += 1;
            }
            i += 1;
            nums.clear();
        } else if c == b'-' || c == b'.' || c.is_ascii_digit() {
            let s = i;
            i += 1;
            while i < t.len() && (t[i] == b'.' || t[i].is_ascii_digit()) {
                i += 1;
            }
            nums.push(std::str::from_utf8(&t[s..i]).ok().and_then(|x| x.parse().ok()).unwrap_or(0.0));
        } else if c.is_ascii_alphabetic() || c == b'*' {
            let s = i;
            i += 1;
            while i < t.len() && (t[i].is_ascii_alphabetic() || t[i] == b'*') {
                i += 1;
            }
            let op = &t[s..i];
            let brk = match op {
                b"Td" | b"TD" if nums.len() >= 2 => nums[nums.len() - 1] != 0.0,
                b"Tm" if nums.len() >= 6 => {
                    let ny = nums[nums.len() - 1];
                    let b = y != Some(ny);
                    y = Some(ny);
                    b
                }
                b"T*" | b"ET" => true,
                _ => false,
            };
            if brk {
                push(&mut line, out);
            } else if matches!(op, b"Td" | b"TD") && !line.is_empty() && !line.ends_with(' ') {
                line.push(' ');
            }
            nums.clear();
        } else {
            i += 1;
        }
    }
    push(&mut line, out);
}

/// Os itens de "Open Issues in Version" das notas (a última ocorrência: a primeira é o índice).
pub fn open_issues(text: &str) -> Vec<String> {
    let lines: Vec<String> = text.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect();
    let squash = |l: &str| l.replace(' ', "").to_lowercase();
    let Some(start) = lines.iter().rposition(|l| squash(l).contains("openissuesinversion")) else { return Vec::new() };
    let mut items: Vec<String> = Vec::new();
    for l in &lines[start + 1..] {
        if squash(l).contains("issuesnotcaused") || squash(l).contains("openissuesinversion") {
            break;
        }
        if l.starts_with("RN -") || l.starts_with("Release") || l.contains("Driver for Windows, Version") || l.is_empty() {
            continue;
        }
        if let Some(item) = l.strip_prefix("> ").or_else(|| l.strip_prefix('>')) {
            items.push(item.trim().to_string());
        } else if let Some(last) = items.last_mut() {
            last.push(' ');
            last.push_str(l);
        }
    }
    items.retain(|i| i != "N/A" && !i.is_empty());
    items
}

fn nv_official(d: &Driver) -> Result<Vec<String>, ()> {
    if d.notes.is_empty() {
        return Err(());
    }
    super::update::download(&d.notes).map(|pdf| open_issues(&pdf_text(&pdf)))
}

/// Página "release health" da versão instalada do Windows.
fn release_health_url() -> String {
    use super::reg::{text, Hive};
    let dv = text(Hive::Machine, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "DisplayVersion").unwrap_or_default().to_lowercase();
    let os = if win_build() >= 22000 { 11 } else { 10 };
    format!("https://learn.microsoft.com/en-us/windows/release-health/status-windows-{os}-{}", dv.trim())
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' => {
                tag = false;
                out.push(' ');
            }
            _ if !tag => out.push(c),
            _ => {}
        }
    }
    let out = out.replace("&amp;", "&").replace("&quot;", "\"").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&nbsp;", " ");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Os "known issues" da página: "título (KB, situação)".
pub fn parse_known(html: &str) -> Vec<String> {
    let Some(s) = html.find("id=\"known-issues\"") else { return Vec::new() };
    let body = &html[s..];
    let body = &body[..body.find("</table>").unwrap_or(body.len())];
    let mut out = Vec::new();
    for row in body.split("<tr").skip(1) {
        let cells: Vec<&str> = row.split("<td").skip(1).collect();
        if cells.len() < 3 {
            continue;
        }
        let title = cells[0].split("<b>").nth(1).and_then(|t| t.split("</b>").next()).map(strip_tags).unwrap_or_default();
        let kb = strip_tags(cells[1].split_once('>').map_or("", |x| x.1)).split_whitespace().find(|w| w.starts_with("KB")).unwrap_or_default().to_string();
        let status = strip_tags(cells[2].split_once('>').map_or("", |x| x.1));
        if !title.is_empty() {
            out.push(format!("{title} ({})", [kb, status].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(", ")));
        }
    }
    out
}

fn win_official() -> Result<Vec<String>, ()> {
    http_text(&release_health_url()).map(|h| parse_known(&h)).ok_or(())
}

/// O idioma da tela, para a IA escrever os bugs nele.
fn lang_name() -> &'static str {
    use crate::i18n::*;
    match lang() {
        PT => "Brazilian Portuguese",
        ES => "Spanish",
        RU => "Russian",
        ZH => "Simplified Chinese",
        _ => "English",
    }
}

const ANSWER: &str = "Answer ONLY with one JSON object, no other text: {\"bugs\":[{\"text\":\"short description\",\"severity\":\"high|medium|low\"}],\"ok_percent\":0-100,\"recommend\":true|false,\"sources\":[\"url\", ...]}. ok_percent is your estimate of the share of users without problems, from how many complain. recommend is whether a typical gamer should install it now. Only list bugs that users actually report for this release, most serious first, at most 8.";

pub fn nv_prompt(version: &str, installed: &str, official: &[String]) -> String {
    format!(
        "You are checking if NVIDIA GeForce Game Ready driver {version} for Windows is safe to install (the user has {installed}). Search the web for user reports about {version}: first the r/nvidia megathread titled \"Game Ready Driver {version} FAQ/Discussion\", then the NVIDIA forums driver feedback thread, Guru3D, Overclockers and other recent discussions. NVIDIA lists these open issues: {}. Write bug texts in {}. {ANSWER}",
        if official.is_empty() { "none".into() } else { official.join("; ") },
        lang_name()
    )
}

pub fn win_prompt(updates: &[&WinUpd], known: &[String]) -> String {
    let list: Vec<String> = updates.iter().map(|u| u.title.clone()).collect();
    format!(
        "You are checking if these pending Windows updates are safe to install on a gaming PC ({}): {}. Search the web for user reports about them: first the r/Windows11 (or r/Windows10) megathread titled \"Cumulative Updates <release date>\" for this month, then Windows Latest, BleepingComputer, Neowin and other recent discussions. Microsoft lists these known issues: {}. Write bug texts in {}. {ANSWER}",
        if win_build() >= 22000 { "Windows 11" } else { "Windows 10" },
        list.join("; "),
        if known.is_empty() { "none".into() } else { known.join("; ") },
        lang_name()
    )
}

/// Busca no Reddit para abrir no navegador (o Reddit não deixa ler sem login).
pub fn reddit_nv(version: &str) -> String {
    format!("https://www.reddit.com/r/nvidia/search/?q=Game+Ready+Driver+{version}&restrict_sr=1&sort=new")
}

pub fn reddit_win() -> String {
    let sub = if win_build() >= 22000 { "Windows11" } else { "Windows10" };
    format!("https://www.reddit.com/r/{sub}/search/?q=Cumulative+Updates&restrict_sr=1&sort=new")
}

/// Modelos em ordem: o 2.5 Flash e o Flash-Lite têm a busca grátis (500 por dia); o "latest" é o
/// substituto quando eles saírem do ar.
const MODELS: [&str; 3] = ["gemini-2.5-flash", "gemini-2.5-flash-lite", "gemini-flash-latest"];

/// Pergunta ao Gemini com a busca do Google. A chave vai só no cabeçalho, nunca no log.
pub fn gemini(key: &str, prompt: &str) -> Result<Verdict, String> {
    let body = serde_json::json!({"contents":[{"parts":[{"text":prompt}]}],"tools":[{"google_search":{}}]}).to_string();
    let headers = format!("x-goog-api-key: {}\r\nContent-Type: application/json\r\n", key.trim());
    let mut last = String::new();
    for m in MODELS {
        let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{m}:generateContent");
        let Ok((status, text)) = super::update::post(&url, &headers, &body) else { return Err("rede".into()) };
        match status {
            200 => return parse_reply(&text).ok_or_else(|| "resposta".into()),
            404 => last = "modelo".into(),
            429 => return Err("limite".into()),
            400 | 401 | 403 => return Err("chave".into()),
            s => return Err(format!("http {s}")),
        }
    }
    Err(last)
}

fn sev(s: &str) -> Sev {
    match s.to_lowercase().as_str() {
        "high" | "grave" | "critical" | "severe" => Sev::High,
        "low" | "baixo" | "minor" => Sev::Low,
        _ => Sev::Medium,
    }
}

/// O veredito na resposta do Gemini: o JSON pedido no texto (às vezes entre crases), e as páginas da
/// busca quando ele não cita fontes.
pub fn parse_reply(json: &str) -> Option<Verdict> {
    let v: Value = serde_json::from_str(json).ok()?;
    let cand = &v["candidates"][0];
    let text: String = cand["content"]["parts"].as_array()?.iter().filter_map(|p| p["text"].as_str()).collect();
    let (a, b) = (text.find('{')?, text.rfind('}')?);
    let r: Value = serde_json::from_str(text.get(a..=b)?).ok()?;
    let bugs = r["bugs"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|b| {
                    let t = b["text"].as_str().or(b.as_str())?.trim();
                    (!t.is_empty()).then(|| Bug { text: t.to_string(), sev: sev(b["severity"].as_str().unwrap_or("")) })
                })
                .collect()
        })
        .unwrap_or_default();
    let ok_pct = r["ok_percent"].as_f64().or_else(|| r["ok_percent"].as_str().and_then(|s| s.trim_end_matches('%').parse().ok()))?.clamp(0.0, 100.0).round() as u8;
    let recommend = r["recommend"].as_bool().unwrap_or(false);
    let mut sources: Vec<String> = r["sources"].as_array().map(|a| a.iter().filter_map(|s| s.as_str()).filter(|s| s.starts_with("http")).map(str::to_string).collect()).unwrap_or_default();
    if sources.is_empty() {
        sources = cand["groundingMetadata"]["groundingChunks"].as_array().map(|a| a.iter().filter_map(|c| c["web"]["uri"].as_str()).map(str::to_string).collect()).unwrap_or_default();
    }
    sources.truncate(8);
    Some(Verdict { bugs, ok_pct, recommend, sources })
}

/// A atualização pesa na lista do Windows: as definições do Defender saem várias vezes por dia.
fn counts(u: &WinUpd) -> bool {
    !u.definition && !u.hidden
}

/// Começa a acompanhar `name` se ele mudou; devolve se é novidade.
fn track(slot: &mut Option<Watch>, name: String, now: u64) -> bool {
    if slot.as_ref().is_some_and(|w| w.name == name) {
        return false;
    }
    *slot = Some(Watch { name, seen: now, ..Default::default() });
    true
}

/// Como `track`, mas a lista do Windows só é novidade quando entra um KB: quando só sai um (bloqueado
/// ou instalado), o veredito e a espera continuam valendo.
fn track_win(slot: &mut Option<Watch>, kbs: &[String], now: u64) -> bool {
    if let Some(w) = slot.as_mut().filter(|w| kbs.iter().all(|k| w.name.split(", ").any(|o| o == k))) {
        w.name = kbs.join(", ");
        return false;
    }
    track(slot, kbs.join(", "), now)
}

/// Roda o veredito de uma novidade: as fontes oficiais e, com chave, o Gemini.
fn judge(w: &mut Watch, key: &str, official: impl FnOnce() -> Result<Vec<String>, ()>, prompt: impl FnOnce(&[String]) -> String, now: u64) {
    if let Ok(o) = official() {
        w.official = o;
    }
    if key.trim().is_empty() {
        w.judged = now;
        return;
    }
    match gemini(key, &prompt(&w.official)) {
        Ok(v) => {
            w.verdict = Some(v);
            w.error.clear();
            w.judged = now;
        }
        Err(e) => {
            crate::config::log(&format!("Atualizações seguras: o Gemini falhou ({e})"));
            w.error = format!("{e}@{now}");
        }
    }
}

/// Avisos que a conferência devolve para a tela.
pub enum Notice {
    NewDriver(String),
    NewWindows(usize),
    Verdict(String),
}

/// Uma conferência completa (de outra thread: é rede e COM). `force` pesquisa já, sem esperar os relatos.
pub fn run(mut st: State, key: &str, wait_h: u32, force: bool) -> (State, Vec<Notice>) {
    let now = now();
    let mut notes = Vec::new();
    st.checked = now;
    // NVIDIA
    st.installed = super::drs::driver_version().map(version_text).unwrap_or_default();
    if !st.installed.is_empty() {
        match nv_latest() {
            Some(d) => {
                st.nv_error = false;
                if newer(&d.version, &st.installed) {
                    if track(&mut st.nv, d.version.clone(), now) {
                        notes.push(Notice::NewDriver(d.version.clone()));
                        if let (Some(w), Ok(o)) = (st.nv.as_mut(), nv_official(&d)) {
                            w.official = o;
                        }
                    }
                } else {
                    st.nv = None;
                }
                st.driver = Some(d);
            }
            None => st.nv_error = true,
        }
    }
    if let (Some(w), Some(d)) = (st.nv.as_mut(), st.driver.clone()) {
        if force && w.verdict.is_none() || waiting_done(w, wait_h, now) {
            let installed = st.installed.clone();
            judge(w, key, || nv_official(&d), |o| nv_prompt(&d.version, &installed, o), now);
            if w.judged != 0 {
                notes.push(Notice::Verdict(format!("NVIDIA {}", d.version)));
            }
        }
    }
    // Windows
    match super::winupd::pending() {
        Ok(list) => {
            st.win_error.clear();
            st.updates = list;
            let mut kbs: Vec<String> = st.updates.iter().filter(|u| counts(u)).map(|u| if u.kb.is_empty() { u.title.clone() } else { u.kb.clone() }).collect();
            kbs.sort();
            if kbs.is_empty() {
                st.win = None;
            } else if track_win(&mut st.win, &kbs, now) {
                notes.push(Notice::NewWindows(kbs.len()));
                if let (Some(w), Ok(o)) = (st.win.as_mut(), win_official()) {
                    w.official = o;
                }
            }
        }
        Err(e) => st.win_error = e,
    }
    let ups: Vec<WinUpd> = st.updates.iter().filter(|u| counts(u)).cloned().collect();
    if let Some(w) = st.win.as_mut() {
        if force && w.verdict.is_none() || waiting_done(w, wait_h, now) {
            judge(w, key, win_official, |o| win_prompt(&ups.iter().collect::<Vec<_>>(), o), now);
            if w.judged != 0 {
                notes.push(Notice::Verdict("Windows Update".into()));
            }
        }
    }
    (st, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nv_escolhe_game_ready_e_decodifica() {
        let j = r#"{"Success":"2","IDS":[
            {"downloadInfo":{"Version":"621.24","IsCRD":"1","ReleaseDateTime":"Thu Nov 12, 2026","DownloadURL":"https://x/studio.exe","OtherNotes":""}},
            {"downloadInfo":{"Version":"621.24","IsCRD":"0","ReleaseDateTime":"Thu Nov 12, 2026","DownloadURL":"https://x/gr.exe","DetailsURL":"https://www.nvidia.com/d/1/",
             "OtherNotes":"%3Cul%3E%3Cli%3E%3Ca%20href%3D%22https%3A//us.download.nvidia.com/Windows/621.24/621.24-geforce-win11-x64-release-notes.pdf%22%3ENotes%3C/a%3E%3C/li%3E%3C/ul%3E"}}]}"#;
        let d = parse_nv(j).unwrap();
        assert_eq!(d.url, "https://x/gr.exe");
        assert_eq!(d.notes, "https://us.download.nvidia.com/Windows/621.24/621.24-geforce-win11-x64-release-notes.pdf");
        assert_eq!(d.date, "Thu Nov 12, 2026");
        // Só o Studio na lista: fica com ele.
        let j = r#"{"IDS":[{"downloadInfo":{"Version":"600.10","IsCRD":"1"}}]}"#;
        assert_eq!(parse_nv(j).unwrap().version, "600.10");
        assert!(parse_nv("{\"Success\":\"0\"}").is_none());
        assert_eq!(url_decode("a%20b%2"), "a b%2");
    }

    #[test]
    fn pfid_pelo_nome() {
        let j = r#"{"desktop":{"GeForce RTX 4070":"1015","GeForce RTX 4070 SUPER":"1039"},"notebook":{"GeForce RTX 4070 Laptop GPU":"1006"}}"#;
        assert_eq!(pfid(j, "NVIDIA GeForce RTX 4070").as_deref(), Some("1015"));
        assert_eq!(pfid(j, "NVIDIA GeForce RTX 4070 Laptop GPU").as_deref(), Some("1006"));
        assert_eq!(pfid(j, "NVIDIA GeForce RTX 9999"), None);
    }

    #[test]
    fn versoes() {
        assert_eq!(version_text(58197), "581.97");
        assert_eq!(version_text(62105), "621.05");
        assert!(newer("621.24", "581.97"));
        assert!(newer("621.24", "621.5"));
        assert!(!newer("621.24", "621.24"));
    }

    #[test]
    fn veredito_do_gemini() {
        let inner = "```json\n{\"bugs\":[{\"text\":\"Tela preta\",\"severity\":\"high\"},{\"text\":\"Flicker\",\"severity\":\"LOW\"},{\"text\":\" \",\"severity\":\"low\"}],\"ok_percent\":\"85%\",\"recommend\":false,\"sources\":[\"https://reddit.com/r/nvidia/x\",\"nada\"]}\n```";
        let j = serde_json::json!({"candidates":[{"content":{"parts":[{"text":inner}]}}]}).to_string();
        let v = parse_reply(&j).unwrap();
        assert_eq!(v.bugs, vec![Bug { text: "Tela preta".into(), sev: Sev::High }, Bug { text: "Flicker".into(), sev: Sev::Low }]);
        assert_eq!(v.ok_pct, 85);
        assert!(!v.recommend);
        assert_eq!(v.sources, vec!["https://reddit.com/r/nvidia/x".to_string()]);
        // % fora da faixa e fontes só na busca.
        let j = serde_json::json!({"candidates":[{"content":{"parts":[{"text":"{\"bugs\":[],\"ok_percent\":140,\"recommend\":true}"}]},
            "groundingMetadata":{"groundingChunks":[{"web":{"uri":"https://g/1"}}]}}]})
        .to_string();
        let v = parse_reply(&j).unwrap();
        assert_eq!((v.ok_pct, v.recommend), (100, true));
        assert_eq!(v.sources, vec!["https://g/1".to_string()]);
        assert!(parse_reply("{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"sem json\"}]}}]}").is_none());
    }

    #[test]
    fn agenda() {
        let mut st = State { checked: 1000, ..Default::default() };
        assert!(!due(&st, 0, 72, 10_000_000), "desligado");
        assert!(!due(&st, 1, 72, 1000 + 86399));
        assert!(due(&st, 1, 72, 1000 + 86400));
        // Novidade vista: a espera de 24 h vence antes da semana.
        st.nv = Some(Watch { name: "621.24".into(), seen: 5000, ..Default::default() });
        assert!(!due(&st, 7, 24, 5000 + 86399));
        assert!(due(&st, 7, 24, 5000 + 86400));
        // Já julgada: volta a seguir só o intervalo.
        st.nv.as_mut().unwrap().judged = 6000;
        assert!(!due(&st, 7, 24, 5000 + 86400));
        // Falhou: espera uma hora antes de tentar de novo.
        let w = st.nv.as_mut().unwrap();
        w.judged = 0;
        w.error = "rede@100000".into();
        assert!(!due(&st, 7, 24, 100000 + 3599));
        assert!(due(&st, 7, 24, 100000 + 3600));
    }

    #[test]
    fn acompanha_novidade() {
        let mut w = None;
        assert!(track(&mut w, "621.24".into(), 10));
        assert!(!track(&mut w, "621.24".into(), 20));
        assert_eq!(w.as_ref().unwrap().seen, 10);
        assert!(track(&mut w, "622.00".into(), 30));
        // Windows: bloquear um item não reinicia a espera nem apaga o veredito; um KB novo sim.
        let kb = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let mut w = None;
        assert!(track_win(&mut w, &kb(&["KB1", "KB2"]), 10));
        w.as_mut().unwrap().verdict = Some(Verdict::default());
        assert!(!track_win(&mut w, &kb(&["KB2"]), 20));
        assert_eq!((w.as_ref().unwrap().seen, w.as_ref().unwrap().name.as_str()), (10, "KB2"));
        assert!(w.as_ref().unwrap().verdict.is_some());
        assert!(track_win(&mut w, &kb(&["KB2", "KB3"]), 30));
        assert!(w.as_ref().unwrap().verdict.is_none());
    }

    #[test]
    fn notas_em_pdf() {
        // Texto de um bloco já descomprimido, como sai das notas da NVIDIA.
        let ops = b"BT /F1 9 Tf 72 700 Td (3.2  Open Issues in  Version 621.24) Tj 0 -14 Td (> Jogo X pode piscar com o G-SYNC) Tj 0 -12 Td (ligado. [5012345]) Tj 0 -12 Td (> Tela preta ao sair do \\223modo\\224 ) Tj 0 -12 Td (RN - 08399-001_v621) Tj 0 -12 Td (Issues Not Caused by NVIDIA) Tj ET";
        let mut out = Vec::new();
        text_ops(ops, &mut out);
        let txt = out.join("\n");
        assert_eq!(open_issues(&txt), vec!["Jogo X pode piscar com o G-SYNC ligado. [5012345]".to_string(), "Tela preta ao sair do \"modo\"".to_string()]);
        // Índice e seção vazia.
        let txt = "Open Issues in Version 621.24 .... 5\nOpen Issues in Version 621.24\n> N/A\nIssues Not Caused";
        assert!(open_issues(txt).is_empty());
        // O bloco comprimido do PDF inteiro.
        let z = miniz_oxide::deflate::compress_to_vec_zlib(b"BT (Ola) Tj 10 0 Td (mundo) Tj ET", 6);
        let mut pdf = b"%PDF-1.4\n1 0 obj<<>>stream\r\n".to_vec();
        pdf.extend_from_slice(&z);
        pdf.extend_from_slice(b"\nendstream endobj");
        assert_eq!(pdf_text(&pdf), "Ola mundo");
    }

    #[test]
    fn problemas_conhecidos_da_microsoft() {
        let html = r##"<h2 id="known-issues">Known issues</h2><table><tr><th>Summary</th></tr>
<tr><td><a href="#x"><b>Some games can&#39;t start</b></a><br>Desc<br></td><td class="has-no-wrap">OS Build 26300.100<br> <a href="y">KB5127648</a><br>2026-10-13</td><td>Confirmed<br></td><td>2026-10-14</td></tr>
<tr><td><b>Audio &amp; HDMI</b><br></td><td>OS Build 1<br></td><td>Resolved</td></tr></table><table><tr><td><b>fora</b></td><td>a</td><td>b</td></tr></table>"##;
        assert_eq!(parse_known(html), vec!["Some games can't start (KB5127648, Confirmed)".to_string(), "Audio & HDMI (Resolved)".to_string()]);
        assert!(parse_known("<html></html>").is_empty());
    }
}
