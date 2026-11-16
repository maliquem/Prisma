//! Perfil em texto, no jeito do AutoEQ / Equalizer APO, para importar perfis de fone sem esforço:
//!
//! ```text
//! preamp=-4                 (ou "Preamp: -4 dB", como vem do AutoEQ)
//! protecao=sim              (corta o que fica abaixo e acima do audível; opcional)
//! limitador=nao             (o limitador vem ligado: segura os picos quando o ganho passa de 0 dB)
//! grafico=0 0 1 2 3 2 0 0 -1 -2   (10, 15 ou 31 ganhos; opcional)
//! Filter 1: ON LSC Fc 80 Hz Gain 3 dB Q 0.7
//! [C] ganho=2 atraso=0      (daqui para baixo, só nesses canais)
//! Filter 1: ON PK Fc 2500 Hz Gain 2 dB Q 1
//! [RL RR] atraso=8
//! ```

use crate::Kind;

#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    pub on: bool,
    pub kind: Kind,
    pub freq: f64,
    pub gain_db: f64,
    pub q: f64,
}

/// Ajustes de alguns canais: filtros a mais, ganho e atraso.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Group {
    pub channels: Vec<String>,
    pub gain_db: f64,
    pub delay_ms: f64,
    pub filters: Vec<Filter>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    /// Ganho geral; acima de 0 dB o limitador segura os picos.
    pub preamp_db: f64,
    /// Corte do subsônico e do ultrassônico (`PROTECT_*`), antes dos outros filtros.
    pub protect: bool,
    /// Limitador no fim (`chain::LIMIT_DB`). Ligado por padrão.
    pub limiter: bool,
    /// Ganhos do modo gráfico (10, 15 ou 31), somados aos filtros.
    pub graphic: Vec<f64>,
    /// Filtros de todos os canais.
    pub filters: Vec<Filter>,
    pub groups: Vec<Group>,
}

impl Default for Profile {
    fn default() -> Self {
        Profile { preamp_db: 0.0, protect: false, limiter: true, graphic: Vec::new(), filters: Vec::new(), groups: Vec::new() }
    }
}

/// Proteção: passa-altas em 20 Hz e passa-baixas em 20 kHz, Butterworth de 4ª ordem (24 dB/oitava,
/// dois biquads com Q 0,541 e 1,307 cada). Custa ~0,2 dB em 30 Hz e ~0,7 dB em 16 kHz.
pub const PROTECT_HP_HZ: f64 = 20.0;
pub const PROTECT_LP_HZ: f64 = 20_000.0;
const BW4_Q: [f64; 2] = [0.541_196_1, 1.306_563];
pub(crate) const PROTECT: [(Kind, f64, f64); 4] = [
    (Kind::HighPass, PROTECT_HP_HZ, BW4_Q[0]),
    (Kind::HighPass, PROTECT_HP_HZ, BW4_Q[1]),
    (Kind::LowPass, PROTECT_LP_HZ, BW4_Q[0]),
    (Kind::LowPass, PROTECT_LP_HZ, BW4_Q[1]),
];

/// Frequências ISO do modo gráfico e o Q da largura de cada banda (1, 2/3 ou 1/3 de oitava).
pub fn graphic_freqs(bands: usize) -> Option<(&'static [f64], f64)> {
    const B10: [f64; 10] = [31.5, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];
    const B15: [f64; 15] = [25.0, 40.0, 63.0, 100.0, 160.0, 250.0, 400.0, 630.0, 1000.0, 1600.0, 2500.0, 4000.0, 6300.0, 10000.0, 16000.0];
    const B31: [f64; 31] = [
        20.0, 25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0, 500.0, 630.0, 800.0, 1000.0, 1250.0, 1600.0, 2000.0,
        2500.0, 3150.0, 4000.0, 5000.0, 6300.0, 8000.0, 10000.0, 12500.0, 16000.0, 20000.0,
    ];
    // Q de uma banda de N oitavas: sqrt(2^N) / (2^N - 1).
    let q = |n: f64| 2f64.powf(n).sqrt() / (2f64.powf(n) - 1.0);
    match bands {
        10 => Some((&B10, q(1.0))),
        15 => Some((&B15, q(2.0 / 3.0))),
        31 => Some((&B31, q(1.0 / 3.0))),
        _ => None,
    }
}

fn kind_of(t: &str) -> Option<Kind> {
    Some(match t {
        "PK" | "PEQ" => Kind::Peak,
        "LSC" | "LS" => Kind::LowShelf,
        "HSC" | "HS" => Kind::HighShelf,
        "LP" | "LPQ" => Kind::LowPass,
        "HP" | "HPQ" => Kind::HighPass,
        _ => return None,
    })
}

pub(crate) fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Peak => "PK",
        Kind::LowShelf => "LSC",
        Kind::HighShelf => "HSC",
        Kind::LowPass => "LPQ",
        Kind::HighPass => "HPQ",
    }
}

pub(crate) fn num(s: &str) -> Result<f64, String> {
    // Aceita vírgula decimal (perfil escrito à mão no Windows em português).
    s.replace(',', ".").parse::<f64>().ok().filter(|v| v.is_finite()).ok_or_else(|| format!("número inválido: {s}"))
}

/// `ON PK Fc 250 Hz Gain -2 dB Q 1.2` (o que vem depois de "Filter N:").
pub(crate) fn parse_filter(rest: &str) -> Result<Filter, String> {
    let t: Vec<&str> = rest.split_whitespace().collect();
    let on = match t.first() {
        Some(&"ON") => true,
        Some(&"OFF") => false,
        _ => return Err("falta ON ou OFF".into()),
    };
    let kind = t.get(1).and_then(|k| kind_of(k)).ok_or("tipo de filtro desconhecido")?;
    let after = |name: &str| t.iter().position(|w| w.eq_ignore_ascii_case(name)).and_then(|i| t.get(i + 1));
    let freq = num(after("Fc").ok_or("falta Fc")?)?;
    if freq <= 0.0 {
        return Err("Fc precisa ser maior que zero".into());
    }
    let gain_db = after("Gain").map(|g| num(g)).transpose()?.unwrap_or(0.0);
    // "BW Oct 1" (largura em oitavas, do Equalizer APO) vira Q.
    let bw = t.iter().position(|w| w.eq_ignore_ascii_case("BW")).and_then(|i| t.get(i + 2)).map(|n| num(n)).transpose()?;
    let q = match (after("Q"), bw) {
        (Some(q), _) => num(q)?,
        (None, Some(n)) if n > 0.0 => 2f64.powf(n).sqrt() / (2f64.powf(n) - 1.0),
        _ => std::f64::consts::FRAC_1_SQRT_2,
    };
    if q <= 0.0 {
        return Err("Q precisa ser maior que zero".into());
    }
    Ok(Filter { on, kind, freq, gain_db, q })
}

/// `ganho=2 atraso=8`
fn parse_settings(g: &mut Group, words: &[&str]) -> Result<(), String> {
    for w in words {
        match w.split_once('=') {
            Some(("ganho", v)) => g.gain_db = num(v)?,
            Some(("atraso", v)) => g.delay_ms = num(v)?.clamp(0.0, crate::MAX_DELAY_MS),
            _ => return Err(format!("ajuste desconhecido: {w}")),
        }
    }
    Ok(())
}

impl Profile {
    /// Lê o texto. Erro diz a linha (contando de 1) e o motivo.
    pub fn parse(text: &str) -> Result<Profile, String> {
        let mut p = Profile::default();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.trim();
            let fail = |e: String| format!("linha {}: {e}", n + 1);
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix('[') {
                let (names, settings) = rest.split_once(']').ok_or_else(|| fail("falta ]".into()))?;
                let channels: Vec<String> = names.split_whitespace().map(|c| c.to_ascii_uppercase()).collect();
                if channels.is_empty() {
                    return Err(fail("grupo sem canais".into()));
                }
                let mut g = Group { channels, ..Group::default() };
                parse_settings(&mut g, &settings.split_whitespace().collect::<Vec<_>>()).map_err(fail)?;
                p.groups.push(g);
            } else if let Some(rest) = line.strip_prefix("Filter") {
                let (_, f) = rest.split_once(':').ok_or_else(|| fail("falta : depois de Filter".into()))?;
                let f = parse_filter(f).map_err(fail)?;
                match p.groups.last_mut() {
                    Some(g) => g.filters.push(f),
                    None => p.filters.push(f),
                }
            } else if let Some(v) = line.strip_prefix("preamp=").or_else(|| line.strip_prefix("Preamp:")) {
                p.preamp_db = num(v.trim().trim_end_matches("dB").trim()).map_err(fail)?;
            } else if let Some(v) = line.strip_prefix("grafico=") {
                let gains = v.split_whitespace().map(num).collect::<Result<Vec<_>, _>>().map_err(fail)?;
                if graphic_freqs(gains.len()).is_none() {
                    return Err(fail(format!("o gráfico tem {} ganhos; precisa de 10, 15 ou 31", gains.len())));
                }
                p.graphic = gains;
            } else if let Some(v) = line.strip_prefix("protecao=") {
                p.protect = match v.trim() {
                    "sim" => true,
                    "nao" | "não" => false,
                    _ => return Err(fail(format!("protecao=sim ou protecao=nao, não {v}"))),
                };
            } else if let Some(v) = line.strip_prefix("limitador=") {
                p.limiter = match v.trim() {
                    "sim" => true,
                    "nao" | "não" => false,
                    _ => return Err(fail(format!("limitador=sim ou limitador=nao, não {v}"))),
                };
            } else if line.starts_with("modo=") {
                // Só para a página lembrar a aba; o som é o mesmo.
            } else if let Some(g) = p.groups.last_mut() {
                parse_settings(g, &line.split_whitespace().collect::<Vec<_>>()).map_err(fail)?;
            } else {
                return Err(fail(format!("não entendi: {line}")));
            }
        }
        Ok(p)
    }

    /// Volta para texto; `parse(to_text())` dá o mesmo perfil.
    pub fn to_text(&self) -> String {
        let mut s = format!("preamp={}\n", self.preamp_db);
        if self.protect {
            s += "protecao=sim\n";
        }
        if !self.limiter {
            s += "limitador=nao\n";
        }
        if !self.graphic.is_empty() {
            let g: Vec<String> = self.graphic.iter().map(|v| v.to_string()).collect();
            s += &format!("grafico={}\n", g.join(" "));
        }
        let filters = |s: &mut String, fs: &[Filter]| {
            for (i, f) in fs.iter().enumerate() {
                let on = if f.on { "ON" } else { "OFF" };
                *s += &format!("Filter {}: {on} {} Fc {} Hz Gain {} dB Q {}\n", i + 1, kind_name(f.kind), f.freq, f.gain_db, f.q);
            }
        };
        filters(&mut s, &self.filters);
        for g in &self.groups {
            s += &format!("[{}] ganho={} atraso={}\n", g.channels.join(" "), g.gain_db, g.delay_ms);
            filters(&mut s, &g.filters);
        }
        s
    }

    /// Todos os filtros que valem num canal (gráfico + comuns + os dos grupos daquele canal).
    pub fn filters_for(&self, channel: &str) -> Vec<Filter> {
        let mut out = Vec::new();
        if self.protect {
            out.extend(PROTECT.iter().map(|&(kind, freq, q)| Filter { on: true, kind, freq, gain_db: 0.0, q }));
        }
        if let Some((freqs, q)) = graphic_freqs(self.graphic.len()) {
            for (&freq, &gain_db) in freqs.iter().zip(&self.graphic) {
                if gain_db != 0.0 {
                    out.push(Filter { on: true, kind: Kind::Peak, freq, gain_db, q });
                }
            }
        }
        let groups = self.groups.iter().filter(|g| g.channels.iter().any(|c| c == channel));
        out.extend(self.filters.iter().chain(groups.flat_map(|g| &g.filters)).filter(|f| f.on).cloned());
        out
    }

    /// Ganho (dB, com o preamp) e atraso (ms) de um canal.
    pub fn gain_delay_for(&self, channel: &str) -> (f64, f64) {
        self.groups.iter().filter(|g| g.channels.iter().any(|c| c == channel)).fold((self.preamp_db, 0.0), |(g, d), gr| (g + gr.gain_db, d + gr.delay_ms))
    }

    /// Curva de um canal em dB, para desenhar (inclui o preamp e o ganho do canal).
    pub fn response_db(&self, channel: &str, freq: f64, rate: f64) -> f64 {
        let filters: f64 = self.filters_for(channel).iter().map(|f| crate::Biquad::new(f.kind, f.freq, f.gain_db, f.q, rate).response_db(freq, rate)).sum();
        filters + self.gain_delay_for(channel).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXEMPLO: &str = "modo=parametrico
preamp=-4
Filter 1: ON LSC Fc 80 Hz Gain 3 dB Q 0.7
Filter 2: ON PK Fc 250 Hz Gain -2 dB Q 1.2
Filter 3: OFF PK Fc 9000 Hz Gain 9 dB Q 2
[C] ganho=2 atraso=0
Filter 1: ON PK Fc 2500 Hz Gain 2 dB Q 1
[RL RR] atraso=8
";

    #[test]
    fn le_o_exemplo_do_plano() {
        let p = Profile::parse(EXEMPLO).unwrap();
        assert_eq!(p.preamp_db, -4.0);
        assert_eq!(p.filters.len(), 3);
        assert_eq!(p.groups.len(), 2);
        assert_eq!(p.groups[1].channels, ["RL", "RR"]);
        assert_eq!(p.groups[1].delay_ms, 8.0);
        // Desligado não entra; o do grupo C só no C.
        assert_eq!(p.filters_for("L").len(), 2);
        assert_eq!(p.filters_for("C").len(), 3);
        assert_eq!(p.gain_delay_for("C"), (-2.0, 0.0));
        assert_eq!(p.gain_delay_for("RR"), (-4.0, 8.0));
    }

    #[test]
    fn le_arquivo_do_autoeq() {
        let p = Profile::parse("Preamp: -6.2 dB\nFilter 1: ON LSC Fc 105 Hz Gain 5.5 dB Q 0.70\nFilter 2: ON HPQ Fc 20 Hz Q 0.7\n").unwrap();
        assert_eq!(p.preamp_db, -6.2);
        assert_eq!(p.filters[1].kind, Kind::HighPass);
        assert_eq!(p.filters[1].gain_db, 0.0);
    }

    #[test]
    fn ida_e_volta_pelo_texto() {
        let mut p = Profile::parse(EXEMPLO).unwrap();
        p.graphic = vec![1.0; 10];
        p.protect = true;
        assert_eq!(Profile::parse(&p.to_text()).unwrap(), p);
        p.limiter = false;
        assert_eq!(Profile::parse(&p.to_text()).unwrap(), p);
    }

    #[test]
    fn protecao_corta_so_fora_do_audivel() {
        let p = Profile::parse("protecao=sim").unwrap();
        assert!(p.protect);
        let db = |f: f64| p.response_db("L", f, 48_000.0);
        assert!((db(PROTECT_HP_HZ) + 3.0).abs() < 0.2, "20 Hz: {}", db(PROTECT_HP_HZ));
        assert!(db(10.0) < -20.0, "10 Hz: {}", db(10.0));
        assert!(db(30.0) > -0.5, "30 Hz: {}", db(30.0));
        assert!(db(1000.0).abs() < 0.01);
        assert!(db(16_000.0) > -1.5, "16 kHz: {}", db(16_000.0));
        assert!(db(23_500.0) < -6.0, "23,5 kHz: {}", db(23_500.0));
        assert!(!Profile::parse("protecao=nao").unwrap().protect);
        assert!(Profile::parse("protecao=talvez").is_err());
    }

    #[test]
    fn erros_dizem_a_linha() {
        assert_eq!(Profile::parse("preamp=0\nFilter 1: ON XX Fc 1 Hz").unwrap_err(), "linha 2: tipo de filtro desconhecido");
        assert!(Profile::parse("grafico=1 2 3").unwrap_err().contains("10, 15 ou 31"));
        assert!(Profile::parse("Filter 1: ON PK Fc -5 Hz").is_err());
        assert!(Profile::parse("bobagem").is_err());
    }

    #[test]
    fn grafico_vira_picos_com_a_curva_certa() {
        let mut g = vec![0.0; 31];
        g[17] = 6.0; // 1 kHz
        let p = Profile { graphic: g, ..Profile::default() };
        assert_eq!(p.filters_for("L").len(), 1);
        assert!((p.response_db("L", 1000.0, 48_000.0) - 6.0).abs() < 0.01);
        assert!(p.response_db("L", 2000.0, 48_000.0) < 1.0);
    }
}
