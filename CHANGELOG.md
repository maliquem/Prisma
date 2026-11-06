# Changelog

## 0.5.2

- Fixed the Meter freezing on an old FPS number. With no new frames it now shows a dash and comes back by itself, and it restarts its reading if Windows stops it. / Corrigido o Medidor preso em um número antigo de FPS. Sem quadros novos ele mostra um traço e volta sozinho, e religa a leitura se o Windows a interromper.
- Hiding the meter with the shortcut clears the measurements, so FPS, averages and lows start over. / Esconder o medidor pelo atalho zera as medições, e FPS, médias e lows recomeçam.
- Limiter shortcuts during the game, for any open game: Ctrl + Alt + 3 turns it on or off, Ctrl + Alt + ↑ and ↓ change the limit. The full meter shows the new value and the one active now. The driver reads the limit when the game opens, so changes apply when it reopens. / Atalhos do limitador durante o jogo, para qualquer jogo aberto: Ctrl + Alt + 3 liga ou desliga, Ctrl + Alt + ↑ e ↓ mudam o limite. O medidor completo mostra o novo valor e o ativo agora. O driver lê o limite quando o jogo abre, então a mudança vale ao reabrir.
- New optional data log: every frame, every second and the CPU of each game thread saved in Documents\Prisma\Logs, with a summary at the end. Choose 1, 5 or 10 minutes of game time, or no limit, and start it from the Meter page or with Ctrl + Alt + 4. / Novo registro de dados opcional: cada quadro, cada segundo e a CPU de cada thread do jogo salvos em Documentos\Prisma\Logs, com um resumo no final. Escolha 1, 5 ou 10 minutos de jogo, ou sem limite, e ligue na aba Medidor ou com Ctrl + Alt + 4.
- Frame timing no longer depends on the PC's clock frequency. / A medição de quadros não depende mais da frequência do relógio do PC.

## 0.5.1

- New Report a bug page: choose the feature and severity, describe the problem, add a picture, and your browser opens GitHub with everything written. You confirm before anything is published. / Nova aba Reportar bug: escolha a função e a gravidade, descreva o problema, junte uma foto, e o navegador abre o GitHub com tudo escrito. Nada é publicado antes de você confirmar.
- The report can carry PC data and a short log, with no user name, PC name or file paths, and you can see it before sending or leave it out. / O relatório pode levar dados do PC e um registro curto, sem nome de usuário, nome do PC ou caminhos de arquivo, e dá para ver antes de enviar ou deixar de fora.
- The side menu no longer overlaps the power card on small windows. / O menu lateral não invade mais o cartão de energia em janelas pequenas.

## 0.5.0

- New Meter page: compact bar and full panel over the game, with FPS, 1% and 0.1% lows, frame time, CPU, GPU, VRAM and RAM. Nothing runs inside the game. / Nova aba Medidor: barra compacta e painel completo sobre o jogo, com FPS, 1% e 0,1% low, frame time, CPU, GPU, VRAM e RAM. Nada roda dentro do jogo.
- FPS limit per game through the NVIDIA driver, manual or automatic after 5 minutes of play. It applies the next time the game opens. / Limite de FPS por jogo pelo driver NVIDIA, manual ou automático depois de 5 minutos de jogo. Vale na próxima abertura do jogo.
- DLSS profile per game, upscaler and frame generation detection, Reflex detection, automatic HDR per game. / Perfil de DLSS por jogo, detecção de upscaler, geração de quadros e Reflex, e HDR automático por jogo.
- Keep the sound awake on the Sound page, against the cut at the start of sounds on HDMI and Dolby. / Manter o som acordado na aba Som, contra o corte no começo do som em HDMI e Dolby.
- Power plans: select a plan, then Import, Export or Edit, or right-click it. / Planos de energia: selecione um plano e use Importar, Exportar ou Editar, ou clique com o botão direito.

## 0.4.1

- Power plans can be edited with every option Windows registers, including hidden ones, and imported or exported as .pow files. The first edit keeps a copy of the original plan. / Planos de energia editáveis com todas as opções que o Windows registra, inclusive as ocultas, e importação e exportação em arquivos .pow. A primeira edição guarda uma cópia do plano original.
- New shortcut for a photo of the focused window, opening the editor with the window selected. / Novo atalho para foto da janela em foco, abrindo o editor com a janela selecionada.
- Record Window and Record Screen are now separate buttons and shortcuts. / Gravar Janela e Gravar Tela agora são botões e atalhos separados.
- The photo editor shows what each button does and its key, Ctrl + C copies, each tool has a key, and the cursor is a cross. / O editor de foto mostra o que cada botão faz e a tecla, Ctrl + C copia, cada ferramenta tem uma tecla e o cursor é uma cruz.
- Fixed scrolling in the Auto HDR add list. / Corrigida a rolagem da lista de adicionar do Auto HDR.

## 0.4.0

- Auto HDR per game on the HDR page: force it, turn it off or leave it to Windows, for DX11 and DX12 games, with nothing injected into the game. / Auto HDR por jogo na aba HDR: forçar, desligar ou deixar com o Windows, em jogos DX11 e DX12, sem injetar nada no jogo.

## 0.3.9

- Sound delay test on the HDR page: a flash and a click in fixed cycles, to compare the delay with and without spatial sound. / Teste de atraso do som na aba HDR: clarão e clique em ciclos fixos, para comparar o atraso com e sem som espacial.

## 0.3.8

- Calibration patterns now go through Windows composition, like the Windows calibration app. The T key switches to the direct path to compare. / Os padrões da calibração agora passam pela composição do Windows, como no app de calibração. A tecla T troca para o caminho direto, para comparar.

## 0.3.7

- Fixed the peak patterns: the dark square now sits on a full-signal background, so it vanishes at the point where the display clips. / Corrigidos os padrões de pico: o quadrado escuro agora fica sobre um fundo no máximo do sinal e some no ponto em que a tela corta.
- The nits counter changes on every step, without getting stuck. / O contador de nits muda a cada passo, sem travar.

## 0.3.6

- HDR calibration of your own: exact HDR test patterns for black, peak at five window sizes and each color channel, then a profile with the display's real primaries, saved per TV picture mode. / Calibração própria de HDR: padrões exatos para preto, pico em cinco tamanhos de janela e cada canal de cor, e um perfil com as primárias reais da tela, salvo por modo de imagem da TV.
- A result where the shape never vanishes is refused, because the TV is rolling off instead of clipping. / Resultado em que a forma nunca some é recusado, porque a TV está suavizando em vez de cortar.

## 0.3.5

- New HDR page: reads the display, the calibration profile, declared brightness, Dolby Vision and sound, and warns in plain words. It changes nothing on the PC. / Nova aba HDR: lê a tela, o perfil de calibração, o brilho declarado, o Dolby Vision e o som, e avisa em palavras simples. Ela não muda nada no PC.

## 0.3.4

- Fullscreenizer adds programs from the list or by .exe, like Sound. / Fullscreenizer adiciona programas pela lista ou pelo .exe, igual ao Som.
- The program list now has the Start menu, Microsoft Store apps and whatever is open. / A lista de programas agora traz o menu Iniciar, aplicativos da Microsoft Store e o que está aberto.
- Updates download in parts, about three times faster. / As atualizações baixam em partes, cerca de três vezes mais rápido.
- Monitor no longer shows cut names such as Spotify8FileV. / O Monitor não mostra mais nomes cortados como Spotify8FileV.

## 0.3.3

- Monitor sorts by name. / Monitor ordena por nome.
- Force close shortcut for the focused program, Alt+F4 by default. / Atalho para fechar à força o programa em foco, Alt+F4 por padrão.
- Sound page: choose output and input devices per program, and add programs by hand. / Aba Som: aparelhos de saída e de entrada por programa, com programas adicionados à mão.
- Program icons on Sound, Optimize and Fullscreenizer. / Ícones de programas em Som, Otimizar e Fullscreenizer.
- Scroll bar no longer touches list items. / A barra de rolagem não encosta mais nos itens.

## 0.3.2

- Installing now turns on starting with Windows, and it can be turned off in Settings. / Instalar agora liga o início com o Windows, que pode ser desligado em Configurações.

## 0.3.1

- Fixed the Start menu shortcut, so Windows search finds the installed Prisma. / Corrigido o atalho do menu Iniciar, para a pesquisa do Windows achar o Prisma instalado.

## 0.3.0

- Fixed the crash when Prisma closed, which also made installing look like it did nothing. / Corrigida a queda ao fechar o Prisma, que também fazia a instalação parecer não funcionar.
- Monitor: right-click menu and Details tab with every process. / Monitor: menu do botão direito e aba Detalhes com cada processo.
- Draggable scroll bar on every list. / Barra de rolagem para arrastar em todas as listas.
- Confirmation pop-ups instead of clicking twice on the same button. / Popups de confirmação no lugar de dois cliques no mesmo botão.
- Services: right-click menu with start, stop, restart and properties. / Serviços: menu do botão direito com iniciar, parar, reiniciar e propriedades.
- Uninstall: real step-by-step, wider leftover search and a result message. / Desinstalar: passo a passo real, busca de restos mais ampla e mensagem de resultado.
- Each tool keeps its own log. / Cada ferramenta guarda o próprio log.
- Power: choose the percentage and time for CPU and GPU load. / Energia: porcentagem e tempo de carga configuráveis.
- Fullscreenizer: saved program list with Automatic option. / Fullscreenizer: lista de programas salvos com opção Automático.
- Capture: Print Screen can be used as a shortcut, and a window can be recorded with only its own sound. / Captura: aceita Print Screen e grava só o som da janela.
- Button to turn the Task Manager replacement on and off. / Botão para ligar e desligar a troca do Gerenciador de Tarefas.
- Default button on each page. / Botão Padrão em cada tela.

## 0.2.0

- Updates from GitHub, in Settings. / Atualização pelo GitHub, em Configurações.
- English and Brazilian Portuguese, with a language selector. / Inglês e português do Brasil, com seletor de idioma.
- Sort Startup by name or state, and Uninstall by name, size or date. / Ordenação em Inicialização e Desinstalar.
- Right-click a service to open its Windows properties. / Botão direito abre as propriedades do serviço.
- Step-by-step log in Windows tweaks and Tools. / Log passo a passo em Ajustes do Windows e Ferramentas.
- Local crash report. / Relatório local de falha.
- Fixed dropdown menus that closed as soon as they opened. / Corrigidos os menus suspensos que fechavam ao abrir.

## 0.1.0

- First version: Monitor, Disks and networks, Disk space, Startup, Services, Uninstall, Windows tweaks, Tools, Optimize, Power, Fullscreenizer, Volume, Capture, Replay and Widget.
