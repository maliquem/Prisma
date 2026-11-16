# Estudo: widget preso na barra de tarefas (canto esquerdo)

Pedido: "Veja possibilidade do widget ficar acoplado na barra de tarefas do lado esquerdo."
Estudo apenas; nenhum código de produto mudou.

## Como o widget é hoje

- `Widget::new` (`src/overlays.rs`) cria a janela com `create_overlay` (`src/ui/layered.rs`):
  `WS_POPUP` + `WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`, desenhada por
  `Layered::paint` com `UpdateLayeredWindow` (alfa por pixel, Direct2D).
- Posição livre em `cfg.widget_x/widget_y` (arrastar com `WM_NCHITTEST → HTCAPTION`, salva em `WM_EXITSIZEMOVE`).
- `Widget::update` é chamado a cada leitura do monitor e já reafirma `HWND_TOPMOST` com `SetWindowPos`.
- O estilo 0 é uma faixa horizontal de 36 DIP de altura: cabe na barra do Windows 11 (48 DIP no tamanho
  padrão). Os estilos 1 e 2 são altos demais para a barra.
- A escala já vem de `GetDpiForWindow`, por monitor.

## Jeitos possíveis no Windows 11

### 1. Janela topmost própria posicionada sobre a barra (recomendado)

Continua a janela que já existe; só a posição passa a ser calculada:

- `SHAppBarMessage(ABM_GETTASKBARPOS)` dá o retângulo e o lado (`uEdge`) da barra principal;
  `ABM_GETSTATE` diz se ela se esconde sozinha. Barras dos outros monitores: `FindWindowEx` em
  `Shell_SecondaryTrayWnd` + `GetWindowRect`.
- Encaixe: x = borda esquerda da barra + margem, y = centro vertical da barra − metade da altura do
  widget. Altura do widget igual à da barra menos 2×4 px; fundo transparente ou da cor da barra.
- Atualizar a posição quando: muda o tamanho/DPI (`WM_DPICHANGED`, `WM_DISPLAYCHANGE`,
  `WM_SETTINGCHANGE` com `SPI_SETWORKAREA`), o Explorer reinicia (mensagem registrada `"TaskbarCreated"`).

Funciona: é o que vários apps fazem no Windows 11 (veja abaixo) e não depende de detalhes internos do Explorer.

Riscos:
- **Some atrás da barra ao clicar nela.** A barra também é topmost; clicar nela a sobe na ordem Z. O
  Prisma já reafirma `HWND_TOPMOST` a cada atualização (~1 s), então o widget volta, mas pode piscar
  por até um ciclo. Remédio barato: `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` e, quando o primeiro
  plano for `Shell_TrayWnd`/menu Iniciar, reafirmar na hora. Iniciar, busca e central de notificações
  ficam acima de qualquer topmost comum (banda de z do shell), o que é o comportamento desejado.
- **Tela cheia.** Um jogo em tela cheia exclusiva cobre tudo; em janela sem bordas o widget topmost
  apareceria por cima do jogo. Esconder quando `SHQueryUserNotificationState` disser
  `QUNS_BUSY`/`QUNS_RUNNING_D3D_FULL_SCREEN`, ou quando a janela em primeiro plano cobrir o monitor
  inteiro (a barra também some nesse caso, então o widget deve sumir junto).
- **Barra escondida automaticamente.** Com auto-hide, o widget ficaria flutuando sobre a área vazia.
  Opção simples: no modo barra, se `ABM_GETSTATE` tiver `ABS_AUTOHIDE`, esconder o widget quando o
  retângulo da barra estiver fora da tela (barra recolhida) — checar no mesmo tique.
- **Vários monitores.** A barra principal vem do `SHAppBarMessage`; as outras de `Shell_SecondaryTrayWnd`.
  Primeira versão: só no monitor da barra principal (ou o monitor escolhido), sem cópias por tela.
- **Widget de clima do Windows** ocupa exatamente o canto esquerdo quando os ícones estão centralizados.
  Não há API para saber a largura dele; o jeito é a usuária desligar "Widgets" nas configurações da
  barra (o Prisma pode só avisar). Com ícones alinhados à esquerda, o canto esquerdo é do botão Iniciar
  e o widget teria de ir para perto da bandeja.
- **Ícones centralizados crescendo.** Com muitos apps abertos, os ícones centrais podem chegar ao canto
  e ficar por baixo do widget. Aceitável com a faixa estreita; dá para ler a largura de
  `MSTaskListWClass`, mas no Windows 11 esse filho é XAML e o retângulo nem sempre corresponde aos ícones.
- **Captura.** O widget já sai das capturas (`hide_from_capture`), igual hoje.

Custo: pequeno. Nada novo no desenho; só posição, um hook de primeiro plano e as regras de esconder.

### 2. `SetParent` na `Shell_TrayWnd`

Tornar o widget filho da janela da barra (`WS_CHILD`, `SetParent(hwnd, FindWindow("Shell_TrayWnd"))`).

- Funciona: é o caminho que as bibliotecas feitas para o Windows 11 usam (Deskband11Lib,
  ComposeWindowsTaskBar, abaixo). A vantagem é que o widget sobe e desce junto com a barra (auto-hide,
  tela cheia e ordem Z resolvidos de graça).
- A barra do Windows 11 é desenhada em XAML por cima da `Shell_TrayWnd`; a largura do Iniciar, da
  pesquisa e do botão de widgets muda entre builds, e as bibliotecas usam UI Automation para achar esses
  botões e recalcular a posição várias vezes por segundo. (Não achei fonte oficial sobre como um filho
  Win32 se compõe com a parte XAML; tratar como risco a testar.)
- Janela em camadas como filho só funciona a partir do Windows 8, e o `UpdateLayeredWindow` passa a
  usar coordenadas do pai: precisa mudar `Layered::paint`.
- O processo do Prisma prende uma janela dentro do `explorer.exe`: se o Prisma travar, a barra pode
  engasgar; se o Explorer reiniciar, o widget some e precisa ser recriado ao receber `"TaskbarCreated"`.
- Diferença de DPI/integridade entre processos: se o Prisma rodar elevado e o Explorer não, `SetParent`
  falha (UIPI). O Prisma roda sem elevação, então ok, mas é mais uma condição.

Custo: médio, risco alto e quebra a cada atualização grande do Windows.

### 3. Deskband (`IDeskBand`)

Removido no Windows 11: a barra nova não carrega deskbands. Só volta com a barra antiga (ExplorerPatcher
ou similares), que a usuária não usa. Além disso exigiria uma DLL COM registrada no Windows, o que o
Prisma não faz. **Descartado.**

## Como outros apps fazem

Pesquisa só de leitura (páginas e código no GitHub, nada baixado).

- **TrafficMonitor** (`TrafficMonitor/TaskBarDlg.cpp`): acha `Shell_TrayWnd` (e as barras secundárias),
  lê o retângulo com `GetWindowRect`, posiciona com `MoveWindow` em coordenadas de tela e reafirma
  `SetWindowPos(wndTopMost)` quando a barra recebe o foco — ou seja, o mesmo remédio de ordem Z do
  jeito 1. Não confirmei se a parte do arquivo que não li faz `SetParent`. O histórico de versões mostra
  o custo de manutenção: 1.84/1.85 corrigem sobreposição com os widgets do Windows 11 dos dois lados,
  1.84.1 corrige posição com botão de pesquisa mais largo, 1.85 corrige posição em tela de toque a partir
  da build 22621, 1.85.1 cria opção para evitar sobreposição, 1.51/1.77 tratam o Explorer reiniciando,
  1.86 traz monitor secundário.
  Fontes: github.com/zhongyang219/TrafficMonitor (TaskBarDlg.cpp, UpdateLog/update_log.md).
- **TaskbarX**: feito para o Windows 10; move os ícones da barra e não cria janela própria (inferido;
  o README não explica a técnica). No Windows 11 só o estilo de fundo funciona, e até a build
  22621.1265. Repositório arquivado em 06/2025. Não serve de modelo. Fonte: github.com/ChrisAnd1998/TaskbarX.
- **Deskband11Lib** (github.com/airtaxi/Deskband11Lib): janela popup vira filha de `Shell_TrayWnd`/
  `Shell_SecondaryTrayWnd` com `SetParent`, depois `SetWindowPos` + `SetWindowRgn`; UI Automation para
  achar Iniciar, Widgets e o grupo de apps; lê `TaskbarAl` no registro para saber o alinhamento. Tem
  eventos para barra recriada, mudança de DPI (desanexa e reanexa) e monitor desconectado.
- **ComposeWindowsTaskBar** (github.com/LionZXY/ComposeWindowsTaskBar): diz que "Windows has no API for
  this" e que o `IDeskBand` saiu no Windows 11; janela transparente sem borda como `WS_CHILD` de
  `Shell_TrayWnd`, geometria reconciliada várias vezes por segundo, com modo reserva "Overlay"
  (janela por cima, o jeito 1).
- **Windhawk / ExplorerPatcher**: alteram o próprio Explorer (injeção ou barra antiga). Fora de questão
  para o Prisma; não pesquisados a fundo.

Sem fonte encontrada (vem da experiência com a API, testar): ordem Z ao clicar na barra, auto-hide,
tela cheia e a mensagem `"TaskbarCreated"`.

## Recomendação

Fazer o **jeito 1**: um modo "Preso na barra" na tela Widget, que só aceita o estilo 0 (faixa), ignora
`widget_x/widget_y` e calcula a posição pela barra principal.

Mudança estimada no Prisma (~120–180 linhas, sem tocar no desenho):
- `src/config.rs`: um campo `widget_dock: bool` (salvar/carregar/padrão).
- `src/overlays.rs`: função `dock_pos()` (SHAppBarMessage + centro vertical + escala) usada em
  `Widget::update` no lugar de `widget_x/y`; esconder com auto-hide recolhido e com tela cheia;
  `WM_NCHITTEST` devolve `HTCLIENT` no modo preso (não arrasta).
- `src/app.rs`: registrar `"TaskbarCreated"` e reagir a `WM_DISPLAYCHANGE`/`WM_SETTINGCHANGE`;
  um `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` para reafirmar topmost quando a barra vier à frente.
- `src/pages/widget.rs`: o interruptor e um aviso para desligar o widget de clima do Windows se ele
  estiver ligado (lido em `HKCU\...\Explorer\Advanced\TaskbarDa`).
- Manuais (pt/en) e `docs/MAPA.md` no mesmo commit da implementação.

Por que não `SetParent` primeiro, se as bibliotecas usam: o jeito 1 reaproveita a janela em camadas
como está, não prende nada dentro do `explorer.exe` e é o modo reserva dessas mesmas bibliotecas.
`SetParent` fica como plano B se, na prática, o piscar ao clicar na barra ou o auto-hide incomodarem;
aí o custo sobe para ~250–350 linhas (coordenadas do pai em `Layered::paint`, recriação ao reiniciar o
Explorer, UI Automation para os botões).
