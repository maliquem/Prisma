# Prisma

Utilitário leve para Windows 11, todo em Rust, sem framework de interface. Fica na bandeja e só carrega o que está em uso.

## Primeiro uso

1. Abra `prisma.exe`.
2. Em **Ajustes**, clique em **Reabrir como administrador**. Sem isso o Prisma não alcança jogos e programas abertos como administrador.
3. Marque **Iniciar com o Windows**. A partir daí ele abre sozinho, sem pedir confirmação.
4. Em **Widget**, escolha um dos quatro estilos.

## Atalhos padrão

Todos podem ser trocados no programa.

| Ação | Atalho |
| --- | --- |
| Fullscreenizer | Ctrl + Alt + F |
| Volume do programa em foco | Ctrl + teclas de volume e mudo |
| Widget | Ctrl + Alt + W |
| Otimizar | Ctrl + Alt + O |
| Foto da área | Ctrl + Alt + S |
| Foto da tela inteira | Ctrl + Alt + A |
| Gravar e parar | Ctrl + Alt + R |
| Salvar replay | Ctrl + Alt + B |

Na foto: Enter copia, Ctrl + S salva, Ctrl + Z desfaz e Esc cancela. Foto fixada: arraste para mover, botão direito fecha.

## Funções

- **Monitor:** CPU por núcleo, GPU, RAM e VRAM, com uso de cada programa.
- **Otimizar:** sugere o que fechar e sempre pergunta. Blacklist nunca fecha, whitelist sempre fecha. A detecção automática sugere nomes para as duas listas.
- **Energia:** detecta jogo ou carga alta e troca o plano. Volta ao repouso depois de 60 s sem carga.
- **Perfil por jogo:** lembra tela sem bordas e volume de cada programa.
- **Captura:** foto com anotações e vídeo em AV1, HEVC ou H.264. Com a tela em HDR, grava em SDR ou HDR10.
- **Replay:** em modo jogo, guarda os últimos segundos na memória.

## Limites

- Jogo em tela cheia exclusiva precisa estar em modo janela antes do Fullscreenizer.
- Sem temperatura e consumo da CPU, porque exigem driver de kernel.
- H.264 não tem 10 bits: gravar em HDR10 com ele usa HEVC.
- Conteúdo protegido, como Netflix, sai preto nas capturas.

## Compilar

```bash
cargo build --release
```

Precisa do Rust e das Build Tools do Visual Studio. Configuração em `%APPDATA%\Prisma`.
