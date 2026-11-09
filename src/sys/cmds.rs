//! Comandos do PowerShell e do CMD para completar o que se digita depois do `>`. A lista sai do
//! próprio Windows uma vez (Get-Command, a ajuda do CMD e os programas do PATH) e fica guardada.
//! Opcional: um PowerShell escondido completa como o terminal, com caminhos e histórico.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

const NO_WINDOW: u32 = 0x0800_0000;
const BELOW_NORMAL: u32 = 0x0000_4000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Cmdlet,
    Function,
    Alias,
    Cmd,
    Exe,
    Param,
    Folder,
    File,
    History,
    /// Vindo do PowerShell escondido, com a descrição dele.
    Shell,
}

#[derive(Clone)]
struct Entry {
    name: String,
    low: String,
    kind: Kind,
    /// Alias: o comando de verdade. CMD: a descrição. Programa: a pasta.
    note: String,
    params: Vec<String>,
}

/// Uma sugestão: a linha inteira como fica ao aceitar, o que aparece e uma nota.
#[derive(Clone, Debug, PartialEq)]
pub struct Sug {
    pub text: String,
    pub label: String,
    pub note: String,
    pub kind: Kind,
}

#[derive(Default)]
struct List {
    all: Vec<Entry>,
    loading: bool,
    loaded: bool,
}

static LIST: LazyLock<Mutex<List>> = LazyLock::new(Default::default);
static VER: AtomicU64 = AtomicU64::new(0);

/// Muda quando a lista ou as respostas do PowerShell escondido mudam.
pub fn version() -> u64 {
    VER.load(Ordering::Relaxed)
}

/// Quantos comandos a lista tem. `None` enquanto ela é lida.
pub fn count() -> Option<usize> {
    let l = LIST.lock().unwrap();
    (l.loaded && !l.loading).then_some(l.all.len())
}

fn cache() -> PathBuf {
    crate::config::data_dir().join("comandos.tsv")
}

fn wake(notify: isize) {
    use windows::Win32::Foundation::*;
    use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
    VER.fetch_add(1, Ordering::Relaxed);
    unsafe {
        let h = Some(HWND(notify as *mut _));
        let _ = PostMessageW(h, crate::app::WM_FIND_DONE, WPARAM(0), LPARAM(0));
        let _ = PostMessageW(h, crate::app::WM_FIND_DONE, WPARAM(1), LPARAM(0));
    }
}

/// Carrega a lista guardada ou, sem ela, monta uma em segundo plano.
pub fn ensure(notify: isize) {
    load(notify, false);
}

/// Monta a lista de novo, para pegar módulos e programas instalados depois.
pub fn rebuild(notify: isize) {
    load(notify, true);
}

fn load(notify: isize, fresh: bool) {
    {
        let mut l = LIST.lock().unwrap();
        if l.loading || (l.loaded && !fresh) {
            return;
        }
        l.loading = true;
    }
    let _ = std::thread::Builder::new().name("comandos".into()).spawn(move || {
        let all = match (fresh, read_cache()) {
            (false, Some(all)) => all,
            _ => {
                let all = build();
                write_cache(&all);
                all
            }
        };
        let mut l = LIST.lock().unwrap();
        l.all = all;
        l.loaded = true;
        l.loading = false;
        drop(l);
        wake(notify);
    });
}

/// Tira a lista da memória. O arquivo fica, para ligar de novo sem esperar.
pub fn forget() {
    let mut l = LIST.lock().unwrap();
    if !l.loading {
        l.all = Vec::new();
        l.loaded = false;
    }
}

fn kind_char(k: Kind) -> char {
    match k {
        Kind::Cmdlet => 'P',
        Kind::Function => 'F',
        Kind::Alias => 'A',
        Kind::Cmd => 'C',
        _ => 'E',
    }
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\r', '\n'], " ")
}

fn write_cache(all: &[Entry]) {
    let mut s = String::new();
    for e in all {
        s.push_str(&format!("{}\t{}\t{}\t{}\n", kind_char(e.kind), clean(&e.name), clean(&e.note), e.params.join(",")));
    }
    let p = cache();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, s);
}

fn read_cache() -> Option<Vec<Entry>> {
    let s = std::fs::read_to_string(cache()).ok()?;
    let mut out = Vec::new();
    for line in s.lines() {
        let mut f = line.split('\t');
        let (Some(k), Some(name), note, params) = (f.next(), f.next(), f.next().unwrap_or_default(), f.next().unwrap_or_default()) else { continue };
        let kind = match k {
            "P" => Kind::Cmdlet,
            "F" => Kind::Function,
            "A" => Kind::Alias,
            "C" => Kind::Cmd,
            _ => Kind::Exe,
        };
        out.push(entry(name, kind, note, params));
    }
    (!out.is_empty()).then_some(out)
}

fn entry(name: &str, kind: Kind, note: &str, params: &str) -> Entry {
    Entry {
        name: name.to_string(),
        low: name.to_lowercase(),
        kind,
        note: note.to_string(),
        params: params.split(',').filter(|p| !p.is_empty()).map(String::from).collect(),
    }
}

/// Roda um programa escondido e devolve o que ele escreveu, com limite de tempo.
fn capture(mut cmd: Command, limit: Duration) -> Option<Vec<u8>> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).creation_flags(NO_WINDOW | BELOW_NORMAL).spawn().ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = out.read_to_end(&mut v);
        v
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(None) if start.elapsed() < limit => std::thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            _ => break,
        }
    }
    reader.join().ok()
}

const PS_LIST: &str = r#"$ErrorActionPreference='SilentlyContinue'
$ProgressPreference='SilentlyContinue'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
$common=@([System.Management.Automation.Cmdlet]::CommonParameters)+@([System.Management.Automation.Cmdlet]::OptionalCommonParameters)
foreach($c in Get-Command -CommandType Cmdlet,Function,Alias){
  $p=''
  if($c.CommandType -ne 'Alias'){ try{
    $k=@($c.Parameters.Keys | Where-Object { $_ })
    # Num módulo ainda não carregado, a lista vem vazia: pedir pelo nome completo carrega.
    if($k.Count -eq 0 -and $c.ModuleName){ $k=@((Get-Command -Name "$($c.ModuleName)\$($c.Name)").Parameters.Keys | Where-Object { $_ }) }
    $p=($k | Where-Object { $common -notcontains $_ }) -join ','
  }catch{} }
  $d=if($c.CommandType -eq 'Alias'){ $c.Definition } else { '' }
  [Console]::Out.WriteLine("$($c.CommandType)`t$($c.Name)`t$p`t$d")
}"#;

fn powershell_list() -> Vec<Entry> {
    let mut cmd = Command::new(crate::sys::quick::powershell());
    let enc = crate::sys::quick::b64(&PS_LIST.encode_utf16().flat_map(|u| u.to_le_bytes()).collect::<Vec<u8>>());
    cmd.raw_arg(format!("-NoLogo -NoProfile -NonInteractive -EncodedCommand {enc}"));
    let Some(out) = capture(cmd, Duration::from_secs(240)) else { return Vec::new() };
    let text = String::from_utf8_lossy(&out);
    let mut all = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 4 {
            continue;
        }
        let kind = match f[0] {
            "Cmdlet" => Kind::Cmdlet,
            "Function" => Kind::Function,
            "Alias" => Kind::Alias,
            _ => continue,
        };
        let name = f[1].trim();
        // Funções que o PowerShell cria para trocar de disco e de pasta.
        let drive = name.len() == 2 && name.ends_with(':');
        if name.is_empty() || drive || name.starts_with("cd.") || name == "cd\\" {
            continue;
        }
        let note = if kind == Kind::Alias { f[3].trim() } else { "" };
        all.push(entry(name, kind, note, f[2]));
    }
    all
}

/// Texto na página de código do console, como o CMD escreve.
fn oem(b: &[u8]) -> String {
    use windows::Win32::Globalization::*;
    unsafe {
        let n = MultiByteToWideChar(CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), b, None);
        if n <= 0 {
            return String::from_utf8_lossy(b).to_string();
        }
        let mut w = vec![0u16; n as usize];
        MultiByteToWideChar(CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), b, Some(&mut w));
        String::from_utf16_lossy(&w)
    }
}

/// Comandos da ajuda do CMD, com a descrição no idioma do Windows.
fn cmd_help() -> Vec<Entry> {
    let mut cmd = Command::new(crate::sys::system_tool("cmd"));
    cmd.raw_arg("/d /c help");
    let Some(out) = capture(cmd, Duration::from_secs(20)) else { return Vec::new() };
    parse_help(&oem(&out))
}

fn parse_help(text: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let mut open = false;
    for line in text.lines() {
        if line.trim().is_empty() {
            open = false;
            continue;
        }
        if line.starts_with([' ', '\t']) {
            if let Some(e) = out.last_mut().filter(|_| open) {
                e.note.push(' ');
                e.note.push_str(line.trim());
            }
            continue;
        }
        let name: String = line.chars().take_while(|c| !c.is_whitespace()).collect();
        let rest = &line[name.len()..];
        if name.len() >= 2 && name.chars().all(|c| c.is_ascii_uppercase()) && rest.starts_with("  ") {
            out.push(entry(&name.to_lowercase(), Kind::Cmd, rest.trim(), ""));
            open = true;
        } else {
            open = false;
        }
    }
    out
}

fn build() -> Vec<Entry> {
    let mut all = powershell_list();
    all.extend(cmd_help());
    let mut seen: HashSet<String> = all.iter().map(|e| e.low.clone()).collect();
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for f in rd.flatten() {
            let p = f.path();
            let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if !matches!(ext.as_str(), "exe" | "cmd" | "bat" | "com") {
                continue;
            }
            let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().to_string()) else { continue };
            if seen.insert(stem.to_lowercase()) {
                all.push(entry(&stem, Kind::Exe, &dir.display().to_string(), ""));
            }
        }
    }
    all
}

// ---------- Completar ----------

/// Onde começa a última palavra, se ela é o nome do comando e qual é o comando dela.
fn last_token(line: &str) -> (usize, bool, String) {
    let mut quote: Option<char> = None;
    let mut in_tok = false;
    let mut tok_start = line.len();
    let mut first = true;
    let mut cur_first = true;
    let mut head = String::new();
    for (i, c) in line.char_indices() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            ' ' | '\t' => {
                if in_tok && cur_first {
                    head = line[tok_start..i].to_string();
                    first = false;
                }
                in_tok = false;
            }
            '|' | ';' | '&' | '(' | '{' => {
                in_tok = false;
                first = true;
                head.clear();
            }
            _ => {
                if !in_tok {
                    in_tok = true;
                    tok_start = i;
                    cur_first = first;
                }
                if c == '"' || c == '\'' {
                    quote = Some(c);
                }
            }
        }
    }
    if in_tok {
        (tok_start, cur_first, head)
    } else {
        (line.len(), first, head)
    }
}

fn looks_path(tok: &str) -> bool {
    let t = tok.trim_start_matches(['"', '\'']);
    let b = t.as_bytes();
    t.contains(['\\', '/']) || t.starts_with('~') || (b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic())
}

/// Sugestões para a linha, só com a lista guardada e a pasta do disco.
pub fn complete(line: &str, max: usize) -> Vec<Sug> {
    let (start, cmd_pos, head) = last_token(line);
    let (before, tok) = line.split_at(start);
    if looks_path(tok) {
        return paths(before, tok, max);
    }
    if cmd_pos {
        return if tok.is_empty() { Vec::new() } else { commands(before, tok, max) };
    }
    if tok.is_empty() || tok.starts_with('-') {
        return params(before, &head, tok, max);
    }
    Vec::new()
}

fn commands(before: &str, tok: &str, max: usize) -> Vec<Sug> {
    let l = LIST.lock().unwrap();
    let t = tok.to_lowercase();
    let mut pre: Vec<&Entry> = l.all.iter().filter(|e| e.low.starts_with(&t)).collect();
    pre.sort_by(|a, b| (a.low != t, a.low.len(), &a.low).cmp(&(b.low != t, b.low.len(), &b.low)));
    let mut pick: Vec<&Entry> = pre.into_iter().take(max).collect();
    if pick.len() < max && t.chars().count() >= 3 {
        let mut mid: Vec<&Entry> = l.all.iter().filter(|e| !e.low.starts_with(&t) && e.low.contains(&t)).collect();
        mid.sort_by(|a, b| (a.low.len(), &a.low).cmp(&(b.low.len(), &b.low)));
        pick.extend(mid.into_iter().take(max - pick.len()));
    }
    pick.iter()
        .map(|e| Sug { text: format!("{before}{}", e.name), label: e.name.clone(), note: e.note.clone(), kind: e.kind })
        .collect()
}

fn params(before: &str, head: &str, tok: &str, max: usize) -> Vec<Sug> {
    let l = LIST.lock().unwrap();
    let h = head.to_lowercase();
    let ps = |e: &&Entry| matches!(e.kind, Kind::Cmdlet | Kind::Function);
    let mut cmd = l.all.iter().filter(ps).find(|e| e.low == h);
    if cmd.is_none() {
        if let Some(a) = l.all.iter().find(|e| e.kind == Kind::Alias && e.low == h) {
            let target = a.note.to_lowercase();
            cmd = l.all.iter().filter(ps).find(|e| e.low == target);
        }
    }
    let Some(cmd) = cmd else { return Vec::new() };
    let want = tok.trim_start_matches('-').to_lowercase();
    cmd.params
        .iter()
        .filter(|p| p.to_lowercase().starts_with(&want))
        .take(max)
        .map(|p| Sug { text: format!("{before}-{p}"), label: format!("-{p}"), note: cmd.name.clone(), kind: Kind::Param })
        .collect()
}

fn paths(before: &str, tok: &str, max: usize) -> Vec<Sug> {
    let raw = tok.trim_matches(['"', '\'']);
    let cut = raw.rfind(['\\', '/']).map(|i| i + 1);
    let (shown_dir, prefix) = match cut {
        Some(i) => (&raw[..i], &raw[i..]),
        None if raw.len() == 2 && raw.ends_with(':') => (raw, ""),
        None => ("", raw),
    };
    let home = crate::sys::quick::home();
    let mut dir = if let Some(rest) = shown_dir.strip_prefix('~') {
        home.join(rest.trim_start_matches(['\\', '/']))
    } else {
        PathBuf::from(shown_dir)
    };
    if shown_dir.len() == 2 && shown_dir.ends_with(':') {
        dir = PathBuf::from(format!("{shown_dir}\\"));
    } else if !dir.has_root() && !shown_dir.starts_with(['\\', '/']) {
        dir = home.join(&dir);
    }
    let sep = if shown_dir.len() == 2 && shown_dir.ends_with(':') { "\\" } else { "" };
    let want = prefix.to_lowercase();
    let mut found: Vec<(bool, String)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for f in rd.flatten().take(5000) {
            let name = f.file_name().to_string_lossy().to_string();
            if name.to_lowercase().starts_with(&want) {
                found.push((!f.file_type().is_ok_and(|t| t.is_dir()), name));
            }
        }
    }
    found.sort_by(|a, b| (a.0, a.1.to_lowercase()).cmp(&(b.0, b.1.to_lowercase())));
    found
        .into_iter()
        .take(max)
        .map(|(file, name)| {
            let mut full = format!("{shown_dir}{sep}{name}");
            if !file {
                full.push('\\');
            }
            if full.contains(' ') {
                full = format!("\"{full}\"");
            }
            let label = if file { name } else { format!("{name}\\") };
            Sug { text: format!("{before}{full}"), label, note: String::new(), kind: if file { Kind::File } else { Kind::Folder } }
        })
        .collect()
}

// ---------- Como o terminal ----------

const PS_TERM: &str = r#"$ErrorActionPreference='SilentlyContinue'
[Console]::InputEncoding=[Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
Set-Location -LiteralPath $env:USERPROFILE
while($true){
  $l=[Console]::In.ReadLine(); if($null -eq $l){break}
  $id,$b=$l.Split(' ',2)
  $o=@(); $ri=0; $rl=0
  try{
    $t=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($b))
    $r=TabExpansion2 -inputScript $t -cursorColumn $t.Length
    $ri=$r.ReplacementIndex; $rl=$r.ReplacementLength
    $o=@($r.CompletionMatches | Select-Object -First 12 | ForEach-Object { ($_.CompletionText -replace '[\t\r\n]',' ') + "`t" + ($_.ListItemText -replace '[\t\r\n]',' ') + "`t" + $_.ResultType + "`t" + (($_.ToolTip -replace '\s+',' ').Trim()) })
  }catch{}
  [Console]::Out.WriteLine("@@$id`t$ri`t$rl`t$($o.Count)")
  foreach($x in $o){[Console]::Out.WriteLine($x)}
  [Console]::Out.Flush()
}"#;

#[derive(Default)]
struct TermOut {
    pending: HashMap<u64, String>,
    line: String,
    sugs: Vec<Sug>,
}

#[derive(Default)]
struct Term {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    next: u64,
    asked: String,
    used: Option<Instant>,
    out: Arc<Mutex<TermOut>>,
    history: Vec<String>,
}

static TERM: LazyLock<Mutex<Term>> = LazyLock::new(Default::default);

/// Fecha o PowerShell escondido depois disso sem uso.
const IDLE: Duration = Duration::from_secs(120);

fn history_file() -> PathBuf {
    crate::sys::known_folder(&windows::Win32::UI::Shell::FOLDERID_RoamingAppData).join("Microsoft\\Windows\\PowerShell\\PSReadLine\\ConsoleHost_history.txt")
}

fn read_history(p: &Path) -> Vec<String> {
    let Ok(s) = std::fs::read_to_string(p) else { return Vec::new() };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for l in s.lines().rev() {
        let l = l.trim();
        if l.is_empty() || l.ends_with('`') || !seen.insert(l.to_lowercase()) {
            continue;
        }
        out.push(l.to_string());
        if out.len() >= 5000 {
            break;
        }
    }
    out
}

/// Utf-16 é a contagem do PowerShell: corta a linha nesse ponto.
fn utf16_cut(s: &str, units: usize) -> usize {
    let mut n = 0;
    for (i, c) in s.char_indices() {
        if n >= units {
            return i;
        }
        n += c.len_utf16();
    }
    s.len()
}

fn reader(out: Arc<Mutex<TermOut>>, stdout: std::process::ChildStdout, notify: isize) {
    let mut lines = BufReader::new(stdout).lines();
    while let Some(Ok(l)) = lines.next() {
        let Some(h) = l.strip_prefix("@@") else { continue };
        let f: Vec<usize> = h.split('\t').filter_map(|x| x.trim().parse().ok()).collect();
        if f.len() != 4 {
            continue;
        }
        let (id, idx, len, n) = (f[0] as u64, f[1], f[2], f[3]);
        let mut got = Vec::new();
        for _ in 0..n {
            let Some(Ok(m)) = lines.next() else { return };
            got.push(m);
        }
        let mut o = out.lock().unwrap();
        let Some(line) = o.pending.remove(&(id)) else { continue };
        o.pending.retain(|&k, _| k > id);
        let a = utf16_cut(&line, idx);
        let b = utf16_cut(&line, idx + len);
        let (before, after) = (&line[..a], &line[b..]);
        o.sugs = got
            .iter()
            .filter_map(|m| {
                let f: Vec<&str> = m.split('\t').collect();
                let (text, label, kind, tip) = (f.first()?, f.get(1).unwrap_or(&""), f.get(2).unwrap_or(&""), f.get(3).unwrap_or(&""));
                // Parâmetro do PowerShell aparece como se digita, com o tipo de nota.
                let label = if *kind == "ParameterName" { *text } else { *label };
                let kind = match *kind {
                    "ProviderContainer" => Kind::Folder,
                    "ProviderItem" => Kind::File,
                    _ => Kind::Shell,
                };
                let note = if matches!(kind, Kind::Folder | Kind::File) || **tip == *label || tip == text { String::new() } else { tip.to_string() };
                let label = if label.is_empty() { text.to_string() } else { label.to_string() };
                Some(Sug { text: format!("{before}{text}{after}"), label, note, kind })
            })
            .collect();
        o.line = line;
        drop(o);
        wake(notify);
    }
}

fn spawn(t: &mut Term, notify: isize) -> bool {
    let enc = crate::sys::quick::b64(&PS_TERM.encode_utf16().flat_map(|u| u.to_le_bytes()).collect::<Vec<u8>>());
    let Ok(mut child) = Command::new(crate::sys::quick::powershell())
        .raw_arg(format!("-NoLogo -NoProfile -NonInteractive -EncodedCommand {enc}"))
        .current_dir(crate::sys::quick::home())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(NO_WINDOW | BELOW_NORMAL)
        .spawn()
    else {
        return false;
    };
    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return false;
    };
    t.out = Arc::new(Mutex::new(TermOut::default()));
    let out = t.out.clone();
    let _ = std::thread::Builder::new().name("powershell-escondido".into()).spawn(move || reader(out, stdout, notify));
    t.child = Some(child);
    t.stdin = Some(stdin);
    t.history = read_history(&history_file());
    t.asked.clear();
    true
}

/// Pede ao PowerShell escondido as sugestões desta linha. Abre ele se preciso.
pub fn term_ask(line: &str, notify: isize) {
    let mut t = TERM.lock().unwrap();
    t.used = Some(Instant::now());
    if t.asked == line {
        return;
    }
    let alive = t.child.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)));
    if !alive {
        stop(&mut t);
        if !spawn(&mut t, notify) {
            return;
        }
    }
    t.asked = line.to_string();
    t.next += 1;
    let id = t.next;
    t.out.lock().unwrap().pending.insert(id, line.to_string());
    let msg = format!("{id} {}\n", crate::sys::quick::b64(line.as_bytes()));
    let ok = t.stdin.as_mut().is_some_and(|s| s.write_all(msg.as_bytes()).is_ok() && s.flush().is_ok());
    if !ok {
        stop(&mut t);
    }
}

/// Sugestões do PowerShell escondido, se já chegaram para esta linha.
pub fn term_get(line: &str) -> Option<Vec<Sug>> {
    let t = TERM.lock().unwrap();
    let o = t.out.lock().unwrap();
    (o.line == line).then(|| o.sugs.clone())
}

/// Linhas do histórico do PowerShell que começam com o que foi digitado.
pub fn history(line: &str, max: usize) -> Vec<Sug> {
    let t = TERM.lock().unwrap();
    let want = line.trim_start().to_lowercase();
    if want.is_empty() {
        return Vec::new();
    }
    t.history
        .iter()
        .filter(|h| h.to_lowercase().starts_with(&want) && h.len() > want.len())
        .take(max)
        .map(|h| Sug { text: h.clone(), label: h.clone(), note: String::new(), kind: Kind::History })
        .collect()
}

fn stop(t: &mut Term) {
    t.stdin = None;
    if let Some(mut c) = t.child.take() {
        // O processo é este filho, pelo identificador dele: nunca pelo nome.
        let _ = c.kill();
        let _ = c.wait();
    }
    t.asked.clear();
    t.history = Vec::new();
}

/// Fecha o PowerShell escondido.
pub fn term_stop() {
    stop(&mut TERM.lock().unwrap());
}

/// Fecha o PowerShell escondido parado há algum tempo.
pub fn term_tick() {
    let mut t = TERM.lock().unwrap();
    if t.child.is_some() && t.used.is_none_or(|u| u.elapsed() > IDLE) {
        stop(&mut t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens() {
        assert_eq!(last_token("get-proc"), (0, true, String::new()));
        assert_eq!(last_token("Get-Process -Na"), (12, false, "Get-Process".into()));
        assert_eq!(last_token("dir | sel"), (6, true, String::new()));
        assert_eq!(last_token("cd \"C:\\Program Fi"), (3, false, "cd".into()));
        assert_eq!(last_token("ping "), (5, false, "ping".into()));
    }

    #[test]
    fn help_parse() {
        let t = "Para obter mais informações\nASSOC          Exibe ou modifica associações\n               de extensões.\nATTRIB         Exibe atributos.\n\nPara obter mais\n";
        let e = parse_help(t);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].name, "assoc");
        assert_eq!(e[0].note, "Exibe ou modifica associações de extensões.");
    }

    #[test]
    fn paths_look() {
        assert!(looks_path("C:\\Win"));
        assert!(looks_path("\"C:\\Program"));
        assert!(looks_path("~\\Doc"));
        assert!(looks_path("D:"));
        assert!(!looks_path("-Name"));
        assert!(!looks_path("ipconfig"));
    }

    #[test]
    fn cuts() {
        assert_eq!(utf16_cut("ação x", 2), "aç".len());
        assert_eq!(utf16_cut("ab", 9), 2);
    }
}
