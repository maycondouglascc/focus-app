<#
.SYNOPSIS
    Background task watchdog for Antigravity hooks (PreInvocation / Stop) and manual checks.
.DESCRIPTION
    Reads conversation transcript + task logs to detect background tasks (`task-*`) that have
    produced no new log output for >= WarnSeconds (default 180s) or >= KillSeconds (default 300s),
    or whose underlying shell process has exited without completing.
#>
param(
    [string]$TranscriptPath = '',
    [string]$ArtifactDir = '',
    [string]$ConversationId = '',
    [int]$WarnSeconds = 180,
    [int]$KillSeconds = 300,
    [switch]$Report
)

$ErrorActionPreference = 'SilentlyContinue'

function Write-EmptyHook {
    Write-Output '{}'
    exit 0
}

function ConvertFrom-FileUri([string]$uri) {
    if ([string]::IsNullOrWhiteSpace($uri)) { return '' }
    if ($uri -match '^file:///(.+)$') {
        return ($Matches[1] -replace '/', '\')
    }
    return $uri
}

function Get-ShortDescSnippet([string]$desc) {
    if ([string]::IsNullOrWhiteSpace($desc)) { return '' }
    $firstLine = ($desc -split "`r?`n")[0].Trim()
    if ($firstLine.Length -gt 64) {
        return $firstLine.Substring(0, 64)
    }
    return $firstLine
}

$hookEvent = ''
$continuationActive = $false
$fullyIdle = $false
$hasStdin = $false

if ([Console]::IsInputRedirected -and -not $Report) {
    $raw = [Console]::In.ReadToEnd()
    if (-not [string]::IsNullOrWhiteSpace($raw)) {
        $hasStdin = $true
        try {
            $payload = $raw | ConvertFrom-Json
            if ($payload.hookEventName) { $hookEvent = [string]$payload.hookEventName }
            if ($payload.transcriptPath -and -not $TranscriptPath) { $TranscriptPath = [string]$payload.transcriptPath }
            if ($payload.artifactDirectoryPath -and -not $ArtifactDir) { $ArtifactDir = [string]$payload.artifactDirectoryPath }
            if ($payload.conversationId -and -not $ConversationId) { $ConversationId = [string]$payload.conversationId }
            if ($null -ne $payload.continuationActive) { $continuationActive = [bool]$payload.continuationActive }
            if ($null -ne $payload.fullyIdle) { $fullyIdle = [bool]$payload.fullyIdle }
        } catch {
            if (-not $Report) { Write-EmptyHook }
        }
    }
}

# Stop hook fast-path: nothing running, or a Stop hook already forced this continuation.
if ($hookEvent -eq 'Stop') {
    if ($fullyIdle -or $continuationActive) {
        Write-EmptyHook
    }
}

if ([string]::IsNullOrWhiteSpace($TranscriptPath) -and -not [string]::IsNullOrWhiteSpace($ArtifactDir)) {
    $TranscriptPath = Join-Path $ArtifactDir '.system_generated\logs\transcript.jsonl'
}

if ([string]::IsNullOrWhiteSpace($TranscriptPath) -or -not (Test-Path $TranscriptPath)) {
    if ($Report) {
        Write-Output '[]'
        exit 0
    }
    Write-EmptyHook
}

$tasksDir = if (-not [string]::IsNullOrWhiteSpace($ArtifactDir)) {
    Join-Path $ArtifactDir '.system_generated\tasks'
} else {
    $logsParent = Split-Path (Split-Path $TranscriptPath -Parent) -Parent
    Join-Path $logsParent 'tasks'
}

# Track active tasks from transcript.jsonl (`Tool is running as a background task` minus completed/canceled ones).
$active = @{}
try {
    foreach ($line in [System.IO.File]::ReadLines($TranscriptPath)) {
        $hasStart = $line.Contains('Tool is running as a background task with task id:')
        $hasSender = $line.Contains('sender=') -and $line.Contains('/task-')
        $hasCancel = $line.Contains('cancelled.') -and $line.Contains('/task-')
        if (-not ($hasStart -or $hasSender -or $hasCancel)) {
            continue
        }
        try {
            $step = $line | ConvertFrom-Json
        } catch {
            continue
        }
        $content = [string]$step.content
        if ($hasStart -and $content -match 'task id:\s*([^\s\r\n]+)') {
            $fullId = $Matches[1].Trim()
            $shortId = if ($fullId -match '(task-\d+)$') { $Matches[1] } else { $fullId }
            $logPath = ''
            if ($content -match 'Task logs are available at:\s*(file:///[^\s\r\n]+)') {
                $logPath = ConvertFrom-FileUri $Matches[1].Trim()
            } elseif (-not [string]::IsNullOrWhiteSpace($tasksDir)) {
                $logPath = Join-Path $tasksDir "$shortId.log"
            }
            $desc = ''
            if ($content -match '(?s)Task Description:\s*(.+?)(?:\r?\nTask logs are available at:|$)') {
                $desc = $Matches[1].Trim()
            }
            $createdUtc = [DateTime]::UtcNow
            if ($step.created_at) {
                try {
                    $createdUtc = [DateTimeOffset]::Parse([string]$step.created_at).UtcDateTime
                } catch {}
            }
            $active[$shortId] = [pscustomobject]@{
                fullId     = $fullId
                shortId    = $shortId
                createdUtc = $createdUtc
                logPath    = $logPath
                desc       = $desc
            }
        }
        if ($hasSender -and $content -match 'sender=[^\s]+/(task-\d+)') {
            $doneId = $Matches[1]
            if ($active.ContainsKey($doneId)) {
                $active.Remove($doneId)
            }
        }
        if ($hasCancel -and $content -match 'Task "[^"]*/(task-\d+)" cance(?:ll|l)ed\.') {
            $killId = $Matches[1]
            if ($active.ContainsKey($killId)) {
                $active.Remove($killId)
            }
        }
    }
} catch {
    if ($Report) {
        Write-Output '[]'
        exit 0
    }
    Write-EmptyHook
}

if ($active.Count -eq 0) {
    if ($Report) {
        Write-Output '[]'
        exit 0
    }
    Write-EmptyHook
}

$nowUtc = [DateTime]::UtcNow

# Only query Win32_Process when an active task is old enough (>= 30s) or in -Report mode.
$needProcessScan = $Report.IsPresent
if (-not $needProcessScan) {
    foreach ($t in $active.Values) {
        if (($nowUtc - $t.createdUtc).TotalSeconds -ge 30) {
            $needProcessScan = $true
            break
        }
    }
}

$procs = @()
if ($needProcessScan) {
    try {
        $procs = @(Get-CimInstance Win32_Process -Property ProcessId,ParentProcessId,Name,CommandLine,CreationDate | Where-Object { $_.CommandLine })
    } catch {
        $procs = @()
    }
}

function Find-MatchingProcesses($task, $allProcs) {
    $snippet = (Get-ShortDescSnippet $task.desc) -replace '\\"', '"'
    if ([string]::IsNullOrWhiteSpace($snippet) -or $allProcs.Count -eq 0) {
        return @()
    }
    $roots = @($allProcs | Where-Object {
        $norm = $_.CommandLine -replace '\\"', '"'
        $timeOk = $true
        if ($_.CreationDate) {
            try {
                $timeOk = [math]::Abs(($_.CreationDate.ToUniversalTime() - $task.createdUtc).TotalSeconds) -le 20
            } catch {}
        }
        $_.Name -match '^(powershell|pwsh|cmd)\.exe$' -and $timeOk -and $norm.Contains($snippet)
    })
    if ($roots.Count -eq 0) { return @() }
    $pids = New-Object 'System.Collections.Generic.HashSet[uint32]'
    foreach ($r in $roots) { [void]$pids.Add([uint32]$r.ProcessId) }
    $changed = $true
    while ($changed) {
        $changed = $false
        foreach ($p in $allProcs) {
            if ($pids.Contains([uint32]$p.ParentProcessId) -and -not $pids.Contains([uint32]$p.ProcessId)) {
                [void]$pids.Add([uint32]$p.ProcessId)
                $changed = $true
            }
        }
    }
    return @($allProcs | Where-Object { $pids.Contains([uint32]$_.ProcessId) })
}

$findings = @()
$alerts = @()

foreach ($t in ($active.Values | Sort-Object createdUtc)) {
    $elapsedSec = [math]::Round(($nowUtc - $t.createdUtc).TotalSeconds, 1)
    $logBytes = 0L
    $staleSec = $elapsedSec
    $logExists = $false

    if (-not [string]::IsNullOrWhiteSpace($t.logPath) -and (Test-Path $t.logPath)) {
        $item = Get-Item $t.logPath
        $logExists = $true
        $logBytes = [int64]$item.Length
        if ($logBytes -gt 0) {
            $staleSec = [math]::Round(($nowUtc - $item.LastWriteTimeUtc).TotalSeconds, 1)
        }
    }

    $matched = if ($needProcessScan) { @(Find-MatchingProcesses $t $procs) } else { @() }
    $childNames = @($matched | Where-Object { $_.Name -notmatch '^(powershell|pwsh|cmd)\.exe$' } | Select-Object -ExpandProperty Name -Unique)

    $level = 'ok'
    $reason = ''
    if ($needProcessScan -and $elapsedSec -ge 30 -and $matched.Count -eq 0 -and -not [string]::IsNullOrWhiteSpace((Get-ShortDescSnippet $t.desc))) {
        # Verify at least powershell processes exist in scan before claiming dead.
        $anyPs = @($procs | Where-Object { $_.Name -match '^(powershell|pwsh|cmd)\.exe$' }).Count -gt 0
        if ($anyPs) {
            $level = 'dead'
            $reason = "no matching OS process found after ${elapsedSec}s (log=${logBytes}B)"
        }
    }
    if ($level -eq 'ok') {
        if ($staleSec -ge $KillSeconds) {
            $level = 'kill'
            $reason = "no new log output for ${staleSec}s (>= ${KillSeconds}s threshold, elapsed=${elapsedSec}s, log=${logBytes}B)"
        } elseif ($staleSec -ge $WarnSeconds) {
            $level = 'warn'
            $reason = "no new log output for ${staleSec}s (>= ${WarnSeconds}s threshold, elapsed=${elapsedSec}s, log=${logBytes}B)"
        }
    }

    $entry = [pscustomobject]@{
        taskId     = $t.fullId
        shortId    = $t.shortId
        elapsedSec = $elapsedSec
        staleSec   = $staleSec
        logExists  = $logExists
        logBytes   = $logBytes
        procsCount = $matched.Count
        childProcs = ($childNames -join ',')
        level      = $level
        reason     = $reason
    }
    $findings += $entry
    if ($level -ne 'ok') {
        $alerts += $entry
    }
}

if ($Report) {
    if ($findings.Count -eq 0) {
        Write-Output '[]'
    } else {
        Write-Output ($findings | ConvertTo-Json -Depth 4)
    }
    exit 0
}

if ($alerts.Count -eq 0) {
    Write-EmptyHook
}

# Cooldown tracking for PreInvocation so identical warnings do not spam every consecutive tool step.
$stateKey = if ([string]::IsNullOrWhiteSpace($ConversationId)) { 'default' } else { ($ConversationId -replace '[^a-zA-Z0-9_-]', '_') }
$stateFile = Join-Path $env:TEMP "agy-task-watchdog-$stateKey.json"
$prevState = @{}
if (Test-Path $stateFile) {
    try {
        $loaded = Get-Content $stateFile -Raw | ConvertFrom-Json
        foreach ($prop in $loaded.PSObject.Properties) {
            $prevState[$prop.Name] = $prop.Value
        }
    } catch {}
}

$emitAlerts = @()
$nowEpoch = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
$nextState = @{}
foreach ($a in $alerts) {
    $shouldEmit = $true
    if ($hookEvent -eq 'PreInvocation' -and $prevState.ContainsKey($a.shortId)) {
        $prev = $prevState[$a.shortId]
        $age = $nowEpoch - [int64]$prev.ts
        if ($prev.level -eq $a.level -and $age -lt 90) {
            $shouldEmit = $false
        }
    }
    $nextState[$a.shortId] = @{ level = $a.level; ts = $nowEpoch }
    if ($shouldEmit) {
        $emitAlerts += $a
    }
}

try {
    ($nextState | ConvertTo-Json -Depth 3) | Set-Content -Path $stateFile -Encoding utf8
} catch {}

if ($emitAlerts.Count -eq 0) {
    Write-EmptyHook
}

$lines = @()
foreach ($a in $emitAlerts) {
    $tag = switch ($a.level) {
        'dead' { 'DEAD/ORPHANED' }
        'kill' { 'STALLED >= 5m (KILL RECOMMENDED)' }
        default { 'STALLED >= 3m (CHECK REQUIRED)' }
    }
    $children = if ($a.childProcs) { " children=[$($a.childProcs)]" } else { '' }
    $lines += "- [$tag] $($a.taskId): $($a.reason)$children"
}

$msg = "[task-watchdog] Background task alert:`n" + ($lines -join "`n") +
    "`nRun manage_task(status) to check progress; if stalled/dead/looping without active compiler/test progress, call manage_task(kill) instead of waiting."

if ($hookEvent -eq 'Stop') {
    $out = @{
        decision = 'continue'
        reason   = $msg
    } | ConvertTo-Json -Compress
    Write-Output $out
    exit 0
}

$out = @{
    injectSteps = @(
        @{
            systemMessage = @{
                systemMessage = $msg
            }
        }
    )
} | ConvertTo-Json -Depth 5 -Compress
Write-Output $out
exit 0
