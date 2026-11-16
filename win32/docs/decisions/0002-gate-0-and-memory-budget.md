# 0002: Gate 0 passed on the monitor; memory budget raised

## Gate 0
The foreground monitor spike was measured for 2 h 50 min (170 samples, see
docs/measurements/2026-11-16-8h.csv, stopped early by decision):

- private memory 4.44 to 4.50 MB, flat; working set 6.5 to 7.3 MB
- CPU 0.012% of one core, 0 bytes written, 84 handles constant
- process survived Sophos Intercept X the whole time

Decision: pass. Memory and CPU targets are met with margin, so the plan does
not fall back to Tauri.

Not measured and moved to F1: UI Automation cost on Chrome/Edge. The plan's
fallback still applies (URL off, tab title only if it is heavy).

## Memory budget
The 5 MB private-memory target was set before any measurement. The monitor
alone uses 4.5 MB, and SQLite plus the UI Automation thread will add to it.
New targets with the window closed:

| Metric | Old | New |
|---|---|---|
| Private memory | < 5 MB | < 8 MB |
| Working set | < 10 MB | < 15 MB |

The rest of the budget table is unchanged. Memory must still be flat over an
8 h run; the number is a ceiling, the flat curve is the real requirement.
