# Automated checks every commit and every gate depends on. Exit code 0 = pass.
# Usage (from anywhere): .\win32\scripts\check.ps1 [-Release]
#   default  : fmt, clippy, tests
#   -Release : also builds release and enforces the binary size budget (< 5 MB)
param([switch]$Release)

$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"

function Step($name, [scriptblock]$body) {
    Write-Host "== $name"
    & $body
    if ($LASTEXITCODE -ne 0) { Write-Host "FAIL: $name"; exit 1 }
}

Step 'cargo fmt --check' { cargo fmt --check }
Step 'cargo clippy' { cargo clippy --all-targets -- -D warnings }
Step 'cargo test' { cargo test }

if ($Release) {
    Step 'cargo build --release' { cargo build --release }
    $exe = Get-Item 'target\release\focus.exe'
    $mb = [math]::Round($exe.Length / 1MB, 2)
    Write-Host "binary size: $mb MB (budget 5 MB)"
    if ($exe.Length -ge 5MB) { Write-Host 'FAIL: binary over budget'; exit 1 }
}
Write-Host 'OK'
