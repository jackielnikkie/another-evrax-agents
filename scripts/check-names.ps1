$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot
Push-Location $repo
try {
    $words = @(
        (('o', 'p', 'e', 'n', 'c', 'o', 'd', 'e') -join ''),
        (('h', 'e', 'r', 'm', 'e', 's') -join '')
    )
    $allow = @('AGENTS.md', 'docs/', 'skill-evrax/', 'LICENSE', '.gitignore')

    $files = & git ls-files
    $bad = 0
    foreach ($file in $files) {
        $skip = $false
        foreach ($prefix in $allow) {
            if ($file -eq $prefix -or $file.StartsWith($prefix)) { $skip = $true }
        }
        if ($skip) { continue }
        if (-not (Test-Path -LiteralPath $file)) { continue }

        $text = Get-Content -LiteralPath $file -Raw -ErrorAction SilentlyContinue
        if ($null -eq $text) { continue }
        foreach ($word in $words) {
            if ($text.IndexOf($word, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
                Write-Host "name guard: forbidden term in $file"
                $bad++
                break
            }
        }
    }

    if ($bad -gt 0) {
        Write-Host "name guard: $bad file(s) failed"
        exit 1
    }
    Write-Host 'name guard: clean'
} finally {
    Pop-Location
}
