# Mapa do Prisma: quem faz o quê e onde fica

Este arquivo ajuda quem vai mexer no código a achar rápido a função de cada recurso, como o usuário a chama e em que linha ela está. Os números de linha valem para a versão de 2026-11-12.

**Regra:** atualize este mapa no mesmo commit sempre que uma função mudar de lugar, nascer ou sumir.

Convenção: `arquivo:linha nome`. "Atalho" é a combinação global (chaves `hk_*` do config.ini). "Controle" é o botão mapeado (ações de `remap::ACTIONS`). "Launcher" é o Prisma Launcher, pela API `prisma-link`.

---

## 1. Como o código se organiza

| Pasta / arquivo | O que mora ali |
|---|---|
| `src/main.rs` | Ponto de entrada (`main`, linha 28): flags de linha de comando (`--background`, `--uninstall`, `--setup=`, `--updated=`, `--wait=`, `--set-taskmgr=`), nome único (`Local\PrismaApp`), cópia de administrador. |
| `src/app.rs` | Estado central (`App`) e janelas (`hub_proc`, `main_proc`). Recebe atalhos, bandeja, ações dos controles e pedidos do Launcher. |
| `src/config.rs` | `config.ini` (`data_dir`, linha 498). Structs `Config`, `Hotkey`, `Profile`, `FsApp`, `SndApp`, `SndKey`, `MeterGame`, `HdrGame`. Leitura em `load`, gravação em `save`. |
| `src/overlays.rs` | Janelas por cima da tela: Widget, Toast (avisos), Prompt (pergunta), Hud (gravação), Keyboard (teclado do controle), Radial. |
| `src/meter.rs` | Medidor de FPS, CPU e GPU (janela, limite de FPS, registro). |
| `src/startup.rs` | Instalação, desinstalação, tarefa de inicialização, elevação (administrador), setup. |
| `src/optimizer.rs` | Escolha e fechamento de programas da página Otimizar. |
| `src/link.rs` | Ponte com o Prisma Launcher: `caps`, `wait_of`, `on_link`. |
| `src/report.rs` | Relatório de bug: montagem, limpeza de dados pessoais, envio. |
| `src/crash.rs` | Registro de queda do programa. |
| `src/i18n.rs` | Idioma. `tr` traduz textos. |
| `src/selftest.rs` | Testes de fumaça das flags `--test-*` (só build de debug). |
| `src/pages/` | Uma página por arquivo (seção 4). `pages/mod.rs` tem a barra lateral (`NAV`, linha 255) e o desenho geral (`draw`, linha 286). |
| `src/sys/` | Módulos de sistema: controles (`pads`, `remap`, `osk`, `radial`, `nav`, `macros`, `keymap`), Windows (`hdr`, `power`, `powerplan`, `audio`, `services`, `autostart`, `procs`, `disks`, `index`), atualização (`update`), desinstalação (`uninstall`), registro (`reg`), NVIDIA (`drs`, `nvset`, `nvml`) e outros. |
| `src/capture/` | Foto (`shot`), gravação (`engine`, `live`, `live_enc`), codificadores (`nvenc`, `encoder`, `decoder`, `nvdec`), HDR (`hdr`), áudio (`sound`), arquivo (`mux`). `capture/mod.rs` tem `grab`, `save_png`, `copy_to_clipboard`, `choose_folder` etc. |
| `src/stream/` | Compartilhar tela: `host` (quem transmite), `view` (quem assiste), `service` (serviço), `kvm` (controle remoto), `pads` (controle remoto do controle), `net`, `opts`, `clip`, `ear` (áudio), `hud`, `weblink` (convite `prisma://`). `stream/mod.rs` tem `run_args`. |
| `src/ui/` | Desenho próprio: `mod.rs` (`Ui`, botões, cartões, menus, rolagem), `gfx.rs` (Direct2D e DirectWrite), `layered.rs` (janela em camadas), `edit.rs` (caixa de texto), `icons.rs`, `glyph.rs` (ícones dos botões do controle). |
| `crates/prisma-link/src/` | API do Prisma Launcher: `lib.rs` (capacidades e códigos), `proto.rs` (`Request`, `Reply`), `client.rs`, `server.rs`, `pipe.rs`, `frame.rs`. |
| `res/` | Traduções (`*.tsv`) e imagens em bruto (`kofi.bgra` etc.). |
| `docs/guide/` | `GUIA.md` e `GUIDE.md` (guia do usuário, com imagens). |

---

## 2. Fluxos que atravessam arquivos

### 2.1 Atalho global
1. O Windows manda `WM_HOTKEY` para `hub_proc` (`src/app.rs:3469`).
2. `on_hotkey` (`src/app.rs:1138`) escolhe a ação pelo número `HK_*` (`src/app.rs:141-165`).
3. Os números são registrados por `register_hotkeys` (`src/app.rs:1038`), que lê os campos `hk_*` de `Config` (`src/config.rs:191-267`).
4. O atalho de fechar à força tem gancho próprio: `hook_force_key` e `sync_hook` (`src/app.rs:1182`, `1181`).
5. Os atalhos ficam pausados enquanto o teclado do Prisma ou a captura de atalho está aberta (`pause_hotkeys`, `src/app.rs:1109`).

### 2.2 Controle (do controle físico até a ação)
1. Leitura: `src/sys/pads.rs` (`start` linha 235, `list` 240, leitura HID e XInput). Cada leitura vai para o mapeamento em `sys/pads.rs:1244` (HID) e `sys/pads.rs:1375` (XInput), ambos chamando `remap::feed`.
2. `remap::feed` (`src/sys/remap.rs:1971`) primeiro entrega a leitura ao teclado do Prisma (`osk::feed`, `src/sys/osk.rs:278`) e ao radial (`radial::feed`, `src/sys/radial.rs:264`). Com a janela do Prisma em foco, `nav::feed` (`src/sys/nav.rs:171`, chamado em `remap.rs:1977`) faz a navegação e devolve ao mapeamento só a parada de emergência, o botão do radial e o movimento (gyro, acelerômetro e touchpad), que a navegação não usa (`rest`, `nav.rs:195`). Se nenhum deles pegar, o mapeamento (`Engine::step`, `remap.rs:1305`) gera ações.
3. As ações saem como `Act` e vão para `remap::send` (`remap.rs:2400`), que manda teclas e cliques ao Windows. A ligação de macro sai como `Act::Down`/`Act::Up(Out::Macro(n))` e `feed` (`remap.rs:2069`) a passa a `macros::press` e `macros::release`.
4. Eventos para a janela principal entram por `pads::emit` (`pads.rs:339`) e saem por `pads::take_events` (`pads.rs:271`). Quem consome é `pad_events` (`src/app.rs:2983`).
5. `Event::Action { id, pad }` vai para `pad_action` (`src/app.rs:3120`): `RADIAL` abre o radial, `KEYBOARD` abre ou fecha o teclado, `SOUND + n` troca a combinação de som, e o resto é atalho (tabela `HK`, `src/app.rs:3123`) que chama `on_hotkey`.
6. `Event::Keyboard` chama `keyboard_refresh` (`app.rs:3049`). `Event::Radial` chama `self.radial.update` dentro de `pad_events` (`app.rs:2983`).
7. `apply_pads` (`src/app.rs:3190`) manda ao leitor os limites de aviso e o mapeamento ligado (`pads::set_limits` `pads.rs:281`, `remap::configure` `remap.rs:1867`).

### 2.3 Ações do Prisma (`remap::ACTIONS`)
`ACTIONS` tem 54 itens (`src/sys/remap.rs:73-129`). O índice é o número salvo no perfil do controle, e a ordem não pode mudar (só cresce no fim). Quem despacha é `pad_action` (`src/app.rs:3120`), com a tabela `HK` (`src/app.rs:3123`) na mesma ordem.

| Índice | Nome em `ACTIONS` | O que o Prisma faz | Despacho |
|---|---|---|---|
| 0 | Foto da área | `take_shot` (`app.rs:1743`) | `HK_SHOT` via `on_hotkey` |
| 1 | Foto da tela inteira | `take_full_shot` (`app.rs:1781`) | `HK_SHOT_FULL` |
| 2 | Foto da janela em foco | `take_window_shot` (`app.rs:1752`) | `HK_SHOT_WIN` |
| 3 | Gravar Janela e parar | `toggle_record(true)` (`app.rs:1324`) | `HK_REC` |
| 4 | Gravar Tela e parar | `toggle_record(false)` (`app.rs:1324`) | `HK_REC_FULL` |
| 5 | Salvar replay | `save_replay` (`app.rs:1355`) | `HK_REPLAY` |
| 6 | Medidor compacto | `meter_view(VIEW_COMPACT)` (`app.rs:2129`) | `HK_METER_C` |
| 7 | Medidor completo | `meter_view(VIEW_FULL)` | `HK_METER_F` |
| 8 | Esconder o Medidor | `meter_view(VIEW_OFF)` | `HK_METER_O` |
| 9 | Controles no medidor | `meter_controls` (`app.rs:2179`) | `HK_METER_KEYS` |
| 10 | Ligar e desligar o registro do Medidor | `meter_log` (`app.rs:2157`) | `HK_LOG` |
| 11 | Ligar e desligar o limite de FPS | `meter_limit(0)` (`app.rs:2213`) | `HK_LIMIT_T` |
| 12 | Aumentar o limite de FPS | `meter_limit(1)` | `HK_LIMIT_UP` |
| 13 | Diminuir o limite de FPS | `meter_limit(-1)` | `HK_LIMIT_DOWN` |
| 14 | Ligar e desligar o HDR | `hdr::set_hdr` em thread, dentro de `pad_action` (`app.rs:3120`) | sem atalho próprio |
| 15 | Aumentar o volume | `volume(Step(+))` (`app.rs:1795`) | `HK_VOL_UP` |
| 16 | Diminuir o volume | `volume(Step(-))` | `HK_VOL_DOWN` |
| 17 | Ligar e desligar o som | `volume(ToggleMute)` | `HK_MUTE` |
| 18 | Tela cheia sem bordas na janela em foco | `toggle_fullscreen` (`app.rs:1832`) | `HK_FULL` |
| 19 | Mostrar e esconder o widget | `toggle_widget` (`app.rs:2743`) | `HK_WIDGET` |
| 20 | Otimizar | `optimize_prompt(true)` (`app.rs:2788`) | `HK_OPT` |
| 21 | Fechar à força | `force_close` (`app.rs:1204`) | `HK_FORCE` |
| 22 | Reiniciar o Explorer | `explorer_key` (`app.rs:1228`) | `HK_EXPLORER` |
| 23 | Abrir e fechar a busca | `toggle_find` (`app.rs:2371`) | `HK_FIND` |
| 24 | Desconectar o Bluetooth | `pads::disconnect` (`pads.rs:304`) em `pad_action` | sem atalho |
| 25 | Mostrar a bateria | `battery_label` (`pages/controllers.rs:173`) em `pad_action` | sem atalho |
| 26 | Abrir e fechar o teclado do Prisma | `set_keyboard` (`app.rs:3068`); constante `KEYBOARD` (`remap.rs:207`) | sem atalho |
| 27 | Abrir o radial do Prisma | `radial::open_action` (`radial.rs:199`); constante `RADIAL` (`remap.rs:209`) | sem atalho |
| 28 | Abrir o Prisma | `show_main` em `pad_action` (`app.rs:3120`) | sem atalho |
| 29 a 31 | Plano de energia automático, de jogo e de repouso | `link_power` (`link.rs:481`), o mesmo do Launcher `SetPowerPlan`, em `pad_action` (`app.rs:3120`); aviso com o nome da ação ou "O Windows não trocou o plano" | sem atalho |
| 32 | Criar convite do Compartilhar tela | `create_invite` e abre a página Compartilhar tela (`app.rs:3161`), só com a feature `stream` | sem atalho |
| 33 a 53 | Página X (uma por item da barra lateral) | `nav_page(n - PAGES)` (`pages/mod.rs:233`) dá a página e o ícone; `go` + `show_main` (`app.rs:3170`). `PAGES` = 33 (`remap.rs:204`). Ícone de cada ação: `action_icon` (`pages/mod.rs:238`) | sem atalho |
| 100+ | Combinação de som n | `sound_default(n)` (`app.rs:1813`); `SOUND` = 100 (`remap.rs:209`) | `HK_SND` (100+) |

### 2.4 Prisma Launcher (`prisma-link`)
1. Capacidades anunciadas: constantes em `crates/prisma-link/src/lib.rs:30-60` (ex.: `PAGE_METER`, linha 31; `HDR`, linha 43). A lista enviada é `caps_with` em `src/link.rs:144`, chamada por `caps` (`link.rs:139`).
2. Pedidos chegam como `Request` (`crates/prisma-link/src/proto.rs:5`): `OpenPage`, `SaveReplay`, `ShareInvite`, `CaptureDirs`, `GameStarted`, `GameStopped`, `MeterView`, `PowerPlans`, `SetPowerPlan`, `Fullscreen`, `Volume`, `Hdr`, `NvidiaCatalog`, `NvidiaGet`, `NvidiaSet`, `WindowShot`, `RecordWindow`, `Controllers`, `PadProfiles`, `SetPadProfile`, `PadStop`, `PadMap`, `Keyboard`.
3. `wait_of` (`src/link.rs:123`) decide se o Launcher espera resposta (convite, pastas).
4. `on_link` (`src/link.rs:315`) atende cada `Request`:

| Request | Linha em `on_link` | O que faz |
|---|---|---|
| OpenPage | 317 | `go(page)` (`app.rs:1414`) |
| SaveReplay | 334 | `save_replay` (`app.rs:1355`) |
| ShareInvite | 339 | `pages::share::create_invite` (`share.rs:222`) |
| CaptureDirs | 349 | devolve `cap_dir` |
| GameStarted / GameStopped | 350 / 354 | registra e tira jogo do Medidor e do Fullscreen |
| MeterView | 358 | `set_meter_view` (`app.rs:2136`) |
| Controllers | 366 | lista de controles (`pads::list`, `pads.rs:246`) |
| PadProfiles / SetPadProfile | 367 / 368 | perfis de controle (`pages/pad_profiles.rs`) |
| PadStop / PadMap / Keyboard | 377 / 384 / 390 | parar mapeamento, ligar mapeamento, teclado |
| PowerPlans / SetPowerPlan | 395 / 401 | lista e troca de plano (`link_power`) |
| Fullscreen | 402 | tela cheia do programa com `pid` |
| Volume | 412 | volume de um programa (`pid`) |
| Hdr | 419 | `hdr_reply`, em `Outcome::Later` |
| NvidiaCatalog / NvidiaGet / NvidiaSet | 420 / 427 / 448 | driver NVIDIA (`pages/nvidia.rs`, `link_nvidia_set`) |
| WindowShot / RecordWindow | 449 / 456 | foto e gravação da janela em foco |

Tipos de resposta: `Reply` (`proto.rs:197`), `Page` (`proto.rs:179`), `MeterView` (`proto.rs:105`), `VolumeChange` (`proto.rs:113`), `NvidiaKind` (`proto.rs:157`). Códigos de erro: `lib.rs:63-76`. Versão do protocolo: `lib.rs:26`.

### 2.5 config.ini
- Caminho: `data_dir` (`src/config.rs:532`) mais `config.ini` (`config.rs:551`). Instalado, fica em `%APPDATA%\Prisma`; portátil, em `PrismaData` ao lado do .exe.
- Leitura: `Config::load` (`src/config.rs:555`), uma linha `chave=valor` por vez, com `match` nas chaves (a partir de `config.rs:569`).
- Gravação: `Config::save` (`src/config.rs:765`), que usa `write_safe` (`config.rs:483`) para não deixar arquivo pela metade.
- Valores padrão: `impl Default for Config` (`src/config.rs:352`). `hk_fullscreen`, `hk_widget` e `hk_optimize` = Ctrl+Alt+F, W e O (`config.rs:347-349`); `hk_force` = Alt+F4 (`config.rs:359`).
- Atalho em texto: `Hotkey::parse` (`config.rs:79`) e `Hotkey::label` (`config.rs:85`). Nome de tecla: `vk_name` (`config.rs:107`).
- Radial: `parse_radial` (`config.rs:510`) e `parse_radial_prog` (`config.rs:522`).
- Restaurar padrões de uma página: `Config::reset(Defaults)` (`config.rs:946`). As páginas que aceitam isso estão em `enum Defaults` (`config.rs:162`).
- Perfis por programa: `profile_mut` (`config.rs:1043`) e `profile` (`config.rs:1051`).
- Testes: `src/config.rs` tem 2 testes (`#[test]`).

### 2.6 Traduções
- O texto em português no código é a chave. `i18n::tr` (`src/i18n.rs:177`) procura a tradução do idioma escolhido; sem tradução, mostra o próprio texto.
- Idioma: `system_lang` (`i18n.rs:60`), `set_lang` (`i18n.rs:72`), `lang` (`i18n.rs:77`), `is_english` (`i18n.rs:82`), `set_english` (`i18n.rs:87`).
- Arquivos: `res/*.tsv`.
- Textos sem tradução: a variável `PRISMA_I18N_MISS` (`src/i18n.rs:207`) faz o programa gravá-los.

### 2.7 Fotos de teste, flags e variáveis de ambiente
As flags de teste ficam em `src/app.rs` (`start`, linha 366, e as checagens de `std::env::args`). Servem para gerar fotos e rodar um caso sem depender do uso manual.

| Flag / variável | Onde | O que faz |
|---|---|---|
| `--test-shot=<arquivo>` | `app.rs:2509` em `test_shot_tick` (`app.rs:2506`); desenho em `render_test` (`app.rs:2601`) | Salva uma foto da página atual. |
| `--test-delay=<s>` | `app.rs:2510` | Espera antes da foto (padrão 5). |
| `--test-page=<página>` | `app.rs:2538` | Abre a página antes da foto. |
| `--test-scroll=<página>:<pos>` | `app.rs:2636` | Rola a página antes da foto. |
| `--test-click=x,y` | `app.rs:2639` | Clica na posição antes da foto. |
| `--test-popup` | `app.rs:2511` | Foto com a janela de pop-up. |
| `--test-opts` e `--test-query=` | `app.rs:2512` e `2516` | Foto das opções de pesquisa. |
| `--test-en` e `--test-lang=n` | `app.rs:2526` e `2529` | Foto em inglês ou no idioma n. |
| `--test-warm=<s>` | `app.rs:2607` | Tempo de aquecimento (padrão 3). |
| `--test-grab=`, `--test-rec=`, `--test-stream=`, `--test-uninstall=`, `--test-misc=`, `--test-tweaks=`, `--test-tools=`, `--test-audit=`, `--test-explorer=`, `--test-report=`, `--test-patterns=`, `--test-restore=`, `--test-profile=`, `--test-hdr=`, `--test-autohdr=`, `--test-plan=`, `--test-keep=`, `--test-frames=`, `--test-inject=`, `--test-drs=`, `--test-catalog=`, `--test-index=`, `--test-update=`, `--test-sign=`, `--test-scan=`, `--test-force=`, `--test-route=`, `--test-proc-audio=`, `--test-svc=`, `--test-task=`, `--test-overlay=` | `app.rs:555` a `864`, uma por linha (ex.: `--test-hdr=` em 672, `--test-stream=` em 603, `--test-update=` em 743) | Caso de teste de cada módulo. |
| `--test-crash`, `--test-panic`, `--test-gpu`, `--test-audio`, `--twlog`, `--profiles`, `--shot-clean`, `--update` | `app.rs:631`, `646`, `827`, `841`, `790`, `794`, `872`, `455` | Casos sem valor. |
| `--test-force-key=` | `app.rs:1088` e `1208` | Permite a combinação de fechar à força no teste. |
| `PRISMA_OVERFLOW=<arquivo>` | `src/ui/gfx.rs:212` `overflow_path` (lê a variável, `:214`); `note_overflow` `:241`; caixas em `BOXES` `:223`, `note_box` `:227`, registradas por `clear` `:385`, `round` `:399`, `round_line` `:406`, `push_clip` `:600`, `pop_clip` `:610`, `translate` `:616`, `scale_at` `:627`; `clear` chamado por `src/ui/layered.rs:121` | Só build de depuração. Grava o texto que não cabe no próprio espaço (em uma linha marca "reticências") e mede cada texto contra a menor caixa desenhada em volta; quando passa, grava "borda <lado> Npx". |
| `PRISMA_TEST_PAD=1` | `src/ui/mod.rs:297`, dentro de `Ui::begin` | Foto com o anel e a faixa de dicas do controle. |
| `PRISMA_LAYER_DUMP=<pasta>` | `src/ui/layered.rs:129`, dentro de `paint` (`layered.rs:108`); definido em `src/stream/hud.rs:575` | Grava as camadas de desenho da janela. |
| `PRISMA_METER_GAME`, `PRISMA_METER_PID`, `PRISMA_TEST_METER` | `app.rs:2283`, `2289`, `2295` | Jogo e processo do Medidor para teste. |
| `PRISMA_TEST_REC` | `src/meter.rs:1338` | Gravação do Medidor no teste. |
| `PRISMA_AUTO_SECS` | `src/meter.rs:1052` | Tempo da medição automática. |
| `PRISMA_SETUP_OUT` e `PRISMA_URL_OUT` | `app.rs:1628` e `src/report.rs:294` | Arquivo de saída da instalação e do link do relatório. |
| `PRISMA_CODEC`, `PRISMA_HDR10` | `app.rs:574` e `588` | Codec e HDR10 na gravação de teste (`--test-rec=`). |
| `PRISMA_FORCE_HDR`, `PRISMA_POOL`, `PRISMA_MININT`, `PRISMA_DROP`, `PRISMA_DROP_LATE`, `PRISMA_FMT`, `PRISMA_NO_WAKE`, `PRISMA_ENC`, `PRISMA_NVDEC_CPU` | `capture/engine.rs:343`, `capture/live.rs:282`, `292`, `359`, `360`, `666`, `682`, `capture/live_enc.rs:30`, `capture/nvdec.rs:386` | Ajustes de captura para testes de desempenho. |

---

## 3. Janela principal, bandeja, atalhos e ciclo de vida

| Função (o que o usuário vê) | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Abrir o Prisma (janela principal) | Duplo clique no ícone, ou "Abrir o Prisma" na bandeja | `show_main` `src/app.rs:3300`; `WM_SHOW` em `hub_proc` `app.rs:3487` | `--background` não abre a janela (`main.rs`, `background`). |
| Início do programa | Ao abrir o .exe | `start` `src/app.rs:366`; `main` `src/main.rs:28` | Cria a janela oculta (`hub_proc`) e inicia os leitores. |
| Fechar o programa | "Sair" na bandeja | `shutdown` `src/app.rs:939`; `TRAY_EXIT` `app.rs:176` | |
| Menu da bandeja | Clique direito no ícone | `tray_menu` `src/app.rs:3439`; `WM_TRAY` `app.rs:3753` | Itens: Abrir (`TRAY_OPEN` `app.rs:172`), Widget (`TRAY_WIDGET` `173`), Otimizar agora (`TRAY_OPT` `174`), Tirar foto da área (`TRAY_SHOT` `175`), Sair (`TRAY_EXIT` `176`). |
| Clique esquerdo no ícone | Clique esquerdo | `WM_LBUTTONUP` em `app.rs:3755` | |
| Tela de boas-vindas | Primeira abertura | `first_run` `src/app.rs:1003`; `pages/welcome.rs:61` `draw` | |
| Trocar de página | Clique na barra lateral | `go` `src/app.rs:1414`; lista `NAV` `src/pages/mod.rs:273` | Chama `refresh_page` (`app.rs:3212`) para carregar a página. Onde o clique chama `go` não foi lido. |
| Atualizar a página aberta | Automático | `refresh_page` `src/app.rs:3212` | |
| Relógio e atualização das telas | Automático | `on_tick` `src/app.rs:2849` (`WM_TICK` `app.rs:3480`) | |
| Notícias | Automático | `check_news` `src/app.rs:1019` | |
| Registrar atalhos | Ao abrir e ao salvar | `register_hotkeys` `src/app.rs:1038` | Erros de atalho ficam em `hk_errors`. |
| Pedido de outra cópia para abrir a janela | Abrir o .exe de novo | `WM_SHOW` `src/app.rs:3487` | Nome único `Local\PrismaApp` (`main.rs`). |
| Busca de programas e arquivos (barra) | Atalho HK_FIND; ação 23 | `toggle_find` `src/app.rs:2371`, `show_find` `app.rs:2380`, `close_find` `app.rs:2427` | |
| Teclado e navegação | Teclado | `on_key` `src/app.rs:3402`; `nav_key` `app.rs:3797`; `typed_char` `app.rs:3808` | |
| Navegar com o controle | Prisma em foco: direcional, A, B, LB/RB, LT/RT, X, Y e os dois sticks (esquerdo mouse, direito rolagem, como no Desktop) | Controle: `sys/nav.rs:171` `feed`, `:43` `Pad` (`step`), `:142` `dir_of`, `:195` `rest`, `:207` `point_to`, `:220` `click`, `:147` `set_window` (em `show_main` `app.rs:3300` e `WM_DESTROY` `app.rs:4141`). Janela: `pad_nav` `app.rs:3078` (evento `Event::Nav`, `app.rs:3014`); `pad_off` `app.rs:3778`; `nav_step` `pages/mod.rs:267`. Interface: `Target` `src/ui/mod.rs:72`, `pick` `:98`, `pad_move` `:190`, `scroll_by` `:227`, `pad_field` `:244`, `cancel` `:249`, `target` `:316` (chamado em `clickable`, nas linhas da lista aberta e nos botões da pergunta), `tab_step` `:918` (abas em `pages/nvidia.rs:68`, `pages/share.rs:309` e `pages/controllers.rs:402`), `pad_bar` `:1436` | O foco é o próprio cursor: o direcional leva o cursor ao centro do próximo alvo e A manda um clique de verdade. Entradas do Prisma levam a marca `remap::MARK` (`remap.rs:59`); mouse e teclado sem ela apagam o anel e a faixa (`pad_off`). Com a faixa, a página encolhe `PAD_BAR` (`ui/mod.rs:80`). XInput a 8 ms com a janela aberta (`window_open`, `nav.rs:158`). Testes: `nav.rs` 2, `ui/mod.rs` 1. Pausa: `nav::pause` (`sys/nav.rs:154`), chamado a cada desenho em `paint` (`app.rs:3362`) com campo de tecla esperando, macro gravando, medição ou teste dos botões rodando (`pages/controllers.rs:10` `measuring`, que inclui `State.testing` `:35`); pausado, `feed` só deixa passar a parada de emergência. |
| Rolagem e clique do mouse | Mouse | `mouse_xy` `src/app.rs:3787`; `find_mouse` `app.rs:3829`; `picker_mouse` `app.rs:3918` | |
| Pintura da janela | Automático | `paint` `src/app.rs:3362`; `invalidate` `app.rs:3354` | |
| Seletor de programa (.exe) | Botões que pedem um programa | `open_picker` `src/app.rs:1924`; `show_picker` `app.rs:1933`; `paint_picker` `app.rs:1990`; `pick_exe_for` `app.rs:2697`; `close_picker` `app.rs:2691`; página `pages/picker.rs:67` `draw` | |
| Escolher um .exe | Botão | `pick_exe` `src/app.rs:1732` | |
| Registro de ações (log) | Automático | `log` `src/app.rs:1739`; `crate::config::log` `src/config.rs:542` | |
| Foto do relatório | Botão | `pick_report_photo` `src/app.rs:2059`; `copy_report_photo` `app.rs:2072` | |
| Avisos (mensagens rápidas) | Automático | `Toast::info` `src/overlays.rs:252`; `Toast::info_at` `overlays.rs:272`; `Toast::keep` `overlays.rs:257` e `hide` `:264` (aviso fixo); `Toast::volume` `overlays.rs:239` | Ver seção 5. |

### 3.1 Instalação, atualização e desinstalação

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Instalar o Prisma | Botão de instalação em Configurações | `install_now` `src/app.rs:1617`; `run_setup` `app.rs:1624`; `setup_done` `app.rs:1641`; `startup::install` `src/startup.rs:268` | Copia o .exe e cria a pasta `%APPDATA%\Prisma`. |
| Modo de instalação | Chamado pelo instalador (`--setup=`) | `startup::setup` `src/startup.rs:339`; `main.rs` (`--setup=`) | `PRISMA_SETUP_OUT` (`app.rs:1628`). |
| Ver se está instalado | Automático | `is_installed` `src/startup.rs:240` | Decide a pasta de dados (`config.rs:532`). |
| Abrir a pasta instalada | Botão | `open_installed` `src/startup.rs:361` | |
| Desinstalar | Botão de remover em Configurações; `--uninstall` | `startup::uninstall` `src/startup.rs:366`; `main.rs` (`--uninstall`) | Pergunta antes; as configurações ficam guardadas. |
| Iniciar com o Windows | Opção em Configurações | `startup::is_enabled` `src/startup.rs:11`; `set_enabled` `startup.rs:22`; `create_task` `startup.rs:71`; `autostart::set_enabled` `src/sys/autostart.rs:195` | Tarefa do Agendador com `--background`. |
| Abrir como administrador | Opção em Configurações | `sync_admin` `startup.rs:89`; `open_as_admin` `startup.rs:106`; `is_elevated` `startup.rs:116`; `run_elevated` `startup.rs:140`; `relaunch_elevated` `startup.rs:179` | Cria a tarefa só se a opção está ligada. |
| Atualizar | Botão de atualizar em Configurações (`pages/adjust.rs:151` `draw_update`); `--update` | `update_now` `src/app.rs:1572`; `update_tick` `app.rs:1521`; `sys/update.rs:358` `check`; `:159` `download`; `:420` `install`; `:473` `wait_exit`; `:484` `after_update` | Baixa só ferramentas, nunca toca em saves. Teste: `--test-update=` (`app.rs:743`). 2 testes em `sys/update.rs`. |
| Confirmar assinatura | Automático | `sys/update.rs:336` `signed`; `--test-sign=` (`app.rs:748`) | |
| Página Configurações | Barra lateral | `pages/adjust.rs:4` `draw` | Ver 4.21. |

---

## 4. Funções por página (barra lateral)

A barra lateral é `NAV` (`src/pages/mod.rs:273-295`), na ordem abaixo. Cada página tem um `draw` no próprio arquivo. O carregamento ao abrir fica em `refresh_page` (`src/app.rs:3212`).

### 4.1 Monitor (`src/pages/monitor.rs`, `draw` linha 53)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Ver processos e uso de CPU, RAM e GPU | Página Monitor | `pages/monitor.rs:53` `draw`; `sys/monitor.rs:330` `spawn` | Uso de memória: `sys/monitor.rs:78` `ram_pct`, `:85` `vram_pct`. |
| Encerrar um processo | Botão (com pergunta) | `ask_end` `pages/monitor.rs:38` | |
| Reiniciar o Explorer | Botão ou atalho | `ask_explorer` `pages/monitor.rs:45`; `restart_explorer` `src/app.rs:1239` | Atalho `HK_EXPLORER` (`app.rs:159`), ação 22. |
| Fechar à força | Botão ou atalho | `force_close` `src/app.rs:1204`; `force_stubborn` `app.rs:2781`; `sys/procs.rs:266` `force_close` | Atalho `HK_FORCE` (`app.rs:148`), ação 21. |
| Janelas de processo | Automático | `window_pids` `pages/monitor.rs:23`; `sys/procs.rs:102` `window_pid`; `:117` `top_windows`; `:142` `main_window` | |

### 4.2 Medidor (`src/pages/meter.rs`, `draw` linha 464)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Medidor compacto | Atalho `HK_METER_C` (`app.rs:151`); ação 6; Launcher `MeterView` | `meter_view` `src/app.rs:2129`; `set_meter_view` `app.rs:2136`; `meter::Meter::new` `src/meter.rs:458` | `hk_meter_compact` em `config.rs:253`. |
| Medidor completo | Atalho `HK_METER_F` (`app.rs:152`); ação 7 | `set_meter_view` `app.rs:2136` | `hk_meter_full` em `config.rs`. |
| Esconder o Medidor | Atalho `HK_METER_O` (`app.rs:153`); ação 8 | `set_meter_view` `app.rs:2136` | `hk_meter_off` em `config.rs`. |
| Controles do Medidor | Atalho `HK_METER_KEYS` (`app.rs:158`); ação 9 | `meter_controls` `app.rs:2179`; `meter::set_controls` `meter.rs:650` | |
| Registro do Medidor (gravar em arquivo) | Atalho `HK_LOG` (`app.rs:157`); ação 10 | `meter_log` `app.rs:2157`; `show_saved_log` `app.rs:2172`; `meter::toggle_log` `meter.rs:573` | `hk_log` em `config.rs:260`. |
| Limite de FPS | Atalhos `HK_LIMIT_T`/`UP`/`DOWN` (`app.rs:154-156`); ações 11 a 13 | `meter_limit` `app.rs:2213`; `meter::change_limit` `meter.rs:837`; `pages/meter.rs:181` `limiter` | Config `limit_mode`, `limit_fps`. |
| Toque na tela do Medidor (arrastar, clicar) | Mouse | `meter_input` `app.rs:2198`; `meter::input` `meter.rs:679`; `meter::act` `meter.rs:778` | |
| Atualização a cada tick | Automático | `meter_tick` `app.rs:2279`; `meter::tick` `meter.rs:950`; `meter_timer` `app.rs:2226` | |
| Pintura | Automático | `meter_paint` `app.rs:2308`; `meter::paint` `meter.rs:1169` | |
| Jogos do Medidor | Página | `pages/meter.rs:256` `games`; `add_meter_game` `app.rs:2319` | Config `meter_games`. |
| Opções gerais | Página | `pages/meter.rs:57` `general`; `:116` `items`; `:206` `log` | |
| Driver de medição (PawnIO) | Página | `pages/meter.rs:148` `pawnio_installed`; `:158` `pawnio_row` | |
| Limite seguro e anti-trapaça | Automático | `meter.rs:324` `safe_cap`; `:404` `anti_cheat`; `:428` `driver_limit`; `:437` `uses_injection`; `:442` `write_limit`; `:446` `write_preset` | |
| Detectar DLSS | Automático | `meter.rs:186` `has_dlss` | |
| Primeiro limite de FPS | Automático | `meter.rs:1782` `first_limit` | |
| Mostrar e iniciar o Medidor | Automático | `meter::show` `meter.rs:638`; `meter::start` `meter.rs:1036`; `meter::leave` `meter.rs:1133` | |

### 4.3 Discos e redes (`src/pages/storage.rs`, `draw` linha 40)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Ver discos, saúde e velocidade | Página | `pages/storage.rs:40` `draw`; `:56` `draw_disks`; `sys/disks.rs:224` `list`; `:234` `sample`; `:516` `spawn_health` | |
| Ver redes (adaptadores) | Página | `pages/storage.rs:154` `draw_nets`; `sys/nets.rs:35` `list`; `:91` `sample` | |
| Cores e nomes de saúde | Automático | `pages/storage.rs:6` `level_color`; `:15` `level_name`; `:32` `fmt_hours` | |

### 4.4 Espaço em disco (`src/pages/analyze.rs`, `draw` linha 80)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Mapa de pastas e arquivos (treemap) | Página | `pages/analyze.rs:80` `draw`; `:153` `draw_result`; `:26` `squarify` | |
| Abrir pasta no Explorer | Clique | `open_in_explorer` `pages/analyze.rs:65` | |
| Leitura da tabela de arquivos (MFT) | Automático | `sys/mft.rs:672` `start`; `:685` `drives`; `:205` `protected` | `sys/mft.rs` não tem `#[test]`. |

### 4.5 Pesquisar (`src/pages/search.rs`, `draw` linha 862)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Pesquisa de arquivos, programas, web e comandos | Atalho `HK_FIND` (`app.rs:160`); ação 23; campo na página | `open` `pages/search.rs:301`; `toggle_find` `app.rs:2371`; `set_search` `app.rs:2331` | Config `search_on`, `search_apps`, `search_web`, `search_cmds`, `search_term`, `search_drives`, `search_engines`. |
| Índice de arquivos | Automático | `sys/index.rs:580` `set`; `:603` `turn_off`; `:609` `refresh`; `:615` `flush`; `:1165` `key`; `:1383` `items` | 4 testes em `sys/index.rs`. |
| Linhas do resultado | Página | `pages/search.rs:99` `row`; `:95` `rows` | |
| Ações sobre um resultado | Menu do resultado | `act` `pages/search.rs:309`; `act_extra` `:330`; `menu` `:820`; `open_menu` `:832` | |
| Abrir o arquivo ou programa | Enter ou clique | `send` `pages/search.rs:154`; `sys/quick.rs:141` `launch_app` | |
| Procurar programas instalados | Automático | `sys/quick.rs:91` `find_apps`; `:49` `refresh_apps` | 2 testes em `sys/quick.rs`. |
| Pesquisa na web | Resultado de pesquisa na web | `sys/procs.rs:192` `search_online` | |
| Comandos (terminal e PowerShell) | Campo de comando | `sys/cmds.rs:383` `complete`; `:632` `term_ask`; `:664` `term_get`; `:690` `term_stop`; `:695` `term_tick`; `:124` `forget`; `:88` `ensure` | 4 testes em `sys/cmds.rs`. |
| Propriedades de arquivo | Menu | `sys/procs.rs:173` `file_properties`; `sys/fileinfo.rs:94` `check` | |

### 4.6 Inicialização (`src/pages/startup.rs`, `draw` linha 6)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Listar programas que iniciam com o Windows | Página | `sys/autostart.rs:149` `list`; `:117` `exe_of` | |
| Ligar ou desligar um item | Caixa de seleção | `sys/autostart.rs:195` `set_enabled`; `select_boot` `app.rs:1606`; `sort_boot` `app.rs:1586` | |
| Adicionar ou remover um item | Botões | `sys/autostart.rs:210` `add`; `:228` `remove` | |

### 4.7 Serviços (`src/pages/services.rs`, `draw` linha 27)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Listar serviços do Windows | Página | `sys/services.rs:54` `list`; `:104` `details` | |
| Iniciar, parar ou reiniciar | Botões (com pergunta) | `sys/services.rs:145` `control`; `pages/services.rs:6` `run_action`; `:19` `ask_action` | `select_service` `app.rs:1611`. |
| Abrir propriedades nativas | Botão | `sys/services.rs:169` `open_native`; `:339` `open_properties`; `:347` `show_properties` | |
| Fechar janelas de console | Botão | `sys/services.rs:365` `close_consoles` | |

### 4.8 Desinstalar (`src/pages/uninstall.rs`, `draw` linha 7)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Listar programas instalados | Página | `sys/uninstall.rs:127` `list`; `:184` `command`; `pages/uninstall.rs:26` `draw_list` | |
| Revisar o que vai sumir | Botão | `pages/uninstall.rs:140` `draw_review`; `sys/uninstall.rs:447` `scan`; `:769` `scan_preview` | |
| Ponto de restauração | Automático | `sys/uninstall.rs:282` `restore_point`; `:255` `restore_begin`; `:269` `restore_end` | |
| Remover programa | Botão | `sys/uninstall.rs:639` `remove`; `:625` `to_recycle`; `:728` `start` | Vai para a Lixeira quando possível. |
| Acompanhar a desinstalação | Página | `pages/uninstall.rs:118` `draw_running` | |
| Pastas que sobram | Automático | `sys/uninstall.rs:684` `lines` | |

### 4.9 Ajustes do Windows (`src/pages/tweaks.rs`, `draw` linha 17)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Aplicar um ajuste | Botão | `sys/tweaks.rs:442` `apply`; `:469` `apply_acts`; `:266` `may_remove` | Grava diário em `sys/tweaks.rs:305` `load_journal`. |
| Desfazer | Botão | `sys/tweaks.rs:529` `undo`; `app.rs:1480` `tw_undo` | |
| Ver o que está aplicado | Página | `sys/tweaks.rs:430` `status`; `:394` `installed_apps`; `pages/tweaks.rs:81` `draw_profiles` | |
| Lista de ajustes que sobraram | Página | `pages/tweaks.rs:235` `draw_left_out` | |
| Auditoria do sistema | Botão | `app.rs:1463` `start_audit`; `sys/audit.rs:71` `build`; `:81` `run`; `:36` `hvci_running` | `sys/audit.rs` não tem `#[test]`. |
| Desenho da auditoria | Página | `pages/tweaks.rs:35` `draw_audit` | |
| Teste | `--test-tweaks=` | `app.rs:619`; `selftest.rs:110` `tweaks` | |

### 4.10 Ferramentas (`src/pages/tools.rs`, `draw` linha 345)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Rodar uma ferramenta (SFC, DISM, limpeza etc.) | Botão | `sys/tools.rs:162` `start`; `:51` `cmd`; `:58` `native`; `:204` `simple` | Pede administrador: `pages/tools.rs:95` `elevate_needed`. |
| Ver o resultado | Página | `app.rs:1564` `tool_log`; `app.rs:1568` `tool_busy`; `sys/tools.rs:230` `sfc_verdict`; `:254` `read_cbs` | |
| Escolher pasta da ferramenta | Botão | `pick_tool_folder` `app.rs:1499` | |
| Plano de execução | Automático | `pages/tools.rs:99` `plan` | |
| Teste | `--test-tools=` | `app.rs:623`; `selftest.rs:189` `toolrun` | |

### 4.11 Otimizar (`src/pages/optimize.rs`, `draw` linha 33)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Lista de programas que usam recurso | Página | `pages/optimize.rs:4` `list_card`; `app.rs:2755` `refresh_candidates`; `optimizer.rs:62` `candidates` | |
| Fechar os escolhidos | Botão; atalho `HK_OPT` (`app.rs:143`); ação 20; bandeja | `optimize_prompt` `app.rs:2788`; `optimizer::close` `src/optimizer.rs:109` | Pergunta antes, exceto no atalho manual. |
| Fechar o item selecionado | Botão | `close_selected` `app.rs:2767` | |
| Não fechar processos do sistema | Automático | `optimizer.rs:32` `is_system` | |
| Forçar fechamento | Botão | `force_stubborn` `app.rs:2781`; `optimizer::force` `optimizer.rs:122` | |
| Lembrar o uso do dia | Automático | `optimizer.rs:163` `load`; `:183` `save`; `:196` `note`; `:215` `used_today`; `:220` `suggest` | |
| Pergunta com o mouse | Janela | `prompt_mouse` `app.rs:2810`; `prompt_hide` `app.rs:2826` | |

### 4.12 Energia (`src/pages/energy.rs`, `draw` linha 25)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Ver e trocar plano de energia | Página; Launcher `PowerPlans` e `SetPowerPlan` | `sys/power.rs:54` `list`; `:80` `active`; `:92` `set_active`; `pages/energy.rs:6` `plan_select`; `:16` `load_row` | |
| Criar os planos do Prisma | Automático | `sys/power.rs:162` `ensure_prisma_plans` | 5 testes em `sys/power.rs`. |
| Trocar o plano ao abrir um jogo | Automático | `apply_power_choice` `app.rs:2831`; `apply_plan` `app.rs:2843`; `sys/power.rs:259` `hint`; `:264` `unhint`; `:292` `tick` | Config `power_auto`, `plan_game`, `plan_rest`, `plan_fixed`. |
| Ler, gravar, exportar, importar e fazer backup de planos | Botões | `sys/powerplan.rs:108` `read`; `:131` `load`; `:143` `write`; `:174` `export`; `:183` `import`; `:192` `backup`; `:160` `has_battery` | |
| Editar um plano | Página Edição | `open_plan` `app.rs:2027`; `close_plan` `app.rs:2035`; `set_plan_value` `app.rs:2041`; `pages/plan_edit.rs:58` `draw` | |
| Importar e exportar plano (arquivo) | Botões | `import_plan` `app.rs:2116`; `export_plan` `app.rs:2122` | |
| Atualizar ao abrir a página | Automático | `refresh_page` `app.rs:3212` | |

### 4.13 Fullscreenizer (`src/pages/fullscreen.rs`, `draw` linha 5)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Tela cheia sem bordas na janela em foco | Atalho `HK_FULL` (`app.rs:141`); ação 18; Launcher `Fullscreen` | `toggle_fullscreen` `src/app.rs:1832`; `sys/fullscreen.rs:39` `toggle`; `:58` `apply`; `:88` `restore`; `:109` `sweep`; `:115` `covers_monitor` | `hk_fullscreen` em `config.rs:191`. |
| Lista de programas com tela cheia automática | Página | `pages/fullscreen.rs:5` `draw`; `app.rs:1844` `note_fullscreen`; `app.rs:1850` `toggle_window`; `app.rs:3283` `refresh_windows` | Config `fs_apps`, `fs_remember`. |
| Escolher uma janela | Botão | `WM_PICK_FS` `app.rs:3707` | |

### 4.14 Som (`src/pages/volume.rs`, `draw` linha 199)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Volume geral e por programa | Página; atalhos `HK_VOL_UP`, `HK_VOL_DOWN`, `HK_MUTE` (`app.rs:163-163`); ações 15 a 17 | `volume` `src/app.rs:1795`; `sys/audio.rs:24` `set_volume`; `:29` `set_mute`; `:37` `sessions`; `:106` `change_focused` | Config `vol_step`, `vol_osd`, `vol_mods`. |
| Trocar a saída de som | Lista | `sys/audio.rs:227` `default_output`; `:144` `devices`; `:302` `set_default` | |
| Mandar um programa para outra saída | Lista | `sys/audio.rs:262` `set_route`; `:279` `route_of`; `app.rs:1910` `route_app`; `app.rs:84` `apply_route` | |
| Combinações de som (ação 100 ou mais) | Atalho `HK_SND` (`app.rs:165`) ou controle | `sound_default` `app.rs:1813`; `sound_tick` `app.rs:1857`; `sound_volumes` `app.rs:1878` | Config `snd_apps`, `snd_keys`. |
| Escolher programa para a combinação | Botão | `WM_PICK_SND` `app.rs:3570` | |
| Volume de um processo | Automático | `sys/audio.rs:85` `for_process` | |
| Sincronia de áudio e vídeo | Página | `sys/avsync.rs:315` `run` | |

### 4.15 HDR (`src/pages/hdr.rs`, `draw` linha 381)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Ligar e desligar o HDR | Ação 14; Launcher `Hdr` | `hdr::set_hdr` `src/sys/hdr.rs:92`; `hdr_output` `sys/hdr.rs:76`; `pad_action` `app.rs:3120`; `hdr_reply` (Launcher) `app.rs:419` | Roda em thread, porque o Windows leva segundos. Sem atalho próprio. |
| Brilho e calibração | Página | `sys/hdrcal.rs` (23 funções, ver o arquivo); `pages/hdr.rs:12` `nits` | Não mapeado função a função. |
| Perfis de HDR por jogo | Página | `sys/hdr.rs:301` `parse_profile`; `:332` `default_profile`; `:353` `windows_light`; `:531` `read`; `:541` `load`; `app.rs:2326` `load_hdr` | Config `hdr_profiles`, `hdr_games`. |
| Cartões de tela, perfil, luz e som | Página | `pages/hdr.rs:29` `screen_card`; `:53` `profile_card`; `:93` `light_card`; `:111` `dv_card`; `:139` `sound_card`; `:296` `saved_card`; `:329` `games_card` | |
| Alertas de HDR | Página | `pages/hdr.rs:182` `profile_alerts`; `:191` `alerts` | |
| Revisão de jogo | Página | `pages/hdr.rs:252` `review_card` | |
| Ligar HDR ao abrir um jogo | Automático | `sys/autohdr.rs:13` `apply`; `:25` `clear_all`; `:32` `global_on` | Teste: `--test-autohdr=` (`app.rs:676`). |
| Restaurar HDR se a sessão de compartilhamento caiu | Automático | `stream/host.rs` `restore_hdr` (chamada em `main.rs`) | |
| Formatos e cores | Automático | `sys/hdr.rs:170` `parse`; `:236` `to_xyz`; `:249` `saturation_matrix` | `sys/hdr.rs` não tem `#[test]`. |

### 4.16 Driver NVIDIA (`src/pages/nvidia.rs`, `draw` linha 156)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Página só aparece com placa NVIDIA | Automático | `nvidia::available` `pages/nvidia.rs:28`; filtro `pages/mod.rs:338` | |
| Ajustes por programa | Página | `sys/drs.rs:146` `driver_version`; `:253` `snapshot`; `:305` `write`; `:341` `restore`; `:346` `apply`; `:351` `read`; `:359` `forget`; `pages/nvidia.rs:164` `draw_with` | |
| Lista de programas | Página | `pages/nvidia.rs:33` `games`; `:43` `add_game`; `:55` `stale` | Config `nv_games`. |
| Valores do driver | Página | `sys/nvset.rs:150` `ids`; `:155` `value_name`; `pages/nvidia.rs:100` `row` | |
| Backups | Automático | `sys/drs.rs:270` `backup_dir`; `:275` `backups` | |
| Teste | `--test-drs=` | `app.rs:714`; `sys/drs.rs:373` `test`; `:382` `test_page`; `:409` `test_clear` | |
| Medir a GPU | Automático | `sys/nvml.rs` (o grep não listou as funções: arquivo marcado como binário) | Não mapeado. |
| Driver pelo Launcher | `NvidiaCatalog`, `NvidiaGet`, `NvidiaSet` | `src/link.rs:420`, `:427`, `:448` | |

### 4.17 Captura (`src/pages/capture.rs`, `draw` linha 9)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Foto da área | Atalho `HK_SHOT` (`app.rs:144`); ação 0; bandeja | `take_shot` `src/app.rs:1743`; `capture::grab` `src/capture/mod.rs:293`; `capture::shot::open` `src/capture/shot.rs:728` | `hk_shot` em `config.rs:211`. |
| Foto da tela inteira | Atalho `HK_SHOT_FULL` (`app.rs:145`); ação 1 | `take_full_shot` `src/app.rs:1781` | |
| Foto da janela em foco | Atalho `HK_SHOT_WIN` (`app.rs:149`); ação 2; Launcher `WindowShot` | `take_window_shot` `app.rs:1752`; `save_window_shot` `app.rs:1769` | |
| Editor de foto | Depois da foto | `capture/shot.rs:728` `open`; `:614` `close`; `:853` `pin`; `:879` `debug_setup` | Teste: `--test-grab=` (`app.rs:555`). |
| Copiar foto ou texto | Botão no editor | `capture/mod.rs:401` `copy_to_clipboard`; `:440` `copy_text`; `:464` `clipboard_text`; `:490` `clipboard_image`; `:532` `load_image` | |
| Salvar foto (PNG e JPEG XR) | Botão | `capture/mod.rs:370` `save_png`; `:375` `save_jxr`; `:364` `new_path` | |
| Gravar janela ou tela | Atalho `HK_REC` (janela, `app.rs:146`) ou `HK_REC_FULL` (tela, `app.rs:150`); ações 3 e 4; Launcher `RecordWindow` | `toggle_record` `src/app.rs:1324`; `toggle_record_of` `app.rs:1329`; `start_engine` `app.rs:1302`; `stop_engine` `app.rs:1309`; motor em `src/capture/engine.rs` | Config `cap_mode`, `cap_codec`, `cap_sys_audio`, `cap_mic`, `cap_cursor`, `cap_hdr`. |
| Replay (últimos segundos) | Atalho `HK_REPLAY` (`app.rs:147`); ação 5; Launcher `SaveReplay` | `save_replay` `src/app.rs:1355`; `replay_changed` `app.rs:2956` | Config `replay_on`, `replay_secs`. |
| Pasta de destino | Botão | `pick_folder` `app.rs:1258`; `capture/mod.rs:548` `choose_folder`; `:564` `choose_file`; `:579` `save_file` | Config `cap_dir`. |
| Indicador de gravação | Automático | `is_recording` `app.rs:1264`; `hud_update` `app.rs:1369`; `hud_stop` `app.rs:1384`; `overlays::Hud` `src/overlays.rs:410` | |
| Codificação de vídeo | Automático | `capture/live_enc.rs:95` `feed`; `capture/nvenc.rs`, `capture/encoder.rs`, `capture/mux.rs` | Funções desses arquivos não mapeadas uma a uma. |
| Teste de gravação | `--test-rec=` | `app.rs:571`; `PRISMA_CODEC` (`app.rs:574`); `PRISMA_HDR10` (`app.rs:588`) | |

### 4.18 Compartilhar tela (`src/pages/share.rs`, `draw` linha 436)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Tela principal e abas | Página | `pages/share.rs:436` `draw`; `:592` `main_tab`; `:301` `tabs` | |
| Criar convite (código ou link) | Botão; Launcher `ShareInvite` | `create_invite` `pages/share.rs:222`; `invite_in` `:232`; `invite_of` `:201`; `link.rs:339` | Validade em minutos (`share_invite.minutes`). |
| Quem transmite (host) | Aba Host | `pages/share.rs:1066` `host_tab`; `stream/host.rs` (70 funções, não listadas) | Config `share_names`, `cap_target`. |
| Quem assiste (cliente) | Aba Cliente | `pages/share.rs:1319` `client_tab`; `stream/view.rs`; `stream/mod.rs:633` `sane_video` (recusa vídeo com 0 FPS ou tamanho impossível) | |
| Rede | Aba Rede | `pages/share.rs:1411` `net_tab`; `stream/net.rs` | |
| Teclas e controle remoto | Aba Teclas | `pages/share.rs:1468` `keys_tab`; `:1459` `hotkey_row`; `stream/kvm.rs` | |
| Controles locais repassados | Aba Controles | `pages/share.rs:1515` `local_pads`; `:1541` `pads_tab`; `stream/pads.rs` | |
| Parar transmissão | Botão | `pages/share.rs:242` `stop`; `:250` `stop_kvm`; `:272` `stop_ear` | |
| Trabalho em segundo plano | Automático | `pages/share.rs:277` `service_job`; `stream/service.rs`; `stream/mod.rs:163` `service_dir`; `stream/mod.rs:665` `run_args` | `main.rs` chama `run_args` antes do nome único. |
| Abrir convite recebido (`prisma://`) | Clique no link | `stream/weblink.rs` `take_arg`; `main.rs` (variável `link`); `stream/weblink.rs:26` `link_alone`, chamado no começo de `main` | Link com outros argumentos junto: nada abre. |
| Testes | `--test-stream=` | `app.rs:603`; testes em `src/stream/*.rs` | 34 testes no total em `stream/`. |

### 4.19 Controles (`src/pages/controllers.rs`, `draw` linha 179)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Lista de controles ligados (conexão, bateria, taxa) | Página; Launcher `Controllers` | `pages/controllers.rs:236` `draw`; `:306` `pad_card` (um chip por controle); `sys/pads.rs:246` `list`; `controllers.rs:68` `refresh`; `:121` `tick` | Bateria: `battery_label` `controllers.rs:173`. A página: linha do perfil (`pad_profiles::top`), cartão do controle, abas Mapeamento, Macros e Ajustes (`tabs` `controllers.rs:399`, aba em `State.tab`; não trocam com macro gravando). Ajustes (duas colunas que rolam juntas, `pads-set`): `settings` `controllers.rs:425` (avisos, mapeamento, radial, teclado, conflitos, diagnóstico e medir sticks). Desenho do controle na aba Mapeamento: `draw_art` `pages/controllers.rs:608` (chamado em `:271`), com as imagens `art` `:592` (PNG em `res/pads/ds.png`, `ds4.png` e `xbox.png`, feitos dos vetores do mock-up sem o filtro de granulado, 960x705) e as áreas dos botões `HITS_DS` `:530`, `HITS_DS4` `:551` e `HITS_XBOX` `:571` (do `hits.json` do mock-up). O botão apertado acende; o escolhido no editor ganha contorno; clicar num botão escolhe ele (`pad_profiles::choose` `pad_profiles.rs:131`, `pad_profiles::chosen` `:126`). O genérico não tem desenho e segue com `draw_live` `:938`. Foto de teste: `--test-pad=ds\|ds4\|xbox` (só build de depuração; `sys/pads.rs:260` `fake`). A coluna da esquerda do Mapeamento rola (`State.art_h` `pages/controllers.rs:32`, `begin_scroll` `:268`). Embaixo do desenho, o cartão de teste `draw_test` `pages/controllers.rs:667` (último botão; "Responderam X de N" `:692`; os que faltam `:695`; histórico dos últimos 6 apertos com duração em ms e "aperto duplo" quando o botão é apertado de novo em menos de `DOUBLE_MS` = 60 ms depois de soltar `:704`; botão Limpar `:664`). Estado `Test` `controllers.rs:75` (campo `State.test` `:33`), alimentado a cada quadro por `test_feed` `:90`, chamado em `tick` `:121`; zera quando muda o controle escolhido (`:91`). 1 teste em `controllers.rs` (`test_counts_presses_and_flags_double` `:1252`). O botão **Testar**/**Parar** do cartão liga `app.pads.testing` (`pages/controllers.rs:713`); ligado, `measuring` (`:10`) para a navegação do Prisma e o controle aperta qualquer botão. Depois, o cartão "O que mudou na camada X" `draw_changes` `pages/controllers.rs:781`, com as linhas de `pad_profiles::layer_changes` `pages/pad_profiles.rs:138`. Fotos: `--test-page=controllers-edit`, `controllers-macro`, `controllers-settings`. |
| Mapeamento ligado ou desligado | Cartão do controle, **Como o jogo vê este controle** (Nativo ou Teclado e mouse; Virar controle Xbox desligado até a Fase 8) | `pad_card` `pages/controllers.rs:306` (config `pad_map`); estado em `mapping` `controllers.rs:910` (aba Ajustes); `open_editor` `:39` (o editor abre sozinho no primeiro desenho) | Launcher `PadMap`. |
| Perfis de controle | Linha do perfil no topo; editor nas abas Mapeamento e Macros | `pages/pad_profiles.rs:601` `top` (lista dos perfis, **Novo perfil** que pede o nome e começa como cópia do aberto ou de fábrica, **Mais** com Renomear, Importar, Exportar, Voltar ao de fábrica (com pergunta; perfil seu vira o Base com o nome e os programas dele; campo `Editor.reset` `:24`), Apagar perfil e os modelos; com o programa em mais de um perfil, **Padrão do programa** e **Abre com o padrão/último usado**); campo `Editor.naming` `:22`; `set_main` `:816`; `:834` `draw` (aba 0 ou 1; a aba Mapeamento começa pelo painel do stick ou gatilho escolhido, montado a partir de `focus` `:922` com `ZONES` `:577`, `stick_row` `:448` (modo na camada e zona morta) e `zone_row` `:563`, depois as ligações, "Ligações" `:941`, o turbo `:1210`, os sticks `:1262` e as zonas mortas `:1272`); `pad_profiles.rs:62` `current`; `:256` `set_program`; `:596` `apply` | Launcher `PadProfiles`, `SetPadProfile`. |
| Modelos prontos | Assistente | `sys/remap.rs:213` `TEMPLATES`; `:499` `template`; `:563` `accessible` | |
| Importar e exportar perfil | Botões | `app.rs:2110` `pad_profile_file` | |
| Radial do controle | Botão | `pages/controllers.rs:939` `radial`; `:1161` `set_radial_program`; `sys/radial.rs:41` `set_programs`; `:87` `configure`; `pages/controllers.rs:1120` `slot_select` (ação ou programa de cada lugar e opção) | Config `pad_radial*`. Lugar **Trocar perfil**: valor `PROFILES` (`sys/radial.rs:18`), o direcional escolhe entre os perfis de `remap::program_profiles`, lidos em `open` (`:162`) para `CHOICES` (`:33`, `choices` `:36`); Confirmar chama `remap::switch_to` em `feed` (`:264`); a janela mostra os nomes na faixa das opções. Lugar com 4 opções: valor `GROUP` (`sys/radial.rs:16`), ligado pela caixa `pad-radial-g<i>` da linha do lugar (em `radial`); ligada, `slot_select` desenha a caixa parada e o lugar perde a ação própria. As opções ficam depois dos 16 lugares, em `sub(i, d)` (`:27`), num total de `ALL` = 80 (`:22`). Pôr `GROUP` num lugar do stick esquerdo liga `pad_radial_keep` (em `radial`). |
| Desenho do radial aberto | Botão segurado (ação 27) | `overlays::Radial` `src/overlays.rs:729`; `sys/radial.rs:139` `view`; `:162` `open`; `:215` `step`; `:264` `feed` | Em `step`, o direcional (`DPAD`, `sys/radial.rs:24`) marca a opção (`sub`) de um lugar `GROUP`, e Confirmar só dispara com uma marcada. O desenho põe a cruz de 4 ícones no lugar e a faixa das 4 opções embaixo (`RD_SUB`, `overlays.rs:749`; `height` `:836`). Foto: `--test-page=radial --test-radial=lugar,opção` (`show_for_test`, `sys/radial.rs:204`). |
| Teclado do Prisma | Ação 26; Launcher `Keyboard` | `sys/osk.rs:226` `view`; `:242` `open`; `:270` `close`; `:278` `feed`; `overlays::Keyboard` `overlays.rs:498`; `set_keyboard` `app.rs:3068` | 6 testes em `sys/osk.rs`. Dois sticks (config `pad_osk_two`, `pad_osk_letters_right`): `configure` (`sys/osk.rs:213`, chamado por `apply_pads`); o círculo de cada stick vem de `wheel_char` (`:101`), e o direcional digita no círculo do direito pela ordem `DPAD` (`:60`); com o stick direito solto, o direcional são as setas (`step`). Fugir do cursor (config `pad_osk_dodge`, ligado de fábrica): `configure` guarda e `dodge` (`sys/osk.rs:222`) lê; o resto fica no desenho (linha abaixo, seção 5). Repetir ao segurar (config `pad_osk_repeat`, `pad_osk_repeat_ms`, 500 ms de fábrica): `configure` guarda, `Osk::new` lê, e `step` (`:139`) repete a tecla de qualquer botão segurado; desligado, só as de `REPEATS` (`sys/osk.rs:62`). Com os dois sticks, LT sozinho dá os acentos (`step`). Cartão: `keyboard` (`pages/controllers.rs:1080`). |
| Turbo (repetir o botão) | Controle | `sys/remap.rs:236` `Turbo`; `:252` `new`; `:258` `TURBO_KEY` | 27 testes em `sys/remap.rs`. |
| Macros | Ligação com destino Macro; editor de perfis, seção Macros | Dados: `sys/remap.rs:92` `Out::Macro`, `:100` `Mode`, `:116` `Macro` (passos `Step`, texto `macro`/`mstep` em `to_text` e `parse`). Rodar: `sys/macros.rs:17` `press`, `:44` `release`, `:54` `stop_all`, `:61` `stop_pad`, `:74` `run` (uma thread por macro). Gravar: `:182` `record_start`, `:192` `record_stop`, `:121` `Rec`, `:260` `hooks` (ganchos de teclado e mouse só enquanto grava; ignora o que o Prisma manda e o que acontece com ele em foco, por `nav::front` `sys/nav.rs:163`). Editor: `pages/pad_profiles.rs:1388` (seção Macros), `:433` `remove_macro`, `:423` `step_text`, `:411` `key_out` | 2 testes em `sys/macros.rs`. Para com a parada de emergência, a troca de perfil, o teclado do Prisma e a saída do controle (`stop_all`/`stop_pad` em `remap.rs`). Anti-cheat: não roda, com aviso (`feed`). Aviso fixo da gravação: `Toast::keep` `overlays.rs:257`, `hide` `:264`. Foto: `--test-page=controllers-macro`. |
| Remapear teclado e mouse | Editor de perfis, origem **Teclado e mouse** (`"pp-src"`) | Dados: `sys/remap.rs:285` `Layer.keys` (texto `kbind` em `to_text` e `parse`; vk do Windows, mouse 4 meio, 5 e 6 laterais). Motor: `:1756` `key_profile` (perfil de botões feito das teclas, até `KEY_SLOTS` `:1647`), `:1788` `set_keys` (liga e desliga os ganchos), `:2104` `key_event`, `:2216` `key_tick`, `:2046` `handle` e `:2092` `apply_want` (comuns com `feed`), `:1300` `held_keys` (repetição), `:1642` `FRONT_ADMIN`. Ganchos: `sys/keymap.rs:16` `sync`, `:23` `key_hook`, `:37` `mouse_hook`, `:60` `run` (thread com relógio de 30 ms). Editor: `pages/pad_profiles.rs:280` `KEY_SOURCES`, `:297` `src_name`, `:1022` seletor | 1 teste em `sys/remap.rs` (`keys_become_buttons_for_the_engine`). Passa direto com o Prisma em foco (`nav::front`), em programa de administrador e com o mapeamento parado; cliques esquerdo e direito nunca passam pelo motor. Foto: `--test-page=controllers-keys`. |
| Texto rápido | Editor de perfis, seção Texto rápido; destino Texto rápido numa ligação; abreviação digitada em qualquer programa | Dados: `sys/remap.rs:128` `Snip`, `:143` `abbr_ok`, `:358` `Profile.texts`, `:94` `Out::Text` (texto `text` em `to_text` e `parse`). Ligação: `handle` digita com `osk::type_text` (`sys/osk.rs:318`). Abreviação: `key_event` chama `:1698` `expansion` e `:1566` `typed_key` (palavra em `Run.typed`, zerada na troca de janela em `on_focus`); `set_keys` liga o gancho de teclado quando há abreviação. Editor: `pages/pad_profiles.rs:1506` (seção), `:399` `short`, `:440` `drop_out` (também usado por `remove_macro` `:412`) | 1 teste em `sys/remap.rs` (`quick_text_round_trip_and_abbreviation`). Com anti-cheat, `set_keys` pede só o relógio (`NEED` 4 em `sys/keymap.rs`): sem gancho, vale também para o remapeamento de teclas. Foto: `--test-page=controllers-text`. A rolagem do editor usa a altura do desenho anterior (`Editor.body_h`). |
| Abre sozinho com e chips de camada | Dropdown **Abre sozinho com** na linha das abas; chips de camada acima do desenho | `pages/pad_profiles.rs:182` `opens_with` (opções: nenhum, jogo atual, todos os jogos do Medidor e "Escolher programa", que abre o seletor em `:203`); `:230` `set_game` (liga o programa ao perfil; os outros perfis do programa continuam com ele); `:242` `layers`; `:249` `set_layer`. Seletor: `Target::PadGame` `pages/picker.rs:20`, `:55` chama `set_game`; `WM_PICK_PAD` `app.rs:59`, `:2705` e `:2710` (WPARAM 1 vai para `set_game`), tratado em `:3684`. Desenho: `art_head` `pages/controllers.rs:604`; `tabs` `:399` devolve onde as abas acabam (a lista fica à direita); `draw_art` `:608` desenha os chips. | Foto: `--test-page=controllers-opens` (`app.rs:2545`). |
| Abrir link | Destino de ligação "Abrir link" (botão, ou tecla do teclado e mouse); editor de perfis, campo do link | Dados: `sys/remap.rs:137` `link_ok` (só http ou https, sem espaço, até 500 caracteres); guardado como texto de programa (`Out::Run`, `pages/pad_profiles.rs:1190`). Editor: `pages/pad_profiles.rs:274` `KINDS` (13 itens, o último "Abrir link"), `:27` `Editor.url`, `:1163` campo, `:1165` aviso, `:376` rótulo "Abrir {}". Importar: `pad_profiles.rs:600` pergunta sobre cada link ou programa, também nas ligações de teclado. Foto: `--test-page=controllers-link` (`app.rs:2545`). | 1 teste em `sys/remap.rs` (`link_only_http`). |
| Parar o mapeamento | Combinação de parada (Guia, Voltar e Start) | `remap::set_stopped` `sys/remap.rs:2283`; `STOP_COMBO` `remap.rs:61` | Launcher `PadStop`. |
| Diagnóstico do controle | Página | `pages/controllers.rs:1173` `diagnostic`; `sys/pads.rs:1070` `rate_of`; `:1088` `packet_rate` | |
| Avisos de desconexão, bateria baixa e variação | Automático | `pads::check_warnings` `sys/pads.rs:1271`; `pad_events` `app.rs:2983` | Config `pad_notify`, `pad_low`, `pad_jitter_ms`. |
| Trocar o perfil conforme a janela em foco (vários perfis por programa) | Automático; na linha de perfis, **Padrão do programa** e **Abre com o padrão/último usado** | `remap::on_focus` `sys/remap.rs:2326`; `focus_tick` `remap.rs:2294`; `pick_for` `:1727` (o de agora se o programa não mudou, o último usado, o padrão, o primeiro); `Profile.main` `:347` (texto `main 1`); `REOPEN_LAST` `:1719` com `set_reopen_last` `:1721` (config `pad_reopen_last`, `config.rs:317`, passada em `apply_pads`); último usado em `Run.last`, gravado por `switch` em `pad_last.txt` da pasta de dados (`last_path` `:1707`, `load_last` `:1711`); `program_profiles` `:1718` (perfis para o radial) | Teste `program_with_several_profiles` (`sys/remap.rs:2549`). Fotos: `--test-page=controllers-naming` e `controllers-shared` (`app.rs:2545`). |
| Enviar teclas e cliques | Automático | `remap::send` `sys/remap.rs:2400` | |
| Calibrar stick e gatilho | Automático | `sys/pads.rs:1425` `rest`; `:1439` `outer`; `:1460` `trigger` | |
| Gyro e touchpad: leitura e calibração | Automático (USB sempre; Bluetooth só no modo completo) | `Input` `sys/pads.rs:82` (campos `gyro`, `accel`, `motion`, `touch`); `GyroCal` `:119` `feed` `:119`; `calibrate_gyro` `:146` (pede pela `CAL_ASK` `:144`); `sony` `:995` lê o relatório completo (`full` `:1002`, `at_full` `:1036`); `full_mode` `:434` lê o relatório de recurso 0x05 no Bluetooth (só leitura), chamado em `read_loop` `:1117`; `read_loop` `:1094` calibra por controle (`cal` `:1152`, `CAL_ASK` `:1212`); `Pad.gyro_cal` `:179` | Testes `dualsense_usb_motion_and_touch` `sys/pads.rs:1520`, `gyro_calibrates_when_still` `:1539`. |
| Cartão Gyro e touchpad | Mapeamento, embaixo do teste dos botões (só DualSense e DualShock 4) | `draw_motion` `pages/controllers.rs:720` (chamado em `:278`, depois de `draw_test` `:667`); botão Calibrar chama `pads::calibrate_gyro` `:742` | |
| Gyro por camada (tipo `Motion`) | Linha Gyro de cada camada no editor de perfis (Igual à camada mãe, Desligado ou Mouse) | `sys/remap.rs:302` `Motion` (`Off`; `Mouse(px por grau)`); campo `Layer.gyro`, texto `gyro` em `to_text` e `parse` | `None` é igual à camada mãe; na Base, desligado. |
| Opções do gyro no perfil | Aparecem no editor quando alguma camada usa Mouse | `sys/remap.rs:312` `GyroOpt` (espaço, inverter, suavizar); campo `Profile.gyro_opt`, texto `gyroopt` em `to_text` e `parse` | Testes `player_space_follows_gravity` e `gyro_moves_mouse_only_where_the_layer_turns_it_on` em `sys/remap.rs`. |
| Virar no espaço local ou do jogador | Automático, pelo motor | `sys/remap.rs:323` `gyro_axes` | Espaço do jogador usa a gravidade média do acelerômetro (`Engine.grav`). |
| Gyro da camada ativa | Automático, pelo motor | `sys/remap.rs:1228` `motion` (`touch` falso) | Sobe pelas camadas mãe até achar uma com gyro; sem nenhuma, `Off`. |
| Mouse pelo giro | Automático, em `Engine::step` | `sys/remap.rs:1312` `step`, trecho do gyro em `:1429` (`if inp.motion`) | Suavização pela média curta (`Engine.soft`); inversão em `inv_x` e `inv_y`. |
| Linha Gyro e opções do gyro no editor | Editor de perfis, aba Mapeamento | `pages/pad_profiles.rs:532` `motion_row`; chamado em `:1300` (gyro) e `:1319` (touchpad), dentro de `draw` | "Virar" e as três caixas (`pp-gyro-ix`, `pp-gyro-iy`, `pp-gyro-sm`) só aparecem se alguma camada usa Mouse. A opção **Passa ao jogo** (antes **Desligado**) tem dica na própria linha. |
| Gestos do touchpad (TPU, TPD, TPL, TPR, TPTAP, TPCL, TPCR) | Fileira de botões do editor de perfis (DualSense e DualShock 4), como qualquer botão | constantes `sys/remap.rs:24` `TPU` a `:30` `TPCR`, `:39` `COUNT` (40); `Engine::gesture` `sys/remap.rs:1486`, chamada em `Engine::step` `:1318`; campos `swipe` `:1135` e `half` `:1137`; lista `buttons` `pages/pad_profiles.rs:327` (nomes `:329` a `:335`); ícones `ui/glyph.rs:67` `width` e `:81` `draw` | Com a camada em Mouse ou Stick, o deslizar fica para o mouse ou o stick e só o toque vale como gesto. |
| Regiões e stick virtual do touchpad | Fileira de botões do editor de perfis (Canto do touchpad ↖ ↗ ↙ ↘) e linha Touchpad de cada camada (Stick esquerdo e Stick direito) | constantes `sys/remap.rs:32` `TPUL` a `:35` `TPDR`; `sys/remap.rs:302` `Motion` (`Stick(lado)`); `sys/remap.rs:1221` `touch_stick`; trecho do stick em `Engine::step` `sys/remap.rs:1319` (procura "Stick virtual: vale"); `pages/pad_profiles.rs:532` `motion_row`; ícone `ui/glyph.rs:155` (procura "Região: o quarto") | O dedo vira stick só com a camada em Stick; nesse modo o deslizar não vale como gesto. As regiões valem em qualquer modo do touchpad. |
| Touchpad por camada (campo `Layer.touch`) | Linha Touchpad de cada camada no editor (Igual à camada mãe, Desligado ou Mouse; 200 a 4000 px) | `sys/remap.rs:297` `touch`; trecho do touchpad em `Engine::step` `:1451` (`if let Motion::Mouse(px) = motion(p, layer, true)`); texto `touch` em `to_text` `:692` e `parse` `:824` | Um dedo move o mouse (`:1452` só conta o dedo que já estava no relatório anterior); dois dedos rolam (`:1464`). |
| Flick stick e teste da volta de 360° | Modo "Flick stick" no stick de cada camada (editor de perfis, linha do stick); botão Testar volta aparece com o flick na camada | `sys/remap.rs:279` `Flick(f32)`; `:1401` trecho `Stick::Flick(px) =>` em `Engine::step`; campos `flick` `:1140` e `flick_go` `:1141`; `:2392` `test_turn`; `pages/pad_profiles.rs:464` `stick_row` (opção em `:459`, valor e passo `:465` a `:503`); botão `:1290` (`pp-flick-test`) | Empurrar o stick a 90% liga e a 80% solta; 0,1 s para virar; valor de 500 a 60000 px, 6000 de fábrica (texto `stick <camada> ls\|rs flick <px>`). |
| Curva do mouse, hair trigger e gatilho até o fim | Curva: linha **Curva do mouse** no painel de cada stick; hair trigger: caixa **Hair trigger** no painel de cada gatilho; **L2/R2 até o fim**: botões na fileira de botões do editor | Dados: `sys/remap.rs:361` `Profile.curve`, `:363` `Profile.hair`, `:37` `LTF`, `:38` `RTF`; textos `curve` (`:619`) e `hair` (`:624`) em `to_text`, `:750` e `:754` em `parse`. Motor: `:1144` `Engine.hair`, `:1203` `hair`, `:1332` a `:1340` (gatilhos, `hair` e `full`) em `Engine::step`, `:1396` expoente da curva. Editor: `pages/pad_profiles.rs:949` (curva), `:955` (hair), `:322` (botões até o fim), `:86` `test_target` (`"trigger"` `:105`). Ícone: `ui/glyph.rs:95` (traço em `:102`) | 1 teste em `sys/remap.rs` (`triggers_hair_and_full_pull`). Curva: 1 Reta, 2 Suave (de fábrica), 3 Forte; texto `curve ls\|rs <1 a 3>`. Hair: aperta com 4% do curso; soltar de todo sempre solta; texto `hair lt\|rt`. Até o fim: liga a 0,95 e solta abaixo de 0,90; o gatilho comum continua valendo. Foto: `--test-page=controllers-trigger` (`app.rs:2545`). |
| Barra de luz (Fase 6) | Linha **Barra de luz** no editor de perfis (DualSense e DualShock 4), com a caixa **Também pelo Bluetooth**, o botão **Escolher cor…** e, pela bateria, **Ajustar cores…** | Dados: `sys/remap.rs:365` `Profile.light`, `:367` `light_bt`; `sys/remap.rs:417` `Light`, `:427` `Batt` (cores da carga e o passo), `:441` `battery_color` (arredonda pelo passo e mistura), `:449` `FIRST_COLOR`; texto `light` em `to_text` `:647` e `parse` `:778` (o `light battery` sem cores é o formato antigo); `sys/remap.rs:1943` `light` (vale também em Nativo; só `STOPPED` apaga) e `preview_light` `:1935` (luz do editor aberto, na frente da do perfil em uso; chamada em `pages/pad_profiles.rs:869`, solta em `pages/controllers.rs:259` e em `app.rs:3553` no timer `TIMER_PADS`). Escrita: `sys/pads.rs:509` `LIGHT_DEFAULT`, `:512` `LIT`, `:515` `crc32`, `:529` `out_report`, `:590` `write_report`, `:663` `Want`, `:671` `PadOut`, `:687` `update`, `:754` `release_pads`; campo `Dev.output_len` `:415`. Ao fechar: `app.rs:940` em `shutdown` chama `release_pads`. Editor: linha em `pages/pad_profiles.rs:1342` (`pp-light`, a amostra, `pp-light-pick`, `pp-light-batt` e `pp-light-bt`); as janelas em `light_sheets` `:1624`, com `swatch` `:1607`, `BATT_ROWS` `:1612` e `batt_color` `:1614`; campos `Editor.batt` `:55` e `Editor.pick` `:58`. Seletor de cor: `ui/mod.rs:705` `color_picker` (quadrado e faixa de matiz desenhados por `Canvas::bitmap`), com `hsv_rgb` `ui/gfx.rs:125` e `rgb_hsv` `:142` | Só modelos em `WRITE_OK` `sys/pads.rs:230`. Cor só é mandada quando muda; escrita que falha não repete até reconectar (`failed`). Texto `light off\|battery [vazia meio cheia passo]\|color rrggbb [bt]`. Testes: `light_reports` (`sys/pads.rs:1565`), `battery_colors_slide` (`sys/pads.rs:1625`), `triggers_hair_and_full_pull` (`sys/remap.rs:2483`) e `battery_light_text_keeps_the_old_format` (`sys/remap.rs:2594`, formato antigo e novo). |
| Gatilhos adaptativos, vibração e háptico (Fase 6) | Painel de cada gatilho no editor de perfis: lista **Gatilho adaptativo** (Passa ao jogo, Solto, Resistência, Arma, Vibração), **Começa em**, **Força** e, na Vibração, **Vibra**; ao lado da barra de luz, **Vibrar ao confirmar** com o botão **Testar**; abaixo, **Vibrar com os botões**, **com os gatilhos** e **com os sticks** (0 a 8) | Dados: `sys/remap.rs:369` `Profile.trig`, `:371` `Profile.buzz`, `:401` `Trig`; texto `trig lt\|rt off\|rigid\|click\|buzz <começa> <força> [hz]` (sem linha é `Pass`, passa ao jogo: o bit do gatilho fica desligado; `Off` liga o bit com o modo 0x05; `PadOut::update` manda um `Off` quando o gatilho volta a `Pass`, e `release_pads` só solta os que o Prisma mandava) e `buzz 1\|2` em `to_text` `:605` e `parse` `:722`. Perfil em uso: `sys/remap.rs:1925` `triggers` (a parada de emergência solta) e `preview_trig` `:1908` (gatilhos do editor aberto, postos em `pages/pad_profiles.rs:870` e soltos em `pages/controllers.rs:260` e `app.rs:3554`). Vibração: `sys/pads.rs:448` `buzz`, chamado em `switch` (`sys/remap.rs:2246`) e `stop` (`:2261`), e `sys/pads.rs:457` `buzzing`. Háptico (motor fraco; no DualSense pelo Bluetooth, os atuadores): `sys/remap.rs:373` `Profile.haptic`, `:378` `Haptic`, `:389` `haptic_level` (gatilhos e sticks, com repouso de 15%), `:1915` `haptic` (perfil em uso; a parada desliga); texto `haptic <botões> <gatilhos> <sticks>`; o toque de 40 ms dos botões fica em `PadOut` (`sys/pads.rs:680` `prev` e `tap`, contado em `update` `:687`), editor em `pages/pad_profiles.rs:1399` (`pp-hap0` a `pp-hap2`). Atuadores pelo Bluetooth: `sys/pads.rs:604` `coil_report` (relatório 0x32, 3 kHz, 8 bits com sinal, estéreo, seno de 170 Hz), `:626` `Coil`, `:630` `coil_thread` (linha própria, um relatório a cada 10,67 ms, alguns mudos ao parar), ligado em `update` (`:706`) e guardado em `PadOut.coil` `:683`; escrita recusada volta ao motor. Escrita: `sys/pads.rs:473` `trig_block` (os 11 bytes de um gatilho), `:529` `out_report`. Editor: `pages/pad_profiles.rs:969` (painel do gatilho: `pp-trig{t}`, `pp-trs{t}`, `pp-trp{t}`, `pp-trh{t}`), `:1383` (`pp-buzz` e `pp-buzz-test`) | Gatilho adaptativo só no DualSense; a vibração vale também no DualShock 4. Modos do firmware: 0x05 solto, 0x21 resiste, 0x25 arma, 0x26 vibra, com o curso em dez zonas; os modos simples antigos e os de depuração (0xFC a 0xFE) ficam de fora. `release_pads` solta gatilhos e vibração ao fechar. Testes: `coil_report_layout` (`sys/pads.rs:1585`), `triggers_and_rumble_in_the_report` (`sys/pads.rs:1600`), `haptic_follows_triggers_and_sticks` (`sys/remap.rs:2510`) e `trigger_and_buzz_text` (`sys/remap.rs:2575`). Pesquisa: `research_notes/dualsense-saida-vibracao-gatilhos.md`, `research_notes/dualsense-haptico-bluetooth.md` (formato do 0x32 só no SAxense, MPL-2.0; sem segunda fonte) e `research_notes/dualsense-haptico-windows.md` (cabo: só por áudio em modo exclusivo, não feito). Foto: `--test-page=controllers-trigger`. |
| Desconectar controle Bluetooth | Ação 24 (nos botões, em **Ação do Prisma**, e no radial); botão **Desconectar** no cartão do controle (sempre ligado) | `pads::disconnect` `sys/pads.rs:304` (controle do XInput: o único aparelho Bluetooth da lista `IG`; com mais de um, nada); botão em `pad_card` `pages/controllers.rs:306` | Pelo cabo, nada acontece. |
| Perfil por controle | Lista **Perfil deste controle** no cartão do controle (**Perfil do programa** ou um perfil) | `Run.fixed` `sys/remap.rs:1676`; `prof_of` `:1680` (o próprio do controle ou o do programa; usado em `feed`, `handle`, `light`, `triggers` e `haptic`); `set_fixed` `:1685` (config `pad_fixed`, `config.rs:307`, passada em `apply_pads`); `pad_switch` `:1699` (radial ou ligação: com perfil próprio, troca só o do controle e manda `Event::PadProfile`, `sys/pads.rs:190`, que o app grava em `app.rs:3018`); `switch_pad` `:1707` (chamado pelo radial) | Teste `pad_with_its_own_profile` (`sys/remap.rs:2533`). A chave é o caminho do aparelho (ou `xinput:N`). |

### 4.20 Widget (`src/pages/widget.rs`, `draw` linha 5)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Mostrar ou esconder o widget | Atalho `HK_WIDGET` (`app.rs:142`); ação 19; bandeja | `toggle_widget` `src/app.rs:2743`; `set_widget` `app.rs:2748` | `hk_widget` em `config.rs:192`. |
| Janela do widget | Automático | `overlays::Widget` `src/overlays.rs:38`; `update` `overlays.rs:76`; `set_visible` `overlays.rs:69` | Config `widget_on`, `widget_style`, `widget_x`, `widget_y`. |
| Prévia na página | Página | `overlays.rs:161` `preview` | |
| Monitor onde ele fica | Automático | `overlays.rs:194` `focus_monitor` | |

### 4.21 Configurações (`src/pages/adjust.rs`, `draw` linha 4)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Idioma, inicialização, administrador, Gerenciador de Tarefas | Página | `pages/adjust.rs:4` `draw`; `app.rs:1667` `set_admin`; `:1680` `set_taskmgr`; `:1709` `taskmgr_done`; `:1724` `taskmgr_refused` | Config `lang`, `always_admin`, `tm_hook`, `pad_notify`. |
| Restaurar padrões de uma página | Botão | `reset_defaults` `app.rs:2719`; `Config::reset` `src/config.rs:946`; `defaults_button` `pages/mod.rs:443` | |
| Atualizar | Ver seção 3.1 | `pages/adjust.rs:151` `draw_update` | |
| Cartões de atalho | Página | `src/ui/mod.rs:784` `hotkey` (desenho do cartão de tecla); `register_hotkeys` `app.rs:1038` | |

### 4.22 Guia e Reportar bug (fora da barra, ao lado do nome)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Guia e tutorial | Botão "Guia e tutorial" | `pages/guide.rs:99` `draw`; `guide.rs:68` `build`; `guide.rs:55` `png_size` | Imagens em `docs/guide/*.png`. Lista `HELP` em `pages/mod.rs`. |
| Reportar bug | Botão "Reportar bug" | `pages/report.rs:240` `draw`; `report.rs:33` `note`; `:52` `new`; `:156` `prepare`; `:181` `encode`; `:209` `scrub`; `:241` `machine`; `:273` `recent_log`; `app.rs:2065` `prepare_report`; `:2087` `send_report`; `:2101` `send_report_direct` | Dados pessoais saem em `scrub`. Teste: `--test-report=` (`app.rs:642`); `report.rs:388` `self_test`. |
| Novidades | Página | `pages/news.rs:82` `draw`; `:34` `has_news` | `check_news` `app.rs:1019`. |
| Boas-vindas | Primeira abertura | `pages/welcome.rs:61` `draw`; `:19` `new`; `:46` `finish` | `first_run` `app.rs:1003`. |

---

## 5. Janelas por cima (overlays)

| Janela | Como aparece | Onde no código | Observações |
|---|---|---|---|
| Medidor (compacto e completo) | Atalhos ou controle | `src/meter.rs:1169` `paint`; `src/app.rs:2308` `meter_paint` | Ver 4.2. |
| Widget | Atalho `HK_WIDGET` | `src/overlays.rs:38` `Widget` | Ver 4.20. |
| Aviso (toast) | Automático | `src/overlays.rs:203` `Toast`; `:252` `info`; `:239` `volume`; `:272` `info_at` | Mensagens do controle, do Medidor, do HDR etc. |
| Pergunta com botões | Automático | `src/overlays.rs:284` `Prompt`; `:294` `PromptAnswer`; `:333` `ask`; `:345` `hide`; `:352` `mouse`; `PROMPT_MS` `overlays.rs:301` (10 s) | |
| Registro (gravação) | Durante a gravação | `src/overlays.rs:410` `Hud`; `:445` `update`; `src/stream/hud.rs` | |
| Teclado do Prisma | Ação 26 | `src/overlays.rs:498` `Keyboard`; `:598` `update`; `:654` `tick`; `src/sys/osk.rs` | Com os dois sticks, a janela dobra de largura (`width`, `overlays.rs:675`). Fugir do cursor: `watch_caret` (`overlays.rs:549`) lê o cursor de texto 5 vezes por segundo numa thread à parte com `caret` (`:523`: `GetGUIThreadInfo`, senão `OBJID_CARET` da acessibilidade) e guarda em `CARET` (`:516`); `tick` sobe o teclado para o alto da tela quando `dodge_up` (`:569`) diz que o cursor está atrás dele embaixo, com 35% do caminho por quadro. Testes: `dodge_only_when_behind` e `caret_now` (ignorado; lê a janela em foco). |
| Ícones dos botões do controle | Aba Controles, teclado, radial | `src/ui/glyph.rs`: `Style` (`:10`, Xbox, PlayStation, Nintendo pelo vid 0x057e, números no genérico) com `of` (`:20`) e `current` (`:30`, o primeiro controle); `draw` (`:80`) desenha um botão e devolve a largura (`width` `:67`); `dpad` (`:184`, braço aceso), `face` (`:199`, os quatro em losango), `hints` (`:233`, linha de dicas com ícone e texto, que encolhe para caber); `DPAD_ANY` (`:64`) é o direcional sem braço aceso. Folha de teste: `--test-page=glyphs` (`sheet` `:211`, só no debug) | Sem cor própria: a cor vem de quem desenha. 1 teste. |
| Radial | Botão segurado (ação 27) | `src/overlays.rs:729` `Radial`; `:767` `update`; `:824` `tick`; `src/sys/radial.rs` | |
| Foto (editor) | Depois do atalho de foto | `src/capture/shot.rs:728` `open` | |
| Seletor de programa | Botão | `src/app.rs:1933` `show_picker` | |

---

## 6. Números e testes de apoio

- Testes `#[test]` por área (contagem feita com grep): `sys/remap.rs` 21; `sys/macros.rs` 2; `link.rs` 19; `sys/pads.rs` 14;  `sys/osk.rs` 6; `sys/radial.rs` 6; `sys/power.rs` 5; `sys/index.rs` 4; `sys/cmds.rs` 4; `config.rs` 2; `sys/update.rs` 2; `sys/quick.rs` 2; `sys/stores.rs` 2; `app.rs` 1; `i18n.rs` 1; `sys/nvset.rs` 1; `pages/nvidia.rs` 1; `pages/mod.rs` 1; `ui/glyph.rs` 1; `sys/nav.rs` 2; `ui/mod.rs` 1; `overlays.rs` 2 (1 ignorado); `stream/` 34 no total.
- `src/selftest.rs` (só debug): `run` (linha 5), `misc` (23), `fake_uninstall` (55), `tweaks` (110), `audit` (162), `toolrun` (189), `tweak_log` (223).

---

## 7. O que ficou incerto ou não foi mapeado

- `src/stream/`: não mapeei uma a uma as funções de `host.rs` (70), `net.rs` (63), `kvm.rs` (34), `opts.rs` (27), `pads.rs` (26), `wave.rs` (24), `service.rs` (19), `view.rs` (16), `hud.rs` (15), `code.rs` (14), `link.rs` (14). Só listei as entradas das abas e os pontos de entrada.
- `src/capture/`: encoder, nvenc, nvdec, decoder, live, live_enc, mux, sound e hdr não foram mapeados função a função. Só as de entrada (`capture/mod.rs`, `shot.rs`, `engine` por nome).
- `src/sys/hdrcal.rs` (23 funções), `src/sys/mft.rs` (28), `src/sys/frames.rs` (15), `src/sys/inject.rs` (12), `src/sys/etw.rs`, `src/sys/gamelog.rs`, `src/sys/cputemp.rs` e `src/sys/kmt.rs`: listei só a entrada usada pela página.
- `src/sys/nvml.rs`: o grep o tratou como arquivo binário e não listou as funções.
- `crates/prisma-link/src/client.rs`, `server.rs`, `pipe.rs` e `frame.rs` não foram abertos: as funções deles não estão mapeadas. Só `lib.rs` e `proto.rs` (nomes e linhas de `Request`, `Reply`, `Page`, `MeterView`, `VolumeChange`, `NvidiaKind`, códigos e capacidades).
- Atalhos padrão: só confirmei `hk_fullscreen`, `hk_widget`, `hk_optimize` (Ctrl+Alt+F, W e O) e `hk_force` (Alt+F4) em `config.rs:347-351`. Os demais padrões não foram conferidos.
- Linhas da tabela `HK` em `pad_action` (`src/app.rs:3059-3064`) e as de `on_link` (`src/link.rs:317` a `456`) vieram de grep e leitura parcial. Confira com grep antes de citar.
- Não confirmei o nome da função que contém `src/stream/hud.rs:575` (a variável `PRISMA_LAYER_DUMP` é definida dentro de um bloco com `proc` na linha 549).
- Não li o ponto do clique da barra lateral que chama `go`. Sei que `NAV` fica em `pages/mod.rs:273` e que `go` é `app.rs:1414`.
- As páginas de HDR e Medidor, e as demais, tiveram só as funções `draw` e auxiliares listadas pelo grep. Os botões de cada aba não foram lidos linha a linha.
- Os números de `docs/guide/GUIA.md` não foram usados como fonte de comportamento; as funções saíram do código.
- Não rodei `cargo` nem `cargo fmt`. Não alterei nenhum arquivo do projeto além deste mapa.
