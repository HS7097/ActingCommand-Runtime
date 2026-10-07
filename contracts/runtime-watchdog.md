# Runtime watchdog

`actingctl watchdog` (Workflow #374) starts the Runtime of an A/B installation again
when it is gone without a formal close: a crash, a closed console window, an end in
Task Manager, a reboot. It never starts a Runtime that was closed formally, that is
alive, that the installer is working on, or whose last log ends in FATAL; those stay
down, and the FATAL case stays down loudly. Task Scheduler runs one tick a minute
through `<root>\tools\actingwatch.exe`.

The watchdog reads the ledger never and writes it never. It adds no event type, no
fact and no persisted Runtime structure. It is a read-only consumer of `owner.lock`
(it never locks it), `runtime-info.json`, `install\active.json` (through the contract
reader) and the selected configuration's `state_root`. Its own memory is the
operational file `<root>\watchdog\state.json`, outside the ledger (below).

## Commands

All take `--root <install root>` and print one JSON object on stdout. They run
before `--state-root` parsing, like `mcp-serve`; no other command changes.

| Command | Effect |
|---|---|
| `status` | Read-only: what `run-once` from the task would decide now, plus attention states (below). It writes nothing and takes no lock but a momentary shared probe of `install\writer.lock`, only when a start would be considered. |
| `run-once [--from-task]` | One tick. `--from-task` is passed only by the launcher; it selects the start method and enables the gap grace. |
| `install` | Registers or refreshes the task (below) and creates `<root>\watchdog\`. Idempotent. |
| `uninstall` | Deletes the task; every file under `<root>\watchdog\` stays. `was_registered` says whether there was one. The root need not be a working installation any more. |

When the process runs with an inherited installation (through the fixed entry
`<root>\runtime\actingctl.exe`), its root must be `--root`; otherwise
`watchdog_root_mismatch`.

## Layout

Only A/B installations qualify: `install\active.json` read by
`InstalledProcess::read_active`. The selection is the process's own
(`process_installation()`), else `active.json` for a direct run. A root without
`active.json` is `watchdog_layout_unknown`. The state root is the selected config's
`state_root` and must be absolute (`watchdog_state_root_invalid`).

## Observation

1. **L**, `<state>\owner.lock`: a read refused by the owner's lock (os error 33) is
   *locked*; a read is *unlocked*; no file is *missing*.
2. **J**, the last complete owner record: a v1 or v2 record, or the checkpoint's
   `last_record` when the checkpoint is the last line. A torn tail is ignored. Any
   other schema is `watchdog_journal_unrecognised`. J carries `owner.lock`'s
   modification time.
3. **Live owner**, with L locked only: the owner that `runtime-info.json` names
   answers a health request through the Runtime client. Health records nothing.
4. **F**, with L unlocked and a record only: among `actingd-*.log` in `<root>` and in
   `<root>\watchdog` modified at or after J's time minus 2 s, the newest whose last
   64 KiB hold a line starting with `FATAL actingd:` or `FATAL acforward:` that does
   not contain `owner_conflict` (a lost start race, not a fault). Both programs print
   FATAL only as their last act; a multi-line detail may follow the line.
5. Only when a start is considered:
   - **W**, `install\writer.lock` tried shared. Held exclusively means acsetup is
     working. A free lock stays held shared until the tick ends, so acsetup cannot
     enter between the check and the start ("writer is occupied", retryable). No file
     counts as free.
   - **Selection**: `install\active.json` read again under that shared lock; a
     different selection stands aside for one tick.
   - **P**, processes named `actingcommand-actingd.exe` whose executable path is under
     `<root>\` (case-insensitive), or whose path cannot be read, through a CIM query
     in a hidden Windows PowerShell.

## Decision

First match wins.

| # | Condition | Decision | run-once exit | Log, on change |
|---|---|---|---|---|
| 1 | layout, selection, config, state root, journal, logs or state file unreadable; root mismatch | `misconfigured` | 13 | ERROR `watchdog_misconfigured` |
| 2 | L locked, the owner answers | `alive` | 0 | INFO `watchdog_formal_start_observed` when the watchdog did not start it |
| 2′ | L locked, no answering owner (starting, `ledger-maintenance`, `unlock-owner`) | `owner_lock_held` | 0 | INFO |
| 3 | no journal, or no record yet | `never_started` | 0 | INFO |
| 4 | J of an unknown schema | `misconfigured` | 13 | ERROR |
| 4′ | J active with `resource_disposition: unconfirmed` (retention) | `owner_retained_unconfirmed` | 10 | ERROR; next step `actingd unlock-owner` |
| 5 | F present | `fatal_hold` | 10 | ERROR, once per log file |
| 6 | J inactive | `formal_close` | 0 | INFO, once per epoch |
| 7a | W held | `installer_busy` | 0 | INFO |
| 7a′ | selection changed | `selection_changed` | 0 | INFO |
| 7b | P present, under 10 minutes | `runtime_process_present` | 0 | INFO |
| 7b′ | P present for 10 minutes or more | `runtime_process_without_owner` | 15 | ERROR |
| 7c | exhaustion recorded | `budget_exhausted` | 11 | ERROR |
| 7d | task tick inside the gap grace | `grace` | 0 | INFO |
| 7e | 3 starts in the last 30 minutes | `budget_exhausted`, recorded | 11 | ERROR |
| 7f | otherwise (J active: a crash, a closed window, Task Manager, a reboot) | start: `started` or `start_failed` | 0 or 12 | WARN `watchdog_started_runtime` or ERROR `watchdog_start_failed`, every attempt |

Row 5 precedes row 6 because a FATAL exit also writes `active=false`. A FATAL before
the owner lock was taken leaves J untouched and is still newer than it. A FATAL of an
earlier owner cannot hold a later one: every later owner appended a record. A
process probe that fails is `start_failed` with `process_probe_failed`.

**Formal starts.** A live owner is the watchdog's own when a start record names its
epoch, or when a start whose outcome is still open (`pending`, `start_timeout`)
spawned it: its recorded start lies in `[at − 2 s, at + 180 s]`. Every other live
owner is a formal start (the logon script, acsetup, the coordinator's scripts, a
console). This is decided again on every tick, from an answering owner only. A formal
start clears sticky exhaustion; starts before it no longer count.

**Budget.** At most 3 starts whose record lies within the last 30 minutes and after
the last formal start. The fourth would-be start records `exhausted_since` and every
tick then answers `budget_exhausted`, past the window, until a formal start. A
`fatal_hold` starts nothing, so a configuration error never uses the budget.

**Gap grace.** A task tick that follows the end of the previous task tick by more
than 180 s (logon, reboot, resume) holds starts for 300 s, longer than the logon
script's 180 s readiness wait, so the logon script wins the race and the watchdog
never loses one into a FATAL `owner_conflict`. The first tick ever has no grace. A
manual `run-once` has none. In grace the watchdog still observes.

## Start

The fixed entry `<root>\runtime\actingcommand-actingd.exe --config <selected
config>`, working directory `<root>`, stdin NUL, stdout and stderr appended to
`<root>\watchdog\actingd-<unix_ms>.log` (created before the spawn). The child gets no
inherited installation selection: the fixed entry reads `install\active.json`, which
the shared writer lock keeps equal to the selection the tick read, and `--config`
names that selection's config. Paths handed to children are plain (no `\\?\`).

- **Task tick (`--from-task`)**: CreateProcess with `CREATE_NO_WINDOW |
  CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP | NORMAL_PRIORITY_CLASS`. The
  fixed entry has a console without a window, which actingd inherits; both leave the
  task's job, so ending the task cannot end them; both run at normal priority although
  the task runs below normal. Only when breakaway is denied (os error 5): WMI.
- **Manual run, and the fallback**: WMI `Win32_Process.Create` through a hidden
  Windows PowerShell, `ShowWindow = 0`, `PriorityClass = 32`, command line
  `"%ComSpec%" /d /s /c ""<entry>" --config "<config>" 1>>"<log>" 2>&1"`. The
  paths travel in environment variables, never in script text; a path holding one of
  `& | < > ^ % ! "` refuses with `watchdog_wmi_path_unsafe`. No handle of the caller
  reaches the Runtime, so a manual run never holds its caller's terminal.
- There is no third method: the Runtime is never started inside a job.

**Readiness**, polled every 500 ms for 180 s: L locked and an answering owner whose
recorded start is not older than the spawn minus 2 s. An exit of the started process
first is `exited_during_startup`, with the log's FATAL line or last line. No answer in
time is `start_timeout`; the process is left running.

The start record is saved before the spawn, so a crash mid-start still counts. The
line on success:

`WARN watchdog_started_runtime generation=<g> method=<breakaway|wmi> pid=<actingd pid> owner_epoch=<e> log=<path> previous_epoch=<J epoch> previous_pid=<J pid> ready_ms=<n>`

## Launcher

`<root>\tools\actingwatch.exe`, shipped in the Tools artifact (layout
`platform-tools-v3`), is the program Task Scheduler runs. It is a GUI-subsystem
program, so it opens no console window, and uses std only. It:

1. requires to be `<root>\tools\actingwatch.exe` itself, else exits 20;
2. opens `<root>\watchdog\watchdog.log` for appending, else (no directory) exits 21;
3. tries `install\writer.lock` shared: held by acsetup, it exits 0 and runs nothing;
   free, it keeps it for the tick, so acsetup cannot replace the launcher or the fixed
   entry meanwhile; a lock that cannot be probed is exit 23;
4. runs `<root>\runtime\actingctl.exe watchdog run-once --root <root> --from-task`
   with `CREATE_NO_WINDOW`, stdin and stdout NUL, stderr appended to the log and the
   working directory `<root>`, and exits with its code; a fixed entry that cannot run
   appends `ERROR actingwatch: cannot run <path>: <error>` and exits 22.

It reads no slot material and takes no slot occupancy.

## Task

`install` registers a per-user task through `schtasks.exe` (actingctl has no COM
access: unsafe code is forbidden there).

- **Preconditions**, each refused with exit 13: an A/B installation;
  `<root>\runtime\actingctl.exe` and `<root>\tools\actingwatch.exe` exist
  (`watchdog_launcher_missing`: deploy the Runtime and Tools that carry the watchdog
  first); the selected config's state root resolves; acsetup's writer lock is free
  at that moment (`watchdog_installer_busy`).
- **State**: `<root>\watchdog\` is created. A valid `state.json` is kept, a document
  of another schema version is set aside as in "Files", an unreadable one is renamed
  `state.json.unreadable-<ms>`; each with a WARN line. The end of the last task tick
  is set to the install time, so the first task tick opens no gap grace.
- **Name**: `ActingCommand Runtime watchdog <fp12>` in the root folder `\`, `fp12` the
  first 12 hex digits of SHA-256 over the plain install root, lowercased. Alice's
  tasks live in `\`; a subfolder could need elevation.
- **Principal**: the current user's SID (`whoami /user`), `InteractiveToken`,
  `LeastPrivilege`. No password is stored and no elevation is needed.
- **Definition**, written to `<root>\watchdog\task.xml` as UTF-16LE with a BOM and
  `encoding="UTF-16"`: a `TimeTrigger` from the install time (UTC) repeating every
  `PT1M` with no duration; `IgnoreNew`; no battery conditions; `StartWhenAvailable`
  false; `ExecutionTimeLimit` `PT5M` with hard termination allowed; not hidden;
  priority 7; no `RestartOnFailure` (a failure stays visible, and the next minute
  retries anyway); action `<root>\tools\actingwatch.exe` with the working directory
  `<root>` and no arguments. The description names the root and how to remove it.
- **Registration**: `schtasks /Create /TN <name> /XML <task.xml> /F`; a non-zero exit
  is `watchdog_task_register_failed` (13) with schtasks' text.
- **Check**: `schtasks /Query /TN <name> /XML` must show the same command
  (case-insensitive), interval `PT1M`, logon type `InteractiveToken`, no run level
  other than `LeastPrivilege` (Task Scheduler omits the default) and no
  `<Enabled>false</Enabled>`; otherwise `watchdog_task_mismatch` (14).
- The log gets `INFO watchdog_installed task=<name> user=<sid>`; `uninstall` writes
  `INFO watchdog_uninstalled task=<name>` when it deleted one.

`status` reads the task read-only: the same `/Query /XML` check, and from `/Query /FO
CSV /V /NH` the columns 4, 6 and 7 (status, last run time, last result) raw, since
Task Scheduler localises them. A missing task (`task_missing`), or one that is
disabled or differs (`task_mismatch`), is attention 14.

Without the product, a registered task fails every minute (the launcher is gone,
0x80070002 in its last result): run `uninstall` before removing an installation.

## Files

`<root>\watchdog\`, created by `run-once` or `install` when missing:

| File | Content |
|---|---|
| `watchdog.log` | UTF-8 lines `<RFC3339 UTC> <unix_ms> <LEVEL> <code> key=value…`, written on a decision change, a start, an error. The launcher's own failures and the tick's stderr land here too. A quiet day writes nothing. |
| `state.json` | Operational memory across ticks, schema `actingcommand.runtime-watchdog-state.v1`: `root`, the last tick and task tick, `grace_until_unix_ms`, `last_decision` (the log-on-change key), `decision_since_unix_ms`, `exhausted_since_unix_ms`, `formal_start_at_unix_ms`, `process_present_since_unix_ms` and the last 10 `starts` (`at_unix_ms`, `method`, `log`, `outcome`, `generation`, `actingd_pid`, `owner_epoch`). Written to a temporary file and renamed. Readers ignore unknown fields. A document of another schema version is renamed `state.json.other-schema-<ms>` with a WARN line and replaced by a fresh state; a corrupt document is `watchdog_state_unreadable` (13). |
| `actingd-<unix_ms>.log` | stdout and stderr of each start. |
| `run.lock` | Held exclusively by a tick; a second concurrent tick answers `run_in_progress` (exit 0). |
| `task.xml` | The definition `install` registered last. |

`state.json` is not a ledger structure and no Runtime component reads it (coordinator
ruling on #374, M7: an operational file outside the ledger, recorded as a deviation
note on #374).

## Exit codes

| Code | `run-once` | `status` |
|---|---|---|
| 0 | healthy or a deliberate stand-aside: `alive`, `owner_lock_held`, `never_started`, `formal_close`, `installer_busy`, `selection_changed`, `runtime_process_present`, `grace`, `run_in_progress`, `started` | none of the attention states below |
| 10 | `fatal_hold`, `owner_retained_unconfirmed` | the same |
| 11 | `budget_exhausted` | the same |
| 12 | `start_failed` | `start_due` (gone without a formal close, the next tick starts it), or the last start since the last formal start failed |
| 13 | misconfigured | the same |
| 14 | — | `task_missing` or `task_mismatch`: the task is not registered, is disabled, or differs from what `install` writes |
| 15 | `runtime_process_without_owner` | the same |
| 16 | — | `repeated_restarts`: 2 or more watchdog starts within 24 hours and since the last formal start |
| 17 | — | `formal_close_unlogged`: no candidate log was written between the closed epoch's start and its close plus 2 s, so a FATAL of an unlogged start would be invisible |

`status` lists every attention state under `attention` and exits with the first of
13, 10, 11, 15, 12, 14, 16, 17. `install` exits 0, 13 or 14; `uninstall` 0 or 13. Usage errors exit 1. The launcher adds 20, 21, 22 and 23
(above) and passes every other code through.

## Starters and logs

FATAL detection sees only `actingd-*.log` files in `<root>` and `<root>\watchdog`.
Every program that starts the Runtime must write stdout and stderr to such a log
(X3):

| Starter | Log |
|---|---|
| acsetup (start after install, upgrade, configuration) | `<root>\actingd-<ms>.log` |
| Alice's logon script | `<root>\actingd-autostart-<stamp>.out.log` and `.err.log` |
| the coordinator's `restart_actingd.ps1` | `<root>\actingd-wmi-<stamp>.log` |
| the watchdog | `<root>\watchdog\actingd-<ms>.log` |
| acsetup's Startup-folder launcher, a hand start in a console | none: a FATAL shows only as `formal_close_unlogged` in `status` |

## Limits

- A crash that leaves an error-report dialog keeps the process, and its owner lock,
  alive until the dialog is dismissed; the watchdog reports `owner_lock_held`. A hung
  but answering Runtime is `alive`.
- Ending a Runtime is never a watchdog action. It starts no emulator, unlocks no
  owner and changes no selection.
- Switched back to a slot whose actingctl predates the watchdog, every tick fails
  with that actingctl's usage error (exit 1) until a newer slot is selected or the
  task is disabled.
- The Task Scheduler history of a per-minute task turns over fast; `watchdog.log` is
  the record, the history shows the last runs.
