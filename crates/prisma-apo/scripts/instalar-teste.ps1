<#
APO de teste do Prisma: instala num PC DE TESTE (ou máquina virtual). Nunca no PC do dia a dia.
Rodar como administrador:  .\instalar-teste.ps1 -Aparelho "Alto-falantes" [-SemProtecao]

Faz, guardando cópia de tudo em C:\ProgramData\Prisma\eq\backup:
  1. copia prisma_apo.dll para C:\Program Files\Prisma\apo-teste\
  2. registra a classe COM e o APO (HKLM\SOFTWARE\Classes)
  3. põe o APO no SFX do aparelho: no fim da lista do Windows 11 ({d3993a3f-...},5) se existir, senão no lugar antigo ,5
  4. com -SemProtecao, liga DisableProtectedAudioDG (o que o Equalizer APO faz); sem ele, testa a carga sem assinatura
  5. reinicia o serviço de áudio
Depois: tocar algo e ler C:\ProgramData\Prisma\eq\apo.log.  Desfazer: .\remover-teste.ps1
#>
param(
    [Parameter(Mandatory)] [string] $Aparelho,   # parte do nome do aparelho de saída
    [string] $Dll = '',   # vazio: a DLL ao lado do script (pacote) ou a do cargo build --release
    [switch] $SemProtecao
)
$ErrorActionPreference = 'Stop'

$clsid   = '{6F1E3A52-8C47-4D0B-9A3E-2B5C7D9E1F40}'
$destino = 'C:\Program Files\Prisma\apo-teste'
$dados   = 'C:\ProgramData\Prisma\eq'
$backup  = Join-Path $dados 'backup'
$fx      = '{d04e05a6-594b-4fb6-a80d-01af5eed7d1d}'   # lugares antigos: ,5 SFX ,6 MFX ,7 EFX
# Listas de efeitos (PKEY_CompositeFX_*): $fx,13 fluxo (SFX), ,14 modo (MFX), ,15 aparelho (EFX).
# Cuidado: {d3993a3f-...},5/6/7 são os MODOS suportados (ex.: {C18E2F7E-...} = modo padrão), não efeitos.
$composta = "$fx,13"
$render  = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render'
$audio   = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Audio'
$iidSfx2 = '{BAFE99D2-7436-44CE-9E0E-4D89AFBFFF56}'   # IAudioSystemEffects2

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Rode como administrador.'
}
if (-not $Dll) {
    $Dll = @((Join-Path $PSScriptRoot 'prisma_apo.dll'), (Join-Path $PSScriptRoot '..\target\release\prisma_apo.dll')) |
        Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $Dll -or -not (Test-Path $Dll)) { throw "DLL não encontrada: $Dll (compile com cargo build --release)" }

# Aparelho: nome amigável fica em Properties, {a45c254e-...},2
$achados = Get-ChildItem $render | Where-Object {
    $p = Get-ItemProperty (Join-Path $_.PSPath 'Properties') -ErrorAction SilentlyContinue
    (Get-ItemProperty $_.PSPath).DeviceState -eq 1 -and $p.'{a45c254e-df1c-4efd-8020-67d146a850e0},2' -like "*$Aparelho*"
}
if (@($achados).Count -ne 1) {
    throw "Achei $(@($achados).Count) aparelhos ativos com '$Aparelho' no nome. Seja mais específico."
}
$id = $achados.PSChildName
$nome = (Get-ItemProperty (Join-Path $achados.PSPath 'Properties')).'{a45c254e-df1c-4efd-8020-67d146a850e0},2'
Write-Host "Aparelho: $nome  $id"

New-Item -ItemType Directory -Force $destino, $backup | Out-Null
# O motor de áudio roda como LOCAL SERVICE: precisa escrever o log.
icacls $dados /grant '*S-1-5-19:(OI)(CI)M' | Out-Null

# 1. DLL
# Uma versão anterior pode estar carregada no audiodg: parar o áudio solta a DLL.
Stop-Service audiosrv -Force
Copy-Item $Dll (Join-Path $destino 'prisma_apo.dll') -Force
$caminho = Join-Path $destino 'prisma_apo.dll'

# 2. registro COM e APO
reg export "HKLM\SOFTWARE\Classes\CLSID\$clsid" (Join-Path $backup 'clsid.reg') /y 2>$null | Out-Null
$k = "HKLM:\SOFTWARE\Classes\CLSID\$clsid"
New-Item -Force "$k\InprocServer32" | Out-Null
Set-ItemProperty $k '(default)' 'Prisma EQ (teste)'
Set-ItemProperty "$k\InprocServer32" '(default)' $caminho
Set-ItemProperty "$k\InprocServer32" 'ThreadingModel' 'Both'

$a = "HKLM:\SOFTWARE\Classes\AudioEngine\AudioProcessingObjects\$clsid"
New-Item -Force $a | Out-Null
$valores = @{ FriendlyName = 'Prisma EQ (teste)'; Copyright = 'Prisma' }
foreach ($n in $valores.Keys) { New-ItemProperty $a $n -Value $valores[$n] -PropertyType String -Force | Out-Null }
$dwords = @{ MajorVersion = 0; MinorVersion = 1; Flags = 14; MinInputConnections = 1; MaxInputConnections = 1;
             MinOutputConnections = 1; MaxOutputConnections = 1; MaxInstances = -1; NumAPOInterfaces = 1 }
foreach ($n in $dwords.Keys) { New-ItemProperty $a $n -Value $dwords[$n] -PropertyType DWord -Force | Out-Null }
New-ItemProperty $a 'APOInterface0' -Value $iidSfx2 -PropertyType String -Force | Out-Null

# 3. lugar SFX do aparelho (teste 6: a chave é do TrustedInstaller/audiosrv; se negar, só relatar)
$fxKey = "HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render\$id\FxProperties"
$copiaFx = Join-Path $backup "fx-$($id.Trim('{}')).reg"
# Só a primeira instalação guarda cópia: rodar de novo não pode sobrescrever o original com o nosso.
if (-not (Test-Path $copiaFx)) { reg export $fxKey $copiaFx /y | Out-Null }
Set-Content (Join-Path $backup 'aparelho.txt') $id
$fxPs = "$render\$id\FxProperties"
$escrito = $false
try {
    # O provedor do PowerShell pede mais direitos do que o administrador tem nesta chave (só SetValue e
    # leitura). Abrir pelo .NET pedindo exatamente esses dois.
    $sub = "SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render\$id\FxProperties"
    $rk = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey($sub, [Microsoft.Win32.RegistryKeyPermissionCheck]::ReadWriteSubTree,
        [System.Security.AccessControl.RegistryRights]'SetValue, QueryValues')
    if (-not $rk) { throw "FxProperties não existe em $id" }
    # Lista de efeitos por fluxo (PKEY_CompositeFX_StreamEffectClsid); quando existe, vale ela.
    $lista = $rk.GetValue($composta)
    if ($lista) {
        if ($lista -notcontains $clsid) {
            $rk.SetValue($composta, [string[]](@($lista) + $clsid), [Microsoft.Win32.RegistryValueKind]::MultiString)
        }
        Set-Content (Join-Path $backup 'mudanca.txt') 'lista13'
        Write-Host "SFX: acrescentado no fim da lista ($(@($lista).Count) efeito(s) do aparelho continuam antes)."
    } else {
        $antigo = $rk.GetValue("$fx,5")
        $copiaM = Join-Path $backup 'mudanca.txt'
        if (-not (Test-Path $copiaM)) { Set-Content $copiaM "antigo|$antigo" }
        if ($antigo) { Write-Host "SFX: troquei $antigo pelo de teste (o do fabricante fica desligado até remover)." }
        $rk.SetValue("$fx,5", $clsid, [Microsoft.Win32.RegistryValueKind]::String)
    }
    # "Melhorias de áudio" desligadas (PKEY_AudioEndpoint_Disable_SysFx = 1): o Windows não carrega efeito nenhum.
    $sysfx = '{1da5d803-d492-4edd-8c23-e0c0ffee7f0e},5'
    $v = $rk.GetValue($sysfx)
    if ($v -eq 1) {
        $copiaS = Join-Path $backup 'melhorias.txt'
        if (-not (Test-Path $copiaS)) { Set-Content $copiaS '1' }
        $rk.SetValue($sysfx, 0, [Microsoft.Win32.RegistryValueKind]::DWord)
        Write-Host 'Melhorias de áudio estavam desligadas no aparelho: liguei (o remover desliga de novo).'
    }
    $rk.Close()
    $escrito = $true
} catch {
    Write-Warning "Não consegui escrever em FxProperties (teste 6: talvez precise tomar posse da chave). Nada mudado nela."
    Write-Warning $_.Exception.Message
}

# 4. proteção do audiodg
if ($SemProtecao) {
    $v = (Get-ItemProperty $audio -ErrorAction SilentlyContinue).DisableProtectedAudioDG
    $copiaP = Join-Path $backup 'protecao.txt'
    if (-not (Test-Path $copiaP)) { Set-Content $copiaP ($(if ($null -eq $v) { 'ausente' } else { $v })) }
    New-ItemProperty $audio 'DisableProtectedAudioDG' -Value 1 -PropertyType DWord -Force | Out-Null
    Write-Host 'DisableProtectedAudioDG = 1'
}

# 5. reinicia o áudio
# AudioEndpointBuilder guarda os efeitos de cada aparelho; reiniciar só o Audiosrv não relê FxProperties.
Restart-Service AudioEndpointBuilder -Force
Start-Service audiosrv
Start-Sleep 3
if ($escrito) {
    # O serviço de áudio pode reescrever FxProperties ao subir: conferir se o nosso ficou.
    $depois = Get-ItemProperty $fxPs -ErrorAction SilentlyContinue
    $ficou = (@($depois.$composta) -contains $clsid) -or ($depois."$fx,5" -eq $clsid)
    if ($ficou) { Write-Host 'Depois de reiniciar o áudio o APO continua no aparelho.' }
    else { Write-Warning 'Depois de reiniciar o áudio o APO SUMIU do aparelho: o Windows reescreveu FxProperties.' }
}
Get-ChildItem $dados -Filter apo.log -ErrorAction SilentlyContinue | ForEach-Object { Write-Host "Log já tem $($_.Length) bytes." }
Write-Host "Pronto. Toque um som e leia $dados\apo.log"
