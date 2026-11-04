# Prisma

[English](README.md) · **Português (Brasil)**

Utilitário leve para Windows 11, todo em Rust, sem framework de interface. Fica na bandeja, só carrega o que está em uso e cabe em um único arquivo de 1,2 MB. Substitui o Gerenciador de Tarefas e reúne as pequenas ferramentas que quem joga no PC costuma juntar uma a uma.

**[Baixar a versão mais recente](https://github.com/maliquem/Prisma/releases/latest)**

![Estilos do widget do Prisma](docs/widget.png)

## Primeiros passos

O Prisma é um único arquivo, `Prisma.exe`, sem mais nada para instalar. O Windows pode avisar sobre editor desconhecido na primeira vez, porque o arquivo não tem assinatura digital.

- **Portátil:** rode de qualquer pasta. As configurações ficam em `PrismaData`, ao lado do programa, e nada é registrado no Windows.
- **Instalado:** abra **Configurações** e clique em **Instalar no PC**. O Prisma vai para Arquivos de Programas, ganha atalho no menu Iniciar e aparece na lista de programas do Windows.

Só a versão instalada pode iniciar com o Windows e substituir o Gerenciador de Tarefas.

A interface vem em português do Brasil e inglês. O Prisma começa no idioma do Windows, e você troca em **Configurações**. Textos que vêm do próprio Windows, como descrições de serviços e saída de comandos, ficam no idioma do Windows.

## Atualizações

O Prisma procura nesta página ao abrir e uma vez por dia. Quando há versão nova, ele avisa, e nada é instalado até você clicar em **Instalar** em **Configurações**. Ele baixa o arquivo daqui, confere o hash SHA-256, troca o programa e reinicia. A busca não envia dados seus e pode ser desligada. A versão instalada precisa do Prisma como administrador para atualizar.

## Funções

- **Monitor:** CPU por núcleo, GPU, RAM, VRAM, disco e rede de cada programa, com ícones e Finalizar tarefa.
- **Discos e redes:** velocidade, tempo ativo e espaço de cada disco, e nota de saúde (Bom, Atenção, Risco) pelo SMART, inclusive em HDs USB. Cada rede mostra velocidade, endereço e total transferido.
- **Espaço em disco:** analisa um disco inteiro lendo a tabela do NTFS, com pastas, maiores arquivos, tipos e mapa visual. Abre no Explorador e, como administrador, manda para a Lixeira.
- **Inicialização:** liga, desliga, adiciona e remove o que abre com o Windows, e mostra quem assinou cada programa. Ordena por nome, ativados ou desativados.
- **Serviços:** descrição, iniciar, parar e reiniciar. Com o Prisma como administrador, o botão direito abre a janela de propriedades do próprio Windows para aquele serviço.
- **Desinstalar:** remove em lote, cria ponto de restauração e procura restos. Ordena por nome, tamanho ou data de instalação. Só vem marcado o que tem o nome exato do programa, pastas vão para a Lixeira e o registro ganha backup antes.
- **Substituir o Gerenciador de Tarefas:** opção que faz Ctrl+Shift+Esc abrir o Prisma. A desinstalação devolve o Gerenciador de Tarefas.
- **Ajustes do Windows:** primeiro uma auditoria que só lê o PC. Depois os perfis Jogo, Desenvolvimento e Privacidade, em camadas, só com ajustes de efeito documentado. A aba Fora da lista explica o que ficou de fora e por quê. Cada mudança guarda o valor anterior, e Desfazer só restaura o que ninguém mexeu depois. Nenhum perfil toca em atualizações, antivírus, UAC, Integridade da Memória ou virtualização. **Ver log** mostra cada mudança, com o antes e o depois.
- **Ferramentas:** reparo do Windows (DISM e depois SFC), histórico de falhas, Windows Update, verificação de disco, rede, DNS, limpeza, winget, diagnóstico e reinício avançado. Em disco suspeito, os modos que escrevem ficam bloqueados. O passo a passo mostra o comando exato, a saída, os erros e o tempo de cada etapa.
- **Otimizar:** sugere o que fechar e sempre pergunta. Blacklist nunca fecha, whitelist sempre fecha. A detecção automática sugere nomes para as duas listas.
- **Energia:** detecta jogo ou carga alta e troca o plano. Volta ao repouso depois de 60 s sem carga.
- **Perfil por jogo:** lembra tela sem bordas e volume de cada programa.
- **Fullscreenizer:** deixa a janela em foco em tela cheia sem bordas.
- **Volume por programa:** as teclas de volume do teclado, com um modificador, mudam só o programa em foco.
- **Widget:** CPU, GPU, RAM e VRAM por cima de tudo, em quatro estilos.
- **Captura:** foto com anotações e vídeo em AV1, HEVC ou H.264. Com a tela em HDR, grava em SDR ou HDR10.
- **Replay:** em modo jogo, guarda os últimos segundos na memória.
- **Relatório de falha:** se o Prisma cair, ele guarda um relatório na pasta de dados e avisa na próxima abertura. O relatório fica só no seu PC.

![Perfis dos Ajustes do Windows](docs/profiles.png)

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

## Limites

- Jogo em tela cheia exclusiva precisa estar em modo janela antes do Fullscreenizer.
- Sem temperatura e consumo da CPU, porque exigem driver de kernel.
- Os perfis não prometem FPS. O valor é privacidade e menos ruído.
- Apps removidos voltam pela Loja, e a volta pode falhar.
- A nota de saúde é um indicador, não uma garantia: disco pode falhar sem aviso.
- Coluna de rede, saúde dos discos e leitura rápida do NTFS exigem o Prisma como administrador.
- Tudo que o Prisma muda no sistema fica registrado em `acoes.log`, na pasta de dados.
- H.264 não tem 10 bits: gravar em HDR10 usa HEVC.
- Conteúdo protegido, como Netflix, sai preto nas capturas.

## Requisitos

Windows 11 de 64 bits.

- Gravar vídeo exige codificador de vídeo na GPU (NVIDIA, AMD ou Intel) e, em edições N do Windows, o Pacote de Recursos de Mídia. Sem isso, só a gravação fica indisponível.
- Temperatura da GPU depende do driver da NVIDIA. As demais leituras funcionam em qualquer placa.

## Compilar

```bash
cargo build --release
```

Precisa do Rust e das Build Tools do Visual Studio.

## Publicar uma versão

1. Atualize `version` no `Cargo.toml`, acrescente a seção no `CHANGELOG.md` e atualize os dois READMEs.
2. Faça o commit e envie uma etiqueta como `v0.3.0`.

O GitHub compila o `Prisma.exe` e publica a versão com o hash, e todas as cópias do Prisma oferecem a atualização.
