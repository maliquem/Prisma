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
| `src/sys/` | Módulos de sistema: controles (`pads`, `pad_sound`, `audio_haptic`, `dsx`, `bt_audio`, `remap`, `osk`, `radial`, `nav`, `macros`, `keymap`), Windows (`hdr`, `power`, `powerplan`, `audio`, `services`, `autostart`, `procs`, `disks`, `index`), atualização (`update`), desinstalação (`uninstall`), registro (`reg`), NVIDIA (`drs`, `nvset`, `nvml`) e outros. |
| `src/capture/` | Foto (`shot`), gravação (`engine`, `live`, `live_enc`), codificadores (`nvenc`, `encoder`, `decoder`, `nvdec`), HDR (`hdr`), áudio (`sound`), arquivo (`mux`). `capture/mod.rs` tem `grab`, `save_png`, `copy_to_clipboard`, `choose_folder` etc. |
| `src/stream/` | Compartilhar tela: `host` (quem transmite), `view` (quem assiste), `service` (serviço), `kvm` (controle remoto), `pads` (controle remoto do controle), `net`, `opts`, `clip`, `ear` (áudio), `hud`, `weblink` (convite `prisma://`). `stream/mod.rs` tem `run_args`. |
| `src/ui/` | Desenho próprio: `mod.rs` (`Ui`, botões, cartões, menus, rolagem), `gfx.rs` (Direct2D e DirectWrite), `layered.rs` (janela em camadas), `edit.rs` (caixa de texto), `icons.rs`, `glyph.rs` (ícones dos botões do controle). |
| `crates/prisma-link/src/` | API do Prisma Launcher: `lib.rs` (capacidades e códigos), `proto.rs` (`Request`, `Reply`), `client.rs`, `server.rs`, `pipe.rs`, `frame.rs`. |
| `res/` | Traduções (`*.tsv`) e imagens em bruto (`kofi.bgra` etc.). |
| `docs/guide/` | `GUIA.md` e `GUIDE.md` (guia do usuário, com imagens). |

---

## 2. Fluxos que atravessam arquivos

### 2.1 Atalho global
1. O Windows manda `WM_HOTKEY` para `hub_proc` (`src/app.rs:3503`).
2. `on_hotkey` (`src/app.rs:1143`) escolhe a ação pelo número `HK_*` (`src/app.rs:141-165`).
3. Os números são registrados por `register_hotkeys` (`src/app.rs:1043`), que lê os campos `hk_*` de `Config` (`src/config.rs:191-267`).
4. O atalho de fechar à força tem gancho próprio: `hook_force_key` e `sync_hook` (`src/app.rs:1174`, `1187`).
5. Os atalhos ficam pausados enquanto o teclado do Prisma ou a captura de atalho está aberta (`pause_hotkeys`, `src/app.rs:1114`).

### 2.2 Controle (do controle físico até a ação)
1. Leitura: `src/sys/pads.rs` (`start` linha 235, `list` 240, leitura HID e XInput). Cada leitura vai para o mapeamento em `sys/pads.rs:1297` (HID) e `sys/pads.rs:1428` (XInput), ambos chamando `remap::feed`.
2. `remap::feed` (`src/sys/remap.rs:2134`) primeiro entrega a leitura ao teclado do Prisma (`osk::feed`, `src/sys/osk.rs:282`) e ao radial (`radial::feed`, `src/sys/radial.rs:264`). Com a janela do Prisma em foco, `nav::feed` (`src/sys/nav.rs:171`, chamado em `remap.rs:1977`) faz a navegação e devolve ao mapeamento só a parada de emergência, o botão do radial e o movimento (gyro, acelerômetro e touchpad), que a navegação não usa (`rest`, `nav.rs:195`). Se nenhum deles pegar, o mapeamento (`Engine::step`, `remap.rs:1417`) gera ações.
3. As ações saem como `Act` e vão para `remap::send` (`remap.rs:2593`), que manda teclas e cliques ao Windows. A ligação de macro sai como `Act::Down`/`Act::Up(Out::Macro(n))` e `feed` (`remap.rs:2134`) a passa a `macros::press` e `macros::release`.
4. Eventos para a janela principal entram por `pads::emit` (`pads.rs:358`) e saem por `pads::take_events` (`pads.rs:272`). Quem consome é `pad_events` (`src/app.rs:3002`).
5. `Event::Action { id, pad }` vai para `pad_action` (`src/app.rs:3153`): `RADIAL` abre o radial, `KEYBOARD` abre ou fecha o teclado, `SOUND + n` troca a combinação de som, e o resto é atalho (tabela `HK`, `src/app.rs:3156`) que chama `on_hotkey`.
6. `Event::Keyboard` chama `keyboard_refresh` (`app.rs:3082`). `Event::Radial` chama `self.radial.update` dentro de `pad_events` (`app.rs:3002`).
7. `apply_pads` (`src/app.rs:3223`) manda ao leitor os limites de aviso e o mapeamento ligado (`pads::set_limits` `pads.rs:282`, `remap::configure` `remap.rs:1977`).

### 2.3 Ações do Prisma (`remap::ACTIONS`)
`ACTIONS` tem 54 itens (`src/sys/remap.rs:156-212`). O índice é o número salvo no perfil do controle, e a ordem não pode mudar (só cresce no fim). Quem despacha é `pad_action` (`src/app.rs:3153`), com a tabela `HK` (`src/app.rs:3156`) na mesma ordem.

| Índice | Nome em `ACTIONS` | O que o Prisma faz | Despacho |
|---|---|---|---|
| 0 | Foto da área | `take_shot` (`app.rs:1748`) | `HK_SHOT` via `on_hotkey` |
| 1 | Foto da tela inteira | `take_full_shot` (`app.rs:1786`) | `HK_SHOT_FULL` |
| 2 | Foto da janela em foco | `take_window_shot` (`app.rs:1757`) | `HK_SHOT_WIN` |
| 3 | Gravar Janela e parar | `toggle_record(true)` (`app.rs:1329`) | `HK_REC` |
| 4 | Gravar Tela e parar | `toggle_record(false)` (`app.rs:1329`) | `HK_REC_FULL` |
| 5 | Salvar replay | `save_replay` (`app.rs:1360`) | `HK_REPLAY` |
| 6 | Medidor compacto | `meter_view(VIEW_COMPACT)` (`app.rs:2134`) | `HK_METER_C` |
| 7 | Medidor completo | `meter_view(VIEW_FULL)` | `HK_METER_F` |
| 8 | Esconder o Medidor | `meter_view(VIEW_OFF)` | `HK_METER_O` |
| 9 | Controles no medidor | `meter_controls` (`app.rs:2184`) | `HK_METER_KEYS` |
| 10 | Ligar e desligar o registro do Medidor | `meter_log` (`app.rs:2162`) | `HK_LOG` |
| 11 | Ligar e desligar o limite de FPS | `meter_limit(0)` (`app.rs:2218`) | `HK_LIMIT_T` |
| 12 | Aumentar o limite de FPS | `meter_limit(1)` | `HK_LIMIT_UP` |
| 13 | Diminuir o limite de FPS | `meter_limit(-1)` | `HK_LIMIT_DOWN` |
| 14 | Ligar e desligar o HDR | `hdr::set_hdr` em thread, dentro de `pad_action` (`app.rs:3153`) | sem atalho próprio |
| 15 | Aumentar o volume | `volume(Step(+))` (`app.rs:1800`) | `HK_VOL_UP` |
| 16 | Diminuir o volume | `volume(Step(-))` | `HK_VOL_DOWN` |
| 17 | Ligar e desligar o som | `volume(ToggleMute)` | `HK_MUTE` |
| 18 | Tela cheia sem bordas na janela em foco | `toggle_fullscreen` (`app.rs:1837`) | `HK_FULL` |
| 19 | Mostrar e esconder o widget | `toggle_widget` (`app.rs:2748`) | `HK_WIDGET` |
| 20 | Otimizar | `optimize_prompt(true)` (`app.rs:2793`) | `HK_OPT` |
| 21 | Fechar à força | `force_close` (`app.rs:1209`) | `HK_FORCE` |
| 22 | Reiniciar o Explorer | `explorer_key` (`app.rs:1233`) | `HK_EXPLORER` |
| 23 | Abrir e fechar a busca | `toggle_find` (`app.rs:2376`) | `HK_FIND` |
| 24 | Desconectar o Bluetooth | `pads::disconnect` (`pads.rs:305`) em `pad_action` | sem atalho |
| 25 | Mostrar a bateria | `battery_label` (`pages/controllers.rs:173`) em `pad_action` | sem atalho |
| 26 | Abrir e fechar o teclado do Prisma | `set_keyboard` (`app.rs:3101`); constante `KEYBOARD` (`remap.rs:214`) | sem atalho |
| 27 | Abrir o radial do Prisma | `radial::open_action` (`radial.rs:199`); constante `RADIAL` (`remap.rs:216`) | sem atalho |
| 28 | Abrir o Prisma | `show_main` em `pad_action` (`app.rs:3153`) | sem atalho |
| 29 a 31 | Plano de energia automático, de jogo e de repouso | `link_power` (`link.rs:481`), o mesmo do Launcher `SetPowerPlan`, em `pad_action` (`app.rs:3153`); aviso com o nome da ação ou "O Windows não trocou o plano" | sem atalho |
| 32 | Criar convite do Compartilhar tela | `create_invite` e abre a página Compartilhar tela (`app.rs:3198`), só com a feature `stream` | sem atalho |
| 33 a 53 | Página X (uma por item da barra lateral) | `nav_page(n - PAGES)` (`pages/mod.rs:233`) dá a página e o ícone; `go` + `show_main` (`app.rs:3334`). `PAGES` = 33 (`remap.rs:218`). Ícone de cada ação: `action_icon` (`pages/mod.rs:238`) | sem atalho |
| 100+ | Combinação de som n | `sound_default(n)` (`app.rs:1818`); `SOUND` = 100 (`remap.rs:256`) | `HK_SND` (100+) |

### 2.4 Prisma Launcher (`prisma-link`)
1. Capacidades anunciadas: constantes em `crates/prisma-link/src/lib.rs:30-60` (ex.: `PAGE_METER`, linha 31; `HDR`, linha 43). A lista enviada é `caps_with` em `src/link.rs:145`, chamada por `caps` (`link.rs:140`).
2. Pedidos chegam como `Request` (`crates/prisma-link/src/proto.rs:5`): `OpenPage`, `SaveReplay`, `ShareInvite`, `CaptureDirs`, `GameStarted`, `GameStopped`, `MeterView`, `PowerPlans`, `SetPowerPlan`, `Fullscreen`, `Volume`, `Hdr`, `NvidiaCatalog`, `NvidiaGet`, `NvidiaSet`, `WindowShot`, `RecordWindow`, `Controllers`, `PadProfiles`, `SetPadProfile`, `PadStop`, `PadMap`, `Keyboard`.
3. `wait_of` (`src/link.rs:124`) decide se o Launcher espera resposta (convite, pastas).
4. `on_link` (`src/link.rs:316`) atende cada `Request`:

| Request | Linha em `on_link` | O que faz |
|---|---|---|
| OpenPage | 317 | `go(page)` (`app.rs:1419`) |
| SaveReplay | 334 | `save_replay` (`app.rs:1360`) |
| ShareInvite | 339 | `pages::share::create_invite` (`share.rs:221`) |
| CaptureDirs | 349 | devolve `cap_dir` |
| GameStarted / GameStopped | 350 / 354 | registra e tira jogo do Medidor e do Fullscreen |
| MeterView | 358 | `set_meter_view` (`app.rs:2141`) |
| Controllers | 366 | lista de controles (`pads::list`, `pads.rs:247`) |
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
- Caminho: `data_dir` (`src/config.rs:541`) mais `config.ini` (`config.rs:576`). Instalado, fica em `%APPDATA%\Prisma`; portátil, em `PrismaData` ao lado do .exe.
- Leitura: `Config::load` (`src/config.rs:586`), uma linha `chave=valor` por vez, com `match` nas chaves (a partir de `config.rs:599`). O texto vem de `read_text` (`config.rs:561`) e `text_of` (`config.rs:576`): bytes fora do UTF-8 e o BOM não fazem perder o resto; arquivo preso tenta por meio segundo. Se não abrir, `keep_file` fica ligado e `save` não grava por cima nesta sessão.
- Gravação: `Config::save` (`src/config.rs:804`), que usa `write_safe` (`config.rs:492`) para não deixar arquivo pela metade.
- Valores padrão: `impl Default for Config` (`src/config.rs:354`). `hk_fullscreen`, `hk_widget` e `hk_optimize` = Ctrl+Alt+F, W e O (`config.rs:347-349`); `hk_force` = Alt+F4 (`config.rs:362`).
- Atalho em texto: `Hotkey::parse` (`config.rs:79`) e `Hotkey::label` (`config.rs:85`). Nome de tecla: `vk_name` (`config.rs:107`).
- Radial: `parse_radial` (`config.rs:519`) e `parse_radial_prog` (`config.rs:531`).
- Restaurar padrões de uma página: `Config::reset(Defaults)` (`config.rs:990`). As páginas que aceitam isso estão em `enum Defaults` (`config.rs:162`).
- Perfis por programa: `profile_mut` (`config.rs:1087`) e `profile` (`config.rs:1095`).
- Testes: `src/config.rs` tem 2 testes (`#[test]`).

### 2.6 Traduções
- O texto em português no código é a chave. `i18n::tr` (`src/i18n.rs:182`) procura a tradução do idioma escolhido; sem tradução, mostra o próprio texto.
- Idioma: `system_lang` (`i18n.rs:65`), `set_lang` (`i18n.rs:77`), `lang` (`i18n.rs:82`), `is_english` (`i18n.rs:87`), `set_english` (`i18n.rs:92`). `every` (`i18n.rs:60`) dá o texto em todos os idiomas.
- Arquivos: `res/*.tsv`.
- Textos sem tradução: a variável `PRISMA_I18N_MISS` (`src/i18n.rs:212`) faz o programa gravá-los.

### 2.7 Fotos de teste, flags e variáveis de ambiente
As flags de teste ficam em `src/app.rs` (`start`, linha 366, e as checagens de `std::env::args`). Servem para gerar fotos e rodar um caso sem depender do uso manual.

| Flag / variável | Onde | O que faz |
|---|---|---|
| `--test-shot=<arquivo>` | `app.rs:2514` em `test_shot_tick` (`app.rs:2511`); desenho em `render_test` (`app.rs:2606`) | Salva uma foto da página atual. |
| `--test-delay=<s>` | `app.rs:2515` | Espera antes da foto (padrão 5). |
| `--test-page=<página>` | `app.rs:2543` | Abre a página antes da foto. |
| `--test-scroll=<página>:<pos>` | `app.rs:2641` | Rola a página antes da foto. |
| `--test-click=x,y` | `app.rs:2644` | Clica na posição antes da foto. |
| `--test-popup` | `app.rs:2516` | Foto com a janela de pop-up. |
| `--test-opts` e `--test-query=` | `app.rs:2517` e `2522` | Foto das opções de pesquisa. |
| `--test-en` e `--test-lang=n` | `app.rs:2531` e `2539` | Foto em inglês ou no idioma n. |
| `--test-warm=<s>` | `app.rs:2612` | Tempo de aquecimento (padrão 3). |
| `--test-grab=`, `--test-rec=`, `--test-stream=`, `--test-uninstall=`, `--test-misc=`, `--test-tweaks=`, `--test-tools=`, `--test-audit=`, `--test-explorer=`, `--test-report=`, `--test-patterns=`, `--test-restore=`, `--test-profile=`, `--test-hdr=`, `--test-autohdr=`, `--test-plan=`, `--test-keep=`, `--test-frames=`, `--test-inject=`, `--test-drs=`, `--test-catalog=`, `--test-index=`, `--test-update=`, `--test-sign=`, `--test-scan=`, `--test-force=`, `--test-route=`, `--test-proc-audio=`, `--test-svc=`, `--test-task=`, `--test-overlay=` | `app.rs:555` a `865`, uma por linha (ex.: `--test-hdr=` em 673, `--test-stream=` em 604, `--test-update=` em 744) | Caso de teste de cada módulo. |
| `--test-crash`, `--test-panic`, `--test-gpu`, `--test-audio`, `--twlog`, `--profiles`, `--shot-clean`, `--update` | `app.rs:632`, `647`, `828`, `842`, `791`, `795`, `873`, `455` | Casos sem valor. |
| `--test-force-key=` | `app.rs:1093` e `1214` | Permite a combinação de fechar à força no teste. |
| `PRISMA_OVERFLOW=<arquivo>` | `src/ui/gfx.rs:212` `overflow_path` (lê a variável, `:214`); `note_overflow` `:250`; caixas em `BOXES` `:223`, `note_box` `:227`, registradas por `clear` `:385`, `round` `:399`, `round_line` `:406`, `push_clip` `:600`, `pop_clip` `:610`, `translate` `:616`, `scale_at` `:636`; `clear` chamado por `src/ui/layered.rs:121` | Só build de depuração. Grava o texto que não cabe no próprio espaço (em uma linha marca "reticências") e mede cada texto contra a menor caixa desenhada em volta; quando passa, grava "borda <lado> Npx". |
| `PRISMA_TEST_PAD=1` | `src/ui/mod.rs:297`, dentro de `Ui::begin` | Foto com o anel e a faixa de dicas do controle. |
| `PRISMA_LAYER_DUMP=<pasta>` | `src/ui/layered.rs:129`, dentro de `paint` (`layered.rs:108`); definido em `src/stream/hud.rs:575` | Grava as camadas de desenho da janela. |
| `PRISMA_METER_GAME`, `PRISMA_METER_PID`, `PRISMA_TEST_METER` | `app.rs:2288`, `2295`, `2301` | Jogo e processo do Medidor para teste. |
| `PRISMA_TEST_REC` | `src/meter.rs:1338` | Gravação do Medidor no teste. |
| `PRISMA_AUTO_SECS` | `src/meter.rs:1052` | Tempo da medição automática. |
| `PRISMA_SETUP_OUT` e `PRISMA_URL_OUT` | `app.rs:1633` e `src/report.rs:294` | Arquivo de saída da instalação e do link do relatório. |
| `PRISMA_CODEC`, `PRISMA_HDR10` | `app.rs:574` e `589` | Codec e HDR10 na gravação de teste (`--test-rec=`). |
| `PRISMA_FORCE_HDR`, `PRISMA_POOL`, `PRISMA_MININT`, `PRISMA_DROP`, `PRISMA_DROP_LATE`, `PRISMA_FMT`, `PRISMA_NO_WAKE`, `PRISMA_ENC`, `PRISMA_NVDEC_CPU` | `capture/engine.rs:343`, `capture/live.rs:282`, `292`, `359`, `360`, `666`, `682`, `capture/live_enc.rs:30`, `capture/nvdec.rs:386` | Ajustes de captura para testes de desempenho. |

---

## 3. Janela principal, bandeja, atalhos e ciclo de vida

| Função (o que o usuário vê) | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Abrir o Prisma (janela principal) | Duplo clique no ícone, ou "Abrir o Prisma" na bandeja | `show_main` `src/app.rs:3334`; `WM_SHOW` em `hub_proc` `app.rs:3503` | `--background` não abre a janela (`main.rs`, `background`). |
| Início do programa | Ao abrir o .exe | `start` `src/app.rs:366`; `main` `src/main.rs:28` | Cria a janela oculta (`hub_proc`) e inicia os leitores. |
| Fechar o programa | "Sair" na bandeja | `shutdown` `src/app.rs:940`; `TRAY_EXIT` `app.rs:176` | |
| Menu da bandeja | Clique direito no ícone | `tray_menu` `src/app.rs:3473`; `WM_TRAY` `app.rs:3786` | Itens: Abrir (`TRAY_OPEN` `app.rs:172`), Widget (`TRAY_WIDGET` `173`), Otimizar agora (`TRAY_OPT` `174`), Tirar foto da área (`TRAY_SHOT` `175`), Sair (`TRAY_EXIT` `176`). |
| Clique esquerdo no ícone | Clique esquerdo | `WM_LBUTTONUP` em `app.rs:3788` | |
| Tela de boas-vindas | Primeira abertura | `first_run` `src/app.rs:1004`; `pages/welcome.rs:61` `draw` | |
| Trocar de página | Clique na barra lateral | `go` `src/app.rs:1419`; lista `NAV` `src/pages/mod.rs:273` | Chama `refresh_page` (`app.rs:3246`) para carregar a página. Onde o clique chama `go` não foi lido. |
| Atualizar a página aberta | Automático | `refresh_page` `src/app.rs:3246` | |
| Relógio e atualização das telas | Automático | `on_tick` `src/app.rs:2854` (`WM_TICK` `app.rs:3514`) | |
| Notícias | Automático | `check_news` `src/app.rs:1020` | |
| Registrar atalhos | Ao abrir e ao salvar | `register_hotkeys` `src/app.rs:1043` | Erros de atalho ficam em `hk_errors`. |
| Pedido de outra cópia para abrir a janela | Abrir o .exe de novo | `WM_SHOW` `src/app.rs:3521` | Nome único `Local\PrismaApp` (`main.rs`). |
| Busca de programas e arquivos (barra) | Atalho HK_FIND; ação 23 | `toggle_find` `src/app.rs:2376`, `show_find` `app.rs:2385`, `close_find` `app.rs:2432` | |
| Teclado e navegação | Teclado | `on_key` `src/app.rs:3436`; `nav_key` `app.rs:3830`; `typed_char` `app.rs:3841` | |
| Navegar com o controle | Prisma em foco: direcional, A, B, LB/RB, LT/RT, X, Y e os dois sticks (esquerdo mouse, direito rolagem, como no Desktop) | Controle: `sys/nav.rs:171` `feed`, `:43` `Pad` (`step`), `:142` `dir_of`, `:195` `rest`, `:207` `point_to`, `:220` `click`, `:147` `set_window` (em `show_main` `app.rs:3334` e `WM_DESTROY` `app.rs:4173`). Janela: `pad_nav` `app.rs:3111` (evento `Event::Nav`, `app.rs:3049`); `pad_off` `app.rs:3811`; `nav_step` `pages/mod.rs:267`. Interface: `Target` `src/ui/mod.rs:72`, `pick` `:98`, `pad_move` `:190`, `scroll_by` `:227`, `pad_field` `:244`, `cancel` `:249`, `target` `:316` (chamado em `clickable`, nas linhas da lista aberta e nos botões da pergunta), `tab_step` `:921` (abas em `pages/nvidia.rs:68`, `pages/share.rs:308` e `pages/controllers.rs:402`), `pad_bar` `:1026` | O foco é o próprio cursor: o direcional leva o cursor ao centro do próximo alvo e A manda um clique de verdade. Entradas do Prisma levam a marca `remap::MARK` (`remap.rs:66`); mouse e teclado sem ela apagam o anel e a faixa (`pad_off`). Com a faixa, a página encolhe `PAD_BAR` (`ui/mod.rs:80`). XInput a 8 ms com a janela aberta (`window_open`, `nav.rs:158`). Testes: `nav.rs` 2, `ui/mod.rs` 1. Pausa: `nav::pause` (`sys/nav.rs:154`), chamado a cada desenho em `paint` (`app.rs:3396`) com campo de tecla esperando, macro gravando, medição ou teste dos botões rodando (`pages/controllers.rs:10` `measuring`, que inclui `State.testing` `:35`); pausado, `feed` só deixa passar a parada de emergência. |
| Rolagem e clique do mouse | Mouse | `mouse_xy` `src/app.rs:3820`; `find_mouse` `app.rs:3862`; `picker_mouse` `app.rs:3951` | |
| Pintura da janela | Automático | `paint` `src/app.rs:3396`; `invalidate` `app.rs:3388` | |
| Seletor de programa (.exe) | Botões que pedem um programa | `open_picker` `src/app.rs:1929`; `show_picker` `app.rs:1938`; `paint_picker` `app.rs:1995`; `pick_exe_for` `app.rs:2702`; `close_picker` `app.rs:2696`; página `pages/picker.rs:67` `draw` | |
| Escolher um .exe | Botão | `pick_exe` `src/app.rs:1737` | |
| Registro de ações (log) | Automático | `log` `src/app.rs:1744`; `crate::config::log` `src/config.rs:551` | |
| Foto do relatório | Botão | `pick_report_photo` `src/app.rs:2064`; `copy_report_photo` `app.rs:2077` | |
| Avisos (mensagens rápidas) | Automático | `Toast::info` `src/overlays.rs:252`; `Toast::info_at` `overlays.rs:272`; `Toast::keep` `overlays.rs:257` e `hide` `:264` (aviso fixo); `Toast::volume` `overlays.rs:239` | Ver seção 5. |

### 3.1 Instalação, atualização e desinstalação

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Instalar o Prisma | Botão de instalação em Configurações | `install_now` `src/app.rs:1622`; `run_setup` `app.rs:1629`; `setup_done` `app.rs:1646`; `startup::install` `src/startup.rs:268` | Copia o .exe e cria a pasta `%APPDATA%\Prisma`. |
| Modo de instalação | Chamado pelo instalador (`--setup=`) | `startup::setup` `src/startup.rs:339`; `main.rs` (`--setup=`) | `PRISMA_SETUP_OUT` (`app.rs:1633`). |
| Ver se está instalado | Automático | `is_installed` `src/startup.rs:240` | Decide a pasta de dados (`config.rs:535`). |
| Abrir a pasta instalada | Botão | `open_installed` `src/startup.rs:361` | |
| Desinstalar | Botão de remover em Configurações; `--uninstall` | `startup::uninstall` `src/startup.rs:372`; `main.rs` (`--uninstall`); sem administrador numa instalação, `startup::uninstall_as_admin` `src/startup.rs:368` reabre como administrador | Pergunta antes; as configurações ficam guardadas. |
| Abrir programa (radial e ligação dos perfis) | **Abrir um programa…** | `sys/quick.rs:323` `open_as_user`, chamado em `app.rs:3065` (`Event::Run`) | Com o Prisma como administrador, abre pelo Explorer com a permissão da pessoa. |
| Iniciar com o Windows | Opção em Configurações | `startup::is_enabled` `src/startup.rs:11`; `set_enabled` `startup.rs:22`; `create_task` `startup.rs:71`; `autostart::set_enabled` `src/sys/autostart.rs:204` | Tarefa do Agendador com `--background`. |
| Abrir como administrador | Opção em Configurações | `sync_admin` `startup.rs:89`; `open_as_admin` `startup.rs:106`; `is_elevated` `startup.rs:116`; `run_elevated` `startup.rs:140`; `relaunch_elevated` `startup.rs:179` | Cria a tarefa só se a opção está ligada. |
| Atualizar | Botão de atualizar em Configurações (`pages/adjust.rs:151` `draw_update`); `--update` | `update_now` `src/app.rs:1577`; `update_tick` `app.rs:1526`; `sys/update.rs:358` `check`; `:159` `download`; `:420` `install`; `:473` `wait_exit`; `:484` `after_update` | Baixa só ferramentas, nunca toca em saves. Teste: `--test-update=` (`app.rs:744`). 2 testes em `sys/update.rs`. |
| Confirmar assinatura | Automático | `sys/update.rs:336` `signed`; `--test-sign=` (`app.rs:749`) | |
| Página Configurações | Barra lateral | `pages/adjust.rs:4` `draw` | Ver 4.21. |

---

## 4. Funções por página (barra lateral)

A barra lateral é `NAV` (`src/pages/mod.rs:273-295`), na ordem abaixo. Cada página tem um `draw` no próprio arquivo. O carregamento ao abrir fica em `refresh_page` (`src/app.rs:3246`).

### 4.1 Monitor (`src/pages/monitor.rs`, `draw` linha 53)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Ver processos e uso de CPU, RAM e GPU | Página Monitor | `pages/monitor.rs:53` `draw`; `sys/monitor.rs:330` `spawn` | Uso de memória: `sys/monitor.rs:78` `ram_pct`, `:85` `vram_pct`. |
| Encerrar um processo | Botão (com pergunta) | `ask_end` `pages/monitor.rs:38` | |
| Reiniciar o Explorer | Botão ou atalho | `ask_explorer` `pages/monitor.rs:45`; `restart_explorer` `src/app.rs:1244` | Atalho `HK_EXPLORER` (`app.rs:159`), ação 22. |
| Fechar à força | Botão ou atalho | `force_close` `src/app.rs:1209`; `force_stubborn` `app.rs:2786`; `sys/procs.rs:266` `force_close` | Atalho `HK_FORCE` (`app.rs:148`), ação 21. |
| Janelas de processo | Automático | `window_pids` `pages/monitor.rs:23`; `sys/procs.rs:102` `window_pid`; `:117` `top_windows`; `:142` `main_window` | |

### 4.2 Medidor (`src/pages/meter.rs`, `draw` linha 464)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Medidor compacto | Atalho `HK_METER_C` (`app.rs:151`); ação 6; Launcher `MeterView` | `meter_view` `src/app.rs:2134`; `set_meter_view` `app.rs:2141`; `meter::Meter::new` `src/meter.rs:458` | `hk_meter_compact` em `config.rs:255`. |
| Medidor completo | Atalho `HK_METER_F` (`app.rs:152`); ação 7 | `set_meter_view` `app.rs:2141` | `hk_meter_full` em `config.rs`. |
| Esconder o Medidor | Atalho `HK_METER_O` (`app.rs:153`); ação 8 | `set_meter_view` `app.rs:2141` | `hk_meter_off` em `config.rs`. |
| Controles do Medidor | Atalho `HK_METER_KEYS` (`app.rs:158`); ação 9 | `meter_controls` `app.rs:2184`; `meter::set_controls` `meter.rs:650` | |
| Registro do Medidor (gravar em arquivo) | Atalho `HK_LOG` (`app.rs:157`); ação 10 | `meter_log` `app.rs:2162`; `show_saved_log` `app.rs:2177`; `meter::toggle_log` `meter.rs:573` | `hk_log` em `config.rs:262`. |
| Limite de FPS | Atalhos `HK_LIMIT_T`/`UP`/`DOWN` (`app.rs:154-156`); ações 11 a 13 | `meter_limit` `app.rs:2218`; `meter::change_limit` `meter.rs:837`; `pages/meter.rs:181` `limiter` | Config `limit_mode`, `limit_fps`. |
| Toque na tela do Medidor (arrastar, clicar) | Mouse | `meter_input` `app.rs:2203`; `meter::input` `meter.rs:679`; `meter::act` `meter.rs:778` | |
| Atualização a cada tick | Automático | `meter_tick` `app.rs:2284`; `meter::tick` `meter.rs:950`; `meter_timer` `app.rs:2231` | |
| Pintura | Automático | `meter_paint` `app.rs:2313`; `meter::paint` `meter.rs:1169` | |
| Jogos do Medidor | Página | `pages/meter.rs:256` `games`; `add_meter_game` `app.rs:2324` | Config `meter_games`. |
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
| Pesquisa de arquivos, programas, web e comandos | Atalho `HK_FIND` (`app.rs:160`); ação 23; campo na página | `open` `pages/search.rs:301`; `toggle_find` `app.rs:2376`; `set_search` `app.rs:2336` | Config `search_on`, `search_apps`, `search_web`, `search_cmds`, `search_term`, `search_drives`, `search_engines`. |
| Índice de arquivos | Automático | `sys/index.rs:580` `set`; `:603` `turn_off`; `:609` `refresh`; `:615` `flush`; `:1165` `key`; `:1327` `items` | 4 testes em `sys/index.rs`. |
| Linhas do resultado | Página | `pages/search.rs:99` `row`; `:95` `rows` | |
| Ações sobre um resultado | Menu do resultado | `act` `pages/search.rs:309`; `act_extra` `:330`; `menu` `:820`; `open_menu` `:832` | |
| Abrir o arquivo ou programa | Enter ou clique | `send` `pages/search.rs:154`; `sys/quick.rs:141` `launch_app` | |
| Procurar programas instalados | Automático | `sys/quick.rs:91` `find_apps`; `:49` `refresh_apps` | 2 testes em `sys/quick.rs`. |
| Pesquisa na web | Resultado de pesquisa na web | `sys/procs.rs:192` `search_online` | |
| Comandos (terminal e PowerShell) | Campo de comando | `sys/cmds.rs:383` `complete`; `:632` `term_ask`; `:657` `term_get`; `:690` `term_stop`; `:695` `term_tick`; `:124` `forget`; `:88` `ensure` | 4 testes em `sys/cmds.rs`. |
| Propriedades de arquivo | Menu | `sys/procs.rs:173` `file_properties`; `sys/fileinfo.rs:94` `check` | |

### 4.6 Inicialização (`src/pages/startup.rs`, `draw` linha 6)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Listar programas que iniciam com o Windows | Página | `sys/autostart.rs:158` `list`; `:126` `exe_of` | |
| Ligar ou desligar um item | Caixa de seleção | `sys/autostart.rs:204` `set_enabled`; `select_boot` `app.rs:1611`; `sort_boot` `app.rs:1591` | |
| Adicionar ou remover um item | Botões | `sys/autostart.rs:219` `add`; `:237` `remove` | |

### 4.7 Serviços (`src/pages/services.rs`, `draw` linha 27)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Listar serviços do Windows | Página | `sys/services.rs:54` `list`; `:104` `details` | |
| Iniciar, parar ou reiniciar | Botões (com pergunta) | `sys/services.rs:145` `control`; `pages/services.rs:6` `run_action`; `:19` `ask_action` | `select_service` `app.rs:1616`. |
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
| Desfazer | Botão | `sys/tweaks.rs:529` `undo`; `app.rs:1485` `tw_undo` | |
| Ver o que está aplicado | Página | `sys/tweaks.rs:430` `status`; `:394` `installed_apps`; `pages/tweaks.rs:81` `draw_profiles` | |
| Lista de ajustes que sobraram | Página | `pages/tweaks.rs:235` `draw_left_out` | |
| Auditoria do sistema | Botão | `app.rs:1468` `start_audit`; `sys/audit.rs:71` `build`; `:81` `run`; `:36` `hvci_running` | `sys/audit.rs` não tem `#[test]`. |
| Desenho da auditoria | Página | `pages/tweaks.rs:35` `draw_audit` | |
| Teste | `--test-tweaks=` | `app.rs:620`; `selftest.rs:110` `tweaks` | |

### 4.10 Ferramentas (`src/pages/tools.rs`, `draw` linha 345)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Rodar uma ferramenta (SFC, DISM, limpeza etc.) | Botão | `sys/tools.rs:162` `start`; `:51` `cmd`; `:58` `native`; `:204` `simple` | Pede administrador: `pages/tools.rs:95` `elevate_needed`. |
| Ver o resultado | Página | `app.rs:1569` `tool_log`; `app.rs:1573` `tool_busy`; `sys/tools.rs:230` `sfc_verdict`; `:254` `read_cbs` | |
| Escolher pasta da ferramenta | Botão | `pick_tool_folder` `app.rs:1504` | |
| Plano de execução | Automático | `pages/tools.rs:99` `plan` | |
| Teste | `--test-tools=` | `app.rs:624`; `selftest.rs:189` `toolrun` | |

### 4.11 Otimizar (`src/pages/optimize.rs`, `draw` linha 33)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Lista de programas que usam recurso | Página | `pages/optimize.rs:4` `list_card`; `app.rs:2760` `refresh_candidates`; `optimizer.rs:62` `candidates` | |
| Fechar os escolhidos | Botão; atalho `HK_OPT` (`app.rs:143`); ação 20; bandeja | `optimize_prompt` `app.rs:2793`; `optimizer::close` `src/optimizer.rs:109` | Pergunta antes, exceto no atalho manual. |
| Fechar o item selecionado | Botão | `close_selected` `app.rs:2772` | |
| Não fechar processos do sistema | Automático | `optimizer.rs:32` `is_system` | |
| Forçar fechamento | Botão | `force_stubborn` `app.rs:2786`; `optimizer::force` `optimizer.rs:122` | |
| Lembrar o uso do dia | Automático | `optimizer.rs:163` `load`; `:183` `save`; `:196` `note`; `:215` `used_today`; `:220` `suggest` | |
| Pergunta com o mouse | Janela | `prompt_mouse` `app.rs:2815`; `prompt_hide` `app.rs:2831` | |

### 4.12 Energia (`src/pages/energy.rs`, `draw` linha 25)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Ver e trocar plano de energia | Página; Launcher `PowerPlans` e `SetPowerPlan` | `sys/power.rs:55` `list`; `:81` `active`; `:93` `set_active`; `pages/energy.rs:6` `plan_select`; `:16` `load_row` | |
| Criar os planos do Prisma | Automático | `sys/power.rs:192` `ensure_prisma_plans`; nome no idioma escolhido: `:185` `rename_prisma_plans` (chamada em `refresh_plans` `app.rs:1033`, só na versão instalada), `:175` `find_prisma`, `:169` `is_prisma`, `:160` `write_name` | 5 testes em `sys/power.rs`. |
| Trocar o plano ao abrir um jogo | Automático | `apply_power_choice` `app.rs:2836`; `apply_plan` `app.rs:2848`; `sys/power.rs:288` `hint`; `:293` `unhint`; `:321` `tick` | Config `power_auto`, `plan_game`, `plan_rest`, `plan_fixed`. |
| Ler, gravar, exportar, importar e fazer backup de planos | Botões | `sys/powerplan.rs:108` `read`; `:131` `load`; `:143` `write`; `:174` `export`; `:183` `import`; `:192` `backup`; `:160` `has_battery` | |
| Editar um plano | Página Edição | `open_plan` `app.rs:2032`; `close_plan` `app.rs:2040`; `set_plan_value` `app.rs:2046`; `pages/plan_edit.rs:58` `draw` | |
| Importar e exportar plano (arquivo) | Botões | `import_plan` `app.rs:2121`; `export_plan` `app.rs:2127` | |
| Atualizar ao abrir a página | Automático | `refresh_page` `app.rs:3246` | |

### 4.13 Fullscreenizer (`src/pages/fullscreen.rs`, `draw` linha 5)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Tela cheia sem bordas na janela em foco | Atalho `HK_FULL` (`app.rs:141`); ação 18; Launcher `Fullscreen` | `toggle_fullscreen` `src/app.rs:1837`; `sys/fullscreen.rs:39` `toggle`; `:58` `apply`; `:88` `restore`; `:109` `sweep`; `:115` `covers_monitor` | `hk_fullscreen` em `config.rs:193`. |
| Lista de programas com tela cheia automática | Página | `pages/fullscreen.rs:5` `draw`; `app.rs:1849` `note_fullscreen`; `app.rs:1855` `toggle_window`; `app.rs:3317` `refresh_windows` | Config `fs_apps`, `fs_remember`. |
| Escolher uma janela | Botão | `WM_PICK_FS` `app.rs:3740` | |

### 4.14 Som (`src/pages/volume.rs`, `draw` linha 199)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Volume geral e por programa | Página; atalhos `HK_VOL_UP`, `HK_VOL_DOWN`, `HK_MUTE` (`app.rs:163-163`); ações 15 a 17 | `volume` `src/app.rs:1800`; `sys/audio.rs:24` `set_volume`; `:29` `set_mute`; `:37` `sessions`; `:106` `change_focused` | Config `vol_step`, `vol_osd`, `vol_mods`. |
| Trocar a saída de som | Lista | `sys/audio.rs:227` `default_output`; `:144` `devices`; `:302` `set_default` | |
| Mandar um programa para outra saída | Lista | `sys/audio.rs:262` `set_route`; `:279` `route_of`; `app.rs:1915` `route_app`; `app.rs:84` `apply_route` | |
| Combinações de som (ação 100 ou mais) | Atalho `HK_SND` (`app.rs:165`) ou controle | `sound_default` `app.rs:1818`; `sound_tick` `app.rs:1862`; `sound_volumes` `app.rs:1883` | Config `snd_apps`, `snd_keys`. |
| Escolher programa para a combinação | Botão | `WM_PICK_SND` `app.rs:3603` | |
| Volume de um processo | Automático | `sys/audio.rs:85` `for_process` | |
| Sincronia de áudio e vídeo | Página | `sys/avsync.rs:315` `run` | |

### 4.15 HDR (`src/pages/hdr.rs`, `draw` linha 381)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Ligar e desligar o HDR | Ação 14; Launcher `Hdr` | `hdr::set_hdr` `src/sys/hdr.rs:92`; `hdr_output` `sys/hdr.rs:76`; `pad_action` `app.rs:3153`; `hdr_reply` (Launcher) `link.rs:550` | Roda em thread, porque o Windows leva segundos. Sem atalho próprio. |
| Brilho e calibração | Página | `sys/hdrcal.rs` (23 funções, ver o arquivo); `pages/hdr.rs:12` `nits` | Não mapeado função a função. |
| Perfis de HDR por jogo | Página | `sys/hdr.rs:301` `parse_profile`; `:332` `default_profile`; `:353` `windows_light`; `:531` `read`; `:541` `load`; `app.rs:2331` `load_hdr` | Config `hdr_profiles`, `hdr_games`. |
| Cartões de tela, perfil, luz e som | Página | `pages/hdr.rs:29` `screen_card`; `:53` `profile_card`; `:93` `light_card`; `:111` `dv_card`; `:139` `sound_card`; `:296` `saved_card`; `:329` `games_card` | |
| Alertas de HDR | Página | `pages/hdr.rs:182` `profile_alerts`; `:191` `alerts` | |
| Revisão de jogo | Página | `pages/hdr.rs:252` `review_card` | |
| Ligar HDR ao abrir um jogo | Automático | `sys/autohdr.rs:13` `apply`; `:25` `clear_all`; `:32` `global_on` | Teste: `--test-autohdr=` (`app.rs:677`). |
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
| Teste | `--test-drs=` | `app.rs:715`; `sys/drs.rs:373` `test`; `:382` `test_page`; `:409` `test_clear` | |
| Medir a GPU | Automático | `sys/nvml.rs` (o grep não listou as funções: arquivo marcado como binário) | Não mapeado. |
| Driver pelo Launcher | `NvidiaCatalog`, `NvidiaGet`, `NvidiaSet` | `src/link.rs:420`, `:427`, `:448` | |

### 4.17 Captura (`src/pages/capture.rs`, `draw` linha 9)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Foto da área | Atalho `HK_SHOT` (`app.rs:144`); ação 0; bandeja | `take_shot` `src/app.rs:1748`; `capture::grab` `src/capture/mod.rs:303`; `capture::shot::open` `src/capture/shot.rs:728` | `hk_shot` em `config.rs:213`. |
| Foto da tela inteira | Atalho `HK_SHOT_FULL` (`app.rs:145`); ação 1 | `take_full_shot` `src/app.rs:1786` | |
| Foto da janela em foco | Atalho `HK_SHOT_WIN` (`app.rs:149`); ação 2; Launcher `WindowShot` | `take_window_shot` `app.rs:1757`; `save_window_shot` `app.rs:1774` | |
| Editor de foto | Depois da foto | `capture/shot.rs:728` `open`; `:614` `close`; `:853` `pin`; `:879` `debug_setup` | Teste: `--test-grab=` (`app.rs:555`). |
| Copiar foto ou texto | Botão no editor | `capture/mod.rs:411` `copy_to_clipboard`; `:450` `copy_text`; `:474` `clipboard_text`; `:507` `clipboard_image` (confere o tamanho com `dib_fits` `:500`); `:555` `load_image` | Os codificadores e decodificadores de vídeo vêm do `MFTEnumEx` por `mft_list` (`capture/mod.rs:136`), que solta os que não foram escolhidos. |
| Salvar foto (PNG e JPEG XR) | Botão | `capture/mod.rs:380` `save_png`; `:385` `save_jxr`; `:374` `new_path` | |
| Gravar janela ou tela | Atalho `HK_REC` (janela, `app.rs:146`) ou `HK_REC_FULL` (tela, `app.rs:150`); ações 3 e 4; Launcher `RecordWindow` | `toggle_record` `src/app.rs:1329`; `toggle_record_of` `app.rs:1334`; `start_engine` `app.rs:1307`; `stop_engine` `app.rs:1314`; motor em `src/capture/engine.rs` | Config `cap_mode`, `cap_codec`, `cap_sys_audio`, `cap_mic`, `cap_cursor`, `cap_hdr`. |
| Replay (últimos segundos) | Atalho `HK_REPLAY` (`app.rs:147`); ação 5; Launcher `SaveReplay` | `save_replay` `src/app.rs:1360`; `replay_changed` `app.rs:2961` | Config `replay_on`, `replay_secs`. |
| Pasta de destino | Botão | `pick_folder` `app.rs:1263`; `capture/mod.rs:571` `choose_folder`; `:587` `choose_file`; `:602` `save_file` | Config `cap_dir`. |
| Indicador de gravação | Automático | `is_recording` `app.rs:1269`; `hud_update` `app.rs:1374`; `hud_stop` `app.rs:1389`; `overlays::Hud` `src/overlays.rs:410` | |
| Codificação de vídeo | Automático | `capture/live_enc.rs:95` `feed`; `capture/nvenc.rs`, `capture/encoder.rs`, `capture/mux.rs` | Funções desses arquivos não mapeadas uma a uma. |
| Teste de gravação | `--test-rec=` | `app.rs:571`; `PRISMA_CODEC` (`app.rs:574`); `PRISMA_HDR10` (`app.rs:589`) | |

### 4.18 Compartilhar tela (`src/pages/share.rs`, `draw` linha 436)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Tela principal e abas | Página | `pages/share.rs:435` `draw`; `:591` `main_tab`; `:300` `tabs` | |
| Criar convite (código ou link) | Botão; Launcher `ShareInvite` | `create_invite` `pages/share.rs:221`; `invite_in` `:231`; `invite_of` `:200`; `link.rs:339` | Validade em minutos (`share_invite.minutes`). |
| Quem transmite (host) | Aba Host | `pages/share.rs:1065` `host_tab`; `stream/host.rs` (70 funções, não listadas) | Config `share_names`, `cap_target`. |
| Quem assiste (cliente) | Aba Cliente | `pages/share.rs:1318` `client_tab`; `stream/view.rs`; `stream/mod.rs:633` `sane_video` (recusa vídeo com 0 FPS ou tamanho impossível) | |
| Rede | Aba Rede | `pages/share.rs:1410` `net_tab`; `stream/net.rs` | |
| Teclas e controle remoto | Aba Teclas | `pages/share.rs:1467` `keys_tab`; `:1458` `hotkey_row`; `stream/kvm.rs` | |
| Controles locais repassados | Aba Controles | `pages/share.rs:1514` `local_pads`; `:1540` `pads_tab`; `stream/pads.rs` | |
| Parar transmissão | Botão | `pages/share.rs:241` `stop`; `:249` `stop_kvm`; `:271` `stop_ear` | |
| Trabalho em segundo plano | Automático | `pages/share.rs:276` `service_job`; `stream/service.rs`; `stream/mod.rs:163` `service_dir`; `stream/mod.rs:665` `run_args` | `main.rs` chama `run_args` antes do nome único. |
| Abrir convite recebido (`prisma://`) | Clique no link | `stream/weblink.rs` `take_arg`; `main.rs` (variável `link`); `stream/weblink.rs:26` `link_alone`, chamado no começo de `main` | Link com outros argumentos junto: nada abre. |
| Testes | `--test-stream=` | `app.rs:604`; testes em `src/stream/*.rs` | 34 testes no total em `stream/`. |

### 4.19 Controles (`src/pages/controllers.rs`, `draw` linha 179)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Lista de controles ligados (conexão, bateria, taxa) | Página; Launcher `Controllers` | `pages/controllers.rs:256` `draw`; `:332` `pad_card` (um chip por controle); `sys/pads.rs:258` `list`; `controllers.rs:68` `refresh`; `:121` `tick` | Bateria: `battery_label` `controllers.rs:173`. A página: linha do perfil (`pad_profiles::top`), cartão do controle, abas Mapeamento, Macros e Ajustes (`tabs` `controllers.rs:399`, aba em `State.tab`; não trocam com macro gravando). Ajustes (duas colunas que rolam juntas, `pads-set`): `settings` `controllers.rs:425` (avisos, mapeamento, radial, teclado, conflitos, diagnóstico e medir sticks). Desenho do controle na aba Mapeamento: `draw_art` `pages/controllers.rs:648` (chamado em `:271`), com as imagens `art` `:632` (PNG em `res/pads/ds.png`, `ds4.png` e `xbox.png`, feitos dos vetores do mock-up sem o filtro de granulado, 960x705) e as áreas dos botões `HITS_DS` `:533`, `HITS_DS4` `:591` e `HITS_XBOX` `:611` (do `hits.json` do mock-up). O botão apertado acende; o escolhido no editor ganha contorno; clicar num botão escolhe ele (`pad_profiles::choose` `pad_profiles.rs:131`, `pad_profiles::chosen` `:155`). O genérico não tem desenho e segue com `draw_live` `:938`. Foto de teste: `--test-pad=ds\|ds4\|xbox` (só build de depuração; `sys/pads.rs:272` `fake`). A coluna da esquerda do Mapeamento rola (`State.art_h` `pages/controllers.rs:32`, `begin_scroll` `:268`). Embaixo do desenho, o cartão de teste `draw_test` `pages/controllers.rs:681` (último botão; "Responderam X de N" `:695`; os que faltam `:698`; histórico dos últimos 6 apertos com duração em ms e "aperto duplo" quando o botão é apertado de novo em menos de `DOUBLE_MS` = 60 ms depois de soltar `:707`; botão Limpar `:667`). Estado `Test` `controllers.rs:75` (campo `State.test` `:33`), alimentado a cada quadro por `test_feed` `:90`, chamado em `tick` `:121`; zera quando muda o controle escolhido (`:91`). 1 teste em `controllers.rs` (`test_counts_presses_and_flags_double` `:1255`). O botão **Testar**/**Parar** do cartão liga `app.pads.testing` (`pages/controllers.rs:716`); ligado, `measuring` (`:10`) para a navegação do Prisma e o controle aperta qualquer botão. Depois, o cartão "O que mudou na camada X" `draw_changes` `pages/controllers.rs:795`, com as linhas de `pad_profiles::layer_changes` `pages/pad_profiles.rs:138`. Fotos: `--test-page=controllers-edit`, `controllers-macro`, `controllers-settings`. |
| Mapeamento ligado ou desligado | Cartão do controle, **Como o jogo vê este controle** (Nativo ou Teclado e mouse; Virar controle Xbox desligado até a Fase 8). O motor de perfis roda nos dois modos: Nativo usa o perfil Base fora dos jogos, Teclado e mouse usa o Desktop | `pad_card` `pages/controllers.rs:332` (config `pad_map`); estado em `mapping` `pages/controllers.rs:1003` (aba Ajustes, sem o app); `open_editor` `:39` (o editor abre sozinho no primeiro desenho); `sys/remap.rs:1970` `configure` (recebe `kbm`); `sys/remap.rs:1735` `KBM` e `:1991` `kbm`; escolha em `sys/remap.rs:2507` `on_focus` (`named(DESKTOP)` em `:2481`); `link.rs:79` `pad_profiles` manda `mapping` (`:84`) com `remap::kbm()` | Launcher `PadMap`.. No Nativo só age perfil feito pela pessoa: `sys/remap.rs:2117` `native_runs` (Desktop nunca, Base de fábrica não), checado em `feed`; `:2134` `live` decide a leitura de 4 ms do XInput (`set_timer`, `sys/pads.rs`); `switch` chama `pads::poke`. Teste `native_runs_only_profiles_the_person_made`. |
| Testar alto-falante | Ajustes, cartão Avisos, botão **Testar alto-falante** (sem DualSense ou DS4 ligado, mostra um aviso) | `settings` `pages/controllers.rs:451` (botão `pad-speaker-test` `:456`; chama `pad_sound::play` com `Chime::Test` `:75`); `sys/pad_sound.rs:77` `play` | Toca pelo mesmo caminho dos avisos. |
| Ações da seção Controles fora de Ação do Prisma | Editor de perfis, destino da ação (aba Mapeamento) | `sys/remap.rs:223` `PAD_ACTIONS`; `pages/pad_profiles.rs:282` `KINDS` (16 itens) e `:284` `PAD_KINDS`; lista `prisma_ids` em `draw` `pages/pad_profiles.rs:875` | Teclado do Prisma, Radial do Prisma, Desconectar o Bluetooth e Mostrar a bateria saem da lista Ação do Prisma e têm destino próprio. |
| Seção antes do nome da ação | Lista Ação do Prisma, e nomes dos lugares do radial | `sys/remap.rs:227` `action_section`; `pages/pad_profiles.rs:292` `action_choice` (lista em `:1199`); `pages/controllers.rs:986` `radial` (nomes para `slot_select` `:1218`) | |
| Perfil por tipo de controle | Campo `made_for` do perfil; menu do topo do editor | `sys/remap.rs:409` `made_for`; `:214` `made_for_name`; `:220` `to_text` (linha `for dualsense\|ds4\|xbox`); `:786` `parse` (perfil antigo sem a linha: DualSense se `lost_on(Xbox)` não estiver vazio, senão Xbox, `:1012`); `:1018` `lost_on`; `pages/pad_profiles.rs:626` `top` (recebe `family`; itens `KINDS_FOR` `:759`, gravados em `:795`); aviso `app.rs:2989` `profile_loss`, chamado em `pad_events` `app.rs:3002` para `Event::Connected` (`:3013`), `Event::Profile` (`:3029`) e `Event::PadProfile` (`:3053`); teste `profile_knows_its_controller` `sys/remap.rs:2776` | |
| Perfis de controle | Linha do perfil no topo; editor nas abas Mapeamento e Macros | `pages/pad_profiles.rs:626` `top` (lista dos perfis, **Novo perfil** que pede o nome e começa como cópia do aberto ou de fábrica, **Mais** com Renomear, Importar, Exportar, Voltar ao de fábrica (com pergunta; perfil seu vira o Base com o nome e os programas dele; campo `Editor.reset` `:24`), Apagar perfil e os modelos; com o programa em mais de um perfil, **Padrão do programa** e **Abre com o padrão/último usado**); campo `Editor.naming` `:22`; `set_main` `:857`; `:876` `draw` (aba 0 ou 1; a aba Mapeamento começa pelo painel do stick ou gatilho escolhido, montado a partir de `focus` `:922` com `ZONES` `:577`, `stick_row` `:487` (modo na camada e zona morta) e `zone_row` `:563`, depois as ligações, "Ligações" `:941`, o turbo `:1210`, os sticks `:1262` e as zonas mortas `:1272`); `pad_profiles.rs:62` `current`; `:62` `set_program`; `:264` `apply` | Launcher `PadProfiles`, `SetPadProfile`. |
| Modelos prontos | Assistente | `sys/remap.rs:247` `TEMPLATES`; `:253` `template`; `:560` `accessible` | |
| Importar e exportar perfil | Botões | `app.rs:2115` `pad_profile_file` | |
| Radial do controle | Botão | `pages/controllers.rs:986` `radial`; `:1037` `set_radial_program`; `sys/radial.rs:41` `set_programs`; `:87` `configure`; `pages/controllers.rs:1167` `slot_select` (ação ou programa de cada lugar e opção) | Config `pad_radial*`. Lugar **Trocar perfil**: valor `PROFILES` (`sys/radial.rs:18`), o direcional escolhe entre os perfis de `remap::program_profiles`, lidos em `open` (`:162`) para `CHOICES` (`:33`, `choices` `:36`); Confirmar chama `remap::switch_to` em `feed` (`:264`); a janela mostra os nomes na faixa das opções. Lugar com 4 opções: valor `GROUP` (`sys/radial.rs:16`), ligado pela caixa `pad-radial-g<i>` da linha do lugar (em `radial`); ligada, `slot_select` desenha a caixa parada e o lugar perde a ação própria. As opções ficam depois dos 16 lugares, em `sub(i, d)` (`:27`), num total de `ALL` = 80 (`:22`). Pôr `GROUP` num lugar do stick esquerdo liga `pad_radial_keep` (em `radial`).. Combinação do perfil com o botão do radial (PS + Start): `radial::trigger` recebe `combo` (calculado em `remap::feed`), marca `Trig.combo` e deixa o perfil ver os dois, sem abrir nem fazer a função normal. |
| Desenho do radial aberto | Botão segurado (ação 27) | `overlays::Radial` `src/overlays.rs:729`; `sys/radial.rs:139` `view`; `:162` `open`; `:215` `step`; `:264` `feed` | Em `step`, o direcional (`DPAD`, `sys/radial.rs:24`) marca a opção (`sub`) de um lugar `GROUP`, e Confirmar só dispara com uma marcada. O desenho põe a cruz de 4 ícones no lugar e a faixa das 4 opções embaixo (`RD_SUB`, `overlays.rs:749`; `height` `:836`). Foto: `--test-page=radial --test-radial=lugar,opção` (`show_for_test`, `sys/radial.rs:204`). |
| Teclado do Prisma | Ação 26; Launcher `Keyboard` | `sys/osk.rs:228` `view`; `:244` `open`; `:274` `close`; `:282` `feed`; `overlays::Keyboard` `overlays.rs:498`; `set_keyboard` `app.rs:3101` | 6 testes em `sys/osk.rs`. Dois sticks (config `pad_osk_two`, `pad_osk_letters_right`): `configure` (`sys/osk.rs:213`, chamado por `apply_pads`); o círculo de cada stick vem de `wheel_char` (`:101`), e o direcional digita no círculo do direito pela ordem `DPAD` (`:60`); com o stick direito solto, o direcional são as setas (`step`). Fugir do cursor (config `pad_osk_dodge`, ligado de fábrica): `configure` guarda e `dodge` (`sys/osk.rs:224`) lê; o resto fica no desenho (linha abaixo, seção 5). Repetir ao segurar (config `pad_osk_repeat`, `pad_osk_repeat_ms`, 500 ms de fábrica): `configure` guarda, `Osk::new` lê, e `step` (`:140`) repete a tecla de qualquer botão segurado; desligado, só as de `REPEATS` (`sys/osk.rs:62`). Com os dois sticks, LT sozinho dá os acentos (`step`). Cartão: `keyboard` (`pages/controllers.rs:1178`).. Fechar pelo mesmo atalho: `remap::keyboard_buttons` devolve as máscaras (botão com a combinação) das ligações ao teclado no perfil em uso e nos perfis próprios de cada controle; `Osk.close_on` fecha com a máscara apertada de novo (botão sozinho que o teclado usa, `USED`, não fecha). Teste `the_button_that_calls_it_closes_it_unless_the_keyboard_uses_it`. |
| Turbo (repetir o botão) | Controle | `sys/remap.rs:277` `Turbo`; `:283` `new`; `:292` `TURBO_KEY` | 27 testes em `sys/remap.rs`. |
| Macros | Ligação com destino Macro; editor de perfis, seção Macros | Dados: `sys/remap.rs:92` `Out::Macro`, `:123` `Mode`, `:107` `Macro` (passos `Step`, texto `macro`/`mstep` em `to_text` e `parse`). Rodar: `sys/macros.rs:17` `press`, `:44` `release`, `:54` `stop_all`, `:61` `stop_pad`, `:74` `run` (uma thread por macro). Gravar: `:182` `record_start`, `:192` `record_stop`, `:121` `Rec`, `:260` `hooks` (ganchos de teclado e mouse só enquanto grava; ignora o que o Prisma manda e o que acontece com ele em foco, por `nav::front` `sys/nav.rs:163`). Editor: `pages/pad_profiles.rs:1388` (seção Macros), `:457` `remove_macro`, `:456` `step_text`, `:446` `key_out` | 2 testes em `sys/macros.rs`. Para com a parada de emergência, a troca de perfil, o teclado do Prisma e a saída do controle (`stop_all`/`stop_pad` em `remap.rs`). Anti-cheat: não roda, com aviso (`feed`). Aviso fixo da gravação: `Toast::keep` `overlays.rs:257`, `hide` `:264`. Foto: `--test-page=controllers-macro`. |
| Remapear teclado e mouse | Editor de perfis, origem **Teclado e mouse** (`"pp-src"`) | Dados: `sys/remap.rs:326` `Layer.keys` (texto `kbind` em `to_text` e `parse`; vk do Windows, mouse 4 meio, 5 e 6 laterais). Motor: `:1856` `key_profile` (perfil de botões feito das teclas, até `KEY_SLOTS` `:1754`), `:1888` `set_keys` (liga e desliga os ganchos), `:2283` `key_event`, `:2290` `key_tick`, `:2402` `handle` e `:2271` `apply_want` (comuns com `feed`), `:1398` `held_keys` (repetição), `:1742` `FRONT_ADMIN`. Ganchos: `sys/keymap.rs:16` `sync`, `:23` `key_hook`, `:37` `mouse_hook`, `:60` `run` (thread com relógio de 30 ms). Editor: `pages/pad_profiles.rs:304` `KEY_SOURCES`, `:321` `src_name`, `:321` seletor | 1 teste em `sys/remap.rs` (`keys_become_buttons_for_the_engine`). Passa direto com o Prisma em foco (`nav::front`), em programa de administrador e com o mapeamento parado; cliques esquerdo e direito nunca passam pelo motor. Foto: `--test-page=controllers-keys`. |
| Texto rápido | Editor de perfis, seção Texto rápido; destino Texto rápido numa ligação; abreviação digitada em qualquer programa | Dados: `sys/remap.rs:129` `Snip`, `:135` `abbr_ok`, `:150` `Profile.texts`, `:94` `Out::Text` (texto `text` em `to_text` e `parse`). Ligação: `handle` digita com `osk::type_text` (`sys/osk.rs:322`). Abreviação: `key_event` chama `sys/remap.rs:2342` `expansion` e `sys/remap.rs:2359` `typed_key` (palavra em `Run.typed`, zerada na troca de janela em `on_focus`); `set_keys` liga o gancho de teclado quando há abreviação. Editor: `pages/pad_profiles.rs:1506` (seção), `:423` `short`, `:422` `drop_out` (também usado por `remove_macro` `:456`) | 1 teste em `sys/remap.rs` (`quick_text_round_trip_and_abbreviation`). Com anti-cheat, `set_keys` pede só o relógio (`NEED` 4 em `sys/keymap.rs`): sem gancho, vale também para o remapeamento de teclas. Foto: `--test-page=controllers-text`. A rolagem do editor usa a altura do desenho anterior (`Editor.body_h`). |
| Abre sozinho com e chips de camada | Dropdown **Abre sozinho com** na linha das abas; chips de camada acima do desenho | `pages/pad_profiles.rs:190` `opens_with` (opções: nenhum, jogo atual, todos os jogos do Medidor e "Escolher programa", que abre o seletor em `:203`); `:238` `set_game` (liga o programa ao perfil; os outros perfis do programa continuam com ele); `:250` `layers`; `:257` `set_layer`. Seletor: `Target::PadGame` `pages/picker.rs:20`, `:55` chama `set_game`; `WM_PICK_PAD` `app.rs:59`, `:2710` e `:2715` (WPARAM 1 vai para `set_game`), tratado em `:3717`. Desenho: `art_head` `pages/controllers.rs:644`; `tabs` `:425` devolve onde as abas acabam (a lista fica à direita); `draw_art` `:648` desenha os chips. | Foto: `--test-page=controllers-opens` (`app.rs:2550`). |
| Abrir link | Destino de ligação "Abrir link" (botão, ou tecla do teclado e mouse); editor de perfis, campo do link | Dados: `sys/remap.rs:138` `link_ok` (só http ou https, sem espaço, até 500 caracteres); guardado como texto de programa (`Out::Run`, `pages/pad_profiles.rs:1190`). Editor: `pages/pad_profiles.rs:282` `KINDS` (13 itens, o último "Abrir link"), `:27` `Editor.url`, `:1163` campo, `:1165` aviso, `:376` rótulo "Abrir {}". Importar: `pad_profiles.rs:600` pergunta sobre cada link ou programa, também nas ligações de teclado. Foto: `--test-page=controllers-link` (`app.rs:2550`). | 1 teste em `sys/remap.rs` (`link_only_http`). |
| Parar o mapeamento | Combinação de parada (Guia, Voltar e Start) | `remap::set_stopped` `sys/remap.rs:2473`; `STOP_COMBO` `remap.rs:68` | Launcher `PadStop`. |
| Diagnóstico do controle | Página | `pages/controllers.rs:1220` `diagnostic`; `sys/pads.rs:1388` `rate_of`; `:1500` `packet_rate` | |
| Avisos de desconexão, bateria baixa e variação | Automático | `pads::check_warnings` `sys/pads.rs:1719`; `pad_events` `app.rs:3002` | Config `pad_notify`, `pad_low`, `pad_jitter_ms`. |
| Trocar o perfil conforme a janela em foco (vários perfis por programa) | Automático; na linha de perfis, **Padrão do programa** e **Abre com o padrão/último usado** | `remap::on_focus` `sys/remap.rs:2514`; `focus_tick` `remap.rs:2482`; `pick_for` `:1834` (o de agora se o programa não mudou, o último usado, o padrão, o primeiro); `Profile.main` `:347` (texto `main 1`); `REOPEN_LAST` `:1826` com `set_reopen_last` `:1828` (config `pad_reopen_last`, `config.rs:319`, passada em `apply_pads`); último usado em `Run.last`, gravado por `switch` em `pad_last.txt` da pasta de dados (`last_path` `:1751`, `load_last` `:1755`); `program_profiles` `:1762` (perfis para o radial) | Teste `program_with_several_profiles` (`sys/remap.rs:2743`). Fotos: `--test-page=controllers-naming` e `controllers-shared` (`app.rs:2550`). |
| Enviar teclas e cliques | Automático | `remap::send` `sys/remap.rs:2593` | |
| Calibrar stick e gatilho | Automático | `sys/pads.rs:1756` `rest`; `:1870` `outer`; `:1884` `trigger` | |
| Gyro e touchpad: leitura e calibração | Automático (USB sempre; Bluetooth só no modo completo) | `Input` `sys/pads.rs:106` (campos `gyro`, `accel`, `motion`, `touch`); `GyroCal` `:131` `feed` `:145`; `calibrate_gyro` `:172` (pede pela `CAL_ASK` `:170`); `sony` `:1218` lê o relatório completo (`full` `:1046`, `at_full` `:1080`); `full_mode` `:513` lê o relatório de recurso 0x05 no Bluetooth (só leitura), chamado em `read_loop` `:1526`; `read_loop` `:1526` calibra por controle (`cal` `:1199`, `CAL_ASK` `:170`); `Pad.gyro_cal` `:179` | Testes `dualsense_usb_motion_and_touch` `sys/pads.rs:1605`, `gyro_calibrates_when_still` `:2081`. |
| Cartão Gyro e touchpad | Mapeamento, embaixo do teste dos botões (só DualSense e DualShock 4) | `draw_motion` `pages/controllers.rs:760` (chamado em `:278`, depois de `draw_test` `:707`); botão Calibrar chama `pads::calibrate_gyro` `:745` | |
| Gyro por camada (tipo `Motion`) | Linha Gyro de cada camada no editor de perfis (Igual à camada mãe, Desligado ou Mouse) | `sys/remap.rs:336` `Motion` (`Off`; `Mouse(px por grau)`); campo `Layer.gyro`, texto `gyro` em `to_text` e `parse` | `None` é igual à camada mãe; na Base, desligado. |
| Opções do gyro no perfil | Aparecem no editor quando alguma camada usa Mouse | `sys/remap.rs:346` `GyroOpt` (espaço, inverter, suavizar); campo `Profile.gyro_opt`, texto `gyroopt` em `to_text` e `parse` | Testes `player_space_follows_gravity` e `gyro_moves_mouse_only_where_the_layer_turns_it_on` em `sys/remap.rs`. |
| Virar no espaço local ou do jogador | Automático, pelo motor | `sys/remap.rs:357` `gyro_axes` | Espaço do jogador usa a gravidade média do acelerômetro (`Engine.grav`). |
| Gyro da camada ativa | Automático, pelo motor | `sys/remap.rs:1326` `motion` (`touch` falso) | Sobe pelas camadas mãe até achar uma com gyro; sem nenhuma, `Off`. |
| Mouse pelo giro | Automático, em `Engine::step` | `sys/remap.rs:1410` `step`, trecho do gyro em `:1429` (`if inp.motion`) | Suavização pela média curta (`Engine.soft`); inversão em `inv_x` e `inv_y`. |
| Linha Gyro e opções do gyro no editor | Editor de perfis, aba Mapeamento | `pages/pad_profiles.rs:556` `motion_row`; chamado em `:1300` (gyro) e `:1319` (touchpad), dentro de `draw` | "Virar" e as três caixas (`pp-gyro-ix`, `pp-gyro-iy`, `pp-gyro-sm`) só aparecem se alguma camada usa Mouse. A opção **Passa ao jogo** (antes **Desligado**) tem dica na própria linha. |
| Gestos do touchpad (TPU, TPD, TPL, TPR, TPTAP, TPCL, TPCR) | Fileira de botões do editor de perfis (DualSense e DualShock 4), como qualquer botão | constantes `sys/remap.rs:24` `TPU` a `:30` `TPCR`, `:30` `COUNT` (40); `Engine::gesture` `sys/remap.rs:1486`, chamada em `Engine::step` `:1417`; campos `swipe` `:1135` e `half` `:1137`; lista `buttons` `pages/pad_profiles.rs:351` (nomes `:329` a `:335`); ícones `ui/glyph.rs:69` `width` e `:84` `draw` | Com a camada em Mouse ou Stick, o deslizar fica para o mouse ou o stick e só o toque vale como gesto. |
| Regiões e stick virtual do touchpad | Fileira de botões do editor de perfis (Canto do touchpad ↖ ↗ ↙ ↘) e linha Touchpad de cada camada (Stick esquerdo e Stick direito) | constantes `sys/remap.rs:32` `TPUL` a `:35` `TPDR`; `sys/remap.rs:35` `Motion` (`Stick(lado)`); `sys/remap.rs:1319` `touch_stick`; trecho do stick em `Engine::step` `sys/remap.rs:1319` (procura "Stick virtual: vale"); `pages/pad_profiles.rs:556` `motion_row`; ícone `ui/glyph.rs:155` (procura "Região: o quarto") | O dedo vira stick só com a camada em Stick; nesse modo o deslizar não vale como gesto. As regiões valem em qualquer modo do touchpad. |
| Touchpad por camada (campo `Layer.touch`) | Linha Touchpad de cada camada no editor (Igual à camada mãe, Desligado ou Mouse; 200 a 4000 px) | `sys/remap.rs:331` `touch`; trecho do touchpad em `Engine::step` `:1417` (`if let Motion::Mouse(px) = motion(p, layer, true)`); texto `touch` em `to_text` `:666` e `parse` `:792` | Um dedo move o mouse (`:1452` só conta o dedo que já estava no relatório anterior); dois dedos rolam (`:1464`). |
| Flick stick e teste da volta de 360° | Modo "Flick stick" no stick de cada camada (editor de perfis, linha do stick); botão Testar volta aparece com o flick na camada | `sys/remap.rs:313` `Flick(f32)`; `:1401` trecho `Stick::Flick(px) =>` em `Engine::step`; campos `flick` `:1140` e `flick_go` `:1141`; `:2573` `test_turn`; `pages/pad_profiles.rs:488` `stick_row` (opção em `:459`, valor e passo `:465` a `:503`); botão `:1290` (`pp-flick-test`) | Empurrar o stick a 90% liga e a 80% solta; 0,1 s para virar; valor de 500 a 60000 px, 6000 de fábrica (texto `stick <camada> ls\|rs flick <px>`). |
| Curva do mouse, hair trigger e gatilho até o fim | Curva: linha **Curva do mouse** no painel de cada stick; hair trigger: caixa **Hair trigger** no painel de cada gatilho; **L2/R2 até o fim**: botões na fileira de botões do editor | Dados: `sys/remap.rs:395` `Profile.curve`, `:1301` `Profile.hair`, `:37` `LTF`, `:38` `RTF`; textos `curve` (`:619`) e `hair` (`:1308`) em `to_text`, `:666` e `:754` em `parse`. Motor: `:1301` `Engine.hair`, `:1301` `hair`, `:1308` a `:1340` (gatilhos, `hair` e `full`) em `Engine::step`, `:1417` expoente da curva. Editor: `pages/pad_profiles.rs:949` (curva), `:955` (hair), `:322` (botões até o fim), `:86` `test_target` (`"trigger"` `:105`). Ícone: `ui/glyph.rs:95` (traço em `:102`) | 1 teste em `sys/remap.rs` (`triggers_hair_and_full_pull`). Curva: 1 Reta, 2 Suave (de fábrica), 3 Forte; texto `curve ls\|rs <1 a 3>`. Hair: aperta com 4% do curso; soltar de todo sempre solta; texto `hair lt\|rt`. Até o fim: liga a 0,95 e solta abaixo de 0,90; o gatilho comum continua valendo. Foto: `--test-page=controllers-trigger` (`app.rs:2550`). |
| Pro Controller do Switch (Fase 7) | Automático ao ligar um 057E:2009 (Pro Controller ou quem se passa por ele); sem teste em hardware | Família `Family::Switch` `sys/pads.rs:70`, escolhida em `family` `:946`; `switch_setup` `:1372` (subcomandos pelo relatório de saída 0x01, cada um esperando a resposta 0x21: lê a calibração de fábrica dos sticks em 0x603D, liga vibração, relatório completo 0x30, gyro e a luz 1); `StickCal` `:1318` (centro e alcance que cresce, como o SDL); `switch_pro` `:1440` (0x30/0x21 com gyro, bateria em 8 níveis e ZL/ZR digitais; 0x3F simples antes do completo); nomes Nintendo em `button_names` `pages/controllers.rs:200` e ZL/ZR no desenho dos gatilhos; estilo Nintendo em `Style::of` | Fonte do protocolo: `SDL_hidapi_switch.c` (zlib). Sem vibração nem calibração do usuário (0x8010). O Pro 3 no modo S com o receptor do Manba One ligado se conecta no receptor e chega como Xbox 360 pelo XInput (medido em 2026-11-14). Teste `switch_pro_report`. Foto: `--test-pad=switch`. |
| 8BitDo pelo canal do fabricante (Fase 7) | Automático ao ligar um 8BitDo no modo D (Pro 3 testado pelo Bluetooth) | Família `Family::EightBitDo` `sys/pads.rs:67`, escolhida em `family` `:946` (Pro 2, Pro 3, Ultimate 2, 2C e 3; SF30 e SN30 Pro seguem genéricos); `eightbitdo_mode` `:502` (lê o recurso 0x06, ou 0x30 no Ultimate 3, ao abrir); `eightbitdo` `:1280` (relatório 0x01, 0x03 ou 0x04: direcional, sticks, gatilhos, botões, bateria no byte 14, gyro e acelerômetro dos bytes 15 a 26 virados para os eixos do DualSense); vibração em `out_report` (relatório 0x05 de 5 bytes; o Pro 3 está em `WRITE_OK`). Botões de trás `pads::L4`, `R4`, `PL`, `PR` `sys/pads.rs:58` (bits 40 a 43, depois dos virtuais), com nome no perfil em `remap::NAMES` (`COUNT` = 44); `lost_on` avisa "botões de trás" e aceita gyro no 8BitDo; ícone em `ui/glyph.rs` (`width`, `draw`; `Style::of` dá o estilo Nintendo); nomes impressos em `button_names` `pages/controllers.rs:200` | Fonte do formato: `SDL_hidapi_8bitdo.c` (zlib). Taxa do Pro 3 pelo Bluetooth medida em 2026-11-14: 79/s em uso, p50 7,6 ms, p99 45 ms (o relógio do controle dá 78/s: as pausas são dele); parado cai para uns 58/s. Teste `eightbitdo_pro3_report`. Foto: `--test-pad=8bitdo`. |
| Barra de luz (Fase 6) | Linha **Barra de luz** no editor de perfis (DualSense e DualShock 4), com a caixa **Também pelo Bluetooth**, o botão **Escolher cor…** e, pela bateria, **Ajustar cores…** | Dados: `sys/remap.rs:2082` `Profile.light`, `:401` `light_bt`; `sys/remap.rs:455` `Light` (agora com `Blink([u8;3])`, que acende e apaga a cada 500 ms, e `Rainbow`, que volta nas cores em 3 s), `:469` `light_color` (cor da luz no instante, efeitos em degraus de 50 ms, até 20 escritas por segundo; chamada em `PadOut::update` `sys/pads.rs:811` com o relógio do sistema, dois controles no mesmo passo); `sys/remap.rs:482` `Batt` (cores da carga e o passo), `:496` `battery_color` (arredonda pelo passo e mistura), `:504` `FIRST_COLOR`; texto `light` em `to_text` `:666` e `parse` `:792` (o `light battery` sem cores é o formato antigo); `sys/remap.rs:2082` `light` (vale também em Nativo; só `STOPPED` apaga) e `preview_light` `:2083` (pedidos do quadro em `NEXT`, que só valem em `preview_commit`, chamada no fim de `draw` de `pages/controllers.rs`; `preview_clear` ao sair da página; luz do editor aberto, na frente da do perfil em uso; chamada em `pages/pad_profiles.rs:881`, solta em `pages/controllers.rs:259` e em `app.rs:3587` no timer `TIMER_PADS`). Escrita: `sys/pads.rs:556` `LIGHT_DEFAULT`, `:588` `LIT`, `:591` `crc32`, `:594` `out_report`, `:610` `write_report`, `:678` `Want`, `:785` `PadOut`, `:793` `update`, `:811` `release_pads`; campo `Dev.output_len` `:435`. Ao fechar: `app.rs:941` em `shutdown` chama `release_pads`. Editor: linha em `pages/pad_profiles.rs:1358` (`pp-light`, a amostra, `pp-light-pick`, `pp-light-batt` e `pp-light-bt`); as janelas em `light_sheets` `:1640`, com `swatch` `:1623`, `BATT_ROWS` `:1648` e `batt_color` `:1650`; campos `Editor.batt` `:55` e `Editor.pick` `:58`. Seletor de cor: `ui/mod.rs:706` `color_picker` (quadrado e faixa de matiz desenhados por `Canvas::bitmap`), com `hsv_rgb` `ui/gfx.rs:125` e `rgb_hsv` `:142` | Só modelos em `WRITE_OK` `sys/pads.rs:259`. Cor só é mandada quando muda; escrita que falha não repete até reconectar (`failed`). Texto `light off\|battery [vazia meio cheia passo]\|color rrggbb [bt]\|blink rrggbb [bt]\|rainbow [bt]`. Testes: `light_reports` (`sys/pads.rs:1650`), `battery_colors_slide` (`sys/pads.rs:1719`), `triggers_hair_and_full_pull` (`sys/remap.rs:2630`), `battery_light_text_keeps_the_old_format` (`sys/remap.rs:2746`, formato antigo e novo) e `light_effects_blink_and_cycle` (`sys/remap.rs:2796`). Foto: `--test-page=controllers-blink` (`app.rs:2550`; alvo `"blink"` em `test_target`, `pages/pad_profiles.rs:108`). |
| Gatilhos adaptativos, vibração e háptico (Fase 6) | Painel de cada gatilho no editor de perfis: lista **Gatilho adaptativo** (Passa ao jogo, Solto, Resistência, Arma, Vibração), **Começa em**, **Força** e, na Vibração, **Vibra**; ao lado da barra de luz, **Vibrar ao confirmar** com o botão **Testar**; abaixo, **Vibrar com os botões**, **com os gatilhos** e **com os sticks** (0 a 8) | Dados: `sys/remap.rs:403` `Profile.trig`, `:405` `Profile.buzz`, `:439` `Trig`; texto `trig lt\|rt off\|rigid\|click\|buzz <começa> <força> [hz]` (sem linha é `Pass`, passa ao jogo: o bit do gatilho fica desligado; `Off` liga o bit com o modo 0x05; `PadOut::update` manda um `Off` quando o gatilho volta a `Pass`, e `release_pads` só solta os que o Prisma mandava) e `buzz 1\|2` em `to_text` `:656` e `parse` `:792`. Perfil em uso: `sys/remap.rs:2061` `triggers` (a parada de emergência solta) e `preview_trig` `:1931` (gatilhos do editor aberto, postos em `pages/pad_profiles.rs:882` e soltos em `pages/controllers.rs:260` e `app.rs:3587`). Vibração: `sys/pads.rs:495` `buzz`, chamado em `switch` (`sys/remap.rs:2390`) e `stop` (`:2304`), e `sys/pads.rs:504` `buzzing`. Háptico (motor fraco; no DualSense pelo Bluetooth, os atuadores): `sys/remap.rs:2051` `Profile.haptic`, `:414` `Haptic`, `:427` `haptic_level` (gatilhos e sticks, com repouso de 15%), `:2051` `haptic` (perfil em uso; a parada desliga); texto `haptic <botões> <gatilhos> <sticks>`; o toque de 40 ms dos botões fica em `PadOut` (`sys/pads.rs:770` `prev` e `:771` `tap`, contado em `update` `:731`), editor em `pages/pad_profiles.rs:1418` (`pp-hap0` a `pp-hap2`). Atuadores pelo Bluetooth: `sys/pads.rs:684` `coil_report` (relatório 0x32, sequência no byte 1, 3 kHz, 8 bits com sinal, estéreo, seno de 170 Hz; formato conferido no SAxense e no dsz), `:709` `Coil`, `:713` `coil_thread` (linha própria, um relatório a cada 10,67 ms, alguns mudos ao parar), ligado em `update` (`:753`) e guardado em `PadOut.coil` `:727`; escrita recusada volta ao motor. Escrita: `sys/pads.rs:520` `trig_block` (os 11 bytes de um gatilho), `:578` `out_report`. Editor: `pages/pad_profiles.rs:982` (painel do gatilho: `pp-trig{t}`, `pp-trs{t}`, `pp-trp{t}`, `pp-trh{t}`), `:1401` (`pp-buzz` e `pp-buzz-test`) | Gatilho adaptativo só no DualSense; a vibração vale também no DualShock 4. Modos do firmware: 0x05 solto, 0x21 resiste, 0x25 arma, 0x26 vibra, com o curso em dez zonas; os modos simples antigos e os de depuração (0xFC a 0xFE) ficam de fora. `release_pads` solta gatilhos e vibração ao fechar. Testes: `coil_report_layout` (`sys/pads.rs:2129`), `triggers_and_rumble_in_the_report` (`sys/pads.rs:2153`), `haptic_follows_triggers_and_sticks` (`sys/remap.rs:2699`) e `trigger_and_buzz_text` (`sys/remap.rs:2769`). Pesquisa: `research_notes/dualsense-saida-vibracao-gatilhos.md`, `research_notes/dualsense-haptico-bluetooth.md` (formato do 0x32 só no SAxense, MPL-2.0; sem segunda fonte) e `research_notes/dualsense-haptico-windows.md` (cabo: só por áudio em modo exclusivo, não feito). Foto: `--test-page=controllers-trigger`. |
| Avisos pelo alto-falante (Fase 6) | Caixa **Som no alto-falante do controle** no cartão **Avisos** da aba Ajustes (desligada de fábrica) | Módulo novo `sys/pad_sound.rs`: `Chime` `:17`, `for_event` `:28`, `notes` `:40`, `samples` `:51`, `is_pad_endpoint` `:66`, `play` `:74`, `play_all` `:96`, `play_on` `:110`; chamado em `pad_events` `app.rs:3002` (chamada em `:3006`) (laço de `take_events`); config `pad_speaker` `config.rs:301` (campo), `:449` (padrão), `:752` (leitura), `:881` (escrita); caixa `"pad-speaker"` `pages/controllers.rs:453` | Toca em modo compartilhado em toda saída de som cujo nome tem "Wireless Controller", "DualSense" ou "DualShock" Pelo Bluetooth também toca, por `bt_audio::play` chamado em `play` de `sys/pad_sound.rs:96`, um controle de cada vez; pelo cabo, na saída de som do controle. Canais 1 e 2 apenas (no DualSense, 3 e 4 são o háptico). Ao conectar, tenta por até 3 s, porque a saída de som aparece depois do controle. Teste `chimes_are_short_and_soft` `sys/pad_sound.rs:163`. |
| Som do PC para o háptico (Fase 6) | Quarto campo **Vibrar com o som do PC** (0 a 8) no editor de perfis, embaixo de Vibrar com os sticks | Módulo novo `src/sys/audio_haptic.rs`: `demand` `:37`, `envelope` `:56`, `usb_active` `:61`, `take` `:67`, `Shape` `:87`, `gain` `:108`, `usb_pads` `:113`, `run` `:154`, `work` `:159`; estáticos `DEMAND` `:21`, `RUNNING` `:22`, `ENV` `:24`, `USB` `:26`, `RING` `:28`; campo `Haptic.audio` `src/sys/remap.rs:386` (texto `haptic <botões> <gatilhos> <sticks> [som]`, `remap.rs:2058` e `:797`); `src/sys/pads.rs`: `coil_report` `:716` (recebe `audio`), `type Coil` `:628` (terceiro campo), `coil_thread` `:745` chama `audio_haptic::take` `:649`, `PadOut::update` `:811` chama `audio_haptic::demand` `:708` e usa `envelope` `:735`; loopback por `capture::sound::open` `src/capture/sound.rs:46` e `read` `:135` | DualSense no cabo escreve nos canais 3 e 4 da saída de som de 4 canais do controle em modo compartilhado; pula se o controle é a saída padrão (o loopback leria o que ele escreve). Bluetooth pelo relatório 0x32 a 3 kHz (média de 16 amostras de 48 kHz). Os demais pelo motor fraco, com o pico de cada 10 ms. Passa-baixa de um polo em 500 Hz. Vale a maior força entre os controles. O sinal para 2 s depois do último pedido. Testes: `ring_readers_keep_their_place` `audio_haptic.rs:247`, `loopback_starts_and_stops` `:237` (ignorado; abre o loopback de verdade), `coil_report_layout` `pads.rs:1670`, `haptic_follows_triggers_and_sticks` `remap.rs:2699`. |
| Mods do DSX (Fase 6) | Cartão **Mods do DSX** na aba Ajustes, caixa **Aceitar mods pela porta UDP 6969** (desligada de fábrica) | Módulo novo `src/sys/dsx.rs`: `PORT` `:14`, `Over` `:18`, `light` `:28`, `triggers` `:33`, `set_enabled` `:38`, `serve` `:49`, `reply` `:71`, `apply` `:87`, `INSTRUCTIONS` `:114`, `MODES` `:115`, `zone` `:118`, `power` `:123`, `trigger` `:131`, `Json` `:179` (`parse` `:189`, `get` `:195`, `code` `:214`); ligado em `src/sys/remap.rs` dentro de `triggers` `:2068` (chama `dsx::triggers` `:2068`) e de `light` `:2089` (chama `dsx::light` `:2089`); `set_enabled` chamado em `apply_pads` `src/app.rs:3223` (`dsx::set_enabled` `:3228`); config `dsx_udp` em `src/config.rs`: campo `:303`, padrão `:450`, leitura `:753`, escrita `:882`; cartão `dsx` `src/pages/controllers.rs:1022` (caixa `"pad-dsx"` `:951`) | Escuta só 127.0.0.1:6969. JSON `{"instructions":[{"type":N,"parameters":[...]}]}`, nomes sem diferença de maiúsculas, type por número ou nome; usa TriggerUpdate (1), RGBUpdate (2) e ResetToUserSettings (7). Os modos 0 a 23 do DSX viram Off, Resistência (Rigid), Arma (Click) ou Vibração (Buzz). O pedido vale para todos os DualSense, também pelo Bluetooth, e passa na frente do perfil; desligar ou fechar solta; responde no formato do DSX. Testes: `dsx_messages_set_light_and_triggers` `dsx.rs:338`, `dsx_port_answers` `dsx.rs:320` (ignorado; abre a porta de verdade). Pesquisa: `research_notes/dsx-udp-6969.md`. |
| Áudio pelo Bluetooth (Fase 6) | Automático, pelo `pad_sound::play` (`sys/pad_sound.rs:77`) quando o controle é Bluetooth | Módulo novo `src/sys/bt_audio.rs`: `play` `:24`, `rate_of` `:33`, `pcm` `:37`, `pace` `:42`, `ds_volume_report` `:55`, `ds_audio_report` `:68`, `opus_frames` `:89`, `dualsense` `:123`, `ds4_volume_report` `:149`, `ds4_audio_report` `:163`, `sbc_frames` `:178`, `ds4` `:198`; constantes `DS_VOLUME` `:19` e `DS4_VOLUME` `:20`; teste `bluetooth_audio_reports` `:221` | DualSense pelo relatório 0x39 (547 bytes, dois quadros Opus de 10 ms e 200 bytes, CELT a 160 kb/s fixos, TOC 0xF4, a cada 21,3 ms, três relatórios calados antes; volume do alto-falante no 0x31, byte 5 da parte comum com o bit 0x20). DualShock 4 pelo 0x17 (462 bytes, quatro quadros SBC de 109 bytes, 32 kHz, estéreo conjunto, 16 blocos, 8 sub-bandas, SNR, bitpool 48, a cada 16 ms; volume no 0x11 com o byte 2 = 0xA0). Bibliotecas: `opus-head-sys` (libopus, BSD, compilada pelo CMake) e `nanoaudio-sbc` (MIT), em `Cargo.toml`. `crc32` e `write_report` de `src/sys/pads.rs` agora são `pub(crate)`. Formato só no dsz (MIT); pesquisa em `research_notes/bluetooth-audio-haptico.md`. |
| Desconectar controle Bluetooth | Ação 24 (nos botões, em **Ação do Prisma**, e no radial); botão **Desconectar** no cartão do controle (sempre ligado) | `pads::disconnect` `sys/pads.rs:351` (controle do XInput: o único aparelho Bluetooth da lista `IG`; com mais de um, nada); botão em `pad_card` `pages/controllers.rs:332` | Pelo cabo, nada acontece. |
| Perfil por controle | Lista **Perfil deste controle** no cartão do controle (**Perfil do programa** ou um perfil) | `Run.fixed` `sys/remap.rs:1697`; `prof_of` `:1787` (o próprio do controle ou o do programa; usado em `feed`, `handle`, `light`, `triggers` e `haptic`); `set_fixed` `:1792` (config `pad_fixed`, `config.rs:309`, passada em `apply_pads`); `pad_switch` `:1714` (radial ou ligação: com perfil próprio, troca só o do controle e manda `Event::PadProfile`, `sys/pads.rs:191`, que o app grava em `app.rs:3053`); `switch_pad` `:1730` (chamado pelo radial) | Teste `pad_with_its_own_profile` (`sys/remap.rs:2727`). A chave é o caminho do aparelho (ou `xinput:N`). |

### 4.20 Widget (`src/pages/widget.rs`, `draw` linha 5)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Mostrar ou esconder o widget | Atalho `HK_WIDGET` (`app.rs:142`); ação 19; bandeja | `toggle_widget` `src/app.rs:2748`; `set_widget` `app.rs:2753` | `hk_widget` em `config.rs:194`. |
| Janela do widget | Automático | `overlays::Widget` `src/overlays.rs:112`; `update` `overlays.rs:200`; `set_visible`; `size` `overlays.rs:260` (tamanho conforme o texto) | Config `widget_on`, `widget_style`, `widget_x`, `widget_y`, `widget_items`. |
| Itens que aparecem (lista "O que aparece") | Página, caixas `wi0`..`wi8` | `ITEMS` `overlays.rs:22`; `parse_items` `:35`; `toggle_item` `:43` (o último ligado não desliga); `items` `:81`; teste `widget_tests` `:374` | `widget_items` em `config.rs:226`, um bit por item. |
| Leituras novas (só dos itens ligados) | Automático | `Widget::refresh` `overlays.rs:171` (plano e HDR a cada 5 s, VRR a cada 5 s em outra thread); `power::active`; `hdr::hdr_on` `src/sys/hdr.rs:92`; `drs::vrr_active` `src/sys/drs.rs:273` (`vrr_state` decide, teste `vrr_tests`); `kmt::Kmt::temp` `src/sys/kmt.rs:83`; `monitor::GPU_TEMP` `monitor.rs:101`; `cputemp::WIDGET` `cputemp.rs:41` | VRR pela NvAPI_Disp_GetVRRInfo da tela do widget: Ligado só com a tela em modo VRR; o G-SYNC global é só a preferência e não serve. Outras placas mostram —. Temperatura da GPU pelo D3DKMT (qualquer placa). |
| Prévia na página | Página | `overlays.rs:364` `preview` | |
| Monitor onde ele fica | Automático | `overlays.rs:194` `focus_monitor` | |

### 4.21 Configurações (`src/pages/adjust.rs`, `draw` linha 4)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Idioma, inicialização, administrador, Gerenciador de Tarefas | Página | `pages/adjust.rs:4` `draw`; `app.rs:1672` `set_admin`; `:1685` `set_taskmgr`; `:1714` `taskmgr_done`; `:1729` `taskmgr_refused` | Config `lang`, `always_admin`, `tm_hook`, `pad_notify`. |
| Restaurar padrões de uma página | Botão | `reset_defaults` `app.rs:2724`; `Config::reset` `src/config.rs:990`; `defaults_button` `pages/mod.rs:443` | |
| Atualizar | Ver seção 3.1 | `pages/adjust.rs:151` `draw_update` | |
| Cartões de atalho | Página | `src/ui/mod.rs:785` `hotkey` (desenho do cartão de tecla); `register_hotkeys` `app.rs:1043` | |

### 4.22 Guia e Reportar bug (fora da barra, ao lado do nome)

| Função | Como é chamada | Onde no código | Observações |
|---|---|---|---|
| Guia e tutorial | Botão "Guia e tutorial" | `pages/guide.rs:99` `draw`; `guide.rs:68` `build`; `guide.rs:55` `png_size` | Imagens em `docs/guide/*.png`. Lista `HELP` em `pages/mod.rs`. |
| Reportar bug | Botão "Reportar bug" | `pages/report.rs:240` `draw`; `report.rs:33` `note`; `:52` `new`; `:156` `prepare`; `:181` `encode`; `:209` `scrub`; `:241` `machine`; `:273` `recent_log`; `app.rs:2070` `prepare_report`; `:2092` `send_report`; `:2106` `send_report_direct` | Dados pessoais saem em `scrub`. Teste: `--test-report=` (`app.rs:643`); `report.rs:388` `self_test`. |
| Novidades | Página | `pages/news.rs:82` `draw`; `:34` `has_news` | `check_news` `app.rs:1020`. |
| Boas-vindas | Primeira abertura | `pages/welcome.rs:61` `draw`; `:19` `new`; `:46` `finish` | `first_run` `app.rs:1004`. |

---

## 5. Janelas por cima (overlays)

| Janela | Como aparece | Onde no código | Observações |
|---|---|---|---|
| Medidor (compacto e completo) | Atalhos ou controle | `src/meter.rs:1169` `paint`; `src/app.rs:2313` `meter_paint` | Ver 4.2. |
| Widget | Atalho `HK_WIDGET` | `src/overlays.rs:38` `Widget` | Ver 4.20. |
| Aviso (toast) | Automático | `src/overlays.rs:203` `Toast`; `:252` `info`; `:239` `volume`; `:272` `info_at` | Mensagens do controle, do Medidor, do HDR etc. |
| Pergunta com botões | Automático | `src/overlays.rs:284` `Prompt`; `:294` `PromptAnswer`; `:333` `ask`; `:345` `hide`; `:352` `mouse`; `PROMPT_MS` `overlays.rs:301` (10 s) | |
| Registro (gravação) | Durante a gravação | `src/overlays.rs:410` `Hud`; `:445` `update`; `src/stream/hud.rs` | |
| Teclado do Prisma | Ação 26 | `src/overlays.rs:498` `Keyboard`; `:598` `update`; `:654` `tick`; `src/sys/osk.rs` | Com os dois sticks, a janela dobra de largura (`width`, `overlays.rs:675`). Fugir do cursor: `watch_caret` (`overlays.rs:549`) lê o cursor de texto 5 vezes por segundo numa thread à parte com `caret` (`:523`: `GetGUIThreadInfo`, senão `OBJID_CARET` da acessibilidade) e guarda em `CARET` (`:516`); `tick` sobe o teclado para o alto da tela quando `dodge_up` (`:569`) diz que o cursor está atrás dele embaixo, com 35% do caminho por quadro. Testes: `dodge_only_when_behind` e `caret_now` (ignorado; lê a janela em foco). |
| Ícones dos botões do controle | Aba Controles, teclado, radial | `src/ui/glyph.rs`: `Style` (`:10`, Xbox, PlayStation, Nintendo pelo vid 0x057e, números no genérico) com `of` (`:20`) e `current` (`:30`, o primeiro controle); `draw` (`:84`) desenha um botão e devolve a largura (`width` `:69`); `dpad` (`:184`, braço aceso), `face` (`:199`, os quatro em losango), `hints` (`:233`, linha de dicas com ícone e texto, que encolhe para caber); `DPAD_ANY` (`:66`) é o direcional sem braço aceso. Folha de teste: `--test-page=glyphs` (`sheet` `:211`, só no debug) | Sem cor própria: a cor vem de quem desenha. 1 teste. |
| Radial | Botão segurado (ação 27) | `src/overlays.rs:729` `Radial`; `:767` `update`; `:824` `tick`; `src/sys/radial.rs` | |
| Foto (editor) | Depois do atalho de foto | `src/capture/shot.rs:728` `open` | |
| Seletor de programa | Botão | `src/app.rs:1938` `show_picker` | |

---

## 6. Números e testes de apoio

- Testes `#[test]` por área (contagem feita com grep): `sys/remap.rs` 21; `sys/macros.rs` 2; `link.rs` 19; `sys/pads.rs` 14;  `sys/osk.rs` 6; `sys/radial.rs` 6; `sys/power.rs` 5; `sys/index.rs` 4; `sys/cmds.rs` 4; `config.rs` 2; `sys/update.rs` 2; `sys/quick.rs` 2; `sys/stores.rs` 2; `app.rs` 1; `i18n.rs` 2; `sys/nvset.rs` 1; `pages/nvidia.rs` 1; `pages/mod.rs` 1; `ui/glyph.rs` 1; `sys/nav.rs` 2; `ui/mod.rs` 1; `overlays.rs` 3 (1 ignorado); `stream/` 34 no total.
- `src/selftest.rs` (só debug): `run` (linha 5), `misc` (23), `fake_uninstall` (55), `tweaks` (110), `audit` (162), `toolrun` (189), `tweak_log` (223).

---

## 7. O que ficou incerto ou não foi mapeado

- `src/stream/`: não mapeei uma a uma as funções de `host.rs` (70), `net.rs` (63), `kvm.rs` (34), `opts.rs` (27), `pads.rs` (26), `wave.rs` (24), `service.rs` (19), `view.rs` (16), `hud.rs` (15), `code.rs` (14), `link.rs` (14). Só listei as entradas das abas e os pontos de entrada.
- `src/capture/`: encoder, nvenc, nvdec, decoder, live, live_enc, mux, sound e hdr não foram mapeados função a função. Só as de entrada (`capture/mod.rs`, `shot.rs`, `engine` por nome).
- `src/sys/hdrcal.rs` (23 funções), `src/sys/mft.rs` (28), `src/sys/frames.rs` (15), `src/sys/inject.rs` (12), `src/sys/etw.rs`, `src/sys/gamelog.rs`, `src/sys/cputemp.rs` e `src/sys/kmt.rs`: listei só a entrada usada pela página.
- `src/sys/nvml.rs`: o grep o tratou como arquivo binário e não listou as funções.
- `crates/prisma-link/src/client.rs`, `server.rs`, `pipe.rs` e `frame.rs` não foram abertos: as funções deles não estão mapeadas. Só `lib.rs` e `proto.rs` (nomes e linhas de `Request`, `Reply`, `Page`, `MeterView`, `VolumeChange`, `NvidiaKind`, códigos e capacidades).
- Atalhos padrão: só confirmei `hk_fullscreen`, `hk_widget`, `hk_optimize` (Ctrl+Alt+F, W e O) e `hk_force` (Alt+F4) em `config.rs:347-351`. Os demais padrões não foram conferidos.
- Linhas da tabela `HK` em `pad_action` (`src/app.rs:3156-3160`) e as de `on_link` (`src/link.rs:316` a `456`) vieram de grep e leitura parcial. Confira com grep antes de citar.
- Não confirmei o nome da função que contém `src/stream/hud.rs:575` (a variável `PRISMA_LAYER_DUMP` é definida dentro de um bloco com `proc` na linha 549).
- Não li o ponto do clique da barra lateral que chama `go`. Sei que `NAV` fica em `pages/mod.rs:273` e que `go` é `app.rs:1419`.
- As páginas de HDR e Medidor, e as demais, tiveram só as funções `draw` e auxiliares listadas pelo grep. Os botões de cada aba não foram lidos linha a linha.
- Os números de `docs/guide/GUIA.md` não foram usados como fonte de comportamento; as funções saíram do código.
- Não rodei `cargo` nem `cargo fmt`. Não alterei nenhum arquivo do projeto além deste mapa.
