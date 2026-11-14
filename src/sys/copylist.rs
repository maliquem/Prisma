//! Listas de cópia (arquivos e pastas de vários lugares para um destino, com agenda) e o histórico
//! das cópias. Tudo em texto na pasta de dados do Prisma. Nada é registrado no Windows: a agenda
//! roda com o Prisma aberto, e um horário perdido (PC desligado) roda quando o Prisma abrir.
//!
//! Horários em minutos locais desde 1º/1/1970 (`now`): a agenda segue o relógio que a pessoa vê.

use std::path::{Path, PathBuf};

use super::copy::{KEEP_BOTH, SKIP};

/// Quando a lista roda. Minuto do dia de 0 a 1439; dias da semana em bits, bit 0 = domingo.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum When {
    Manual,
    Once(i64),
    Daily(u16),
    Weekly(u8, u16),
}

#[derive(Clone, PartialEq, Debug)]
pub struct Job {
    pub id: u64,
    pub name: String,
    pub items: Vec<PathBuf>,
    pub dest: PathBuf,
    pub when: When,
    /// `SKIP`, `REPLACE` ou `KEEP_BOTH`: sem ninguém olhando, nunca `ASK`.
    pub policy: u8,
    pub verify: bool,
    pub mv: bool,
    /// Quando rodou ou foi salva pela última vez: a agenda só conta horários depois disso.
    pub last: i64,
}

/// Resultado de uma cópia no histórico.
pub const OK: u8 = 0;
pub const VERIFIED: u8 = 1;
pub const ERRORS: u8 = 2;
pub const CANCELED: u8 = 3;
pub const NO_SPACE: u8 = 4;

/// Cópias guardadas no histórico; o arquivo é aparado ao passar de `KEEP + 100`.
pub const KEEP: usize = 500;
/// Erros guardados por cópia.
const MAX_ERRORS: usize = 50;

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Record {
    pub at: i64,
    pub mv: bool,
    pub result: u8,
    pub bytes: u64,
    pub files: u32,
    /// Nome da lista, vazio numa cópia avulsa.
    pub name: String,
    pub dest: String,
    pub items: Vec<String>,
    /// (arquivo, motivo).
    pub errors: Vec<(String, String)>,
}

pub fn jobs_path() -> PathBuf {
    crate::config::data_dir().join("copias.txt")
}

pub fn history_path() -> PathBuf {
    crate::config::data_dir().join("copias_historico.txt")
}

/// Tab, quebra de linha e `|` separam campos; em nome e motivo viram espaço.
fn clean(s: &str) -> String {
    s.chars().map(|c| if matches!(c, '\t' | '\n' | '\r' | '|' | '\u{1f}') { ' ' } else { c }).collect()
}

fn paths(v: &str) -> Vec<PathBuf> {
    v.split('|').filter(|s| !s.is_empty()).map(PathBuf::from).collect()
}

fn join(items: &[PathBuf]) -> String {
    items.iter().map(|p| clean(&p.to_string_lossy())).collect::<Vec<_>>().join("|")
}

impl When {
    fn encode(self) -> String {
        match self {
            When::Manual => "m".into(),
            When::Once(t) => format!("o{t}"),
            When::Daily(m) => format!("d{m}"),
            When::Weekly(d, m) => format!("w{d},{m}"),
        }
    }

    fn parse(v: &str) -> Option<When> {
        let minute = |s: &str| s.parse::<u16>().ok().filter(|&m| m < 1440);
        Some(match v.split_at_checked(1)? {
            ("m", _) => When::Manual,
            ("o", t) => When::Once(t.parse().ok()?),
            ("d", m) => When::Daily(minute(m)?),
            ("w", r) => {
                let (d, m) = r.split_once(',')?;
                When::Weekly(d.parse::<u8>().ok()? & 0x7f, minute(m)?)
            }
            _ => return None,
        })
    }
}

impl Job {
    pub fn line(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.id,
            clean(&self.name),
            clean(&self.dest.to_string_lossy()),
            self.when.encode(),
            self.policy,
            self.verify as u8,
            self.mv as u8,
            self.last,
            join(&self.items)
        )
    }

    pub fn parse(line: &str) -> Option<Job> {
        let f: Vec<&str> = line.split('\t').collect();
        let [id, name, dest, when, policy, verify, mv, last, items] = f[..] else { return None };
        let items = paths(items);
        if items.is_empty() || dest.is_empty() {
            return None;
        }
        let policy = policy.parse::<u8>().ok().filter(|p| (SKIP..=KEEP_BOTH).contains(p)).unwrap_or(SKIP);
        Some(Job { id: id.parse().ok()?, name: name.into(), items, dest: dest.into(), when: When::parse(when)?, policy, verify: verify == "1", mv: mv == "1", last: last.parse().ok()? })
    }
}

pub fn load_jobs(path: &Path) -> Vec<Job> {
    std::fs::read_to_string(path).map(|t| t.lines().filter_map(Job::parse).collect()).unwrap_or_default()
}

pub fn save_jobs(path: &Path, jobs: &[Job]) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let text: String = jobs.iter().map(|j| j.line() + "\n").collect();
    crate::config::write_safe(path, text)
}

/// Uma cópia para a janela de cópia rodar (`prisma.exe --copy-file=`): grava a lista num
/// arquivo temporário, que a janela lê e apaga.
pub fn spec_file(job: &Job) -> std::io::Result<PathBuf> {
    let p = std::env::temp_dir().join(format!("prisma_copia_{}_{}.txt", std::process::id(), job.id));
    std::fs::write(&p, job.line())?;
    Ok(p)
}

pub fn read_spec(path: &Path) -> Option<Job> {
    let text = std::fs::read_to_string(path).ok()?;
    let _ = std::fs::remove_file(path);
    Job::parse(text.lines().next()?)
}

// --- agenda ---

/// Dia da semana de um dia desde 1970, 0 = domingo (1º/1/1970 foi quinta).
fn weekday(day: i64) -> i64 {
    (day + 4).rem_euclid(7)
}

/// Último horário marcado até `now` (inclusive).
pub fn last_slot(when: When, now: i64) -> Option<i64> {
    let day = now.div_euclid(1440);
    match when {
        When::Manual => None,
        When::Once(t) => (t <= now).then_some(t),
        When::Daily(m) => {
            let t = day * 1440 + m as i64;
            Some(if t <= now { t } else { t - 1440 })
        }
        When::Weekly(days, m) => (0..8).map(|k| day - k).filter(|&d| days & (1 << weekday(d)) != 0).map(|d| d * 1440 + m as i64).find(|&t| t <= now),
    }
}

/// Próximo horário marcado depois de `now`.
pub fn next_slot(when: When, now: i64) -> Option<i64> {
    let day = now.div_euclid(1440);
    match when {
        When::Manual => None,
        When::Once(t) => (t > now).then_some(t),
        When::Daily(m) => {
            let t = day * 1440 + m as i64;
            Some(if t > now { t } else { t + 1440 })
        }
        When::Weekly(days, m) => (0..8).map(|k| day + k).filter(|&d| days & (1 << weekday(d)) != 0).map(|d| d * 1440 + m as i64).find(|&t| t > now),
    }
}

/// A lista tem um horário que passou depois da última vez. Vários perdidos rodam uma vez só.
pub fn due(job: &Job, now: i64) -> bool {
    last_slot(job.when, now).is_some_and(|t| t > job.last)
}

/// Dias desde 1970 de uma data (algoritmo de Howard Hinnant).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// (ano, mês, dia, hora, minuto) de um horário em minutos locais.
pub fn civil(t: i64) -> (i64, u32, u32, u32, u32) {
    let z = t.div_euclid(1440) + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + (m <= 2) as i64;
    let min = t.rem_euclid(1440) as u32;
    (y, m, d, min / 60, min % 60)
}

/// Agora, em minutos locais.
pub fn now() -> i64 {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    days_from_civil(t.wYear as i64, t.wMonth as u32, t.wDay as u32) * 1440 + t.wHour as i64 * 60 + t.wMinute as i64
}

/// Próxima vez em que `dia/mês hora:minuto` acontece depois de `now` (este ano ou o próximo).
/// Dia que não existe no mês (31/4) passa para o seguinte, como o calendário conta.
pub fn once_at(now: i64, month: u32, day: u32, minute: u16) -> i64 {
    let y = civil(now).0;
    let at = |y: i64| days_from_civil(y, month, 1) * 1440 + (day as i64 - 1) * 1440 + minute as i64;
    if at(y) > now { at(y) } else { at(y + 1) }
}

// --- histórico ---

impl Record {
    pub fn line(&self) -> String {
        let errors: Vec<String> = self.errors.iter().take(MAX_ERRORS).map(|(p, w)| format!("{}\u{1f}{}", clean(p), clean(w))).collect();
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.at,
            if self.mv { "m" } else { "c" },
            self.result,
            self.bytes,
            self.files,
            clean(&self.name),
            clean(&self.dest),
            self.items.iter().map(|s| clean(s)).collect::<Vec<_>>().join("|"),
            errors.join("|")
        )
    }

    pub fn parse(line: &str) -> Option<Record> {
        let f: Vec<&str> = line.split('\t').collect();
        let [at, mv, result, bytes, files, name, dest, items, errors] = f[..] else { return None };
        Some(Record {
            at: at.parse().ok()?,
            mv: mv == "m",
            result: result.parse().ok().filter(|&r| r <= NO_SPACE)?,
            bytes: bytes.parse().ok()?,
            files: files.parse().ok()?,
            name: name.into(),
            dest: dest.into(),
            items: items.split('|').filter(|s| !s.is_empty()).map(String::from).collect(),
            errors: errors.split('|').filter_map(|e| e.split_once('\u{1f}')).map(|(p, w)| (p.into(), w.into())).collect(),
        })
    }
}

/// Acrescenta uma cópia. A janela de cópia é outro processo: acrescentar no fim não atropela ninguém.
pub fn append(path: &Path, r: &Record) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", r.line())?;
    drop(f);
    let text = std::fs::read_to_string(path)?;
    let n = text.lines().count();
    if n > KEEP + 100 {
        let kept: String = text.lines().skip(n - KEEP).map(|l| format!("{l}\n")).collect();
        crate::config::write_safe(path, kept)?;
    }
    Ok(())
}

/// O histórico, da cópia mais nova para a mais velha.
pub fn load_history(path: &Path) -> Vec<Record> {
    let mut v: Vec<Record> = std::fs::read_to_string(path).map(|t| t.lines().filter_map(Record::parse).collect()).unwrap_or_default();
    v.reverse();
    v
}

pub fn clear_history(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::copy::REPLACE;

    fn at(y: i64, m: u32, d: u32, h: i64, min: i64) -> i64 {
        days_from_civil(y, m, d) * 1440 + h * 60 + min
    }

    fn job(when: When, last: i64) -> Job {
        Job { id: 7, name: "Projetos à noite".into(), items: vec!["C:\\Projetos".into(), "D:\\notas.xlsx".into()], dest: "\\\\NAS\\backup".into(), when, policy: REPLACE, verify: true, mv: false, last }
    }

    #[test]
    fn calendar_round_trips() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(weekday(days_from_civil(2026, 11, 14)), 6, "14/11/2026 é sábado");
        for t in [0, at(2024, 2, 29, 23, 59), at(2026, 12, 31, 0, 0), at(2000, 3, 1, 12, 30)] {
            let (y, m, d, h, mi) = civil(t);
            assert_eq!(at(y, m, d, h as i64, mi as i64), t);
        }
        assert_eq!(civil(at(2024, 2, 29, 7, 5)), (2024, 2, 29, 7, 5));
    }

    #[test]
    fn daily_runs_once_per_slot() {
        let when = When::Daily(3 * 60);
        let saved = at(2026, 11, 14, 15, 0);
        // Salva às 15h: o horário das 3h de hoje já tinha passado e não conta.
        assert!(!due(&job(when, saved), saved));
        assert!(!due(&job(when, saved), at(2026, 11, 15, 2, 59)));
        assert!(due(&job(when, saved), at(2026, 11, 15, 3, 0)));
        // Rodou às 3h: não roda de novo no mesmo dia.
        assert!(!due(&job(when, at(2026, 11, 15, 3, 0)), at(2026, 11, 15, 23, 0)));
        // PC desligado por três dias: roda uma vez ao abrir.
        assert!(due(&job(when, saved), at(2026, 11, 18, 9, 0)));
        assert_eq!(next_slot(when, saved), Some(at(2026, 11, 15, 3, 0)));
        assert_eq!(next_slot(when, at(2026, 11, 15, 3, 0)), Some(at(2026, 11, 16, 3, 0)));
    }

    #[test]
    fn weekdays_follow_the_mask() {
        // Segunda, quarta e sexta às 22h.
        let when = When::Weekly(0b010_1010, 22 * 60);
        let sat = at(2026, 11, 14, 12, 0);
        assert_eq!(next_slot(when, sat), Some(at(2026, 11, 16, 22, 0)));
        assert_eq!(last_slot(when, sat), Some(at(2026, 11, 13, 22, 0)));
        assert!(!due(&job(when, sat), at(2026, 11, 16, 21, 59)));
        assert!(due(&job(when, sat), at(2026, 11, 16, 22, 0)));
        assert_eq!(next_slot(When::Weekly(0, 60), sat), None);
        assert!(!due(&job(When::Weekly(0, 60), 0), sat));
    }

    #[test]
    fn once_and_manual() {
        let now = at(2026, 11, 14, 12, 0);
        let t = once_at(now, 11, 20, 22 * 60);
        assert_eq!(t, at(2026, 11, 20, 22, 0));
        // Data que já passou neste ano vai para o próximo.
        assert_eq!(once_at(now, 1, 5, 0), at(2027, 1, 5, 0, 0));
        assert!(!due(&job(When::Once(t), now), now));
        assert!(due(&job(When::Once(t), now), t + 5));
        assert!(!due(&job(When::Once(t), t + 5), t + 600));
        assert!(!due(&job(When::Manual, 0), now));
        assert_eq!(next_slot(When::Manual, now), None);
    }

    #[test]
    fn job_lines_round_trip() {
        for when in [When::Manual, When::Once(29_000_000), When::Daily(180), When::Weekly(0b1010, 1320)] {
            let j = job(when, 123);
            assert_eq!(Job::parse(&j.line()), Some(j));
        }
        // Tab e `|` no nome não quebram o arquivo.
        let mut j = job(When::Manual, 0);
        j.name = "a\tb|c".into();
        assert_eq!(Job::parse(&j.line()).unwrap().name, "a b c");
        // Sem pedir ("ASK") não serve para uma lista: vira Pular.
        let line = job(When::Manual, 0).line().replacen("\t2\t1\t0\t", "\t0\t1\t0\t", 1);
        assert_eq!(Job::parse(&line).unwrap().policy, SKIP);
        assert_eq!(Job::parse("lixo"), None);
        assert_eq!(Job::parse(&job(When::Manual, 0).line().replace("C:\\Projetos|D:\\notas.xlsx", "")), None);
    }

    #[test]
    fn history_keeps_the_newest() {
        let dir = std::env::temp_dir().join(format!("prisma_historico_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join("h.txt");
        let rec = |i: i64| Record { at: i, result: ERRORS, bytes: 10, files: 1, dest: "E:\\Backup".into(), items: vec!["D:\\Fotos".into()], errors: vec![("D:\\Fotos\\a.jpg".into(), "Acesso negado".into())], ..Default::default() };
        assert_eq!(Record::parse(&rec(5).line()), Some(rec(5)));
        for i in 0..(KEEP as i64 + 101) {
            append(&p, &rec(i)).unwrap();
        }
        let h = load_history(&p);
        assert_eq!(h.len(), KEEP);
        assert_eq!(h[0].at, KEEP as i64 + 100);
        assert_eq!(h.last().unwrap().at, 101);
        clear_history(&p).unwrap();
        assert!(load_history(&p).is_empty());
        clear_history(&p).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn spec_file_is_read_once() {
        let j = job(When::Daily(60), 9);
        let p = spec_file(&j).unwrap();
        assert_eq!(read_spec(&p), Some(j));
        assert!(!p.exists());
    }
}
