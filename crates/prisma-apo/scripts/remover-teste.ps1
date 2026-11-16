<#
Desfaz o instalar-teste.ps1 a partir das cópias em C:\ProgramData\Prisma\eq\backup. Rodar como administrador.
O log (apo.log) fica, para leitura.
#>
$ErrorActionPreference = 'Stop'
$clsid  = '{6F1E3A52-8C47-4D0B-9A3E-2B5C7D9E1F40}'
$backup = 'C:\ProgramData\Prisma\eq\backup'
$audio  = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Audio'
$render = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render'

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Rode como administrador.'
}

# FxProperties: o administrador só pode mudar valores (não apagar a chave). Desfaz só o que o instalar mudou;
# a cópia completa (fx-<id>.reg) fica no backup para conferência.
$fx       = '{d04e05a6-594b-4fb6-a80d-01af5eed7d1d}'
$modos    = '{d3993a3f-99c2-4402-b5ec-a92a0367664b}'
$aparelho = Join-Path $backup 'aparelho.txt'
$mudanca  = Join-Path $backup 'mudanca.txt'
if ((Test-Path $aparelho) -and (Test-Path $mudanca)) {
    $id = (Get-Content $aparelho).Trim()
    $m  = (Get-Content $mudanca).Trim()
    $sub = "SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render\$id\FxProperties"
    $rk = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey($sub, [Microsoft.Win32.RegistryKeyPermissionCheck]::ReadWriteSubTree,
        [System.Security.AccessControl.RegistryRights]'SetValue, QueryValues')
    # Tira o nosso das duas listas: a de efeitos (,13) e a de modos, onde a versão antiga do instalar errou.
    foreach ($nome in "$fx,13", "$modos,5") {
        $lista = $rk.GetValue($nome)
        if ($lista -and ($lista -contains $clsid)) {
            $rk.SetValue($nome, [string[]](@($lista) | Where-Object { $_ -ne $clsid }), [Microsoft.Win32.RegistryValueKind]::MultiString)
        }
    }
    if ($m -like 'antigo|*') {
        $antigo = $m.Substring('antigo|'.Length)
        if ($antigo) { $rk.SetValue("$fx,5", $antigo, [Microsoft.Win32.RegistryValueKind]::String) }
        else { $rk.DeleteValue("$fx,5", $false) }
    }
    $melhorias = Join-Path $backup 'melhorias.txt'
    if (Test-Path $melhorias) {
        $rk.SetValue('{1da5d803-d492-4edd-8c23-e0c0ffee7f0e},5', [int](Get-Content $melhorias).Trim(), [Microsoft.Win32.RegistryValueKind]::DWord)
        Remove-Item $melhorias
    }
    $rk.Close()
    Remove-Item $mudanca
    Write-Host "FxProperties de $id restaurado."
}

# Proteção do audiodg
$p = Join-Path $backup 'protecao.txt'
if (Test-Path $p) {
    $v = (Get-Content $p).Trim()
    if ($v -eq 'ausente') { Remove-ItemProperty $audio 'DisableProtectedAudioDG' -ErrorAction SilentlyContinue }
    else { Set-ItemProperty $audio 'DisableProtectedAudioDG' ([int]$v) }
    Remove-Item $p
    Write-Host 'DisableProtectedAudioDG restaurado.'
}

# Registro do APO e da classe COM
Remove-Item "HKLM:\SOFTWARE\Classes\AudioEngine\AudioProcessingObjects\$clsid" -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item "HKLM:\SOFTWARE\Classes\CLSID\$clsid" -Recurse -Force -ErrorAction SilentlyContinue

# AudioEndpointBuilder guarda os efeitos de cada aparelho; reiniciar só o Audiosrv não relê FxProperties.
Restart-Service AudioEndpointBuilder -Force
Start-Service audiosrv
# A DLL só sai depois que o audiodg reiniciado a soltou.
Remove-Item 'C:\Program Files\Prisma\apo-teste' -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item $aparelho -ErrorAction SilentlyContinue
Write-Host 'Removido.'
