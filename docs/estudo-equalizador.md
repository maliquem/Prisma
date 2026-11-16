# Estudo: equalizador paramétrico e gráfico na página Som

Pedido: equalizador paramétrico e gráfico **dentro da página Som** (sem página nova), com perfil por
aparelho de som e por programa, e controle de sistemas 5.1/7.1. **Motor próprio do Prisma**: o
Equalizer APO serviu só para entender o que precisa ser feito.
Estudo e plano apenas; nenhum código de produto mudou. Mock-up: `docs/mockup-equalizador.html`.
Fontes com URL: notas `pesquisa-eapo.md` e `pesquisa-apo-proprio.md` na pasta de notas das sessões.

Legenda: **[conf]** confirmado na documentação oficial; **[conferir]** não confirmado, a verificar
num teste antes de depender disso.

---

## Parte 1. O que o Equalizer APO ensina

- **Como entra no som:** é um APO, DLL COM que o motor de áudio do Windows (`audiodg.exe`, conta
  LOCAL SERVICE) carrega no caminho de cada aparelho. Tudo que toca em modo compartilhado passa por
  ele. **[conf]**
- **Instalação por aparelho:** registra a classe COM e a chave
  `HKLM\SOFTWARE\Classes\AudioEngine\AudioProcessingObjects\{clsid}`, e põe o CLSID dele no
  `FxProperties` do aparelho
  (`HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render\{id}\FxProperties`).
  Guarda os valores originais (`HKLM\SOFTWARE\EqualizerAPO\Child APOs`) e **roda o APO do
  fabricante dentro do dele**, para não perder o que o driver faz. Pede reinício. **[conf]**
- **Assinatura:** o caminho protegido do áudio (DRM) só aceita APO assinado. O Equalizer APO liga
  `DisableProtectedAudioDG=1` (em `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Audio`) para
  carregar sem assinatura. **[conf na wiki dele e na Microsoft]**
- **Parâmetros:** o APO não lê a pasta do usuário. A config fica em Program Files e uma thread
  dele observa a pasta e recarrega sozinha. **[conf]**
- **Filtros:** fórmulas do "Audio EQ Cookbook" (RBJ): pico, prateleiras, passa-baixa/alta, corte,
  passa-tudo; gráfico = curva interpolada entre pontos; `Channel`, `Preamp`, `Delay`, `Copy` por
  canal. Nomes de canal: `L R C LFE RL RR RC SL SR` (o sub é **LFE**). **[conf]**
- **Limites dele que valem para o nosso também:** modo exclusivo (WASAPI exclusive, ASIO) passa sem
  efeito; "Melhorias de áudio" desligadas desligam APOs; atualização do driver de áudio pode apagar
  o `FxProperties`; não tem filtro por processo. **[conf]**
- **Licença:** GPL v2. O Prisma (PolyForm Noncommercial) **não copia nada** dele; só usamos o que a
  Microsoft documenta e as fórmulas públicas.

### O que muda por ser nosso

- **Por programa de verdade pode existir.** Um APO no lugar **SFX** (efeito de fluxo) tem **uma
  instância por fluxo, antes da mistura** **[conf]**. Falta saber se a instância descobre de qual
  programa é o fluxo: a interface (`APOInitSystemEffects3`, `IAudioSystemEffects3`) não mostra PID
  **[conferir]**. É o primeiro teste do plano (seção 2.2).
- **Sem estalo na troca de perfil:** o nosso pode fazer uma transição de 20 ms entre o perfil velho
  e o novo; o Equalizer APO recarrega seco.
- **Sem arquivo de texto no meio:** o Prisma manda os parâmetros prontos.
- Referência em Rust: o crate `tympan-apo` 0.1.0 (SFX/MFX com `register_apo!`). Novo demais e
  licença não conferida; serve de leitura, não de dependência. O `windows` 0.62 que o Prisma já usa
  tem `#[implement]` para as interfaces COM.

### Alternativas descartadas

| Caminho | Por que não |
|---|---|
| Usar o Equalizer APO | Decisão dela: motor nosso |
| Driver de áudio virtual (kernel) | Driver assinado pela Microsoft, outro projeto inteiro |
| Captura por processo + tocar de novo (`PROCESS_LOOPBACK`, já em `src/capture/sound.rs`) | O som original continua tocando; 20 a 40 ms de atraso. Fica só como plano C para "por programa" |

---

## Parte 2. Plano

### 2.1 Peças

| Peça | Onde | O que faz |
|---|---|---|
| **DSP** | `crates/prisma-eq` (lib, novo) | Biquads, curva (resposta em frequência para desenhar), gráfico → filtros, atraso, ganho, transição. Sem Windows, sem alocação no processamento. Usado pelo APO **e** pela página (a curva desenhada é a mesma conta que toca) |
| **APO** | `crates/prisma-apo` (cdylib, novo) → `prisma_apo.dll` | Classe COM com `IAudioProcessingObject`, `IAudioProcessingObjectRT`, `IAudioProcessingObjectConfiguration`, `IAudioSystemEffects2` (o mínimo pela Microsoft: `Initialize`, `IsInputFormatSupported`, `LockForProcess`, `UnlockForProcess`, `APOProcess`). Roda o APO original do fabricante dentro dele |
| **Instalação** | `src/sys/eqapo.rs` (novo) | Registrar, pôr e tirar de cada aparelho, guardar e devolver os valores originais. Atrás de `trait Host` como `sys/xbridge.rs` |
| **Estado e perfis** | `src/sys/eq.rs` (novo) | Perfis, regras por aparelho e por programa, escolha do perfil ativo, escrita do arquivo de estado |
| **Página** | `src/pages/volume.rs` (`draw`, linha 210) + `src/pages/equalizer.rs` | Abas **Programas** (como hoje) e **Equalizador**, com o `tabs` de `pages/copies.rs:199` |
| **Laço** | `src/app.rs` (`sound_tick`, linha 2064) | `eq_tick`: perfil ativo de cada aparelho/programa; grava o estado só quando muda |
| **Canais** | `src/sys/audio.rs` | `speakers(id)`: `PKEY_AudioEndpoint_PhysicalSpeakers` (5.1 = 0x3F, 5.1 de lado = 0x60F, 7.1 = 0x63F); `tone(id, canal)`: ruído rosa num canal só |
| **UI** | `src/ui/mod.rs` | `drag_point`, igual ao `slider` (`:765`) em x e y; a curva sai de `c.line` |
| **Desinstalar** | `src/startup.rs` (`uninstall`, linha 396) | Devolve os aparelhos ao original e apaga o registro do APO |

Os dois crates entram em `crates/`, ao lado do `prisma-link`, no mesmo workspace e no mesmo setup.

### 2.2 Primeiro: os testes que decidem o desenho (antes da interface)

Um APO mínimo que só passa o som e escreve um log em `C:\ProgramData\Prisma\eq\apo.log`, num PC
de teste ou máquina virtual (nunca no Windows dela sem ela pedir):

1. **Carrega?** Sem assinatura e com `DisableProtectedAudioDG=1`; assinado, sem a chave; com
   Integridade de memória (HVCI) ligada; no 24H2.
2. **SFX por fluxo sabe o programa?** Logar o que chega em `Initialize` (`APOInitSystemEffects2/3`),
   as propriedades do aparelho e do fluxo, e a ordem de criação das instâncias contra as sessões
   do `IAudioSessionManager2` (que têm PID). Se der para casar com segurança, **EQ por programa de
   verdade**; senão, plano B (seção 2.4).
3. **Formato:** o que chega em `LockForProcess` (taxa, canais, máscara, float32).
4. **Encadear o APO original** (Realtek, Dolby etc.) sem perder som nem efeito.
5. **Recarregar sem reiniciar o PC:** reiniciar o serviço `Audiosrv` ou desligar e ligar o aparelho.
6. **Permissão no `FxProperties`:** se a chave aceita escrita do administrador ou exige tomar posse.

**Roteiro** (o APO de teste está em `crates/prisma-apo`; `cargo test` ali roda o teste dentro do
processo, sem registrar nada):

1. No PC de teste: `cargo build --release` em `crates/prisma-apo`.
2. PowerShell como administrador: `scripts\instalar-teste.ps1 -Aparelho "<parte do nome>"`. Guarda
   cópia de tudo em `C:\ProgramData\Prisma\eq\backup`, registra o APO, põe no lugar SFX do aparelho e
   reinicia o áudio. Se a chave `FxProperties` negar acesso, só avisa (é a resposta do teste 6).
3. Tocar som em dois programas ao mesmo tempo. Ler `C:\ProgramData\Prisma\eq\apo.log`: sem log, não
   carregou (teste 1, sem assinatura e com proteção); uma instância por fluxo e as propriedades de cada
   uma (teste 2); a linha `LockForProcess` (teste 3).
4. Se não carregou: `scripts\remover-teste.ps1`, depois instalar de novo com `-SemProtecao`.
5. Repetir com Integridade de memória ligada.
6. `scripts\remover-teste.ps1` devolve o registro como estava.

Chaves do `FxProperties` (conferidas no PC dela, 16/11/2026):

- `{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},5/6/7`: efeito único antigo (SFX, MFX, EFX).
- `{d04e05a6-...},13/14/15`: listas de efeitos (`PKEY_CompositeFX_*`), que valem quando existem. Os
  fones HyperX têm só a `,13` com o efeito da DTS.
- `{d3993a3f-99c2-4402-b5ec-a92a0367664b},5/6/7`: os **modos** de processamento suportados
  (`{C18E2F7E-...}` é o modo padrão), não efeitos. Uma versão do script pôs o nosso CLSID ali por
  engano: não carregou e deixou o "Beyond TV" (com Dolby Atmos) mudo até remover.

**Teste 1, respondido em parte (16/11/2026, fones HyperX, Windows 26300):** sem assinatura e com
`DisableProtectedAudioDG=1`, o audiodg carrega a DLL. Ele cria o APO **agregado** (`CreateInstance`
com um IUnknown de fora, pedindo `IUnknown`); o `#[implement]` do windows-rs não agrega, então o
`prisma-apo` monta o objeto à mão (`Agg`: IUnknown de dentro próprio, tabelas do windows-rs com o
IUnknown delegando para o de fora). Falta: assinado sem a chave, e com HVCI ligada.

O script acrescenta o nosso no fim da `,13`, depois dos efeitos do aparelho, o que já cobre o teste 4.
Sem lista, ele troca o efeito antigo `,5` pelo de teste.

**Testes 2 a 5 (16/11/2026, mesmo PC e fones):**

- Teste 3: `LockForProcess` recebe float32, 2 canais, 96000 Hz, máscara 0x3, até 1056 quadros por
  chamada, com buffers de entrada e saída separados (o APO tem que copiar). O formato é o do
  aparelho, não o do programa.
- O audiodg cria muitas instâncias (21 numa sessão): a maioria passa por `Initialize` e pela
  pergunta de formato e é destruída sem processar nada (sondagem). As que chegam a `LockForProcess`
  processam um fluxo do começo ao fim.
- Teste 2: **há uma instância por fluxo.** Com dois programas tocando juntos, as instâncias 19
  (56 s) e 21 (53 s) processaram ao mesmo tempo, e a 7 processou sem parar os 271 s da sessão (um
  fluxo que fica aberto, provavelmente sons do sistema ou um programa em segundo plano). Todas rodam
  na mesma thread do audiodg. Mas `Initialize` só traz o modo (padrão), o aparelho (interface USB e
  id do endpoint), 86 propriedades do aparelho e 7 de efeitos: **nada identifica o programa** (nem
  PID, nem exe, nem sessão). EQ por programa de verdade é possível, mas exige casar cada instância
  com a sessão do `IAudioSessionManager2` por outro caminho (hora de início e fim do fluxo contra o
  estado das sessões); sem um casamento seguro, vale o plano B da seção 2.4. Próximo teste: o APO
  escreve a hora de `LockForProcess`/`UnlockForProcess` numa memória compartilhada e o Prisma
  compara com as sessões que começaram e pararam naquele instante.
- Teste 4: o efeito da DTS (`dtsapo64.dll`) continua carregado e o nosso roda depois dele, com som.
- Teste 5: reiniciar `AudioEndpointBuilder` e depois `Audiosrv` recarrega a lista sem reiniciar o PC.

Teste 6, parte já respondida: em `FxProperties` o grupo Administradores só tem `SetValue` e leitura
(o dono é o TrustedInstaller). `New-ItemProperty` pede mais direitos e leva "acesso negado"; abrir a
chave pedindo só `SetValue | QueryValues` funciona sem tomar posse. Apagar a chave não é permitido, então
o remover desfaz valor por valor.

Com "Melhorias de áudio" desligadas no aparelho (`{1da5d803-d492-4edd-8c23-e0c0ffee7f0e},5 = 1`,
`PKEY_AudioEndpoint_Disable_SysFx`) o Windows não carrega efeito nenhum, nem o nosso. Era o caso do
"Beyond TV" dela. O Prisma vai precisar ligar isso ao ativar o equalizador e avisar na tela.

**Dolby Atmos (som espacial):** o "Beyond TV" usa Dolby Atmos, que desliga as Melhorias de áudio.
Forçar as Melhorias de áudio e pôr o APO na lista deixou o aparelho mudo e travou a troca do Atmos nas
configurações; o remover devolveu tudo (16/11/2026). O equalizador não deve tocar em aparelho com som
espacial ativo; a tela avisa e oferece desligar o Atmos primeiro. O teste segue num aparelho comum.

### 2.3 O APO por dentro (`prisma-apo`)

- **Tempo real:** `APOProcess` não aloca, não trava, não lê arquivo, não entra em pânico. Toda
  função exportada passa por `catch_unwind`; qualquer erro vira "passar o som sem mexer".
- **Parâmetros:** uma thread do APO (fora do tempo real) observa
  `C:\ProgramData\Prisma\eq\` (`ReadDirectoryChangesW`), lê o `estado.bin`, calcula os coeficientes
  para a taxa daquele fluxo e publica por troca atômica de ponteiro (três buffers prontos de
  antemão; o velho é liberado pela thread, nunca no tempo real).
- **Transição:** perfil novo entra em 20 ms, misturando a saída velha e a nova.
- **Por canal:** filtros "todos" + filtros do canal, ganho, atraso (linha de atraso alocada no
  `LockForProcess` até 50 ms na taxa do fluxo).
- **Proteção:** pré-amplificação automática opcional (o pico da curva não passa de 0 dB) e limitador
  suave no fim, desligável.
- **Números:** coeficientes e estado do filtro em f64 (TDF-II), restos minúsculos zerados a cada bloco.

`estado.bin` (escrito só pelo Prisma, em `estado.tmp` e trocado atomicamente): versão, e uma lista
de blocos `{aparelho, programa (vazio = qualquer), canais, bandas, ganho, atraso}`. Versão
diferente da que o APO conhece: ele passa o som sem mexer.

### 2.4 Perfis e regras

- Perfil = arquivo de texto na pasta de dados do Prisma (`eq\<nome>.txt`), em linhas fáceis de
  ler e no mesmo jeito do AutoEQ (`Filter 1: ON PK Fc 250 Hz Gain -2 dB Q 1.2`), para importar
  perfis de fone sem esforço:
  ```
  modo=parametrico
  preamp=-4
  Filter 1: ON LSC Fc 80 Hz Gain 3 dB Q 0.7
  Filter 2: ON PK Fc 250 Hz Gain -2 dB Q 1.2
  [C] ganho=2 atraso=0
  Filter 1: ON PK Fc 2500 Hz Gain 2 dB Q 1
  [RL RR] atraso=8
  ```
  Modo gráfico: `grafico=10|15|31` e os ganhos de cada ponto.
- Na config (`src/config.rs`, linhas como `snd_app`):
  - `eq_dev=<id do aparelho>|<perfil>`
  - `eq_app=<exe>|<perfil>|<quando>`, `quando` = `sempre` (só se o teste 2 der certo), `foco`,
    `tocando`.
- **Qual vale** (função pura `eq::active`, testada):
  1. Teste 2 deu certo: o perfil do programa vai no bloco daquele programa e vale **só no som
     dele**, ao mesmo tempo que os outros programas usam o do aparelho.
  2. Plano B: o perfil do aparelho troca enquanto o programa está em foco (ou tocando); programa com
     saída própria (rota da aba Programas) tem o perfil fixo naquela saída.
  3. Senão, o perfil do aparelho.

### 2.5 Canais 5.1 e 7.1

- **Detectar:** máscara de alto-falantes do aparelho → nomes na ordem dos bits (L, R, C, LFE, RL,
  RR, …, SL, SR). "48 kHz · 8 canais" no cartão. Estéreo: o cartão de canais some.
- **EQ por canal:** clicar numa caixa do desenho edita só ela; soma com "Todos".
- **Ganho e atraso** por canal, com "1 ms ≈ 34 cm".
- **Testar:** ruído rosa de 1 s só naquele canal.
- **Trocar 5.1↔7.1:** é do Windows (a Microsoft diz que programas só leem); botão abre o `mmsys.cpl`.
- Depois, se ela quiser: gerência de graves (corte nas caixas, graves para o LFE), estéreo → 5.1,
  troca de canais. O DSP comporta (matriz de mistura antes dos filtros).

### 2.6 Instalação, com consentimento

1. **Sem nada instalado nada muda.** A aba mostra o cartão de consentimento: o que faz, que entra
   no motor de áudio do Windows, que pede administrador uma vez, quais aparelhos, e os limites.
2. **Nada é baixado:** `prisma_apo.dll` vai no pacote do Prisma, em `Program Files\Prisma` (que o
   `audiodg` lê). Na versão portátil, fica sem equalizador e diz por quê.
3. **Uma cópia de administrador** (`prisma.exe --eq=install:<ids>`, como o `--hidhide=` do
   `main.rs`), com o pedido do Windows:
   - registra a classe COM e a chave de `AudioProcessingObjects`;
   - cria `C:\ProgramData\Prisma\eq\` com escrita para a conta dela e leitura para o serviço de áudio;
   - em cada aparelho escolhido: guarda os valores originais do `FxProperties` (em
     `ProgramData\Prisma\eq\original.txt` e no registro, para sobreviver se um sumir), põe o nosso
     CLSID nos lugares SFX/MFX e anota o CLSID original para encadear;
   - liga `DisableProtectedAudioDG` **só se** a decisão abaixo for essa, e anota o valor anterior;
   - reinicia o `Audiosrv` (ou pede reinício, conforme o teste 5).
4. **Daí em diante** o Prisma grava só o `estado.bin`, sem administrador.
5. **Marcar e desmarcar aparelhos** depois: a mesma cópia de administrador, com outra lista.
6. **Remover** (botão, com pergunta, e na desinstalação do Prisma pelo `uninstall_as_admin`): devolve
   cada `FxProperties` exatamente como estava, devolve `DisableProtectedAudioDG`, apaga a classe e
   a pasta, reinicia o áudio.
7. **Driver de áudio atualizado** apagou o nosso CLSID do aparelho: o cartão avisa "o equalizador
   saiu deste aparelho" com **Pôr de novo**.
8. **Emergência:** se o som sumir depois de instalar, o botão **Tirar de todos os aparelhos** e a
   linha de comando `prisma.exe --eq=remove` fazem o passo 6.

**Assinatura: decidida depois do teste 1 da seção 2.2** (decisão dela, 16/11/2026). Opções:
- (a) Assinar o `prisma_apo.dll` (certificado de assinatura de código, por exemplo Azure Trusted
  Signing, pago por mês). Se o teste 1 mostrar que o caminho protegido aceita assinatura comum, não
  precisa mexer em `DisableProtectedAudioDG`.
- (b) Não assinar e ligar `DisableProtectedAudioDG=1`, como o Equalizer APO. Grátis; afrouxa a
  proteção do áudio com DRM no PC todo (o cartão de consentimento diz isso).

### 2.7 A tela (aba Equalizador da página Som)

Ver `docs/mockup-equalizador.html`.
1. **Aparelho de saída** (padrão = saída padrão do Windows), estado ("EQ ativo" / "fora deste
   aparelho"), taxa e canais.
2. **Perfil do aparelho:** lista, Novo, Renomear, Duplicar, Importar (AutoEQ).
3. **Motor:** aparelhos com o equalizador, **Aparelhos…**, **Remover**.
4. **Curva:** 20 Hz a 20 kHz em escala log, ±15 dB; arrastar ponto (frequência e ganho), roda (Q),
   clique duplo cria banda; tabela de bandas embaixo; **Paramétrico/Gráfico** (10/15/31 pontos);
   pré-amplificação com aviso "pode distorcer"; **Comparar sem EQ**.
5. **Canais** (só com mais de 2): desenho das caixas, ganho, atraso, Testar, link para o Windows.
6. **Perfis por programa:** programa, quando (sempre / em foco / tocando), perfil; Adicionar pelo
   `picker` da aba Programas.

### 2.8 Testes

- `prisma-eq` (sem Windows): pico dá o ganho exato em Fc; prateleiras; soma de bandas; gráfico →
  filtros dentro de 0,5 dB dos pontos; impulso estável (sem NaN, sem explodir) com Q 0,1 e 10 em
  44,1 a 192 kHz; atraso exato em amostras; transição sem degrau; **nenhuma alocação** no processar
  (alocador que conta, nos testes).
- `prisma-apo` sem instalar: um teste cria a classe dentro do próprio processo, faz `LockForProcess`
  e passa um arquivo de som; compara com o `prisma-eq`. Estado com versão errada ou corrompido =
  som intacto.
- `sys/eq.rs`: perfis ida e volta, importar AutoEQ real, `active` (programa > foco > tocando >
  aparelho; programa que fecha devolve o do aparelho), `estado.bin` gerado.
- `sys/eqapo.rs` com `Host` falso: instalar e remover deixam o registro igual ao de antes; aparelho
  que já tinha APO do fabricante; segunda instalação não duplica; desinstalar devolve
  `DisableProtectedAudioDG` como estava.
- Caso `--test-eq=<pasta>` e foto `--test-page=volume-eq` (lida pelo subagente de fotos).
- No PC de teste: os testes da seção 2.2, latência (não pode somar atraso), CPU do `audiodg`,
  5.1/7.1 real com Testar, fone USB e Bluetooth, jogo com anti-cheat.

### 2.9 Docs e manuais (mesmo commit da mudança)

- `docs/MAPA.md`: seção 4.14 (Som), funções e linhas novas; os dois crates.
- `docs/guide/GUIA.md` e `GUIDE.md`, seção Som: aba Equalizador, perfis, por programa, canais,
  limites (modo exclusivo, melhorias de áudio), como remover e o botão de emergência.
- `README.md` e as 4 traduções: linha na lista de recursos.
- `CHANGELOG.md`; `res/*.tsv` nos 4 idiomas (agente `tradutor`).

### 2.10 Ordem

1. `prisma-eq` com os testes. **Feito (16/11/2026):** filtros PK/LSC/HSC/LPQ/HPQ, gráfico de
   10/15/31 bandas, ganho e atraso por canal, perfil em texto (lê AutoEQ), curva para desenhar.
2. APO mínimo e os testes da seção 2.2 no PC de teste. **Feito**, menos assinatura e HVCI.
3. `prisma-apo` completo, testado dentro do processo. **Em parte:** já aplica o perfil
   (`perfil-<guid>.txt` ou `perfil.txt` em `C:\ProgramData\Prisma\eq`), lido no `LockForProcess`.
   Falta: limitador, perfil por programa e o `estado.bin` (hoje o APO lê o texto do perfil direto).
   Ouvido nos fones HyperX em 16/11/2026: o perfil de exemplo (graves +8, agudos -10) muda o som.
   Lição: o perfil só vale quando o fluxo recomeça (fechar e abrir o programa), daí a troca ao vivo.
   Troca ao vivo feita (`prisma-apo/src/live.rs`): uma thread por fluxo olha o arquivo do perfil a
   cada 250 ms, monta a corrente nova e entrega por `try_lock`; o tempo real mistura velha e nova
   por 20 ms e devolve a velha para a thread liberar (nada aloca nem libera no tempo real).
4. `sys/eqapo.rs` (instalar, remover, emergência).
5. Aba Equalizador: curva, bandas, perfil do aparelho, consentimento.
6. Perfis por programa (`eq_tick`).
7. Canais 5.1/7.1 e Testar.
8. Docs, traduções, fotos.

Riscos: APO com defeito tira o som do aparelho (por isso o `catch_unwind`, a passagem direta e a
remoção de emergência); atualização do Windows mudar as regras de APO de terceiros; o teste 2 dar
negativo (fica o plano B, que já cobre jogo em foco e player de música).
