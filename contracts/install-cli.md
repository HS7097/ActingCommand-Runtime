# Command-line installation (`actingctl install`)

Workflow #421 (Alice, #405, 2026-10-24: 「你这边直接开始使用/新增命令行安装的方式就行」). The
frozen model is INST-M v1.0 (`rd-1024\install-design\model.md`). `actingctl install` installs,
updates and rolls back a **single-location root** from the command line: verified release
builds kept under the root, an in-place update with a journal, a gated rollback, the
configuration candidate, the instances, the standard packs and a fresh root. It is the
installer of a single-location root ([runtime-watchdog.md](runtime-watchdog.md), "Layout"); the
A/B layout (`install\active.json`) stays acsetup's.

The family exists in the CLI only, like `start` and `restart`: configuration file edits and
daemon restarts belong to the person, so no MCP tool drives it. A UI drives these commands as
child processes and reads their one result line, as acsetup drives `actingctl` today. The
commands are game-neutral: every alias, game, package and value comes from the configuration
or a bundle; nothing in Tools names one, nor a path of a machine.

Slices (model §12): INST-0 this document and the codes; **INST-1** release verify and stage,
adopt, update plan and apply, rollback with its gate, undo, resume, status, leftovers, the
journal, the writer lock, `actingd check-config --install-root`, the `install` configuration
section and watchdog row 7a″; INST-2 the formal stop and start, the hold, the guards and the
composite update; INST-3 the configuration candidate; INST-R and INST-5 the instances; INST-4
the packs; INST-6 a fresh root; INST-7 the offline preparation check. Each section names its
slice; a command of a later slice is refused by an earlier build as an unknown command
(`actingctl_usage_invalid`).

## Layout

A single-location root `<root>` holds `runtime\BUILD-MANIFEST.json`, no `install\active.json`,
and its configuration `actingd.config.json`. The installer owns `<root>\install\`:

| Path | What | Writer |
|---|---|---|
| `install\writer.lock` | held exclusively for every change of `runtime\`, `tools\`, the configuration file or a game root; created when missing | installer |
| `install\state.json` | `actingcommand.install-state.v1` (below): the live build, the kept builds, the pack register, the committed configuration's SHA-256 | installer (`.part`, flush, rename) |
| `install\current.json` | the one unfinished operation `{operation_id, kind, started_at_unix_ms}`; absent when none | installer |
| `install\operations\op-<unix_ms>.json` | `actingcommand.install-operation.v1`, the journal (below) | installer |
| `install\builds\<commit40>\runtime\`, `tools\`, `release.json` | kept builds, each verified against its own manifests; the source of every update and rollback | `release stage`, `adopt` |
| `install\candidate\actingd.config.json`, `base.json` | the staged configuration and the SHA-256 of the live file it was built on (INST-3) | `config *`, `instance *` |

**Kept files.** Tools never deletes anything; `install leftovers` lists what it kept:

- `<file>.old-<ms>`: a replaced program file, game-root item or game root;
- `<file>.rolledback-<ms>`: a file an undo moved aside;
- `actingd.config.json.prev-<ms>`: a replaced configuration (INST-3);
- `<name>.new-<ms>.part`: a file or folder an operation is staging; `<name>.abandoned-<ms>` when an
  undo abandoned it;
- `install\current.json.finished-<ms>`: the marker of a finished operation.

`<ms>` is the operation's Unix milliseconds (`op-<ms>`), so each kept name says which operation
made it. `.old-<unix_ms>` and `.part` are names the bundle grammar reserves for exactly this
(`package.rs`, reserved ids), so they never collide with a pack directory. No installer command
touches the state root or the ledger; `install new` (INST-6) only creates the empty state-root
directory.

## Common rules

- **Form.** `actingctl install <group> <verb> --root <root> …` (`install adopt`, `install undo`,
  `install resume`, `install status` and `install leftovers` have no verb). Like `watchdog` and
  `start`, the family runs before `--state-root` parsing; the state root comes from the root's
  configuration.
- **Output.** Exactly one JSON line on stdout, `{…result, "outcome": …}`, through actingctl's
  result line (#378 A5). A refusal's line carries the members that say why (for example the
  failed `edges` of a plan) beside its outcome. A warning, error or fatal top adds its developer
  line on stderr. The exit code is the top outcome's category: 0 success, info or warning; 1
  error (usage included); 2 fatal. `ACTINGCOMMAND_DETAIL` is honoured. No English prose on the
  interface: codes and fixed-key values only.
- **Codes** (ruling 54, "Outcome codes" below). Every command answers one code of the family,
  its value `operation` naming the command (`install_release_verify`, `install_adopt`, …):
  **done** (`install_command_done`), a **caveat** (`install_caveat_noted`, a warning: the work
  went on, `install_caveat` says what was left unchecked or pending; a `note` link of done, or
  the top when the command's result is itself the caveat), or a **refusal**
  (`install_command_refused`, `install_refusal` says why). A step that fails after a change on
  disk is `install_operation_interrupted` (fatal). Below, "refuses `x`" names the
  `install_refusal` token `x` and "the caveat `y`" the `install_caveat` token `y`; a mutating
  command's done carries `installation_changed`, `false` when there was nothing to do.
- **Read-only commands** write nothing: `release verify`, every `plan`, `status`, `guards`,
  `config show`, `config diff`, `leftovers`.
- **Mutating commands** each open one operation, journalled step by step (below). A root has
  at most one unfinished operation (`install\current.json`); every other mutating command then
  refuses `operation_unfinished` with its `install_operation_id` and leaves the root alone.
- **Plan and apply.** `update`, `rollback`, `pack place`, `pack remove` and `config commit`
  apply only with `--plan <plan_id>` from a plan of the same inputs. `plan_id` is `plan-` and
  the first 16 hex digits of the SHA-256 of the canonical plan document (members sorted, no
  whitespace), which covers the inputs, the builds, the file and item actions, the interface
  edges, the defaults diff, the configuration SHA-256 values and the check-config summaries;
  process ids, times and the rollback gate's ledger figures are not in it. Apply recomputes the
  plan under the writer lock and refuses `plan_changed` on any difference.
- **Guards** (INST-2). Each command lists its guards as `guards: [{id, result: pass | fail |
  not_evaluated, values}]`. A `fail` or `not_evaluated` refuses `guard_failed`
  (`install_guards`) unless the person names it with `--ack-guard <id>`; acknowledgments are
  journalled.
- **Which actingctl runs it.** Any build that carries the family: the root's
  `runtime\actingctl.exe`, or a kept build's `install\builds\<c>\runtime\actingctl.exe`. The
  running program is the installer **I** of the interface checks; its ranges are its own
  declaration (`component-interfaces.json`, the same file its manifest carries). Every root, a
  scratch copy for a rehearsal included, is just `--root`.
- **Progress.** The journal is rewritten after every step; `install status --operation <id>`
  answers it.

## Release verify and stage (INST-1)

```text
actingctl install release verify --dir <dir> [--facts <release-facts.json>] [--expect-tag <tag>] [--expect-commit <hex8+>]
actingctl install release stage  --root <root> --dir <dir> [--facts …] [--expect-tag …] [--expect-commit …] [--local-build]
```

`--dir` holds `actingcommand-runtime-<c>.zip`, `actingcommand-tools-<c>.zip` and `SHA256SUMS`
(`<sha256 hex>  <file name>` lines), as the Release workflow publishes them. `--facts` is the
release-facts file `actingcommand.release-facts.v1` that a fetch script or a UI downloader
writes from the release (Tools reads no token; its network access is only the downloads of
"Downloads" below):

```json
{"repository": "<owner>/<name>", "tag": "<tag>", "commit": "<40 hex>", "prerelease": true,
 "assets": [{"name": "<file name>", "size": 0, "digest": "sha256:<64 hex>"}]}
```

An optional `schema_version` member, when present, is `actingcommand.release-facts.v1`. The
assets name both zips and `SHA256SUMS`.

**Downloads** (Alice, ruling 45, #421 comment 6380164338). `release stage` and `install new` may
download a release's files, and the vision and OCR models of a fresh root, from fixed GitHub
release URLs. The URLs and the expected SHA-256 values come from the configuration or from the
release metadata (the release-facts file, its asset digests), never from the program: nothing in
Tools names a URL. A downloaded file is used only after its SHA-256 matches its expected value;
a mismatch refuses as the local checks do (`digest_mismatch`). No other network access: Tools
reads no token and no `gh` configuration. INST-1 does not download: its `release verify` and
`release stage` read a folder the fetch script or a downloader filled; the download comes in a
later slice (with INST-6's fresh root, whose vision models need it).

Checks (every failure is listed, not only the first; each is a `release_check` token):

1. Exactly one Runtime zip and one Tools zip, of the same commit `<c>` (`zip_set`).
   `SHA256SUMS` lists both (`sums_missing`) with their SHA-256 (`sums_mismatch`).
2. With facts: the file reads (`facts_invalid`); each zip's and `SHA256SUMS`' SHA-256 equals its
   asset digest (`digest_mismatch`); `facts.commit` is `<c>` (`facts_commit_mismatch`);
   `--expect-tag` equals `facts.tag` (`tag_mismatch`; `--expect-tag` without `--facts` is a
   usage error). Without facts the result is the caveat `release_unconfirmed` as the top (a
   warning, with done's `commit_sha`): tag and digests not checked.
3. Each zip is extracted (`zip_unreadable`) into the stage's `.part` folder, or a work folder
   under `%TEMP%` for `verify`: its root `BUILD-MANIFEST.json` reads (`manifest_invalid`); its
   layout is `distribution-v1` (Runtime) or `platform-tools-v3` (Tools) (`manifest_layout`); its
   `repository` equals `facts.repository` when facts are given (`manifest_repository`); its
   `commit_sha` is `<c>` (`manifest_commit`); the extracted files are exactly the manifest's
   files and `BUILD-MANIFEST.json` (`file_set`); every file has the manifest's size and SHA-256
   (`file_mismatch`).
4. Both manifests declare `interfaces` valid by the parse rules of
   [component-interfaces.md](component-interfaces.md) (`interfaces_invalid`) and equal
   `speaks` (`interfaces_differ`). `--expect-commit` is a prefix of `<c>`
   (`commit_prefix_mismatch`).

Results: done (`commit_sha`, `tag`) with the result members `commit`, `tree`, `tag`,
`prerelease`, `interfaces` and `files` (the count), or the refusal `release_check_failed`
(`release_checks`) with `failures: [{check, file?, expected_sha256?, actual_sha256?}]`.
`verify` writes nothing under any root; its work folder is left under `%TEMP%` and named in the
result (`work_dir`).

`stage` is a journalled operation (`kind` `release_stage`). It needs facts or `--local-build`
(`facts_missing`); `--local-build` accepts an exact-SHA Actions build for rehearsals and is
journalled. Steps: `extract` into `install\builds\<c>.new-<ms>.part\{runtime,tools}`, then
`verify_tree`, then `place_build` (the part folder renamed `install\builds\<c>` with
`release.json`: the facts, the verification result and `local_build`), then `record_state` (the
build added to `state.json`). The checks that need no extraction run before the operation
opens, so their refusal writes nothing; a check of the extracted files that fails renames the
part folder `install\builds\<c>.abandoned-<ms>` and finishes the operation with the refusal.
Staging a build that is already kept and verifies answers done with `installation_changed:
false` (the member `unchanged: true`) and opens no operation. A root that `release stage` writes
need not be adopted yet: it writes only `install\`.

## Adopt (INST-1)

```text
actingctl install adopt --root <root>
```

The first command on a root that scripts installed. It verifies `runtime\` and `tools\` against
their own manifests (the same commit, every listed file's SHA-256; files the manifests do not
list, such as `.old-*`, are ignored), copies exactly the listed files and both manifests to
`install\builds\<live commit>\`, and writes `state.json`: the live build, the pack register from
each `<standard-pack root>\<game>\bundle.json` (the configuration's `standard_packs.root`; source
repository and commit, `package_range: null`, since a placed root keeps no `interfaces.json`) and
the live configuration's SHA-256. A listed live file that differs from its manifest refuses
`live_build_modified` (`count`, `files`). A second adopt of the same live build answers done
with `installation_changed: false` (the member `unchanged: true`). Adopt is a journalled
operation (`kind` `adopt`: `copy_build`, `place_build`, `record_state`).

## Update and rollback (INST-1)

```text
actingctl install update   plan  --root <root> --build <commit> [--preview] [--accept-ledger-one-way]
actingctl install update   apply --root <root> --build <commit> --plan <id> [--accept-ledger-one-way]
actingctl install rollback plan  --root <root> --to <commit> [--accept-ledger-one-way]
actingctl install rollback apply --root <root> --to <commit> --plan <id> [--accept-ledger-one-way]
actingctl install undo      --root <root> --operation <id>
actingctl install resume    --root <root> --operation <id>
actingctl install status    --root <root> [--operation <id>] [--check-rollback]
actingctl install leftovers --root <root>
```

`<commit>` is the full 40-hex commit of a kept build. INST-2 adds `--stop --hold <reason>
[--known-pause ALIAS=REASON]… [--start] [--ack-guard <id>]… [--detach]` to apply ("Formal stop,
start and the hold"), and INST-3 adds `--with-config`; without `--stop` apply needs the Runtime
stopped, and until INST-3 it leaves the configuration alone.

**Plan** (read-only; `--preview` allows a running Runtime and reports it instead of refusing):

- **Builds.** The live build: the `runtime\` and `tools\` manifests name one commit and every
  listed file verifies (`live_build_modified` otherwise). The target:
  `install\builds\<commit>` verified again (refused `build_not_kept` or `build_invalid`).
  The root must be adopted (`not_adopted`) and single-location (`not_single_location`; with
  `install\active.json` `layout_ab_unsupported`).
- **Interface edges** (below), one row each in `edges`. A failed edge refuses
  `interface_edge`. An unknown pack declaration is the caveat `pack_declaration_unknown`
  (`game`), a `note` link.
- **File plan.** Per manifest file of the target, `new`, `replace` or `same` (by SHA-256);
  `BUILD-MANIFEST.json` too. Live files outside the target's manifests are listed as `extra` and
  left in place.
- **Defaults diff.** Each key whose default differs between the live and the target
  `runtime\actingd.defaults.json`, and each key added or removed, with the live configuration's
  source of the key: `file` when the configuration names it, else `default`. (Instruction values
  are Runtime state; INST-2's stop reads them.)
- **check-config.** The target build's `actingd check-config --config <root>\actingd.config.json
  --install-root <root>`, and the live build's (`<root>\runtime\`, in place, without the
  option). The plan carries both summaries (outcome code, catalog plan, pending approvals, note
  codes, the standard packs registered with their pack counts) and `differs`, the summary members
  that differ, and `target_install_root`, whether the target's answer was given the option. The
  target refusing refuses the plan (`config_refused_by_target`, its outcome as the link); no
  answer refuses `check_config_unanswered`.
  - A target whose `actingd-config` range ends below 17 (every build before v0.12.18, which
    refuses more than `--config <path>` with `check_config_usage_invalid`) is run without the
    option, and a target that refuses its arguments with it (an error before any check: no
    `report` and no value) is asked again without it (INST-1b-F fix round 1, review S1 F1). Run
    from `install\builds\<c>\runtime\`, it judges the kept build's folder as the install root
    (its adb and its default vision root); the plan carries the caveat
    `check_config_without_install_root` (`commit_sha`, `path` its actingd), a `note` link, and
    only the answer's category counts.
- **Ledger verify.** When the target's `ledger` range differs from the live one, apply first
  runs the target's `actingd ledger-maintenance verify --config <root>\actingd.config.json`
  (cold) and refuses `ledger_verify_failed` unless it exits 0 (the acsetup gate of
  [component-interfaces.md](component-interfaces.md), "Checks per operation").
- **Children and their deadlines** (INST-1b-F fix round 1, review S1 F4). Every program the
  installer runs has a deadline, a key of the `install` section: each check-config
  `install.child.check_config_timeout_ms`, each actingledger read (S0, each page of the rollback
  gate's reader part) `install.child.ledger_read_timeout_ms`, the ledger verification
  `install.child.ledger_verify_timeout_ms`. A child still running at its deadline is left
  running, never killed (the stall rule), and reported: the refusal its failure gives carries
  the child's `pid` and the key as `config_key`, and no cause link. Before the operation opens
  (both check-configs, S0, the gate) the command refuses as that child's failure says
  (`check_config_unanswered`, `ledger_head_unreadable`, the gate's `ledger_readability`) and
  nothing changes; the person runs it again once the child has ended. A ledger verification
  left running ends apply `install_operation_interrupted` with `ledger_verify_failed` as its
  link: nothing has been swapped, and the operation stays for `resume` (which verifies again)
  or `undo`, so the watchdog does not start the Runtime while the verification reads.
- **Running state.** An `actingcommand-actingd.exe` under the root (`runtime_running`) or a held
  owner lock (`owner_lock_held`) refuses unless `--preview`; a running state that cannot be read
  (the process probe, the state root's owner journal) refuses `processes_unreadable`. Images under the root that would be
  replaced while running: `actingctl.exe` is renamed aside and keeps running (listed as
  `running_renamed`); `adb.exe` is never moved and any other image waits
  (`running_image_blocks`, `path`, `pid`). A `.part` of another operation under
  `runtime\` or `tools\` refuses `foreign_part` (`path`).
- **Notice block** for the person's notice before the change: what changes, whether MCP servers
  must reconnect, and whether the rollback window will be bounded (a one-way ledger edge).
  `mcp_clients_reconnect` counts only a renamed `mcp-serve` image whose parent process is not an
  MCP entry running under the root (`actingcommand-mcp-entry.exe`): an entry switches its own
  child to the live build ("The MCP entry"; #411 P49, v0.12.25).

**Apply.** Steps in order, each journalled:

1. The preconditions above, under the writer lock (2), and the plan recomputed: a different
   `plan_id` refuses `plan_changed`.
2. `install\writer.lock` exclusive, created when missing; contention waits
   `install.lock_wait_ms` (`writer_lock_failed` after it).
3. (INST-2: `--stop`.)
4. **S0**: the ledger head, read with the live build's `actingledger --state-root <state> export
   --performance --after 0 --through 0 --limit 1` (its `snapshot_latest_sequence`), recorded in
   the journal's `s0.head` (`ledger_head_unreadable` refuses). On an SQLite root that is a range
   read: one read transaction that authenticates the keyed head row, the contiguity of the
   stored sequences and the head row, and reads no other row (INST-1b-F fix round 1, review S1
   F2; until then S0 was `actingledger open`, a whole-file check and a read of every record, in
   the window with the Runtime stopped). A segment root is read whole, as that command reads it.
5. `ledger_verify`, when planned.
6. `stage_file` for every `new` or `replace` file: the kept build's file copied to
   `<dst>.new-<ms>.part` and its SHA-256 checked.
7. `swap_file`, `tools\` first, then `runtime\`: a replaced `<dst>` renamed `<dst>.old-<ms>`,
   then the part renamed `<dst>`. Unchanged files are never touched.
8. `verify_tree`: every manifest file in place.
9. `record_state`: `state.json`'s live build is the target, installed by this operation.
10. (INST-3: `--with-config`; INST-2: `--start`.)
11. The lock released and `current.json` renamed `current.json.finished-<ms>`.

Result: done (`install_operation_id`, `commit_sha`, `installation_changed: true`) with the
members `operation_id`, `from_commit`, `to_commit`, `replaced`, `added`, `verified`,
`running_renamed: [{pid, mcp_serve}]` and `next` (the undo form). When `running_renamed` is not
empty the caveat `mcp_clients_on_previous_build` (`count`) is a `note` link. A live build equal
to the target answers done with `installation_changed: false` and changes nothing. A step that
fails after the first change on disk leaves the journal and `current.json` and ends
`install_operation_interrupted` (fatal, `operation`, `install_operation_id`) with the step's
failure as its link: the root cannot run normally until `resume` or `undo`.

**Rollback** `--to <commit>` is an update to a kept build, with the rollback gate (below) in its
plan and evaluated again under the writer lock immediately before the first change. A failed
gate refuses `rollback_gate_failed` (`rollback_gate_parts`, and `ledger_sequence` when the
target's reader refused a page): forward fix only. Success answers done, as an update does.

**Undo and resume.**

- `undo` reverses an unfinished operation, or the last finished one when no operation followed
  it; for a finished update or rollback the rollback gate must pass (it passes trivially when no
  Runtime owner started since the operation, read from the owner journal). Every step is reversed
  as the step table says; every displaced file is kept. Result: done (`install_operation_id`).
- `resume` continues an unfinished operation: each step's postcondition is checked on disk, a
  step already true is done, a missing half is finished. Result: done (`install_operation_id`)
  with the operation's own result members, and the operation's own result as its `related`
  link; an operation that ends refused answers its refusal. An operation whose end (or its
  undo's end) is in its journal while `current.json` still names it (a crash or a failed rename
  between the journal's last write and the rename of `current.json`) is not run again: `resume`
  only renames `current.json` `current.json.finished-<ms>` and answers the end it recorded (the
  journal's `result`; done of `install_undo` for an undo) as its `related` link (INST-1b-F fix
  round 1, review S1 F5). A failure of that last write or rename is
  `install_operation_interrupted`, so the person knows to resume.
- `undo` of a finished update or rollback reads the state root's owner journal: a journal that
  cannot be read or whose last record this installer does not recognise refuses
  `processes_unreadable`, and one a running owner holds locked refuses `owner_lock_held`;
  neither counts as "no owner started" (review S1 F6).
- An operation id that names no journal, an operation `resume` cannot continue (finished) or one
  `undo` may not reverse (not the last) is `actingctl_usage_invalid` (`argument` `--operation`,
  `argument_problem` `unusable`).

**Status** (done) answers the layout, the live and kept builds, the pack
register, the committed configuration SHA-256 and whether a candidate exists, the unfinished
operation, the last finished operations, the leftovers count, and the rollback window's cheap
parts (the interface edges toward each kept build, whether an owner started since the live
build's operation). `--operation <id>` answers that journal. `--check-rollback` evaluates the
whole gate toward the previous build.

**Leftovers** (done, `count`) lists every kept file and folder with its
operation id and size. It deletes nothing. The operation is the one whose journal records the
item (an undo's `kept`, an abandoned part), the one a `current.json.finished-<ms>` names, or
`op-<ms>` of the name's `<ms>` when the root keeps that journal. An item no journalled operation
of the root made has `operation_id: null`: a deploy kit's `<file>.old-<stamp>` or
`<file>.rolledback-<stamp>`, whatever its stamp. Such files are tolerated everywhere: `adopt`
ignores them (they are not listed), and a plan lists them under `extra` and leaves them in place;
only a `.part` refuses (`foreign_part`).

### `actingd check-config --install-root <root>` (INST-1)

check-config judges the install root from its own executable; from
`install\builds\<c>\runtime\` that is the kept build's folder. `--install-root <root>` judges it
as `<root>`, exactly as a daemon running from `<root>\runtime` would
([actingd-check-config.md](actingd-check-config.md), "Invocation"). The option comes with
`actingd-config` revision 17 (Runtime v0.12.18); a build before it refuses it
(`check_config_usage_invalid`), so the installer runs such a target without it (the plan's
check-config above).

## Formal stop, start and the hold (INST-2)

```text
actingctl install stop  --root <root> --hold <reason> [--known-pause ALIAS=REASON]... [--ack-guard <id>]...
actingctl install start --root <root> [--ack-guard <id>]...
actingctl install release-hold --root <root> --reason <reason>
actingctl install guards --root <root> --for stop|start [--hold <reason>] [--known-pause ALIAS=REASON]...
actingctl install update apply --root <root> --build <commit> --plan <id> --stop --hold <reason> [--known-pause ALIAS=REASON]... [--start] [--ack-guard <id>]... [--detach]
```

Workflow #421 INST-2 (Runtime v0.12.19; #421 deploy design §4, §8 S2; fix round 1 after review
S2). Alice's decisions: 74 A (an automatic rollback only between the new start and the resume),
75 as changed on 10-27 (no allowed or blocked deploy time span: the installer holds and records
every deferred action, switches, and hands the recorded work back; `schedule_near` only lists,
`install.guard.blocked_windows` is retired). Every command here is one journalled operation
(`kind` `stop`, `start`, `release_hold`; the composite is the update's or rollback's own), every
step resumable with `install resume`; nothing is killed or deleted. Each operation journals the
configuration file's SHA-256 when it began (`inputs.config_sha256`), which the check `config`
judges against. Run `install` from the newest kept build (`install\builds\<commit>\runtime\`):
after an automatic rollback to a build before INST-2, the root's own `runtime\actingctl.exe`
does not read the journals of kind `stop` or `start` or with `rollback` (review S2 L9).

- **The hold** (`stop`, and the composite's first steps). Read from the running Runtime before
  anything changes: S0 (the owner epoch; every pause `{global, instances: [{alias, reason,
  revision}]}`; every run-time instruction `[{config_key, value_json}]` from the program fact
  `host.config_overrides`) is journalled in `s0`, and the hold's **deferred record** in the pause
  step's evidence (`deferred`): per instance it takes, its queued lease requests, whether a run is
  in flight (its lease), and by the Runtime's latest plan (`ScheduleExplanation` with candidates)
  its selected decision, its due tasks and its earliest wake. The owed idle wakes have no read in
  the builds this records from and are recorded as not read (`owed_idle_wakes_read`
  `not_available`). The hold takes every instance S0 shows unpaused (and one already paused with
  `--hold`, a hold a failed step kept): all are paused at once with reason `--hold` (one pause
  per instance, side by side, so no instance keeps dispatching while another drains), each run
  in flight left to reach its end within `install.stop.drain_timeout_ms` and then its next
  checkpoint (the Runtime's own drain; never cut mid-action, never killed); how each drain ended
  is journalled (`drained: [{alias, finished, cancelled}]`). A pause the Runtime refuses while the
  instance has a queued request or a lease (an operator's `ac_run_pack` waiting) is tried again
  once after the status shows no queued request (a run in flight is left to the pause's own
  drain: re-review S2 N2), read every 500 ms within `install.stop.busy_wait_ms`;
  otherwise `runtime_stop_failed` with `alias` and `count` the requests still queued (the
  request ids are not readable from the status). `status` must then show the hold on every
  instance it took and, on every other instance, the pause S0 had or none (its owner may lift it
  meanwhile); `runtime_stop_failed` with `count` and `aliases` otherwise. A hold refused here,
  before anything changed, journals the drains that ran, resumes the instances it paused
  (`hold.kept: false`), reports the runs those drains cut (`runs_not_continued`, re-review S2 N3)
  and still ends refused. Tools pauses only with `--hold` and resumes only that reason; another party's pause is
  kept and allowed only when named by `--known-pause` with exactly its reason; a global pause and
  an unnamed pause refuse (`guard_failed`, which no acknowledgment passes). A Runtime that does
  not answer refuses `runtime_not_running`.
- **The close**: `request-shutdown` and its wait (the owner record closed and the process
  gone) within `install.stop.shutdown_wait_ms`; nothing is killed (a wait that passes refuses
  `runtime_stop_failed`). Just before `request-shutdown` S0's run-time instructions and the
  other parties' pauses are read again and journalled (what another session set or lifted
  during the drains is what the start restores). The closed start's terminal outcome is read
  from the start logs, each dated by the file's own modification time as the watchdog's F
  (WD-MT, since 0.12.28; `contracts/runtime-watchdog.md`, Observation 4): one that is not a stop
  (its category neither `info` nor `success`, as the
  watchdog judges it), or none found, is the caveat `stop_terminal_unconfirmed` (the Runtime is
  gone all the same). S0's ledger head is then read with the live build's reader
  (`ledger_head_unreadable` otherwise) and the running state checked again (`runtime_running`,
  `owner_lock_held`). The upgrade state (`install-control` 2) is not used: no build ships it.
  `stop` answers done (`installation_changed: false`) with `s0`, `held` and `next` (the
  `release-hold` form), and the caveats `runs_not_continued` (`count` the runs its drains
  stopped at a checkpoint) and `idle_wakes_rederived` (`count` the held instances whose idle
  action is not `none`: their owed idle wakes were not read and the Runtime derives them again).
  A failed step keeps the hold's pauses, ends the operation refused and names `release-hold` in
  `next`, and `install start` too once the Runtime is down or may be going down (a close whose
  wait passed after `request-shutdown`: re-review S2 N1; `install start` refuses a Runtime still
  running).
- **Start**: no live owner, no actingd under the root, the configuration passes the check-config
  of `runtime\` (`config_refused_by_target`); guard `adb`; `writer.lock` and
  `<root>\watchdog\run.lock` held; before every attempt the `memory` guard, read again every
  500 ms until it passes within `install.start.memory_wait_ms` (attempt 1 then refuses
  `guard_failed` before any spawn; `install start --ack-guard memory` passes it, a stop's or
  composite's `--ack-guard memory` does not); the watchdog's manual start with the log
  `<root>\watchdog\actingd-<ms>.log`; readiness every 500 ms until
  `install.start.ready_timeout_ms`; every `install.start.sample_interval_ms` a progress sample
  `{at_unix_ms, log_bytes, read_bytes, stalled}` journalled in the start step's evidence, and
  the stall rule's detection, the caveat `start_stalled` (`path` the log) when the log did not
  grow for `install.start.stall_after_ms` or the starting Runtime read more than
  `install.start.stall_read_bytes`: it reports and never kills (§7: ending a process is the
  person's decision). An exit with an error terminal outcome starts the next attempt up to
  `install.start.attempts`, a fatal one does not; a start not ready at its deadline is
  `start_timeout`, left running and never started again beside it. Then the post-start checks
  (below) against the S0 and the hold of the newest operation that stopped the Runtime, unless a
  start log newer than that operation's end shows a Runtime started since (a watchdog restart,
  a reboot): then without an S0; a mismatch is the caveat `post_check_mismatch` (`count`).
  Result: done (`pid`, `owner_epoch`,
  `log`) with `start` (the evidence) and `post_check`, or the refusal `runtime_start_failed`
  (`path` the log) with the start path's own code as its link (`exited_during_startup`;
  `start_timeout`, the process left running).
- **release-hold** resumes only the instances paused with exactly `--reason`, each by the pause
  it shows (`ResumeScheduling` with the owner epoch and revision it saw; another party's pause is
  never touched), and hands the deferred work back: from the hold's deferred record (this
  operation's, else the newest operation whose hold has that reason) it records per instance
  whether it was resumed, each due task and selected decision with `re_evaluated` (the Runtime's
  latest plan was evaluated since the release) or `pending_evaluation`, its queued requests, and
  the runs its drain stopped at a checkpoint (`not_continued`: the Runtime evaluates their tasks
  again from their start; the caveats `runs_not_continued` and `idle_wakes_rederived` with
  `count`). The Runtime itself
  evaluates the deferred dispatches again from its next cycle; the installer sends nothing the
  record names, so no operation already sent is replayed. Result: done (`count` the instances
  resumed) with `release`. `install.hold_after_start` (default `true`) keeps a composite `update
  apply --stop --start` held until the person releases it.
- **guards** reports the guards a `stop` (`foreign_pauses` and `schedule_near` from the running
  Runtime, `not_evaluated` without one; `adb`) or a `start` (`memory`, `adb`) would evaluate now
  and changes nothing (done, `count` the guards that would refuse, with `guards`; the
  informational `schedule_near` is never counted). A hot placement's guards come with INST-4.
- **The composite update** `update apply … --stop --hold <reason> [--start]`. Its plan is made
  with `update plan … --preview` while the Runtime runs (the plan document does not hold the
  running state, so the ids agree); apply recomputes it as a preview, refuses a running image that
  would change and may not be renamed (`running_image_blocks`), takes `writer.lock` before the
  hold and keeps it to the end, evaluates the guards (`foreign_pauses`, `schedule_near` (listed
  only), `adb`, and with `--start` `memory`, where the working set of the running Runtime, which
  the stop releases, counts as available), and journals one operation: `pause`, `close`, the
  update's own steps (S0's head read at the close), then with `--start` the steps `start`,
  `post_check` and, unless `install.hold_after_start` keeps the hold, `release_hold`. Result:
  the update's done (with `start`, `post_check` and `hold`), its caveats as `note` links
  (`runs_not_continued` and `idle_wakes_rederived` whether or not the hold was released). A
  composite refused before its first file step resumes what its hold paused, as `stop` does; one
  refused after the close names `install start` in `next`.
- **The automatic rollback** (decision 74 A; only a composite update with `--start`, only between
  the new start and the resume). A fail-class post-start check that fails, or a new start that
  ends without readiness (an exit; not `start_timeout`, the stall rule's), journals the rollback's
  steps before anything acts (fix round 1, review S2 H2: a crash from then on resumes the
  rollback, never judges the new build again): the planned steps not begun are marked
  `{"skipped": "rollback"}` and after them come `close` (`rollback: true`: first, before anything
  is closed (Runtime v0.12.25, Workflow #421 6441776975), the rollback gate (below) toward the
  build the update replaced, with this operation's S0, judged as a preview while the new Runtime
  may still run: that Runtime and its owner lock, which the close ends, do not refuse it, a
  running image that would change and may not be renamed does (`running_image_blocks` as its
  cause). A gate that fails ends the command with its refusal `rollback_gate_failed`
  (`rollback_gate_parts`, `ledger_sequence`; the result's `gate` and `failed_checks`) and closes
  nothing: the new build, its Runtime (still running, held) and the hold stay, `next` names
  `release-hold` (and `install start` only when no Runtime runs under the root, as after a new
  start that exited); the person decides, a forward fix only. A gate that passes journals
  `rollback {started_at_unix_ms, to, failed, reversed_at_unix_ms}` and its pass in the evidence of
  `reverse_files`, and a close resumed past it does not judge it again. Then the new Runtime is
  closed when it runs, `request-shutdown`; one that does not stop within
  `install.stop.shutdown_wait_ms` ends the command `post_check_failed`, nothing killed; S0 stays
  the operation's own), `reverse_files` (`{to, failed}`: the gate's pass journalled by the close;
  one that finds none judges the gate now, as above but after the close; then, also in a rollback
  resumed past its gate, no owner lock held and no actingd running under
  the root, as `undo` checks (re-review S2 N4; otherwise `install_operation_interrupted` with
  `runtime_running` or `owner_lock_held`, the operation left unfinished for `install resume`),
  then every file step of the operation is reversed as `undo` reverses it, every displaced file
  kept, the previous build verified in place and `state.json` rewritten), `start` and `post_check` of
  the previous build, and `release_hold` when its checks passed (otherwise the hold stays and the
  caveat `post_check_mismatch` says so). The
  command ends refused `deploy_rolled_back` (`commit_sha` the previous build, `count` the failed
  checks; the result's `failed_checks`, `post_check` and `rollback`): loud, never a success,
  also when scheduling resumed. After the resume nothing rolls back by itself; `install rollback
  plan|apply` (gated) remains. A composite rollback, or a stop or start of their own, never rolls
  back: a failed fail-class check ends it `post_check_failed` (the hold stays).
- `--detach` first refuses an unfinished operation and a configuration or install section that
  cannot be read, then runs the same command again without it through the watchdog's
  manual-run method (WMI `Win32_Process.Create`, hidden, no handle of the caller, so it outlives
  the session that started it), its output appended to `install\detached-<ms>.log`, and answers
  at once (done, `installation_changed: false`, `pid`, `log`; `next` names `install status`).
  A stale `--plan` or a refusing guard still shows only in that log.
- **MCP servers on old images.** `install status` lists `mcp_servers_on_old_images: [{pid,
  path, mcp_serve}]`, the running `mcp-serve` processes under the root whose image an update
  renamed aside (`<file>.old-<ms>`); their clients reconnect, except an MCP entry's child, which
  its entry switches.

### The post-start checks (INST-2)

Each `{id, result: pass | fail | not_evaluated, fail_class, values}`, judged against S0, the
configuration and `state.json`, never against per-release expectations. The started build's
own report of its start, the read `StartReport` ([runtime-interface.md](runtime-interface.md)), is
used when the build knows it; a build before it (a rollback target before v0.12.19) is checked
from its status, its configuration facts and its start log's `OUTCOME actingd:` lines, and what
they cannot show is `not_evaluated`. A `StartReport` read that fails otherwise (its client did
not report `runtime_operation_unsupported`) fails the fail-class checks it feeds (`no_seeding`,
`bindings`, `packs`, `instructions`, `no_dispatch`), never a vacuous pass. Only a failed
fail-class check acts (the automatic rollback, or `post_check_failed`); `not_evaluated` never
does.

| id | Judged | Fail-class |
|---|---|---|
| `ready` | the start reached readiness (`ready_ms`), and the owner it made ready still answers (a Runtime gone or restarted at check time fails) | yes |
| `open` | the open's phases sum within its deadline and no `ledger_open_slow` (report, else the start log) | no |
| `no_seeding` | a root that ran before (an S0) loaded its tagged state, not seeded again | yes |
| `bindings` | every configured instance alias bound | yes |
| `packs` | every game of the pack register registered, with its pack count | yes |
| `pauses` | S0's pauses, plus the hold on every instance it took | yes |
| `instructions` | S0's run-time instructions in effect as S0 recorded them, and none the start did not restore (the report's `instructions_dropped`: dropped or superseded). An instruction kept over a default that changed (`instructions_base_changed`, `config_override_base_changed`) is restored: it is named in the row's `base_changed` and never fails it (#421 S2-IF) | yes |
| `no_dispatch` | no dispatch admitted on a held instance since the start (report; else no lease on one) | yes |
| `new_keys` | each key the update adds `default`, or `explicit` where the file names it (`config.parameters.*` sources) | yes |
| `config` | the configuration file's SHA-256 is the one the operation began with (`inputs.config_sha256`: a change during the operation fails, an edit after the adopt does not); a `state.json` hash that differs is reported (`committed_differs`), never failed | yes |
| `indexes` | the open noted no incomplete derived index | no |
| `one_way` | the one-way watch (S6, a later slice): `not_evaluated` | no |

What S2 does not do yet (#421 deploy design §8 and decision 75 of 10-27, for a later slice): a
run the drain stopped at a checkpoint is evaluated again from its start, not continued mid-run
(the caveat `runs_not_continued` says so in every answer that held); the owed idle wakes are not
recorded before the stop (the Runtime derives them again after its start, IDLE-2; the caveat
`idle_wakes_rederived`) nor queued again by the installer; the one-way watch (S6); the
Runtime-chosen window (S4); the configuration transition in the same command (S7); checks a
release ships as data (the design's "any check the release adds"); `packs` compares counts only
when `state.json` records them (it does not yet) and its register is the adopt's.

## Configuration, instances, packs and a fresh root (INST-3 to INST-6)

```text
actingctl install config show|diff     --root <root>
actingctl install config set           --root <root> KEY=JSON...
actingctl install config bind          --root <root> --instance <alias> (--maintenance | --startup <id> | --recognition <id> | --resource-package <id>) [--replace]
actingctl install config policy        --root <root> --section <file> [--catalog-dir <dir> --catalog-name <name>] [--transition replace]
actingctl install config lab-options   --root <root> [--authoring true|false] [--debugging true|false]
actingctl install config commit plan|apply --root <root> [--plan <id>] [--while-running]
actingctl install config discard       --root <root>
actingctl install config restore       --root <root> --prev <actingd.config.json.prev-<ms>>
actingctl install instance add    --root <root> --alias <a> --game <g> --server <s> (--mumu-index <n> | --mumu-name <x>) (--like <alias> | --touch-backend <b> --capture-backend <b>) [--application-id <id>] [--start-emulator true|false] [--bind-maintenance] [--commit-plan]
actingctl install instance remove --root <root> --alias <a> [--commit-plan]
actingctl install pack plan    --root <root> --zip <bundle.zip> --sums <SHA256SUMS> [--facts <file>] [--hot]
actingctl install pack place   --root <root> --zip … --sums … [--facts …] --plan <id> [--hot [--known-pause …]] [--accept-restart-needed] [--ack-guard <id>]...
actingctl install pack remove  plan|apply --root <root> --game <game> [--plan <id>] [--hot]
actingctl install pack restore --root <root> --operation <id>
actingctl install new --root <new root> --build <kept build dir or verified release dir> [--state-root <dir>] [--vision-from <dir>] [--mumu-root <dir>] [--watchdog] [--lab-authoring true|false] [--lab-debugging true|false]
```

- **Configuration** (INST-3). Every write goes to the candidate; the first write copies the live
  file and records its SHA-256 in `base.json` (refused `config_base_changed` once the live file
  changed). `set` takes only keys of the running installer's key table; the Lab options are
  written only by `lab-options` (`lab_options_locked`); `secret_fingerprint_salt`
  is never accepted, printed, diffed or journalled. `commit` plans with the check-config of the build that will run
  (`--install-root <root>`; `config_refused_by_target`, `catalog_plan_unexpected`), then
  applies under the writer lock: candidate → `actingd.config.json.part`, live →
  `.prev-<ms>`, part → live, read back (done; with the Runtime running only with
  `--while-running`, which adds the caveat `restart_required`). `restore` commits a
  kept `.prev-<ms>` the same way. Installer commands never set run-time instructions.
- **Instances** (INST-5, on INST-R). `add` and `remove` write the candidate only; a commit
  follows. A minted `instance_id` is never reused; there is no default backend; no device serial
  is read or written (done, or the refusal `instance_invalid`).
- **Packs** (INST-4). Plan with the contract crate's own readers (`BundleIndex::validate`,
  `read_standard_pack_root`), the package interface edge, the delta per item and the
  Runtime-semantic rules (`pack_binding_broken`, `pack_maintenance_drift`, the caveat
  `pack_restart_needed`); place cold or hot in the reader's order, each item `.part` →
  `.old-<ms>` → in place; the register records game, source repository, commit, tag and the
  declared `package` range, never a digest (done; `instances_not_idle`, the instances staying
  paused; the caveat `pack_full_check_skipped`). `remove` renames the game root
  `packages\<game>.old-<ms>`; `restore` puts one pack operation's kept items back.
- **Fresh root** (INST-6). `new` refuses a non-empty root or state root, copies and verifies the
  build into `runtime\`, `tools\` and `install\builds\<c>\`, writes the first configuration (the
  salt 32 bytes from the OS CSPRNG, never shown), runs its check-config and writes `state.json`;
  nothing is started (done). The vision and OCR models come from
  `--vision-from` or from the downloads above.
- **Lab options** (Alice, ruling 44, #421 comment 6380164338; INST-3 and INST-6). The CLI install
  asks for each Lab option (`lab_module_authoring`, `lab_module_debugging`) in the middle of
  `install new` when it runs interactively (its console is a terminal), with the labels of the
  UI installer's Lab page. Run non-interactively it installs with full permissions: every Lab
  option on, unless `--lab-authoring false` or `--lab-debugging false` names it off. Every option
  an installer command turns on is listed in its JSON result (`lab_options_enabled`). `config
  lab-options` changes them later through the candidate and a commit, and lists the same way.
  Updates keep the values a root has.

## Interface checks

From [component-interfaces.md](component-interfaces.md), "Checks per operation". The parties:
**I** the running installer (its own declaration), **Rp** the live build, **R\*** the build that
will run, **B** a placed bundle (the register's declared range) or a new one (its
`interfaces.json`), **C** the configuration file.

| Operation | Edges (all must pass; each a row of `edges`) |
|---|---|
| update, rollback | `ledger`: Rp.max ∈ R\*; R\*.max > Rp.max needs `--accept-ledger-one-way` (journalled; it closes later rollbacks by the same edge). `runtime-outcomes`: R\*.max ∈ I. `install-control`: I ∩ Rp and I ∩ R\*. `runtime-client`: I ∩ Rp and I ∩ R\*. `package`: every placed B.max ∈ R\* (unknown: the warning). `actingd-config`: C passes R\*'s check-config, and I's key table knows every key C names in the sections the configuration mechanism resolves. Each build's Runtime and Tools manifests declare equal `speaks`. |
| config commit, restore | C passes the check-config of the build in `runtime\`. |
| pack place | B.max ∈ the live Runtime's `package` and ∈ I's. |
| new | I ∩ R for `actingd-config`; R's check-config passes. |

An edge row is `{interface, check, result: pass | fail | warning, parties: [{party, source,
range}]}` (`source` `manifest`, `register` or `bundle`); every failed edge names both parties,
the source of each range and the ranges.

## Safety

### Preconditions

| Command | Runtime | owner.lock | writer.lock | Other |
|---|---|---|---|---|
| `update` / `rollback apply` without `--stop` | stopped: no actingd under the root | unlocked | taken | no unfinished operation; no `.part` of another operation; no running image that would change but `actingctl.exe`; a running `adb.exe` never changes |
| with `--stop` (INST-2) | running (stop is step 3) | — | taken before the hold | as above after the stop |
| `config commit apply` (INST-3) | stopped, or `--while-running` | — | taken | candidate base = live |
| `pack place` (INST-4) | any (cold when stopped; `--hot` when running) | — | taken | hot: its guards |
| `start` (INST-2) | stopped | unlocked | taken (or already held) + `run.lock` | configuration passes check-config |
| `stop` (INST-2) | running | — | not taken alone; taken inside an update | its guards |
| `new` (INST-6) | — | — | created | empty root and state root |

### Locks and the watchdog

- `install\writer.lock` is held exclusively from before the first change to after the last one;
  in a composite update from before the hold until after readiness. While it is held the
  launcher runs nothing, a tick answers `watchdog_installer_busy` (row 7a), and `actingctl start`
  and `restart` refuse. A free lock is held shared by a tick until the tick ends; the installer
  waits `install.lock_wait_ms` for it.
- `<root>\watchdog\run.lock` is held by the installer's start, as by `actingctl start`.
- A `request-shutdown` close is a formal close (row 6); the installer's start is a formal start
  for the watchdog.
- **Row 7a″.** While `install\current.json` names an unfinished update or rollback (or the undo
  of one, which keeps its kind), a tick that would consider a start answers
  `watchdog_installer_unfinished` (warning, `install_operation_id`) and starts nothing, and
  `actingctl start` and `restart` refuse the same way
  ([runtime-watchdog.md](runtime-watchdog.md), "Decision"). The OS releases the writer lock when
  an installer dies, so without the row the watchdog would start a half-swapped root. An
  unfinished `release stage` or `adopt` changes only `install\` and does not hold a start back
  (INST-1b-F fix round 1, review S1 F7); a `current.json` that cannot be read still does.
- `owner.lock` is only read, never locked. The ledger verification and check-config take their
  own locks as their contracts say.

### Running images

`actingctl.exe` images (MCP servers, reads) are renamed aside while running and keep running;
`adb.exe` is never moved and `adb kill-server` is never used; any other running image under the
root that would change refuses (`running_image_blocks`); unchanged files are never
touched; MCP servers left on renamed images are listed so the person reconnects them (an MCP
entry switches its own child; #411 P49).

### The rollback gate

A binary rollback (`rollback apply`, or `undo` of a finished update or rollback) restores exactly
the program files of `runtime\` and `tools\`, from a kept build or from the operation's
`.old-<ms>` files; never the state root, the ledger, `packages\`, `policy\` or the configuration
file (a configuration restore is `config restore`, a pack restore `pack restore`). The gate is
evaluated at `rollback plan`, again at apply under the writer lock, by `status
--check-rollback`, and by a composite's automatic rollback before it closes the new Runtime
(v0.12.25); all four parts must pass (`rollback_gate_part`):

1. `interface_edges`: the update edges with R\* the target and Rp the live build; the live
   build's `ledger` max ∈ the target's range.
2. `ledger_readability`: S0 is the ledger head the operation that installed the live build
   recorded (its journal's `s0.head`). The **target build's own** `actingledger --state-root
   <state> events --after <S0> --limit 1024` reads every page up to the head with exit 0; the
   first page it refuses closes the window (`rollback_gate_failed` with `ledger_sequence`). A
   live build no journalled operation installed has no S0, and this part fails. A composite's
   automatic rollback evaluates the gate before it closes the new Runtime (v0.12.25), so the
   target's reader reads the ledger as of that moment and the pages the closing Runtime writes
   afterwards are not read ahead by it; a ledger interface the target cannot read is already
   refused by part 1 (`interface_edges`).
   - **Only toward the build the live one replaced** (INST-1b-F fix round 1, review S1 F3). S0
     covers only the records written since the live build was installed, so the part passes
     only for the target that operation replaced (its journal's `previous.live`, the build
     `status --check-rollback` checks). Any other target fails the part with `s0: null` and
     `previous` naming the one target it may pass for: a rollback past the previous build is a
     rollback to it first, then another.
   - **The first refused sequence** (review S1 F11). `ledger_sequence` is the least `t` for
     which the target's reader refuses `events --after <a> --through <t> --limit 1024` within
     the refused page (`a` its `--after`, its rows ending at `a + 1025`), found by halving it.
     Every read also authenticates the head row (and, for events with artifacts, the eviction
     rows after the rows read), so `t = a + 1` also when only those refuse: that is as precisely
     as the reader allows. A read that gives no answer ends the search at the least refusing `t`
     found, its failure attached too. The row lists `s0`, `previous`, `page_after` and
     `first_refused` (`null` when the reader gave no usable answer at all).
3. `open_time`: the target's open deadline is the configuration's
   `ledger_observer.open_timeout_ms` when the file names it, else the target's
   `actingd.defaults.json` value; a target whose defaults lack the key is a build before v0.12.11
   and fails. The open time is that of the **target build's own** last ledger open on this root
   (the open time depends on the build that will open): the sum of all eight phases its
   `ledger_opened` daemon line reports (`sql_read_ms`, `verify_ms`, `material_ms`, `restore_ms`,
   `meta_ms`, `tail_ms`, `index_ms`, `import_ms`; a phase a build before LN-8 does not report
   counts 0), unscaled. It must be at most the deadline minus `install.rollback.open_margin_ms`.
   The start logs (`actingd-<ms>.log` in the root, `<root>\watchdog\` and the console's log
   folder; `<ms>` the start, else the log's modified time, read from the file itself and never
   from its directory entry, which NTFS leaves stale while another process holds the log open:
   WD-MT, since 0.12.28) are attributed to builds by the
   operation journals: the build live on the root at `<ms>` is the one the newest journalled
   change before it made live (an adopt's build from the adopt on, an update's or rollback's
   previous build from its start and its target from its end, the previous build again from the
   end of its undo); before the first journalled operation the journals name no build. The
   newest attributed log of the target is judged: when it holds no open line (a start whose
   open failed: a budget exit, a stall) the part fails, and no older open is judged instead
   (INST-1b-F fix round 1, review S1 F8). When the target has no attributed log at all, the
   newest open line of any start log is judged and the gate carries the caveat
   `open_time_from_other_build` (`commit_sha` the target, `path` the log), a `note` link of the
   command's result; no open line at all fails the part. The gate row lists `deadline_ms`,
   `margin_ms`, `estimate_ms`, `log` and `from_target_build`. (Until INST-1b-F the part summed
   only the four complete-read phases of the live build's newest open and scaled them by the head
   over its `event_count`: since LN-8 those phases are 0 and the estimate was 0.)
4. `config`: the live configuration passes the target's check-config (`--install-root
   <root>`, or without it for a target that does not take the option, as the plan's
   check-config says, judged by its outcome's category; the row lists `check_config` and
   `install_root`); a key of a newer `actingd-config` revision fails here, so the person
   restores the configuration first.

A failed gate leaves a forward fix only.

### Guards (INST-2, INST-4)

| id | Check | Key | Commands |
|---|---|---|---|
| `memory` | available physical memory ≥ the key (the watchdog's hidden CIM query); before a stop the working set of the root's running Runtime, which the stop releases, counts as available (`runtime_working_set_mb`, `available_after_stop_mb`); each start attempt reads it again every 500 ms within `install.start.memory_wait_ms`. A stop's or composite's `--ack-guard memory` passes the row before the stop only; the start's own guard is passed only by `install start --ack-guard memory` | `install.guard.min_available_memory_mb`, `install.start.memory_wait_ms` | start (each attempt), update with `--start` |
| `schedule_near` | **informational** (Alice's decision 75 of 10-27): it lists what is due on an instance the hold takes within the key, by the Runtime's latest plan (a selected decision or a due task due now, a wake within the key due then; `not_evaluated` when the plan cannot be read), which the hold defers and records; it never refuses, and `--ack-guard schedule_near` is accepted with no effect | `install.guard.before_next_start_ms` | stop, hot place, update with `--stop` |
| `blocked_window` | retired: never evaluated. Decision 75 of 10-27 has no allowed or blocked deploy time span (the hold records and hands back the deferred work instead); `install.guard.blocked_windows` is retired in `actingd-config` revision 18 (accepted when it is a list, no effect, the caveat `config_key_retired`), and the Mon/Thu restart is no longer data in it | `install.guard.blocked_windows` (retired) | — |
| `foreign_pauses` | no global pause; no unnamed pause of another party on an affected instance; neither is passed by an acknowledgment (a pause is allowed only when `--known-pause` names it) | — | stop, hot place |
| `ladder_holding` | no affected instance in a ladder busy hold | — | hot place |
| `adb` | each configured instance with a known port answers `shell echo ok` through an adb server already running from `<root>\tools\platform-tools\` (a probe that does not answer is tried once more; one still running at its deadline is left running and not tried again); no server is `not_evaluated`; Tools never starts, connects or kills one | `install.guard.adb_timeout_ms` | stop, start, hot place |

The person's notice before a change (D0) is no guard; every plan prints the facts it needs.

### Secrets, identifiers, digests

The salt never leaves the configuration file and the candidate. No device serial is read or
written by any install command. No package digest is computed, pinned or compared; zip SHA-256
values appear only as the download check's evidence.

## Documents

### `install\state.json` (`actingcommand.install-state.v1`)

```json
{"schema_version": "actingcommand.install-state.v1",
 "live": {"commit": "<40 hex>", "operation_id": "op-<ms>" | null},
 "builds": [{"commit": "<40 hex>", "tag": "<tag>" | null, "origin": "release" | "local_build" | "adopted",
             "operation_id": "op-<ms>"}],
 "packs": [{"game": "<id>", "repository": "<owner>/<name>" | null, "commit": "<hex>" | null,
            "tag": "<tag>" | null, "package_range": [min, max] | null}],
 "config_sha256": "sha256:<64 hex>" | null}
```

`live.operation_id` is the operation that installed the live build (`null` after `adopt`).
Written to `state.json.part`, flushed and renamed; read back by the next command, which refuses
any other schema (`state_invalid`).

### `install\operations\op-<ms>.json` (`actingcommand.install-operation.v1`)

```json
{"schema_version": "actingcommand.install-operation.v1",
 "operation_id": "op-<ms>", "kind": "release_stage" | "adopt" | "update" | "rollback" | "stop" | "start" | "release_hold",
 "root": "<root>", "installer": {"commit": "<40 hex>" | null, "path": "<path>"},
 "inputs": {…}, "plan_id": "plan-<16 hex>" | null, "plan": {…} | null,
 "guards": [], "acknowledged": ["accept_ledger_one_way", …],
 "s0": {"head": 123, "epoch": null, "pauses": null, "instructions": null} | null,
 "previous": {"live": "<40 hex>" | null, "state": {…} | null},
 "steps": [{"n": 1, "kind": "<step>", "target": "<relative path>", "intent": {…},
            "state": "planned" | "started" | "done" | "undone", "evidence": {…} | null}],
 "undo": {"started_at_unix_ms": 0, "finished_at_unix_ms": 0 | null} | null,
 "rollback": {"started_at_unix_ms": 0, "to": "<40 hex>", "failed": ["<check id>"], "reversed_at_unix_ms": 0 | null},
 "result": {…} | null, "finished_at_unix_ms": 0 | null}
```

The journal is written `.part` → flushed → renamed before each step starts and after it ends.
`current.json` is written before the first step and renamed `current.json.finished-<ms>` at the
end (also at the end of an undo).

### Steps

Each step has a postcondition checked on the disk or the Runtime; it never trusts the journal's
state alone, so a step repeated after a crash is either already true or done again safely.

| Step | Postcondition | `resume` | `undo` |
|---|---|---|---|
| `extract` | the part folder holds the zips' files | extract again into the same part | part → `.abandoned-<ms2>` |
| `copy_build` | the part folder holds the live listed files | copy again | part → `.abandoned-<ms2>` |
| `verify_tree` | every manifest file matches | verify again | — |
| `place_build` | `install\builds\<c>` holds the verified build | finish the rename | `install\builds\<c>` → `<c>.rolledback-<ms2>` |
| `ledger_verify` | the target's verify exited 0 | run again | — |
| `stage_file` | `<dst>.new-<ms>.part` holds the wanted SHA-256 | copy again | part → `.abandoned-<ms2>` |
| `swap_file` | `<dst>` holds the new SHA-256; a replaced file's `.old-<ms>` holds the old one | finish whichever half is missing | `<dst>` → `.rolledback-<ms2>`; `.old-<ms>` → `<dst>`; an added file → `.rolledback-<ms2>` |
| `record_state` | `state.json` says so | rewrite | rewrite the previous state |
| `pause` | `status` shows the hold on every instance of the intent and every other pause as S0 had it (no Runtime: its pauses persist) | pause what is not yet held | — (the hold stays until `release-hold`) |
| `close` | no Runtime answers; S0's head recorded | close again | — |
| `start` | a Runtime answers whose owner started at or after the step's first spawn | start again | — |
| `post_check` | — | check again | — |
| `release_hold` | no instance paused with the reason | release again | — |
| `reverse_files` | the previous build verified in place | reverse what is not yet reversed | — |
| `config_swap` | INST-3 | | |
| `pack_item` | INST-4, as `swap_file` per item | | |

`<ms2>` is the undo's own Unix milliseconds. Crash points: before the first swap the live tree is
untouched (resume restages, undo abandons the parts); between swaps every file is old or new,
never torn (a rename is atomic), and between the two renames of one swap the live file is its
`.old-<ms>` and the part waits (resume renames the part in, undo renames the `.old-<ms>` back);
after the swaps the new tree is complete and the Runtime stopped (resume finishes, undo rolls
back by record); after the end is journalled and before `current.json` is renamed, resume only
renames it. While `current.json` exists every other mutating
command refuses and the watchdog stands aside (row 7a″). A `.part` of another operation refuses
(`foreign_part`); Tools never removes it.

## Configuration keys

The section `install` of `actingd.config.json` ([runtime-config.md](runtime-config.md), "The
`install` section"; `actingd-config` revision 17, Runtime v0.12.18; revision 18, Runtime
v0.12.19, adds INST-2's `start` and `stop` keys and retires `install.guard.blocked_windows`).
actingd parses and validates it and otherwise
ignores it; `actingctl` reads it from the root's configuration file through
`actingcommand-runtime-config`. The deadlines of the installer's children are its `child` keys
(INST-1b-F fix round 1, "Children and their deadlines" above). Fixed by this contract, not
tunable: the readiness probe period (500 ms, also the period of the start's memory reads and of
the hold's busy-instance reads), the 2 s clock slack, the layouts, schema names and
file names, the ledger page of the gate (1024 events), and the `actingd-config` revision whose
check-config takes `--install-root` (17). The MCP entry's tunables are its `mcp_entry` keys
("The MCP entry" below).

## The MCP entry (Workflow #421 MCP-ENTRY, Runtime v0.12.18)

Every client (Claude Code, Codex, Antigravity, DSH) starts one stable entry instead of
`actingctl mcp-serve`:

`<root>\mcp-entry\g1\actingcommand-mcp-entry.exe --root <root> --client <name> -- mcp-serve --tier …`

- **What it is.** A small program shipped in the Tools half (listed in its `BUILD-MANIFEST.json`).
  `g1` is the entry's own generation (`--generation` prints it); it changes only when the contract
  between the entry and its children changes (`entry_protocol`, now 1). A new generation goes in a
  new `g<N>` directory; nothing overwrites a running entry. Placing `g1` is a deploy step today
  (copy `tools\actingcommand-mcp-entry.exe`); placement by `install update` is a follow-up.
- **What it does.** It pipes JSON-RPC between the client and one current child,
  `<root>\runtime\actingctl.exe` with the words after `--` unchanged (so `--tier` and the client's
  permission scope stay as they are), and keeps the client's handshake to replay it. It watches
  the size and time of `runtime\BUILD-MANIFEST.json` and `runtime\actingctl.exe`; a change that
  stays put for `install.mcp_entry.settle_ms` makes it run
  `actingctl mcp-entry plan --root <root> --entry-protocol 1 --current-build <commit> -- <words>`
  of the current install, never while `install\writer.lock` is held. Whether an unfinished
  operation blocks it is the plan's decision (`operation_unfinished`). The first plan after a
  child started names that child's build only if the install has not changed since the child
  started; otherwise its build is a planned switch. A planned switch whose install changes again
  before its candidate starts is dropped, and the change is planned again once it stays put for
  `settle_ms`, so a candidate never runs a newer file under an older plan's build (#411 P49,
  v0.12.25).
- **The plan** (`actingctl mcp-entry plan`, operation `install_mcp_entry_plan`) prints
  `{switch, reason (build_changed, same_build, no_current_build), build {commit, manifest_sha256},
  child [exe, words…], tunables {poll_ms, settle_ms, verify_timeout_ms, drain_max_ms,
  refused_retry_max_ms}, entry_protocol, outcome}`. It refuses with `install_command_refused`:
  `writer_lock_failed`, `operation_unfinished`, `live_build_invalid` / `live_build_modified` (the
  installed files checked against their manifests, as `install adopt` checks them),
  `config_unreadable`, `install_section_invalid`. It reads the configuration before it checks the
  installed files, and every refusal after that read carries `tunables` too. Consecutive refusals
  double the entry's wait before the next plan, from `settle_ms` up to
  `install.mcp_entry.refused_retry_max_ms`.
- **A switch** happens only between calls. Before it, the entry asks the current child
  `actingcommand/entryStatus` (`{outstanding_handles, kinds, handles, calls_in_flight}`: background
  jobs other than runs whose handle went out to the client and whose results were not read yet,
  jobs still running (a run while it is still being submitted), and calls still running); a child
  before v0.12.18 answers "method not found", which counts as none when no call is in flight. A
  `0` counts only if no call reached the child after the question. While some are outstanding the
  switch waits (status `deferred: handles`, with the `handles`, which `ac_get_run` collects); past
  `install.mcp_entry.drain_max_ms` it still waits and the status says so. Run handles are Runtime
  request ids and need nothing. Cursors (`next_cursor` of `ac_events` and similar tools) belong
  to the child that issued them: after a switch they answer `cursor_invalid`, and the caller
  reads again from `after_sequence`. The candidate child gets the client's `initialize` (and
  `notifications/initialized`) or, for a 2026-07-28 client, `server/discover` with the client's
  `_meta`, then `tools/list`; a different protocol version, an error or no answer within
  `install.mcp_entry.verify_timeout_ms` refuses it, and the current child keeps serving; the
  same install is tried once more after `settle_ms`, then not again until it changes. A
  request in flight when its child ends is answered `mcp_entry_outcome_unknown` (inspect with
  `ac_get_run` or `ac_overview`) and never sent again; the next request goes to a fresh child.
  When the first plan after a fresh child names a build other than its predecessor's, the client
  gets the change notice below.
- **The notice.** Legacy clients get `notifications/tools/list_changed` (the entry answers their
  `initialize` with `capabilities.tools.listChanged: true`). 2026-07-28 clients get the same
  notification on each `subscriptions/listen` stream that asked for `toolsListChanged`, with
  `_meta["io.modelcontextprotocol/subscriptionId"]`; the entry acknowledges and owns those streams
  across children. Whether `tools/list` followed is recorded.
- **Status.** Each entry writes `<root>\mcp-entry\state\entry-<pid>.json`
  (`actingcommand.mcp-entry-status.v1`): generation, pid, the process's creation time as the
  operating system reports it (`process_created_at_unix_ms`, Unix ms; `null`, with an error line
  on the entry's stderr, when it did not say; #411 P49, v0.12.25), client, protocol, child (pid,
  build, since), previous children, last switch, deferral, last candidate failure, last plan,
  notice and whether `tools/list` followed, and the client's refresh entry (`tools_list_changed`
  for Claude Code, `codex_reload`, `antigravity_refresh`, `dsh_plugin_reload`, else
  `client_restart`). `actingctl mcp-entry list --root <root> [--all]` (operation
  `install_mcp_entry_list`) prints them with `stale` when the entry is gone: no process runs with
  its pid, or the one that does is another, since Windows reuses pids. That process's creation
  time differs from `process_created_at_unix_ms`, or, in a file without it (an entry before
  v0.12.25), is later than `started_at_unix_ms`, which the entry read after it was created. Only
  `--all` lists stale ones. A process probe that cannot decide (a session without elevation is
  refused the query for a process of another account) answers only its own row, which is listed
  with or without `--all`: `stale: null` and `probe_failed` (`{outcome}`: `process_probe_failed`,
  `pid`, caused by the probe's reason). The answer then has `warnings` with one
  `process_probe_failed` caused by each of those rows' failures (#411 P49 fix round 1, review
  M1). Nothing deletes a status file.
- **Registration.** `actingctl mcp-config --client claude|codex|antigravity|dsh --entry --root
  <root> [--tier …]` prints the snippet; it writes no client file.

## Outcome codes

Registered by INST-0 in `contracts/outcome-codes/actingctl.json` (owner `actingctl`, `ladder_class`
`none`: request refusals and configuration of the person's own installer, decision-table row 7)
and `actingd.json` (`check_config_install_root_invalid`). Folded by ruling 54 (#420 S4, Alice:
「54就是按推荐做」): one domain code per kind of outcome; the value `operation`, a location
the fragment registers, names the command.

| Code | Category | Values |
|---|---|---|
| `install_command_done` | success | `operation`; `installation_changed` on a mutating command (`false`: nothing to do); `install_operation_id`, `commit_sha`, `tag`, `plan_id`, `count`, `game`, `alias`, `path`, `pid`, `owner_epoch`, `log` as the command has them |
| `install_caveat_noted` | warning | `operation`, `install_caveat`; `commit_sha`, `count`, `game`, `package_id`, `config_key`, `path` as the caveat has them |
| `install_command_refused` | error | `operation`, `install_refusal`; `release_checks`, `rollback_gate_parts`, `ledger_sequence`, `install_guards`, `plan_id`, `install_operation_id`, `commit_sha`, `path`, `pid`, `count`, `files`, `game`, `package_id`, `alias`, `aliases`, `config_key` as the reason has them |
| `install_operation_interrupted` | fatal | `operation`, `install_operation_id`: a step failed after a change on disk; the journal names `resume` and `undo`, and until one runs the root cannot run normally |
| `watchdog_installer_unfinished` | warning | `install_operation_id` when known: the watchdog's decision row 7a″, also the cause of `start_runtime_refused` |
| `check_config_install_root_invalid` (actingd) | error | `path`: `--install-root` names no install root |

- **Locations** (`operation`): `install_release_verify`, `install_release_stage`,
  `install_adopt`, `install_update_plan`, `install_update_apply`, `install_rollback_plan`,
  `install_rollback_apply`, `install_undo`, `install_resume`, `install_status`,
  `install_leftovers` (INST-1); `install_stop`, `install_start`, `install_release_hold`,
  `install_guards_report` (INST-2); `install_config_show`, `install_config_diff`,
  `install_config_set`, `install_config_bind`, `install_config_policy`,
  `install_config_lab_options`, `install_config_commit_plan`, `install_config_commit_apply`,
  `install_config_discard`, `install_config_restore` (INST-3); `install_pack_plan`,
  `install_pack_place`, `install_pack_remove_plan`, `install_pack_remove_apply`,
  `install_pack_restore` (INST-4); `install_instance_add`, `install_instance_remove` (INST-5);
  `install_new` (INST-6).
- **`install_refusal`, INST-2**: `runtime_not_running`, `runtime_stop_failed`,
  `post_check_failed`, `deploy_rolled_back` ("Formal stop, start and the hold").
- **`install_caveat`, INST-2**: `start_stalled`, `stop_terminal_unconfirmed`,
  `runs_not_continued`.
- **`install_refusal`**: `not_single_location`, `layout_ab_unsupported`, `not_adopted`,
  `state_invalid`, `operation_unfinished`, `foreign_part`, `writer_lock_failed`,
  `release_check_failed`, `live_build_invalid`, `live_build_modified`, `build_not_kept`,
  `build_invalid`, `config_unreadable`, `install_section_invalid`, `config_refused_by_target`,
  `check_config_unanswered`, `config_base_changed`, `lab_options_locked`,
  `catalog_plan_unexpected`, `runtime_running`, `owner_lock_held`, `processes_unreadable`,
  `running_image_blocks`, `interface_edge`, `ledger_verify_failed`, `ledger_head_unreadable`,
  `plan_changed`, `rollback_gate_failed`, `guard_failed`, `runtime_start_failed`,
  `pack_invalid`, `pack_binding_broken`, `pack_maintenance_drift`, `instances_not_idle` (the
  instances stay paused), `instance_invalid`. Nothing changed, but what the reason says.
- **`install_caveat`**: `release_unconfirmed`, `mcp_clients_on_previous_build`,
  `pack_declaration_unknown`, `post_check_mismatch`, `restart_required`,
  `config_overridden_by_instruction`, `pack_restart_needed`, `pack_full_check_skipped`,
  `open_time_from_other_build` (INST-1b-F: the rollback gate judged another build's open),
  `check_config_without_install_root` (INST-1b-F fix round 1: the target's check-config ran
  without `--install-root`).
- **Keys**: `install_operation_id` (id), `plan_id` (id), `install_refusal` (vocabulary
  `install_refusal`), `install_caveat` (vocabulary `install_caveat`), `release_checks` (list of
  `release_check`), `rollback_gate_parts` (list of `rollback_gate_part`), `ledger_sequence`
  (integer), `install_guards` (list of `install_guard`); the rest are existing keys.

A refusal's causes chain the Runtime's or the contract's own code: check-config's outcome, the
start path's code, a child process's failure (`foreign_other_error`), an OS error
(`foreign_os_error`). A child left running at its deadline is no cause: the refusal names it by
its `pid` and the deadline's key (`config_key`).
