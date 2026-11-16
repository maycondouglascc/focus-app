# Handoff: F2 (Base da UI), state on 2026-11-16

Read `AGENTS.md`, then `docs/gates.md` (criteria and decision rules), then this
file. Branch: `win32/f2-base-ui` (pushed; last commit `9420d34`). `main` is at
the F1 merge plus the process docs; tag `win32-gate-1` exists. Working tree was
clean at handoff.

## Done so far

Project level
- F0 and F1 finished. F0's last item (UI Automation cost on Chrome, Edge and
  Firefox) is decision `0003`: cheap enough, keep URL capture via UIA on its own
  thread, read only when the title changes. F1 merged to `main` without squash
  and tagged `win32-gate-1` (version 0.1.0).
- Autonomy docs: `docs/gates.md` (per-phase exit criteria), `AGENTS.md`
  (board commands, commit rules, hard design rules), `scripts/check.ps1`
  (fmt, clippy, tests; `-Release` adds the 5 MB binary check).
- Board: GitHub Project 5 "Focus Win32". The F2 item is `In Progress`. The F1
  "Merge na main e tag win32-gate-1" item was not moved by me: check it and mark
  it `Done` (the merge and tag exist).

F2 code (all under `win32/src/ui/`, 49 tests passing)
- `frame.rs`: pure geometry of the custom frame, `hit_test` for the title bar,
  caption buttons and resize borders at any DPI (Snap Layouts needs
  `HTMAXBUTTON`). Unit tested.
- `window.rs`: window created on open and destroyed on close, frameless client
  area, DPI per monitor v2, `WM_DPICHANGED`, caption button hover and click,
  paint through the renderer, rebuild after device loss on the next paint.
- `render.rs`: Direct3D 11 + DirectComposition + Direct2D surface. Everything
  device-level is dropped when the window closes; the DirectWrite text system
  lives for the process (see decision 0004 for why).
- `text.rs`: DirectWrite factory, text-format cache, `measure()` that works
  without a window (used by layout and tests).
- `tree.rs`: retained widget tree with taffy flexbox layout in DIPs, text leaves
  measured with DirectWrite, `rect()` gives absolute window coordinates. Style
  shorthands in `tree::style`.
- `chrome.rs`: title bar laid out with the tree (title left, room for the caption
  buttons on the right, relayout only when the width changes).
- `theme.rs` (color tokens), `strings.rs` (all user-visible strings),
  `selftest.rs` (`focus.exe --ui-cycle N` leak test, `--ui-show SECONDS`).

Decisions made along the way
- `0004`: Direct3D/Direct2D memory stays resident after the window closes, about
  21 to 27 MB private after the first open. The user chose to keep one process
  and accept it. Gate 2's ceiling is **30 MB** after the window has been opened,
  flat after a 100-open warm-up (growth under 1 MB). Before the first open the
  8 MB background budget still holds.
- Tried and rejected: keeping the D3D/D2D/DComp devices alive between windows
  (flat but 52 MB); `Flush`, `ClearState`, DComp `Commit` and `IDXGIDevice3::Trim`
  did not help. Memory differs about 5 MB between identical runs (driver/GPU
  state), so never judge by a single run; compare within one run or use several.

## Pending for gate 2 (all of `gates.md`, "F2")

Not started:
1. Pointer and keyboard input: use `WM_POINTER*` (mouse, touch, pen on one path),
   accumulate `WM_MOUSEWHEEL`/`WM_MOUSEHWHEEL` with fractional deltas; hit
   testing against the tree; capture (`SetCapture`) support for later drags.
2. Focus model: tab order, visible focus ring, Esc closes popups. Needs tests on
   the focus model (pure logic, no window).
3. Accessibility with `accesskit` (Windows adapter): every widget publishes role,
   name and state; add a test that walks the tree and fails on a widget with no
   name. The plan says accesskit comes with the first widget, so do this together
   with the first real widget.
4. Popups (select, popover, date picker): `WS_POPUP` windows, reposition with
   `MonitorFromPoint` and the monitor work area, close on lost activation.
5. Widgets, in the plan's order, **only the ones a screen needs**: button, toggle,
   segmented, select, text field (borderless `RICHEDIT50W` child, canvas draws the
   border, spellcheck on), dialog, popover, table, virtual list, chart
   primitives. Close and document the list in `docs/decisions/` before starting F3.
6. Device loss test: force `D2DERR_RECREATE_TARGET`/`DXGI_ERROR_DEVICE_REMOVED`
   and assert the renderer is recreated and draws again. The recovery code
   exists (`window.rs` `on_paint`, `render.rs` `DrawError::DeviceLost`) but no
   test forces it.
7. Windows text-size setting: read `UISettings.TextScaleFactor` (WinRT) and
   apply it to fonts. Also high contrast (`SPI_GETHIGHCONTRAST`), reduced motion
   (`SPI_GETCLIENTAREAANIMATION`), Mica backdrop with the solid-color fallback.
8. Measurements for the gate, committed as CSV in `docs/measurements/`: window
   open and idle (< 60 MB working set, 0% CPU without animation), cold open to
   first frame (< 200 ms; `--ui-cycle` prints the first-open time, about 55 to
   145 ms so far, it includes more than the first frame), 6 DPI cases (100, 150,
   200%) with the manual note in the decision file.
9. Minimal state for later screens: owning the tree per screen, invalidation
   when content changes (only redraw when something changed, no continuous loop).
10. Gate ritual when everything passes: `docs/gates.md` "Gate ritual" (decision
    note `0005-gate-2.md`, bump to 0.2.0, merge `--no-ff`, tag `win32-gate-2`,
    push, create `win32/f3-telas`, set the board item `Done`).

Known loose ends in current code
- The caption buttons and the title are drawn but the window has no content
  below the title bar yet; `Leaf::Box`, `Tree::add_container` and most of
  `tree::style` are marked `#[allow(dead_code)]` until a screen uses them.
- `window.rs` handles close with `DestroyWindow`. F4 changes that to "stay in
  the tray"; do not change it earlier.
- The main app (`main.rs`) still never opens the window; only the `--ui-cycle` and
  `--ui-show` selftest flags do. Wiring the window to the tray and a hotkey is F4.
- Hover is tracked for the caption buttons only (non-client messages). Client-area
  hover needs `WM_POINTERUPDATE`/`TrackMouseEvent` once widgets exist.

## Practical gotchas on this machine

- A hung test process can leave `win32\target\release\focus.exe` locked
  (`Acesso negado` when building). Build release into another directory:
  `CARGO_TARGET_DIR=<scratchpad>/target cargo build --release`. Debug builds,
  tests and `check.ps1` use `win32/target/debug` and were not affected.
- Run the UI self-test with a hard timeout, from PowerShell:
  `Start-Process <exe> -ArgumentList "--ui-cycle",50 -RedirectStandardOutput <file> -PassThru -WindowStyle Hidden`
  then `WaitForExit(ms)` and stop only that PID. `cargo run` or a pipe through
  `tail` hides hangs.
- **Never** kill every `focus.exe` or run `taskkill /IM focus.exe`: the user's
  own recorder may be running. Match on the command line (`--ui-`) first.
- `--ui-cycle` exits with the graphics objects released (`reset_shared`); leaving
  the DComp device alive at process exit hangs the process.
- The repository is public: no window titles, URLs, databases, dumps, or details
  of this machine's security tooling in git. Screenshots are only for looking:
  capture just the window region and delete the file.
- Pre-commit hook runs fmt, clippy and tests when `win32/` Rust or TOML is staged.
  Float literals passed to taffy helpers need an explicit `_f32` suffix or clippy
  (`-D warnings`) fails.
- windows crate is 0.62: `GetSystemMetricsForDpi` is in `Win32::UI::HiDpi`;
  Direct2D points are `windows_numerics::Vector2`; the `windows-numerics` crate
  is a direct dependency.

## When to stop and ask the user

Same list as `gates.md`. Likely triggers in F2: accessibility cannot be done in
the toolkit (then the plan's fallback is Tauri/WebView2 with the Rust core), or
a widget would push private memory over the 30 MB ceiling.
