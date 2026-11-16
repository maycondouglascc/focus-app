# Gates: exit criteria and decision rules

This file lets an agent (or a person) decide, without asking, whether a phase
is done and whether to move to the next one. Rule of thumb: **a criterion
passes only with evidence committed in the repo** (a test, a CSV in
`docs/measurements/`, or a note in `docs/decisions/`). "It looks fine" is not
evidence. Targets come from `docs/plan.md`, section "Orçamento de recursos".

Status of the board is the GitHub Project "Focus Win32" (number 5, owner
`maycondouglascc`). Phase and item order below mirror it.

## Decision procedure for every board item

1. Take the first item that is `Todo`, in board order, whose phase's previous
   gate has passed. Move it to `In Progress` (see `AGENTS.md` for the command).
2. Work in small commits on the phase branch (`AGENTS.md`, "Commits").
3. When the item's **Done when** list is fully true, run
   `win32/scripts/check.ps1` (add `-Release` for gate items), commit the
   evidence, set the item to `Done`.
4. When every item of the phase is `Done`, evaluate the phase's **Gate**
   section. All **Pass** conditions true: do the gate ritual and continue.
   Any **Fail** condition true: follow that phase's **If it fails** rule.
   Anything neither clearly passing nor failing: **stop and ask the user**.
5. Never skip a gate, never lower a target to pass, never delete a failing
   measurement. A target may change only through a new
   `docs/decisions/NNNN-*.md` that states the measured number and why.

## Gate ritual (all gates)

1. `win32/scripts/check.ps1 -Release` passes on the phase branch.
2. Evidence committed; a decision note `docs/decisions/NNNN-gate-N.md` records
   pass/fail with the numbers.
3. Export the plan if it changed: `docs/plan.md` (remove any line describing
   this machine's security tooling: the repository is public).
4. Bump `version` in `win32/Cargo.toml` (0.2.0 at gate 2, 0.3.0 at gate 3,
   0.4.0 at gate 4, 1.0.0 at the end of F5), commit with `Cargo.lock`.
5. `git checkout main`, `git merge --no-ff win32/<phase-branch>` (no squash),
   `git tag win32-gate-N`, `git push origin main <phase-branch> win32-gate-N`.
6. Create the next phase branch from `main`.
7. Set the gate item on the board to `Done`.

Pushing to `origin` and tagging are pre-authorised for gates **only when every
Pass condition is true**. Anything else that leaves the machine (new repos,
releases, issues on other repos, installing software) needs the user.

## When to stop and ask the user (all phases)

- Anything needs administrator rights, installing software, or an exception
  from IT (for example the Build Tools, or the antivirus quarantining the exe).
- A Pass condition cannot be measured on this machine.
- Two attempts at the same failing criterion did not move the number.
- A change would alter scope decisions in `docs/decisions/0001-scope.md`
  (Windows 11 x64 only, own machine only, English and light theme only, no
  browser extension, no installer or signing).
- Anything would put real window titles, URLs or database files in git.

## F1: Núcleo (gate 1, passed 2026-11-16, tag `win32-gate-1`)

Done. Monitor, database, idleness, Store apps and friendly names, Claude Code
sessions, UI Automation cost (decision 0003).

## F2: Base da UI (toolkit Direct2D, layout, acessibilidade) → gate 2

Branch `win32/f2-base-ui`. Goal: a small retained-mode toolkit able to host
every F3 screen, measured against the resource budget before any screen is
built. Build the pieces in the plan's order: window and title bar, DirectComposition
surface, DirectWrite text, tree and layout (taffy), pointer and keyboard input,
focus, accessibility (accesskit), popups, then widgets.

Done when (all, each with a test or a measurement):
- Window is created on open and fully destroyed on close; opening and closing
  it 50 times in an automated test keeps private memory under 25 MB and
  growing by less than 1 MB (decision 0004: the graphics stack keeps about
  21 MB after the first open; CSV in `docs/measurements/`).
- Per-monitor DPI v2: layout in DIPs, correct at 100%, 150%, 200% (unit tests
  on layout plus a manual note in the decision file).
- Custom title bar keeps Snap Layouts (`HTMAXBUTTON`), drag, double click and
  system menu.
- Keyboard: tab order, visible focus ring, Esc closes popups; covered by tests
  on the focus model.
- Accessibility: every widget publishes role, name and state through accesskit;
  a test walks the tree and fails on a widget without a name.
- Device loss (`D2DERR_RECREATE_TARGET`, `DXGI_ERROR_DEVICE_REMOVED`) recreates
  resources; covered by a test that forces it.
- Widget list closed and documented in `docs/decisions/` before F3: button,
  toggle, segmented, select, text field (RichEdit child), dialog, popover,
  table, virtual list, chart primitives. No widget without a screen that uses
  it.
- All user-visible strings live in one module; colors are tokens.
- Window open and idle: working set < 60 MB, CPU 0% without animation (CSV).
- Window closed: monitor targets from F0 still hold (private < 8 MB,
  working set < 15 MB, decision 0002).
- Cold open to first frame < 200 ms (log timestamp, CSV).

Gate 2 **Pass**: all of the above, plus `check.ps1 -Release` passes.
**If it fails**: if the failing metric is only resource use and one fix attempt
(smaller caches, lazy init, fewer swap chain buffers) does not recover it, or
accessibility cannot be done in the toolkit, write the decision note and **stop
and ask the user**: the plan's fallback is to host the UI in Tauri/WebView2
while keeping the Rust core. Do not start F3 on a toolkit that failed.

## F3: Telas (calendário, atividade, relatórios, projetos, configurações) → gate 3

Branch `win32/f3-telas`. Port behaviour from the Electron app (still in
`src/`, untouched until the end of F5) screen by screen. Order: settings and
welcome (smallest, exercises widgets), projects, timer bar and entry editor,
weekly calendar, activity popover, activity screen, reports, project report,
CSV export, demo mode.

Done when, per screen:
- Behaviour matches the Electron screen for the cases in its component tests
  (the overlap algorithm in `layout.ts` is ported with the same test cases).
- Works with keyboard only and publishes an accessibility tree (test).
- Resource use with that screen open stays inside the F2 window budgets.
- No screen-specific code path loads data beyond the visible range (query by
  range, virtual lists).

Done when, for the phase:
- All six screens plus tray-independent flows work against a real database and
  against `--demo` (in-memory, generated data; screenshots allowed only from
  demo mode).
- Calendar drag: 60 fps at 4K and 200% in a diagnostic build (frame counter
  output committed); Esc cancels, Alt disables snap, autoscroll works.
- CSV export opens correctly in Excel pt-BR (UTF-8 BOM, locale separators;
  unit test on the writer) and has the "standard CSV" option.
- Window memory after closing stays under 25 MB private and flat across 50
  open/close cycles (decision 0004), re-measured with all six screens opened.
- One-day side-by-side use against Electron is **not** required here (backlog).

Gate 3 **Pass**: all above plus `check.ps1 -Release`.
**If it fails**: fix and re-measure. If one screen alone blows its budget after
two attempts, write the decision note and stop and ask the user whether to
simplify that screen or accept a higher budget.

## F4: Build, bandeja e script de instalação → gate 4

Branch `win32/f4-build`. Goal: `Focus.exe` that runs unattended all day.

Done when:
- Tray icon with stopped / running / idle-paused states, flyout, context menu,
  optional mini timer; recreated on `TaskbarCreated`; icon sizes 16 to 48 px.
- Closing the window keeps the app in the tray; "Sair" exits; first-time
  notification via `NIF_INFO`.
- `--background` start; `HKCU\...\Run` entry managed from Settings, with real
  state read from `StartupApproved\Run`.
- `scripts/install.ps1`: asks the running app to quit, copies to
  `%LOCALAPPDATA%\Programs\Focus`, recreates the Run key, relaunches. Data stays
  in `%LOCALAPPDATA%\Focus`, never touched by the script. Idempotent: running it
  twice leaves the same state (test it).
- Panic hook plus `MiniDumpWriteDump` into `%LOCALAPPDATA%\Focus\crashes`; log
  rotates at 1 MB; neither is committed.
- Full resource budget table from the plan, **every row measured** on the
  release build and committed (CSV or decision note): closed-window private
  memory < 5 MB (agreed ceiling 8 MB, flat curve over 8 h) until the window
  is first opened, then < 25 MB (decision 0004), CPU < 0.05%,
  wakeups < 1/s, writes < 1 MB/h, start to tray < 150 ms, binary < 5 MB.
- 8 h run with the final exe, real use, Sophos not quarantining it
  (note any detection).

Gate 4 **Pass**: all of the above. **If it fails**: a metric over budget gets
one profiling pass (WPR/WPA) and one fix; still failing, stop and ask. An
antivirus block is never worked around in code: stop and ask the user to
request the exception from IT.

## F5: Uso diário e ajustes → release 1.0.0

Branch `win32/f5-uso`. Goal: trust the app for real work.

Done when:
- Used as the only tracker for 5 consecutive working days with the exe started
  by Windows; bugs found are board items or commits, not memory.
- Daily comparison with the Electron data model: totals per project per day
  reconcile within 1 minute for days that both recorded (write the comparison
  query as a test-free script in `scripts/`, commit only numbers).
- 8 h measurement repeated after the last code change: curve flat, targets met.
- No open bug that loses or corrupts recorded time.
- Weekly `PRAGMA quick_check` passes and the daily backup copy exists.

Pass: all of the above. Then: bump to 1.0.0, merge, tag `win32-gate-5`, push.
Removing the Electron app from the repository is **never** autonomous: ask.

## Backlog items

- "Comparar o Focus Win32 com o app Electron lado a lado por um dia": do it in
  F5; the output is a short `docs/decisions/` note listing behaviour
  differences with a keep/fix verdict for each.
- "Thread dedicada de banco e EcoQoS nas threads de fundo": do it when the DB
  work shows up in a profile or at the start of F4, whichever comes first.
  Done when: DB writes happen off the monitor thread (test with a slow
  writer), background threads set `ThreadPowerThrottling` and low memory
  priority, and the closed-window CPU and wakeup numbers are not worse.

## Keeping this file honest

When a criterion turns out to be wrong, change it in a commit that explains
what was measured. If a criterion is vague enough that two agents could
disagree on pass or fail, make it measurable before relying on it.
