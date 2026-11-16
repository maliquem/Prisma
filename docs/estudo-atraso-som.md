# Estudo: diagnóstico e reparo do atraso de som

Pergunta: o Prisma consegue medir o atraso de cada saída de som, mostrar de onde ele vem e oferecer
cortes com consentimento e volta atrás? Caso concreto: soundbar "Beyond TV" com o Dolby Atmos for Home
Theater (Dolby Access) ligado, com falas fora de sincronia e estalos de vez em quando.

Medido em 16/11/2026, Windows 11 build 26300, só leitura, com a sonda `tools/som-atraso` (ver
apêndice). Nada no Windows foi mudado. Estudos ligados: `estudo-som-multicanal.md` (EDID e Dolby) e
`estudo-equalizador.md` (APO próprio).

## 1. Resposta curta

- **O que o PC acrescenta dá para medir por software, e aqui é pequeno: cerca de 37 ms em todas as
  saídas**, inclusive no Beyond TV com Dolby Atmos. São 22 ms de buffer do programa e cerca de 15 ms
  do motor de áudio e do driver.
- **O atraso grande do Dolby Atmos não aparece em nenhuma medida de software.** O relógio do aparelho
  marca o mesmo atraso com e sem Atmos. O tempo extra fica no codificador MAT/TrueHD depois do relógio
  e na soundbar, que decodifica. Os relatos falam em 200 a 250 ms. Esse atraso só pode ser medido com
  um teste físico: o teste de clarão e clique que já existe ou um microfone.
- Nenhum dos aparelhos daqui aceita o período curto do `IAudioClient3`: todos os drivers
  respondem com mínimo igual ao padrão, 10 ms. Logo, o corte "período mínimo do motor" não rende
  nada neste PC. Num driver que aceite, ele rende de 5 a 8 ms.
- Reparo real para o Beyond TV: desligar o Dolby Atmos for Home Theater e passar para LPCM, como no
  estudo multicanal, ou compensar o atraso no menu de sincronia da TV ou da soundbar. Nos fones,
  o DTS só acrescenta um bloco do APO.

## 2. De onde vem o atraso (cadeia)

```
programa → [buffer do programa] → motor (audiodg): APOs por fluxo → mistura → APOs por modo/aparelho
        → [período do motor] → driver (WaveRT/USB/BT) → [FIFO e transporte] → aparelho
        → [codec BT / decodificação Dolby / processamento da TV ou da soundbar] → alto-falante
```

| Origem | Faixa típica | Software vê? | Como |
|---|---|---|---|
| Buffer do programa | 3 a 100 ms (o comum fica perto de 20 ms) | Sim, mas só do próprio fluxo | `GetBufferSize`, `GetCurrentPadding` |
| Período do motor | 10 ms (padrão); 2,67 a 10 ms com `IAudioClient3` | Sim | `GetSharedModeEnginePeriod`, `GetCurrentSharedModeEnginePeriod` |
| Modo exclusivo | tira a mistura e os APOs; período até 3 ms | Sim | `GetDevicePeriod` (mínimo), `IsFormatSupported(EXCLUSIVE)` |
| Reamostragem | menos de 1 ms (o filtro do motor) | Sim (taxa da mistura ≠ taxa do aparelho) | `GetMixFormat` × `PKEY_AudioEngine_DeviceFormat` |
| APOs (DTS, Dolby, Sonic, Realtek…) | 0 a cerca de 20 ms (lookahead do limitador, convolução) | Lista sim; o atraso de cada um, não | `FxProperties` no registro; o APO conhece a própria latência (`IAudioProcessingObject::GetLatency`), mas ela só é consultada dentro do audiodg |
| Som espacial (Sonic, Atmos HP/HT, DTS:X) | Sonic e fones: poucos ms; **Atmos HT: codificador MAT + soundbar, centenas de ms** | Só o formato ativo | `SpatialAudioDeviceConfiguration`; o formato do aparelho vira bitstream (IEC 61937) |
| Driver e FIFO | 2 a 10 ms | Sim, em conjunto | `IAudioClock::GetPosition` com QPC (seção 3.2) |
| Bluetooth | SBC/AAC de 150 a 250 ms; aptX de 70 a 120 ms; LE Audio/LC3 de 30 a 60 ms | Não há API pública para o codec | Barramento (`BTHENUM` = clássico, `BTHLEDEVICE` = LE Audio) e tabela; o valor real só com teste físico |
| HDMI/eARC, TV, soundbar | de 0 a mais de 250 ms (modo de imagem, decodificação, upmix) | Quase nunca | EDID: campo `Audio_Latency` do VSDB HDMI, quando existe (seção 3.4) |

## 3. O que o Windows deixa medir por software

### 3.1 Leitura sem tocar nada (rápida, sem administrador)

Por aparelho ativo (`IMMDeviceEnumerator::EnumAudioEndpoints`):

- **`IAudioClient::GetDevicePeriod`**: dá o período padrão (motor compartilhado) e o mínimo (modo
  exclusivo).
- **`IAudioClient3::GetSharedModeEnginePeriod(mix)`**: dá o padrão, o passo, o mínimo e o máximo
  em quadros. Se o mínimo for igual ao padrão, o driver não aceita período curto. Isso acontece com
  drivers de fabricante (Realtek, NVIDIA HDMI) e com áudio USB e virtual; o driver próprio do Windows
  para HD Audio costuma aceitar.
- **`GetCurrentSharedModeEnginePeriod`**: período em uso agora. Outro programa pode já ter baixado.
- **`GetStreamLatency`** depois de `Initialize` (sem `Start`): em modo compartilhado **todos os drivers
  daqui devolvem 0**. Não serve como medida e não deve aparecer na tela.
- **Formato da mistura e formato do aparelho**: quando são diferentes, o Windows reamostra ou, no caso
  do bitstream, um codificador como o Dolby MAT entra no caminho.
- **Melhorias e APOs**: `MMDevices\Audio\Render\{guid}\FxProperties`. A chave `{1da5d803…},5` igual a 1
  desliga as melhorias. As chaves `{d04e05a6…},1/2/5/6/7/13/14/15` guardam os CLSIDs, e o nome de cada
  um fica em `HKCR\CLSID`.
- **Som espacial ativo** (`SpatialAudioDeviceConfiguration`, que o Prisma já lê em
  `sys/audio.rs:239`).
- **Barramento** (`PKEY_Device_EnumeratorName`): HDAUDIO, USB, BTHENUM, BTHLEDEVICE ou ROOT (virtual).

### 3.2 Fila real até o driver (toca silêncio por 2 s)

Abrir um fluxo compartilhado, escrever silêncio e, depois de 0,5 s, comparar os **quadros escritos**
com a **posição do relógio do aparelho** (`IAudioClock::GetPosition`, trazida até o instante atual pelo
QPC). A diferença é tudo o que está na fila entre o programa e o ponto em que o driver diz que toca:
programa, motor, APOs (os que avisam o atraso) e o buffer do driver. É a única medida de software que
soma de verdade. Não abrange o que vem depois do relógio: codec BT, codificador Dolby, TV e
soundbar. Não se ouve nada e o fluxo é fechado logo em seguida.

### 3.3 ETW (estalos e falhas do motor)

- O provedor `Microsoft-Windows-Audio` e o perfil de áudio do WPR registram as falhas (glitches) do
  audiodg: o período em que o motor não entregou a tempo, o processo e o APO. É o que explica os
  estalos. **Precisa de administrador**, porque abre uma sessão de rastreamento. Fica como botão
  "Gravar diagnóstico de estalos (administrador)", com pedido explícito, gravando 30 s em
  `%LOCALAPPDATA%\Prisma\diag\`.
- Sem administrador existe um sinal mais simples: capturar o **loopback** do aparelho por alguns
  segundos e contar os pacotes com `AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY`, que o motor marca quando
  falha. Isso não diz qual foi o APO, mas confirma "o PC está falhando" contra "o estalo vem de fora"
  (soundbar ou HDMI). O Prisma já tem a captura de loopback (`capture/sound.rs:46`).

### 3.4 EDID (o que a TV declara)

O VSDB HDMI 1.4 tem os campos `Video_Latency` e `Audio_Latency`, presentes quando o bit 7 do byte 8
está ligado. **O Beyond TV não declara** (byte de flags `0x21`), e a LG antiga também não (`0x20`).
Quando houver, o Prisma mostra o valor como "a TV diz que atrasa N ms". Como raramente vem
preenchido, fica como bônus.

## 4. O que só um teste físico mede

Tudo o que vem depois do relógio do driver: o codec Bluetooth, o codificador Dolby MAT, a
decodificação e o upmix da soundbar e o modo de imagem da TV (que atrasa o vídeo, não o som). Para o
Beyond TV, o atraso que incomoda está todo aí.

Dois jeitos:

1. **Clarão e clique (já existe)**, em `sys/avsync.rs`, no botão "Medir atraso" do cartão Som da
   página HDR. A pessoa ajusta até coincidir ou filma com o celular. Ele mede **som contra imagem**,
   que é exatamente a reclamação da falta de sincronia. Não precisa de microfone.
2. **Microfone ouvindo um pulso.** Tocar uma varredura curta (chirp de 50 ms) e capturar o
   microfone ao mesmo tempo, com os dois relógios em QPC. A correlação cruzada dá o atraso de ida e
   volta. Subtraindo o atraso de entrada do microfone (o período de captura, medido por software),
   sobra o atraso de saída com precisão de cerca de 1 ms. Repetir 5 vezes e usar a mediana.
   - **Vale oferecer**, mas como opção, com aviso de que vai tocar um bipe e ouvir o microfone por
     2 s, sem gravar nada em disco. Funciona para os alto-falantes e a soundbar. Para fones não
     funciona: o microfone não ouve, salvo encostando o fone nele. Microfone Bluetooth no mesmo
     fone acrescenta o próprio atraso e estraga a conta; a tela avisa e sugere outro microfone.
   - Mede **som absoluto**, e não som contra imagem. Para a sincronia labial, o clarão e clique continua
     sendo o teste certo; o microfone serve para comparar caminhos ("Atmos 230 ms, LPCM 40 ms") sem
     a pessoa precisar olhar.

## 5. Medidas deste PC

Os três aparelhos de uso, com o `--tocar`:

| Aparelho | Barramento | Mistura → aparelho | Motor (padrão/mín.) | Fila medida | Efeitos | Espacial |
|---|---|---|---|---|---|---|
| Beyond TV (NVIDIA HDMI) | HDAUDIO | float 8 ch 48 kHz → **bitstream MLP/TrueHD 8 ch 192 kHz** | 10 / 10 ms | 36,5 ms | melhorias desligadas (WM LFX/GFX não carregam) | **Dolby Atmos for Home Theater** |
| HyperX Cloud III Wireless | USB | 2 ch 96 kHz → PCM 24 bits 96 kHz (sem reamostrar) | 10 / 10 ms | 36,8 ms | **DTS_SFX_APO** por fluxo | desligado |
| Realtek Digital Output | HDAUDIO | 2 ch 48 kHz → PCM 16 bits 48 kHz | 10 / 10 ms | 37,6 ms | ligadas, sem APO | desligado |

Leitura:

- Mesmo PC, mesma fila (~37 ms) com e sem Atmos: o atraso do Dolby **não** está no motor do Windows.
- O DTS dos fones não aparece na fila. Ou ele não declara latência, ou ela é de um bloco só. O APO do
  Prisma, que roda depois dele, vê blocos de até 1056 quadros (11 ms), o que dá o limite de cima.
- Não existe reamostragem em nenhum dos caminhos.
- **Estalos com Atmos:** é provável que venham do codificador MAT em tempo real ou da troca de modo da
  soundbar, e não do período do motor. O teste do loopback (3.3) confirma de que lado está.

## 6. O que dá para cortar (sempre com licença e volta atrás)

Cada corte guarda o valor antigo em `%LOCALAPPDATA%\Prisma\som-atraso.json` antes de mudar, e o
botão "Desfazer" o restaura. Os que precisam de administrador dizem isso no botão e pedem a elevação
na hora.

| Corte | Quando oferecer | Ganho | Como | Admin | Volta |
|---|---|---|---|---|---|
| Desligar o som espacial | Atmos HT ligado e atraso físico alto, ou a pessoa reclama de sincronia | de 150 a 250 ms (medir antes e depois) | Não há API pública para trocar o formato; abrir **Configurações → Som → aparelho → Áudio espacial** já no lugar certo (`ms-settings:sound-properties`) e medir de novo quando ela voltar | não | ela religa no mesmo lugar |
| Desligar as melhorias de um aparelho | Há APO de terceiros (DTS, Sonic Studio, Realtek, Nahimic) e a pessoa não usa | 1 bloco, até ~10 ms; e menos estalos | `FxProperties\{1da5d803…},5 = 1` e reiniciar o aparelho (desligar e ligar pelo `IMMDevice`) ou o serviço `Audiosrv` | **sim** (HKLM) | valor antigo salvo |
| Casar a taxa da mistura com a do aparelho | mistura ≠ aparelho em PCM | < 1 ms, e menos CPU | "Formato padrão" está em `{f19f064d…},0`; escrever exige admin e reinício do aparelho. Preferir abrir `mmsys.cpl` na aba Avançado | sim, ou manual | valor antigo salvo |
| Período mínimo do motor | o driver aceita mínimo < padrão | de 5 a 8 ms | Não é uma configuração do Windows: vale enquanto algum programa mantém um fluxo aberto com `InitializeSharedAudioStream(mín.)`. O Prisma pode manter **um fluxo de silêncio** com o período mínimo enquanto houver jogo em primeiro plano. Se outro programa já tiver fixado um período, vale o dele: conferir com `GetCurrentSharedModeEnginePeriod` | não | parar o fluxo |
| Trocar driver de fabricante pelo do Windows (HD Audio) | o driver não aceita período curto e o atraso importa | de 5 a 8 ms | só como **instrução**: o Prisma não troca driver | — | — |
| Ajuste de sincronia na TV ou na soundbar | o teste físico mostra o atraso fora do PC | todo o restante | aviso com o valor medido: "atrase o vídeo ou adiante o som em N ms no menu da TV ou da soundbar (Sincronia de áudio, AV Sync, Lip Sync)" | — | ela mesma |

Notas:

- O fluxo de silêncio com período curto é o único corte "de verdade" que não mexe no sistema. Neste
  PC ele não rende nada (o driver não aceita), e a tela deve dizer isso em vez de oferecer o corte.
- Com o Atmos HT ligado, o Windows **não carrega** os APOs do aparelho (`Disable_SysFx = 1` neste
  aparelho), então o equalizador do Prisma também não age nele. É mais um motivo para o caminho LPCM do
  estudo multicanal.
- Bluetooth: só informar ("SBC/AAC ≈ 200 ms") e sugerir o modo de jogo do fone, se existir. O codec
  não é escolhido por API pública.

## 7. Plano para a página Som (sem página nova)

Uma seção **Atraso** dentro do cartão de cada saída (em `pages/volume.rs`, junto com a lista de
saídas), fechada por padrão:

1. **Abrir a seção** chama a leitura 3.1 e mostra, em linhas curtas:
   `Barramento · Mistura 8 ch 48 kHz → aparelho TrueHD 192 kHz · Motor 10 ms (não aceita menor) ·
   Efeitos: DTS · Espacial: Dolby Atmos HT`.
2. **Botão "Medir no PC"** (3.2): mostra "No PC: 37 ms".
3. **Botão "Medir com o microfone"** (seção 4), opcional e com aviso, ou o link "Medir som contra
   imagem", que abre o teste de clarão e clique que já existe. Resultado: "Fora do PC: ~190 ms (TV,
   soundbar ou Bluetooth)".
4. **Barra empilhada** com PC, espacial/codec e fora do PC. O que não foi medido fica hachurado com
   "estimado".
5. **Sugestões** (seção 6), uma por linha, cada uma com "Aplicar" e "Desfazer". Só aparecem as que
   se aplicam ao aparelho.
6. **"Verificar estalos"**: 10 s de loopback contando as descontinuidades (3.3, sem administrador). Se
   houver, aparece o botão do ETW com administrador.

Código:

- `sys/audio.rs`: `latency(id) -> Latency` com a leitura de 3.1, que move a sonda de
  `tools/som-atraso` para cá, e `measure_queue(id)` de 3.2. Reaproveita `endpoint_path` e
  `spatial_name`, que já existem.
- `sys/avsync.rs`: passa a aceitar o aparelho além do padrão. Ganha uma função de microfone (chirp,
  captura e correlação) quando a opção do microfone for aprovada.
- Testes: a análise do formato do aparelho (blob do registro), a correlação com um pulso sintético
  atrasado em N amostras e a tabela de sugestões (entrada: leitura; saída: lista de cortes).
- Manuais em inglês e português: seção "Atraso do som", no mesmo commit em que a tela entrar.

Ordem: (1) leitura e "Medir no PC"; (2) sugestões que não precisam de admin (som espacial, aviso de
sincronia, Bluetooth); (3) verificação de estalos; (4) microfone; (5) cortes com administrador.

## 8. Próximos testes (pedir licença antes de cada um)

1. Medir o Beyond TV com o clarão e clique (página HDR) **com** e **sem** o Atmos HT. Desligar o
   Atmos é ação dela, em Configurações. O número dá o tamanho real do corte.
2. "Verificar estalos" (loopback) com o Atmos ligado, durante os estalos: se o motor não marcar
   descontinuidade, o estalo vem do codificador Dolby ou da soundbar.
3. Com licença e administrador: um WPR de 30 s durante um estalo.

## Apêndice: a sonda

`tools/som-atraso` (Rust, crate à parte, `windows` 0.62 como o Prisma):

```
cd tools/som-atraso
cargo run --release            # só leitura
cargo run --release -- --tocar # + fila real, toca 2 s de silêncio em cada saída
```

Abre fluxos sem `Start` e lê o registro (HKLM e HKCR) sem escrever nada. Com `--tocar`, escreve
silêncio por 2 s em cada saída ativa. Teste: `cargo test`.
