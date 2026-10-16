# Runtime watchdog

`actingctl watchdog` (Workflow #374) starts the Runtime of an A/B installation again
when it is gone without a formal close: a crash, a closed console window, an end in
Task Manager, a reboot; and when its last start ended in an error, a held start that
stopped among them (Workflow #381 A, #378 A6a). It never starts a Runtime that was
closed formally, that is alive, that the installer is working on, or whose last start
ended in a fatal outcome; those stay down, and the fatal case stays down loudly. Task
Scheduler runs one tick a minute through `<root>\tools\actingwatch.exe`.

The watchdog reads the ledger never and writes it never. It adds no event type, no
fact and no persisted Runtime structure. It is a read-only consumer of `owner.lock`
(it never locks it), `runtime-info.json`, `install\active.json` (through the contract
reader) and the selected configuration's `state_root`. Its own memory is the
operational file `<root>\watchdog\state.json`, outside the ledger (below).

## Commands

All take `--root <install root>` and print one report on stdout (see "Exit codes").
They run before `--state-root` parsing, like `mcp-serve`; no other command changes.

| Command | Effect |
|---|---|
| `status` | Read-only: what `run-once` from the task would decide now, plus attention outcomes (below). It writes nothing and takes no lock but a momentary shared probe of `install\writer.lock`, only when a start would be considered. |
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
`active.json` is `watchdog_layout_unknown`: `status` and `run-once` answer
`watchdog_misconfigured` with it as the cause (row 1), and `install` refuses with it.
The state root is the selected config's `state_root` and must be absolute
(`watchdog_state_root_invalid`).

In v0.12.0 a single-location installation (one root without `install\`, whose Runtime
is started with `--config`) is not watched: the watchdog reports
`watchdog_layout_unknown` for it, and this is expected. Watching a single-location
(`--config`) root is Workflow #390 WD-1, planned for v0.12.1.

## Observation

1. **L**, `<state>\owner.lock`: a read refused by the owner's lock (os error 33) is
   *locked*; a read is *unlocked*; no file is *missing*.
2. **J**, the last complete owner record: a v1 or v2 record, or the checkpoint's
   `last_record` when the checkpoint is the last line. A torn tail is ignored. Any
   other schema is `watchdog_journal_unrecognised`. J carries `owner.lock`'s
   modification time. For row 6′ only, every owner's last record too (one per run of
   records of one epoch, the checkpoint's `last_record` included); a complete line
   that is no owner record is then `watchdog_journal_unrecognised`.
3. **Live owner**, with L locked only: the owner that `runtime-info.json` names
   answers a health request through the Runtime client. Health records nothing.
4. **F**, with L unlocked and a record only: among `actingd-*.log` in `<root>`, in
   `<root>\watchdog` and in the console's log directory
   `%LOCALAPPDATA%\ActingCommand\logs` (the task runs as the same user; actingd's
   fallback log lies there too) modified at or after J's time minus 2 s, the newest
   whose last 64 KiB hold a terminal outcome: the last `OUTCOME actingd:` or
   `OUTCOME acforward:` line that holds an outcome envelope (Workflow #378 A6a). A
   terminal outcome whose `install_root` names another installation is not this
   root's; one without `install_root` is every root's. A log without one is passed
   over for the next older log. actingd's last daemon line is its terminal outcome:
   `runtime_stopped` after a requested stop, `runtime_stopped` followed by the
   stopping condition after any other stop, or the refusal that ended a start (a held
   start that stopped among them).
5. Only when a start is considered:
   - **W**, `install\writer.lock` tried shared. Held exclusively means acsetup is
     working. A free lock stays held shared until the tick ends, so acsetup cannot
     enter between the check and the start ("writer is occupied", retryable). No file
     counts as free.
   - **Selection**: `install\active.json` read again under that shared lock; a
     different selection stands aside for one tick.
   - **P**, processes named `actingcommand-actingd.exe` whose executable path is under
     `<root>\` (case-insensitive), or whose path cannot be read, through a CIM query
     in a hidden Windows PowerShell. The same query reads the last boot time
     (`Win32_OperatingSystem.LastBootUpTime`) for the gap grace.

## Decision

First match wins.

Each decision is one registered code; its category sets the exit (Workflow #378 A5,
"Exit codes").

| # | Condition | Decision | run-once exit | Log, on change |
|---|---|---|---|---|
| 1 | layout, selection, config, state root, journal, logs or state file unreadable; root mismatch | `watchdog_misconfigured`, the failure as its `caused_by` link and `failure_code` | 1 | ERROR `watchdog_misconfigured` |
| 2 | L locked, the owner answers | `watchdog_alive` | 0 | INFO `watchdog_formal_start_observed` when the watchdog did not start it |
| 2′ | L locked, no answering owner (starting, `ledger-maintenance`, `unlock-owner`) | `watchdog_owner_lock_held` | 0 | INFO |
| 3 | no journal, or no record yet | `watchdog_never_started` | 0 | INFO |
| 4 | J of an unknown schema | `watchdog_misconfigured` (`watchdog_journal_unrecognised`) | 1 | ERROR |
| 4′ | J active with `resource_disposition: unconfirmed` (retention) | `watchdog_owner_retained_unconfirmed` | 1 | ERROR; next step: a formal start, whose startup releases a retained owner whose process has exited |
| 5 | F's terminal outcome is an error (a held start that stopped, a lost start race, a start refused with an error) | a start is considered: rows 7a-7f | as rows 7a-7f | as rows 7a-7f |
| 5′ | F's terminal outcome is fatal | `watchdog_fatal_hold`, the terminal outcome as its link | 2 | ERROR, once per log file |
| 6′ | J inactive, no candidate log written between its start and its close plus 2 s, and among the 16 owners before it the newest whose epoch a log covers (from its start to its close plus 2 s, or to the next owner's start) has a terminal outcome in the newest such log | rows 5 and 5′ for that terminal outcome | as those rows | as those rows |
| 6 | J inactive otherwise | `watchdog_formal_close` | 0 | INFO, once per epoch |
| 7a | W held | `watchdog_installer_busy` | 0 | INFO |
| 7a′ | selection changed | `watchdog_selection_changed` | 0 | INFO |
| 7b | P present, under 10 minutes | `watchdog_runtime_process_present` | 0 | INFO |
| 7b′ | P present for 10 minutes or more | `watchdog_runtime_process_without_owner` | 1 | ERROR |
| 7c | exhaustion recorded | `watchdog_budget_exhausted` or `panic_unrecovered` (see "Budget") | 1 or 2 | ERROR |
| 7d | task tick inside the gap grace | `watchdog_grace` | 0 | INFO |
| 7e | 3 starts in the last 30 minutes | as row 7c, recorded | 1 or 2 | ERROR |
| 7f | otherwise (J active: a crash, a closed window, Task Manager, a reboot) | start: `watchdog_started_runtime` or `watchdog_start_failed` | 0 or 1 | WARN `watchdog_started_runtime` or ERROR `watchdog_start_failed`, every attempt |

Row 5 precedes row 6 because a stop by a condition also writes `active=false`. A
refusal before the owner lock was taken leaves J untouched and is still newer than it.
Rows 5 and 5′ never see a terminal outcome of an earlier owner: every later owner
appended a record; only row 6′ looks back, past a close that no log covers. Rows 5
and 5′ read only the terminal outcome's category, never its code, operation or links
(Workflow #378 A6a): a fatal outcome holds, since only an external fix helps; an error
is a halt a restart or the person may fix, so the start rows, the budget among them,
decide as after a crash. A held start that stopped (`install_startup_stopped`,
`held_timeout`, `release_timeout`; Workflow #381 A R5′) and a lost start race
(`owner_conflict`) are errors like any other. A success, info or warning terminal
outcome leaves the journal rows to decide. Row 6′ (R5b) covers an installer that,
after such a stop, closed a maintenance owner without a log of its own: that later
record would otherwise hide the terminal outcome as a formal close. An owner that
closed formally ends in `runtime_stopped` (info), so it stays `watchdog_formal_close`.
A held start whose shutdown was accepted ends in `runtime_stopped` too, with
`install_startup_shut_down` (info, Workflow #378 A7-R) as its link, and actingd exits
0: a formal close (row 6) that the watchdog never restarts. A process probe that fails
is `watchdog_start_failed` caused by `process_probe_failed`.

### Upgrade-state close (Workflow #389 U-4; not before v0.12.x)

The installer's upgrade state (`runtime-interface.md`, section Upgrade state) closes
the Runtime for an upgrade, not as a stop. Its close is the one formal close the
watchdog brings back:

- The Runtime that accepts its own close in the upgrade state writes a marker on its
  owner-journal close record (an optional field of the owner record, named in
  `owner-journal.md`; the schema number does not change). A RequestShutdown close
  never carries it.
- A maintenance owner (`ledger-maintenance`) whose preceding record is a marked close
  copies the marker onto its own close, so the installer's cold ledger verification
  after the upgrade state does not hide it. The marker is used up when a
  non-maintenance Runtime owner starts; that owner's own close does not carry it.
- U-4 adds one row after rows 5, 5′ and 6′ and before row 6 (its number is U-4's):
  **J inactive and its close carries the upgrade-state marker → a start is
  considered: rows 7a-7f.** So a held installer (row 7a, `watchdog_installer_busy`),
  a changed selection (7a′) and the budget (7c, 7e) still apply; otherwise the
  selected generation starts.
- A held new Runtime has its own owner, so rows 2 and 2′ match first and the new row no
  longer does; a held start that times out is row 5 (an error); a new Runtime that
  stops with a fatal outcome after its release is row 5′.
- The installer holds `install\writer.lock` in every flow that closes the Runtime and
  starts it again, from before the close until after the start, so the new row never
  starts a Runtime the installer is still working on.
- It works only where the task is registered (`install`); acsetup never registers it,
  and on a machine without it the Runtime stays stopped as before. The watchdog still
  only reads `owner.lock`; the marker is written by the Runtime and the maintenance
  owner that copies it.

**Formal starts.** A live owner is the watchdog's own when a start record names its
epoch, or when a start whose result is still open (`watchdog_start_pending`,
`start_timeout`) spawned it: its recorded start lies in `[at − 2 s, at + 180 s]`.
Every other live owner is a formal start (acsetup, a logon script, an operator's
restart script, the console). This is decided again on every tick, from an answering
owner only. A formal start clears sticky exhaustion; starts before it no longer count.

**Budget.** At most 3 starts whose record lies within the last 30 minutes and after
the last formal start. The fourth would-be start records `exhausted_since` and every
tick then answers as row 7c, past the window, until a formal start. `run-once` reads
the terminal outcome of each start that ran the budget out from the log it gave that
start: when every one ended in `runtime_restart_required`, neither the Runtime nor the
person recovered it and the decision is `panic_unrecovered` (fatal); otherwise it is
`watchdog_budget_exhausted` (error) with the newest of those terminal outcomes as its
link (Workflow #378 A6a). `status` answers `watchdog_budget_exhausted` in both cases. A
`watchdog_fatal_hold` starts nothing, so a fatal terminal outcome never uses the
budget.

**Gap grace.** A task tick whose previous task tick ended more than 90 s earlier (one
and a half tick periods: a logon, a resume), or ended before the current boot (a
reboot; a scheduled reboot leaves gaps of only about 100-180 s), holds starts for
300 s, so that a logon starter can start the Runtime itself first: a logon script
that waits up to 180 s for readiness, acsetup's autostart. The boot time comes from
the same hidden query as P. The first tick ever has no grace, and a manual `run-once`
has none. In grace the watchdog still observes, and a Runtime process under the root
(row 7b) or an owner lock (row 2) keeps it standing aside after the grace too.

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
first is `exited_during_startup` (`log`, and the child's `exit_code` or the `cmd`'s
`pid`). No answer in time is `start_timeout` (`log`, `timeout_ms`); the process is
left running.

The start record is saved before the spawn, so a crash mid-start still counts. The
line on success:

`WARN watchdog_started_runtime generation=<g> method=<breakaway|wmi> owner_epoch=<e> pid=<actingd pid> log=<path> previous_epoch=<J epoch> previous_pid=<J pid> ready_ms=<n>`

On failure, `ERROR watchdog_start_failed` with `generation`, `method`, the failure as
`failure_code` and a `cause` field, `previous_epoch`, `previous_pid` and `last_line`:
the start log's terminal outcome, else its last line (developer text).

## Launcher

`<root>\tools\actingwatch.exe`, shipped in the Tools artifact (layout
`platform-tools-v3`), is the program Task Scheduler runs. It is a GUI-subsystem
program, so it opens no console window, and uses std only. It:

1. requires to be `<root>\tools\actingwatch.exe` itself, else exits 1
   (`watchdog_launcher_misplaced`);
2. opens `<root>\watchdog\watchdog.log` for appending, else (no directory) exits 1
   (`watchdog_launcher_log_unavailable`);
3. tries `install\writer.lock` shared: held by acsetup, it exits 0 and runs nothing;
   free, it keeps it for the tick, so acsetup cannot replace the launcher or the fixed
   entry meanwhile; a lock that cannot be probed is
   `watchdog_launcher_writer_lock_failed`, exit 1;
4. runs `<root>\runtime\actingctl.exe watchdog run-once --root <root> --from-task`
   with `CREATE_NO_WINDOW`, stdin and stdout NUL, stderr appended to the log and the
   working directory `<root>`, and exits with its code (0, 1 or 2); a fixed entry that
   cannot run, or ends without an exit code, is `watchdog_launcher_spawn_failed`, exit 1.

Its own failures are registered codes (Workflow #378 A5); from step 3 on, it appends
`ERROR actingwatch: <code> <path>: <error>` to the log. It reads no slot material and
takes no slot occupancy.

## Task

`install` registers a per-user task through `schtasks.exe` (actingctl has no COM
access: unsafe code is forbidden there).

- **Preconditions**, each refused with exit 1: an A/B installation
  (`watchdog_layout_unknown`); `<root>\runtime\actingctl.exe` and
  `<root>\tools\actingwatch.exe` exist
  (`watchdog_launcher_missing`: deploy the Runtime and Tools that carry the watchdog
  first); the selected config's state root resolves; acsetup's writer lock is free
  at that moment (`watchdog_install_refused` caused by `watchdog_installer_busy`).
- **State**: `<root>\watchdog\` is created. A valid `state.json` is kept, a document
  of another schema version is set aside as in "Files", an unreadable one is renamed
  `state.json.unreadable-<ms>`; each with a WARN `watchdog_state_reset` line. The end
  of the last task tick is set to the install time, so the first task tick opens no
  gap grace.
- **Name**: `ActingCommand Runtime watchdog <fp12>` in the root folder `\`, `fp12` the
  first 12 hex digits of SHA-256 over the plain install root without trailing
  separators, lowercased. The root folder takes a user's own task without elevation;
  a subfolder could need it.
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
  is `watchdog_task_register_failed` (exit 1), caused by schtasks' failure with its
  text.
- **Check**: the definition Task Scheduler holds (`Export-ScheduledTask`, below) must
  show the same command
  (case-insensitive), interval `PT1M`, logon type `InteractiveToken`, no run level
  other than `LeastPrivilege` (Task Scheduler omits the default) and no
  `<Enabled>false</Enabled>`; otherwise `watchdog_task_mismatch` (exit 1), with the
  `names` of what differs.
- The log gets `INFO watchdog_installed task=<name> user=<sid>`; `uninstall` writes
  `INFO watchdog_uninstalled task=<name>` when it deleted one.

The task is read through the ScheduledTasks module in a hidden Windows PowerShell that
answers in UTF-8 JSON (`schtasks` prints in the console code page, which need not be
UTF-8): `Get-ScheduledTask` in `\`, `Export-ScheduledTask` for the definition and
`Get-ScheduledTaskInfo` for the run state. `status` reports the definition, `status`
(the state's name: `Ready`, `Running`, `Disabled`), `last_run_time`, `last_result`
and `next_run_time`. A missing task is the attention outcome `watchdog_task_missing`,
one that is disabled or differs `watchdog_task_mismatch` (with the `names` of what
differs); a query that failed is `watchdog_misconfigured` in `attention`, and the
section carries it as `error`.

`uninstall` ends a tick in flight (`schtasks /End`; a start it already issued keeps
running), deletes the task (`schtasks /Delete /F`), then waits up to 200 s for
`run.lock` and reports `tick_in_progress` if a tick still holds it. When the name is
not registered it lists `other_watchdog_tasks` in `\`, so a root spelled differently
than at install can be found. `install` holds `run.lock` (for up to 30 s, else
`watchdog_install_refused` caused by `watchdog_run_in_progress`) while it rewrites
`state.json`.

Without the product, a registered task fails every minute (the launcher is gone,
0x80070002 in its last result): run `uninstall` before removing an installation.

## Files

`<root>\watchdog\`, created by `run-once` or `install` when missing:

| File | Content |
|---|---|
| `watchdog.log` | UTF-8 lines `<RFC3339 UTC> <unix_ms> <LEVEL> <code> key=value…`, written on a decision change, a start, an error; a decision line's fields are its outcome's values and one `cause` field per link, then developer fields (Workflow #378 A5). The launcher's own failures and the tick's stderr land here too; a task tick prints no per-tick summary there, and repeats a misconfiguration only when the last structured line is a different one. A quiet day writes nothing. |
| `state.json` | Operational memory across ticks, schema `actingcommand.runtime-watchdog-state.v2` (Workflow #378 A5): `root`, the last tick and task tick, `grace_until_unix_ms`, `last_decision` (`{code, values}`, the log-on-change key), `decision_since_unix_ms`, `exhausted_since_unix_ms`, `formal_start_at_unix_ms`, `process_present_since_unix_ms` and the last 10 `starts` (`at_unix_ms`, `method`, `log`, `start_result` `{code, values}`: `watchdog_start_pending` until the start ends, then `watchdog_started_runtime`, `start_timeout` or the failure's code; `generation`, `actingd_pid`, `owner_epoch`). Written to a temporary file, flushed to disk and renamed. Readers ignore unknown fields. `run-once` and `install` rename a document of another schema version (version 1 included) `state.json.other-schema-<ms>` with a WARN `watchdog_state_reset` line and replace it by a fresh state, so the budget restarts; a corrupt document is `watchdog_state_unreadable` (row 1). |
| `actingd-<unix_ms>.log` | stdout and stderr of each start. |
| `run.lock` | Held exclusively by a tick; a second concurrent tick answers `watchdog_run_in_progress` (exit 0). `actingctl start` and `restart` hold it through their start (Workflow #389 MCP-10). |
| `task.xml` | The definition `install` registered last. |

`state.json` is not a ledger structure and no Runtime component reads it (Workflow
#374, ruling M7: an operational file outside the ledger, recorded as a deviation note
on #374).

## Exit codes

Every subcommand, and the usage exit, prints one stdout line, the report
`actingcommand.watchdog.report.v2`: `{schema_version, outcome, attention, …data}`
(Workflow #378 A5). `outcome` is the top outcome's envelope: the decision for
`run-once` and `status`, the result for `install` and `uninstall`. `attention` lists
`status`'s attention outcomes and is empty for the others. The data members follow
(for example `live_owner.started_by_watchdog`, and `terminal {log, outcome}`, the
terminal outcome a decision read). A warning, error or fatal top also prints the
developer line `<LEVEL> actingctl: <code> …` on stderr; a task tick, whose stderr is
`watchdog.log`, prints it only for a misconfiguration it could not write there.

The process exits by the top outcome's category: 0 success, info or warning; 1 error;
2 fatal. `attention` never changes the exit. This replaces the exit codes 10–17 and
the launcher's 20–23; their conditions are outcome codes now.

| Exit | `run-once` and `status` |
|---|---|
| 0 | healthy or a deliberate stand-aside: `watchdog_alive`, `watchdog_owner_lock_held`, `watchdog_never_started`, `watchdog_formal_close`, `watchdog_installer_busy`, `watchdog_selection_changed`, `watchdog_runtime_process_present`, `watchdog_grace`; `run-once` also `watchdog_run_in_progress` and `watchdog_started_runtime`; `status` also `watchdog_start_due` (gone without a formal close, the next tick starts it) |
| 1 | `watchdog_misconfigured`, `watchdog_owner_retained_unconfirmed`, `watchdog_runtime_process_without_owner`, `watchdog_budget_exhausted`, `watchdog_start_failed` |
| 2 | `watchdog_fatal_hold`; `run-once` also `panic_unrecovered` |

`status`'s attention outcomes:

| Attention | When |
|---|---|
| `watchdog_task_missing` | the task is not registered |
| `watchdog_task_mismatch` | the task is disabled or differs from what `install` writes |
| `watchdog_owner_lock_unanswered` | the owner lock held for 10 minutes or more with no answering owner (an error-report dialog, a start that never publishes runtime-info) |
| `watchdog_last_start_failed` | no owner answers, and the last watchdog start since the last formal start ended other than `watchdog_started_runtime`; its result is the link |
| `watchdog_restarts_repeated` | 2 or more watchdog starts within 24 hours and since the last formal start |
| `watchdog_formal_close_unlogged` | no candidate log was written between the closed epoch's start and its close plus 2 s, so the terminal outcome of an unlogged start would be invisible |
| `watchdog_misconfigured` | the log directories for the close evidence, or the task, could not be read |

`install` exits 0 with `watchdog_installed`, else 1 with its failure; a refusal by an
info decision (acsetup's writer lock, a tick's run lock) is `watchdog_install_refused`
caused by it. `uninstall` exits 0 with `watchdog_uninstalled` (a `schtasks /End` that
failed is its `secondary` link), else 1. A usage error is `actingctl_usage_invalid`,
exit 1. The launcher exits 1 on its own failures (above) and passes the tick's exit
through.

## Starters and logs

The terminal outcome is read only from `actingd-*.log` files in `<root>`,
`<root>\watchdog` and `%LOCALAPPDATA%\ActingCommand\logs`. Every program that starts
the Runtime must write stdout and stderr to such a log (X3):

| Starter | Log |
|---|---|
| acsetup (start after install, upgrade, configuration) | `<root>\actingd-<ms>.log` |
| the console | `%LOCALAPPDATA%\ActingCommand\logs\actingd-<ms>.log` |
| a logon script | `<root>\actingd-autostart-<stamp>.out.log` and `.err.log` |
| an operator's WMI restart script | `<root>\actingd-wmi-<stamp>.log` |
| `actingctl start`, `actingctl restart` (Workflow #389 MCP-10: the manual start above, holding `run.lock`; while acsetup holds `install\writer.lock` both refuse as row 7a does, with `start_runtime_refused` caused by `watchdog_installer_busy`, `restart` before it stops anything; `restart --wait-idle` reads Status every 10 s while it waits for idle, since each Status records a ledger observation) | `<root>\actingd-<ms>.log` |
| the watchdog | `<root>\watchdog\actingd-<ms>.log` |
| acsetup's Startup-folder launcher | the UI's side: the UI major's launcher has acforward write its own and actingd's output to `<root>\actingd-autostart-<stamp>.log` (Workflow #378, model §16 A-3). A launcher start that writes no log is, for the Runtime, a hand start (next row). |
| a hand start in a terminal | actingd's fallback log, for a start refusal only: when its stderr is a console or absent, actingd also appends the refusal's daemon lines to `%LOCALAPPDATA%\ActingCommand\logs\actingd-<ms>.log`, with `install_root` once it resolved its installation (Workflow #378 A6a). A stop after the start has no log: it shows only as `watchdog_formal_close_unlogged` in `status`. |

The console's log directory, where actingd's fallback log lies too, is the user's, not
the root's: every root's watchdog reads it. A terminal outcome that names another
root's `install_root` is ignored (Workflow #378 A6a); actingd puts `install_root` on a
start refusal once it resolved its installation. Any other terminal outcome there, a
console-started Runtime's stop or a refusal before actingd resolved its installation,
also counts for the other root of a machine with two installation roots while it is
newer than that root's journal.

## Limits

- A crash that leaves an error-report dialog keeps the process, and its owner lock,
  alive until the dialog is dismissed; the watchdog reports `watchdog_owner_lock_held`,
  and `status` adds the attention `watchdog_owner_lock_unanswered` after 10 minutes. A
  hung but answering Runtime is `watchdog_alive`.
- Ending a Runtime is never a watchdog action. It starts no emulator, unlocks no
  owner and changes no selection.
- Switched back to a slot whose actingctl predates the watchdog, every tick fails
  with that actingctl's usage error (exit 1) until a newer slot is selected or the
  task is disabled.
- The Task Scheduler history of a per-minute task turns over fast; `watchdog.log` is
  the record, the history shows the last runs.
