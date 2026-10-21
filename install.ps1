[CmdletBinding()]
param(
    [string]$Version = "",
    [string]$InstallDir = "",
    [switch]$System,
    [string]$LocalZip = "",
    [switch]$Force,
    [switch]$NoPath,
    [switch]$Uninstall,
    [switch]$Purge
)

$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Repo = 'jackielnikkie/another-evrax-agents'
$ExeName = 'evrax.exe'

function Fail([string]$Message) {
    Write-Host "evrax install: $Message" -ForegroundColor Red
    exit 1
}

function Test-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($id)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Assert-NotRunning {
    $proc = Get-Process -Name 'evrax' -ErrorAction SilentlyContinue
    if ($null -ne $proc) {
        Fail "evrax sedang berjalan (PID $($proc.Id -join ', ')). Tutup dulu (Ctrl+C), lalu ulangi."
    }
}

function Get-PathScope {
    if ($System) { return 'Machine' }
    return 'User'
}

function Add-PathEntry([string]$Dir) {
    if ($NoPath) { return }
    $scope = Get-PathScope
    if ($scope -eq 'Machine' -and -not (Test-Admin)) {
        Fail '-System butuh PowerShell sebagai Administrator (atau pakai -NoPath).'
    }
    $current = [Environment]::GetEnvironmentVariable('Path', $scope)
    if ($null -eq $current) { $current = '' }
    $entries = @($current -split ';' | Where-Object { $_ -ne '' })
    $exists = $false
    foreach ($entry in $entries) {
        if ($entry.TrimEnd('\') -ieq $Dir.TrimEnd('\')) { $exists = $true }
    }
    if (-not $exists) {
        $entries += $Dir
        [Environment]::SetEnvironmentVariable('Path', ($entries -join ';'), $scope)
        Write-Host "PATH ($scope) += $Dir"
    }
}

function Remove-PathEntry([string]$Dir) {
    foreach ($scope in @('User', 'Machine')) {
        if ($scope -eq 'Machine' -and -not (Test-Admin)) { continue }
        $current = [Environment]::GetEnvironmentVariable('Path', $scope)
        if ($null -eq $current) { continue }
        $entries = @($current -split ';' | Where-Object { $_ -ne '' -and $_.TrimEnd('\') -ine $Dir.TrimEnd('\') })
        $updated = $entries -join ';'
        if ($updated -ne $current) {
            [Environment]::SetEnvironmentVariable('Path', $updated, $scope)
            Write-Host "PATH ($scope) -= $Dir"
        }
    }
}

if (-not $InstallDir) {
    if ($System) {
        $InstallDir = Join-Path $env:ProgramFiles 'evrax\bin'
    } else {
        $InstallDir = Join-Path $env:USERPROFILE '.evrax\bin'
    }
}

if ($Uninstall) {
    Assert-NotRunning
    $exe = Join-Path $InstallDir $ExeName
    if (Test-Path -LiteralPath $exe) {
        Remove-Item -LiteralPath $exe -Force
        Write-Host "dihapus: $exe"
    } else {
        Write-Host "tidak ada binary di $exe"
    }
    Remove-PathEntry $InstallDir
    if ((Test-Path -LiteralPath $InstallDir) -and -not (Get-ChildItem -LiteralPath $InstallDir -Force)) {
        Remove-Item -LiteralPath $InstallDir -Force
    }
    if ($Purge) {
        foreach ($dir in @((Join-Path $env:APPDATA 'evrax'), (Join-Path $env:LOCALAPPDATA 'evrax'))) {
            if (Test-Path -LiteralPath $dir) {
                Remove-Item -LiteralPath $dir -Recurse -Force
                Write-Host "dihapus: $dir"
            }
        }
    }
    Write-Host 'evrax di-uninstall.'
    exit 0
}

Assert-NotRunning

$tmp = Join-Path $env:TEMP ("evrax-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp -Force | Out-Null

try {
    $zip = ""
    if ($LocalZip) {
        $zip = (Resolve-Path -LiteralPath $LocalZip).Path
        if (-not (Test-Path -LiteralPath $zip)) { Fail "ZIP tidak ditemukan: $LocalZip" }
        Write-Host "pakai ZIP lokal: $zip"
    } else {
        if (-not $Version) {
            Write-Host 'mengambil rilis terbaru...'
            $release = Invoke-RestMethod -UseBasicParsing -Uri "https://api.github.com/repos/$Repo/releases/latest"
            $Version = $release.tag_name
        }
        if ($Version -notmatch '^v') { $Version = "v$Version" }
        $asset = "evrax-$Version-x86_64-pc-windows-msvc.zip"
        $base = "https://github.com/$Repo/releases/download/$Version"
        $zip = Join-Path $tmp $asset
        $sums = Join-Path $tmp 'SHA256SUMS'
        Write-Host "mengunduh $asset ..."
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile $zip
        Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile $sums

        $expected = ""
        foreach ($line in Get-Content -LiteralPath $sums) {
            $parts = $line.Trim() -split '\s+'
            if ($parts.Count -ge 2 -and $parts[1] -eq $asset) { $expected = $parts[0].ToLower() }
        }
        if (-not $expected) { Fail "SHA256 untuk $asset tidak ada di SHA256SUMS" }
        $actual = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLower()
        if ($actual -ne $expected) { Fail "SHA-256 tidak cocok (dapat $actual, harus $expected)" }
        Write-Host 'SHA-256 OK'
    }

    $extract = Join-Path $tmp 'extract'
    Expand-Archive -LiteralPath $zip -DestinationPath $extract -Force
    $found = Get-ChildItem -LiteralPath $extract -Recurse -Filter $ExeName | Select-Object -First 1
    if ($null -eq $found) { Fail "binary $ExeName tidak ada di dalam ZIP" }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    $target = Join-Path $InstallDir $ExeName
    Copy-Item -LiteralPath $found.FullName -Destination $target -Force
    Write-Host "terpasang: $target"

    Add-PathEntry $InstallDir
    Write-Host ''
    Write-Host 'Selesai. Buka terminal baru lalu jalankan: evrax --version'
} finally {
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
