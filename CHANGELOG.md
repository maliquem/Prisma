# Changelog

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
