# Prisma

[English](README.md) · **Português (Brasil)** · [Español](README.es.md) · [Русский](README.ru.md) · [简体中文](README.zh-CN.md)

Utilitário leve para Windows 11, todo em Rust, sem framework de interface. Fica na bandeja, só carrega o que está em uso e cabe em um único arquivo de 2 MB. Substitui o Gerenciador de Tarefas e reúne as pequenas ferramentas que quem joga no PC costuma juntar uma a uma.

**[Baixar a versão mais recente](https://github.com/maliquem/Prisma/releases/latest)** · **[Guia: todas as funções explicadas](docs/guide/GUIA.md)**

[![Buy Me a Coffee at ko-fi.com](https://ko-fi.com/img/githubbutton_sm.svg)](https://ko-fi.com/maliquem)

![Estilos do widget do Prisma](docs/widget.png)

## Primeiros passos

O Prisma é um único arquivo, `Prisma.exe`. O Windows pode avisar sobre editor desconhecido na primeira vez, porque o arquivo não tem assinatura digital.

Ao abrir pela primeira vez, a tela de boas-vindas pergunta se você quer instalar, dar permissão de administrador e substituir o Gerenciador de Tarefas. As três vêm marcadas, e o Windows pede a permissão uma vez só. A tela também abre o guia, que explica cada função, o que dá e o que não dá para fazer. Ela só aparece uma vez, a não ser que você desmarque **Não mostrar novamente**.

- **Portátil:** rode de qualquer pasta. As configurações ficam em `PrismaData`, ao lado do programa, e nada é registrado no Windows. Com administrador, o Windows pergunta a cada abertura.
- **Instalado:** o Prisma vai para Arquivos de Programas, ganha atalho no menu Iniciar e aparece na lista de programas do Windows. Com administrador, abre sem perguntar. Também dá para instalar depois, em **Configurações**.

A versão instalada inicia com o Windows por padrão, e você desliga isso em **Configurações**. A portátil não inicia com o Windows. O Gerenciador de Tarefas pode ser substituído nas duas, mas na portátil o atalho para de funcionar se você mover ou apagar o arquivo.

O guia e o **Reportar bug** ficam em dois botões ao lado do nome Prisma, no alto da janela. Passe o mouse em cima para ver o que cada um faz.

A interface vem em português do Brasil, inglês, espanhol, russo e chinês simplificado. O Prisma começa no idioma do Windows, e você troca em **Configurações**. O guia e as novidades de cada versão vêm em português e inglês; nos outros idiomas, aparecem em inglês. Textos que vêm do próprio Windows, como descrições de serviços e saída de comandos, ficam no idioma do Windows.

## Atualizações

O Prisma procura nesta página uma vez por dia. Quando há versão nova e você abre a janela, ele pergunta se quer atualizar. Minimizado ou na bandeja, ele não interrompe. Se você responder Sim, ele baixa o arquivo daqui em partes, confere o hash SHA-256, troca o programa e reinicia. A busca não envia dados seus. Marque **Não mostrar de novo** na pergunta, ou desligue em **Configurações**, para parar. Na versão instalada, o Windows pede permissão se o Prisma não estiver como administrador.

## Funções

- **Monitor:** CPU por núcleo, GPU, RAM, VRAM, disco e rede de cada programa, com ícones. O botão direito finaliza a tarefa, vai para os detalhes, abre o local do arquivo, pesquisa online e mostra as propriedades. A aba Detalhes lista cada processo com o PID. Busca por nome ou PID, e reinicia o Explorer quando a barra de tarefas trava.
- **Discos e redes:** velocidade, tempo ativo e espaço de cada disco, e nota de saúde (Bom, Atenção, Risco) pelo SMART, inclusive em HDs USB. Cada rede mostra velocidade, endereço e total transferido.
- **Espaço em disco:** analisa um disco inteiro lendo a tabela do NTFS, com pastas, maiores arquivos, tipos e mapa visual. Abre no Explorador e, como administrador, manda para a Lixeira.
- **Pesquisar:** acha arquivos e pastas pelo nome em todos os discos enquanto você digita, na aba ou numa janelinha com Ctrl + Alt + Espaço. Como administrador, o índice sai da tabela do NTFS em segundos e acompanha cada mudança na hora. Programas instalados vêm primeiro, perguntas vão para o navegador ou uma IA instalada e `>` roda comandos do PowerShell ou do CMD com sugestões. Opcional, vem desligada.
- **Inicialização:** liga, desliga, adiciona e remove o que abre com o Windows, e mostra quem assinou cada programa. Ordena por nome, ativados ou desativados.
- **Serviços:** descrição, iniciar, parar e reiniciar. O botão direito mostra iniciar, parar, reiniciar e propriedades, que abre a janela do próprio Windows para aquele serviço.
- **Desinstalar:** remove em lote, cria ponto de restauração, procura restos e informa o que foi excluído. Ordena por nome, data ou tamanho. Só vem marcado o que tem o nome exato, nomes parecidos aparecem para você conferir, pastas vão para a Lixeira e o registro ganha backup antes.
- **Fechar à força:** um atalho configurável, Alt+F4 por padrão, encerra na hora o programa em foco pelo PID, mesmo travado em tela cheia. O que não foi salvo é perdido. A área de trabalho, a barra de tarefas e os processos do Windows continuam com o Alt+F4 normal.
- **Substituir o Gerenciador de Tarefas:** um botão em Configurações faz Ctrl+Shift+Esc abrir o Prisma, e outro devolve. A desinstalação também devolve o Gerenciador de Tarefas.
- **Ajustes do Windows:** primeiro uma auditoria que só lê o PC. Depois os perfis Jogo, Desenvolvimento e Privacidade, em camadas, só com ajustes de efeito documentado. A aba Fora da lista explica o que ficou de fora e por quê. Cada mudança guarda o valor anterior, e Desfazer só restaura o que ninguém mexeu depois. Nenhum perfil toca em atualizações, antivírus, UAC, Integridade da Memória ou virtualização. **Ver log** mostra cada mudança, com o antes e o depois.
- **Ferramentas:** reparo do Windows (DISM e depois SFC), histórico de falhas, Windows Update, verificação de disco, rede, DNS, limpeza, winget, diagnóstico e reinício avançado. Em disco suspeito, os modos que escrevem ficam bloqueados. Cada ferramenta guarda o próprio passo a passo, com o comando exato, a saída, os erros e o tempo de cada etapa.
- **Otimizar:** sugere o que fechar e sempre pergunta. Blacklist nunca fecha, whitelist sempre fecha. A detecção automática sugere nomes para as duas listas. A pergunta quando um jogo abre vem desligada e dá para ligar em Energia.
- **Medidor:** barra compacta e painel completo sobre o jogo, só pelo atalho, que mede o programa em foco, põe o jogo na lista e, apertado de novo, esconde o medidor. Mostra FPS, 1% e 0,1% low, frame time, CPU, GPU, VRAM e RAM. As leituras vêm dos eventos do Windows, sem nada dentro do jogo. Limite de FPS por jogo, manual ou automático depois de 5 minutos de jogo. Por padrão, um pequeno módulo do Prisma entra no jogo quando o limitador está ligado, e o limite muda na hora em DirectX 10, 11 e 12. Se falhar, o jogo cair ou houver anti-cheat detectado, o limite passa para o driver NVIDIA e vale na próxima abertura. Os botões ficam no painel completo: Ctrl + Alt + 5 libera mouse e teclado para eles, e dá para digitar o limite. Também escolhe o perfil de DLSS (só nos jogos com DLSS), o Auto HDR do Windows por jogo, igual ao da aba HDR, e o plano de energia, mostra upscaler, geração de quadros e Reflex carregados, e liga o HDR do Windows nos jogos da lista ou em todos, sem desligar o HDR que já estava ligado. Precisa do Prisma como administrador. Em tela cheia exclusiva, a barra pode não aparecer.
- **Registro de dados:** gravação opcional para estudar desempenho, ligada na aba Medidor ou por atalho durante o jogo. Guarda cada quadro, cada segundo e a CPU de cada thread do jogo em `Documentos\Prisma\Logs`, com um resumo no final; em placas NVIDIA, também a GPU (controlador de memória, energia e limite, o que segurou o clock, PCIe e VRAM), para estudar mods de otimização. Digite os minutos de jogo (5 é o padrão, 0 é sem limite). Só roda se você ligar, e a leitura de threads fica desligada em jogos com anti-cheat detectado.
- **Energia:** detecta jogo ou carga alta e troca o plano. Volta ao repouso depois de 60 s sem carga. Você escolhe a porcentagem e o tempo que contam como carga na CPU e na GPU. Selecione um plano para editar com todas as opções que o Windows registra, inclusive as ocultas, ou importar e exportar em arquivos .pow. O botão direito no plano também oferece Exportar e Editar. A primeira edição guarda uma cópia do plano original.
- **Perfil por jogo:** lembra o volume de cada programa.
- **Fullscreenizer:** deixa a janela em foco em tela cheia sem bordas. Quem usa uma vez entra numa lista salva, onde dá para adicionar pela lista de programas ou pelo .exe, remover e marcar Automático para aplicar sempre que o programa abrir.
- **Som:** volume por programa, e as teclas de volume do teclado, com um modificador, mudam só o programa em foco. Você escolhe o aparelho de saída e o de entrada de cada programa, inclusive os adicionados à mão pela lista de programas ou pelo .exe. A lista traz o que o menu Iniciar mostra, aplicativos da Microsoft Store, os programas instalados, os jogos da Steam e da Epic e o que está aberto. Isso usa um recurso interno do Windows, e alguns programas só trocam de aparelho ao reabrir. Programas fixos ficam na lista mesmo fechados e voltam com o volume guardado. Atalhos de aparelho padrão trocam a saída e a entrada do Windows de uma vez, cada um com os aparelhos que você escolher. Manter o som acordado toca um fluxo sem som audível, para HDMI e Dolby não cortarem o começo do próximo som, e para sozinho depois de 10 minutos sem uso.
- **Widget:** CPU, GPU, RAM e VRAM por cima de tudo, em quatro estilos.
- **HDR:** painel de leitura que mostra o modo da tela, o perfil de calibração ativo com a saturação real, o brilho que cada fonte declara, o que o Windows informa aos jogos, o Dolby Vision declarado e os formatos de som aceitos. Avisa quando algo não confere. A leitura direta da ficha da tela existe só em placas NVIDIA. A calibração própria abre padrões de teste em tela cheia, com o preto, o pico em cinco tamanhos de janela e cada canal de cor, e grava um perfil por modo de imagem da TV, com as primárias reais da tela. O Windows pode continuar informando os valores antigos aos jogos por um tempo depois de ativar. O perfil não muda o que a TV faz. A lista de Auto HDR por jogo força, desliga ou deixa com o Windows, em jogos DX11 e DX12, sem injetar nada no jogo. O teste de atraso do som toca um clique junto de um clarão, para comparar o atraso com e sem som espacial.
- **Driver NVIDIA:** só com placa NVIDIA. Opções do driver no perfil global ou por jogo, como o NVIDIA Profile Inspector, sem as que não fazem mais nada (SLI, 3D Vision, Ansel, CSAA, MFAA, depuração e OpenGL antigo). Sincronia e FPS, DLSS e geração de quadros, antisserrilhado e nitidez, texturas, RTX HDR e indicadores, e opções do sistema, cada uma explicada ao passar o mouse, com Padrão para voltar. Uma cópia de todos os perfis do driver é guardada antes da primeira mudança.
- **Captura:** foto com anotações e vídeo em AV1, HEVC ou H.264. Com a tela em HDR, grava em SDR ou HDR10. Gravar Janela grava a janela em foco, e Gravar Tela grava a tela inteira. Com Som do sistema ligado, grava todos os sons do PC. Desligado, Gravar Janela grava só o som daquela janela.
- **Replay:** em modo jogo, guarda os últimos segundos na memória.
- **Compartilhar tela (em teste):** mostra e controla outro PC pela internet, para acesso remoto ou co-op local à distância, sem conta. Pareia uma vez com um código curto (como `K7M2-QX9P`) ou um link que abre o Prisma, confere um código de confirmação e depois conecta direto, pela VPN ou por relé, sempre criptografado de ponta a ponta. Permissões por aparelho (som, controle, teclado, mouse, copiar e colar, tela segura), até 4 pessoas com controles de Xbox virtuais (precisa do ViGEmBus ou do VIIPER no host), HDR convertido ou desligado enquanto alguém assiste, e um serviço opcional do Windows para a tela de bloqueio, o UAC e o Ctrl+Alt+Del. Opções de resolução, quadros por segundo, codec, banda, prioridade, origem do som, porta e atalhos, além de modo imersivo, medidor de latência, aviso quando a conexão piora e um menu da conexão com atalho para cada opção. As opções do host valem durante a conexão, o programa de chamada que está tocando sai do som, e em notebooks com duas placas de vídeo você escolhe qual codifica e qual decodifica. Os aparelhos pareados ficam em duas abas (os que este PC assiste e os que assistem a ele) e podem ser renomeados; mudar as permissões de um aparelho conectado vale na hora. O host ganha a aba Conectados, para desconectar, banir ou mudar o acesso na hora, e uma notificação quando alguém conecta ou sai. Ctrl + Alt + Shift + Q no host desconecta todo mundo. **Mouse e teclado** (como o Deskflow) usa o mouse e o teclado deste PC num PC pareado ao lado, sem vídeo: o mouse passa pela borda da tela que você escolhe, e o Scroll Lock traz de volta. **Só o som** (como o AudioRelay) toca neste PC o som do PC pareado, sem vídeo.
- **Reportar bug:** clique no botão do inseto, no alto da janela. Escolha a função, a gravidade, descreva o que aconteceu e junte uma foto, se quiser. O navegador abre o GitHub com tudo escrito e você só confirma. A foto vai copiada: é só colar com Ctrl+V. Dá para incluir dados do PC e um registro curto, sem nome de usuário, nome do PC ou caminhos de arquivo. A página do GitHub é pública, e você vê tudo antes de enviar.
- **Relatório de falha:** se o Prisma cair, ele guarda um relatório na pasta de dados e avisa na próxima abertura. O arquivo fica só no seu PC. Em **Reportar bug**, só um resumo do erro pode ir junto, e só se você enviar.

![Perfis dos Ajustes do Windows](docs/profiles.png)

Ações que mudam algo pedem confirmação em um popup, todas as listas têm barra de rolagem para arrastar, e o botão Padrão de cada tela volta as opções dela.

## Atalhos padrão

Todos podem ser trocados no programa.

| Ação | Atalho |
| --- | --- |
| Fullscreenizer | Ctrl + Alt + F |
| Volume do programa em foco | Ctrl + teclas de volume e mudo |
| Widget | Ctrl + Alt + W |
| Otimizar | Ctrl + Alt + O |
| Foto da área | Ctrl + Alt + P (também aceita Print Screen, se você configurar) |
| Mostrar medidor compacto | Ctrl + Alt + 1 |
| Mostrar medidor completo | Ctrl + Alt + 2 |
| Esconder medidor | Ctrl + Alt + 0 |
| Controles do medidor | Ctrl + Alt + 5 |
| Ligar ou desligar o limitador | Ctrl + Alt + 3 |
| Aumentar ou diminuir o limite | Ctrl + Alt + ↑ ou ↓ |
| Ligar ou desligar o registro de dados | Ctrl + Alt + 4 |
| Foto da tela inteira | Ctrl + Alt + A |
| Foto da janela em foco | Ctrl + Alt + J |
| Gravar Janela e parar | Ctrl + Alt + R |
| Gravar Tela e parar | Ctrl + Alt + T |
| Salvar replay | Ctrl + Alt + B |

Na foto: Enter ou Ctrl + C copia, Ctrl + S salva, Ctrl + Z desfaz e Esc cancela. Passe o mouse num botão para ver o que ele faz e a tecla. Foto fixada: arraste para mover, botão direito fecha.

## Limites

- Não use injeção em jogos online. A detecção de anti-cheat reconhece arquivos e títulos conhecidos, e não achar nada não prova que o jogo não tem. Em qualquer jogo, dá para escolher Driver como método na aba Medidor.
- Jogos em DirectX 9 e Vulkan usam o limite do driver, que vale ao reabrir.
- Jogo em tela cheia exclusiva precisa estar em modo janela antes do Fullscreenizer.
- Sem temperatura e consumo da CPU, porque exigem driver de kernel.
- Os perfis não prometem FPS. O valor é privacidade e menos ruído.
- Apps removidos voltam pela Loja, e a volta pode falhar.
- A nota de saúde é um indicador, não uma garantia: disco pode falhar sem aviso.
- Coluna de rede, saúde dos discos e leitura rápida do NTFS exigem o Prisma como administrador.
- Tudo que o Prisma muda no sistema fica registrado em `acoes.log`, na pasta de dados.
- H.264 não tem 10 bits: gravar em HDR10 usa HEVC.
- Conteúdo protegido, como Netflix, sai preto nas capturas.
- Compartilhar tela está em teste. Sem caminho direto nem VPN, a conexão passa pelos relés públicos da n0 (criptografada de ponta a ponta). O código curto fica 10 minutos em três relés públicos de nomes (dns.iroh.link, relay.pkarr.org e pkarr.pubky.org), assinado e sem nada que identifique você; se nenhum responder, o convite longo funciona sem eles. O microfone de quem assiste não vai.

## Requisitos

Windows 11 de 64 bits.

- Gravar vídeo exige codificador de vídeo na GPU (NVIDIA, AMD ou Intel) e, em edições N do Windows, o Pacote de Recursos de Mídia. Sem isso, só a gravação fica indisponível.
- Temperatura da GPU depende do driver da NVIDIA. As demais leituras funcionam em qualquer placa.

## Compilar

```bash
cargo build --release
```

Precisa do Rust e das Build Tools do Visual Studio.

## Programas de terceiros

Compartilhar tela usa bibliotecas de código aberto, sem mudanças:

- [iroh e iroh-dns](https://github.com/n0-computer/iroh) (MIT ou Apache-2.0), que fura o NAT e, sem caminho direto, passa a conexão pelos relés públicos da n0, sempre criptografada de ponta a ponta.
- [attohttpc](https://github.com/sbstp/attohttpc) (MPL-2.0), usado pelo iroh para abrir portas no roteador por UPnP. O código-fonte está no link.
- [tokio](https://github.com/tokio-rs/tokio) (MIT), [blake3](https://github.com/BLAKE3-team/BLAKE3) (CC0-1.0 ou Apache-2.0), [data-encoding](https://github.com/ia0/data-encoding) (MIT), [mini-nvcodec e mini-cuda](https://github.com/AdrianEddy/wgpu-interop) (MIT ou Apache-2.0), [vigem-client](https://github.com/CasualX/vigem-client) (MIT), [viiper-client](https://github.com/Alia5/VIIPER) (MIT), [url](https://github.com/servo/rust-url) (MIT ou Apache-2.0) e [webpki-roots](https://github.com/rustls/webpki-roots) (CDLA-Permissive-2.0, a lista de autoridades de certificado).

A lista de opções da tela Driver NVIDIA foi estudada no [NVIDIA Profile Inspector](https://github.com/Orbmu2k/nvidiaProfileInspector) (MIT, Orbmu2k e colaboradores): só ids e valores das opções, nenhum código.

O ViGEmBus e o VIIPER não vêm com o Prisma: quem quiser controles virtuais instala um deles por conta própria.

## Licença

[PolyForm Noncommercial 1.0.0](LICENSE). Você pode usar, alterar e compartilhar o Prisma para qualquer fim não comercial. Vender ou usar comercialmente exige permissão à parte do autor.

## Publicar uma versão

1. Atualize `version` no `Cargo.toml`, acrescente a seção no `CHANGELOG.md` e atualize os dois READMEs e os dois guias em `docs/guide`. O programa usa esses mesmos arquivos.
2. Faça o commit e envie uma etiqueta como `v0.3.0`.

O GitHub compila o `Prisma.exe` e cria a versão em rascunho.

3. No PC que guarda a chave de atualização, faça checkout da etiqueta e rode `tools/publish.ps1 -Version 0.3.0`. Ele compila de novo, assina o `Prisma.exe` com a chave de `tools/update-key.ps1`, troca os arquivos do rascunho e publica. Aí todas as cópias do Prisma oferecem a atualização.

O Prisma recusa atualização sem essa assinatura, então o acesso à conta do GitHub sozinho não publica uma versão. A chave secreta nunca vai para o git nem para o GitHub: fica em `%LOCALAPPDATA%\PrismaRelease`, cifrada pelo Windows para aquela conta. Guarde uma cópia feita com `tools/update-key.ps1 -Backup`; sem a chave, as cópias instaladas não conseguem mais se atualizar sozinhas.
