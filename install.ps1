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
    # Throw instead of exit: this script is commonly run via `irm ... | iex`,
    # where `exit` tears down the whole PowerShell window and the user never
    # sees the reason.
    throw "evrax install failed: $Message"
}

function Test-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($id)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Assert-NotRunning {
    $proc = Get-Process -Name 'evrax' -ErrorAction SilentlyContinue
    if ($null -ne $proc) {
        Fail "evrax is running (PID $($proc.Id -join ', ')). Close it first (Ctrl+C) and retry."
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
        Fail '-System requires PowerShell running as Administrator (or use -NoPath).'
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

# Resolves the latest tag without depending on the GitHub API alone: the REST
# endpoint enforces a 60/hour anonymous per-IP rate limit (shared behind NAT,
# VPN or a proxy), so a 403 there must not abort the install. The
# /releases/latest web endpoint redirects to /releases/tag/<tag> with no rate
# limit and falls back cleanly.
function Resolve-LatestVersion {
    try {
        $release = Invoke-RestMethod -UseBasicParsing -Headers @{ 'User-Agent' = 'evrax' } `
            -Uri "https://api.github.com/repos/$Repo/releases/latest"
        if ($release.tag_name) { return $release.tag_name }
    } catch {
        Write-Host 'api.github.com is rate-limited or unavailable; using the web redirect...' -ForegroundColor DarkGray
    }
    $location = ""
    try {
        $req = [System.Net.HttpWebRequest]::Create("https://github.com/$Repo/releases/latest")
        $req.AllowAutoRedirect = $false
        $req.Method = 'GET'
        $req.UserAgent = 'evrax'
        $resp = $req.GetResponse()
        $location = $resp.Headers['Location']
    } catch [System.Net.WebException] {
        $resp = $_.Exception.Response
        if ($resp) { $location = $resp.Headers['Location'] }
    }
    if ($location -and ($location -match '/releases/tag/([^/?#]+)')) { return $Matches[1] }
    return ""
}

# Everything below runs inside one try/catch so a failure prints a message and
# the script ends normally. Combined with `throw` in Fail (never `exit`), this
# keeps the caller's PowerShell window open whether the script is run from a
# console or piped through `irm ... | iex`.
try {
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
        Write-Host "removed: $exe"
    } else {
        Write-Host "no binary at $exe"
    }
    Remove-PathEntry $InstallDir
    if ((Test-Path -LiteralPath $InstallDir) -and -not (Get-ChildItem -LiteralPath $InstallDir -Force)) {
        Remove-Item -LiteralPath $InstallDir -Force
    }
    if ($Purge) {
        foreach ($dir in @((Join-Path $env:APPDATA 'evrax'), (Join-Path $env:LOCALAPPDATA 'evrax'))) {
            if (Test-Path -LiteralPath $dir) {
                Remove-Item -LiteralPath $dir -Recurse -Force
                Write-Host "removed: $dir"
            }
        }
    }
    Write-Host 'evrax uninstalled.'
    return
}

Assert-NotRunning

$tmp = Join-Path $env:TEMP ("evrax-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp -Force | Out-Null

try {
    $zip = ""
    if ($LocalZip) {
        $zip = (Resolve-Path -LiteralPath $LocalZip).Path
        if (-not (Test-Path -LiteralPath $zip)) { Fail "ZIP not found: $LocalZip" }
        Write-Host "using local ZIP: $zip"
    } else {
        if (-not $Version) {
            Write-Host 'resolving the latest release...'
            $Version = Resolve-LatestVersion
            if (-not $Version) {
                Fail "could not resolve the latest release (GitHub API rate-limited and the web redirect failed). Retry later or pin a version, e.g. -Version 0.1.5."
            }
        }
        if ($Version -notmatch '^v') { $Version = "v$Version" }
        $asset = "evrax-$Version-x86_64-pc-windows-msvc.zip"
        $base = "https://github.com/$Repo/releases/download/$Version"
        $zip = Join-Path $tmp $asset
        $sums = Join-Path $tmp 'SHA256SUMS'
        Write-Host "downloading $asset ..."
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile $zip
        Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile $sums

        $expected = ""
        foreach ($line in Get-Content -LiteralPath $sums) {
            $parts = $line.Trim() -split '\s+'
            if ($parts.Count -ge 2 -and $parts[1] -eq $asset) { $expected = $parts[0].ToLower() }
        }
        if (-not $expected) { Fail "SHA256 for $asset is missing from SHA256SUMS" }
        $actual = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLower()
        if ($actual -ne $expected) { Fail "SHA-256 mismatch (got $actual, expected $expected)" }
        Write-Host 'SHA-256 OK'
    }

    $extract = Join-Path $tmp 'extract'
    Expand-Archive -LiteralPath $zip -DestinationPath $extract -Force
    $found = Get-ChildItem -LiteralPath $extract -Recurse -Filter $ExeName | Select-Object -First 1
    if ($null -eq $found) { Fail "binary $ExeName is missing from the ZIP" }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    $target = Join-Path $InstallDir $ExeName
    Copy-Item -LiteralPath $found.FullName -Destination $target -Force
    Write-Host "installed: $target"

    Add-PathEntry $InstallDir
    Write-Host ''
    Write-Host 'Done. Open a new terminal and run: evrax --version'
} finally {
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
} catch {
    Write-Host ''
    Write-Host "evrax install did not complete: $($_.Exception.Message)" -ForegroundColor Red
    Write-Host 'Fix the problem above and run the installer again.'
}
