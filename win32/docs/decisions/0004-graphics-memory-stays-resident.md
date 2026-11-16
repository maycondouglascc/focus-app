# 0004: Direct3D and Direct2D memory stays resident after the window closes

Status: **open, needs the user's decision** (gate 2 criterion "private memory
back under 5 MB after closing the window" fails as designed).

## Measurement
`focus.exe --ui-cycle N` (release build), private memory of the process. The
window itself is destroyed and every COM object released each cycle.

| Step kept alive per window | Private MB after closing |
|---|---|
| Nothing (process baseline) | 4.22 |
| D3D11 device only | 11.63 |
| D3D11 device and swap chain | 11.74 |
| plus Direct2D factory and device context | 21.12 |
| full renderer (plus DirectComposition) | 21.1 to 22.7 |

Repeating open and close 50 times adds only 0.03 to 0.46 MB: it is not a leak,
it is memory the graphics stack keeps after first use (driver user-mode heaps
and library state). Releasing our objects does not return it. Trimming the
working set would hide it, which the plan forbids (no `EmptyWorkingSet`).

## Options
1. **Show the UI in a short-lived second process** (`focus.exe --ui`, same
   binary). The background process stays near 4.5 MB; all graphics memory goes
   away when the window closes. Costs: the plan's "one process, no IPC" base
   decision changes; the two processes share the SQLite file (WAL allows
   concurrent readers and a writer) and a named event to ask the background
   process to reload settings or the running timer.
2. Keep one process and accept about 21 MB private after the first time the
   window is opened, until the app restarts. Update the budget accordingly.
3. Draw without Direct3D/Direct2D (GDI-based). Probably far less retained
   memory, but worse text and anti-aliasing and a different toolkit.

Recommendation: option 1.
