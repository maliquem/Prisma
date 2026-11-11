# PyroWave no Compartilhar tela: medições

Protótipo no ramo local `pyrowave`, 2026-11-10. Máquina: RTX 4070 SUPER, 1920×1080 a 60 quadros, 600 quadros (10 s) por rodada.
Medido com `prisma.exe --test-wave=saida.txt|10|docs\guide\welcome-pt.png|formato` (só build de teste; formato 420, 444, hdr ou hdr444), que faz codificação e decodificação no mesmo PC, sem rede.
A imagem é a página de boas-vindas, uma faixa de 1920×2160 rolando 7 px por quadro: muito texto, todo quadro muda.

## Resumo

- **Latência do codec:** o PyroWave não é mais rápido que o NVENC nesta máquina. Mediana de 3,9 ms, do quadro no D3D11 até a imagem desenhada, contra 2,9 ms do AV1 e 3,2 ms do HEVC (estes sem o desenho, que leva menos de 0,2 ms).
- **Tempo no fio:** 448 KB por quadro levam uns 3,6 ms a 1 Gbit/s e 1,4 ms a 2,5 Gbit/s; o AV1 manda 27 KB, uns 0,2 ms. Numa rede de 1 Gbit/s, o PyroWave fica uns 4 a 5 ms atrás do NVENC de ponta a ponta.
- **O que ele ganha:** imagem quase sem perda (43,9 dB de PSNR, no RGB, contra a original), quadros que não dependem uns dos outros, e perda de pacote que custa só aquele quadro, sem pedir quadro-chave nem esperar a renovação gradual.
- **A ponte D3D11↔Vulkan** não copia nada: as texturas e a fence são as mesmas na memória da GPU. Sincronizar pela fence compartilhada foi o jeito mais estável; esperar na CPU teve picos de 22 ms no p99.
- **Recomendação:** manter como opção experimental para rede local cabeada, onde a qualidade e a resistência à perda valem os 200 Mbit/s. Para menor atraso, o NVENC continua melhor.

## Como ficou

- **Host:** captura no D3D11 (como hoje), cópia para uma textura `SHARED_NTHANDLE` importada no Vulkan do PyroWave (`pyrowave_image_create`, D3D11_TEXTURE_BIT), e `pyrowave_encoder_encode_gpu_scaled` faz a escala e o BGRA→YCbCr 4:2:0 ou 4:4:4, BT.709 de faixa cheia. No HDR10, a textura é R10G10B10A2 em PQ e BT.2020 (a mesma que o NVENC HDR10 recebe), e o PyroWave só passa para o YCbCr do BT.2020 em planos de 16 bits, sem mapear tons. Uma `ID3D11Fence` compartilhada vira semáforo timeline do Vulkan: o D3D11 sinaliza, o Vulkan espera, o Vulkan sinaliza, o D3D11 espera. O Vulkan é criado no mesmo adaptador do D3D11 (pelo LUID).
- **Pacotes:** o PyroWave parte o quadro em blocos de até o tamanho de um datagrama. Um bloco maior que o datagrama (uns 4 por quadro) segue em pedaços com a marca nova `more` no `Msg::Video` (o byte 5, que era reservado em 0). Os pacotes das 2 bandas mais baixas, as críticas, vão duas vezes (uns 26 por quadro).
- **Quem assiste:** decodifica no Vulkan em três texturas compartilhadas (Y, Cb, Cr), R8 no SDR e R16 no HDR10, com os planos de cor no tamanho cheio no 4:4:4. Um pixel shader D3D11 converte para RGB (BT.709, ou BT.2020 ainda em PQ no HDR10) e desenha no swapchain que já existe, que passa para HDR10 como no NVENC.
  Desvio deliberado do pedido de apresentar em Vulkan: o swapchain D3D11 já tem o medidor, o menu da conexão, o HDR, a tela cheia e o VSync. Refazer tudo em Vulkan seria outro programa, e o desenho em D3D11 leva 0,16 ms. O Vulkan entra só para o codec, como pedido.
  Quadro incompleto vai para o decodificador como está quando chega um mais novo; o PyroWave aceita blocos vazios (`decode_is_ready_with_sideband` com mínimo de 0,8).
- **Protocolo:** codec id 3 = PyroWave; bit `WAVE = 8` em `Hello.codecs`, que só quem tem a DLL liga. Versões antigas nunca ligam o bit e nunca recebem PyroWave. O formato vai nos campos `hdr10` e `yuv444` do `Msg::Info`, como no NVENC.
- **HDR10 e 4:4:4:** HDR10 nas mesmas condições do NVENC (as duas telas em HDR e a opção de 10 bits do host). 4:4:4 quando todos os que assistem pedem e o host aceita; o pedido vai na marca nova `ASKS_444 = 16` do `Hello.flags`, porque `DECODES_444` continua querendo dizer "decodifica HEVC 4:4:4" (NVDEC). Por isso o Pedir 4:4:4 da aba Cliente e do menu da conexão aparece também em placas sem NVDEC quando a DLL está lá. Com o 4:4:4, o automático leva 1,5 vez a banda, como no NVENC.
- **Só rede local ou VPN:** `Link::near()`. No UDP direto, IP local ou CGNAT (Tailscale); no iroh, o caminho escolhido tem que ser IP local ou de VPN, nunca relé. O host confere a cada segundo; se alguém deixar de caber (ou passar a caber), a sala recomeça no automático (ou no PyroWave). Quem chega pelo relé faz a sala recomeçar no automático.
- **Página:** "PyroWave" na lista de codecs da aba Host, com aviso em laranja da banda e do limite de rede, e outro aviso se a DLL falta.
- **Limite de banda:** com o PyroWave escolhido, a lista vai até 700 Mbit/s (200, 300, 500, 700 a mais); com os outros codecs, continua até 150, e voltar para eles traz o limite para 150. O host também corta em 150 o que vai para o NVENC. Com o limite em automático, o PyroWave usa 200 Mbit/s em 1080p60, proporcional aos pixels por segundo, até 700. Um limite fixo abaixo disso vale também para o PyroWave e baixa a qualidade.
- **Biblioteca:** `libpyrowave-shared-1.dll` fica em `res/pyrowave` e vai dentro do `Prisma.exe` (`include_bytes!`), coberta pela assinatura da atualização. Na primeira vez que o codec é usado, ela é gravada em `%LOCALAPPDATA%\Prisma\pyrowave` com o `LICENSE.txt` ao lado e carregada por `LoadLibraryW` (ou a de `PRISMA_PYROWAVE`, para testar outra). O nome leva um hash do conteúdo, então uma versão nova não esbarra na velha aberta por outro Prisma. Um arquivo diferente do embutido é gravado de novo antes de carregar. No serviço, a pasta é a do SYSTEM, que só ele e os administradores alteram.
  - **Licença:** tudo o que entra na DLL é MIT: PyroWave e Granite (Hans-Kristian Arntzen), volk (Arseny Kapoulkine), o cabeçalho do RenderDoc (Baldur Karlsson) e os cabeçalhos do Vulkan (Khronos, Apache-2.0 ou MIT, usados como MIT). Distribuir pede só levar os avisos, que estão em `res/pyrowave/LICENSE.txt` e saem junto com a DLL. Nada de GPL.
  - **Como foi feita:** `tools/build_pyrowave.ps1` compila sem mudanças o PyroWave 7fe0e26 com o Granite 5309042 (MSVC, runtime embutido, só KERNEL32 e USER32), uns 1,1 MB.

## Medições

PyroWave a 200 Mbit/s (o automático: 200 Mbit/s em 1080p60, proporcional aos pixels por segundo, teto de 700): 407 KB e 464 datagramas por quadro, 215,1 Mbit/s na rede com cabeçalhos e críticos em dobro.

| Rodada | Codificar | Decodificar | Desenhar | Total sem rede (mediana / p95 / p99) | PSNR mediana (pior) | Mostrados |
|---|---|---|---|---|---|---|
| Fence, ritmo de 60 (como na transmissão) | 2,23 | 1,57 | 0,16 | 3,94 / 6,95 / 8,46 | 43,9 (43,0) | 600/600 |
| Fence, sem pausa entre quadros | 0,86 | 0,73 | 0,09 | 1,66 / 4,05 / 6,43 | 43,9 (43,0) | 600/600 |
| CPU espera o D3D11, ritmo de 60 | 1,39 | 1,03 | 0,13 | 2,54 / 5,30 / 22,39 | 43,9 (43,0) | 600/600 |
| Fence, 1 datagrama em 50 perdido | 3,03 | 1,98 | 0,19 | 5,28 / 9,34 / 15,98 | 36,2 (32,8) | 600/600 |
| Fence, 1 datagrama em 10 perdido | 1,54 | 1,02 | 0,14 | 2,73 / 8,81 / 15,01 | 29,4 (27,4) | 590/600 |

Tempos em ms, medianas, medidos na CPU (do pedido até o resultado pronto).
"Codificar" vai do quadro no D3D11 até os pacotes na CPU; "decodificar", dos pacotes até os planos prontos no D3D11.

Tempo de GPU do PyroWave por quadro: no host, escala 0,14 ms, DWT 0,21, quantização 0,42, análise 0,07, empacotamento 0,14 (uns 1,0 ms); em quem assiste, dequantização 0,17 e DWT inversa 0,14 (uns 0,3 ms). Memória de vídeo: uns 55 MB no host e 31 MB em quem assiste.

Abrir: 238 ms no host e 118 ms em quem assiste, quase tudo criando o Vulkan. Acontece a cada início de vídeo, não a cada quadro.

NVENC atual, mesma imagem, decodificando pelo Media Foundation como na transmissão:

| Codec | Banda de fato | Por quadro | Codificar | Decodificar | Total sem rede (mediana / p95 / p99) |
|---|---|---|---|---|---|
| AV1 | 13,2 Mbit/s | 27 KB | 2,49 | 0,39 | 2,92 / 6,79 / 14,34 |
| HEVC | 5,6 Mbit/s | 11 KB | 2,68 | 0,48 | 3,20 / 6,19 / 7,18 |

O codificar do NVENC inclui a conversão para NV12. O PSNR do NVENC não foi medido.

### Tempo no fio, calculado

| Rede | PyroWave (448 KB) | AV1 (27 KB) |
|---|---|---|
| 1 Gbit/s | 3,6 ms | 0,2 ms |
| 2,5 Gbit/s | 1,4 ms | 0,1 ms |
| Wi-Fi 6 real, uns 600 Mbit/s | 6,0 ms | 0,4 ms |

A VPN pela internet raramente tem 200 Mbit/s de subida; na prática o PyroWave é para a rede local cabeada.

## HDR10 e 4:4:4

Mesma máquina e mesma imagem, 300 quadros (5 s) por formato, ponte pela fence no ritmo de 60 quadros. No HDR10, os valores de 8 bits da imagem viram códigos PQ de 10 bits (a imagem fica escura, mas a medida vale igual) e o PSNR é calculado em 10 bits; ele só mostra que a ida e a volta pela matriz do BT.2020 estão certas, e não se compara com o do SDR.

| Formato | Automático | Por quadro | Total sem rede (mediana / p95 / p99) | PSNR mediana (pior) |
|---|---|---|---|---|
| 4:2:0 | 200 Mbit/s | 407 KB, 464 datagramas | 2,91 / 3,75 / 4,37 | 44,0 (43,3) |
| 4:4:4 | 300 Mbit/s | 610 KB, 689 datagramas | 2,75 / 3,83 / 4,28 | 50,5 (49,9) |
| 4:2:0 HDR10 | 200 Mbit/s | 407 KB, 463 datagramas | 2,43 / 3,75 / 4,00 | 45,2 (44,5), em 10 bits |
| 4:4:4 HDR10 | 300 Mbit/s | 610 KB, 690 datagramas | 3,07 / 4,38 / 4,90 | 54,6 (52,8), em 10 bits |

O 4:4:4 ganha uns 6,5 dB no texto pelo mesmo atraso. O custo está na rede: 319 Mbit/s com cabeçalhos, uns 5,3 ms por quadro numa rede de 1 Gbit/s.

## Variação entre rodadas

As medianas mudam até 1 ms entre rodadas iguais (a rodada com perda de 1/50 teve codificar mais lento que a sem perda, e a codificação não depende da perda). Em rodadas anteriores apareceram atrasos de uns 15 ms, um quadro de 60 Hz, tanto no PyroWave quanto na decodificação do Media Foundation. É o agendamento e o clock da GPU (ela estava a 210 MHz antes de começar), não a ponte. Por isso as tabelas usam medianas, com o p95 e o p99 ao lado.

## Riscos

- A API C do PyroWave é declarada instável; a DLL é conferida pela versão maior 1 ao abrir.
- A DLL no repositório é binária; atualizar o PyroWave é rodar `tools/build_pyrowave.ps1` com os commits novos e trocar os commits citados no `LICENSE.txt`.
- O Hello não renegocia: se o caminho do iroh passar de direto para relé no meio, o host percebe em até 1 s e recomeça no automático.
- O HDR10 passa sem mapear tons: quem assiste vê o PQ como o host mandou, e a tela de quem assiste faz o resto (como no NVENC HDR10).
- O 4:4:4 a 300 Mbit/s numa rede de 1 Gbit/s deixa menos folga; com o limite de banda fixo abaixo disso, a qualidade cai.
- 200 Mbit/s numa rede doméstica podem atrapalhar outras coisas na mesma rede.

## O que falta

- Medir de ponta a ponta pela rede, com host e quem assiste em PCs diferentes. Precisa iniciar o host, o que só acontece quando a usuária pedir.
- Medir em outra GPU (AMD e Intel) e em Wi-Fi.
