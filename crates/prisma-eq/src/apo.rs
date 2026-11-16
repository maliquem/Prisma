//! Troca de perfis com outros equalizadores: o `config.txt` do Equalizer APO (e do Peace, que usa o
//! mesmo), o `ParametricEQ.txt` e o `GraphicEQ.txt` do AutoEQ (também lido pelo Wavelet) e o
//! "Filter Settings" do REW. Importar nunca falha: o que o Prisma não faz vira aviso.

use crate::profile::{PROTECT, kind_name, num, parse_filter};
use crate::{Filter, Group, Kind, Profile, graphic_freqs};

/// Nome do canal no Equalizer APO <-> no Prisma (os outros são iguais).
const RENAMES: [(&str, &str); 2] = [("SUB", "LFE"), ("RC", "BC")];
/// `Channel: 1 2` usa a ordem do aparelho; esta é a do 7.1 do Windows.
const BY_NUMBER: [&str; 8] = ["L", "R", "C", "LFE", "RL", "RR", "SL", "SR"];

fn from_apo_name(c: &str) -> String {
    let c = c.to_ascii_uppercase();
    if let Some(n) = c.parse::<usize>().ok().filter(|n| (1..=8).contains(n)) {
        return BY_NUMBER[n - 1].into();
    }
    RENAMES.iter().find(|r| r.0 == c).map_or(c, |r| r.1.into())
}

fn to_apo_name(c: &str) -> &str {
    RENAMES.iter().find(|r| r.1 == c).map_or(c, |r| r.0)
}

/// Q de uma prateleira dada pela inclinação S (fórmula do RBJ cookbook).
fn shelf_q(gain_db: f64, slope: f64) -> f64 {
    let a = 10f64.powf(gain_db / 40.0);
    1.0 / ((a + 1.0 / a) * (1.0 / slope - 1.0) + 2.0).sqrt()
}

/// `GraphicEQ: 20 -2.1; 21 -2; ...` -> ganhos nas bandas ISO. Pontos que já são as bandas de 10, 15
/// ou 31 entram como estão; o resto é lido na curva (reta entre pontos, em escala de oitavas).
fn graphic_from_points(points: &[(f64, f64)]) -> Vec<f64> {
    for n in [10, 15, 31] {
        let (freqs, _) = graphic_freqs(n).unwrap();
        if points.len() == n && points.iter().zip(freqs).all(|(p, f)| (p.0 - f).abs() < 0.5) {
            return points.iter().map(|p| p.1).collect();
        }
    }
    let at = |f: f64| match points.iter().position(|p| p.0 >= f) {
        None => points.last().map_or(0.0, |p| p.1),
        Some(0) => points[0].1,
        Some(i) => {
            let ((f0, g0), (f1, g1)) = (points[i - 1], points[i]);
            g0 + (g1 - g0) * (f / f0).ln() / (f1 / f0).ln()
        }
    };
    graphic_freqs(31).unwrap().0.iter().map(|&f| (at(f) * 10.0).round() / 10.0).collect()
}

impl Profile {
    /// Lê um perfil do Prisma ou de outro programa. Avisos: o que ficou de fora ou aproximado.
    pub fn import(text: &str) -> (Profile, Vec<String>) {
        match Profile::parse(text) {
            Ok(p) => (p, Vec::new()),
            Err(_) => Profile::from_apo(text),
        }
    }

    /// Formato do Equalizer APO. Linhas que o Prisma não faz (Copy, Include, Device, Convolution,
    /// filtros BP/NO/AP/IIR...) ficam de fora com um aviso.
    pub fn from_apo(text: &str) -> (Profile, Vec<String>) {
        let mut p = Profile::default();
        let mut warn = Vec::new();
        // Canais do `Channel:` em vigor; vazio = todos.
        let mut sel: Vec<String> = Vec::new();
        let mut points: Vec<(f64, f64)> = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.trim();
            let mut skip = |why: &str| warn.push(format!("linha {}: {why}: {line}", n + 1));
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((cmd, arg)) = line.split_once(':') else {
                skip("não entendi");
                continue;
            };
            let (cmd, arg) = (cmd.trim(), arg.trim());
            let group = |p: &mut Profile, sel: &[String]| -> Option<usize> {
                if sel.is_empty() {
                    return None;
                }
                Some(p.groups.iter().position(|g| g.channels == sel).unwrap_or_else(|| {
                    p.groups.push(Group { channels: sel.to_vec(), ..Group::default() });
                    p.groups.len() - 1
                }))
            };
            match cmd.split_whitespace().next().unwrap_or("") {
                "Channel" => {
                    sel = arg.split_whitespace().map(from_apo_name).collect();
                    if sel.iter().any(|c| c == "ALL") {
                        sel.clear();
                    }
                }
                "Preamp" => match num(arg.trim_end_matches("dB").trim()) {
                    Ok(db) => match group(&mut p, &sel) {
                        Some(i) => p.groups[i].gain_db += db,
                        None => p.preamp_db += db,
                    },
                    Err(e) => skip(&e),
                },
                "Delay" => {
                    let (v, unit) = arg.split_once(' ').unwrap_or((arg, "ms"));
                    let ms = match (num(v), unit.trim()) {
                        (Ok(v), "ms") => v,
                        (Ok(v), "samples") => {
                            skip("atraso em amostras: contei a 48 kHz");
                            v / 48.0
                        }
                        _ => {
                            skip("atraso inválido");
                            continue;
                        }
                    };
                    match group(&mut p, &sel) {
                        Some(i) => {
                            let g = &mut p.groups[i];
                            g.delay_ms = (g.delay_ms + ms).clamp(0.0, crate::MAX_DELAY_MS);
                        }
                        None => skip("atraso em todos os canais não muda nada: ignorei"),
                    }
                }
                "Filter" => {
                    let mut f = match parse_filter(arg) {
                        Ok(f) => f,
                        Err(e) => {
                            skip(&format!("{e}, ficou de fora"));
                            continue;
                        }
                    };
                    let words: Vec<&str> = arg.split_whitespace().collect();
                    let has_q = words.iter().any(|w| w.eq_ignore_ascii_case("Q") || w.eq_ignore_ascii_case("BW"));
                    if matches!(f.kind, Kind::LowShelf | Kind::HighShelf) && !has_q {
                        if words.get(2).is_some_and(|w| w.eq_ignore_ascii_case("6dB")) {
                            // shortcut: prateleira de 1ª ordem vira de 2ª com Q 0,5; fazer a de 1ª se alguém usar.
                            skip("prateleira de 6 dB/oitava aproximada");
                            f.q = 0.5;
                        } else if !words.get(2).is_some_and(|w| w.eq_ignore_ascii_case("12dB")) {
                            f.q = shelf_q(f.gain_db, 0.9); // LS/HS sem Q: inclinação 0,9 do Equalizer APO
                        }
                    }
                    match group(&mut p, &sel) {
                        Some(i) => p.groups[i].filters.push(f),
                        None => p.filters.push(f),
                    }
                }
                "GraphicEQ" => {
                    if !sel.is_empty() {
                        skip("GraphicEQ de um canal só: usei em todos");
                    }
                    for pair in arg.split(';') {
                        let mut w = pair.split_whitespace().map(num);
                        if let (Some(Ok(f)), Some(Ok(g))) = (w.next(), w.next()) {
                            points.push((f, g));
                        }
                    }
                }
                _ => skip("o Prisma não faz isso, ficou de fora"),
            }
        }
        if !points.is_empty() {
            points.sort_by(|a, b| a.0.total_cmp(&b.0));
            p.graphic = graphic_from_points(&points);
        }
        // A proteção que o Prisma exporta volta como proteção.
        let prot: Vec<Filter> = PROTECT.iter().map(|&(kind, freq, q)| Filter { on: true, kind, freq, gain_db: 0.0, q }).collect();
        if let Some(i) = p.filters.windows(4).position(|w| w.iter().zip(&prot).all(|(a, b)| a.kind == b.kind && a.on && a.freq == b.freq && (a.q - b.q).abs() < 1e-3)) {
            p.filters.drain(i..i + 4);
            p.protect = true;
        }
        (p, warn)
    }

    /// Texto para o `config.txt` do Equalizer APO (e Peace). Proteção e gráfico viram filtros, para
    /// o som sair igual ao do Prisma.
    pub fn to_apo(&self) -> String {
        let mut s = String::from("# Exportado do Prisma\n");
        s += &format!("Preamp: {} dB\n", self.preamp_db);
        let filter = |s: &mut String, f: &Filter| {
            let on = if f.on { "ON" } else { "OFF" };
            let gain = if matches!(f.kind, Kind::LowPass | Kind::HighPass) { String::new() } else { format!(" Gain {} dB", f.gain_db) };
            *s += &format!("Filter: {on} {} Fc {} Hz{gain} Q {}\n", kind_name(f.kind), f.freq, f.q);
        };
        // Só os filtros fixos (proteção e gráfico): os do perfil vêm depois, com os desligados.
        let fixed = Profile { protect: self.protect, graphic: self.graphic.clone(), ..Profile::default() };
        for f in fixed.filters_for("") {
            filter(&mut s, &f);
        }
        for f in &self.filters {
            filter(&mut s, f);
        }
        for g in &self.groups {
            let names: Vec<&str> = g.channels.iter().map(|c| to_apo_name(c)).collect();
            s += &format!("Channel: {}\n", names.join(" "));
            if g.gain_db != 0.0 {
                s += &format!("Preamp: {} dB\n", g.gain_db);
            }
            if g.delay_ms != 0.0 {
                s += &format!("Delay: {} ms\n", g.delay_ms);
            }
            for f in &g.filters {
                filter(&mut s, f);
            }
        }
        if !self.groups.is_empty() {
            s += "Channel: all\n";
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;

    fn same_sound(a: &Profile, b: &Profile, ch: &str) {
        for f in [15.0, 30.0, 100.0, 440.0, 1000.0, 3000.0, 9000.0, 16000.0, 21000.0] {
            let (x, y) = (a.response_db(ch, f, RATE), b.response_db(ch, f, RATE));
            assert!((x - y).abs() < 1e-6, "{ch} {f} Hz: {x} != {y}");
        }
        assert_eq!(a.gain_delay_for(ch), b.gain_delay_for(ch));
    }

    #[test]
    fn exportar_e_importar_da_o_mesmo_som() {
        let p = Profile::parse(
            "preamp=-4\nprotecao=sim\ngrafico=0 0 1 2 3 2 0 0 -1 -2\nFilter 1: ON LSC Fc 80 Hz Gain 3 dB Q 0.7\nFilter 2: OFF PK Fc 9000 Hz Gain 9 dB Q 2\n[C LFE] ganho=2 atraso=3\nFilter 1: ON PK Fc 2500 Hz Gain 2 dB Q 1\n[RL RR] atraso=8\n",
        )
        .unwrap();
        let text = p.to_apo();
        assert!(text.contains("Channel: C SUB\n") && text.contains("Delay: 8 ms\n") && text.ends_with("Channel: all\n"), "{text}");
        let (back, warn) = Profile::from_apo(&text);
        assert!(warn.is_empty(), "{warn:?}");
        assert!(back.protect);
        assert_eq!(back.filters.iter().filter(|f| !f.on).count(), 1);
        for ch in ["L", "C", "LFE", "RR"] {
            same_sound(&p, &back, ch);
        }
    }

    #[test]
    fn le_config_do_equalizer_apo() {
        let text = "Device: Speakers\nPreamp: -6 dB\nFilter 1: ON PK Fc 1000 Hz Gain 3 dB BW Oct 1\nFilter: ON LS Fc 100 Hz Gain 4 dB\nFilter: ON BP Fc 500 Hz Q 2\nChannel: SUB\nPreamp: 2 dB\nFilter: ON LP Fc 120 Hz\nChannel: 7 8\nDelay: 5 ms\nChannel: all\nCopy: L=R\n";
        let (p, warn) = Profile::import(text);
        assert_eq!(p.preamp_db, -6.0);
        assert_eq!(p.filters.len(), 2);
        assert!((p.filters[0].q - 1.414).abs() < 0.01, "BW Oct 1 = Q 1,41");
        assert!((p.filters[1].q - shelf_q(4.0, 0.9)).abs() < 1e-9);
        assert_eq!(p.gain_delay_for("LFE"), (-4.0, 0.0));
        assert_eq!(p.gain_delay_for("SR"), (-6.0, 5.0));
        assert_eq!(p.filters_for("LFE").len(), 3);
        assert_eq!(warn.len(), 3, "Device, BP e Copy: {warn:?}");
        assert!(warn[1].starts_with("linha 5:"));
    }

    #[test]
    fn importa_o_formato_do_prisma_sem_avisos() {
        let (p, warn) = Profile::import("preamp=-2\n[C] ganho=1\n");
        assert!(warn.is_empty());
        assert_eq!(p.groups[0].gain_db, 1.0);
    }

    #[test]
    fn graphic_eq_vira_31_bandas() {
        let (p, warn) = Profile::from_apo("GraphicEQ: 20 0; 100 6; 1000 0; 20000 -4");
        assert!(warn.is_empty());
        assert_eq!(p.graphic.len(), 31);
        assert_eq!(p.graphic[7], 6.0, "100 Hz");
        assert_eq!(p.graphic[17], 0.0, "1 kHz");
        assert_eq!(p.graphic[30], -4.0, "20 kHz");
        let g10 = "GraphicEQ: 31.5 1; 63 2; 125 3; 250 4; 500 5; 1000 6; 2000 7; 4000 8; 8000 9; 16000 10";
        assert_eq!(Profile::from_apo(g10).0.graphic, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0]);
    }
}
