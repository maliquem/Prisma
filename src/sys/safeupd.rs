//! Atualizações seguras: o que saiu do driver da NVIDIA e do Windows Update, e se vale instalar.
//!
//! Fontes: o endpoint de busca do site da NVIDIA, as "Open Issues" das notas em PDF, os "known
//! issues" da Microsoft (release health) e, com a chave da pessoa, o Gemini (busca do Google) e/ou o
//! DeepSeek (busca própria), que leem os relatos (megathreads do r/nvidia e do r/Windows11, fórum da
//! NVIDIA, Guru3D, sites de notícias...). Com os dois, os resultados são juntados num veredito só.
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
    /// O que cada IA disse quando pesquisaram juntas (vazio com uma só).
    #[serde(default)]
    pub views: Vec<View>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Default, Debug, PartialEq)]
pub enum Ai {
    #[default]
    Gemini,
    DeepSeek,
    Claude,
    ChatGpt,
}

/// Todas as IAs, na ordem em que aparecem e são chamadas: a primeira que responde junta os resultados.
pub const ALL: [Ai; 4] = [Ai::Gemini, Ai::DeepSeek, Ai::Claude, Ai::ChatGpt];

impl Ai {
    pub fn name(self) -> &'static str {
        match self {
            Ai::Gemini => "Gemini",
            Ai::DeepSeek => "DeepSeek",
            Ai::Claude => "Claude",
            Ai::ChatGpt => "ChatGPT",
        }
    }

    /// O bit da IA em `Config::safe_ais`.
    pub fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// A parte de uma IA no veredito juntado; `error` quando ela falhou e o veredito ficou com a outra.
#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
pub struct View {
    pub ai: Ai,
    pub ok_pct: u8,
    pub recommend: bool,
    pub error: String,
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

const ANSWER: &str = "Answer ONLY with one JSON object, no other text: {\"bugs\":[{\"text\":\"short description\",\"severity\":\"high|medium|low\"}],\"ok_percent\":0-100,\"recommend\":true|false,\"sources\":[\"url\", ...]}. ok_percent is your estimate of the share of users without problems, from how many complain. recommend is whether a typical gamer should install it now. List the problems that affect this release, including ones that started in earlier releases and are still reported for it: crashes, stutter and black screens, but also background problems such as excessive SSD or disk writes, high CPU or power use at idle, and broken features. Do not drop a problem because only news sites report it or because the vendor has not confirmed it (then say \"unconfirmed\" in the text). Most serious first, at most 8.";

pub fn nv_prompt(version: &str, installed: &str, official: &[String]) -> String {
    format!(
        "You are checking if NVIDIA GeForce Game Ready driver {version} for Windows is safe to install (the user has {installed}). Run several web searches: the r/nvidia megathread titled \"Game Ready Driver {version} FAQ/Discussion\"; tech news from the last 30 days about NVIDIA driver {version} and about problems in recent NVIDIA drivers (TechPowerUp, Wccftech, Tom's Hardware, VideoCardz, Notebookcheck, PC Gamer, Neowin); then the NVIDIA forums driver feedback thread, Guru3D and Overclockers. NVIDIA lists these open issues: {}. Write bug texts in {}. {ANSWER}",
        if official.is_empty() { "none".into() } else { official.join("; ") },
        lang_name()
    )
}

pub fn win_prompt(updates: &[&WinUpd], known: &[String]) -> String {
    let list: Vec<String> = updates.iter().map(|u| u.title.clone()).collect();
    format!(
        "You are checking if these pending Windows updates are safe to install on a gaming PC ({}): {}. Run several web searches: the r/Windows11 (or r/Windows10) megathread titled \"Cumulative Updates <release date>\" for this month; tech news from the last 30 days about these KBs and about problems in recent Windows updates (Windows Latest, BleepingComputer, Neowin, Windows Central, Tom's Hardware, The Register); then Microsoft Q&A and other recent discussions. Microsoft lists these known issues: {}. Write bug texts in {}. {ANSWER}",
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

/// As IAs marcadas em `mask` que têm chave (`keys` na ordem de `ALL`), na ordem de `ALL`. Só o Gemini
/// é grátis: uma IA paga desmarcada nunca é chamada.
pub fn chosen(mask: u8, keys: [&str; 4]) -> Vec<(Ai, String)> {
    ALL.into_iter()
        .zip(keys)
        .filter(|(a, k)| mask & a.bit() != 0 && !k.trim().is_empty())
        .map(|(a, k)| (a, k.trim().to_string()))
        .collect()
}

/// Pergunta a uma IA; `search` liga a busca na web. Devolve o texto e as páginas que ela leu. A chave
/// vai só no cabeçalho, nunca no log nem no endereço.
fn ask(ai: Ai, key: &str, prompt: &str, search: bool) -> Result<(String, Vec<String>), String> {
    match ai {
        Ai::Gemini => gemini(key, prompt, search),
        Ai::DeepSeek => deepseek(key, prompt, search),
        Ai::Claude => claude(key, prompt, search),
        Ai::ChatGpt => chatgpt(key, prompt, search),
    }
}

/// Uma pergunta ao Gemini com a busca do Google, para quem não é desta página (a aba Jogos).
pub fn ask_gemini(key: &str, prompt: &str) -> Result<String, String> {
    gemini(key, prompt, true).map(|r| r.0)
}

fn gemini(key: &str, prompt: &str, search: bool) -> Result<(String, Vec<String>), String> {
    let mut body = serde_json::json!({"contents":[{"parts":[{"text":prompt}]}]});
    if search {
        body["tools"] = serde_json::json!([{"google_search":{}}]);
    }
    let body = body.to_string();
    let headers = format!("x-goog-api-key: {}\r\nContent-Type: application/json\r\n", key.trim());
    let mut last = String::new();
    for m in MODELS {
        let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{m}:generateContent");
        let Ok((status, text)) = super::update::post(&url, &headers, &body) else { return Err("rede".into()) };
        match status {
            200 => return gemini_text(&text).ok_or_else(|| "resposta".into()),
            404 => last = "modelo".into(),
            429 => return Err("limite".into()),
            400 | 401 | 403 => return Err("chave".into()),
            s => return Err(format!("http {s}")),
        }
    }
    Err(last)
}

/// O texto da resposta do Gemini e as páginas da busca.
pub fn gemini_text(json: &str) -> Option<(String, Vec<String>)> {
    let v: Value = serde_json::from_str(json).ok()?;
    let cand = &v["candidates"][0];
    let text = cand["content"]["parts"].as_array()?.iter().filter_map(|p| p["text"].as_str()).collect();
    let pages = cand["groundingMetadata"]["groundingChunks"].as_array().map(|a| a.iter().filter_map(|c| c["web"]["uri"].as_str()).map(str::to_string).collect()).unwrap_or_default();
    Some((text, pages))
}

/// O DeepSeek pesquisa sozinho no servidor (busca e leitura de páginas) e devolve tudo numa resposta.
/// Com o raciocínio ligado (o padrão) a busca é a profunda; a junção, sem busca, vai sem raciocínio.
fn deepseek(key: &str, prompt: &str, search: bool) -> Result<(String, Vec<String>), String> {
    let mut body = serde_json::json!({"model":"deepseek-flash","messages":[{"role":"user","content":prompt}]});
    if search {
        body["tools"] = serde_json::json!([{"type":"deepseek_web_search"},{"type":"deepseek_read_url"}]);
    } else {
        body["thinking"] = serde_json::json!({"type":"disabled"});
    }
    let headers = format!("Authorization: Bearer {}\r\nContent-Type: application/json\r\n", key.trim());
    let Ok((status, text)) = super::update::post("https://api.deepseek.com/chat/completions", &headers, &body.to_string()) else { return Err("rede".into()) };
    match status {
        200 => deepseek_text(&text).ok_or_else(|| "resposta".into()),
        401 => Err("chave".into()),
        402 => Err("saldo".into()),
        429 => Err("limite".into()),
        s => Err(format!("http {s}")),
    }
}

/// O texto da resposta do DeepSeek e as páginas citadas. Antes do JSON vêm quebras de linha (ele as
/// manda enquanto pesquisa), e um erro depois do 200 vem no corpo, sem `choices`.
pub fn deepseek_text(json: &str) -> Option<(String, Vec<String>)> {
    let v: Value = serde_json::from_str(json.trim()).ok()?;
    let m = &v["choices"][0]["message"];
    let text = m["content"].as_str().filter(|t| !t.trim().is_empty())?.to_string();
    let pages = m["annotations"].as_array().map(|a| a.iter().filter_map(|c| c["url_citation"]["url"].as_str()).map(str::to_string).collect()).unwrap_or_default();
    Some((text, pages))
}

const CLAUDE_MODEL: &str = "claude-haiku-5-5";

/// O Claude (Haiku) pesquisa com a busca da Anthropic, no servidor, e responde numa chamada. Se a busca
/// passa do limite de voltas do servidor (`pause_turn`), a chamada segue com a resposta parcial.
fn claude(key: &str, prompt: &str, search: bool) -> Result<(String, Vec<String>), String> {
    let mut body = serde_json::json!({"model":CLAUDE_MODEL,"max_tokens":16000,"messages":[{"role":"user","content":prompt}]});
    if search {
        body["tools"] = serde_json::json!([{"type":"web_search_20250305","name":"web_search","max_uses":8}]);
    } else {
        body["thinking"] = serde_json::json!({"type":"disabled"});
    }
    let headers = format!("x-api-key: {}\r\nanthropic-version: 2026-10-21\r\nContent-Type: application/json\r\n", key.trim());
    for _ in 0..3 {
        let Ok((status, text)) = super::update::post("https://api.anthropic.com/v1/messages", &headers, &body.to_string()) else { return Err("rede".into()) };
        match status {
            200 => {}
            401 | 403 => return Err("chave".into()),
            402 => return Err("saldo".into()),
            400 if text.contains("credit balance") => return Err("saldo".into()),
            404 => return Err("modelo".into()),
            429 => return Err("limite".into()),
            s => return Err(format!("http {s}")),
        }
        let v: Value = serde_json::from_str(&text).map_err(|_| "resposta".to_string())?;
        if v["stop_reason"] != "pause_turn" {
            return claude_text(&v).ok_or_else(|| "resposta".into());
        }
        if let Some(m) = body["messages"].as_array_mut() {
            m.push(serde_json::json!({"role":"assistant","content":v["content"]}));
        }
    }
    Err("resposta".into())
}

/// O texto da resposta do Claude e as páginas citadas (sem repetir). Uma recusa vem sem texto.
pub fn claude_text(v: &Value) -> Option<(String, Vec<String>)> {
    let blocks = v["content"].as_array()?.iter().filter(|b| b["type"] == "text");
    let mut text = String::new();
    let mut pages: Vec<String> = Vec::new();
    for b in blocks {
        text += b["text"].as_str().unwrap_or("");
        for u in b["citations"].as_array().into_iter().flatten().filter_map(|c| c["url"].as_str()) {
            if !pages.iter().any(|p| p == u) {
                pages.push(u.to_string());
            }
        }
    }
    (!text.trim().is_empty()).then_some((text, pages))
}

const GPT_MODEL: &str = "gpt-5-mini";

/// O ChatGPT pesquisa com a busca da OpenAI (API Responses), no servidor, e responde numa chamada.
fn chatgpt(key: &str, prompt: &str, search: bool) -> Result<(String, Vec<String>), String> {
    let mut body = serde_json::json!({"model":GPT_MODEL,"input":prompt,"reasoning":{"effort":"low"}});
    if search {
        body["tools"] = serde_json::json!([{"type":"web_search"}]);
    }
    let headers = format!("Authorization: Bearer {}\r\nContent-Type: application/json\r\n", key.trim());
    let Ok((status, text)) = super::update::post("https://api.openai.com/v1/responses", &headers, &body.to_string()) else { return Err("rede".into()) };
    match status {
        200 => chatgpt_text(&text).ok_or_else(|| "resposta".into()),
        401 => Err("chave".into()),
        404 => Err("modelo".into()),
        // O 429 serve para o limite por minuto e para a conta sem crédito.
        429 if ["quota", "credit", "spend_limit", "usage_limit"].iter().any(|w| text.contains(w)) => Err("saldo".into()),
        429 => Err("limite".into()),
        s => Err(format!("http {s}")),
    }
}

/// O texto da resposta do ChatGPT (os itens "message" da saída) e as páginas citadas.
pub fn chatgpt_text(json: &str) -> Option<(String, Vec<String>)> {
    let v: Value = serde_json::from_str(json).ok()?;
    let parts: Vec<&Value> = v["output"].as_array()?.iter().filter(|o| o["type"] == "message").flat_map(|o| o["content"].as_array().into_iter().flatten()).filter(|c| c["type"] == "output_text").collect();
    let text: String = parts.iter().filter_map(|c| c["text"].as_str()).collect();
    let mut pages: Vec<String> = Vec::new();
    for u in parts.iter().flat_map(|c| c["annotations"].as_array().into_iter().flatten()).filter(|a| a["type"] == "url_citation").filter_map(|a| a["url"].as_str()) {
        if !pages.iter().any(|p| p == u) {
            pages.push(u.to_string());
        }
    }
    (!text.trim().is_empty()).then_some((text, pages))
}

fn sev(s: &str) -> Sev {
    match s.to_lowercase().as_str() {
        "high" | "grave" | "critical" | "severe" => Sev::High,
        "low" | "baixo" | "minor" => Sev::Low,
        _ => Sev::Medium,
    }
}

/// O objeto JSON pedido no texto (às vezes entre crases).
pub fn json_in(text: &str) -> Option<Value> {
    let (a, b) = (text.find('{')?, text.rfind('}')?);
    serde_json::from_str(text.get(a..=b)?).ok()
}

fn bugs_of(r: &Value) -> Vec<Bug> {
    r["bugs"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|b| {
                    let t = b["text"].as_str().or(b.as_str())?.trim();
                    (!t.is_empty()).then(|| Bug { text: t.to_string(), sev: sev(b["severity"].as_str().unwrap_or("")) })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// O veredito no texto da IA; as páginas da busca valem como fontes quando ela não cita nenhuma.
pub fn parse_verdict(text: &str, pages: Vec<String>) -> Option<Verdict> {
    let r = json_in(text)?;
    let ok_pct = r["ok_percent"].as_f64().or_else(|| r["ok_percent"].as_str().and_then(|s| s.trim_end_matches('%').parse().ok()))?.clamp(0.0, 100.0).round() as u8;
    let recommend = r["recommend"].as_bool().unwrap_or(false);
    let mut sources: Vec<String> = r["sources"].as_array().map(|a| a.iter().filter_map(|s| s.as_str()).filter(|s| s.starts_with("http")).map(str::to_string).collect()).unwrap_or_default();
    if sources.is_empty() {
        sources = pages;
    }
    sources.truncate(8);
    Some(Verdict { bugs: bugs_of(&r), ok_pct, recommend, sources, views: Vec::new() })
}

fn research(ai: Ai, key: &str, prompt: &str) -> Result<Verdict, String> {
    let (text, pages) = ask(ai, key, prompt, true)?;
    parse_verdict(&text, pages).ok_or_else(|| "resposta".into())
}

/// A IA junta as listas de problemas das pesquisas, sem buscar de novo.
fn merge_bugs(ai: Ai, key: &str, got: &[(Ai, Verdict)]) -> Option<Vec<Bug>> {
    let lists: Vec<Value> = got.iter().map(|(a, v)| serde_json::json!({"ai": a.name(), "bugs": v.bugs.iter().map(|b| serde_json::json!({"text": b.text, "severity": format!("{:?}", b.sev).to_lowercase()})).collect::<Vec<_>>()})).collect();
    let prompt = format!(
        "Several AIs researched the same update and listed its problems. Merge the lists into one: join entries that describe the same problem, keep every distinct problem and any \"unconfirmed\" mark, keep the language of the texts, most serious first, at most 10. Answer ONLY with one JSON object, no other text: {{\"bugs\":[{{\"text\":\"short description\",\"severity\":\"high|medium|low\"}}]}}. Lists: {}",
        Value::Array(lists)
    );
    let bugs = bugs_of(&json_in(&ask(ai, key, &prompt, false).ok()?.0)?);
    (!bugs.is_empty()).then_some(bugs)
}

/// Junta os vereditos das IAs: a média da estimativa, recomenda só se todas recomendam e as fontes de
/// todas. `bugs` é a lista que a IA juntou; sem ela, a união sem repetidos, os graves primeiro.
pub fn combine(got: &[(Ai, Verdict)], bugs: Option<Vec<Bug>>) -> Verdict {
    let n = got.len().max(1) as f32;
    let bugs = bugs.unwrap_or_else(|| {
        let mut all: Vec<Bug> = got.iter().flat_map(|(_, v)| v.bugs.iter().cloned()).collect();
        all.sort_by_key(|b| b.sev as u8);
        let mut seen = std::collections::HashSet::new();
        all.retain(|b| seen.insert(b.text.to_lowercase()));
        all.truncate(10);
        all
    });
    let mut sources: Vec<String> = Vec::new();
    for s in got.iter().flat_map(|(_, v)| &v.sources) {
        if !sources.contains(s) {
            sources.push(s.clone());
        }
    }
    sources.truncate(10);
    Verdict {
        bugs,
        ok_pct: (got.iter().map(|(_, v)| v.ok_pct as f32).sum::<f32>() / n).round() as u8,
        recommend: !got.is_empty() && got.iter().all(|(_, v)| v.recommend),
        sources,
        views: Vec::new(),
    }
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

/// Roda o veredito de uma novidade: as fontes oficiais e, com chave, as IAs (juntas, ao mesmo tempo).
/// Uma IA que falha não derruba o veredito da outra; só quando todas falham a pesquisa volta a tentar.
fn judge(w: &mut Watch, keys: &[(Ai, String)], official: impl FnOnce() -> Result<Vec<String>, ()>, prompt: impl FnOnce(&[String]) -> String, now: u64) {
    if let Ok(o) = official() {
        w.official = o;
    }
    if keys.is_empty() {
        w.judged = now;
        return;
    }
    let p = prompt(&w.official);
    let res: Vec<(Ai, Result<Verdict, String>)> = std::thread::scope(|s| {
        let hs: Vec<_> = keys.iter().map(|(ai, k)| (*ai, s.spawn(|| research(*ai, k, &p)))).collect();
        hs.into_iter().map(|(ai, h)| (ai, h.join().unwrap_or_else(|_| Err("resposta".into())))).collect()
    });
    let mut views = Vec::new();
    let mut got = Vec::new();
    let mut err = String::new();
    for (ai, r) in res {
        match r {
            Ok(v) => {
                views.push(View { ai, ok_pct: v.ok_pct, recommend: v.recommend, error: String::new() });
                got.push((ai, v));
            }
            Err(e) => {
                crate::config::log(&format!("Atualizações seguras: o {} falhou ({e})", ai.name()));
                views.push(View { ai, error: e.clone(), ..Default::default() });
                if err.is_empty() {
                    err = format!("{}:{e}", ai.name());
                }
            }
        }
    }
    let Some(ai) = got.first().map(|(a, _)| *a) else {
        w.error = format!("{err}@{now}");
        return;
    };
    let mut v = if got.len() == 1 {
        got.pop().map(|(_, v)| v).unwrap_or_default()
    } else {
        let key = keys.iter().find(|(a, _)| *a == ai).map_or("", |(_, k)| k.as_str());
        combine(&got, merge_bugs(ai, key, &got))
    };
    if keys.len() > 1 {
        v.views = views;
    }
    w.verdict = Some(v);
    w.error.clear();
    w.judged = now;
}

/// Avisos que a conferência devolve para a tela.
pub enum Notice {
    NewDriver(String),
    NewWindows(usize),
    Verdict(String),
}

/// Uma conferência completa (de outra thread: é rede e COM). `force` pesquisa já, sem esperar os relatos.
pub fn run(mut st: State, keys: &[(Ai, String)], wait_h: u32, force: bool) -> (State, Vec<Notice>) {
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
            judge(w, keys, || nv_official(&d), |o| nv_prompt(&d.version, &installed, o), now);
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
            judge(w, keys, win_official, |o| win_prompt(&ups.iter().collect::<Vec<_>>(), o), now);
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
        let (t, p) = gemini_text(&j).unwrap();
        let v = parse_verdict(&t, p).unwrap();
        assert_eq!(v.bugs, vec![Bug { text: "Tela preta".into(), sev: Sev::High }, Bug { text: "Flicker".into(), sev: Sev::Low }]);
        assert_eq!(v.ok_pct, 85);
        assert!(!v.recommend);
        assert_eq!(v.sources, vec!["https://reddit.com/r/nvidia/x".to_string()]);
        // % fora da faixa e fontes só na busca.
        let j = serde_json::json!({"candidates":[{"content":{"parts":[{"text":"{\"bugs\":[],\"ok_percent\":140,\"recommend\":true}"}]},
            "groundingMetadata":{"groundingChunks":[{"web":{"uri":"https://g/1"}}]}}]})
        .to_string();
        let (t, p) = gemini_text(&j).unwrap();
        let v = parse_verdict(&t, p).unwrap();
        assert_eq!((v.ok_pct, v.recommend), (100, true));
        assert_eq!(v.sources, vec!["https://g/1".to_string()]);
        assert!(parse_verdict("sem json", Vec::new()).is_none());
    }

    #[test]
    fn resposta_do_deepseek() {
        let content = "{\"bugs\":[{\"text\":\"Grava demais no SSD (unconfirmed)\",\"severity\":\"medium\"}],\"ok_percent\":70,\"recommend\":false,\"sources\":[]}";
        let j = serde_json::json!({"choices":[{"message":{"role":"assistant","content":content,
            "annotations":[{"type":"url_citation","url_citation":{"url":"https://techpowerup.com/x","title":"t","start_index":0,"end_index":3}}]}}]});
        // As quebras de linha que ele manda enquanto pesquisa.
        let (t, p) = deepseek_text(&format!("\n\n{j}")).unwrap();
        let v = parse_verdict(&t, p).unwrap();
        assert_eq!(v.bugs, vec![Bug { text: "Grava demais no SSD (unconfirmed)".into(), sev: Sev::Medium }]);
        assert_eq!(v.sources, vec!["https://techpowerup.com/x".to_string()]);
        // Erro depois do 200, e conteúdo barrado pelo filtro.
        assert!(deepseek_text("{\"error\":{\"message\":\"x\"},\"usage\":{}}").is_none());
        assert!(deepseek_text("{\"choices\":[{\"message\":{\"content\":\"\"}}]}").is_none());
    }

    #[test]
    fn so_chama_a_ia_escolhida() {
        let ais = |m, k| chosen(m, k).into_iter().map(|(a, _)| a).collect::<Vec<_>>();
        let all = ["g", "d", "c", "o"];
        assert_eq!(ais(1, all), vec![Ai::Gemini]);
        assert_eq!(ais(2, all), vec![Ai::DeepSeek]);
        assert_eq!(ais(4, all), vec![Ai::Claude]);
        assert_eq!(ais(8, all), vec![Ai::ChatGpt]);
        assert_eq!(ais(15, all), ALL.to_vec());
        assert_eq!(ais(15, [" ", "d", "", "o"]), vec![Ai::DeepSeek, Ai::ChatGpt]);
        assert!(ais(0, all).is_empty());
        assert!(ais(1, ["", "d", "c", "o"]).is_empty());
        assert_eq!(chosen(4, ["", "", " k ", ""]), vec![(Ai::Claude, "k".to_string())]);
    }

    #[test]
    fn resposta_do_claude() {
        let v: Value = serde_json::from_str(
            r#"{"stop_reason":"end_turn","content":[{"type":"server_tool_use","name":"web_search"},{"type":"web_search_tool_result","content":[{"type":"web_search_result","url":"https://x"}]},{"type":"text","text":"Relatos: ","citations":[{"type":"web_search_result_location","url":"https://a"}]},{"type":"text","text":"{\"ok_percent\":70}","citations":[{"url":"https://a"},{"url":"https://b"}]}]}"#,
        )
        .unwrap();
        let (text, pages) = claude_text(&v).unwrap();
        assert_eq!(text, "Relatos: {\"ok_percent\":70}");
        assert_eq!(pages, vec!["https://a".to_string(), "https://b".into()]);
        // Recusa: sem texto.
        assert!(claude_text(&serde_json::json!({"stop_reason":"refusal","content":[]})).is_none());
    }

    #[test]
    fn resposta_do_chatgpt() {
        let (text, pages) = chatgpt_text(
            r#"{"output":[{"type":"web_search_call","action":{"type":"search"}},{"type":"message","content":[{"type":"output_text","text":"{\"ok_percent\":90}","annotations":[{"type":"url_citation","url":"https://a"},{"type":"url_citation","url":"https://a"}]}]}]}"#,
        )
        .unwrap();
        assert_eq!(text, "{\"ok_percent\":90}");
        assert_eq!(pages, vec!["https://a".to_string()]);
        assert!(chatgpt_text(r#"{"output":[{"type":"reasoning"}]}"#).is_none());
    }

    #[test]
    fn junta_as_duas_ias() {
        let b = |t: &str, sev| Bug { text: t.into(), sev };
        let g = Verdict { bugs: vec![b("Flicker", Sev::Low), b("Tela preta", Sev::High)], ok_pct: 80, recommend: true, sources: vec!["https://a".into(), "https://b".into()], views: Vec::new() };
        let d = Verdict { bugs: vec![b("tela preta", Sev::High), b("SSD", Sev::Medium)], ok_pct: 65, recommend: false, sources: vec!["https://b".into(), "https://c".into()], views: Vec::new() };
        let got = [(Ai::Gemini, g), (Ai::DeepSeek, d)];
        // Sem a junção da IA: a união sem repetidos, os graves primeiro.
        let v = combine(&got, None);
        assert_eq!(v.bugs, vec![b("Tela preta", Sev::High), b("SSD", Sev::Medium), b("Flicker", Sev::Low)]);
        assert_eq!((v.ok_pct, v.recommend), (73, false));
        assert_eq!(v.sources, vec!["https://a".to_string(), "https://b".into(), "https://c".into()]);
        // Com a lista juntada pela IA, e com uma só.
        assert_eq!(combine(&got, Some(vec![b("x", Sev::Low)])).bugs, vec![b("x", Sev::Low)]);
        let v = combine(&got[..1], None);
        assert_eq!((v.ok_pct, v.recommend, v.bugs.len()), (80, true, 2));
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
