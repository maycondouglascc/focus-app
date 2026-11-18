# Measures cold startup latency (process creation -> Shell_NotifyIconW return) across 10 runs.
# Usage: .\win32\scripts\measure-startup.ps1 [-Exe <path>] [-Runs 10]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\focus.exe'),
    [int]$Runs = 10
)

$ErrorActionPreference = 'Stop'
if (-not (Test-Path $Exe)) {
    throw "Executable not found: $Exe"
}
$exeFull = (Resolve-Path $Exe).Path

$outDir = Join-Path $PSScriptRoot '..\docs\measurements'
New-Item -ItemType Directory -Force $outDir | Out-Null
$csv = Join-Path $outDir ("{0}-startup.csv" -f (Get-Date -Format 'yyyy-MM-dd'))
"run,tray_ms,entry_ms,win_ms" | Set-Content -Path $csv -Encoding ASCII

$sandbox = Join-Path $env:TEMP ("focus-startup-meas-" + $PID)
if (Test-Path $sandbox) { Remove-Item -Recurse -Force $sandbox }
New-Item -ItemType Directory -Force $sandbox | Out-Null

$samples = New-Object System.Collections.Generic.List[double]
$entrySamples = New-Object System.Collections.Generic.List[double]
$ci = [System.Globalization.CultureInfo]::InvariantCulture
try {
    for ($i = 1; $i -le $Runs; $i++) {
        $runDir = Join-Path $sandbox ("run-$i")
        $logDir = Join-Path $runDir 'logs'
        $dbPath = Join-Path $runDir 'startup.db'
        New-Item -ItemType Directory -Force $logDir | Out-Null
        $logFile = Join-Path $logDir 'focus.log'

        $env:FOCUS_TEST_INSTANCE = "startup-$PID-$i"
        $env:FOCUS_LOG_DIR = $logDir

        $p = Start-Process -FilePath $exeFull -ArgumentList @('--db', $dbPath, '--background') -PassThru
        $trayMs = $null
        $entryMs = $null
        $winMs = $null
        $deadline = (Get-Date).AddSeconds(10)
        while ((Get-Date) -lt $deadline) {
            if (Test-Path $logFile) {
                $content = Get-Content $logFile -Raw -ErrorAction SilentlyContinue
                if ($content -and $content -match 'tray_ms=([0-9]+(?:\.[0-9]+)?)') {
                    $trayMs = [double]$matches[1]
                    if ($content -match 'entry_ms=([0-9]+(?:\.[0-9]+)?)') { $entryMs = [double]$matches[1] }
                    if ($content -match 'win_ms=([0-9]+(?:\.[0-9]+)?)') { $winMs = [double]$matches[1] }
                    if ($null -ne $entryMs -and $null -ne $winMs) { break }
                }
            }
            if ($p.HasExited) {
                throw "Focus exited early on run $i with code $($p.ExitCode)"
            }
            Start-Sleep -Milliseconds 30
        }

        & $exeFull --db $dbPath --quit | Out-String | Out-Null
        if ($p) {
            [void]$p.WaitForExit(10000)
        }
        if ($null -eq $trayMs) {
            throw "Timed out waiting for startup tray_ms log metric on run $i"
        }

        $samples.Add($trayMs)
        if ($null -ne $entryMs) { $entrySamples.Add($entryMs) }
        $eStr = if ($null -ne $entryMs) { ([math]::Round($entryMs, 2)).ToString($ci) } else { '' }
        $wStr = if ($null -ne $winMs) { ([math]::Round($winMs, 2)).ToString($ci) } else { '' }
        "$i,$(([math]::Round($trayMs, 2)).ToString($ci)),$eStr,$wStr" |
            Add-Content -Path $csv -Encoding ASCII
    }

    $sorted = $samples | Sort-Object
    $p50 = $sorted[[math]::Floor(($sorted.Count - 1) * 0.50)]
    $p95 = $sorted[[math]::Ceiling(($sorted.Count - 1) * 0.95)]
    $max = $sorted[$sorted.Count - 1]
    $eNote = ''
    if ($entrySamples.Count -eq $sorted.Count) {
        $es = $entrySamples | Sort-Object
        $ep50 = $es[[math]::Floor(($es.Count - 1) * 0.50)]
        $eNote = [string]::Format($ci, ' entry_p50_ms={0:F2}', $ep50)
    }
    Write-Output ([string]::Format($ci, "runs={0} p50_ms={1:F2} p95_ms={2:F2} max_ms={3:F2}{4} (budget < 150 ms) csv={5}",
        $sorted.Count, $p50, $p95, $max, $eNote, $csv))
    if ($p95 -ge 150.0) {
        Write-Host "FAIL: startup p95 over 150 ms budget"
        exit 1
    }
    Write-Output "OK"
    exit 0
} finally {
    Remove-Item Env:\FOCUS_TEST_INSTANCE -ErrorAction SilentlyContinue
    Remove-Item Env:\FOCUS_LOG_DIR -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $sandbox -ErrorAction SilentlyContinue
}
