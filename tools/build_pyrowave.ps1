# Recompila res\pyrowave\libpyrowave-shared-1.dll a partir do código do PyroWave (MIT), nos commits
# citados em res\pyrowave\LICENSE.txt. Pede git, CMake, Ninja e o Visual Studio Build Tools (C++).
#
#   tools/build_pyrowave.ps1                    # compila em %TEMP%\prisma-pyrowave e troca a DLL do repositório
#   tools/build_pyrowave.ps1 -Work K:\pasta     # outra pasta de trabalho
#   tools/build_pyrowave.ps1 -Out C:\teste.dll  # grava em outro lugar, sem mexer na do repositório
param(
    [string]$Work = (Join-Path ([IO.Path]::GetTempPath()) 'prisma-pyrowave'),
    [string]$Out = (Join-Path (Split-Path $PSScriptRoot) 'res\pyrowave\libpyrowave-shared-1.dll')
)
$ErrorActionPreference = 'Stop'
$pyrowave = '7fe0e2667cd3076b8b2bf04bcb80bc8d691b800b'
$granite = '53090429539bf3404250d18fe58db971f0907c04'

function Run-Git { & git.exe @args; if ($LASTEXITCODE) { throw "git $args falhou" } }

New-Item -ItemType Directory -Force $Work | Out-Null
$src = Join-Path $Work 'pyrowave'
if (-not (Test-Path $src)) { Run-Git clone https://github.com/Themaister/pyrowave $src }
Run-Git -C $src fetch origin
Run-Git -C $src checkout --quiet $pyrowave
$g = Join-Path $src 'Granite'
if (-not (Test-Path $g)) { Run-Git clone https://github.com/Themaister/Granite $g }
Run-Git -C $g fetch origin
Run-Git -C $g checkout --quiet $granite
foreach ($m in 'third_party/volk', 'third_party/khronos/vulkan-headers') {
    Run-Git -C $g submodule sync $m
    Run-Git -C $g submodule update --init $m
}

# O compilador C++ da Microsoft, com o runtime embutido: a DLL só depende do KERNEL32 e do USER32.
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw 'Visual Studio Build Tools com C++ não encontrado.' }
$build = Join-Path $Work 'build'
$cmd = "call `"$vs\VC\Auxiliary\Build\vcvars64.bat`" >nul && " +
    "cmake -S `"$src`" -B `"$build`" -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_C_COMPILER=cl -DCMAKE_CXX_COMPILER=cl -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded && " +
    "cmake --build `"$build`" --parallel 2 --target pyrowave-shared"
cmd /c $cmd
if ($LASTEXITCODE) { throw 'A compilação do PyroWave falhou.' }

$dll = Join-Path $build 'libpyrowave-shared-1.dll'
Copy-Item $dll $Out
"Gravada em ${Out}: SHA-256 $((Get-FileHash $dll -Algorithm SHA256).Hash.ToLower())"
