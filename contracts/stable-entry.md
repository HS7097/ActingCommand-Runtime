# Stable entries of an A/B root (`actingcommand-entry`)

Workflow #429 R-15b (since 0.12.35). The frozen design is `rd-1031/design-r15b-ab-v4.1.md`
(sha256 `36ed6a98…2eaca`), §3.13, §6.1, §6.2 and §6.4, frozen in #429 comment 6541651601 on
Alice's rulings 131 and 132 A (#429 comment 6539887801). This document is the contract; where it
abbreviates, the design section named in brackets decides.

**Status.** R-15b freezes this contract and registers the entries' codes under the owner
`actingcommand-entry` (`contracts/outcome-codes/actingcommand-entry.json`, the workspace member
`apps/actingcommand-entry`). R-15c builds the two programs and ships them in the Tools half
(`tools-layout` 4); R-15d places and refreshes them; R-15e places them at a conversion. Until
then no build carries them, a production root stays single-location and its `runtime\` holds the
real programs. The UI's `acforward` retires with UI-2e1.

## What an entry is

An A/B root ([install-cli.md](install-cli.md), "The A/B layout") keeps, outside its two slots,
fixed program names that never move:

- `<root>\runtime\<name>.exe`: a copy of the **console entry** `actingcommand-entry.exe`, one per
  program the Runtime member's manifest lists (`actingcommand-actingd.exe`, `actingctl.exe`, …);
- `<root>\ui\<name>.exe`: a copy of the **graphical entry** `actingcommand-entry-gui.exe`, one per
  program the UI member's manifest lists, and after a conversion one per name the old `ui\`
  programs had (design §5.5).

An entry forwards to `<root>\<slot>\<dir>\<name>.exe` of the selection `install\active.json`
names. It is written anew in the Runtime repository under AGPL, uses the standard library only
and forbids unsafe code, like the launcher `actingwatch.exe` (no GPL code of the UI repository is
copied). Shortcuts point at `<root>\ui\<name>.exe` and are never tracked by the installer.

## Routing (design §6.1)

- **By folder, never by a list of names.** The entry's own folder `<dir>` is `runtime` or `ui`
  and its own file name is `<name>.exe`. Anything else, or a root that is no A/B root, is
  `entry_layout_unknown`.
- **Only a listed program.** The entry forwards only when the selected member's
  `BUILD-MANIFEST.json` (the Runtime member for `runtime\`, the UI member for `ui\`) lists
  `<name>.exe`; otherwise `entry_target_not_in_member` (`entry`, `install_member`,
  `commit_sha`). A member that drops a program, or a rollback to a version without it, therefore
  answers a code instead of looking for a missing file.
- **The target checked.** Before it starts the target the entry checks its size and SHA-256
  against the manifest: `entry_target_modified` (`path`, `expected_sha256`, `actual_sha256`)
  otherwise.
- `tools\` has no entries: a tool is started by a process that has fixed its selection, with
  `pin_child`, from its own slot.

## Arguments, environment and the selection (design §6.1, §3.13)

- **Arguments pass unchanged.** No `--state-root`, no `--root`, no `--config` rewrite (acforward
  did all three): the configuration of an A/B root is `<root>\actingd.config.json`, and every
  caller passes what it needs. A program started without its arguments answers its own usage
  error.
- **No inherited selection:** the entry reads `install\active.json`, takes the selected slot's
  shared lock (`InstallSlotLock`) and hands the selection to its child.
- **An inherited selection equal to `install\active.json`** (the same bytes) passes on as it is.
- **An inherited selection that differs** refuses for every entry,
  `entry_selection_inherited_stale` (`inherited_sha256`, `active_sha256`). Every caller clears
  `ACTINGCOMMAND_INSTALL_ROOT` and `ACTINGCOMMAND_INSTALL_SELECTION` before it starts an entry
  ([install-cli.md](install-cli.md), "Start rules"), so a stale one can only be a caller that
  forgot: following it would hide the error.
- **`install\active.json` missing or unreadable**, or not install-selection v2:
  `entry_selection_unavailable` (`path`).
- **The actingd entry and an unfinished operation.** When `install\current.json` names an
  unfinished installer operation of a kind row 7a″ stands aside for (`update`, `rollback`, their
  `undo`, `convert`, `restart`; the pass list is `release_stage`, `adopt`, `stop`, `start`,
  `release_hold`, `prepare`, `clean`), the entry `runtime\actingcommand-actingd.exe` refuses to
  start the Runtime, `entry_operation_unfinished` (`install_operation_id`). Only the installer's
  own start passes: it puts `ACTINGCOMMAND_INSTALL_OPERATION=<op>` (the contract crate's
  `INSTALL_OPERATION_ENV`) in the environment, the entry checks it equals the operation
  `install\current.json` names and removes it from the child's environment. An old Startup-folder
  `ActingCommand.cmd` therefore cannot start the Runtime in the middle of an interrupted update.

## The two variants (design §6.1)

| | console `actingcommand-entry.exe` | graphical `actingcommand-entry-gui.exe` |
|---|---|---|
| placed at | `runtime\<name>.exe` | `ui\<name>.exe` |
| subsystem | console | windows (no console window) |
| waits | for the child to end; stdin, stdout and stderr pass through; exits with the child's exit code. The watchdog, the installer and the MCP entry rely on this | at most 10 seconds after the spawn (a fixed value of this contract): a child that exits non-zero within them is `entry_child_failed_early` (`child_pid`, `child_exit_code`); a child still running, or one that exited 0, is `entry_launched` (`child_pid`, `child_state`, `child_exit_code`) |
| exit code | the child's, on success; its own code's category on its own failure (0 success or info, 1 error, 2 fatal) | its code's category: `entry_launched` 0; a refusal that started nothing (`entry_selection_unavailable`, `entry_selection_inherited_stale`, `entry_target_not_in_member`, `entry_target_modified`, `entry_operation_unfinished`, `entry_layout_unknown`) 1; `entry_spawn_failed` and `entry_child_failed_early` 2 |
| stdout | the child's; the entry writes nothing on success (`mcp-serve` speaks on it) | the child's stdio is null; the entry writes **one line** of JSON, on success and on failure, `{"outcome": {"code", "values"}}` (the `outcome` member of actingctl's result line); without a stdout the line is dropped, the exit code stays |
| its own failure | one stderr line `OUTCOME actingcommand-entry: <code> <values>` | the dialog below |
| `ACTINGCOMMAND_ENTRY_NO_DIALOG=1` | — | no dialog and no Notepad: only the exit code, the stdout line and the fixed log. The entry removes the variable from the child's environment (the contract crate's `ENTRY_NO_DIALOG_ENV`) |
| icon | none | the application icon, built in with `winresource` from an image in `distribution/windows/` |

`child_state` (vocabulary `entry_child_state`) is `running` when the child still ran after the 10
seconds and `exited` when it exited 0 within them; `child_exit_code` is the exit code then.

**The dialog** (graphical entry, design §6.1 and §3.13). The entry first releases the selection
and the slot's shared lock, then shows a dialog through Windows PowerShell's
`System.Windows.Forms.MessageBox` (no unsafe code): the code, one plain sentence from the entry's
own small table of its codes (English and Chinese), the log's place, and, when `install\installer.json`
names a `recovery_ui`, a button "open the recovery UI". PowerShell is started without the two
selection variables, and the button starts `recovery_ui.path` with `recovery_ui.argv`
(`--recovery --root <root>`) from that clean environment. Two time limits:

1. PowerShell writes the line `shown` to its stdout just before it shows the dialog; the entry
   waits for it at most 30 seconds from the spawn of PowerShell;
2. once `shown` is read, the entry waits at most 30 more seconds for the button and then exits
   with its code whether or not a button was pressed; the dialog stays, PowerShell handles a
   later press, and no Notepad opens (the person saw the dialog).

**The fallback.** When PowerShell does not start or does not write `shown` within its limit, the
entry writes the same content, in plain words with the recovery UI's whole command line, to
`install\entry-failure-last.txt` (UTF-8 with a BOM, rewritten each time) and opens it with
`%SystemRoot%\System32\notepad.exe`. When Notepad does not start either nothing more is tried; the
failure is in the exit code, the stdout line and the fixed log all the same.

## The fixed log (design §6.1, §3.12)

On its own failure each variant appends one JSON line `{at_unix_ms, entry, variant, code,
values}` (`variant` of the vocabulary `install_entry_variant`: `console`, `gui`) to
`install\entry-failures.log`. The entry only appends and never renames; the next installer
operation that takes the writer lock renames a log over 1 MiB to `entry-failures.log.old-<ms>`.
`install status` lists the lines by a cursor that carries the file's code, so a rotation is
reported, not read past ([install-cli.md](install-cli.md), "The read face"). The rotated files and
`install\entry-failure-last.txt` are records: the clean never deletes them and a conversion's name
set leaves them out.

The result line prefix `OUTCOME actingcommand-entry:` is registered here; the watchdog's
observation F reads it beside `OUTCOME actingd:` from R-15c on
([runtime-watchdog.md](runtime-watchdog.md), "Observation" 4), which closes the gap acforward
left: its plain `FATAL acforward:` line was invisible to the watchdog.

## The identity query `--entry-report` (design §6.1)

With `--entry-report` as its only argument an entry starts nothing and prints one JSON line (the
graphical one to the stdout its caller gives), then exits with `entry_report_done` (0):

```json
{"schema_version": "actingcommand.entry-report.v1",
 "entry": {"path": "<root>/ui/acui.exe", "variant": "gui", "sha256": "…", "tools_commit": "…"},
 "inherited": false,
 "selection": {"selection_id": "sel-…", "sha256": "…", "slot": "B", "generation": 4},
 "target": {"path": "<root>/B/ui/acui.exe", "member": "ui", "commit": "…", "listed": true, "sha256_ok": true},
 "outcome": {"code": "entry_report_done", "values": {}}}
```

It reads the selection and the manifest and hashes one file; it takes no writer lock and writes
no log. Only the case where it is the one argument counts, so a forwarded argument never meets
it (no program of either repository takes that name). Workflow #429 §3.1's "an entry reports the
members it really runs" is this query, and test E01's "the entry points at the right selection"
reads it.

## Refreshing an entry (design §6.2)

- Only an operation that changes the Tools member (so the Runtime too, with `--stop`) refreshes
  an existing entry (step `refresh_entries`); an update of the UI alone only adds the names its new
  UI member brings. Entries only move forward: a rollback and an automatic rollback leave them.
- Each changed entry: its new bytes to `<entry>.new-<ms>.part`, flushed, SHA-256 checked. When
  the entry does not run (CIM image path), the current bytes are copied to `<entry>.old-<ms>`
  (create new, flushed, checked) and the part renamed over the entry in one rename: the name is
  never absent. When it runs, Windows cannot overwrite the image: the entry is renamed
  `<entry>.old-<ms>` and the part renamed in, the old forwarder running on; between the two
  renames the name is absent for milliseconds, and a UI takes the path "an entry is absent"
  ([install-cli.md](install-cli.md), "Start rules"). Both paths have the fault points F2 and F14.
- **Names are only added.** A name once placed stays; a member without its program answers
  `entry_target_not_in_member`. An undo leaves a name its operation added, and the clean never
  deletes an entry or a forwarder's `.old-<ms>` copy.
- `install-state` v2 records each entry's SHA-256 and source commit (`entries`); `install status`
  compares the bytes and notes a difference as the caveat entry_modified, and a plan refuses with
  the refusal of that name ([install-cli.md](install-cli.md)).

## Interfaces (design §2.7, §6.4)

- The entries read install selection v2: their Tools build declares `install-selection` [2, 2]
  (party F of [component-interfaces.md](component-interfaces.md), "Checks per operation").
- The graphical entry's exit codes, its stdout line, `--entry-report` and the meaning of
  `ACTINGCOMMAND_ENTRY_NO_DIALOG` belong to the interface `install-cli` 1, written by F and read
  by the UI (UI-2d starts a new acui through `<root>\ui\acui.exe` with the variable set and judges
  the result by them). A change a UI of `install-cli` 1 cannot read raises `install-cli`.
- `tools-layout` 4 is the Tools layout that carries both entry programs.

### Registered codes and words (Workflow #429 R-15b)

Registered in `contracts/outcome-codes` (`since` 0.12.35, review pending); the gate
`gate_r15b_registry` checks every item against the catalog and the fragments.

- codes of `actingcommand-entry`: `entry_launched`, `entry_selection_unavailable`,
  `entry_selection_inherited_stale`, `entry_target_not_in_member`, `entry_target_modified`,
  `entry_operation_unfinished`, `entry_layout_unknown`, `entry_spawn_failed`,
  `entry_child_failed_early`, `entry_report_done`.
- vocabulary `install_entry_variant` (new): `console`, `gui`.
- vocabulary `entry_child_state` (new): `running`, `exited`.
- keys: `child_pid`, `child_state`, `child_exit_code`, `inherited_sha256`, `active_sha256`,
  `install_member`.
