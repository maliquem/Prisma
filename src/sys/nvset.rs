//! Opções do driver NVIDIA que a tela Driver NVIDIA mostra.
//!
//! Os ids e valores vêm do estudo do NVIDIA Profile Inspector (MIT, Orbmu2k e colaboradores).
//! Ficam de fora o que não serve mais: SLI, 3D Vision, Ansel, CSAA, MFAA, depuração,
//! compatibilidade em hexadecimal e o resto só de OpenGL antigo.

/// Como o valor é escolhido.
pub enum Kind {
    /// Lista fechada: valor no driver e nome na tela.
    List(&'static [(u32, &'static str)]),
    /// Número com passo e unidade; `start` é o valor ao trocar de Padrão para próprio.
    Number { min: u32, max: u32, step: u32, unit: &'static str, start: u32 },
}

pub struct Setting {
    pub id: u32,
    pub name: &'static str,
    pub tip: &'static str,
    /// Versão mínima do driver, em centésimos (57186 é o 571.86). 0 é qualquer uma.
    pub min_driver: u32,
    /// Vale para a placa toda: só aparece no perfil global.
    pub global_only: bool,
    pub kind: Kind,
}

const fn s(id: u32, name: &'static str, tip: &'static str, kind: Kind) -> Setting {
    Setting { id, name, tip, min_driver: 0, global_only: false, kind }
}

const fn since(min_driver: u32, mut v: Setting) -> Setting {
    v.min_driver = min_driver;
    v
}

const fn global(mut v: Setting) -> Setting {
    v.global_only = true;
    v
}

const OFF_ON: Kind = Kind::List(&[(0, "Desligado"), (1, "Ligado")]);
const NEWEST: Kind = Kind::List(&[(0, "Desligado"), (1, "Usar o mais novo do driver")]);
const QUALITY: Kind = Kind::List(&[(3, "O jogo decide"), (5, "Desempenho ultra"), (0, "Desempenho"), (1, "Equilibrado"), (2, "Qualidade"), (4, "DLAA"), (6, "Personalizada")]);
const GSYNC_MODE: Kind = Kind::List(&[(0, "Desligado"), (1, "Só tela cheia"), (2, "Tela cheia e janela")]);
const PERCENT: Kind = Kind::Number { min: 0, max: 100, step: 5, unit: "%", start: 50 };
const RT: Kind = Kind::List(&[(0, "Desligado"), (1, "Ligado")]);

const SYNC: &[Setting] = &[
    s(0x1083_5002, "Limite de FPS", "Limita os quadros por segundo no driver, com ritmo estável. O limitador do Medidor usa esta mesma opção nos jogos dele.", Kind::Number { min: 10, max: 1000, step: 1, unit: " FPS", start: 60 }),
    s(0x1083_5005, "Limite de FPS em segundo plano", "Limite usado enquanto o jogo não está em primeiro plano, para gastar menos energia.", Kind::Number { min: 5, max: 60, step: 1, unit: " FPS", start: 30 }),
    s(0x0005_F543, "Modo de baixa latência", "O mesmo do Painel de Controle da NVIDIA: menos quadros esperando na fila da placa. Em jogos com Reflex, o Reflex manda.", Kind::List(&[(0, "Desligado"), (1, "Ligado"), (2, "Ultra")])),
    s(0x1083_5000, "Baixa latência ultra", "A parte Ultra do modo acima, que acerta o ritmo para a fila ficar vazia. O Painel de Controle liga junto quando o modo é Ultra.", OFF_ON),
    s(0x007B_A09E, "Quadros preparados antes", "Quantos quadros a CPU prepara antes da placa mostrar. Menos dá menos atraso; mais pode suavizar com CPU fraca.", Kind::List(&[(0, "O jogo decide"), (1, "1"), (2, "2"), (3, "3"), (4, "4")])),
    s(0x00A8_79CF, "VSync", "Sincronia vertical pelo driver. Com G-SYNC, VSync ligado e FPS um pouco abaixo dos Hz evitam cortes na imagem.", Kind::List(&[(0x6092_5292, "O jogo decide"), (0x0841_6747, "Forçar desligado"), (0x4781_4940, "Forçar ligado"), (0x1888_8888, "Fast Sync"), (0x3261_0244, "Metade dos Hz"), (0x7127_1021, "Um terço dos Hz"), (0x1324_5256, "Um quarto dos Hz")])),
    s(0x005A_375C, "VSync adaptativo", "Adaptativo desliga a sincronia quando o FPS cai abaixo dos Hz: troca travadas por cortes na imagem.", Kind::List(&[(0x9686_1077, "Normal"), (0x9994_1284, "Adaptativo")])),
    s(0x20FD_D1F9, "Buffer triplo (OpenGL)", "Só vale em jogos OpenGL com VSync ligado.", OFF_ON),
    s(0x0064_B541, "Taxa de atualização preferida", "Em tela cheia exclusiva, usa a taxa mais alta da tela mesmo quando o jogo pede outra.", Kind::List(&[(0, "O jogo decide"), (1, "A mais alta")])),
    s(0x20D6_90F8, "Apresentação Vulkan e OpenGL", "Pelo DXGI, jogos Vulkan e OpenGL se comportam como DirectX: ganham Auto HDR, sobreposições e janela sem borda com menos atraso.", Kind::List(&[(0, "Nativa"), (1, "Pelo DXGI"), (2, "Automática")])),
    global(s(0x1094_F157, "G-SYNC", "Liga ou desliga o G-SYNC na placa toda.", OFF_ON)),
    global(s(0x1094_F1F7, "Modo do G-SYNC", "Onde o G-SYNC age: só em tela cheia ou também em janela.", GSYNC_MODE)),
    s(0x1194_F158, "Modo do G-SYNC no jogo", "Onde o G-SYNC age neste perfil.", GSYNC_MODE),
    s(0x10A8_79CF, "Estado do G-SYNC", "Forçar desligado ou taxa fixa servem para jogos que tremem com G-SYNC.", Kind::List(&[(0, "Permitir"), (1, "Forçar desligado"), (2, "Não permitir"), (3, "ULMB"), (4, "Taxa fixa")])),
];

const DLSS: &[Setting] = &[
    s(0x10E4_1E01, "DLSS mais novo", "Troca o DLSS que veio com o jogo pelo mais novo que o driver tem. É preciso para escolher o preset.", NEWEST),
    since(61656, s(0x10E4_1E06, "DLSS mais novo pelo Streamline", "O mesmo, em jogos que carregam o DLSS pelo Streamline.", OFF_ON)),
    s(0x0063_4291, "Escolha do preset", "Recomendado deixa a NVIDIA escolher; escolhido aqui usa o preset abaixo.", Kind::List(&[(0, "O jogo decide"), (1, "Recomendado pela NVIDIA"), (2, "Escolhido aqui")])),
    s(0x10E4_1DF3, "Preset do DLSS", "Modelo do DLSS. Os Transformer (J em diante) dão imagem mais nítida e custam um pouco mais. O Medidor grava esta mesma opção.", Kind::List(&[(0, "O jogo decide"), (0x00FF_FFFF, "Recomendado"), (1, "Preset A (CNN)"), (2, "Preset B (CNN)"), (3, "Preset C (CNN)"), (4, "Preset D (CNN)"), (5, "Preset E (CNN)"), (6, "Preset F (CNN)"), (10, "Preset J (Transformer)"), (11, "Preset K (Transformer)"), (12, "Preset L (Transformer 2)"), (13, "Preset M (Transformer 2)")])),
    s(0x10AF_B768, "Qualidade do DLSS", "Força o modo do DLSS, mesmo que o jogo peça outro. Personalizada usa a escala abaixo.", QUALITY),
    since(57283, s(0x10E4_1DF5, "Escala do DLSS", "Resolução interna em porcentagem da tela, com a qualidade Personalizada. 100% é DLAA.", Kind::Number { min: 33, max: 100, step: 1, unit: "%", start: 67 })),
    s(0x10AF_B76C, "Desempenho vira Desempenho ultra", "Quando o jogo pede Desempenho, o DLSS usa Desempenho ultra.", OFF_ON),
    s(0x10E4_1DF4, "Todo modo vira DLAA", "Qualquer modo do DLSS vira DLAA, na resolução da tela.", OFF_ON),
    s(0x10E4_1E02, "Ray Reconstruction mais novo", "Troca o Ray Reconstruction do jogo pelo mais novo do driver.", NEWEST),
    s(0x10E4_1DF7, "Preset do Ray Reconstruction", "Modelo do Ray Reconstruction. D em diante são Transformer.", Kind::List(&[(0, "O jogo decide"), (0x00FF_FFFF, "Recomendado"), (1, "Preset A (CNN)"), (2, "Preset B (CNN)"), (3, "Preset C (CNN)"), (4, "Preset D (Transformer)"), (5, "Preset E (Transformer)"), (6, "Preset F (Transformer 2)")])),
    s(0x10BD_9423, "Qualidade do Ray Reconstruction", "Força o modo do Ray Reconstruction. Personalizada usa a escala abaixo.", QUALITY),
    since(57283, s(0x10C7_D4A2, "Escala do Ray Reconstruction", "Resolução interna em porcentagem da tela, com a qualidade Personalizada.", Kind::Number { min: 33, max: 100, step: 1, unit: "%", start: 67 })),
    s(0x10E4_1E03, "Geração de quadros mais nova", "Troca a geração de quadros do jogo pela mais nova do driver.", NEWEST),
    s(0x10E4_1DF1, "Preset da geração de quadros", "Modelo da geração de quadros.", Kind::List(&[(0, "O jogo decide"), (0x00FF_FFFE, "Recomendado"), (0x00FF_FFFF, "O mais novo"), (1, "Preset A (padrão)"), (2, "Preset B (interface)"), (3, "Preset C (desempenho)")])),
    s(0x1030_8298, "Modo da geração múltipla", "Fixo gera sempre o mesmo número de quadros; dinâmico ajusta para chegar ao FPS alvo.", Kind::List(&[(0, "O jogo decide"), (2, "Fixo"), (4, "Dinâmico")])),
    s(0x104D_6667, "Quadros gerados (fixo)", "Multiplicador no modo fixo. Acima de 2x precisa de RTX 50.", Kind::List(&[(0, "O jogo decide"), (1, "2x"), (2, "3x"), (3, "4x"), (4, "5x"), (5, "6x")])),
    since(59597, s(0x1056_2D0F, "Quadros gerados (dinâmico)", "Multiplicador máximo no modo dinâmico.", Kind::List(&[(0, "O jogo decide"), (1, "Até 2x"), (2, "Até 3x"), (3, "Até 4x"), (4, "Até 5x"), (5, "Até 6x")]))),
    since(59597, s(0x10CF_4125, "FPS alvo (dinâmico)", "O FPS que o modo dinâmico tenta alcançar.", Kind::Number { min: 60, max: 500, step: 1, unit: " FPS", start: 120 })),
    s(0x1045_96A1, "Pausar em menus", "Para de gerar quadros quando um menu cobre a tela, para o menu não borrar.", Kind::List(&[(0, "O jogo decide"), (1, "Permitir"), (2, "Forçar")])),
    since(57186, s(0xB0D3_84C0, "Smooth Motion", "Geração de quadros do driver para jogos sem DLSS, em DirectX 11, DirectX 12 e Vulkan. RTX 40 e 50.", OFF_ON)),
    since(57186, s(0xB0CC_0875, "Smooth Motion em", "Em quais tipos de jogo o Smooth Motion pode agir.", Kind::List(&[(7, "Todos"), (1, "DirectX 12"), (2, "DirectX 11"), (3, "DirectX 11 e 12"), (4, "Vulkan"), (5, "DirectX 12 e Vulkan"), (6, "DirectX 11 e Vulkan")]))),
];

const AA: &[Setting] = &[
    since(44187, s(0x0059_8928, "Nitidez", "Filtro de nitidez do driver, aplicado na imagem final.", OFF_ON)),
    since(44187, s(0x002E_D8CD, "Força da nitidez", "Quanto o filtro de nitidez realça.", PERCENT)),
    since(44187, s(0x002E_D8CE, "Ignorar granulação", "Quanto o filtro deixa de realçar o ruído e a granulação de filme.", Kind::Number { min: 0, max: 100, step: 5, unit: "%", start: 15 })),
    s(0x1074_C972, "FXAA", "Antisserrilhado rápido do driver. Borra um pouco; serve para jogos antigos.", OFF_ON),
    s(0x107E_FC5B, "Modo do MSAA", "MSAA forçado pelo driver só funciona em jogos DirectX 9 e OpenGL antigos.", Kind::List(&[(0, "O jogo decide"), (1, "Trocar o do jogo"), (2, "Melhorar o do jogo")])),
    s(0x10D7_73D2, "MSAA", "Nível do antisserrilhado forçado. Os SS e os mistos (S) são supersampling: mais bonitos e bem mais pesados.", Kind::List(&[(0, "Desligado"), (0x0E, "2x"), (0x0F, "2xQ"), (0x10, "4x"), (0x25, "8xQ"), (0x05, "2x2 SS"), (0x0A, "3x3 SS"), (0x0C, "4x4 SS"), (0x18, "8xS"), (0x19, "8xSQ"), (0x1A, "16xS"), (0x29, "32xS")])),
    s(0x10FC_2D9C, "Transparência com multisampling", "Suaviza as bordas de folhas e grades quando o MSAA está ligado.", Kind::List(&[(0, "Desligado"), (4, "Ligado")])),
    s(0x10D4_8A85, "Transparência com supersampling", "Mais pesado e mais bonito que o multisampling. O esparso (SGSSAA) suaviza a imagem toda.", Kind::List(&[(0, "Desligado"), (0x14, "2x"), (0x24, "4x"), (0x34, "8x"), (0x18, "2x esparso"), (0x28, "4x esparso"), (0x38, "8x esparso")])),
    s(0x107D_639D, "Correção de gama no antisserrilhado", "Acerta o brilho das bordas suavizadas.", Kind::List(&[(0, "Desligado"), (2, "Ligado")])),
];

const TEXTURES: &[Setting] = &[
    s(0x10D2_BB16, "Modo do anisotrópico", "Definido aqui usa o nível abaixo em todas as texturas.", Kind::List(&[(0, "O jogo decide"), (1, "Definido aqui")])),
    s(0x101E_61A9, "Filtro anisotrópico", "Deixa nítidas as texturas vistas de lado, como o chão. Custa quase nada nas placas atuais.", Kind::List(&[(1, "Desligado"), (2, "2x"), (4, "4x"), (8, "8x"), (12, "12x"), (16, "16x")])),
    s(0x00CE_2691, "Qualidade da filtragem", "Alta qualidade desliga os atalhos de filtragem. A diferença de desempenho é pequena hoje.", Kind::List(&[(0xFFFF_FFF6, "Alta qualidade"), (0, "Qualidade"), (0x0A, "Desempenho"), (0x14, "Alto desempenho")])),
    s(0x002E_CAF2, "Otimização trilinear", "Atalho de filtragem. Não faz nada em Alta qualidade.", Kind::List(&[(0, "Ligada"), (1, "Desligada")])),
    s(0x0084_CD70, "Otimização do anisotrópico", "Atalho que filtra menos algumas camadas de textura.", OFF_ON),
    s(0x00E7_3211, "Otimização de amostras do anisotrópico", "Atalho que usa menos amostras no anisotrópico.", OFF_ON),
    s(0x0019_BB68, "LOD negativo", "Travar evita texturas tremendo quando o jogo pede nitidez demais.", Kind::List(&[(0, "Permitir"), (1, "Travar")])),
    s(0x0073_8E8F, "Ajuste de LOD (DirectX)", "Negativo deixa as texturas mais nítidas e pode tremer; positivo borra. Combina com supersampling.", Kind::List(&[(0xFFFF_FFE8, "-3"), (0xFFFF_FFF0, "-2"), (0xFFFF_FFF4, "-1,5"), (0xFFFF_FFF8, "-1"), (0xFFFF_FFFC, "-0,5"), (0xFFFF_FFFE, "-0,25"), (0, "0"), (2, "+0,25"), (4, "+0,5"), (8, "+1"), (16, "+2"), (24, "+3")])),
];

const IMAGE: &[Setting] = &[
    s(0x00DD_48FB, "RTX HDR", "HDR feito pelo driver para jogos SDR. Precisa do HDR do Windows ligado.", OFF_ON),
    s(0x0043_2F84, "RTX HDR pelo driver", "Liga o RTX HDR direto no perfil, sem o app da NVIDIA, e escolhe quanto suaviza as faixas de cor.", Kind::List(&[(0, "Desligado"), (0x06, "Ligado, sem suavizar faixas"), (0x0A, "Ligado, suavização média"), (0x02, "Ligado, suavização máxima"), (0x03, "Máxima e com indicador")])),
    s(0x0098_0880, "RTX Dynamic Vibrance", "Cores mais vivas, ajustadas pelo driver cena a cena.", OFF_ON),
    s(0x00AB_AB13, "Saturação extra", "Quanto o Dynamic Vibrance aumenta a saturação.", Kind::Number { min: 0, max: 100, step: 5, unit: "%", start: 25 }),
    s(0x00AB_AB22, "Intensidade do Dynamic Vibrance", "Quanto o Dynamic Vibrance age na imagem.", PERCENT),
    s(0x00DE_429A, "Ray tracing (DirectX)", "Desligado esconde o ray tracing do jogo, que passa a achar que a placa não tem.", RT),
    s(0x20BC_1A2B, "Ray tracing (Vulkan)", "O mesmo, em jogos Vulkan.", RT),
    s(0x809D_5F60, "Sobreposição do app da NVIDIA", "Libera a captura da área de trabalho para este programa ou tira as estatísticas do app.", Kind::List(&[(0, "Normal"), (1, "Liberar captura da área de trabalho"), (0x1000_0000, "Sem estatísticas do app")])),
    s(0x1002_9538, "Indicador do G-SYNC", "Mostra na tela quando o G-SYNC está agindo.", OFF_ON),
    s(0x008D_F510, "Indicador de suporte ao G-SYNC", "Mostra na tela se o jogo aceita G-SYNC.", Kind::List(&[(0, "Desligado"), (4, "Ligado")])),
    s(0x002C_F156, "Indicador de quadros", "Gráfico do driver com o ritmo dos quadros, bom para conferir o limitador.", Kind::List(&[(0, "Desligado"), (1, "Gráfico do FPS"), (0x1FF, "Tudo")])),
    s(0x00A8_8888, "Indicador de MPO", "Mostra quando a imagem usa planos sobrepostos (MPO), que dão menos atraso em janela.", OFF_ON),
];

const SYSTEM: &[Setting] = &[
    s(0x1057_EB71, "Gerenciamento de energia", "Desempenho máximo mantém o clock alto e gasta mais energia, mesmo com o jogo parado num menu.", Kind::List(&[(0, "Adaptativo"), (5, "Desempenho ideal"), (1, "Desempenho máximo")])),
    s(0x20C1_221E, "Otimização em vários núcleos", "Divide o trabalho do driver em mais núcleos da CPU. Ajuda em jogos OpenGL e DirectX antigos e atrapalha alguns.", Kind::List(&[(0, "Automática"), (1, "Ligada"), (2, "Desligada")])),
    s(0x0019_8FFF, "Cache de shaders", "Guarda os shaders compilados para o jogo não compilar de novo a cada abertura.", OFF_ON),
    global(s(0x00AC_8497, "Tamanho do cache de shaders", "Mais espaço guarda os shaders de mais jogos.", Kind::List(&[(0, "Desligado"), (0x400, "1 GB"), (0x1000, "4 GB"), (0x2000, "8 GB"), (0x2800, "10 GB"), (0x4000, "16 GB"), (0x8000, "32 GB"), (0x1_9000, "100 GB"), (0xFFFF_FFFF, "Ilimitado")]))),
    since(59597, s(0x00EA_D189, "Pré-compilar shaders", "Compila shaders antes de o jogo pedir, em segundo plano. Mais compila mais e usa mais a CPU.", Kind::List(&[(0, "Desligado"), (1, "Pouco"), (2, "Médio"), (3, "Muito")]))),
    s(0x208F_3F2E, "Cache de shaders (OpenGL)", "O cache de shaders em jogos OpenGL.", OFF_ON),
    s(0x000F_00BA, "Resizable BAR", "Deixa a CPU ver a memória da placa toda. Precisa estar ligado na BIOS; a NVIDIA só liga em jogos testados.", OFF_ON),
    since(61656, s(0x000B_FA21, "Resizable BAR (novo)", "A versão nova da mesma opção.", Kind::List(&[(0, "Desligado"), (1, "Automático"), (2, "Ligado")]))),
    since(59144, s(0x5067_ECC4, "PhysX de 32 bits na placa", "Volta o PhysX acelerado de jogos antigos nas RTX 50.", OFF_ON)),
    s(0x5016_6C5E, "CUDA: forçar P2", "Ligado, programas CUDA rodam com a memória num clock mais baixo. Desligado, a memória vai ao clock máximo.", OFF_ON),
    s(0x10EC_ECC9, "CUDA: memória do sistema", "Quando a memória da placa acaba, programas CUDA (como IA local) podem usar a do sistema, mais lenta. Evitar dá erro em vez de lentidão.", Kind::List(&[(0, "Automático"), (1, "Evitar"), (2, "Preferir")])),
];

pub const GROUPS: &[(&str, &[Setting])] = &[
    ("Sincronia e FPS", SYNC),
    ("DLSS", DLSS),
    ("Antisserrilhado", AA),
    ("Texturas", TEXTURES),
    ("Imagem e indicadores", IMAGE),
    ("Sistema", SYSTEM),
];

/// Todos os ids, na ordem dos grupos.
pub fn ids() -> Vec<u32> {
    GROUPS.iter().flat_map(|g| g.1.iter().map(|s| s.id)).collect()
}

/// Nome do valor na tela, ou o número quando não está na lista.
pub fn value_name(s: &Setting, v: u32) -> String {
    match &s.kind {
        Kind::List(l) => l.iter().find(|x| x.0 == v).map(|x| crate::i18n::tr(x.1).into_owned()).unwrap_or_else(|| format!("0x{v:08X}")),
        // Abaixo do mínimo é o driver dizendo que está desligado (o 0 do limite de FPS).
        Kind::Number { min, .. } if v < *min => crate::i18n::tr("Desligado").into_owned(),
        Kind::Number { unit, .. } => format!("{v}{unit}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_values_are_unique() {
        let mut all = ids();
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), n, "id repetido");
        for (_, g) in GROUPS {
            for s in *g {
                if let Kind::List(l) = &s.kind {
                    let mut v: Vec<u32> = l.iter().map(|x| x.0).collect();
                    v.sort();
                    v.dedup();
                    assert_eq!(v.len(), l.len(), "valor repetido em {}", s.name);
                }
                if let Kind::Number { min, max, start, .. } = s.kind {
                    assert!(min <= start && start <= max, "{}", s.name);
                }
            }
        }
    }
}
