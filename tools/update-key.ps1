# Chave que assina as atualizações do Prisma. A parte secreta nunca vai para o git nem para o GitHub:
# fica em %LOCALAPPDATA%\PrismaRelease, cifrada pelo Windows para esta conta. Quem entrar na conta do
# GitHub não consegue publicar uma versão que o Prisma aceite.
#
#   -New                 cria a chave e grava a parte pública em res/update-key.pub
#   -Backup <arquivo>    cópia protegida por senha, para guardar fora do PC (pen drive)
#   -Restore <arquivo>   volta a cópia num Windows novo
#   -Sign <exe> -Version 1.2.3   grava <exe>.sig
param(
    [switch]$New,
    [string]$Backup,
    [string]$Restore,
    [string]$Sign,
    [string]$Version
)
$ErrorActionPreference = 'Stop'
$Crypto = [System.Security.Cryptography.ECDsa]
$Store = Join-Path $env:LOCALAPPDATA 'PrismaRelease'
$KeyFile = Join-Path $Store 'update-key.bin'
$PubFile = Join-Path $PSScriptRoot '..\res\update-key.pub'

function Public-Bytes($ecdsa) {
    $q = $ecdsa.ExportParameters($false).Q
    [byte[]]($q.X + $q.Y)
}

function Save-Key($ecdsa) {
    New-Item -ItemType Directory -Force $Store | Out-Null
    $blob = [System.Security.Cryptography.ProtectedData]::Protect($ecdsa.ExportPkcs8PrivateKey(), $null, 'CurrentUser')
    [IO.File]::WriteAllBytes($KeyFile, $blob)
}

function Load-Key {
    if (-not (Test-Path $KeyFile)) { throw "Sem chave em $KeyFile. Use -Restore com a cópia de segurança." }
    $raw = [System.Security.Cryptography.ProtectedData]::Unprotect([IO.File]::ReadAllBytes($KeyFile), $null, 'CurrentUser')
    $e = $Crypto::Create()
    $e.ImportPkcs8PrivateKey($raw, [ref]0)
    $pub = [IO.File]::ReadAllBytes($PubFile)
    if ([Convert]::ToBase64String((Public-Bytes $e)) -ne [Convert]::ToBase64String($pub)) {
        throw "A chave desta conta não é a de res/update-key.pub."
    }
    $e
}

function Ask-Password($twice) {
    $a = Read-Host -AsSecureString 'Senha da cópia de segurança'
    if ($twice) {
        $b = Read-Host -AsSecureString 'Repita a senha'
        if ((ConvertFrom-SecureString $a -AsPlainText) -ne (ConvertFrom-SecureString $b -AsPlainText)) { throw 'As senhas não batem.' }
    }
    ConvertFrom-SecureString $a -AsPlainText
}

if ($New) {
    if ((Test-Path $KeyFile) -or (Test-Path $PubFile)) { throw 'Já existe uma chave. Trocar a chave impede quem tem a versão atual de atualizar.' }
    $e = $Crypto::Create([System.Security.Cryptography.ECCurve+NamedCurves]::nistP256)
    Save-Key $e
    [IO.File]::WriteAllBytes($PubFile, (Public-Bytes $e))
    "Chave criada em $KeyFile"
    "Parte pública em res/update-key.pub. Faça já uma cópia de segurança com -Backup."
}
elseif ($Backup) {
    $e = Load-Key
    $pbe = [System.Security.Cryptography.PbeParameters]::new('Aes256Cbc', [System.Security.Cryptography.HashAlgorithmName]::SHA256, 600000)
    [IO.File]::WriteAllBytes($Backup, $e.ExportEncryptedPkcs8PrivateKey((Ask-Password $true), $pbe))
    "Cópia gravada em $Backup. Guarde fora deste PC e não suba para o GitHub."
}
elseif ($Restore) {
    $e = $Crypto::Create()
    $e.ImportEncryptedPkcs8PrivateKey((Ask-Password $false), [IO.File]::ReadAllBytes($Restore), [ref]0)
    $pub = [IO.File]::ReadAllBytes($PubFile)
    if ([Convert]::ToBase64String((Public-Bytes $e)) -ne [Convert]::ToBase64String($pub)) { throw 'Essa cópia não é a chave de res/update-key.pub.' }
    Save-Key $e
    "Chave de volta em $KeyFile"
}
elseif ($Sign) {
    if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw 'Informe -Version no formato 1.2.3.' }
    $e = Load-Key
    $hash = (Get-FileHash $Sign -Algorithm SHA256).Hash.ToLower()
    # Mesmo texto que src/sys/update.rs confere: a assinatura vale para este arquivo e esta versão.
    $message = [Text.Encoding]::UTF8.GetBytes("Prisma $Version`n$hash")
    $sig = $e.SignData($message, [System.Security.Cryptography.HashAlgorithmName]::SHA256, [System.Security.Cryptography.DSASignatureFormat]::IeeeP1363FixedFieldConcatenation)
    [IO.File]::WriteAllBytes("$Sign.sig", $sig)
    "Assinado: $Sign.sig"
}
else {
    Get-Content $PSCommandPath -TotalCount 9 | Select-Object -Skip 3
}
