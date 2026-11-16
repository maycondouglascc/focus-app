# Samples the running focus.exe once a minute and appends numbers to a CSV.
# Only numbers are written (no window titles), so the CSV is safe to commit.
# Usage: .\scripts\measure.ps1 [-Hours 8] [-IntervalSeconds 60] [-Name 8h]
param(
    [double]$Hours = 8,
    [int]$IntervalSeconds = 60,
    [string]$Name = '8h'
)

$dir = Join-Path $PSScriptRoot '..\docs\measurements'
New-Item -ItemType Directory -Force $dir | Out-Null
$csv = Join-Path $dir ("{0}-{1}.csv" -f (Get-Date -Format 'yyyy-MM-dd'), $Name)
if (-not (Test-Path $csv)) {
    'timestamp,private_mb,working_set_mb,cpu_seconds,threads,handles,io_write_bytes' | Out-File $csv -Encoding ascii
}

$end = (Get-Date).AddHours($Hours)
Write-Host "Writing to $csv until $end (Ctrl+C to stop early)"

while ((Get-Date) -lt $end) {
    $p = Get-Process focus -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $p) {
        Write-Host "$(Get-Date -Format T) focus.exe is not running; waiting..."
        Start-Sleep -Seconds 10
        continue
    }
    $w = Get-CimInstance Win32_Process -Filter "ProcessId=$($p.Id)"
    $ci = [cultureinfo]::InvariantCulture
    $row = [string]::Format($ci, '{0},{1:F2},{2:F2},{3:F3},{4},{5},{6}',
        (Get-Date).ToString('s'),
        ($p.PrivateMemorySize64 / 1MB), ($p.WorkingSet64 / 1MB),
        $p.CPU, $p.Threads.Count, $p.HandleCount, $w.WriteTransferCount)
    $row | Out-File $csv -Append -Encoding ascii
    Start-Sleep -Seconds $IntervalSeconds
}
Write-Host 'Done.'
