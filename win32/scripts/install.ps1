# Installs, upgrades, or uninstalls Focus.exe in %LOCALAPPDATA%\Programs\Focus.
# Never deletes user data in %LOCALAPPDATA%\Focus.
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\focus.exe'),
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\Focus'),
    [string]$DataDir = (Join-Path $env:LOCALAPPDATA 'Focus'),
    [string]$RunKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run',
    [string]$ShortcutPath = (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Focus.lnk'),
    [string]$InstanceTag = '',
    [switch]$NoLaunch,
    [switch]$Uninstall
)

$ErrorActionPreference = 'Stop'
$targetExe = Join-Path $InstallDir 'Focus.exe'
$prevExe = Join-Path $InstallDir 'Focus.prev.exe'
$newExe = Join-Path $InstallDir 'Focus.exe.new'

$defaultDataDir = Join-Path $env:LOCALAPPDATA 'Focus'

function Set-TestEnv {
    if ($InstanceTag) {
        $env:FOCUS_TEST_INSTANCE = $InstanceTag
    }
    if ($DataDir -and -not $DataDir.Equals($defaultDataDir, [System.StringComparison]::OrdinalIgnoreCase)) {
        $env:FOCUS_LOG_DIR = Join-Path $DataDir 'logs'
        $env:FOCUS_TEST_CRASH_DIR = Join-Path $DataDir 'crashes'
    }
}

function Stop-RunningFocus {
    param([string]$QuitExePath)
    Set-TestEnv
    $procs = Get-Process -Name 'focus' -ErrorAction SilentlyContinue | Where-Object {
        try {
            $_.Path -and ($_.Path.Equals($targetExe, [System.StringComparison]::OrdinalIgnoreCase))
        } catch { $false }
    }
    if (-not $procs) {
        return
    }
    if ($QuitExePath -and (Test-Path $QuitExePath)) {
        & $QuitExePath '--quit' | Out-String | Out-Null
    }
    $deadline = (Get-Date).AddSeconds(10)
    while ((Get-Date) -lt $deadline) {
        $alive = Get-Process -Name 'focus' -ErrorAction SilentlyContinue | Where-Object {
            try {
                $_.Path -and ($_.Path.Equals($targetExe, [System.StringComparison]::OrdinalIgnoreCase))
            } catch { $false }
        }
        if (-not $alive) {
            return
        }
        Start-Sleep -Milliseconds 250
    }
    throw "Timed out waiting 10s for running $targetExe to exit gracefully via --quit"
}

if ($Uninstall) {
    Stop-RunningFocus -QuitExePath $targetExe
    foreach ($f in @($targetExe, $prevExe, $newExe)) {
        if (Test-Path $f) { Remove-Item -Force $f }
    }
    if ($ShortcutPath -and (Test-Path $ShortcutPath)) {
        Remove-Item -Force $ShortcutPath
    }
    if (Test-Path $RunKey) {
        Remove-ItemProperty -Path $RunKey -Name 'Focus' -ErrorAction SilentlyContinue
    }
    if ((Test-Path $InstallDir) -and -not (Get-ChildItem -Force $InstallDir -ErrorAction SilentlyContinue)) {
        Remove-Item -Force $InstallDir -ErrorAction SilentlyContinue
    }
    Write-Output "Uninstalled Focus from $InstallDir (data preserved at $DataDir)"
    exit 0
}

if (-not (Test-Path $Exe)) {
    throw "Source binary not found: $Exe"
}
$exeItem = Get-Item $Exe
if ($exeItem.Length -ge 5MB) {
    throw "Refusing to install binary >= 5 MB ($($exeItem.Length) bytes)"
}

Set-TestEnv
$verOut = (& $exeItem.FullName '--version' | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or -not ($verOut -match '^focus\s+\d+\.\d+\.\d+')) {
    throw "Binary --version check failed: exit=$LASTEXITCODE out='$verOut'"
}

$quitExe = if (Test-Path $targetExe) { $targetExe } else { $exeItem.FullName }
Stop-RunningFocus -QuitExePath $quitExe

New-Item -ItemType Directory -Force $InstallDir | Out-Null
if (Test-Path $newExe) { Remove-Item -Force $newExe }
Copy-Item -Force $exeItem.FullName $newExe

if (Test-Path $targetExe) {
    if (Test-Path $prevExe) { Remove-Item -Force $prevExe }
    Move-Item -Force $targetExe $prevExe
    try {
        Move-Item -Force $newExe $targetExe
    } catch {
        if (Test-Path $prevExe -and -not (Test-Path $targetExe)) {
            Move-Item -Force $prevExe $targetExe
        }
        throw
    }
} else {
    Move-Item -Force $newExe $targetExe
}

if ($ShortcutPath) {
    $scDir = Split-Path -Parent $ShortcutPath
    if ($scDir) { New-Item -ItemType Directory -Force $scDir | Out-Null }
    $ws = New-Object -ComObject WScript.Shell
    $sc = $ws.CreateShortcut($ShortcutPath)
    $sc.TargetPath = $targetExe
    $sc.WorkingDirectory = $InstallDir
    $sc.IconLocation = "$targetExe,0"
    $sc.Description = 'Focus time & activity tracker'
    $sc.Save()
}

# Update HKCU\...\Run\Focus command ONLY if already registered; never touch StartupApproved.
if (Test-Path $RunKey) {
    $existing = Get-ItemProperty -Path $RunKey -Name 'Focus' -ErrorAction SilentlyContinue
    if ($null -ne $existing) {
        Set-ItemProperty -Path $RunKey -Name 'Focus' -Value ('"' + $targetExe + '" --background') -Type String
    }
}

if (-not $NoLaunch) {
    $launchArgs = if ($DataDir -and -not $DataDir.Equals($defaultDataDir, [System.StringComparison]::OrdinalIgnoreCase)) {
        @('--db', (Join-Path $DataDir 'focus.db'), '--background')
    } else {
        @('--background')
    }
    $p = Start-Process -FilePath $targetExe -ArgumentList $launchArgs -PassThru
    Start-Sleep -Milliseconds 750
    if ($p.HasExited) {
        throw "Launched Focus exited unexpectedly with code $($p.ExitCode)"
    }
}

Write-Output "Installed $verOut -> $targetExe"
exit 0
