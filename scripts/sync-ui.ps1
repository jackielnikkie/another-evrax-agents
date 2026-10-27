#requires -Version 5.1
<#
.SYNOPSIS
  Mirror the evrax UI assets into the separate `evrax-ui` repository.

.DESCRIPTION
  The canonical UI lives inside the main repo, embedded into the binary:
    - crates/evrax-remote/src/web/index.html   (web remote, include_str!)
    - crates/evrax-tui/src/theme.rs            (TUI themes; reference only)
    - crates/evrax-tui/src/banner_{wide,compact}.txt
    - docs/themes-notes.md, docs/ui-reference.md
    - assets/*.png (screenshots)

  `evrax-ui` is a view-only mirror so the UI can be browsed and prototyped
  without the Rust source. It is never a build input. Run this script after
  changing any UI file, then commit what it copies.

.PARAMETER Dest
  Path to a checkout of the evrax-ui repository. Required.

.PARAMETER Commit
  Also stage, commit and push the mirrored files in the destination repo.

.PARAMETER Message
  Commit message when -Commit is used.

.EXAMPLE
  .\scripts\sync-ui.ps1 -Dest ..\evrax-ui -Commit
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Dest,
    [switch]$Commit,
    [string]$Message = "chore: sync UI assets from main repo"
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$dest = (Resolve-Path -LiteralPath $Dest).Path

if (-not (Test-Path -LiteralPath (Join-Path $dest ".git"))) {
    throw "not a git repository: $dest"
}
if ((Resolve-Path -LiteralPath $repoRoot).Path -eq $dest) {
    throw "destination must differ from the source repository"
}

# source -> destination, relative to each repo root.
$map = @(
    @{ Src = "crates/evrax-remote/src/web/index.html"; Dst = "index.html" }
    @{ Src = "crates/evrax-tui/src/banner_wide.txt";    Dst = "assets/banner_wide.txt" }
    @{ Src = "crates/evrax-tui/src/banner_compact.txt"; Dst = "assets/banner_compact.txt" }
    @{ Src = "docs/themes-notes.md";                    Dst = "docs/themes-notes.md" }
    @{ Src = "docs/ui-reference.md";                    Dst = "docs/ui-reference.md" }
    @{ Src = "assets/evrax1.png";                       Dst = "assets/evrax1.png" }
    @{ Src = "assets/evrax2.png";                       Dst = "assets/evrax2.png" }
)

$copied = 0
foreach ($entry in $map) {
    $srcPath = Join-Path $repoRoot $entry.Src
    if (-not (Test-Path -LiteralPath $srcPath)) {
        throw "missing source file: $($entry.Src)"
    }
    $dstPath = Join-Path $dest $entry.Dst
    $dstDir = Split-Path -Parent $dstPath
    if (-not (Test-Path -LiteralPath $dstDir)) {
        New-Item -ItemType Directory -Path $dstDir -Force | Out-Null
    }

    $srcHash = (Get-FileHash -LiteralPath $srcPath -Algorithm SHA256).Hash
    $same = $false
    if (Test-Path -LiteralPath $dstPath) {
        $same = ((Get-FileHash -LiteralPath $dstPath -Algorithm SHA256).Hash -eq $srcHash)
    }

    if ($same) {
        Write-Host "unchanged  $($entry.Dst)"
    }
    else {
        Copy-Item -LiteralPath $srcPath -Destination $dstPath -Force
        Write-Host "copied     $($entry.Dst)  ($($srcHash.Substring(0, 12)))"
        $copied++
    }
}

if ($Commit) {
    Push-Location $dest
    try {
        git add -A
        $staged = (git diff --cached --name-only)
        if (-not $staged) {
            Write-Host "nothing staged; destination already up to date"
            return
        }
        git commit -m $Message | Out-Host
        git push | Out-Host
        Write-Host "pushed to $(git remote get-url origin)"
    }
    finally {
        Pop-Location
    }
}
else {
    Write-Host ""
    Write-Host "$copied file(s) updated in $dest"
    Write-Host "review and commit there, or re-run with -Commit"
}
