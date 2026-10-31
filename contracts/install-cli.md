# Command-line installation (`actingctl install`)

Workflow #421 (Alice, #405, 2026-10-24: 「你这边直接开始使用/新增命令行安装的方式就行」). The
frozen model is INST-M v1.0 (`rd-1024\install-design\model.md`). `actingctl install` installs,
updates and rolls back a **single-location root** from the command line: verified release
builds kept under the root, an in-place update with a journal, a gated rollback, the
configuration candidate, the instances, the standard packs and a fresh root. It is the
installer of a single-location root ([runtime-watchdog.md](runtime-watchdog.md), "Layout").

**The A/B layout** (Workflow #429, Alice's rulings 131 and 132 A of 10-31): `actingctl install`
is also the one writer of the formal A/B layout, frozen by slice R-15b in "The A/B layout" below
(`install\active.json` install selection v2, two program slots, the commit and recovery
protocol, the clean) and built by R-15c to R-15e; a production root converts once both sides are
ready (`convert`, R-15e). Until then a root stays single-location and none of the A/B paths run.
acsetup's own A/B roots of v1 are not converted and not read (no old-version compatibility,
Alice 10-11).

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

**Kept files.** On a single-location root Tools never deletes anything; `install leftovers` lists
what it kept. On an A/B root it deletes only in the clean, the seventh phase, and only the
whitelist of "The clean" below, file by file by list (Alice's ruling 131); every name below is
kept there too. The single-location kept files:

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
(`<sha256 hex>  <file name>` lines), as the Release workflow publishes them, or (Workflow #429
R-15b, A.4 b, since 0.12.35) a **UI release**: `acui-windows-<u>.zip` and `SHA256SUMS`, as the UI
repository's release workflow publishes them, its files at the zip's root. The reader tells the
two apart by the zip names. `--facts` is the
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

1. Exactly one Runtime zip and one Tools zip, of the same commit `<c>`, or exactly one UI zip of
   commit `<u>` and no other release zip (`zip_set`). Since 0.12.35 (R-15b) a zip the reader
   recognises as none of the three (any name ending `.zip`, in any case, that is not
   `actingcommand-runtime-<40 hex>.zip`, `actingcommand-tools-<40 hex>.zip` or
   `acui-windows-<40 hex>.zip`) refuses `zip_set` too, one failure per zip with its name as
   `file`, instead of being passed over; so does a UI zip beside another release zip, each zip
   named. Files that are not zips (a facts file, notes) are not looked at. `SHA256SUMS` lists
   every zip (`sums_missing`) with its SHA-256 (`sums_mismatch`).
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
5. **A UI release** (R-15b): the facts' `repository` is the UI repository's, and the checks of 2
   and 3 apply to its one zip, extracted into `ui\`: the manifest reads (`manifest_invalid`), its
   `repository` equals the facts' (`manifest_repository`), its `commit_sha` is `<u>`
   (`manifest_commit`), the files are exactly the manifest's (`file_set`) with their sizes and
   SHA-256 (`file_mismatch`); a UI manifest names no payload layout. Its `interfaces` is a UI
   declaration valid by the parse rules (`interfaces_invalid`), and its
   `installation_selection_schema` is `actingcommand.install-selection.v<N>` with N in the
   declared `install-selection` range (`selection_schema_mismatch`; a manifest without one breaks
   it too). Results and the kept `release.json` carry `member: "ui"`; a Runtime pair's carry no
   such member, so they are as before.

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

A **UI release's stage** (R-15b) is the same journalled operation with three steps: `extract`
into `install\ui-builds\<u>.new-<ms>.part\ui`, `verify_tree`, and `place_build` (the part renamed
`install\ui-builds\<u>` with its `release.json`). It writes no `state.json`: `install-state` v1
lists no UI build, and v2 lists them (`ui_builds`) from R-15c, which reads the kept UI builds by
folder. A kept UI build that verifies answers `unchanged`; one that does not refuses
`build_invalid`. `resume` and `undo` take its journal as they take a Runtime stage's; a folder
that holds the other kind of release when a stage resumes ends it interrupted with `zip_set`.

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
  `install start` form: the Runtime is down, so `release-hold`, which needs a running Runtime,
  is no next step; the hold is released from the start's result), and the caveats
  `runs_not_continued` (`count` the runs its drains stopped at a checkpoint) and
  `idle_wakes_rederived` (`count` the held instances whose idle action is not `none`: their owed
  idle wakes were not read and the Runtime derives them again). A failed step keeps the hold's
  pauses and ends the operation refused; while the Runtime still runs its `next` names
  `release-hold`, and once the Runtime is down or may be going down (a close whose wait passed
  after `request-shutdown`: re-review S2 N1) it names only `install start`, the hold again
  released from the start's result (`install start` refuses a Runtime still running). These two
  `next` rules are Workflow #429 R-15b's (design v4.1 §3.6, review v2-4) and hold for every
  answer of the family: a Runtime that is down, or may be going down, gets `start` and never
  `release-hold`. A build before R-15d still names `release-hold` beside `install start` there
  (`hold_kept`); R-15d changes the code.
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
- **Lab options from the graphical wizard** (Workflow #429 A.4 h, R-15a, since 0.12.30). Each of
  `--lab-authoring` and `--lab-debugging` takes an explicit `true` or `false`, and a flag given
  decides its option as given, interactive or not; only an option no flag names is asked for or
  defaulted on. The graphical wizard (acsetup, which drives `install new` as a child process)
  **always** passes both, `--lab-authoring true|false` and `--lab-debugging true|false`, with the
  person's choice on its Lab page; it never leaves either out, so the non-interactive default
  (both on) never decides for it. It shows the result's `lab_options_enabled` to the person as
  the CLI returns it, neither recomputed nor filtered. `install new` is not built yet (INST-6,
  slice R-15e), so this rule has no code before it; no flag of an existing command changes.

## Interface checks

From [component-interfaces.md](component-interfaces.md), "Checks per operation". The parties:
**I** the running installer (its own declaration), **Rp** the live build, **R\*** the build that
will run, **B** a placed bundle (the register's declared range) or a new one (its
`interfaces.json`), **C** the configuration file, **U\*** the UI that runs after the operation
("UI edges" below).

| Operation | Edges (all must pass; each a row of `edges`) |
|---|---|
| update, rollback | `ledger`: Rp.max ∈ R\*; R\*.max > Rp.max needs `--accept-ledger-one-way` (journalled; it closes later rollbacks by the same edge). `runtime-outcomes`: R\*.max ∈ I. `install-control`: I ∩ Rp and I ∩ R\*. `runtime-client`: I ∩ Rp and I ∩ R\*. `package`: every placed B.max ∈ R\* (unknown: the warning). `actingd-config`: C passes R\*'s check-config, and I's key table knows every key C names in the sections the configuration mechanism resolves. Each build's Runtime and Tools manifests declare equal `speaks`. |
| config commit, restore | C passes the check-config of the build in `runtime\`. |
| pack place | B.max ∈ the live Runtime's `package` and ∈ I's. |
| new | I ∩ R for `actingd-config`; R's check-config passes. |

An edge row is `{interface, check, result: pass | fail | warning, parties: [{party, source,
range}]}` (`source` `manifest`, `register` or `bundle`); every failed edge names both parties,
the source of each range and the ranges.

### UI edges (Workflow #429 A.4 a, R-15a, since 0.12.30)

**U\*** is the UI that runs after the operation. Its ranges are the declaration its UI build
carries: the UI repository's `crates/acui-setup/component-interfaces.json`, which the UI build
copies unchanged into its `BUILD-MANIFEST.json` as `interfaces`, read by the parse rules of
[component-interfaces.md](component-interfaces.md) (all six v1 interfaces named; a declaration
that breaks them is no U\*). A declaration that does not name `runtime-outcomes` counts as
[0, 0]. The U\* side of "Checks per operation":

| Interface | Check | Edge |
|---|---|---|
| `actingd-config` | `installer_ui_next_common` | one revision common to I, U\* and R\* (I ∩ U\* ∩ R\* not empty; I writes R\*'s configuration, the console edits it, R\* reads it) |
| `ledger` | `previous_max_in_ui` | Rp.max ∈ U\* |
| `ledger` | `next_max_in_ui` | R\*.max ∈ U\* |
| `runtime-client` | `ui_meets_next` | U\* ∩ R\* |
| `runtime-outcomes` | `next_max_in_ui` | R\*.max ∈ U\* |

`install-selection` has no edge here: a single-location root holds no `install\active.json`, so
the installer writes no selection that R\* or U\* would read (neither side is checked for it).
Each row names every party of its check with the source and the range of its own side: I
`declaration`, Rp and R\* `manifest`, U\* `manifest`, or `default` for a `runtime-outcomes` its
declaration does not name ([0, 0]). A failed U\* edge counts in `interface_edge` like any other.
`component-interfaces.md` defines no edge of the UI being replaced, so there is no such party.

The update and rollback plans and the rollback gate's part 1 add these rows once a root carries a
UI member: the release reader (R-15b) and the layout (R-15c) bring the UI build, and R-15d adds
the rows. Until then a root has no U\*, and its plans hold no U\* row.

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
  (INST-1b-F fix round 1, review S1 F7); a `current.json` that cannot be read still does. The
  code lists the kinds that pass (`release_stage`, `adopt`, `stop`, `start`, `release_hold`);
  every other kind stands aside. R-15c adds `prepare` and `clean` to the pass list; `convert` and
  `restart` stay off it (design v4.1 §3.4; "The A/B layout" below).
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
   folder; `<ms>` the start, else the log's modified time, read from the file itself and not
   from its directory entry, which NTFS can leave stale while another process holds the log
   open: WD-MT, since 0.12.28; `contracts/runtime-watchdog.md`, Observation 4) are
   attributed to builds by the
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

## The A/B layout (Workflow #429 R-15b, frozen; built by R-15c to R-15e)

The formal A/B layout of an installation root and the protocol that writes it. The frozen design
is `rd-1031/design-r15b-ab-v4.1.md` (sha256 `36ed6a98…2eaca`), frozen in #429 comment
6541651601 on Alice's rulings 131 (the clean) and 132 A (the conversion), #429 comment
6539887801; the UI channel reviewed it from v1 to v4.1. This section is the contract; where it
abbreviates, the design section named in brackets decides. A change after the freeze goes by the
bump rules of `install-cli` and `install-root` ([component-interfaces.md](component-interfaces.md)).

**Status (since 0.12.35).** R-15b ships this text, the contract crate's `InstallSelectionV2` and
`InstallSlotDocument` (parsed and checked, read by nothing yet), the release reader's UI side
("Release verify and stage") and the registry of codes and words at the end of this section.
R-15c builds the slot material, the slot locks, `install-state` v2, the readers of selection v2
and the fixed entries ([stable-entry.md](stable-entry.md)); R-15d the fresh root, the
activation, update, rollback, undo, resume, the clean, `install restart` and the read face; R-15e
the bootstrap and the conversion of a single-location root. Until a root is converted it stays
single-location and nothing here runs on it.

### Root, slots and root-fixed items (design §2.1, §2.2)

```text
<root>\
  actingd.config.json              the one file configuration (never in a slot)
  runtime\<name>.exe               fixed entries: copies of the console forwarder (stable-entry.md)
  ui\<name>.exe                    fixed entries: copies of the graphical forwarder
  tools\actingwatch.exe            root-fixed: the watchdog launcher (the task's action)
  tools\platform-tools\…           root-fixed: adb (pinned 37.0.1; its server runs from here)
  mcp-entry\g1\, g2\, state\       root-fixed: the MCP entry's generations
  A\  B\                           the program slots: SLOT.json and the members
     runtime\…, tools\…, ui\…      each a member's whole zip content with its BUILD-MANIFEST.json
  install\
     writer.lock  slot-A.lock  slot-B.lock
     active.json                   the one active selection (install selection v2)
     selections\sel-<ms>.json      every selection written, immutable
     state.json                    install-state v2 (register and mirror)
     current.json  operations\op-<ms>.json (install-operation v2)
     operations\op-<ms>.attempt.lock, .attempt-<n>.json, .attempt-<n>.spawn.json
     health\runtime-<sel>.json, health\ui-<sel>.json      the two halves of a health confirmation
     installer.json                the installer list and the recovery UI (install-cli 1)
     entry-failures.log, entry-failure-last.txt           the entries' fixed log and fallback
     builds\<commit40>\{runtime,tools,release.json}       kept Runtime/Tools builds
     ui-builds\<commit40>\{ui,release.json}               kept UI builds (release stage, R-15b)
     slot-<X>.new-<ms>.part\       a slot being assembled
     cleaning\<item>-<ms>\         material moved aside for the clean
     candidate\                    the configuration candidate (INST-3)
  watchdog\  state\  packages\  …  unchanged
```

- An A/B root is marked by `install\active.json`; it has no `runtime\BUILD-MANIFEST.json`, so the
  two marks never hold together.
- **A slot** is the three members' whole content and its `SLOT.json`: the Tools are in the slot,
  so one pointer change switches the Runtime, its paired Tools and the UI, and a rollback only
  writes the pointer back. The **root-fixed items** are a fixed list: the entries,
  `tools\actingwatch.exe`, `tools\platform-tools\**` and `mcp-entry\g<N>\`. They only move
  forward and never follow a rollback; each keeps its own protocol version (the launcher's command
  line, the adb pin, `entry_protocol` and the generation folder, the entries' `install-selection`
  range). Candidate Tools whose `platform-tools` bytes differ from the root's refuse
  `platform_tools_changed`: changing adb is a planned event, and Tools never stops the adb server.

### Install selection v2 and the slot document (design §2.3 to §2.6)

```json
{"schema_version": "actingcommand.install-selection.v2",
 "selection_id": "sel-1730352000000", "generation": 3, "slot": "A",
 "slot_document": {"path": "A/SLOT.json", "sha256": "<64 hex>"},
 "members": {
   "runtime": {"repository": "<owner>/<name>", "commit": "<40 hex>", "tag": "v0.12.34", "manifest_sha256": "<64 hex>"},
   "tools":   {"repository": "<owner>/<name>", "commit": "<40 hex>", "tag": "v0.12.34", "manifest_sha256": "<64 hex>"},
   "ui":      {"repository": "<owner>/<name>", "commit": "<40 hex>", "tag": null,      "manifest_sha256": "<64 hex>"}},
 "made_by": "op-1730352000000"}
```

Every failure has its registered code (the contract crate, `InstallSelectionV2::from_json`):

- 1 byte to 16 KiB, else `install_selection_size_invalid`; JSON of exactly this shape (an
  unknown, missing or mistyped member, a slot other than `A` or `B`), else
  `install_selection_json_invalid`;
- `schema_version` exactly `actingcommand.install-selection.v2`, `selection_id` `sel-` and 1 to 20
  digits, `generation` at least 1, `made_by` `op-` and 1 to 20 digits, else
  `install_selection_v2_invalid`; a writer also checks the generation is 1 for a root's first
  selection and the baseline's plus 1 after it (`validate_successor`, the same code);
- `slot_document` a valid file reference (`invalid_install_file_reference`) whose path is
  `<slot>/SLOT.json` (`install_slot_document_mismatch`; it replaces v1's `<slot>/MEMBERS.json`);
- every member: `repository` `owner/name`, `commit` 40 lowercase hex digits, `manifest_sha256` 64,
  `tag` `null` or 1 to 128 visible characters, else `install_member_identity_invalid`;
- the pairing: `runtime` and `tools` have the same `repository` and `commit`, else
  `install_member_pairing_mismatch`. The UI has its own commit; which UI works with which Runtime
  is decided by the interface edges, never by a pair table.
- v2 has no `config`, no `provider` and no `install/generations/` rule: the configuration is
  `<root>\actingd.config.json`, whose online values stay the Runtime's (`host.config_overrides`).
  Updates, rollbacks and the clean never change that file.

A reader that fixes a process's selection (design §2.3): parses it and takes the slot's shared
lock; reads `<slot>\SLOT.json` (at most 64 KiB) and checks its SHA-256; checks the document's
slot and members equal the selection's (`InstallSlotDocument::matches_selection`,
`install_slot_document_mismatch`); reads the three members' `BUILD-MANIFEST.json` and checks their
SHA-256, `repository` and `commit_sha`. Files are hashed one by one only when a slot is assembled,
by `install status --verify`, before a commit, and by an entry for the one program it starts.

`<slot>\SLOT.json` (`actingcommand.install-slot.v1`, `InstallSlotDocument::from_json`): 1 byte to
64 KiB and exactly the shape (`install_slot_document_invalid`), `schema_version` and
`prepared_by` (`op-<ms>`, the same code); each member is the selection's identity plus `source`
(`install/builds/<commit>/runtime` or `/tools`, `install/ui-builds/<commit>/ui`, of the member's own
commit), `files` (at least 1) and `bytes` (`install_member_identity_invalid`); the pairing as
above. `prepare` writes it into the slot's `.part` once; a slot's content is never changed in
place, only retired whole and replaced.

The member versions: a kept build's `release.json` names the tag of its facts, `null` for a local
build; the UI's kept builds come from `release stage` of a UI release into `install\ui-builds\`.

### Slots: copies, occupancy and their states (design §2.8, §2.9)

- `prepare` copies the members from `install\builds\<c>\` and `install\ui-builds\<u>\` into
  `install\slot-<X>.new-<ms>.part\` (each file flushed), writes `SLOT.json` and checks every file;
  `apply` moves the target slot's old content into `install\cleaning\slot-<X>-<ms>` and renames the
  part `X`. No hard link and no build reference: a slot is a private copy, and the kept builds stay
  the untouched source of every update and rollback.
- **Occupancy**, three layers, any one refusing `slot_occupied` before any change: the slot lock's
  exclusive take, the processes whose image lies under `<root>\X\` (CIM), and the rename of the
  folder (OS error 5 or 32). The refusal lists `occupants [{pid, image, kind, pinned_selection?}]`
  (`kind` of `install_occupant_kind`) with `needs_person` `close_slot_occupants`; nothing is ever
  ended. Occupancy is reckoned at plan and again at apply and is not in `plan_id`; only the
  members' part of `requires_stop` is. Only the Runtime this apply stops may occupy it: then the
  placement comes after the close.
- The active slot is never prepared (`slot_is_active`).
- The other slot is in one state of `install_slot_state` (status `slots.<X>.state`):

| state | what | placing new material |
|---|---|---|
| `empty` | no slot folder (the usual state after a clean) | the part is renamed in |
| `previous` | holds `state.previous` while the active selection's health confirmation is incomplete | refused `previous_slot_unconfirmed` (`values.missing` the halves), nothing changed; `next` by the table of "install restart and the next step" (131: until it is confirmed the old slot is the way back) |
| `cleanup_pending` | confirmed, waiting for its occupants | by the three layers; once free it moves into `install\cleaning\` and the part comes in |
| `prepared_left` | a verified candidate an undo left in place | the same |
| `cleanup_failed` | the clean could not empty it | refused `slot_not_clean` while its folder exists (`needs_person` `remove_unlisted_files`, `next` `clean`); leftovers only in `install\cleaning\` do not block |
| `unrecognised` | no `SLOT.json`, or unreadable or not matching | refused `slot_unrecognised` (`needs_person` `inspect_slot {slot}`) |

- **Updates of the UI alone in a row** (design §2.9): the first does not stop the Runtime, which
  then runs from the old slot Y (`runtime_on_previous_slot`, Y `previous`); `install restart`
  writes the Runtime half and the UI half follows (UI-2d's own confirmation, before it a person's
  `--via cli`); the clean empties Y, and the second update goes into Y without a stop; a third
  goes back into X. Passing `--stop --hold <reason> --start` on the first stops only once. Until
  both halves are confirmed any update into Y, a Runtime deploy too, is refused
  `previous_slot_unconfirmed`.

### install-state v2 (design §2.10; private, `install-root` 1)

`actingcommand.install-state.v2` holds `layout: "ab"`, `root_format` (the `install-root`
revision of the root's private files), `active` (a mirror of the pointer: id, generation, the
SHA-256 of its bytes), `previous` (the selection the active one replaced, with
`rollback_material {state: available | unavailable, reason, cleaned_by}`), `slots`, `cleaning`,
`builds`, `ui_builds`, `entries`, `root_fixed`, `packs` and `config_sha256`. `install\active.json`
is the one authority: a mirror behind it is the caveat `state_behind_selection`, a plan refuses
`state_selection_mismatch`, `resume` rewrites it. The health confirmation is not in it (the files
of `install\health\`). `update apply`, `rollback apply` and `convert apply` commit a newly written
selection (a new id, generation plus 1), so its health confirmation starts again; an automatic
rollback and an undo write the baseline's bytes back and keep its id.

### Commands (design §3.1; frozen by R-15b, built by R-15c to R-15e)

```text
actingctl install release stage --root <r> --dir <d> [--facts <f>] [...] [--client-ref <t>] [--detach]
actingctl install prepare   --root <r> [--build <c>] [--ui-build <u>] [--to-selection <sel>] [--client-ref <t>] [--detach]
actingctl install update   plan  --root <r> --prepared <op> [--preview] [--accept-ledger-one-way] [--hold <reason>]
actingctl install update   apply --root <r> --prepared <op> --plan <id> [--stop --hold <reason> [--known-pause A=R]... [--start]] [--ack-guard <id>]... [--client-ref <t>] [--detach]
actingctl install rollback plan  --root <r> --to-selection <sel> [--prepared <op>] [--preview] [--hold <reason>]
actingctl install rollback apply --root <r> --to-selection <sel> [--prepared <op>] --plan <id> [as update apply]
actingctl install convert  plan  --root <r> --ui-build <u> [--preview] [--hold <reason>]
actingctl install convert  apply --root <r> --ui-build <u> --plan <id> --stop --hold <reason> [--known-pause A=R]... [--start] [--ack-guard <id>]... [--client-ref <t>] [--detach]
actingctl install clean    --root <r> [--ui-confirmed <sel> --via ui|cli] [--client-ref <t>] [--detach]
actingctl install restart  --root <r> [--hold <reason>] [--known-pause A=R]... [--ack-guard <id>]... [--client-ref <t>] [--detach]
actingctl install undo     --root <r> --operation <op> [--ack-guard <id>]... [--client-ref <t>] [--detach]
actingctl install resume   --root <r> --operation <op> [--ack-guard <id>]... [--client-ref <t>] [--detach]
actingctl install start    --root <r> [--ack-guard <id>]... [--client-ref <t>] [--detach]
actingctl install stop     --root <r> --hold <reason> [...] [--client-ref <t>] [--detach]
actingctl install release-hold --root <r> --reason <reason> [--player-hold-verified <alias>=<hold_id>]... [--client-ref <t>] [--detach]
actingctl install status   --root <r> [--operation <op> | --client-ref <t>] [--hold <reason>] [--check-rollback] [--verify] [--light] [--entry-failures-after <cursor>]
actingctl install leftovers --root <r>
actingctl install new      --root <new> --build <dir> --ui-build <dir> [--facts <f>]... [--local-build] [...] [--client-ref <t>] [--detach]
```

- `--prepared` names a finished `prepare` whose `.part` is still there and verifies; the plan id
  covers the part's `SLOT.json` SHA-256. `rollback --to-selection` names only `state.previous`; a
  slot of it still intact needs no `prepare`; one cleaned or not intact refuses
  `rollback_material_unavailable` ("The read face", `rollback`).
- **One undo:** `install undo --operation <op>`, a conversion's too. A `clean` is never undone
  (`clean_irreversible`).
- **On an A/B root** `--detach` and `--ack-guard` no longer need `--stop`; `--hold`, `--start`
  and `--known-pause` still come only with `--stop`. A single-location root keeps today's rule.
- **Every changing command takes `--detach` and `--client-ref`**; the read-only commands
  (`plan`, `status`, `leftovers`, `guards`) do not. `--client-ref` is 1 to 64 of
  `[A-Za-z0-9._-]`, journalled, returned and never interpreted.
- **Read-only `--hold`** (v4.1): `update plan`, `rollback plan`, `convert plan` and `status` take
  `--hold <reason>`, checked as apply's `--hold`, only to compute the forms; it changes nothing and
  is not in `plan_id` (the precedent is `install guards --for stop --hold`). The read-only commands
  take no `--known-pause`: which to name is what the forms say.
- **`--known-pause` and `--hold` never share a reason:** a command whose `--known-pause
  <alias>=<reason>` names its own `--hold` answers `actingctl_usage_invalid` and does nothing (as
  today). A pause of that reason is the operation's own (a failed step kept it), taken over by the
  `pause` step; no form lists it and no `next` puts the two together.
- **`release-hold` per instance:** an instance whose player hold is not verified is released only
  by `--player-hold-verified <alias>=<hold_id>`, the `hold_id` equal to the hold read at that
  moment, else refused.
- **`install new --local-build`** takes, as `release stage --local-build`, an exact-commit CI
  build without facts (`tag` null, `release.json` `local_build: true`, journalled): for test roots
  and rehearsals only; production converts, never builds a new root.
- A single-location root keeps `update` and `rollback` until its conversion; after the code for
  them is removed (a later slice) it answers `layout_single_convert_first`.

### The forms (design §3.1)

`update plan`, `rollback plan` and `convert plan` answer `apply_form`; `status` answers
`restart_form` and `start_form` whenever `next` or `cleanup.next` holds `restart` or `start`, and
so do the answers that leave the Runtime on the previous slot or stopped. The UI builds its
buttons only from the forms, `next`, `actions`, `cleanup.next` and `needs_person`, never from a
guard's `values`.

```json
"apply_form": {"requires_stop": true, "allows_detach": true, "keeps_hold_after_start": true,
  "hold_reason": "acsetup_update", "required_acks": ["instructions_dropped"],
  "required_known_pauses": [{"alias": "inst-3", "reason": "<another party's reason>", "revision": 1}],
  "blocked_by": [{"guard": "player_holds", "values": {"player_holds": [{"alias": "inst-1", "hold_id": "…", "hold_phase": "held"}], "player_hold_range": [0, 0]}}],
  "target_slot": "B", "target_slot_state": "empty", "unblock": null,
  "occupants": [{"pid": 4242, "image": "<root>\\B\\runtime\\actingcommand-actingd.exe", "kind": "runtime", "pinned_selection": "sel-…"}]}
```

- `requires_stop`: the candidate changes the Runtime/Tools members, or the target slot is held by
  this root's Runtime, or it is a conversion; then apply needs `--stop --hold <reason>`, usually
  with `--start`. Only the members' half is in `plan_id`; apply reckons the occupancy again.
- `allows_detach` is always true. `keeps_hold_after_start` is `install.hold_after_start` with
  `--start` (default true: the deployment pause stays until a person releases it), `null` without
  a stop.
- `hold_reason` (v4.1) is the read-only `--hold` the form was computed for, `null` when none was
  given. With `requires_stop` the apply's `--hold` equals it; a form whose `hold_reason` is
  `null` is planned again with `--hold` before an apply (the UI always passes `acsetup_update`
  from UI-2c1 on).
- `required_acks`: the guards that do not pass now and are acknowledgeable (`ackable`), each one
  `--ack-guard` once a person confirmed it.
- `required_known_pauses`: every unnamed instance pause of another party (`foreign_pauses`'s
  `values.unnamed`, `{alias, reason, revision}`), computed by `hold_reason`: a pause of that
  reason is the operation's own and is not listed; every other reason is, another deployment
  pause reason's too (an `install_restart` pause a command-line `restart` left). With `hold_reason`
  `null` every instance pause is listed. Each becomes `--known-pause <alias>=<reason>`.
- `blocked_by`: what no acknowledgment or naming passes; the apply is then refused, and the UI
  gives no "run" button. Each kind has its registered `needs_person` (the three forms share the
  table below); a new kind joins `blocked_by` only with its row here.
- `target_slot_state` is the slot state above; `previous`, `cleanup_failed` (folder present) and
  `unrecognised` refuse as the slot table says and add `{guard: "target_slot", values: {state,
  missing?}}` to `blocked_by`; `unblock` then gives `{next, needs_person}` of the table of
  "install restart and the next step" (`null` otherwise).
- `occupants`: the target slot's occupants other than the Runtime this apply stops; not empty
  refuses `slot_occupied` (`close_slot_occupants`).
- The forms are hints of the moment, never in `plan_id`; apply computes everything again.
- **`restart_form`** has the same shape with `requires_stop` always true and `target_slot`,
  `target_slot_state`, `occupants` and `unblock` `null`. Its `hold_reason` is `status`'s read-only
  `--hold`, by default `install_restart`, never `null`; `next.restart` names `--hold
  <hold_reason>`. Its `required_acks` and `blocked_by` are the guards and check-config of
  `install restart`; its `memory` row is the one before the close (the running Runtime's working
  set counted available), which does not pass `restart`'s own start step.
- **`start_form`** (v4.1) has the same shape with `requires_stop` always false, `hold_reason`
  always `null`, `required_known_pauses` always empty (the Runtime is down; S0's pauses are
  checked by the post-start `pauses` row) and the four slot members `null`;
  `keeps_hold_after_start` is true when `kept_holds` is not empty (`install start` has no
  `release_hold` step); `required_acks` holds `adb` (a cold start before the emulators run),
  `memory`, and `player_holds` in the third case without S0; `blocked_by` holds `check_config`
  and `player_holds` out of range; and `s0_from` names the operation whose S0 the start's
  post-start checks will judge against ("judged S0" below), `null` when none counts: then the
  Runtime half stays unconfirmed and `next` will be `restart`, which the UI says on its
  confirmation page. Its `memory` row is read once; `install start` without the acknowledgment
  waits `install.start.memory_wait_ms` again, with it not at all.

| `blocked_by[].guard` | in | when | `needs_person` |
|---|---|---|---|
| `foreign_pauses` (`values.global`) | `apply_form`, `restart_form` | a global pause | `global_pause_blocks {reason, revision}`: the party that set it lifts it; the installer never does |
| `player_holds` (out of range) | all three | a player hold while R\*'s `player-hold` range does not hold 1 | `player_hold_blocks_target {player_holds, player_hold_range}`: wait for the holds to end or use a build that declares `player-hold`; the installer never ends a hold |
| `check_config` | `start_form`, `restart_form` | the root configuration fails the active Runtime's check-config (`check_config_unanswered` or `config_refused_by_target`) | `fix_configuration {code, commit, received}`: change the root configuration (before INST-3 by hand, checked with `check-config --install-root`; after it with `install config`), then read `status` again |
| `target_slot` | `apply_form` | the target slot is `previous`, `cleanup_failed` (folder present) or `unrecognised` | from `apply_form.unblock`: for `previous` the table of "install restart and the next step"; for `cleanup_failed` `remove_unlisted_files` and the like with `next` `clean`; for `unrecognised` `inspect_slot {slot}` |

`occupants` is a member of its own, not a `blocked_by` kind. Every guard outside the table is
acknowledgeable and goes to `required_acks`.

### Phases, steps and the journal v2 (design §3.2, §3.4)

| phase | command or steps | locks |
|---|---|---|
| 1 material and target slot | `release stage`; `prepare`: `materialize_part`, `verify_part` | the attempt lock; no writer lock (the pass list of row 7a″) |
| 2 plan | `update plan`, `rollback plan` (read-only; the baseline is `active.json`'s SHA-256 and generation; `plan_id` covers the inputs, the part's digest, the edges, the defaults diff, check-config, the gate, `instructions_at_risk` and the members' half of `requires_stop`) | none |
| 3 lock, drain, stop, place | apply: the attempt lock, then the writer lock exclusive; the plan again (`plan_changed`); `pause`, `close`, `ledger_verify` as needed; `place_slot`; `refresh_entries`, `refresh_root_fixed` | both to the end |
| 4 persist and commit | `write_selection` (`install\selections\<id>.json`, create new, flushed); `commit_selection` (compare and swap) | held |
| 5 start and check | `record_state`; `start` (entry → forwarder → slot X); `post_check` with the rows `selection`, `members` and `player_holds`; `record_health`; `release_hold` | held to readiness |
| 6 recover | `resume`, `undo`, the automatic rollback, `status` | each by its rules, the attempt lock first |
| 7 clean (131) | `clean_check`, `clean_slot`, `clean_builds`, `clean_converted`, `delete_cleaning`, `record_cleanup` | the attempt lock; no writer lock |
| 3 and 5, Runtime only | `install restart`: `pause`, `close`, `start`, `post_check`, `record_health`, `release_hold` | the attempt lock, the writer lock to the end; off the pass list |

`place_slot`: the exclusive slot lock and the three layers; an existing X (`cleanup_pending`,
`prepared_left`) renamed `install\cleaning\slot-<X>-<ms>` (its `SLOT.json` SHA-256 journalled);
the part renamed `X`; `SLOT.json` and the manifests checked again. With X free it runs before
`pause`; held only by the Runtime this apply stops, after `close`.

The step kinds by phase (vocabulary `install_step_kind`; the UI draws progress by this table and
knows no other word): phase 1 `extract`, `copy_build`, `verify_tree`, `place_build`,
`materialize_part`, `verify_part`; phase 3 `pause`, `close`, `ledger_verify`, `place_slot`,
`stage_file`, `swap_file`, `retire_live_files`, `place_entries`, `refresh_entries`,
`refresh_root_fixed`, `init_slot_locks`; phase 4 `write_selection`, `commit_selection`; phase 5
`record_state`, `start`, `post_check`, `record_health`, `release_hold`; phase 6 `reverse_files`,
`revert_selection`; phase 7 `clean_check`, `clean_slot`, `clean_builds`, `clean_converted`,
`delete_cleaning`, `record_cleanup`. `record_health`'s postcondition is its file present with
the journalled content; `resume` writes it again and `undo` never removes it.

The journal `actingcommand.install-operation.v2` (private, `install-root` 1) adds to v1
`selection {baseline, candidate}`, `prepared {operation_id, slot, slot_sha256}`, `state`
(`pending`, `running`, `ended`), `argv` (the command without `--detach` and `--client-ref`),
`attempts [{n, verb, detached, argv}]` (`attempts[n].argv` that attempt's own command; the first's
is the journal's `argv`), `client_ref`, `outcome_kind`, `chained {clean}` and `health {runtime?}`.
`current.json` keeps its shape; its kinds (vocabulary `install_operation_kind`) add `prepare`,
`convert`, `clean` and `restart`. **Row 7a″'s pass list** adds `prepare` and `clean`; `convert`
and `restart` stand aside; the actingd entry's `entry_operation_unfinished` uses the same list.
`state.json` is written only by `record_state` and `record_cleanup`. The journals and
`state.json` stay private: the UI reads only `status`'s judgments.

### The commit point and the pointer (design §3.3, §3.5)

The commit point is the one rename in `commit_selection` that replaces `install\active.json` with
`install\active.json.<id>.part`: the step journals `expected {baseline_sha256,
baseline_generation}`, reads the pointer (its bytes' SHA-256 must be the baseline's), writes the
part from `install\selections\<id>.json` (create new, `sync_all`), renames it over the pointer,
reads it back equal to the candidate and journals the step done; `start` begins only after that.

| when the baseline changed | the disk | the end | `next` |
|---|---|---|---|
| apply's plan, computed again first | nothing changed | refused `plan_changed`, `selection.active_now` given | plan again |
| `commit_selection`, its read | the Runtime may be closed, X placed, the entries refreshed; the pointer is a foreign selection F | ended refused after its changes, `selection_changed_externally` (`outcome_kind` `refused_after_change`), `changed` listing them; the deployment pause kept, the Runtime stopped; the root consistent (F active, X inactive) | only `start` (from F) and `undo --ack-guard selection_external`; no `release_hold`, which a stopped Runtime refuses |

- **One volume, no links:** the part slot, `install\selections\` and the pointer's part lie under
  `<root>\install\`, on the slots' volume: a cross-volume rename refuses `not_same_volume` before
  any change, and a reparse point or link on the path refuses `reparse_point`.
- **Persistence:** content is written `.part`, `sync_all`, rename; copied slot files are flushed
  one by one; recovery trusts only the pointer's bytes on disk.
- **No `ReplaceFileW`** (two of its three partial failures leave the pointer's name absent): one
  `std::fs::rename` over the old name, which is never absent.
- **A failing rename** (OS error 5 or 32) is tried every 500 ms up to `install.lock_wait_ms`, the
  old pointer read back after each try; past it the operation stays unfinished,
  `install_operation_interrupted` with `selection_commit_failed` as its link, and `next` holds only
  `resume` (rename again) and `undo`.
- **Read loudly:** an A/B root (marked by `slot-A.lock`) without `install\active.json` is
  `install_selection_unavailable` to the contract readers, `watchdog_layout_unknown` to the
  watchdog and `entry_selection_unavailable` to the entries; an `active.json.*.part` without its
  unfinished operation is the caveat `foreign_part`.

### Recovery and the next step (design §3.6 to §3.8)

| interrupted at | `resume` | `undo` |
|---|---|---|
| `prepare`, copying | copies on into the same part and checks it | the part renamed `.abandoned-<ms2>` |
| `pause`, before `close` | goes on | resumes the pauses it set |
| after `close`, before the commit | goes on | starts nothing; `next` `install start` |
| `place_slot`, the old X moved aside, the part not in | renames the part `X` | renames the old X back, the part `.abandoned` |
| `place_slot` done | goes on | does not require X moved away (a UI undoing its own update runs from X): with X free it moves aside and the old X comes back; occupied, X stays (`prepared_left`, caveat `slot_left_prepared {slot, occupants}`); an undo never deletes |
| `refresh_entries`, between an entry's two renames | renames the part in | renames the `.old` back |
| `refresh_entries`, an entry copied over | goes on | copies the `.old` to a part, checks it and renames it over (two renames while it runs) |
| `refresh_entries`, a name newly placed | goes on | keeps it: names are only added; the member's absence answers `entry_target_not_in_member` |
| `commit_selection`, part written, not renamed | checks the baseline again and renames | the part `.abandoned` |
| `commit_selection`, the rename failing to its end | renames again | the part `.abandoned`, the steps before reversed |
| renamed, the step not done | reads the candidate, marks done | writes the baseline back from `install\selections\` (expecting the candidate) |
| done, before `record_state` | rewrites the state | writes the baseline back, rewrites the state |
| `start` or after | goes on to `post_check` by the start's postcondition | writes the baseline back only when the rollback gate passes; otherwise a forward fix only |
| journalled committed, the pointer the baseline (`commit_lost`) | before `start`: commits again; after it: refused `selection_commit_lost`, `needs_person` `selection_commit_lost {active_sha256, candidate_sha256, runtime_slot}` | as uncommitted; after `start` the gate as usual |
| the pointer neither (`external`) | refused `selection_changed_externally`, `needs_person` `inspect_selection_external {active_sha256, baseline_sha256, candidate_sha256}` | refused unless `--ack-guard selection_external` (guard `selection_external`, `not_evaluated`, `ackable`, the values F's SHA-256): then F checked again, its bytes kept as `install\selections\external-<ms2>.json`, the baseline written back (expecting F), the rest reversed |
| the end journalled, `current.json` still there | only renames `current.json` | — |

`status` gives with every row the pointer's real SHA-256 and slot, the journal's steps and the
mirror, and the judgments `commit_state`, `runtime_state` and `actions`.

**`next` holds only commands that run as they are**, each a whole argv the installer accepts in
that state; what a person must do (close occupants, look at a foreign selection, move files out
of `ui\` …) is `needs_person [{code, values}]` (vocabulary `install_needs_person`), which the UI
translates by the code table. The conditions:

- **`install start`** is accepted when all nine hold (v4.1): (1) no unfinished operation;
  (2) the root configuration and its `install` section read, `writer.lock` within
  `install.lock_wait_ms`, `watchdog\run.lock` for the start step; (3) the active programs verify
  (single-location: the live tree; A/B: the reader's four steps of the active slot, else
  `slot_unrecognised` with `inspect_slot`); (4) no live owner and no actingd under the root;
  (5) the root configuration passes the active Runtime's check-config (else `start_form.blocked_by`
  `check_config`); (6) `player_holds` is not out of range (else `blocked_by`); (7) `adb` passes or
  is acknowledged; (8) `player_holds` passes or, in the third case without S0, is acknowledged;
  (9) the start step's `memory` row passes within `install.start.memory_wait_ms` or `install
  start` itself carries `--ack-guard memory` (another command's does not count). `next.start` is
  given only when (1), (3), (4), (5) and (6) hold, with every `--ack-guard` of
  `start_form.required_acks`.
- **`release-hold`**: a running Runtime and no unfinished operation; so a Runtime that is down or
  may be going down never gets it, the hold being released from the start's result.
- **`resume`, `undo`**: an unfinished operation, or `undo` of the last finished one with no later
  changing operation; not after a `clean` deleted, and not while an attempt is handed over.
- **`install clean`**: no unfinished operation (`--ui-confirmed` excepted).
- **`install restart`**: no unfinished operation, a Runtime that runs under the root and answers
  `Status`, the root configuration passing its check-config, `restart_form.blocked_by` empty; the
  argv names `--hold <restart_form.hold_reason>`, every `--known-pause` and `--ack-guard` of the
  form, and never a `--known-pause` of the hold's reason.
- **While a chained clean runs**, the answer and `status` carry `next_after_chained_clean`: every
  `next` but `status` waits for it.

**Who runs an argv** (design §3.6): `next` and `actions` hold the words after the program's name
(`install …`); the caller runs them with the installer it picks from `install\installer.json`
("Start rules"), with both selection variables cleared, as a new child. The installer never runs
a `next` itself. The caller adds only `--detach` and `--client-ref`, never changes or drops a
word. `next.restart` is `["install", "restart", "--root", <r>, "--hold", <hold_reason>,
("--known-pause", "<alias>=<reason>")…, ("--ack-guard", <id>)…]`; to use another reason, read
`status --hold <reason>` again. A refusal `previous_slot_unconfirmed` names its own apply's
`--hold`; `apply_form.unblock` names the plan's `hold_reason`, or `install_restart` without one.
A person copying by hand uses `installer.running.path` or the list's first entry.

**The automatic rollback** (Alice's 74 A, design §3.7) keeps its moment and its triggers; on an
A/B root its steps are `close` (the gate judged first, as a preview), `revert_selection` (the
pointer written back, expecting the candidate; the baseline slot was not touched), the baseline's
`start` and `post_check`, and `release_hold` when they passed. No file is reversed and the
entries stay forward. It ends refused `deploy_rolled_back`, `outcome_kind` `rolled_back`. It
happens before any health confirmation, so the old slot is intact, and no clean follows: X stays
`prepared_left`.

**The rollback gate** (design §3.8) keeps its four parts with "the live build" read as the active
selection's Runtime and "the build it replaced" as `state.previous`; start logs are attributed to
selections by the commit time. A rollback that changes only the UI judges only part 1's U\*
edges. Part 5 `slot_intact`: the target slot's `SLOT.json`, manifests and files verify; else
`rollback_material_unavailable`, its `values` status's `rollback.material` with `{selection_id,
sha256, members, cleaned_by}`: with every build still kept, `next` is `prepare --to-selection
<sel>` and then `rollback plan --prepared <op>` (no download); with a build cleaned,
`needs_person` `restage_previous_build` names only the missing ones.

### Player holds (design §3.10, Workflow #429 R-7a, [player-hold.md](player-hold.md))

- Inside `actingctl install …` a bare "hold" is the installer's deployment pause (`--hold`,
  `release-hold`, `release_hold`, `kept_holds`, `install.hold_after_start`,
  `keeps_hold_after_start`); a player hold always carries `player_hold` (the guard
  `player_holds`, S0's `player_holds`, the post-start row `player_holds`, the adb row's
  `skipped_player_held`, the caveat `player_hold_persistence_off`). R-7a's top-level `actingctl
  hold`, `hold-release` and MCP's `ac_hold*` are the player hold's; the installer never calls them.
  R-7a's value keys are used as R-7a named them: `hold_phase`, `alias`, `source_actor`,
  `source_channel`, `source_via`, `ended_by_actor`, `ended_by_channel`, `ended_by_via`.
- **Guard `player_holds`** (`install_guard`): judged on the R\* of the A/B edge table
  ([component-interfaces.md](component-interfaces.md)); while any hold exists R\*'s
  `player-hold` range holds 1, undeclared [0, 0]; no hold passes (`player_holds: []`). Holds come
  from `Status` while the Runtime runs, else from the S0 of the operation that stopped it (with
  the offline read after the close), counted only by "judged S0". **Without S0:** R\* declaring
  `player-hold` 1 passes (`values.reason` `target_restores_player_holds`); an active Runtime
  declaring none passes (`no_player_hold_support`); otherwise `not_evaluated`, acknowledgeable.
  This narrows R-7a v3.1 §5.3's "without S0, not_evaluated" to the third case; the installer
  follows this text. A failure is `guard_failed` with `{player_holds [{alias, hold_id,
  hold_phase}], player_hold_range}`, never acknowledgeable.
- **After the close** the installer reads `host.player_hold` once more offline, by the whole key
  of its index row, never by a ledger scan (`not_applicable` for a live build without
  `player-hold`); each hold found joins S0 with `seen_by` `offline_after_close`, and the guard is
  judged again. A failure, or a failed read while R\* does not know holds, ends the operation
  refused after its changes (`guard_failed`, `next` `start` and `undo`); a failed read while R\*
  knows them is the caveat `player_hold_offline_read_failed`.
- **The post-start row `player_holds`** (fail-class) passes for each hold of S0 that is restored
  (same instance and `hold_id`, `held`, also `restored_from_phase: acquiring`), or was formally
  released after the start (gone or `releasing`, this epoch's `player_hold_operation` a `release`
  of that `hold_id`, `running` or `completed`; values `ended_by_*`), or was released and taken
  again in this epoch (another `hold_id` without `restored_from_owner_epoch`, or an `acquire` of
  another `hold_id`; values `ended_then_reacquired {hold_id, source_actor, source_channel,
  source_via?}`); otherwise it fails, and so does a hold in `player_holds_dropped`.
- `StartReport.player_hold_not_durable` is the caveat `player_hold_persistence_off` (a warning;
  `values.start_report_member`), never R-7a's error code of that name.
- `release_hold` releases a deployment pause per instance only where S0 has no hold or the three
  conditions pass again at that moment; otherwise the pause stays in `kept_holds` (`why`
  `player_hold_unverified`, `needs_person` `player_hold_unverified {alias, hold_id}`).
- `kept_holds [{alias, reason, operation_id, since_unix_ms, why}]` (`why` of
  `install_kept_hold_why`) comes from the journals and, with a running Runtime, is compared with
  its pauses (`kept_hold_mismatch`); a UI recognises deployment pauses only by it and the
  unfinished operation.

### Background operations and attempts (design §3.11; both layouts, from R-15d)

- **The attempt lock** `install\operations\op-<ms>.attempt.lock`, one per operation, empty, held
  exclusively by the process that runs an attempt, from before anything to the attempt's end
  (foreground, background child, `resume`, `undo`, every kind). It is not `writer.lock`: it only
  says that one process at a time runs this operation. `prepare` takes no writer lock (the
  watchdog probes the writer lock only when it considers a start; holding it for a copy would only
  keep a crashed Runtime down).
- **Minting is create new:** a new id `op-<ms>` creates its attempt lock file; a name taken adds 1
  ms and tries again, at most 1000 times, then `operation_id_unavailable`.
- **The attempt record** `op-<ms>.attempt-<n>.json` `{n, verb, pid, process_created_unix_ms,
  detached, started_at_unix_ms, log, ended_at_unix_ms, outcome}`, written by the attempt's own
  process (foreground too); a background attempt also has the spawn record
  `op-<ms>.attempt-<n>.spawn.json` `{n, state: spawning | spawned | spawn_failed, parent_pid,
  parent_created_unix_ms, spawned_pid, process_created_unix_ms, log, code}`, written only by its
  parent. Each has one writer; each write goes `.part`, flush, rename, the first too, which
  refuses `attempt_record_exists` when the final name exists. Readers read final names only.
- **A changing command with `--detach`** (`release stage`, `prepare`, `update apply`, `rollback
  apply`, `convert apply`, `clean`, `stop`, `start`, `restart`, `release-hold`, `new`):
  1. the parent's cheap checks (an unfinished operation, the configuration) refuse at once; these
     refusals come before the mint and have no id. `clean --ui-confirmed --detach` does the clean's
     steps 1 to 3 here: a stale selection refuses `ui_confirmation_stale` unminted, the UI half is
     written, and with another operation unfinished it mints, ends the journal `cleanup_deferred`
     without `current.json` and answers at once (`detached: false`);
  2. the parent mints, writes the journal `state: pending`, `kind`, `argv`, `client_ref`,
     `attempts [{n: 1, verb: "apply", detached: true, argv}]`, then creates `current.json` (a lost
     race ends its own journal `refused` and answers `operation_unfinished`);
  3. it writes the spawn record `spawning`, then spawns the child by the watchdog's manual-run
     method; **the child's command line** is the parent's own command without `--detach` and
     `--client-ref <t>`, plus `--operation-id <op> --attempt <n>`, never the operation's `argv`;
     a failed spawn writes `spawn_failed` and ends the journal `detach_spawn_failed`;
  4. it releases the attempt lock and answers done at once: `install_operation_id`, `client_ref`,
     `detached: true`, `pid`, `log`, `selection` and `next: {status: [… "--operation", <op>]}`;
  5. the child takes the attempt lock, writes its attempt record and checks its identity; a
     mismatch is only its own record's `detach_identity_mismatch`, except for the first attempt of
     an operation with no change yet, whose journal ends `refused` with it; a journal with changes,
     or a `resume` or `undo` attempt, stays unfinished: `interrupted` with the caveat
     `operation_attempt_mismatch {attempt, verb}`;
  6. **"this operation itself"** holds when all five do: `current.json` names it, this process
     holds its attempt lock, its `--attempt` is the journal's latest, its command (without the
     internal arguments) equals `attempts[n].argv` and its verb `attempts[n].verb`, and the journal
     is not ended; whatever judges "an unfinished operation" (`no_unfinished`,
     `refuse_unfinished`) counts such an operation as none;
  7. a refusal before the operation opens (`plan_changed`, `guard_failed`, `writer_lock_failed`,
     `rollback_gate_failed`, `slot_occupied`, `interface_edge`, `previous_slot_unconfirmed`, …)
     is written under the id: `state: ended`, the whole refusal as `result`, `changed` empty,
     `outcome_kind: refused`, `current.json` renamed `finished-<ms>`;
  8. then the foreground's rules, step by step.
- **`resume` and `undo` with `--detach`** mint nothing: the parent tries the attempt lock without
  waiting (the short retry below), held is `operation_attempt_running` (`attempt_phase` `running`);
  taken, it checks the handover first (`attempt_phase` `handover`), adds attempt `n+1` with its own
  verb and `argv`, writes the spawn record, spawns (the same command-line rule) and answers. It
  never takes the writer lock.
- **Handover:** the latest attempt has a spawn record and no attempt record, and the spawned child
  is alive (`spawned`, pid and creation time checked) or the parent is (`spawning`). Then the
  state is `pending`, both actions are unusable (`operation_attempt_running`, `attempt_phase`
  `handover`) and `resume` and `undo` refuse with that code; once neither process lives the state
  is `interrupted` with `operation_attempt_lost`.
- **The short retry:** `status`'s shared try and `resume`'s and `undo`'s exclusive try of the
  attempt lock try 4 more times 20 ms apart (about 100 ms, fixed by this contract, no key) before
  they conclude.

| `operation_state` (`install_operation_state`) | when |
|---|---|
| `pending` | handed over (caveat `operation_attempt_unconfirmed {attempt, spawned_pid?, since_unix_ms}`), or `state: pending` with the attempt lock held |
| `refused` | ended `refused`: nothing changed |
| `running` | not ended, the lock held, the latest attempt reported |
| `interrupted` | not ended, the lock free, no handover; caveat `operation_attempt_lost {attempt, pid?, log?}`, or `operation_attempt_mismatch` after a mismatch |
| `ended` | ended, `outcome_kind` not `refused` |

`outcome_kind` (`install_outcome_kind`, journalled at the end): `done`, `done_with_caveats`,
`refused`, `refused_after_change` (`changed` lists the changes), `rolled_back`
(`deploy_rolled_back`, `convert_rolled_back`), `undone`, `abandoned` (undone before any step).
An undo in progress is `running` with `progress.attempt.verb` `undo`.

- **Finding a result again:** `install status --root <r> --client-ref <t>` answers the newest
  operation with that reference as `--operation` does; none is `client_ref_unknown`, which the UI
  shows as an unconfirmed result, never a success. A UI sends every changing command with
  `--detach` (UI-2a), so every refusal after the mint has an id.
- **`recent_operations`** (always): the 20 newest operations, ended or not, the refused
  background ones included, `{operation_id, kind, client_ref?, operation_state, outcome_kind?,
  code?, started_at_unix_ms, ended_at_unix_ms?, chained_clean?}`. It replaces `last_operations`,
  which a single-location root answers until its code is removed.
- **`progress`** `{stage, step_kind, step_n, steps_total, step_state, attempt: {n, verb},
  sample?}`: `stage` the phase 1 to 7; `prepare`'s copy and check and the clean's deletions write a
  sample `{at_unix_ms, files_done, files_total, bytes_done, bytes_total}` every 2 seconds (fixed,
  no key); `start` keeps its samples.
- **`prepare` answers** done with `install_operation_id`, `client_ref`, `installation_changed:
  true`, `prepared {operation_id, slot, part, slot_sha256}`, `selection {baseline, candidate,
  active_now}`, `members {runtime, tools, ui}` (each `{commit, tag, source, files, bytes}`),
  `verified {files, bytes}` and `next.plan` (`update plan … --prepared <op> --preview`, or
  `rollback plan … --to-selection <sel> --prepared <op> --preview` with `--to-selection`).

### The read face (design §3.12; `install-cli` 1)

- **The selection triple** in every answer on an A/B root: `selection {baseline, candidate,
  active_now}`, each `{selection_id, sha256, slot, generation, members {runtime, tools, ui:
  {commit, tag}}}` (`active_now` read from the pointer's bytes now: `null` when absent, only its
  `sha256` when foreign and unreadable). An update of the UI alone differs only in `ui`.
- **A single-location root** answers `selection: null` and `live {runtime, tools: {commit, tag},
  ui: null | {kind: "u0", files [{name, sha256, bytes, same_as [{source}]}]}}`; the old `ui\`
  programs carry no manifest and their identity is not guessed.
- **The judgments** (for `--operation` or `--client-ref`, else the unfinished operation):
  `operation_state`, `outcome_kind`; `commit_state` (`install_commit_state`: `none` for `stop`,
  `start`, `restart`, `prepare`, `release-hold`, `clean`; `uncommitted`, `committed`, `reverted`,
  `commit_lost`, `external`); `runtime_state` (`install_runtime_state` by the CIM image path:
  `stopped`, `on_active_slot`, `on_previous_slot` with the caveat `runtime_on_previous_slot {slot,
  selection_id}`, `on_unknown_image`, `unknown` with the probe's failure, and v4.1's
  `not_answering` `{pid?, image?, slot?, since_unix_ms?, cause}`: an actingd under the root or a
  held owner lock while `Status` does not answer; the three `on_*` only for a Runtime that
  answers); `actions {resume: {usable, reason?, values?, argv?}, undo: {usable, reason?, values?,
  needs_ack?, argv?}}` (`reason` of `install_action_reason`); `progress`; `next`, `needs_person`;
  `next_after_chained_clean`.
- **The root members** (always): `recent_operations`; `kept_holds`; `installers`, `recovery_ui`,
  `installer.running`; `slots {A: {state, selection_id?}, B: …}`; `health {selection_id,
  runtime: {state, by?, verb?, at_unix_ms?}, ui: {state, via?, at_unix_ms?}, missing}` (states of
  `install_health_state`, halves of `install_health_half`, `via` of `install_health_via`;
  computed from `active.json` and `install\health\`; the UI half is `not_required` when the UI
  member is the replaced one; a first selection's UI half, a conversion's included, is always
  `unconfirmed`); `cleanup {state, operation_id?, blockers [{code, values}], remaining [{path,
  files}], next, needs_person}` (`install_cleanup_state`; blockers of `install_cleanup_blocker`;
  occupancy reckoned now by the first two layers, so a freed slot is `ready` without any command);
  `restart_form`, `start_form`; `rollback {target: {selection_id, sha256, members}, available,
  reason?, material: {slot, builds: {runtime, ui}}, next?, needs_person?}` (`null` without
  `state.previous`; the slot `intact`, `cleaned` or `not_intact`, each build `kept` or `cleaned`:
  intact gives `next` `rollback plan --to-selection <sel> --preview`, builds kept give `prepare
  --to-selection <sel>`, a build cleaned gives `restage_previous_build` for the missing ones);
  `entry_failures {entries [{at_unix_ms, entry, variant, code, values}], cursor, size_bytes,
  rotated_since_cursor}` (the cursor `"<file code>:<byte position>"`, the file code the first 16
  hex digits of the SHA-256 of the log's first whole line, `empty` while it has none;
  `--entry-failures-after <cursor>` answers the whole lines after it when the code matches, else
  from the file's start with `rotated_since_cursor: true` and the rotated files named; a cursor of
  `empty` never counts as a rotation; without a cursor the last 20 lines); `live` on a
  single-location root.
- **The root caveats** (`status` always, a changing command when they arise): `ui_restart_required
  {slot, pids}`, `runtime_on_previous_slot`, `state_behind_selection`, `foreign_part`,
  `entry_modified`, `installer_not_newest`, `legacy_autostart`, `slot_left_prepared`,
  `operation_attempt_unconfirmed`, `operation_attempt_lost`, `operation_attempt_mismatch`,
  `kept_hold_mismatch`, `cleanup_pending`, `cleanup_failed`, `cleanup_waiting_health {missing}`,
  `cleanup_deferred {operation_id}`, `player_hold_persistence_off`,
  `player_hold_offline_read_failed`.
- **`--light`** (polled every 1 to 2 seconds during an operation) reads only files under
  `install\` plus one try of the attempt lock (and a pid query for the handover): no CIM, no
  Runtime, no owner record, no digest, no ledger. What it cannot give is `null` with `light:
  true`: `runtime_state`, `ui_restart_required`, the occupants, `kept_hold_mismatch`,
  `entry_modified`, the file-checking parts of `rollback`, `restart_form`, `start_form`, and
  `cleanup.state` where only occupancy tells `waiting_occupants` from `ready`.
- **Guard rows on the wire** `{id, result, values, ackable, informational?}`: `ackable` is
  serialized. `--ack-guard` takes, beside today's four, `disk`, `root_inventory`,
  `instructions_dropped`, `selection_external` and `player_holds` (the third case without S0).
  The global pause, an unnamed pause of another party and `player_holds` out of range are never
  acknowledgeable.
- **`install-cli` 1** is this face: the command forms with the read-only `--hold`, the three forms
  and the `blocked_by` table, the answer members and words of this section and of background
  operations, the step-kind table, the clean's and `install restart`'s read face, the codes,
  `install\installer.json`, the release-facts file the UI writes, the graphical entry's exit code,
  its stdout line, `--entry-report` and `ACTINGCOMMAND_ENTRY_NO_DIALOG`, and the UI's duty: a UI
  that declares `install-cli` with 1 supports `acui.exe --recovery --root <r>`, which takes no
  installation identity, finds the installer by `installer.json`, reads only `status` and builds
  its recovery page from `next`, `actions`, `cleanup.next` and `needs_person`, never starting the
  Runtime itself. The Runtime declares [1, 1] from R-15d, the UI from UI-2a; `convert`, `install
  config` and `install instance` join revision 1 unraised.

### Start rules, the installer list and recovery (design §3.13)

- **Clear the selection variables.** Whoever starts a UI (`<root>\ui\<name>.exe`), a Runtime or an
  installer first clears `ACTINGCOMMAND_INSTALL_ROOT` and `ACTINGCOMMAND_INSTALL_SELECTION`, so the
  started program resolves `active.json` itself (the watchdog already does; the installer's start
  does; the UI follows). Only a process that fixed its selection keeps it for a tool of its own
  slot (`pin_child`), which never passes a fixed entry; an entry refuses an inherited selection
  that differs from the pointer.

| command | run by | environment |
|---|---|---|
| `actingctl install …`, `install status` included | the actingctl the caller picks from `install\installer.json` by its own `install-cli` range | cleared, `--root` named |
| other commands that change the root: `actingctl watchdog install`, `watchdog uninstall`, `actingd unlock-owner` | the fixed entry `<root>\runtime\<name>.exe` | cleared |
| read-only commands: `overview`, `packs`, `devices`, `actingctl status`, `mcp-serve`, `mcp-config`, `actingd check-config` | the fixed entry, or a process's own slot program | cleared, or inherited by `pin_child` |
| starting the Runtime | only `install start` or `install restart` (first row), never the slot's actingd directly | as the first row |

- **`install\installer.json`** (`actingcommand.install-installer.v1`, `install-cli` 1):
  `{schema_version, root, root_format, selection_version, written_by, installers [{path, commit,
  tag, install_cli, install_root, install_selection, workflow_run_id}], recovery_ui {path, commit,
  tag, install_cli, argv: ["--recovery", "--root", <root>]} | null}`. An installer is a kept build
  `install\builds\<c>\` that verifies; on an A/B root its Runtime also declares `install-selection`
  with 2 and `install-root` with the root's `root_format` (a single-location root,
  `selection_version: null`, needs only the check). Order: the larger `install_cli` max first,
  then the larger `workflow_run_id` (a manifest without one counts 0), then the commit. A UI takes
  the first whose `install_cli[1]` lies in its own range, else shows `installer_unavailable` with
  the list. `recovery_ui` is a verified UI build of `install\ui-builds\` whose `install-cli` holds
  an installer's `install_cli[1]`: first the active UI member's build when confirmed, then the
  confirmed build with the largest `workflow_run_id`, then any with the largest; `null` when none.
  The file is rewritten (`.part`, flush, rename) at every `release stage` and every operation's
  end; a UI reads it and never writes it. `status` answers the same with `installer.running
  {path, commit, install_cli}` and the caveat `installer_not_newest {first, running}` when another
  than the first runs. A UI never runs install commands from `<root>\runtime\actingctl.exe`: after
  a rollback it is the older installer.
- **An entry absent or refusing** (between a running entry's two renames, a conversion's
  `retire_live_files` and `place_entries`, a missing or unreadable pointer, an entry's refusal):
  the graphical entry's dialog offers `recovery_ui` ([stable-entry.md](stable-entry.md)); a UI or
  an outside GUI reads `installer.json`, picks an installer and runs `install status --root <r>`
  with a cleared environment (on a single-location root that never met an installer of R-15c or
  later, `<root>\runtime\actingctl.exe install status`); failing both it shows
  `installer_unavailable` with the root and the manual way through any
  `install\builds\<c>\runtime\actingctl.exe`.
- **While an operation is unfinished:** the actingd entry refuses a start of an operation off row
  7a″'s pass list, `entry_operation_unfinished`, except the installer's own start, which carries
  `ACTINGCOMMAND_INSTALL_OPERATION=<op>` ([stable-entry.md](stable-entry.md)); `actingd
  unlock-owner` refuses while `install\current.json` exists, `unlock_owner_install_unfinished`
  (actingd), since the undo and the gate judge "a Runtime started since" by the owner record; the
  UI's start runs only the picked installer's `install start`, after it showed `start_form`.

### The clean, phase 7 (Alice's ruling 131, design §3.14; from R-15d)

The ruling: once a new version is installed and its health confirmation passed, the previous
version goes too: the replaced slot is emptied (the spare slot is empty again, so the next update
goes straight in) and the old program builds nothing refers to are deleted. Its four constraints
are below. On a single-location root nothing is deleted ("Kept files").

- **An operation of its own:** kind `clean`, with its id, journal, attempt lock and result; on the
  pass list of row 7a″, no writer lock, never undone (`clean_irreversible`; `resume` finishes it).
  It opens by itself after every changing operation but a clean when both halves are confirmed and
  something is to be cleaned (the earlier answer carries `chained {clean}`, `cleanup` and
  `next_after_chained_clean`), and by hand with `install clean --root <r>` or `install clean
  --root <r> --ui-confirmed <sel> --via ui|cli`.
- **Constraint 1, the health confirmation.** Two halves, each an immutable file by selection id:
  `install\health\runtime-<sel>.json` (`actingcommand.install-health.v1`, `half: "runtime"`,
  `selection_id`, `active_sha256`, `by`, `verb`, `at_unix_ms`, `rows`) is written by the step
  `record_health` when an installer operation started the Runtime from the active slot and its
  post-start checks **judged S0** with `ready`, `selection`, `members`, `pauses`, `player_holds`
  and `config` passing and no other fail-class row failing (the composite with `--start`,
  `convert`, `install start` with S0, `install restart`); `install\health\ui-<sel>.json`
  (`half: "ui"`, `via`, `client_ref?`, `ui {commit, tag}`, `at_unix_ms`) by `install clean
  --ui-confirmed`, needed only when the UI member changed (`not_required` otherwise; a first
  selection always needs it). A file is written `.part`, flushed and renamed only when its final
  name is absent; it is a record, never deleted. A confirmation never passes to another selection
  id. Until both halves exist the replaced slot stays whole (`previous`) and `cleanup.blockers`
  says what it waits for (`runtime_unconfirmed`, `ui_unconfirmed`).
- **`install clean --ui-confirmed <sel> --via ui|cli`:** (1) `<sel>` not the active selection's
  id refuses `ui_confirmation_stale {confirmed, active}` and writes nothing; (2) the UI half is
  written before any other check; (3) with another operation unfinished it mints an id and ends
  its journal at once, `done_with_caveats` with `cleanup_deferred {operation_id}`, no
  `current.json` (that operation's chained check takes the confirmation); (4) otherwise a clean
  opens: without the Runtime half it ends `done_with_caveats` with `cleanup_waiting_health
  {missing}`, with an occupant `done_with_caveats` with `cleanup_pending`, else it cleans; (5) every
  answer carries `health` and `cleanup`. `--via` is required (a usage error without it); the
  installer records it as given. UI-2d uses only the selection its new acui fixed and treats a
  `health.selection_id` that differs as stale.
- **Constraint 2, occupancy across slots.** Occupants (Runtime, `mcp-serve`, CLI, UI, Lab …) are
  never ended; the clean waits for them to move or exit (the Runtime by `install restart`, the MCP
  entry g2 by its switch, the UI by UI-2d's relaunch). An item is free when all three layers of
  the slots pass. An occupied item stays and the clean ends done with the caveat `cleanup_pending
  {slot, occupants}`; `status` reckons the occupancy each time, so `cleanup.state` turns `ready`
  with `cleanup.next` `clean` once they left. A deletion that fails ends done with `cleanup_failed
  {items [{path, remaining, reason}]}` (`reason` of `install_cleanup_failure`), `cleanup.state`
  `failed`, and never rolls the new version back nor fails the operation before it.

| `reason` | what | `needs_person` | then |
|---|---|---|---|
| `delete_failed` | the OS refused a deletion | none | `clean` again |
| `unlisted` | files outside the list keep the folder | `remove_unlisted_files {paths}` | `clean` takes the empty folders; left alone it blocks no placement |
| `reparse_point` | a junction or link in the item, never followed or deleted | `remove_reparse_point {paths}` (the link only) | as above |
| `modified_listed_file` | a listed file's size or SHA-256 differs | `inspect_modified_file {path, expected_sha256, actual_sha256}` | `clean` once handled |

`cleanup.next` is always `clean`: it deletes only what its list still holds.

- **Constraint 3, a strict whitelist,** each class deleted file by file by its list:

| class | what | its list | when |
|---|---|---|---|
| the old slot | the replaced slot Y, and slot material `place_slot` or an undo moved into `install\cleaning\slot-*` | `SLOT.json`, each member's `BUILD-MANIFEST.json` and the files it lists | Y: both halves confirmed and free; moved material: its operation ended |
| old Runtime/Tools builds | `install\builds\<c>\` outside the keep set with a `workflow_run_id` below the active Runtime build's | `release.json`, both manifests and their files | the confirmation passed |
| old UI builds | `install\ui-builds\<u>\` the same against the active UI build | `release.json`, the manifest and its files | the confirmation passed |
| a conversion's single-location material | the `<f>.old-<ms>` its `retire_live_files` made, and the `legacy_program_copy` and `legacy_program_file` entries its journal lists with their evidence | the paths, sizes and SHA-256 values its journal records (`after_health: clean`) | the conversion's confirmation passed |

- **The keep set** (never deleted): the three active members' builds; the builds a finished,
  unapplied `prepare`'s part names; the running installer's own build; the installers
  `installers` picks for the active UI's and `recovery_ui`'s `install-cli` ranges; the
  `recovery_ui` build; builds staged after the active selection was committed. "Old" is by
  `workflow_run_id`: after a rollback the newer build stays.
- **Paths:** every item's canonical path lies under `<root>\A`, `<root>\B`,
  `<root>\install\cleaning\`, `<root>\install\builds\<40 hex>` or `<root>\install\ui-builds\<40
  hex>`, or is a path the conversion journalled, with no reparse point from the root to it; a
  reparse point inside is never followed (`reparse_point`).
- **How:** the item renamed into `install\cleaning\<kind>-<id>-<ms>\` (`clean_slot`,
  `clean_builds`, `clean_converted`), then each listed file deleted after its size and SHA-256
  are checked (`delete_cleaning`), then the empty folders bottom up (`remove_dir`, never
  `remove_dir_all`); what stays is reported, never called clean.
- **Never touched:** `state\` and every state root (`state.placeholder-*` too); the configuration,
  its backups and `install\candidate\`; the standard-pack root and every folder the configuration
  names; account material; `_archive\`, `_deployment\`, `downloads\`, `installed-members.json`;
  the records (`install\operations\`, `install\selections\`, `install\health\`,
  `install\current.json.finished-*`, `install\detached-*.log`, `install\entry-failures.log*`,
  `install\state.json.v1-*`); the root-fixed items; the entries and the forwarders' `.old-<ms>`
  copies; the active slot; `watchdog\` and the root's logs; the lock files.
- **Constraint 4, the rollback material told as it is.** When `clean_slot` first touches the old
  slot, `state.previous.rollback_material` becomes `{state: unavailable, reason: cleaned,
  cleaned_by}` (and again in `record_cleanup`); the previous selection's id, SHA-256 and file in
  `install\selections\` stay. `status`'s `rollback.material` says whether the slot can be
  assembled again from kept builds (`prepare --to-selection`) or a build must be staged again
  (`restage_previous_build`); the replaced update's `undo` turns unusable,
  `rollback_material_unavailable`, and so does a conversion's after its clean.
- **Its steps:** `clean_check` (journals what goes, by which list, and what stays and why; done
  again on resume), `clean_slot` and `clean_builds` and `clean_converted` (what moved is not moved
  again), `delete_cleaning` (deletes what its list still holds), `record_cleanup` (rewrites
  `state.json` and `installer.json`). On an A/B root `leftovers` still deletes nothing and lists
  what the clean does not take and its leftovers in `install\cleaning\`.

### `install restart` and the next step of every state (design §3.15; from R-15d)

`actingctl install restart --root <r> [--hold <reason>] [--known-pause A=R]... [--ack-guard
<id>]... [--client-ref <t>] [--detach]` restarts only the Runtime, under a deployment pause, from
the active slot (Workflow #429 review v3 block 1, the coordinator's option ②).

- **What:** before any change the active Runtime's check-config of the root configuration (as
  `install start`; `check_config_unanswered` or `config_refused_by_target`, nothing changed); then
  `pause`, `close` (with the offline read), `start` (the entry → forwarder → the active slot),
  `post_check` (every A/B row, against the S0 its own close took), `record_health`, and
  `release_hold` unless `install.hold_after_start` keeps it.
- **What not:** no file, no slot, no pointer, no entry changes; it never commands an emulator. A
  Runtime of 0.12.34 or later starts no emulator of a paused instance (PAUSE-AUTO), so under the
  deployment pause a stopped emulator stays stopped; an A/B root's Runtime is never older than
  R-15c. On a single-location root the installer cannot tell the version and only says it
  commands no emulator.
- **Locks:** the attempt lock; the writer lock exclusive from before the pause to the end; off
  row 7a″'s pass list, so an interrupted restart keeps the watchdog and the actingd entry aside
  and `resume` starts the Runtime from the active slot.
- **Guards:** as the composite with `--start` (`foreign_pauses` with `--known-pause`,
  `schedule_near` listed, `adb`, `memory` with the running Runtime's working set counted) plus
  `player_holds` on the active Runtime; judged before the close, so a Runtime that does not answer
  `Status` has no restart. Its `--ack-guard memory` passes only that row, never its own start
  step.
- **`--hold`:** by default the registered `install_restart` (vocabulary
  `install_default_hold_reason`), else the reason given; a deployment pause reason like
  `acsetup_update` (UI-2c1 on), recognised by `kept_holds`.
- **A failed check** ends `post_check_failed`, `refused_after_change`, the pause kept, no health
  written, no rollback; the Runtime runs: `next` `release_hold`, `needs_person` the failed rows.
- **A start that does not come up** ends refused after its changes, the pause kept, no health:

| case | what | the Runtime | `next` | `needs_person` |
|---|---|---|---|---|
| one | every attempt exited with an error, or one fatally (`exited_during_startup`) | gone | `start` with `start_form`'s acknowledgments | — |
| two | not ready at `install.start.ready_timeout_ms` (`start_timeout`; left running, never a second beside it) | present, not answering | none | `runtime_not_answering` |
| three | the start step's `memory` row refused (`guard_failed` before the first spawn, or a failure of the start after an attempt exited) | gone | `start --ack-guard memory` with the form's other acknowledgments | — |

`install start` fails the same three ways; the composite with `--start` and `convert` still roll
back (or undo) by themselves in case one.

- **Not between 04:40 and 05:40 JST on Mondays and Thursdays** (the machine's planned restart at
  05:20): a restart, or any operation that stops the Runtime, sent then could be cut between its
  close and its start and keep the Runtime down until a person resumes it. The installer reads no
  scheduled task: the sender keeps this rule (the coordinator's deploys, the UI's buttons).
- **Answer:** done with `start`, `post_check`, `hold`, `health`, `cleanup`, `chained.clean` and
  `outcome_kind`. A single-location root takes it too, without the `selection` and `members` rows
  and without a health file.

**The next step of every state** (`status`'s `next`, `cleanup.next` and `needs_person`, the `next`
of `previous_slot_unconfirmed`, `apply_form.unblock`):

| state | when | `next` | `needs_person` |
|---|---|---|---|
| the Runtime is down | `runtime_state: stopped` | `start` with `start_form`'s acknowledgments; it writes the Runtime half when it judges S0 and the six rows pass; none while `start_form.blocked_by` is not empty | `start_form.blocked_by`'s rows (`fix_configuration`, `player_hold_blocks_target`); an active slot that does not verify `inspect_slot` |
| it runs from the old slot | `on_previous_slot` | `restart` | — |
| it runs from the active slot, unconfirmed | `on_active_slot`, `health.missing` holds `runtime` | `restart` | — |
| it runs from outside both slots | `on_unknown_image` | `restart` | `inspect_runtime_image {image}` |
| the probe failed | `unknown` | — | the probe's failure `{code, values}` |
| present, not answering | `not_answering` | none (`start` and `restart` both refuse) | `runtime_not_answering {pid?, image?, slot?, since_unix_ms?, cause}`; whether to end the process is the person's decision |
| only the UI half is missing | `health.missing` is `ui` alone | none (from UI-2d the new acui confirms itself) | `confirm_ui_running {selection_id, ui {commit, tag}, confirm_argv}`, `confirm_argv` the `install clean --root <r> --ui-confirmed <sel> --via cli` a person runs after seeing the new UI start |
| both halves, the old slot held by this root's Runtime | `cleanup.state: waiting_occupants`, a `runtime` occupant | `restart` | — |
| both halves, held by another process | `waiting_occupants` | none: `ready` once it leaves | `close_slot_occupants {occupants}` |
| both halves, free | `ready` | `clean` | — |
| the clean failed | `failed` | `clean` | by its failure table |
| an operation is unfinished | `current.json` exists | by M2 and M3 | as that operation's |

- **M1:** every `next` of a row first meets the acceptance conditions of "Recovery and the next
  step"; one that does not is left out and its reason goes to `needs_person` by the `blocked_by`
  table.
- **M2:** with `current.json` only its row counts: no `start`, `restart`, `clean` or
  `release_hold` (a chained clean in progress too).
- **M3:** that operation `running` or `pending` (handover included) gives only `status
  --operation <op>`; `interrupted` gives its usable `resume` and `undo`; an unfinished `clean` has
  only `resume`.
- **M4:** without an unfinished operation the answer joins the Runtime row (one of the six
  `runtime_state`s) and the health and clean row; both rows' `needs_person` are given, the
  Runtime's `next` first. A missing Runtime half keeps the clean `waiting_health`, so `clean` and
  `restart` never come together; a stopped Runtime with both halves and a `ready` slot gives
  `start` and `clean`, in either order.
- **M5:** with nothing to clean `cleanup.next` is `null` and `needs_person` empty; `release_hold`
  may still come for a running Runtime with `kept_holds`.

**Who writes the Runtime half:** the composite with `--start`, `convert` and `install restart`
(each judges the S0 of its own close); `install start` when it judges S0. Never the watchdog, the
start after a planned restart, `actingctl start`, `actingctl restart` or a person starting a
fixed entry: none of them takes S0 or the installer's post-start checks, and the table then gives
`restart`. **Judged S0:** a start's checks judge S0 when that S0 describes the Runtime the start
follows: the composite, `convert` and `restart` always (their own close); `install start` takes
the S0 of the newest operation P that stopped the Runtime, and it counts only when after P's
`close` step no Runtime became ready: the start logs since then are only P's own attempts that
ended `exited_during_startup`. Any other start log since (the watchdog's, a restart's, an
`actingctl start`'s, or P's own start that became ready or timed out) means no S0: the start is
checked without it, writes no Runtime half and `start_form.s0_from` is `null`. R-15d changes
`last_stop` to this definition on both layouts (it compares P's end today).

### The conversion's checks and inventory (design §5.2, §5.3; R-15e builds them)

`convert plan` judges each recognition condition as a row `{check, result: pass | fail, values}`
(vocabulary `convert_check`); all must pass, else `convert_layout_unrecognised` with the rows and
nothing changed. Their `values` keys are frozen; a key added later goes by `install-cli`'s bump
rule:

| check | `values` |
|---|---|
| `single_location` | `{manifest: "runtime/BUILD-MANIFEST.json", present}` |
| `no_ab_traces` | `{found: [path…]}` |
| `state_v1` | `{schema, live_commit, manifest_commit}` |
| `live_verified` | `{files, mismatched: [{path, expected_sha256, actual_sha256}]}` |
| `live_kept` | `{build: "install/builds/<c>", verified}` |
| `live_supports_ab` | `{install_selection: [min, max], tools_layout}` |
| `ui_recognised` | `{present, files: [name…], unexpected: [path…]}` |
| `no_unfinished` | `{operation_id: null \| "op-…", own, own_checks: {current, attempt_lock, latest_attempt, argv, unfinished}}` (`own_checks` the five identity checks, left out when `operation_id` is null) |
| `no_foreign_part` | `{foreign: [path…]}` |
| `config_readable` | `{state_root, inside_slots}` |
| `state_root_consistent` | `{configured, running, s0_head_readable, placeholder}` (`state_root_mismatch`, `state_root_placeholder`) |
| `inventory_classified` | `{unclassified: [path…]}` |
| `same_volume_plain` | `{volumes: [path…], reparse_points: [path…]}` |
| `ui_build_kept` | `{build: "install/ui-builds/<u>", verified, install_selection: [min, max]}` |

**The inventory** is one flat JSON array sorted by `path` (relative to the root, `/`), grouped by
the UI itself:

```json
[{"class": "program_live", "path": "ui/acsetup.exe", "action": "rename_aside",
  "evidence": {"bytes": 0, "sha256": "…", "manifest_commit": null, "named_by": null,
               "same_as": [{"source": "install/ui-builds/<u>/ui/acsetup.exe"}]},
  "needs_confirmation": true, "confirmation_reason": "u0_identity", "after_health": "clean"}]
```

`class` of `convert_inventory_class` (a `legacy` entry adds `legacy_kind` of
`convert_legacy_kind`), `action` of `convert_inventory_action`, `after_health` of
`convert_after_health` (whether the clean after the confirmation deletes it), `confirmation_reason`
of `convert_confirmation_reason`; `evidence.bytes` is for the person only and never in `plan_id`.
Any entry that needs a person's confirmation makes the guard `root_inventory` `not_evaluated` and
acknowledgeable: the apply carries `--ack-guard root_inventory` (`guard_failed` without it, never
`convert_layout_unrecognised`). An unrecognised form names its `needs_person`:
`inspect_live_modified`, `remove_ab_traces`, `remove_foreign_ui_files`, `check_state_root`; an old
Startup-folder launcher is the caveat `legacy_autostart` with `stop_legacy_autostart`, and the MCP
entry g1, which follows the conversion once, the caveat `mcp_entry_generation_stale`.

### Registered codes and words (Workflow #429 R-15b)

Registered in `contracts/outcome-codes` (`since` 0.12.35, review pending) so the UI's code table
(UI-0b) has every word before UI-2a and UI-2e2 start; the refusals, caveats, checks and guards are
tokens of the family's folded codes (ruling 54), and the new words of the read face are catalog
vocabularies. R-7a's value keys (`hold_phase`, `alias`, `source_*`, `ended_by_*`) are referenced,
not registered again. The gate `gate_r15b_registry` checks every item against the catalog, the
fragments and this build's enums; the forwarder's codes are in
[stable-entry.md](stable-entry.md).

- vocabulary `install_refusal` (extended): `slot_occupied`, `slot_is_active`, `slot_unrecognised`,
  `slot_not_clean`, `previous_slot_unconfirmed`, `platform_tools_changed`,
  `state_selection_mismatch`, `selection_changed_externally`, `selection_commit_lost`,
  `selection_commit_failed`, `not_same_volume`, `reparse_point`, `rollback_material_unavailable`,
  `operation_attempt_running`, `operation_id_unavailable`, `attempt_record_exists`,
  `detach_spawn_failed`, `detach_identity_mismatch`, `client_ref_unknown`,
  `ui_confirmation_stale`, `clean_irreversible`, `entry_modified`, `config_candidate_pending`,
  `convert_layout_unrecognised`, `convert_rolled_back`, `state_root_mismatch`,
  `state_root_placeholder`, `layout_single_convert_first`.
- vocabulary `install_caveat` (extended): `ui_restart_required`, `runtime_on_previous_slot`,
  `state_behind_selection`, `foreign_part`, `entry_modified`, `installer_not_newest`,
  `legacy_autostart`, `slot_left_prepared`, `operation_attempt_unconfirmed`,
  `operation_attempt_lost`, `operation_attempt_mismatch`, `kept_hold_mismatch`, `cleanup_pending`,
  `cleanup_failed`, `player_hold_persistence_off`, `cleanup_waiting_health`, `cleanup_deferred`,
  `player_hold_offline_read_failed`, `mcp_entry_generation_stale`.
- vocabulary `release_check` (extended): `selection_schema_mismatch`.
- vocabulary `install_guard` (extended): `disk`, `root_inventory`, `instructions_dropped`,
  `selection_external`, `player_holds`.
- vocabulary `install_blocked_by` (new): `foreign_pauses`, `player_holds`, `check_config`,
  `target_slot`.
- vocabulary `install_needs_person` (new): `close_slot_occupants`, `inspect_slot`,
  `remove_unlisted_files`, `remove_reparse_point`, `inspect_modified_file`,
  `restage_previous_build`, `selection_commit_lost`, `inspect_selection_external`,
  `player_hold_unverified`, `confirm_ui_running`, `inspect_runtime_image`,
  `runtime_not_answering`, `fix_configuration`, `player_hold_blocks_target`,
  `global_pause_blocks`, `stop_legacy_autostart`, `inspect_live_modified`, `remove_ab_traces`,
  `remove_foreign_ui_files`, `check_state_root`.
- vocabulary `install_operation_state` (new): `pending`, `refused`, `running`, `interrupted`,
  `ended`.
- vocabulary `install_outcome_kind` (new): `done`, `done_with_caveats`, `refused`,
  `refused_after_change`, `rolled_back`, `undone`, `abandoned`.
- vocabulary `install_commit_state` (new): `none`, `uncommitted`, `committed`, `reverted`,
  `commit_lost`, `external`.
- vocabulary `install_runtime_state` (new): `stopped`, `on_active_slot`, `on_previous_slot`,
  `on_unknown_image`, `unknown`, `not_answering`.
- vocabulary `install_attempt_phase` (new): `running`, `handover`.
- vocabulary `install_attempt_verb` (new): `apply`, `resume`, `undo`.
- vocabulary `install_action_reason` (new): `operation_finished`, `not_last_operation`,
  `rollback_gate_failed`, `operation_attempt_running`, `rollback_material_unavailable`,
  `clean_irreversible`.
- vocabulary `install_slot_state` (new): `active`, `empty`, `previous`, `cleanup_pending`,
  `prepared_left`, `cleanup_failed`, `unrecognised`.
- vocabulary `install_cleanup_state` (new): `not_needed`, `waiting_health`, `waiting_occupants`,
  `ready`, `running`, `done`, `failed`.
- vocabulary `install_cleanup_blocker` (new): `runtime_unconfirmed`, `ui_unconfirmed`.
- vocabulary `install_cleanup_failure` (new): `delete_failed`, `unlisted`, `reparse_point`,
  `modified_listed_file`.
- vocabulary `install_occupant_kind` (new): `runtime`, `ui`, `mcp_serve`, `tool`, `other`.
- vocabulary `install_kept_hold_why` (new): `hold_after_start`, `step_failed`,
  `player_hold_unverified`, `operation_unfinished`.
- vocabulary `install_health_state` (new): `confirmed`, `unconfirmed`, `not_required`.
- vocabulary `install_health_half` (new): `runtime`, `ui`.
- vocabulary `install_health_via` (new): `ui`, `cli`.
- vocabulary `install_rollback_slot_material` (new): `intact`, `cleaned`, `not_intact`.
- vocabulary `install_rollback_build_material` (new): `kept`, `cleaned`.
- vocabulary `install_operation_kind` (new): `release_stage`, `adopt`, `update`, `rollback`,
  `stop`, `start`, `release_hold`, `prepare`, `convert`, `clean`, `restart`.
- vocabulary `install_step_kind` (new): `extract`, `copy_build`, `verify_tree`, `place_build`,
  `materialize_part`, `verify_part`, `pause`, `close`, `ledger_verify`, `place_slot`,
  `stage_file`, `swap_file`, `retire_live_files`, `place_entries`, `refresh_entries`,
  `refresh_root_fixed`, `init_slot_locks`, `write_selection`, `commit_selection`, `record_state`,
  `start`, `post_check`, `record_health`, `release_hold`, `reverse_files`, `revert_selection`,
  `clean_check`, `clean_slot`, `clean_builds`, `clean_converted`, `delete_cleaning`,
  `record_cleanup`.
- vocabulary `install_member` (new): `runtime`, `tools`, `ui`.
- vocabulary `install_row_reason` (new): `target_restores_player_holds`, `no_player_hold_support`,
  `ui_drives_no_installer`.
- vocabulary `install_default_hold_reason` (new): `install_restart`.
- vocabulary `install_player_hold_seen_by` (new): `offline_after_close`.
- vocabulary `convert_check` (new): `single_location`, `no_ab_traces`, `state_v1`,
  `live_verified`, `live_kept`, `live_supports_ab`, `ui_recognised`, `no_unfinished`,
  `no_foreign_part`, `config_readable`, `state_root_consistent`, `inventory_classified`,
  `same_volume_plain`, `ui_build_kept`.
- vocabulary `convert_inventory_class` (new): `program_live`, `root_fixed`, `installer_own`,
  `config_named`, `legacy`, `foreign`.
- vocabulary `convert_legacy_kind` (new): `legacy_program_copy`, `legacy_program_file`,
  `legacy_state_root`, `legacy_config_backup`, `legacy_download`, `legacy_record`.
- vocabulary `convert_after_health` (new): `clean`, `keep`.
- vocabulary `convert_inventory_action` (new): `rename_aside`, `place_entry`,
  `rename_aside_and_place_entry`, `compare_keep`, `refresh`, `create`, `rewrite_state`, `keep`.
- vocabulary `convert_confirmation_reason` (new): `u0_identity`, `config_named_dir`,
  `legacy_item`, `foreign_item`, `state_root_three_way`.
- keys: `attempt_phase`, `hold_reason`, `s0_from`, `player_hold_range`, `confirm_argv`,
  `active_sha256`, `install_member`.
- locations of `actingctl`: `install_prepare`, `install_convert_plan`, `install_convert_apply`,
  `install_clean`, `install_restart`.
- codes of `contract`: `install_selection_v2_invalid`, `install_slot_document_mismatch`,
  `install_member_identity_invalid`, `install_member_pairing_mismatch`,
  `install_slot_document_invalid`.
- codes of `actingd`: `unlock_owner_install_unfinished`.

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
  `install_new` (INST-6); `install_prepare`, `install_convert_plan`, `install_convert_apply`,
  `install_clean`, `install_restart` (Workflow #429 R-15b, registered for R-15c to R-15e).
- **Workflow #429 R-15b** (since 0.12.35) adds the tokens, vocabularies, keys and codes of
  "Registered codes and words" above; `install_command_refused` takes the value `attempt_phase`
  (`operation_attempt_running`).
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
