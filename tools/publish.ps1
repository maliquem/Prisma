# Fecha uma versão depois que o GitHub Actions cria o rascunho: compila aqui, assina com a chave desta
# conta e só então publica. A cópia do GitHub serve só para conferir que compila; a que vai para as
# pessoas sai deste PC, do mesmo commit da etiqueta.
#
#   tools/publish.ps1 -Version 1.2.3
param([Parameter(Mandatory)][string]$Version)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
$tag = "v$Version"
$gh = 'C:\Program Files\GitHub CLI\gh.exe'
Set-Location $root

# 1. A etiqueta do GitHub é a mesma daqui, e a pasta não tem mudança fora do commit.
$here = (git rev-parse "$tag^{commit}").Trim()
$there = ((git ls-remote origin "refs/tags/$tag^{}") -split '\s')[0]
if (-not $there) { $there = ((git ls-remote origin "refs/tags/$tag") -split '\s')[0] }
if ($here -ne $there) { throw "A etiqueta $tag no GitHub ($there) não é a daqui ($here)." }
if ((git rev-parse HEAD).Trim() -ne $here) { throw "Faça checkout de $tag antes de publicar." }
if (git status --porcelain --untracked-files=no) { throw 'Há mudanças fora do commit.' }

# 2. O rascunho existe: o GitHub Actions terminou.
$info = & $gh release view $tag --json isDraft | ConvertFrom-Json
if (-not $info.isDraft) { throw "$tag não está em rascunho." }

# 3. Compila e assina aqui.
$env:RUSTUP_HOME = 'K:\rust\rustup'; $env:CARGO_HOME = 'K:\rust\cargo'; $env:PATH = "K:\rust\cargo\bin;$env:PATH"
# A DLL de shaders sai também em 32 bits, para os jogos de 32 bits.
rustup target add i686-pc-windows-msvc
cargo test --locked
if ($LASTEXITCODE) { throw 'Os testes falharam.' }
cargo build --release --locked
if ($LASTEXITCODE) { throw 'A compilação falhou.' }
$out = Join-Path ([IO.Path]::GetTempPath()) "prisma-$tag"
Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue
New-Item -ItemType Directory $out | Out-Null
$exe = Join-Path $out 'Prisma.exe'
Copy-Item target\release\prisma.exe $exe
$hash = (Get-FileHash $exe -Algorithm SHA256).Hash.ToLower()
[IO.File]::WriteAllText("$exe.sha256", "$hash  Prisma.exe`n")
& "$PSScriptRoot\update-key.ps1" -Sign $exe -Version $Version
# O modo Injeção vai num zip à parte, assinado como o exe: o Prisma só põe no jogo a DLL conferida e da mesma versão.
$zip = Join-Path $out 'Prisma-injecao.zip'
# Cada arquivo leva a própria assinatura (.sig no zip): o Prisma confere de novo antes de cada injeção.
$inj = Join-Path $root 'target\release\injecao'
foreach ($f in 'prisma_shader_hook.dll', 'prisma_shader_hook32.dll', 'prisma_hook32.exe') {
    & "$PSScriptRoot\update-key.ps1" -Sign (Join-Path $inj $f) -Version $Version
}
Compress-Archive -Path "$inj\*" -DestinationPath $zip
$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
[IO.File]::WriteAllText("$zip.sha256", "$hash  Prisma-injecao.zip`n")
& "$PSScriptRoot\update-key.ps1" -Sign $zip -Version $Version

# 4. Troca os arquivos do rascunho e publica.
& $gh release upload $tag $exe "$exe.sha256" "$exe.sig" $zip "$zip.sha256" "$zip.sig" --clobber
if ($LASTEXITCODE) { throw 'O envio falhou. O rascunho continua sem publicar.' }
& $gh release edit $tag --draft=false
"Publicado $tag, assinado."
