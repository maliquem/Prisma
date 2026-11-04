# Prisma

Utilitário leve para Windows 11, todo em Rust, sem framework de interface. Fica na bandeja e só carrega o que está em uso.

## Portátil ou instalado

É um único `prisma.exe`.

- **Portátil:** rode de qualquer pasta. As configurações ficam em `PrismaData`, ao lado do programa, e nada é registrado no Windows.
- **Instalado:** em **Ajustes**, clique em **Instalar no PC**. O Prisma vai para Arquivos de Programas, ganha atalho no menu Iniciar e aparece na lista de programas do Windows.

Só a versão instalada pode iniciar com o Windows e substituir o Gerenciador de Tarefas. Um programa que abre como administrador sem perguntar precisa ficar numa pasta que só administrador altera.

## Primeiro uso

1. Abra `prisma.exe` e, em **Ajustes**, clique em **Instalar no PC**.
2. Marque **Iniciar com o Windows** e, se quiser, a opção de abrir o Prisma com Ctrl+Shift+Esc.
3. Em **Widget**, escolha um dos quatro estilos.

## Atalhos padrão

Todos podem ser trocados no programa.

| Ação | Atalho |
| --- | --- |
| Fullscreenizer | Ctrl + Alt + F |
| Volume do programa em foco | Ctrl + teclas de volume e mudo |
| Widget | Ctrl + Alt + W |
| Otimizar | Ctrl + Alt + O |
| Foto da área | Ctrl + Alt + P |
| Foto da tela inteira | Ctrl + Alt + A |
| Gravar e parar | Ctrl + Alt + R |
| Salvar replay | Ctrl + Alt + B |

Na foto: Enter copia, Ctrl + S salva, Ctrl + Z desfaz e Esc cancela. Foto fixada: arraste para mover, botão direito fecha.

## Funções

- **Monitor:** CPU por núcleo, GPU, RAM, VRAM, disco e rede de cada programa, com ícones e Finalizar tarefa.
- **Discos e redes:** velocidade, tempo ativo e espaço de cada disco, e nota de saúde (Bom, Atenção, Risco) pelo SMART, inclusive em HDs USB. Cada rede mostra velocidade, endereço e total transferido.
- **Espaço em disco:** analisa um disco inteiro lendo a tabela do NTFS, com pastas, maiores arquivos, tipos e mapa visual. Abre no Explorador e, como administrador, manda para a Lixeira.
- **Inicialização:** liga, desliga, adiciona e remove o que abre com o Windows, e mostra quem assinou cada programa.
- **Serviços:** lista com descrição em português, iniciar, parar e reiniciar. Editar abre a janela do Windows.
- **Desinstalar:** remove em lote, cria ponto de restauração e procura restos. Só vem marcado o que tem o nome exato do programa, pastas vão para a Lixeira e o registro ganha backup em `%APPDATA%\Prisma\backups`.
- **Substituir o Gerenciador de Tarefas:** opção em Ajustes que faz Ctrl+Shift+Esc abrir o Prisma. A desinstalação devolve o Gerenciador de Tarefas.
- **Ajustes do Windows:** primeiro uma auditoria que só lê o PC. Depois os perfis Jogo, Desenvolvimento e Privacidade, em camadas, só com ajustes de efeito documentado. A aba Fora da lista explica o que ficou de fora e por quê. Cada mudança guarda o valor anterior em `diario.txt`, e Desfazer só restaura o que ninguém mexeu depois. Nenhum perfil toca em atualizações, antivírus, UAC, Integridade da Memória ou virtualização.
- **Ferramentas:** reparo do Windows (DISM e depois SFC), histórico de falhas, Windows Update, verificação de disco, rede, DNS, limpeza, winget, diagnóstico e reinício avançado, com resultado em português. Em disco suspeito, os modos que escrevem ficam bloqueados.
- **Otimizar:** sugere o que fechar e sempre pergunta. Blacklist nunca fecha, whitelist sempre fecha. A detecção automática sugere nomes para as duas listas.
- **Energia:** detecta jogo ou carga alta e troca o plano. Volta ao repouso depois de 60 s sem carga.
- **Perfil por jogo:** lembra tela sem bordas e volume de cada programa.
- **Captura:** foto com anotações e vídeo em AV1, HEVC ou H.264. Com a tela em HDR, grava em SDR ou HDR10.
- **Replay:** em modo jogo, guarda os últimos segundos na memória.

## Limites

- Jogo em tela cheia exclusiva precisa estar em modo janela antes do Fullscreenizer.
- Sem temperatura e consumo da CPU, porque exigem driver de kernel.
- Os perfis não prometem FPS: debloat não tem ganho medido em CPU moderna. O valor é privacidade e menos ruído.
- Apps removidos voltam pela Loja, e a volta pode falhar.
- A nota de saúde é um indicador, não uma garantia: disco pode falhar sem aviso.
- Coluna de rede, saúde dos discos e leitura rápida do NTFS exigem o Prisma como administrador.
- Tudo que o Prisma muda no sistema fica registrado em `%APPDATA%\Prisma\acoes.log`.
- H.264 não tem 10 bits: gravar em HDR10 com ele usa HEVC.
- Conteúdo protegido, como Netflix, sai preto nas capturas.

## Requisitos

Windows 11 de 64 bits. Nada para instalar: o `prisma.exe` traz tudo que precisa.

- Gravar vídeo exige codificador de vídeo na GPU (NVIDIA, AMD ou Intel) e, em edições N do Windows, o Pacote de Recursos de Mídia. Sem isso, só a gravação fica indisponível.
- Temperatura da GPU depende do driver da NVIDIA; as demais leituras funcionam em qualquer placa.

## Compilar

```bash
cargo build --release
```

Precisa do Rust e das Build Tools do Visual Studio. Configuração em `%APPDATA%\Prisma`.
