# Focus Win32: instructions for agents and contributors

Native Win32 rewrite of Focus in Rust (Windows 11 x64, one executable, this
machine only). The Electron/macOS app in the repository root is behavior
reference only; do not change it. Work only inside `win32/` unless told
otherwise.

Read in this order: `docs/gates.md` (what to do next and when to stop),
`docs/plan.md` (design and resource budget), `docs/decisions/` (why things are
the way they are).

## Finding the next task

The board is GitHub Project number 5, "Focus Win32", owner `maycondouglascc`.

```bash
gh project item-list 5 --owner maycondouglascc --limit 100 --format json
```

Take the first `Todo` item whose phase's previous gate has passed
(`git tag` lists `win32-gate-N`). `docs/gates.md` defines "done" for each item
and each gate. Change status (ids are stable):

```bash
# project id PVT_kwHODghAkM4Br425, Status field PVTSSF_lAHODghAkM4Br425zhqJcIw
# options: Todo f75ad846, In Progress 47fc9ee4, Done 98236657
gh project item-edit --project-id PVT_kwHODghAkM4Br425 --id <item-id> \
  --field-id PVTSSF_lAHODghAkM4Br425zhqJcIw --single-select-option-id <option-id>
```

If this fails with a missing scope, run `gh auth refresh -s project` (needs the
user) and continue with the repo work meanwhile.

## Commands

```bash
powershell -File win32/scripts/check.ps1            # fmt, clippy, tests
powershell -File win32/scripts/check.ps1 -Release   # plus binary size budget
cargo run --release --example uia_probe -- firefox.exe 30 60 30   # UIA cost
powershell -File win32/scripts/measure.ps1 -Hours 8 # resource CSV of focus.exe
```

`.githooks/pre-commit` runs fmt, clippy and tests when `win32/` Rust or TOML is
staged. Enable once per clone: `git config core.hooksPath .githooks`. Never
bypass it.

## Commits

- One small commit per change that compiles and passes `check.ps1`.
- Subject in English, imperative ("Add foreground window hook"); the body says
  why and, when relevant, measurements before and after.
- Phase branches: `win32/f2-base-ui`, `win32/f3-telas`, `win32/f4-build`,
  `win32/f5-uso`, cut from `main`. WIP commits only on a phase branch, subject
  starting `WIP:`.
- Agent-authored commits end with a `Co-Authored-By:` trailer for the agent.
- Push the phase branch at the end of each work session.
- Gates: merge into `main` with `--no-ff` (never squash), tag `win32-gate-N`.
  Procedure in `docs/gates.md`.

## Never in git

Real window titles, URLs, app names from the user's own use, databases
(`*.db`, `-wal`, `-shm`), crash dumps, `Focus.exe`, `win32/target/`. Tests use
in-memory databases and synthetic data. Measurement CSVs contain numbers only.
The repository is public: do not write details of this machine's security
setup or the user's employer into the repo.

## Hard rules from the design

- No `SetWindowsHookEx`, no global Raw Input, no UPX, no `EmptyWorkingSet`
  tricks. Monitor is event driven (`SetWinEventHook`, out of context).
- UI Automation: query the browser UI tree with chained `TreeScope_Children`,
  never `FindAll` on a subtree, never subscribe to UIA events, never read the
  page `Document`. Read the address bar only when the title changes, on its
  own thread (latency up to ~200 ms, decision 0003). Chrome and Firefox return
  the address without the scheme.
- Everything with a window, Direct2D or accessibility exists only while the
  window is open; the background footprint is the monitor, Claude and DB
  threads only.
- Every `unsafe` block gets a one-line comment stating the invariant it relies
  on.
- Resource targets are ceilings, the flat memory curve is the real requirement.

## Stop and ask the user

See "When to stop and ask the user" in `docs/gates.md`. In short: admin rights,
installing software, antivirus or IT exceptions, scope changes, unmeasurable
criteria, and anything that would put personal data in git.

## Handoff

Leave the repo in a state the next agent can continue from: committed, pushed,
board item status updated, and any half-finished reasoning written down in a
`docs/decisions/` note instead of in chat.
