# Estudo: todos os canais da soundbar sem o Dolby Access

Pergunta: o Prisma consegue fazer o Windows enxergar a soundbar Atmos (aparelho "Beyond TV": subwoofer,
alto-falantes para cima e laterais) como 5.1, 7.1 ou 7.1.4 sem o Dolby Access? Com o Dolby Access o
som atrasa (falas fora de sincronia) e às vezes estala; sem ele, o Windows mostra 2.0.

Leitura feita em 16/11/2026, só leitura, Windows 11 build 26300 (como, no apêndice). Nada foi
mudado.

## 1. Por que o Windows mostra 2.0

**É o EDID.** O PC está ligado na TV pela placa NVIDIA (o monitor ativo é `DISPLAY\XXX9615`, nome
"Beyond TV"), e a TV anuncia no bloco de áudio do EDID:

| Formato | Canais | Observação |
|---|---|---|
| LPCM | **2** (duas vezes) | 32 a 192 kHz, 16/20/24 bits |
| Dolby TrueHD (MLP) | 8 | com bit de Atmos |
| Dolby Digital Plus (E-AC-3) | 8 | com JOC (Atmos) |
| Dolby Digital (AC-3) | 6 | |
| DTS | 6 | |

Speaker Allocation `0x01 0x00 0x00`: só frente esquerda e direita.

Ou seja: **multicanal só em bitstream** (Dolby/DTS comprimido); som comum (LPCM) só em estéreo. O
driver HDMI da NVIDIA monta a lista do "Configurar alto-falantes" e do "Formato padrão" a partir disso,
por isso só aparece Estéreo. Os formatos Dolby com 8 canais e o bit de Atmos são o que a soundbar
anuncia pelo eARC e a TV repassa; o LPCM 2.0 é a TV dizendo que não repassa LPCM multicanal (ou que
não foi configurada para isso).

Comparação: o EDID antigo da mesma porta (uma TV LG, `GSMC0A0`) anunciava **LPCM 8 canais**. Com ela
o Windows ofereceria 7.1 sem nada.

Estado atual do "Beyond TV" (com o Dolby Access ativo):

- formato do aparelho (`PKEY_AudioEngine_DeviceFormat`): 8 canais, 192 kHz, subtipo IEC 61937
  **MLP/TrueHD** (bitstream de alta taxa, o que o "Dolby Atmos for home theater" manda);
- formato de mistura: float32, 8 canais, 48 kHz, máscara `0x63F` (7.1);
- Melhorias de áudio desligadas (`Disable_SysFx = 1`); efeito antigo da Microsoft na lista.

Então hoje o Windows já mistura em 7.1 e o Dolby codifica tudo em TrueHD/MAT em tempo real. É nesse
codificador (mais o decodificador da soundbar) que nasce o atraso. Há relatos de 200 a 250 ms com o
"Dolby Atmos for home theater" no Windows 11
([fórum da Dolby](https://professionalsupport.dolby.com/s/question/0D54u0000AKnIFyCQN/delay-in-windows-11-when-atmos-is-on?language=en_US),
[Sonos](https://en.community.sonos.com/home-theater-229129/atmos-audio-sync-issue-windows-11-6925935));
número não confirmado em fonte oficial. Sobre os estalos não achei relato.

## 2. Caminhos para liberar multicanal sem o Dolby Access

| Caminho | Dá quantos canais | Atraso que acrescenta | Risco |
|---|---|---|---|
| a. "Configurar alto-falantes" / formato padrão | nenhum a mais (lista vem do EDID) | 0 | nenhum, mas não resolve |
| b. Ajuste na TV (modo eARC/passthrough, "EDID" da porta HDMI) | até 7.1 LPCM, se a TV tiver | 0 | nenhum; depende do menu da TV |
| c. EDID personalizado (CRU): acrescentar LPCM 8 canais e Speaker Allocation 7.1 | 7.1 LPCM | 0 no PC | médio: se a TV não repassar LPCM multicanal pelo eARC, fica mudo ou só toca a frente. Desfaz com `reset-all.exe` |
| d. PC direto na entrada HDMI da soundbar (se ela tiver), TV na saída | 7.1 LPCM (o EDID da soundbar costuma anunciar) | 0 | baixo; só cabo. Pode limitar o vídeo (4K/120, VRR) ao que a soundbar repassa |
| e. Bitstream do próprio programa (Kodi, mpv, MPC + LAV em WASAPI exclusivo) | 7.1.4 Atmos de verdade, **só para filmes já codificados** | o da soundbar decodificar (o player compensa) | baixo; não serve para jogo nem para o som do Windows |
| f. Prisma codifica o som do PC em AC-3 5.1 em tempo real ("Dolby Digital Live" caseiro) | 5.1 | ~60 a 100 ms (quadro de 32 ms + buffers + decodificar) | alto: precisa de um driver de áudio virtual 5.1 assinado para capturar, e o aparelho fica em modo exclusivo |
| g. APO nosso que faz upmix 2.0 → multicanal | nenhum enquanto o aparelho for 2.0 | 0 a 10 ms | o APO só escreve nos canais que o aparelho tem; no modo Atmos o Windows não carrega APO (ver 2.2 do estudo do equalizador: forçar deixou mudo) |

Notas:

- **a.** O Windows só lê: o `mmsys.cpl` oferece as configurações que o driver anuncia, e o driver
  segue o EDID. Não há chave de registro que crie canais que o driver não aceita.
- **b e c** resolvem a mesma coisa: fazer o EDID dizer "LPCM 8 canais". Se a TV aceitar (b), não
  precisa de CRU. Se não houver opção, o CRU testa se a TV repassa mesmo assim: o eARC carrega até 8
  canais LPCM, mas é a TV que decide repassar. A soundbar, recebendo LPCM 7.1, usa o próprio upmix
  para os alto-falantes de cima (ver item 3).
- **b tem precedente:** na LG CX, com a saída de áudio em "Auto" o LPCM pelo eARC fica 2.0; em
  "Passthrough" sai 7.1 ([AVForums](https://www.avforums.com/threads/cx77-if-digital-sound-out-set-to-auto-i-get-atmos-on-native-tv-apps-but-earc-pcm-only-2-0-vs-if-set-to-passthrough-no-atmos-on-apps-but-pcm-7-1.2430817/)).
  Há relatos parecidos de Samsung 2020 anunciando só 2 canais.
- **c é incerto na NVIDIA:** há relato de RTX 3090 em que acrescentar LPCM 8 canais no CRU não mudou
  nada ([fórum do CRU](https://www.monitortests.com/forum/Thread-Custom-Resolution-Utility-CRU?page=754)),
  e a NVIDIA ignora o override se não sobrarem 2 bytes livres no bloco. Por isso b vem antes.
- **c** mexe no registro do monitor (override de EDID por `HKLM\...\Enum\DISPLAY\...`) e reinicia o
  driver de vídeo (`restart64.exe`). Precisa de administrador. Errar o bloco de vídeo pode tirar
  resoluções; por isso mexer **só** no bloco de áudio e guardar o EDID original exportado.
- **e** já funciona hoje sem o Dolby Access: o EDID anuncia TrueHD Atmos e DD+ JOC. É o melhor
  caminho para filmes, não é solução para o PC inteiro.
- **f** não vale o esforço: 5.1 com perda, atraso parecido com o do Dolby, e exige driver virtual (o
  Windows 11 não tem um embutido; os prontos, como o VB-Cable, são de terceiros). Codificador AC-3
  livre existe (ffmpeg); as patentes do AC-3 já caíram.
- **g** só faz sentido **depois** de b, c ou d: com o aparelho em 7.1 LPCM, um APO do Prisma pode
  fazer upmix de estéreo para 7.1 (matriz, como na seção 2.5 do estudo do equalizador) com atraso
  de um buffer.

## 3. "Atmos simulado" pelo Prisma: viável?

**Atmos de verdade, não.** Os alto-falantes para cima só recebem som específico por bitstream Atmos
(E-AC-3 JOC ou TrueHD/MAT com metadados de objetos). Não existe codificador livre disso que o
hardware aceite: o projeto [dolby-atmos-encoder](https://github.com/raress96/dolby-atmos-encoder)
monta JOC e metadados, mas aparelhos certificados recusam por falta da assinatura que só a licença
Dolby gera. O licenciado é o próprio Dolby Access. Pelo HDMI, LPCM vai no máximo a 8 canais: **7.1.4 em LPCM não
existe**.

O que dá para fazer, com limites:

1. **Soundbar faz o upmix (recomendado).** Com LPCM 7.1 chegando (caminho b, c ou d), a soundbar
   costuma aplicar o próprio upmixer (Dolby Surround / modo "virtualizador") e manda parte do som
   para cima. Atraso: o processamento interno dela, que já existe em qualquer modo. Não depende do
   Prisma.
2. **5.1.2 em LPCM** (8 canais com "frente-alto" esquerda/direita no lugar das laterais traseiras):
   o padrão HDMI (CTA-861) tem essa alocação, mas o driver da NVIDIA e a soundbar precisam aceitar.
   Especulativo; só um teste diz.
3. **APO do Prisma "espacializa"** (upmix de estéreo para 7.1 com ambiência nas laterais e graves
   no LFE): viável depois que o aparelho for 7.1, com atraso de um buffer (~10 ms). Não cria altura.
4. Em estéreo (como está hoje), virtualização feita pelo Prisma brigaria com a da soundbar. Não
   vale.

## Recomendação

1. **Primeiro, sem software:** ver no menu da TV se há opção de áudio da saída eARC em "passthrough"
   ou "automático/bitstream", e se a porta HDMI do PC tem modo de EDID ("HDMI ULTRA HD Deep Colour",
   "EDID 2.1" etc.). Ver se a soundbar tem entrada HDMI (caminho d).
2. **Depois, se ela permitir:** o teste do CRU (c), que é reversível e não acrescenta atraso.
3. Para filmes, usar bitstream do player (e), que já funciona.
4. O APO de upmix (g) só entra no plano do equalizador depois que o aparelho virar 7.1.
5. Se nada repassar LPCM multicanal: ficar com 2.0 + upmix da soundbar, ou com o Dolby Access e
   compensar o atraso (ajuste de sincronia na soundbar/TV ou no player).

**Atualização (16/11/2026):** a saída de áudio da TV já está em "Passthrough", e o EDID continua com
LPCM 2.0. O EDID do registro é de 15/11, 19:15 (última vez que o monitor chegou). Se o passthrough foi
ligado depois, falta a TV refazer a conexão; se já estava, esta TV não anuncia LPCM multicanal, e
sobram a entrada HDMI da soundbar (d) e o CRU (c). Releitura depois de reconectar a TV (16/11,
13:00): igual, LPCM 2.0. **Esta TV não anuncia LPCM multicanal nem em Passthrough.**

O caminho d fica descartado: a placa tem uma só saída HDMI, os adaptadores DisplayPort para HDMI que
ela testou deram problema, e o PC ligado na soundbar com a soundbar repassando para a TV também não
funcionou. Além disso, com a soundbar no meio, o som dos aplicativos da TV não chegaria nela. A
ligação fica PC → TV → eARC → soundbar. Sobram:

1. **CRU (c):** o único jeito de testar se a TV repassa LPCM 7.1 mesmo sem anunciar. Pode dar mudo;
   desfaz-se.
2. **Estéreo + upmix da própria soundbar:** sem atraso extra; a soundbar espalha o som (inclusive para
   cima, se tiver modo surround/upmix para PCM). Para o dia a dia, provavelmente o melhor.
3. **Bitstream nos filmes (e):** Atmos de verdade, sem o Dolby Access.
4. **Ficar com o Dolby Access e compensar o atraso** no ajuste de sincronia da soundbar ou do player.

**Teste do CRU, preparado (16/11/2026):** o monitor já tem um `EDID_OVERRIDE` do CRU, idêntico ao
EDID lido (nome "Beyond TV"). Em vez de baixar o CRU, `scripts/teste-edid-lpcm.ps1` muda só dois
bytes do bloco 1 desse override: o 1º descritor LPCM de 2 para 8 canais (`0x09 → 0x0F`, byte 21) e o
Speaker Allocation para 7.1 (`0x01 → 0x4F`, byte 40), e refaz o checksum. Guarda o original em
`C:\ProgramData\Prisma\edid\`; `-Desfazer` devolve. O efeito vale depois de reiniciar o PC. Conferido
em memória: o bloco é o esperado e o checksum fecha.

**Resultado (16/11/2026):** o Windows passou a oferecer 8 canais (o override funciona na NVIDIA), mas
no "Testar" do 7.1 só saíram os dois da frente. **A TV não repassa LPCM multicanal pelo eARC**, nem
em Passthrough. Script desfeito com `-Desfazer`.

**Conclusão com esta TV:** sem Dolby, multicanal pelo PC não é possível. Ficam:

- dia a dia: estéreo e o modo surround/upmix da própria soundbar (sem atraso extra);
- filmes: bitstream TrueHD/DD+ Atmos pelo player (já anunciado no EDID);
- quando quiser o Dolby Access: compensar o atraso no ajuste de sincronia da soundbar ou do player.

O APO de upmix do Prisma (caminho g) não tem onde entrar com esta ligação.

A TV é uma **TCL C845 (Google TV)**. Não há relato específico do modelo, mas há de outras TCL Google
TV mandando multicanal como PCM estéreo pelo eARC
([Plex](https://forums.plex.tv/t/plex-on-my-tcl-google-tv-most-multi-channel-audio-is-output-as-stereo/896255),
[Sonos, C655](https://en.community.sonos.com/home-theater-229129/tcl-c655-apple-tv-4k-sonos-beam-2-hdmi-earc-audio-issues-6934500)),
embora a TCL diga que o eARC dela leva 5.1/7.1 sem compressão. Nenhum firmware achado corrige isso.
Vale reler o EDID depois de cada atualização da TV.

## Próximos testes (pedir licença antes de cada um)

1. **Menu da TV e da soundbar** (ela olha; nada no PC). Depois, reler o EDID com o script do
   apêndice: se aparecer LPCM 8 canais, resolvido.
2. **CRU no bloco de áudio** (administrador):
   - exportar o EDID atual do "Beyond TV" (CRU → Export) e guardar em `C:\ProgramData\Prisma\edid\`;
   - acrescentar um Short Audio Descriptor LPCM 8 canais 32–192 kHz 16/20/24 bits e o Speaker
     Allocation de 7.1 (FL/FR, LFE, FC, RL/RR, RLC/RRC);
   - `restart64.exe`, desligar o Dolby Atmos para home theater, "Configurar alto-falantes" → 7.1;
   - testar canal por canal (o "Testar" do Windows). Mudo ou só a frente = a TV não repassa: rodar
     `reset-all.exe` + `restart64.exe` e voltar ao Dolby;
   - medir o atraso contra o Dolby (vídeo de teste de sincronia).
3. **5.1.2 em LPCM:** no CRU, Speaker Allocation com FLH/FRH; ver se o Windows aceita a máscara com
   `TOP_FRONT_LEFT/RIGHT` e se a soundbar toca em cima.
4. **Bitstream de filme** (mpv ou Kodi, WASAPI exclusivo, passthrough TrueHD/E-AC-3) com o Dolby
   Access desligado, para confirmar o 7.1.4 nos filmes.

## Apêndice: como o estado foi lido

Só leitura do registro:

- `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render\<id>\Properties`:
  `{f19f064d-082c-4e27-bc73-6882a1bb8e4c},0` (formato do aparelho, WAVEFORMATEXTENSIBLE),
  `{e4870e26-3cc5-4cd2-ba46-ca0a9a70ed04},0` (formato de mistura),
  `{1da5d803-d492-4edd-8c23-e0c0ffee7f0e},3` (alto-falantes físicos; ausente = não configurado).
  O id do "Beyond TV" é `{4defefca-800a-4232-b78d-bbb7339ce751}`.
- `HKLM\SYSTEM\CurrentControlSet\Enum\DISPLAY\<monitor>\<instância>\Device Parameters\EDID`: bloco
  CTA-861 (tag 2), Audio Data Block (tag 1: 3 bytes por formato; bits 6–3 = código, 2–0 = canais − 1;
  terceiro byte bit 0 = Atmos em E-AC-3 e TrueHD) e Speaker Allocation (tag 4).

O Prisma pode mostrar isso na página Som ("o aparelho aceita LPCM até N canais; multicanal só em
Dolby/DTS") sem mudar nada, o que já explica o 2.0 para quem vê.
