<#
Diagnóstico do APO de teste (só lê). Rodar como administrador depois de tocar som.
Grava C:\ProgramData\Prisma\eq\diagnostico.txt
#>
$saida = 'C:\ProgramData\Prisma\eq\diagnostico.txt'
$desde = (Get-Date).AddHours(-3)
& {
    "== audiodg: a DLL está carregada?"
    Get-Process audiodg -ErrorAction SilentlyContinue | ForEach-Object {
        "audiodg pid $($_.Id) desde $($_.StartTime)"
        try { $_.Modules | Where-Object ModuleName -match 'prisma|apo|eapo' | ForEach-Object { "  módulo: $($_.FileName)" } }
        catch { "  módulos não legíveis: $($_.Exception.Message)" }
    }
    tasklist /m prisma_apo.dll
    "== integridade de código (bloqueios de DLL)"
    Get-WinEvent -LogName 'Microsoft-Windows-CodeIntegrity/Operational' -MaxEvents 500 -ErrorAction SilentlyContinue |
        Where-Object { $_.TimeCreated -gt $desde -and $_.Message -match 'prisma|audiodg' } |
        Select-Object -First 10 | ForEach-Object { "$($_.TimeCreated) [$($_.Id)] $($_.Message -replace '\s+',' ')" }
    "== erros do audiodg / serviço de áudio"
    Get-WinEvent -FilterHashtable @{ LogName = 'Application', 'System'; StartTime = $desde } -ErrorAction SilentlyContinue |
        Where-Object { $_.Message -match 'audiodg|prisma_apo|Audiosrv|AudioEndpointBuilder' } |
        Select-Object -First 15 | ForEach-Object { "$($_.TimeCreated) $($_.ProviderName) [$($_.Id)] $(($_.Message -replace '\s+',' ').Substring(0, [Math]::Min(300, ($_.Message -replace '\s+',' ').Length)))" }
    "== log em outro lugar?"
    Get-ChildItem 'C:\Windows\ServiceProfiles\LocalService\AppData\Local\Temp' -Filter '*.log' -ErrorAction SilentlyContinue |
        Where-Object LastWriteTime -gt $desde | ForEach-Object { "$($_.FullName) $($_.Length)" }
    $temp = 'C:\Windows\ServiceProfiles\LocalService\AppData\Local\Temp\prisma-apo.log'
    if (Test-Path $temp) { Copy-Item $temp 'C:\ProgramData\Prisma\eq\apo-temp.log' -Force; "copiado para C:\ProgramData\Prisma\eq\apo-temp.log" }
    "== integridade de memória (HVCI)"
    (Get-CimInstance Win32_DeviceGuard -Namespace root\Microsoft\Windows\DeviceGuard -ErrorAction SilentlyContinue).SecurityServicesRunning -join ','
    (Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity' -ErrorAction SilentlyContinue).Enabled
    "== permissões da pasta do log"
    icacls 'C:\ProgramData\Prisma\eq'
} *>&1 | Out-File $saida -Encoding utf8
Write-Host "Gravado em $saida"
