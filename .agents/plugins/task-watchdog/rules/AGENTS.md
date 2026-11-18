# Background Task Watchdog

Follow these rules whenever a shell command (`run_command`) runs in the background and returns a `task-*` ID:

1. **Always arm a watchdog timer before going idle.**
   - Unless you are continuing active work that will finish shortly, call `schedule` with:
     - `DurationSeconds`: `180` (3 minutes; use `240` for expected long Release LTO builds)
     - `TimerCondition`: `"<task-id>"` (so normal completion cancels the timer automatically with zero extra turns)
     - `Prompt`: `"Watchdog check for <task-id>: verify manage_task(status), log growth, and child process progress."`
   - Never wait indefinitely for a background task without an active `schedule` watchdog or an explicit user-requested sleep/run.

2. **Evaluate progress when a watchdog wakes up or a hook warns.**
   - Call `manage_task(Action: "status", TaskId: "<task-id>")` to inspect recent output, and check the task's log mtime/size and child processes (or run `powershell -NoProfile -ExecutionPolicy Bypass -File .agents/plugins/task-watchdog/scripts/check-tasks.ps1 -Report -TranscriptPath "<transcript>"`).
   - **3+ minutes (`>= 180 s`) with no new log output**: treat as suspicious. Check whether child processes (such as `cargo`, `rustc`, `link.exe`) are actively making progress or if a single process is spinning/blocked (e.g., `Get-Counter`, interactive prompt, retry loop).
   - **5+ minutes (`>= 300 s`) with no progress**, **dead/missing OS process**, or **obvious infinite loop** (repeating identical error/output): call `manage_task(Action: "kill", TaskId: "<task-id>")`, diagnose why it stalled, and fix or report the block.
   - **Healthy long-running build/test** (active log growth or active compiler/linker progress): arm one more `schedule` timer (`120`–`180 s`, `TimerCondition: "<task-id>"`).

3. **Never spin in a polling loop.**
   - Use `schedule` timers instead of `Start-Sleep` loops or repeated `manage_task(status)` calls inside the same turn.
