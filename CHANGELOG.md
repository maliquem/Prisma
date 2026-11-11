# Changelog

## Unreleased

- Meter: while the data log records, the compact bar shows a red dot with Recording and the time, and the full panel a red band below the game name; before, it was only a small note that other messages could cover. / Medidor: enquanto o registro de dados grava, a barra compacta mostra um ponto vermelho com Gravando e o tempo, e o painel completo uma faixa vermelha abaixo do nome do jogo; antes era só uma nota pequena, que outras mensagens podiam cobrir.
- Meter: each game in the list can pick a Windows power plan that turns on when it opens; the previous plan comes back when it closes. / Medidor: cada jogo da lista pode escolher um plano de energia do Windows, que liga quando ele abre; o plano de antes volta quando ele fecha.
- Meter: new option to take games out of the list after 7 to 90 days without opening, keeping their settings; they come back when opened, and Show all lists them. Off by default. / Medidor: nova opção de tirar da lista os jogos sem abrir há 7 a 90 dias, com as escolhas guardadas; eles voltam ao abrir, e Mostrar todos lista eles. Desligada por padrão.
- Meter: the power plan in the full panel is now a drop-down list instead of going one by one. / Medidor: o plano de energia no painel completo agora é uma lista suspensa, em vez de passar um por um.
- Fixed: when Windows frame readings failed to open, the meter kept retrying every 6 seconds without saying why, and the injected limiter never started because it waited for frames. The reason now goes to the actions log, and injection starts after 8 seconds of the game in focus even without readings. / Corrigido: quando a leitura de quadros do Windows não abria, o medidor tentava de novo a cada 6 segundos sem dizer o motivo, e o limitador por injeção nunca entrava, porque esperava quadros. Agora o motivo vai para o registro de ações, e a injeção entra depois de 8 segundos de jogo em foco mesmo sem leitura.
- New NVIDIA driver page, only with an NVIDIA card: driver options in the global profile or per game, like NVIDIA Profile Inspector, without SLI, 3D Vision, Ansel, CSAA, MFAA, debug and old OpenGL options. Each option is explained on hover, Default goes back to the value with nothing saved, options that need a newer driver are hidden, and a backup of every driver profile is saved before the first change, with a button to go back to one. / Nova tela Driver NVIDIA, só com placa NVIDIA: opções do driver no perfil global ou por jogo, como o NVIDIA Profile Inspector, sem SLI, 3D Vision, Ansel, CSAA, MFAA, depuração e OpenGL antigo. Cada opção é explicada ao passar o mouse, Padrão volta ao valor sem nada gravado, opções que pedem driver mais novo ficam escondidas, e uma cópia de todos os perfis do driver é guardada antes da primeira mudança, com um botão para voltar a ela.
- Fixed: removing a game from the Meter deleted the driver profile Prisma had made for it even when it had other settings in it; now the profile only goes away when empty. / Corrigido: tirar um jogo do Medidor apagava o perfil do driver feito pelo Prisma mesmo com outras opções nele; agora o perfil só sai quando está vazio.

- Share screen: new Mouse and keyboard button on the PCs this one watches, like Deskflow: this PC's mouse and keyboard go to up to two paired PCs next to it, with no video. Each screen goes on the left, in the middle or on the right under Client > Screens side by side, and the mouse crosses from one screen to the next; Scroll Lock (changeable in Shortcuts) locks it to the current screen, and copied text goes along with the copy and paste permission. Needs Prisma on both PCs. / Compartilhar tela: novo botão Mouse e teclado nos PCs que este assiste, como o Deskflow: o mouse e o teclado deste PC vão para até dois PCs pareados ao lado, sem vídeo. Cada tela fica na esquerda, no meio ou na direita em Cliente > Telas lado a lado, e o mouse passa de uma tela para a vizinha; Scroll Lock (muda em Atalhos) trava o mouse na tela atual, e o texto copiado vai junto com a permissão de copiar e colar. Precisa do Prisma nos dois PCs.
- Share screen: new Sound only button on the PCs this one watches, like AudioRelay: the paired PC's sound plays on this one, with no video, following the sound options in its Host tab. Needs the sound permission and Prisma on both PCs. / Compartilhar tela: novo botão Só o som nos PCs que este assiste, como o AudioRelay: o som do PC pareado toca neste, sem vídeo, seguindo as opções de som do Host de lá. Precisa da permissão de som e do Prisma nos dois PCs.
- Share screen: experimental PyroWave codec in the Host tab, for the local network or a VPN only. It barely compresses (about 200 Mbit/s at 1080p60), so the picture comes out almost identical and a lost packet costs only that frame. It never goes through the relay, and both PCs need this Prisma version or newer; otherwise automatic applies, and the Host tab warns. Its library (MIT) comes inside Prisma and is written to AppData\Local\Prisma\pyrowave, with its license, the first time the codec is used. With PyroWave, the bandwidth limit goes up to 700 Mbit/s. It also does HDR10, and 4:4:4 on any graphics card when the viewer asks for it (about 300 Mbit/s at 1080p60), so Ask for 4:4:4 now shows up on cards without the HEVC 4:4:4 decoder too. / Compartilhar tela: codec PyroWave experimental na aba Host, só para a rede local ou a VPN. Ele quase não comprime (uns 200 Mbit/s em 1080p60), então a imagem sai quase igual e um pacote perdido custa só aquele quadro. Nunca passa pelo relé e precisa desta versão do Prisma ou mais nova nos dois PCs; fora disso vale o automático, e a aba Host avisa. A biblioteca dele (MIT) vem dentro do Prisma e é gravada em AppData\Local\Prisma\pyrowave, com a licença, na primeira vez que o codec é usado. Com o PyroWave, o limite de banda vai até 700 Mbit/s. Ele também faz HDR10, e 4:4:4 em qualquer placa de vídeo quando quem assiste pede (uns 300 Mbit/s em 1080p60), então Pedir 4:4:4 agora aparece também nas placas sem o decodificador de HEVC 4:4:4.

## 0.14.0

- Fixed: the call app was still heard in the sound, because Windows only lets one program be left out and Prisma picked the first call app open (Telegram, open and silent, instead of Discord in a call). Prisma now leaves out the one that is playing and follows the change during the connection. / Corrigido: o programa de chamada continuava no som, porque o Windows só deixa tirar um programa e o Prisma escolhia o primeiro aberto (o Telegram, aberto e calado, em vez do Discord numa chamada). Agora sai o que está tocando, e a escolha acompanha a conexão.
- Fixed: the picture froze for good after changing the frames per second in the connection menu, or any other change that restarts the video, because the frame count started over and the viewer threw the new frames away as old. / Corrigido: a imagem congelava de vez depois de trocar os quadros por segundo no menu da conexão, ou qualquer outra mudança que recomeça o vídeo, porque a contagem dos quadros voltava ao começo e quem assiste jogava os quadros novos fora como velhos.
- Fixed: constant frame pacing did nothing with the Windows service (Desktop Duplication capture); a still screen now repeats the last frame at the usual pace there too. / Corrigido: o ritmo constante não fazia nada com o serviço do Windows (captura pela Desktop Duplication); a tela parada agora repete o último quadro no ritmo de sempre também ali.
- Share screen: host settings apply during the connection. Picture settings restart the video for a moment; frame pacing, sound and the mouse and keyboard rules change without restarting. / Compartilhar tela: as opções do host valem durante a conexão. As da imagem recomeçam o vídeo por um instante; ritmo, som e as regras do mouse e do teclado mudam sem recomeçar.
- Share screen: the connection menu now has VSync, ask for 4:4:4, controller in the background and keyboard as controller, all applied right away. The detailed meter shows the host's priority (lowest delay, balanced or quality). / Compartilhar tela: o menu da conexão agora tem VSync, pedir 4:4:4, controle em segundo plano e teclado como controle, tudo valendo na hora. O medidor detalhado mostra a prioridade do host (menor atraso, equilíbrio ou qualidade).
- Share screen: pick the graphics card on laptops with two. On the host, the card that encodes (by default the monitor's; another card gets the picture through one extra copy); on the viewer, the card that decodes and shows it. Video encoders and decoders now come from the chosen card. / Compartilhar tela: escolha a placa de vídeo em notebooks com duas. No host, a placa que codifica (por padrão a do monitor; outra placa recebe a imagem com uma cópia a mais); em quem assiste, a placa que decodifica e mostra. Os codificadores e decodificadores de vídeo agora são os da placa escolhida.

## 0.13.0

- New languages: Spanish, Russian and Simplified Chinese, in the program and in the GitHub page. Prisma starts in the Windows language and you can switch in Settings; the guide and these notes stay in English in the new languages. / Novos idiomas: espanhol, russo e chinês simplificado, no programa e na página do GitHub. O Prisma começa no idioma do Windows e você troca em Configurações; o guia e estas notas ficam em inglês nos idiomas novos.
- Fixed: the meter did not recognize games when the Power game rules were off, so FPS was not measured and the controls did not open. The meter now measures the program in focus when you press the shortcut and only appears through the shortcut. / Corrigido: o medidor não reconhecia jogos com as regras de modo jogo do Energia desligadas, então o FPS não era medido e os controles não abriam. Agora o medidor mede o programa em foco quando você aperta o atalho e só aparece pelo atalho.
- Fixed: Ctrl+Alt+5 took the focus away from the game, like an Alt+Tab, in games that do not use the monitor resolution. The game now stays in front while the meter controls are open. / Corrigido: o Ctrl+Alt+5 tirava o foco do jogo, como um Alt+Tab, em jogos que não usam a resolução do monitor. Agora o jogo continua na frente com os controles do medidor abertos.
- Meter: power plan in the full panel, with arrows to switch it; DLSS profile only for games that ship DLSS; the Windows Auto HDR per game in the game list, the same choice as the HDR tab; automatic HDR for all games, which never turns off an HDR that was already on. / Medidor: plano de energia no painel completo, com setas para trocar; perfil de DLSS só nos jogos que trazem o DLSS; o Auto HDR do Windows por jogo na lista de jogos, a mesma escolha da aba HDR; HDR automático em todos os jogos, que nunca desliga um HDR que já estava ligado.
- Data log: type the minutes to record (5 by default, 0 is no limit), and on NVIDIA cards it also records the GPU for optimization work: memory controller, power against the limit, what held the clock back, PCIe traffic, VRAM total, driver and clocks, with a summary reading. / Registro de dados: digite os minutos gravados (5 por padrão, 0 é sem limite), e em placas NVIDIA ele grava também a GPU para estudar otimização: controlador de memória, energia em relação ao limite, o que segurou o clock, tráfego PCIe, VRAM total, driver e clocks, com uma leitura no resumo.
- Sound: pin programs to keep them on the list when closed, with their volume, output and input saved; shortcuts that switch the Windows default output and input at once, as many as you add. / Som: fixe programas para eles ficarem na lista fechados, com volume, saída e entrada guardados; atalhos que trocam a saída e a entrada padrão do Windows de uma vez, quantos você adicionar.
- Share screen: paired devices split into two tabs (the PCs this one watches and the ones watching it), each one can be renamed, and permission changes apply right away for a connected device. / Compartilhar tela: os aparelhos pareados ficam em duas abas (os PCs que este assiste e os que assistem a ele), cada um pode ser renomeado, e mudar as permissões vale na hora para um aparelho conectado.
- Fixed: in window mode the viewer image could appear twice, with old frames around a smaller one, because resizing the window failed; switching to an HDR10 window failed for the same reason. / Corrigido: no modo janela, a imagem de quem assiste podia aparecer duplicada, com quadros velhos em volta de um menor, porque mudar o tamanho da janela falhava; passar a janela para HDR10 falhava pelo mesmo motivo.
- Fixed: the call app was sent in the sound when it was also picked as the only sound to send. Leaving the call app out now wins, and call apps leave that list while the option is on. / Corrigido: o programa de chamada ia no som quando também era escolhido como o único som a mandar. Tirar o programa de chamada agora vale mais, e os programas de chamada saem dessa lista enquanto a opção está ligada.
- Fixed: the connection menu froze the stream while open, and changing the frames per second through it could close the viewer. The detailed meter shows the FPS on its own line. / Corrigido: o menu da conexão congelava a transmissão enquanto aberto, e trocar os quadros por segundo por ele podia fechar a janela de quem assiste. O medidor detalhado mostra o FPS em linha própria.
- Fixed: the meter item for the data log read "Registry" in English, and choices in the meter game list could be cut off. Buttons and choices now shrink the text a little instead of cutting it. / Corrigido: o item do registro de dados no medidor aparecia como "Registry" em inglês, e escolhas na lista de jogos do medidor podiam ficar cortadas. Botões e escolhas agora diminuem um pouco a letra em vez de cortar.
- Fixed: clicking most topics on the left of the guide did nothing. It now jumps to the section. / Corrigido: clicar na maioria dos tópicos à esquerda do guia não fazia nada. Agora pula para a seção.

## 0.12.1

- Fixed: updating Prisma did not update the copy that the Windows service runs, so with the service installed, sharing kept the 0.11.0 error and stopped as soon as it started. Prisma now swaps that copy by itself when it opens as administrator. / Corrigido: atualizar o Prisma não atualizava a cópia que o serviço do Windows roda, então, com o serviço instalado, o compartilhamento continuava com o erro da 0.11.0 e parava logo ao ligar. Agora o Prisma troca essa cópia sozinho quando abre como administrador.
- The Share screen page warns when the service keeps crashing. / A página Compartilhar tela avisa quando o serviço fica caindo.
- New connection menu, in Prisma's look: immersive mode (off, mouse, keyboard or both), frames per second, meter, warning, sound, Ctrl+Alt+Del and Disconnect, each with its shortcut beside it. / Menu da conexão novo, com o visual do Prisma: modo imersivo (desligado, mouse, teclado ou os dois), quadros por segundo, medidor, aviso, som, Ctrl+Alt+Del e Desconectar, cada um com o atalho ao lado.
- New viewer shortcuts: disconnect (Ctrl+Alt+Shift+D), immersive mode on and off (Ctrl+Alt+Shift+I), switch the meter (Ctrl+Alt+Shift+M) and sound on and off (Ctrl+Alt+Shift+S), all changeable in the Shortcuts tab. The Viewer tab also picks the frames per second asked of the host. / Atalhos novos em quem assiste: desconectar (Ctrl+Alt+Shift+D), ligar e desligar o modo imersivo (Ctrl+Alt+Shift+I), trocar o medidor (Ctrl+Alt+Shift+M) e ligar e desligar o som (Ctrl+Alt+Shift+S), todos trocáveis na aba Atalhos. A aba Cliente também escolhe os quadros por segundo pedidos ao host.
- New Connected tab on the host, shown while someone is connected: change each device's access right away, disconnect it or ban it. Windows shows a notification when someone connects or leaves. / Aba Conectados nova no host, que aparece enquanto alguém está conectado: muda o acesso de cada aparelho na hora, desconecta ou bane. O Windows mostra uma notificação quando alguém conecta ou sai.
- Less GPU work on the host when the screen changes a lot (Alt+Tab, minimizing): only the newest captured frame is copied, and the HDR conversion and the cursor run only on frames that are sent. / Menos trabalho da GPU no host quando a tela muda muito (Alt+Tab, minimizar): só o quadro capturado mais novo é copiado, e a conversão de HDR e o cursor rodam só nos quadros enviados.
- Meter: turning it on by shortcut adds the game to the list, and the same shortcut again hides it. Add from the program list now brings Steam and Epic games. / Medidor: ligar pelo atalho põe o jogo na lista, e o mesmo atalho de novo esconde. Adicionar da lista de programas agora traz os jogos da Steam e da Epic.

## 0.12.0

- Fixed: in 0.11.0, sharing stopped right after it started, so pairing never got to ask for permission. / Corrigido: na 0.11.0, o compartilhamento parava logo depois de ligar, e o pareamento nem chegava a pedir permissão.
- Short invite code, like K7M2-QX9P, instead of the long invite. Create invite opens a window with Copy code, Copy link and Share (the Windows share window). The link opens Prisma on the other PC, which asks before pairing. The long invite is still there for when the code relays do not answer. / Código de convite curto, como K7M2-QX9P, no lugar do convite longo. Criar convite abre uma janela com Copiar código, Copiar link e Compartilhar (a janela de compartilhar do Windows). O link abre o Prisma no outro PC, que pergunta antes de parear. O convite longo continua lá para quando os relés do código não respondem.
- Share screen settings, in new Host, Viewer, Network, Shortcuts and Controllers tabs: this PC's name, monitor, maximum resolution, frames per second, frame pacing, priority, codec, bandwidth limit, 10-bit and 4:4:4, which sound is sent (whole PC or one program, with the call app left out), how many people can join, one person at a time on the mouse and keyboard, the host's mouse and keyboard first, and refusing connections while this PC is viewing another. / Opções de Compartilhar tela, nas abas novas Host, Cliente, Rede, Atalhos e Controles: nome deste PC, monitor, resolução máxima, quadros por segundo, ritmo dos quadros, prioridade, codec, limite de banda, 10 bits e 4:4:4, que som vai (o PC todo ou um programa, sem o programa de chamada), quantas pessoas entram, uma pessoa por vez no mouse e no teclado, o mouse e o teclado do host primeiro e recusar conexões enquanto este PC assiste a outro.
- Viewer window: borderless fullscreen, VSync, immersive mode (the Windows key and Alt+Tab go to the other PC), a latency meter (simple or detailed), a warning when the connection gets worse, and a connection menu on Ctrl+Alt+Shift+P. / Janela de quem assiste: tela cheia sem borda, VSync, modo imersivo (a tecla Windows e o Alt+Tab vão para o outro PC), medidor de latência (simples ou detalhado), aviso quando a conexão piora e um menu da conexão no Ctrl+Alt+Shift+P.
- Fixed UDP port and the router port request (UPnP) can be set, every share screen shortcut can be changed, and the Controllers tab shows the connected controllers with battery and the viewers' virtual ones. / Dá para fixar a porta UDP e escolher pedir a porta ao roteador (UPnP), todos os atalhos de compartilhar tela podem ser trocados, e a aba Controles mostra os controles ligados, com a bateria, e os virtuais de quem assiste.

## 0.11.0

- New Share screen page (in testing): show and control another PC over the internet, for remote access or local co-op from a distance, with no account and no Prisma server. Pair once with an invite and a confirmation code; after that, the connection goes direct, through your VPN or through iroh's public relays, always end-to-end encrypted. / Nova página Compartilhar tela (em teste): mostre e controle outro PC pela internet, para acesso remoto ou co-op local à distância, sem conta e sem servidor do Prisma. Pareie uma vez com um convite e um código de confirmação; depois, a conexão vai direto, pela sua VPN ou pelos relés públicos do iroh, sempre criptografada de ponta a ponta.
- Permissions per device: sound, controller, keyboard, mouse, copy and paste (text, both ways) and secure screen, plus trust or ask on every connection. / Permissões por aparelho: som, controle, teclado, mouse, copiar e colar (texto, nos dois sentidos) e tela segura, além de confiar ou perguntar a cada conexão.
- Co-op for up to 4 people: each viewer's controller becomes a virtual Xbox controller on the host, with vibration, through ViGEmBus or VIIPER installed by you. Virtual controllers leave when the game in front has an anti-cheat. / Co-op para até 4 pessoas: o controle de cada pessoa vira um controle de Xbox virtual no host, com vibração, pelo ViGEmBus ou pelo VIIPER instalado por você. Os controles virtuais saem quando o jogo em primeiro plano tem anti-cheat.
- HDR on the host is converted for viewers without HDR, or, if you choose, turned off while they watch and turned back on afterwards, even after a crash. / O HDR do host é convertido para quem não tem HDR, ou, se você escolher, desligado enquanto a pessoa assiste e ligado de volta depois, mesmo depois de uma queda.
- Optional Windows service, only with your consent as administrator, to see and use the UAC prompt, the lock screen and the PIN screen remotely, even after a restart, and to send Ctrl+Alt+Del (Ctrl+Alt+End in the viewer). / Serviço opcional do Windows, só com o seu consentimento como administrador, para ver e usar de longe o aviso do UAC, a tela de bloqueio e a do PIN, mesmo depois de reiniciar, e mandar Ctrl+Alt+Del (Ctrl+Alt+End em quem assiste).
- Ctrl+Alt+Shift+Q on the host disconnects everyone at once, and the page shows who is connected. / Ctrl+Alt+Shift+Q no host desconecta todo mundo na hora, e a página mostra quem está conectado.

## 0.10.0

- Search shows installed programs first (on by default; Ctrl+Shift+Enter opens as administrator). / A pesquisa mostra os programas instalados primeiro (vem ligado; Ctrl+Shift+Enter abre como administrador).
- Search can send the text to the default browser or to an AI: Google, Bing, DuckDuckGo, Brave Search, Claude, ChatGPT, Gemini, Copilot, Perplexity, Grok and DeepSeek, chosen in Options. Questions come at the top; installed AIs open in their app. On by default with Google and the installed AIs. / A pesquisa leva o texto ao navegador padrão ou a uma IA: Google, Bing, DuckDuckGo, Brave Search, Claude, ChatGPT, Gemini, Copilot, Perplexity, Grok e DeepSeek, escolhidos em Opções. Perguntas vêm no topo; IAs instaladas abrem no aplicativo. Vem ligado com o Google e as IAs instaladas.
- Commands: start with > to run in PowerShell or CMD, without administrator or, with Ctrl+Shift+Enter, as administrator, with suggestions from the Windows command list and Tab to complete. Optional "Complete like the terminal" uses a hidden PowerShell for paths and history. Off by default. / Comandos: comece com > para rodar no PowerShell ou no CMD, sem administrador ou, com Ctrl+Shift+Enter, como administrador, com sugestões da lista de comandos do Windows e Tab para completar. O opcional "Completar como o terminal" usa um PowerShell escondido para caminhos e histórico. Vem desligado.
- Fixed: choosing no disk in search was lost when Prisma restarted. / Corrigido: escolher nenhum disco na pesquisa se perdia ao reiniciar o Prisma.

## 0.9.0

- New Search tab: finds files and folders by name on all disks as you type, with a pop-up on Ctrl + Alt + Space over any program. As administrator, the index is read from the NTFS table in seconds and follows changes live; disks with health problems stay out unless you include them. Off by default. / Nova aba Pesquisar: acha arquivos e pastas pelo nome em todos os discos enquanto você digita, com uma janelinha no Ctrl + Alt + Espaço por cima de qualquer programa. Como administrador, o índice sai da tabela do NTFS em segundos e acompanha as mudanças na hora; discos com problema de saúde ficam de fora, a não ser que você inclua. Vem desligada.
- Ctrl+V and Ctrl+Backspace work in search boxes. / Ctrl+V e Ctrl+Backspace funcionam nas caixas de busca.

## 0.8.1

- The question to optimize when a game opens now comes off, and can be turned on in Power. / A pergunta para otimizar quando um jogo abre agora vem desligada, e dá para ligar em Energia.
- The meter only shows up when you ask: by shortcut, or on its own for the games in its list. / O medidor só aparece quando você pede: pelo atalho, ou sozinho nos jogos da lista dele.
- Fewer programs mistaken for games: the program in focus has to use the GPU itself, and browsers, video players, Parsec, Discord, OBS and launchers never count. / Menos programas confundidos com jogo: o programa em foco precisa usar a GPU ele mesmo, e navegadores, players de vídeo, Parsec, Discord, OBS e lançadores nunca contam.
- Replace Task Manager works again where Windows protects the change even from administrators, as in recent test builds: Prisma tells you and catches Ctrl+Shift+Esc itself while it is open. Ctrl+Alt+Del and the taskbar menu still open the Windows one. / Substituir o Gerenciador de Tarefas volta a funcionar onde o Windows protege a troca até de administrador, como nas versões de teste recentes: o Prisma avisa e pega o Ctrl+Shift+Esc ele mesmo, enquanto estiver aberto. O Ctrl+Alt+Del e o menu da barra de tarefas ainda abrem o do Windows.

## 0.8.0

- Search in the Monitor process list, by name or PID, and a PID column in the Programs tab too. / Busca na lista de processos do Monitor, por nome ou PID, e coluna PID também na aba Programas.
- Restart Explorer button in the Monitor, for when the taskbar or desktop freezes, with an optional shortcut in Settings that you press twice to confirm. / Botão Reiniciar o Explorer no Monitor, para quando a barra de tarefas ou a área de trabalho travam, com um atalho opcional em Configurações que se aperta duas vezes para confirmar.
- What's new screen after an update, with every version since the one you had. It is also in Settings, next to the version. / Tela de Novidades depois de atualizar, com todas as versões desde a que você usava. Ela também fica em Configurações, ao lado da versão.
- Safer portable copy: Prisma loads Windows DLLs and runs Windows tools only from the Windows folder, never from its own folder. A file with the same name downloaded next to it no longer runs in its place, not even as administrator. / Versão portátil mais segura: o Prisma carrega as DLLs e roda as ferramentas do Windows só da pasta do Windows, nunca da própria pasta. Um arquivo com o mesmo nome baixado ao lado dele não roda mais no lugar, nem como administrador.
- Replace Task Manager is now for the installed version only. In an ordinary folder, any program could swap the file Ctrl+Shift+Esc opens, for every account on the PC. / Substituir o Gerenciador de Tarefas agora é só da versão instalada. Numa pasta comum, qualquer programa poderia trocar o arquivo que o Ctrl+Shift+Esc abre, para todas as contas do PC.
- Updates are now signed. Prisma checks the signature before replacing itself and refuses a file without it, so someone who gets into the GitHub account cannot ship a fake version. / As atualizações agora são assinadas. O Prisma confere a assinatura antes de se trocar e recusa arquivo sem ela, então quem entrar na conta do GitHub não consegue publicar uma versão falsa.
- Program Files, AppData, the Startup folders and the account name now come from Windows itself, not from environment variables that any program can change. Tasks that open as administrator are only created for the installed copy. / Arquivos de Programas, AppData, as pastas Inicializar e o nome da conta agora vêm do próprio Windows, e não de variáveis de ambiente que qualquer programa muda. Tarefas que abrem como administrador só são criadas para a cópia instalada.

## 0.7.0

- Welcome screen on the first open: install, give administrator permission and replace Task Manager, all checked, with one Windows prompt for everything. It shows once unless you uncheck Don't show again. / Tela de boas-vindas na primeira abertura: instalar, dar permissão de administrador e substituir o Gerenciador de Tarefas, tudo marcado, com uma permissão do Windows para tudo. Ela aparece uma vez, a não ser que você desmarque Não mostrar novamente.
- Guide in Portuguese and English, with images: what each feature does, and what it can and can't do. / Guia em português e inglês, com imagens: o que cada função faz, o que dá e o que não dá para fazer.
- The Guide and Report a bug buttons moved next to the Prisma name, with a tip on hover. / Os botões Guia e Reportar bug foram para o lado do nome Prisma, com dica ao passar o mouse.
- Updates are checked once a day, and Prisma asks when you open the window, never from the tray. Don't show again turns the search off, and the option is in Settings. / A busca de atualização roda uma vez por dia, e o Prisma pergunta quando você abre a janela, nunca pela bandeja. Não mostrar de novo desliga a busca, e a opção fica em Configurações.
- Type the FPS limit in the full meter, and bigger CPU and GPU percentages. The welcome screen also lets you choose the language. / Digite o limite de FPS no medidor completo, e as porcentagens de CPU e GPU estão maiores. A tela de boas-vindas também deixa escolher o idioma.
- Always open as administrator in Settings. Installed, it no longer asks each time. / Sempre abrir como administrador em Configurações. Instalado, não pergunta mais a cada abertura.
- Starting with Windows now works on battery, stays open past 72 hours and runs at normal priority. / O início com o Windows agora funciona na bateria, passa de 72 horas aberto e roda em prioridade normal.

## 0.6.0

- The limit now changes right away: by default a small Prisma module enters the game when the limiter is on, in DirectX 10, 11 and 12. It does not enter games with a detected anti-cheat. If it fails or the game crashes, the limit moves to the NVIDIA driver and applies on reopening. Do not use it online. / O limite agora muda na hora: por padrão, um pequeno módulo do Prisma entra no jogo quando o limitador está ligado, em DirectX 10, 11 e 12. Ele não entra em jogos com anti-cheat detectado. Se falhar ou o jogo cair, o limite passa para o driver NVIDIA e vale ao reabrir. Não use em jogos online.
- Buttons in the full meter for mode, limit, unit, method and DLSS profile. Ctrl + Alt + 5 unlocks mouse and arrow keys for them. / Botões no medidor completo para modo, limite, unidade, método e perfil de DLSS. Ctrl + Alt + 5 libera mouse e setas para eles.
- Anti-cheat detection recognizes more files and online titles. / A detecção de anti-cheat reconhece mais arquivos e títulos online.
- Choose where the compact and full meters sit, and the compact bar is closer to the top edge. / Escolha onde ficam o medidor compacto e o completo, e a barra compacta está mais perto da borda de cima.
- The frame time line has a fixed scale and ignores variations up to 1 ms. / A linha de frame time tem escala fixa e ignora variações de até 1 ms.
- The Optimize question is smaller, sits in the top right corner and leaves after 10 seconds. / A pergunta de otimizar é menor, fica no canto superior direito e some em 10 segundos.
- The add program list opens in its own window. / A lista de adicionar programas abre em janela própria.
- Buy me a coffee button in the side menu. / Botão Buy me a coffee no menu lateral.

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
