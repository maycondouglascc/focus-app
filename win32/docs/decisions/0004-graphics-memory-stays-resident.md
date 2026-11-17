# 0004: Direct3D and Direct2D memory stays resident after the window closes

Status: **decided 2026-11-16: option 2** (keep one process, accept about 21 MB
private after the first time the window is opened, until the app restarts).
Gate 2's criterion became "under 30 MB (raised from 25, see addendum) and flat over 50 open/close cycles".
Before the window is first opened the background budget (8 MB) still holds.

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

## Addendum: DirectWrite, warm-up and what was tried
- DirectWrite adds about 3 MB (21 to 24 MB after the first open).
- Memory is not a per-open leak: 100 and 300 cycles both grew about 6.4 MB, a
  warm-up plateau over the first ~100 opens. `--ui-cycle N` therefore runs 100
  warm-up cycles first and measures growth after them (50 cycles: +0.25 MB,
  24.24 to 24.49 MB, budget 25 MB).
- Tried and rejected: keeping the Direct3D/Direct2D/DirectComposition devices
  alive across windows. Flat, but 52 MB resident instead of 24 MB. `Flush`,
  `ClearState`, a DirectComposition `Commit` and `IDXGIDevice3::Trim` did not
  change it. The devices are dropped on close; only the DirectWrite text
  system and its format cache live for the whole process (recreating it on
  every open grew memory).
- Margin to the 25 MB budget is about 0.5 MB. If a later widget needs more,
  raise the budget with a new note rather than trimming silently.

## Addendum 2: run-to-run noise, budget raised to 30 MB
With the layout tree added, five consecutive runs of the same release build
ended at 22.6, 25.0, 25.3, 27.2 MB (and 24.5 MB before the tree). The spread
of about 5 MB comes from driver and GPU state, not from our code, so a 25 MB
ceiling with 0.5 MB margin would fail at random. Private-memory ceiling after
the window has been opened: **30 MB**. The leak criterion is unchanged
(growth after the warm-up under 1 MB) because it compares within one run.
