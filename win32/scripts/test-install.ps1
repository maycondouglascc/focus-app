# Automated verification of win32\scripts\install.ps1 in an isolated temp sandbox.
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\focus.exe')
)

$ErrorActionPreference = 'Stop'
if (-not (Test-Path $Exe)) {
    throw "Release binary not found at $Exe (run check.ps1 -Release first)"
}
$srcItem = Get-Item $Exe
$srcHash = (Get-FileHash $srcItem.FullName -Algorithm SHA256).Hash

$sandbox = Join-Path $env:TEMP ("focus-install-test-" + $PID)
if (Test-Path $sandbox) { Remove-Item -Recurse -Force $sandbox }
$installDir = Join-Path $sandbox 'Programs\Focus'
$dataDir = Join-Path $sandbox 'Data\Focus'
$shortcut = Join-Path $sandbox 'StartMenu\Focus.lnk'
$runKey = "HKCU:\Software\FocusInstallTest-$PID"
$tag = "installtest-$PID"
$installScript = Join-Path $PSScriptRoot 'install.ps1'

New-Item -ItemType Directory -Force (Join-Path $dataDir 'logs') | Out-Null
$seedSentinel = Join-Path $dataDir 'user-backup.dat'
$seedLog = Join-Path $dataDir 'logs\focus.log'
[System.IO.File]::WriteAllBytes($seedSentinel, [byte[]](1..64))
[System.IO.File]::WriteAllText($seedLog, "seed-log-line`n")
$hashSentinelBefore = (Get-FileHash $seedSentinel -Algorithm SHA256).Hash

$env:FOCUS_TEST_INSTANCE = $tag

try {
    # 1. Fresh install (-NoLaunch first, verify binary & shortcut)
    & powershell -ExecutionPolicy Bypass -File $installScript `
        -Exe $srcItem.FullName -InstallDir $installDir -DataDir $dataDir `
        -RunKey $runKey -ShortcutPath $shortcut -InstanceTag $tag -NoLaunch
    if ($LASTEXITCODE -ne 0) { throw "Fresh install failed" }

    $installedExe = Join-Path $installDir 'Focus.exe'
    $installOk = (Test-Path $installedExe) -and
                 ((Get-FileHash $installedExe -Algorithm SHA256).Hash -eq $srcHash) -and
                 (Test-Path $shortcut)

    # Seed existing Run key to simulate upgrade updating command path without clobbering approval
    New-Item -Path $runKey -Force | Out-Null
    Set-ItemProperty -Path $runKey -Name 'Focus' -Value '"C:\Old\Focus.exe" --background' -Type String

    # Launch installed binary once to verify live background startup & graceful upgrade stop
    & powershell -ExecutionPolicy Bypass -File $installScript `
        -Exe $srcItem.FullName -InstallDir $installDir -DataDir $dataDir `
        -RunKey $runKey -ShortcutPath $shortcut -InstanceTag $tag
    if ($LASTEXITCODE -ne 0) { throw "Second (upgrade) install with launch failed" }

    $prevExe = Join-Path $installDir 'Focus.prev.exe'
    $upgradeOk = (Test-Path $prevExe) -and
                 ((Get-FileHash $installedExe -Algorithm SHA256).Hash -eq $srcHash)
    $runVal = (Get-ItemProperty -Path $runKey -Name 'Focus').Focus
    $runKeyOk = ($runVal -eq ('"' + $installedExe + '" --background'))

    $running = Get-Process -Name 'focus' -ErrorAction SilentlyContinue | Where-Object {
        try { $_.Path -and $_.Path.Equals($installedExe, [System.StringComparison]::OrdinalIgnoreCase) } catch { $false }
    }
    $launchOk = ($null -ne $running)

    # 2. Uninstall -> removes program files, shortcut, Run entry; preserves DataDir untouched
    & powershell -ExecutionPolicy Bypass -File $installScript `
        -Exe $srcItem.FullName -InstallDir $installDir -DataDir $dataDir `
        -RunKey $runKey -ShortcutPath $shortcut -InstanceTag $tag -Uninstall
    if ($LASTEXITCODE -ne 0) { throw "Uninstall failed" }

    $stillAlive = Get-Process -Name 'focus' -ErrorAction SilentlyContinue | Where-Object {
        try { $_.Path -and $_.Path.Equals($installedExe, [System.StringComparison]::OrdinalIgnoreCase) } catch { $false }
    }
    $runAfter = Get-ItemProperty -Path $runKey -Name 'Focus' -ErrorAction SilentlyContinue
    $uninstallOk = (-not (Test-Path $installedExe)) -and
                   (-not (Test-Path $prevExe)) -and
                   (-not (Test-Path $shortcut)) -and
                   ($null -eq $stillAlive) -and
                   ($null -eq $runAfter)

    $liveDb = Join-Path $dataDir 'focus.db'
    $dataUntouched = ((Test-Path $seedSentinel) -and
                      ((Get-FileHash $seedSentinel -Algorithm SHA256).Hash -eq $hashSentinelBefore) -and
                      (Test-Path $liveDb) -and
                      ((Get-Item $liveDb).Length -gt 0) -and
                      (Test-Path $seedLog) -and
                      ((Get-Content $seedLog -Raw) -like 'seed-log-line*'))

    Write-Output ("install_ok={0} upgrade_ok={1} launch_ok={2} run_key_ok={3} uninstall_ok={4} data_untouched={5}" -f `
        $installOk, $upgradeOk, $launchOk, $runKeyOk, $uninstallOk, $dataUntouched)

    if (-not ($installOk -and $upgradeOk -and $launchOk -and $runKeyOk -and $uninstallOk -and $dataUntouched)) {
        exit 1
    }
    exit 0
} finally {
    Get-Process -Name 'focus' -ErrorAction SilentlyContinue | Where-Object {
        try { $_.Path -and $_.Path.StartsWith($installDir, [System.StringComparison]::OrdinalIgnoreCase) } catch { $false }
    } | Stop-Process -Force -ErrorAction SilentlyContinue
    Remove-Item -Path $runKey -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -Path $sandbox -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item Env:\FOCUS_TEST_INSTANCE -ErrorAction SilentlyContinue
}
