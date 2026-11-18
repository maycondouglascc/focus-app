# Samples the running focus.exe periodically and appends numbers to a CSV.
# Only numbers are written (no window titles), so the CSV is safe to commit.
# Usage: .\scripts\measure.ps1 [-Hours 8] [-IntervalSeconds 60] [-Name 8h]
param(
    [double]$Hours = 8,
    [int]$IntervalSeconds = 60,
    [string]$Name = '8h'
)

$ctxCode = @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

public static class FocusProcStats {
    [DllImport("ntdll.dll")]
    private static extern int NtQuerySystemInformation(int infoClass, IntPtr buf, int bufLen, out int retLen);

    public static Dictionary<long, uint> ThreadContextSwitches(int pid) {
        var map = new Dictionary<long, uint>();
        int len = 1 << 20;
        IntPtr buf = Marshal.AllocHGlobal(len);
        try {
            int needed;
            while (NtQuerySystemInformation(5, buf, len, out needed) == unchecked((int)0xC0000004)) {
                Marshal.FreeHGlobal(buf);
                len = Math.Max(len * 2, needed + 65536);
                buf = Marshal.AllocHGlobal(len);
            }
            IntPtr cur = buf;
            while (true) {
                int nextOff = Marshal.ReadInt32(cur, 0);
                int numThreads = Marshal.ReadInt32(cur, 4);
                long procId = Marshal.ReadIntPtr(cur, 0x50).ToInt64();
                if (procId == pid) {
                    for (int i = 0; i < numThreads; i++) {
                        int tOff = 0x100 + i * 80;
                        long tid = Marshal.ReadIntPtr(cur, tOff + 0x30).ToInt64();
                        uint cs = (uint)Marshal.ReadInt32(cur, tOff + 0x40);
                        map[tid] = cs;
                    }
                    return map;
                }
                if (nextOff == 0) break;
                cur = IntPtr.Add(cur, nextOff);
            }
            return map;
        } finally {
            Marshal.FreeHGlobal(buf);
        }
    }
}
'@
if (-not ('FocusProcStats' -as [type])) {
    Add-Type -TypeDefinition $ctxCode
}

$dir = Join-Path $PSScriptRoot '..\docs\measurements'
New-Item -ItemType Directory -Force $dir | Out-Null
$csv = Join-Path $dir ("{0}-{1}.csv" -f (Get-Date -Format 'yyyy-MM-dd'), $Name)
if (-not (Test-Path $csv)) {
    'timestamp,private_mb,working_set_mb,cpu_seconds,threads,handles,io_write_bytes,ctx_switches_sec' | Out-File $csv -Encoding ascii
}

$end = (Get-Date).AddHours($Hours)
Write-Host "Writing to $csv until $end (Ctrl+C to stop early)"

$prevPid = $null
$prevThreads = $null
$prevTime = $null
$firstCpu = $null
$firstTime = $null
$lastCpu = $null
$lastTime = $null
$ctxSum = 0.0
$ctxCount = 0

while ((Get-Date) -lt $end) {
    $p = Get-Process focus -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $p) {
        Write-Host "$(Get-Date -Format T) focus.exe is not running; waiting..."
        $prevPid = $null
        $prevThreads = $null
        $prevTime = $null
        Start-Sleep -Seconds ([Math]::Min(10, [Math]::Max(1, $IntervalSeconds)))
        continue
    }
    $now = Get-Date
    $w = Get-CimInstance Win32_Process -Filter "ProcessId=$($p.Id)"
    $curThreads = [FocusProcStats]::ThreadContextSwitches($p.Id)
    $ctxRate = 0.0
    if ($prevPid -eq $p.Id -and $null -ne $prevThreads -and $null -ne $prevTime) {
        $dt = ($now - $prevTime).TotalSeconds
        if ($dt -gt 0.0) {
            $delta = 0L
            foreach ($kv in $curThreads.GetEnumerator()) {
                if ($prevThreads.ContainsKey($kv.Key)) {
                    $d = [int64]$kv.Value - [int64]$prevThreads[$kv.Key]
                    if ($d -gt 0) { $delta += $d }
                } else {
                    $delta += [int64]$kv.Value
                }
            }
            $ctxRate = $delta / $dt
            $ctxSum += $ctxRate
            $ctxCount += 1
        }
    }
    $prevPid = $p.Id
    $prevThreads = $curThreads
    $prevTime = $now

    $cpuVal = if ($null -ne $p.CPU) { [double]$p.CPU } else { 0.0 }
    if ($null -eq $firstCpu) {
        $firstCpu = $cpuVal
        $firstTime = $now
    }
    $lastCpu = $cpuVal
    $lastTime = $now

    $ci = [cultureinfo]::InvariantCulture
    $row = [string]::Format($ci, '{0},{1:F2},{2:F2},{3:F3},{4},{5},{6},{7:F2}',
        $now.ToString('s'),
        ($p.PrivateMemorySize64 / 1MB), ($p.WorkingSet64 / 1MB),
        $cpuVal, $p.Threads.Count, $p.HandleCount, $w.WriteTransferCount, $ctxRate)
    $row | Out-File $csv -Append -Encoding ascii
    Start-Sleep -Seconds $IntervalSeconds
}

if ($null -ne $firstTime -and $null -ne $lastTime -and ($lastTime - $firstTime).TotalSeconds -gt 0) {
    $elapsed = ($lastTime - $firstTime).TotalSeconds
    $avgCpuPct = (($lastCpu - $firstCpu) / $elapsed) * 100.0
    $avgCtx = if ($ctxCount -gt 0) { $ctxSum / $ctxCount } else { 0.0 }
    $ci = [cultureinfo]::InvariantCulture
    Write-Host ([string]::Format($ci, 'Done. avg_cpu_pct={0:F4}% avg_ctx_switches_sec={1:F2}', $avgCpuPct, $avgCtx))
} else {
    Write-Host 'Done.'
}
